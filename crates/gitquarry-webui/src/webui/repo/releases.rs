//! The `releases` tab: named releases wrapping an existing tag, with
//! markdown notes and downloadable asset files, plus the
//! `/_releases/{create,edit,delete,generate_notes,asset_upload,asset_delete}/{repo}`
//! mutation handlers those views' forms POST to.
//! Top-level routes, not nested under the repo's own `/{*repo}` path — same
//! shape as `/_fork` ([`super::super::fork`]'s doc comment explains why:
//! that path is a full-path catch-all already bound to one POST handler, so
//! a second mutation action set needs its own literal route, taking the
//! repo's full name as a hidden form field instead of a path segment).

use crate::auth::web::login;
use crate::webui::page_chrome::{HeaderExtras, header_bar};
use crate::webui::{
    AppError, State_, csrf_field, effective_density, effective_locale, effective_theme, layout, markdown, non_empty,
    raw_link, repo_link, require_csrf, require_login, url_encode,
};
use axum::extract::{Form, Multipart, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::git;
use gitquarry_core::storage::Cas;
use gitquarry_core::validate;
use gitquarry_domain::release;
use gitquarry_i18n::web::repo::releases as t;
use maud::{Markup, PreEscaped, html};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tracing::warn;

/// `true` if the current session has at least `Write` on `repo_name` — what
/// gates showing the create/edit/delete/upload forms at all. The actual
/// mutation handlers below re-check this themselves rather than trusting
/// that a form was only ever shown to someone entitled to submit it.
async fn can_write_releases(ctx: &AppContext, headers: &HeaderMap, repo_name: &str) -> Result<bool, AppError> {
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

/// What survives a "generate notes" round trip back into the create form —
/// same reason [`crate::webui::page_chrome::HeaderExtras`] bundles its own
/// params: threading `tag`/`title`/`notes`/both checkboxes as separate
/// trailing args would trip clippy's `too_many_arguments`.
pub struct ReleaseFormPrefill {
    pub tag: String,
    pub title: String,
    pub notes: String,
    pub is_prerelease: bool,
    pub is_draft: bool,
}

/// Tag names the create-release form suggests: every git tag in the repo
/// that doesn't already have a release, sorted.
///
/// Both halves matter. A release can only ever name a tag that already
/// exists — `domain::release::create` refuses anything else — so an
/// unassisted text field asks the operator to remember exact tag spelling.
/// And a tag that already has a release is not a candidate: offering it
/// would only lead to the "this tag already has a release" rejection.
pub fn taggable_tags<'a>(refs: impl Iterator<Item = &'a String>, released: &[String]) -> Vec<String> {
    let mut names: Vec<String> = refs
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .filter(|name| !released.iter().any(|t| t == name))
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

pub async fn render_releases_list(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    repo_name: &str,
    headers: &HeaderMap,
) -> Result<String, AppError> {
    render_releases_list_with_prefill(ctx, resolved, repo_name, headers, None, None).await
}

pub async fn render_releases_list_with_prefill(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    repo_name: &str,
    headers: &HeaderMap,
    prefill: Option<&ReleaseFormPrefill>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let can_write = can_write_releases(ctx, headers, repo_name).await?;
    let tags = resolved.remote.list_release_tags().await?;
    let mut releases = Vec::with_capacity(tags.len());
    for tag in &tags {
        let (release, etag) = resolved.remote.get_release(tag).await?;
        // A draft is hidden from anyone who couldn't have published it —
        // same "exists, but only to whoever could publish it" gate
        // `render_release_detail` enforces on the detail page itself.
        if etag.is_some() && (can_write || !release.is_draft) {
            releases.push(release);
        }
    }
    releases.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let locale = effective_locale(ctx, headers).await;
    // Suggestions for the tag field, the same affordance the new-merge form
    // gives its branch fields. A release can only ever name a tag that
    // already exists (`domain::release::create` refuses otherwise), so
    // without this the one required field is a guess -- and the tags that
    // already have a release are dropped, since offering those would only
    // suggest the "this tag already has a release" error.
    let taggable = if can_write {
        let (manifest, _etag) = resolved.remote.get_manifest().await?;
        taggable_tags(manifest.refs.keys(), &tags)
    } else {
        // The form these suggestions belong to isn't rendered at all for a
        // reader, so this is a manifest fetch that would buy nothing.
        Vec::new()
    };

    Ok(html! {
        div class="table-scroll" {
        table class="list" {
            tr { th { (t::release_column(locale)) } th { (t::assets_column(locale)) } th { (t::created_column(locale)) } }
            @if releases.is_empty() {
                tr { td colspan="3" {
                    div class="empty-state" {
                        span class="empty-state-icon" { (crate::webui::icons::tag()) }
                        div class="empty-state-title" { (t::no_releases_yet_hint(locale)) }
                    }
                } }
            }
            @for release in &releases {
                tr {
                    td {
                        a href=(repo_link(repo_name, "release", "").tag(&release.tag).to_string()) {
                            (release.title)
                        }
                        " " span class="muted" { "(" (release.tag) ")" }
                        @if release.is_prerelease {
                            " " span class="badge warn" { (t::prerelease_badge(locale)) }
                        }
                        @if release.is_draft {
                            " " span class="badge warn" { (t::draft_badge(locale)) }
                        }
                    }
                    td { (release.assets.len()) }
                    td { (release.created_at) }
                }
            }
        }
        }
        @if can_write {
            @let default_prefill = ReleaseFormPrefill {
                tag: String::new(),
                title: String::new(),
                notes: String::new(),
                is_prerelease: false,
                is_draft: false,
            };
            @let prefill = prefill.unwrap_or(&default_prefill);
            h4 { (t::create_release_heading(locale)) }
            @if let Some(msg) = error {
                p class="alert alert-err" { (msg) }
            }
            form method="post" action=(format!("/_releases/create/{}", url_encode(repo_name))) {
                (PreEscaped(csrf_field(headers)))
                div class="field" {
                    label { (t::tag_label(locale)) }
                    input list="release-tags" name="tag" size="30" value=(prefill.tag) placeholder=(t::tag_placeholder(locale));
                }
                datalist id="release-tags" {
                    @for name in &taggable {
                        option value=(name) { (name) }
                    }
                }
                div class="field" {
                    label { (t::title_label(locale)) }
                    input name="title" size="40" value=(prefill.title);
                }
                div class="field" {
                    label { (t::notes_label(locale)) }
                    textarea name="notes" rows="4" cols="60" { (prefill.notes) }
                    br;
                    button type="submit" formaction=(format!("/_releases/generate_notes/{}", url_encode(repo_name))) { (t::generate_notes_button(locale)) }
                }
                div class="field" {
                    label { input type="checkbox" name="is_prerelease" value="1" checked[prefill.is_prerelease]; " " (t::prerelease_checkbox_label(locale)) }
                }
                div class="field" {
                    label { input type="checkbox" name="is_draft" value="1" checked[prefill.is_draft]; " " (t::draft_checkbox_label(locale)) }
                }
                p { button type="submit" { (t::create_release_button(locale)) } }
            }
        }
    }
    .into_string())
}

/// The asset table alone, wrapped in a stable `id` so `/_releases/asset_delete`
/// can swap just this fragment (`data-async-target="#asset-table"`, see
/// `crate::webui::ASYNC_HEADER`'s own doc comment) instead of re-rendering
/// the whole release detail page.
#[allow(clippy::too_many_arguments)]
fn render_asset_table(
    repo_name: &str,
    tag: &str,
    assets: &[gitquarry_core::storage::remote::AssetMeta],
    source_tar_gz: &str,
    source_zip: &str,
    can_write: bool,
    headers: &HeaderMap,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    html! {
        div id="asset-table" {
        div class="table-scroll" {
        table class="list" {
            tr { th { (t::file_column(locale)) } th { (t::size_column(locale)) } th { (t::sha256_column(locale)) } th {} }
            @if assets.is_empty() {
                tr { td colspan="4" { em { (t::no_assets_hint(locale)) } } }
            }
            @for asset in assets {
                tr {
                    td {
                        a href=(raw_link("release_asset", repo_name, "").tag(tag).filename(&asset.filename).to_string()) {
                            @if asset.filename == source_tar_gz { (t::source_code_tar_gz(locale)) }
                            @else if asset.filename == source_zip { (t::source_code_zip(locale)) }
                            @else { (asset.filename) }
                        }
                        @if asset.signature.is_some() {
                            @if asset.signature_verified {
                                " " span class="badge ok" { (t::signature_verified_badge(locale)) }
                            } @else {
                                " " span class="badge" { (t::signature_unverified_badge(locale)) }
                            }
                        }
                    }
                    td { (asset.size) }
                    td { code { (gitquarry_core::validate::short_sha(&asset.sha256)) } }
                    td {
                        @if can_write {
                            form method="post" action=(format!("/_releases/asset_delete/{}", url_encode(repo_name))) class="inline-form" data-async data-async-target="#asset-table" data-confirm=(t::delete_asset_confirm(locale, &asset.filename)) {
                                (PreEscaped(csrf_field(headers)))
                                input type="hidden" name="tag" value=(tag);
                                input type="hidden" name="filename" value=(&asset.filename);
                                button type="submit" { (t::delete_button(locale)) }
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

pub async fn render_release_detail(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    repo_name: &str,
    tag: &str,
    headers: &HeaderMap,
    locale: gitquarry_i18n::Locale,
) -> Result<String, AppError> {
    let (release, etag) = resolved.remote.get_release(tag).await?;
    let can_write = can_write_releases(ctx, headers, repo_name).await?;
    // A draft 404s for anyone who couldn't have published it, same as if
    // it never existed at all — it doesn't even leak that a release is
    // pending for this tag, same "not found, not forbidden" tolerance
    // `webui::repo::mod`'s own private-repo gate uses.
    if etag.is_none() || (release.is_draft && !can_write) {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::release::release_not_found(locale, tag)
        )));
    }

    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    let tag_still_exists = manifest.refs.contains_key(&format!("refs/tags/{tag}"));
    // The two source-archive assets `release::generate_source_archives`
    // generates and uploads once, right when the release is created — same
    // stored bytes served on every download, never recomputed on the fly.
    // Matched by filename so they render with a GitHub-style label instead
    // of their (still real, still downloadable-by-that-name) raw filename.
    let source_tar_gz = git::browse::archive_filename(repo_name, tag, "tar.gz");
    let source_zip = git::browse::archive_filename(repo_name, tag, "zip");

    Ok(html! {
        h3 {
            (release.title) " " span class="muted" { "(" (release.tag) ")" }
            @if release.is_prerelease {
                " " span class="badge warn" { (t::prerelease_badge(locale)) }
            }
            @if release.is_draft {
                " " span class="badge warn" { (t::draft_badge(locale)) }
            }
        }
        @if !tag_still_exists {
            p class="muted" { (t::dangling_tag_notice_prefix(locale)) code { (tag) } (t::dangling_tag_notice_suffix(locale)) }
        }
        p class="muted" { (t::created_by_prefix(locale)) (release.created_at) (t::created_by_middle(locale)) (release.created_by) }
        (PreEscaped(markdown::render(&release.notes)))

        h4 { (t::assets_heading(locale)) }
        (render_asset_table(repo_name, tag, &release.assets, &source_tar_gz, &source_zip, can_write, headers, locale))

        @if can_write {
            h4 { (t::upload_asset_heading(locale)) }
            form method="post" action=(format!("/_releases/asset_upload/{}", url_encode(repo_name))) enctype="multipart/form-data" {
                (PreEscaped(csrf_field(headers)))
                input type="hidden" name="tag" value=(tag);
                input type="file" name="file";
                br;
                textarea name="signature" rows="4" cols="60" placeholder=(t::signature_placeholder(locale)) {}
                p { button type="submit" { (t::upload_button(locale)) } }
            }

            h4 { (t::edit_heading(locale)) }
            form method="post" action=(format!("/_releases/edit/{}", url_encode(repo_name))) {
                (PreEscaped(csrf_field(headers)))
                input type="hidden" name="tag" value=(tag);
                div class="field" {
                    label { (t::title_label(locale)) }
                    input name="title" size="40" value=(release.title);
                }
                div class="field" {
                    label { (t::notes_label(locale)) }
                    textarea name="notes" rows="4" cols="60" { (release.notes) }
                }
                div class="field" {
                    label { input type="checkbox" name="is_prerelease" value="1" checked[release.is_prerelease]; " " (t::prerelease_checkbox_label(locale)) }
                }
                div class="field" {
                    label { input type="checkbox" name="is_draft" value="1" checked[release.is_draft]; " " (t::draft_checkbox_label(locale)) }
                }
                p { button type="submit" { (t::save_button(locale)) } }
            }

            h4 { (t::delete_release_heading(locale)) }
            form method="post" action=(format!("/_releases/delete/{}", url_encode(repo_name))) data-confirm=(t::delete_release_confirm(locale, tag)) {
                (PreEscaped(csrf_field(headers)))
                input type="hidden" name="tag" value=(tag);
                button type="submit" { (t::delete_release_button(locale)) }
            }
        }
    }
    .into_string())
}

/// Resolves `repo` and checks `me` has at least `Write` on it. Everything
/// past login (which stays inline in each handler — it returns a bare
/// `Response`, not an [`AppError`], see `webui::fork`'s identical shape)
/// funnels through here, since every release mutation needs the same
/// resolve-then-authorize step.
async fn require_write_target(
    ctx: &AppContext,
    me: &User,
    repo: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(ResolvedRepo, String), AppError> {
    let name = validate::sanitize_path(repo).map_err(AppError::bad_request)?;
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &name)
        )));
    };
    let perm =
        gitquarry_core::auth::access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, me, &name)
            .await?;
    if perm < Some(Permission::Developer) {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::write_permission_required(locale)
        )));
    }
    Ok((resolved, name))
}

/// `GET` on a `/_releases/*` mutation target: what a reload or a back
/// button asks for after a rejected submit left that URL in the address
/// bar. The repo is in the path, so it lands on that repo's releases.
pub(crate) async fn handler_mutation_get(Path(repo): Path<String>) -> Response {
    axum::response::Redirect::to(&format!("/{}?view=releases", url_encode(&repo))).into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct ReleaseForm {
    tag: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    notes: String,
    /// `"1"` when checked, absent otherwise — same convention
    /// `repo_settings::general`'s own `archived` checkbox field uses.
    #[serde(default)]
    is_prerelease: String,
    #[serde(default)]
    is_draft: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_create_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ReleaseForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let tag = form.tag.trim();

    // A rejected create (bad tag, tag already has a release, ...)
    // re-renders this same form with the typed title/notes/checkboxes
    // still filled in, instead of losing it to the generic error page —
    // same full-page-with-prefill shape `handler_generate_notes_post`
    // already uses for its own round trip.
    macro_rules! fail {
        ($msg:expr) => {{
            let prefill = ReleaseFormPrefill {
                tag: form.tag.clone(),
                title: form.title.clone(),
                notes: form.notes.clone(),
                is_prerelease: form.is_prerelease == "1",
                is_draft: form.is_draft == "1",
            };
            let body = render_releases_list_with_prefill(&ctx, &resolved, &name, &headers, Some(&prefill), Some(&$msg))
                .await?;
            let (config, _etag) = resolved.remote.get_repo_config().await?;
            let topnav = login::topnav(&ctx, &headers).await;
            let density = effective_density(&ctx, &headers).await;
            let header = header_bar(
                &name,
                "",
                "releases",
                HeaderExtras {
                    description: config.description.as_deref(),
                    display_name: config.display_name.as_deref(),
                    archived: config.archived,
                    visibility: config.visibility,
                    branches: &[],
                    menu_extra: "",
                    below_header: "",
                    sub_tabs: "",
                    locale,
                },
            );
            return Ok(axum::response::Html(layout(
                &name,
                &header,
                &body,
                &topnav,
                effective_theme(&ctx, &headers).await,
                density,
                locale,
            ))
            .into_response());
        }};
    }

    if let Err(e) = validate::validate_rev(tag) {
        fail!(e.to_string());
    }
    let cas = match release::create(
        &ctx,
        &resolved,
        tag,
        form.title.trim(),
        &form.notes,
        form.is_prerelease == "1",
        form.is_draft == "1",
        &me.username,
        locale,
    )
    .await
    {
        Ok(cas) => cas,
        Err(e) => fail!(e.to_string()),
    };
    if cas == Cas::Conflict {
        fail!(gitquarry_i18n::shared::release::release_already_exists_for_tag(
            locale, tag
        ));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "release.create",
        &format!("{name}@{tag}"),
        None,
    )
    .await;

    // `release::generate_source_archives` (opening/syncing the
    // mirror, the archive decode, an upload each) is the slow part of
    // creating a release and has nothing the requester needs to wait on —
    // spawned instead of awaited so the redirect below fires right away
    // regardless of repo size. The two assets simply aren't there yet if the
    // release page loads before this finishes; a reload once it's done shows
    // them, same as any other eventually-consistent background upload.
    let ctx_bg = Arc::clone(&ctx);
    let resolved_bg = ResolvedRepo {
        uid: resolved.uid.clone(),
        remote: resolved.remote.clone(),
    };
    let repo_name_bg = name.clone();
    let tag_bg = tag.to_string();
    let uploaded_by_bg = me.username.clone();
    tokio::spawn(async move {
        if let Err(e) =
            release::generate_source_archives(&ctx_bg, &resolved_bg, &repo_name_bg, &tag_bg, &uploaded_by_bg, locale)
                .await
        {
            warn!(repo = %repo_name_bg, tag = %tag_bg, error = %e, "release source archives failed");
        }
    });

    Ok(Redirect::to(&repo_link(&name, "release", "").tag(tag).to_string()).into_response())
}

/// `POST /_releases/generate_notes`: re-renders the very form it was
/// submitted from with `notes` filled in from
/// [`release::generate_notes`] — never a redirect, since the
/// generated text has nowhere to survive one (no flash-cookie mechanism
/// exists in this codebase for carrying a value, as opposed to a plain
/// message, across a redirect). Same `formaction`-on-a-second-button
/// trick lets this share the create form's own fields (`tag`/`title`/both
/// checkboxes) with zero extra markup — see [`render_releases_list_with_prefill`]'s
/// call site in the create form.
pub(crate) async fn handler_generate_notes_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ReleaseForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let tag = form.tag.trim().to_string();
    let (notes, error) = match validate::validate_rev(&tag) {
        Err(e) => (form.notes.clone(), Some(e.to_string())),
        Ok(()) => match release::generate_notes(&ctx, &resolved, &tag, locale).await {
            Ok(notes) => (notes, None),
            Err(e) => (form.notes.clone(), Some(e.to_string())),
        },
    };
    let prefill = ReleaseFormPrefill {
        tag,
        title: form.title.clone(),
        notes,
        is_prerelease: form.is_prerelease == "1",
        is_draft: form.is_draft == "1",
    };

    let body =
        render_releases_list_with_prefill(&ctx, &resolved, &name, &headers, Some(&prefill), error.as_deref()).await?;
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    let topnav = login::topnav(&ctx, &headers).await;
    let density = effective_density(&ctx, &headers).await;
    let header = header_bar(
        &name,
        "",
        "releases",
        HeaderExtras {
            description: config.description.as_deref(),
            display_name: config.display_name.as_deref(),
            archived: config.archived,
            visibility: config.visibility,
            branches: &[],
            menu_extra: "",
            below_header: "",
            sub_tabs: "",
            locale,
        },
    );
    Ok(axum::response::Html(layout(
        &name,
        &header,
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        density,
        locale,
    ))
    .into_response())
}

pub(crate) async fn handler_edit_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ReleaseForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let tag = form.tag.trim();
    validate::validate_rev(tag).map_err(AppError::bad_request)?;
    let cas = release::edit(
        &resolved,
        tag,
        form.title.trim(),
        &form.notes,
        form.is_prerelease == "1",
        form.is_draft == "1",
        locale,
    )
    .await
    .map_err(AppError::bad_request)?;
    if cas == Cas::Conflict {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "release.edit",
        &format!("{name}@{tag}"),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "release", "").tag(tag).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct ReleaseDeleteForm {
    tag: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_delete_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ReleaseDeleteForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let tag = form.tag.trim();
    validate::validate_rev(tag).map_err(AppError::bad_request)?;
    release::delete(&resolved, tag).await?;

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "release.delete",
        &format!("{name}@{tag}"),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "releases", "").to_string()).into_response())
}

