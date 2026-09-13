//! The `refs` tab: every branch and tag, unabridged (the summary tab only
//! shows the first few — see [`super::components`]).

use super::compare::render_compare_form;
use super::components::split_refs;
use crate::auth::web::login;
use crate::webui::{AppError, csrf_field, icons, repo_link, url_encode};
use anyhow::Result;
use axum::http::HeaderMap;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::git;
use gitquarry_core::validate;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::components as ct;
use gitquarry_i18n::web::repo::refs as t;
use maud::{Markup, PreEscaped, html};
use std::path::Path as FsPath;

/// `true` if the current session has at least `Write` on `repo_name` — same
/// gate and same "the mutation route re-checks it itself" reasoning as
/// `webui::repo::releases::can_write_releases`, duplicated rather than shared
/// since the two live in sibling modules with no natural common parent.
/// Gates the create-tag form and both tables' per-row delete buttons.
async fn can_write_refs(ctx: &AppContext, headers: &HeaderMap, repo_name: &str) -> Result<bool, AppError> {
    let Some(user) = login::current_user(ctx, headers).await else {
        return Ok(false);
    };
    let perm = gitquarry_core::auth::access::effective_permission_for_user(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &user,
        repo_name,
    )
    .await?;
    Ok(perm >= Some(Permission::Developer))
}

/// Same columns as [`ref_table`] plus a trailing delete button when
/// `can_write` — a protected tag still shows the button (same "the mutation
/// route re-checks it itself" reasoning as everywhere else) rather than
/// trying to mirror `RepoConfig.protected_tags` into this render, so the
/// error message on the rejected POST is the one place that has to know
/// about protection at all.
pub(super) fn tag_table(
    repo_name: &str,
    tags: &[git::browse::RefInfo],
    can_write: bool,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    html! {
        div id="tag-table" {
        div class="table-scroll" {
        table class="list" {
            tr {
                th { (ct::name_column(locale)) } th { (ct::commit_column(locale)) }
                th { (ct::message_column(locale)) } th { (ct::date_column(locale)) }
                @if can_write { th {} }
            }
            @if tags.is_empty() {
                tr { td colspan=(if can_write { "5" } else { "4" }) { em { (ct::none_hint(locale)) } } }
            }
            @for r in tags {
                @let short = r.name.strip_prefix("refs/tags/").unwrap_or(&r.name);
                tr {
                    td { a href=(repo_link(repo_name, "tree", short).to_string()) { (short) } }
                    td {
                        a href=(repo_link(repo_name, "commit", "").sha(&r.sha).to_string()) {
                            code { (validate::short_sha(&r.sha)) }
                        }
                    }
                    td { (r.subject) }
                    td class="muted" { (r.date) }
                    @if can_write {
                        td {
                            form method="post" action=(format!("/_tags/delete/{}", url_encode(repo_name))) class="inline-form" data-async data-async-target="#tag-table" data-confirm=(t::delete_tag_confirm(locale, short)) {
                                (PreEscaped(csrf_field(headers)))
                                input type="hidden" name="tag" value=(short);
                                button type="submit" { (t::delete_tag_button(locale)) }
                            }
                        }
                    }
                }
            }
        }
        }
        }
    }
}

/// Same shape as [`tag_table`] — a protected or default branch still shows
/// the button (same "the mutation route re-checks it itself" reasoning),
/// no attempt to mirror `RepoConfig.protected_branches`/`Manifest::head`
/// into this render.
pub(super) fn branch_table(
    repo_name: &str,
    branches: &[git::browse::RefInfo],
    can_write: bool,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    html! {
        div id="branch-table" {
        div class="table-scroll" {
        table class="list" {
            tr {
                th { (ct::name_column(locale)) } th { (ct::commit_column(locale)) }
                th { (ct::message_column(locale)) } th { (ct::date_column(locale)) }
                @if can_write { th {} }
            }
            @if branches.is_empty() {
                tr { td colspan=(if can_write { "5" } else { "4" }) { em { (ct::none_hint(locale)) } } }
            }
            @for r in branches {
                @let short = r.name.strip_prefix("refs/heads/").unwrap_or(&r.name);
                tr {
                    td { a href=(repo_link(repo_name, "tree", short).to_string()) { (short) } }
                    td {
                        a href=(repo_link(repo_name, "commit", "").sha(&r.sha).to_string()) {
                            code { (validate::short_sha(&r.sha)) }
                        }
                    }
                    td { (r.subject) }
                    td class="muted" { (r.date) }
                    @if can_write {
                        td {
                            form method="post" action=(format!("/_branches/delete/{}", url_encode(repo_name))) class="inline-form" data-async data-async-target="#branch-table" data-confirm=(t::delete_branch_confirm(locale, short)) {
                                (PreEscaped(csrf_field(headers)))
                                input type="hidden" name="branch" value=(short);
                                button type="submit" { (t::delete_branch_button(locale)) }
                            }
                        }
                    }
                }
            }
        }
        }
        }
    }
}

/// `tag_prefill`/`error`: a rejected create-tag submit (bad rev, no such
/// source, tag already exists, ...) re-renders this same page with the
/// typed tag name/source still filled in and the reason shown above the
/// form, instead of losing it to the generic error page — `None`/`None`
/// for the plain GET landing.
pub async fn render_refs(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    headers: &HeaderMap,
    tag_prefill: Option<(&str, &str)>,
    error: Option<&str>,
    locale: Locale,
) -> Result<String, AppError> {
    let refs = git::browse::list_refs_detailed(bare_repo, "refs/").await?;
    let (branches, tags) = split_refs(refs);

    fn short_name(r: &git::browse::RefInfo) -> &str {
        r.name
            .strip_prefix("refs/heads/")
            .or_else(|| r.name.strip_prefix("refs/tags/"))
            .unwrap_or(&r.name)
    }
    let ref_names: Vec<&str> = branches.iter().chain(&tags).map(short_name).collect();
    // Branches only: a new tag almost always marks a branch tip, not
    // wherever another tag already points — unlike `render_compare_form`
    // just below (comparing two tags is a real use case), this dropdown
    // has no reason to offer existing tags as a source.
    let branch_names: Vec<&str> = branches.iter().map(short_name).collect();
    let can_write = can_write_refs(ctx, headers, repo_name).await?;

    Ok(html! {
        h3 { (icons::branch()) " " (t::branches_heading(locale)) }
        (branch_table(repo_name, &branches, can_write, headers, locale))
        h3 { (icons::tag()) " " (t::tags_heading(locale)) }
        (tag_table(repo_name, &tags, can_write, headers, locale))
        @if can_write {
            @let (tag_value, source_value) = tag_prefill.unwrap_or(("", ""));
            h4 { (t::create_tag_heading(locale)) }
            @if let Some(msg) = error {
                p class="alert alert-err" { (msg) }
            }
            form method="post" action=(format!("/_tags/create/{}", url_encode(repo_name))) {
                (PreEscaped(csrf_field(headers)))
                div class="field" {
                    label { (t::tag_name_label(locale)) }
                    input name="tag" size="30" value=(tag_value) placeholder=(t::tag_name_placeholder(locale));
                }
                div class="field" {
                    label { (t::tag_source_label(locale)) }
                    select name="source" {
                        @for name in &branch_names {
                            option value=(name) selected[*name == source_value] { (name) }
                        }
                    }
                }
                p { button type="submit" { (t::create_tag_button(locale)) } }
            }
        }
        (render_compare_form(repo_name, &ref_names, locale))
    }
    .into_string())
}