/// Disk-fill guard on the scratch tempfile the multipart body streams
/// into before it's ever uploaded to S3 — not a policy knob (no
/// `ServerSettings` field for it), the same "hardcoded constant next to the
/// handler" precedent `export::PLAIN_MAX_BYTES` already sets.
const MAX_ASSET_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The route's whole-body cap, layered on in `webui::router`: axum's
/// default 2 MiB `DefaultBodyLimit` otherwise rejects any asset past that
/// as an unparseable multipart body. The extra MiB is room for the text
/// fields and multipart framing, so an asset right at
/// [`MAX_ASSET_UPLOAD_BYTES`] still reaches the handler's own, clearer cap.
pub(crate) const MAX_ASSET_UPLOAD_BODY_BYTES: usize = MAX_ASSET_UPLOAD_BYTES as usize + 1024 * 1024;

/// Cap on each text field (`tag`/`csrf`/`signature`): `Field::text`
/// buffers the whole field in memory, and with the body limit raised for
/// the file a single text field could otherwise hold gigabytes. An armored
/// detached signature is a few KiB.
const MAX_TEXT_FIELD_BYTES: usize = 64 * 1024;

async fn read_text_field(
    mut field: axum::extract::multipart::Field<'_>,
    locale: gitquarry_i18n::Locale,
) -> Result<String, AppError> {
    let name = field.name().unwrap_or_default().to_string();
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(|e| {
        AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::release::invalid_multipart(locale, e)
        ))
    })? {
        if bytes.len() + chunk.len() > MAX_TEXT_FIELD_BYTES {
            return Err(AppError::bad_request(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::release::text_field_too_large(locale, &name, MAX_TEXT_FIELD_BYTES)
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|e| AppError::bad_request(anyhow::anyhow!("{e}")))
}

/// Lowercase hex — same tiny local helper `git::lfs` has, not worth a shared
/// utility module for four lines.
fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `POST /_releases/asset_upload/{repo}`: multipart upload. Every text field (
/// `tag`/`csrf`) is read first, the `file` field's bytes are streamed into a
/// scratch tempfile (hashed and size-counted along the way, capped at
/// [`MAX_ASSET_UPLOAD_BYTES`]) — never fully buffered in memory. Login/CSRF/
/// A session is required *before* the body is touched: an anonymous client
/// could otherwise make the server write [`MAX_ASSET_UPLOAD_BYTES`] to disk
/// per request just by opening one, and the cap bounds a single upload, not
/// how many an attacker may start. CSRF and the write-permission check stay
/// after the drain — `csrf` is a body field, and multipart fields can only
/// be read in the order the client sent them.
pub(crate) async fn handler_upload_asset_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    // Headers alone answer "is anyone logged in", so this doesn't have to
    // wait for the body — and it must not: everything below writes the
    // uploaded bytes to disk.
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    let mut tag = String::new();
    let mut csrf = String::new();
    let mut signature = String::new();
    let mut uploaded: Option<(String, tempfile::TempPath, u64, String)> = None;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::release::invalid_multipart(locale, e)
        ))
    })? {
        match field.name() {
            Some("tag") => tag = read_text_field(field, locale).await?,
            Some("csrf") => csrf = read_text_field(field, locale).await?,
            Some("signature") => signature = read_text_field(field, locale).await?,
            Some("file") => {
                let filename = validate::validate_asset_filename(field.file_name().unwrap_or(""))
                    .map_err(AppError::bad_request)?;
                let tmp = tempfile::Builder::new()
                    .prefix(".gitquarry-asset-")
                    .tempfile()
                    .map_err(|e| {
                        AppError::bad_request(anyhow::anyhow!(
                            "{}",
                            gitquarry_i18n::shared::release::creating_temp_file(locale, e)
                        ))
                    })?
                    .into_temp_path();
                let mut file = tokio::fs::File::create(&tmp).await?;
                let mut hasher = Sha256::new();
                let mut size: u64 = 0;
                while let Some(chunk) = field.chunk().await.map_err(|e| {
                    AppError::bad_request(anyhow::anyhow!(
                        "{}",
                        gitquarry_i18n::shared::release::reading_file(locale, e)
                    ))
                })? {
                    size += chunk.len() as u64;
                    if size > MAX_ASSET_UPLOAD_BYTES {
                        return Err(AppError::bad_request(anyhow::anyhow!(
                            "{}",
                            gitquarry_i18n::shared::release::asset_too_large(locale, MAX_ASSET_UPLOAD_BYTES)
                        )));
                    }
                    hasher.update(&chunk);
                    file.write_all(&chunk).await?;
                }
                file.flush().await?;
                drop(file);
                uploaded = Some((filename, tmp, size, encode_hex(&hasher.finalize())));
            }
            _ => {}
        }
    }

    require_csrf(&headers, &csrf)?;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let (filename, tmp_path, size, sha256) = uploaded.ok_or_else(|| {
        AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::release::missing_file_to_upload(locale)
        ))
    })?;
    let tag = tag.trim();
    validate::validate_rev(tag).map_err(AppError::bad_request)?;
    let signature = non_empty(signature);
    let cas = release::add_asset(
        &ctx,
        &resolved,
        release::NewAsset {
            tag,
            filename: &filename,
            source: &tmp_path,
            size,
            sha256: &sha256,
            uploaded_by: &me.username,
            signature: signature.as_deref(),
            locale,
        },
    )
    .await
    .map_err(AppError::bad_request)?;
    if cas == Cas::Conflict {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "release.asset.add",
        &format!("{name}@{tag}"),
        Some(filename),
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "release", "").tag(tag).to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct AssetDeleteForm {
    tag: String,
    filename: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_delete_asset_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<AssetDeleteForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let tag = form.tag.trim();
    validate::validate_rev(tag).map_err(AppError::bad_request)?;
    let filename = validate::validate_asset_filename(&form.filename).map_err(AppError::bad_request)?;
    let cas = release::remove_asset(&resolved, tag, &filename, locale)
        .await
        .map_err(AppError::bad_request)?;
    if cas == Cas::Conflict {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::cas_conflict(locale)
        )));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "release.asset.remove",
        &format!("{name}@{tag}"),
        Some(filename),
    )
    .await;
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        let (release, _etag) = resolved.remote.get_release(tag).await?;
        let source_tar_gz = git::browse::archive_filename(&name, tag, "tar.gz");
        let source_zip = git::browse::archive_filename(&name, tag, "zip");
        return Ok(axum::response::Html(
            render_asset_table(
                &name,
                tag,
                &release.assets,
                &source_tar_gz,
                &source_zip,
                true,
                &headers,
                locale,
            )
            .into_string(),
        )
        .into_response());
    }
    Ok(Redirect::to(&repo_link(&name, "release", "").tag(tag).to_string()).into_response())
}
