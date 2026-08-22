//! `/_account/pat`: self-service local personal access tokens for
//! git-smart-HTTP clone/push — separate from [`super::security`] (SSH keys/
//! password/2FA) and from federation's own fine-grained tokens at
//! `/_account/tokens` (proxied, needs `[federation]` configured). This page
//! works with core alone: [`gitquarry_core::auth::domain::pat`] never
//! touches `ctx.federation`.
//!
//! No username in the URL: always the logged-in session's own tokens,
//! resolved server-side, same as every other `/_account/*` section — see
//! `crate::webui::account_settings_page`'s own doc comment.

use super::login;
use crate::webui::{
    AppError, Flash, State_, account_settings_page, copy_button, csrf_field, effective_density, effective_locale,
    effective_theme, field, layout, render_flash, require_csrf, require_login, settings_section,
};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{Permission, PersonalAccessToken, User};
use gitquarry_core::storage::settings;
use gitquarry_core::storage::uid::Uid;
use maud::{Markup, PreEscaped, html};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(crate) async fn handler_pat_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let user = match require_login(&ctx, &headers, "/_account/pat").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let tokens = access::list_tokens_for_owner(ctx.backend().as_ref(), &ctx.cipher, &user.uid).await?;
    let repo_names = repo_uid_to_name_map(&ctx, &tokens).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_pat_page(&tokens, &repo_names, &csrf_field(&headers), None, None, locale);
    let body = account_settings_page("pat", content, locale).into_string();
    Ok(axum::response::Html(layout(
        "pat",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Every repo uid referenced across `tokens`' `scopes` maps, resolved to
/// its current `full_name` — same purpose as `super::users`'s own
/// `repo_uid_to_name_map`, just scoped to one user's own tokens (expected
/// to stay small, so a plain sequential resolve is fine, no `JoinSet`
/// needed).
async fn repo_uid_to_name_map(
    ctx: &gitquarry_core::app::AppContext,
    tokens: &[PersonalAccessToken],
) -> HashMap<Uid, String> {
    let uids: BTreeSet<Uid> = tokens.iter().flat_map(|t| t.scopes.keys().cloned()).collect();
    let mut map = HashMap::new();
    for uid in uids {
        if let Ok((config, _etag)) = settings::get_repo_config(ctx.backend().as_ref(), uid.as_str(), &ctx.cipher).await
        {
            map.insert(uid, config.full_name);
        }
    }
    map
}

/// Parses the create form's `repo:permission, repo2:permission2` text —
/// same syntax `/_users`' bulk per-repo grants already use
/// (`auth::domain::user::parse_repos_field`) — into a scope map. `"*"` is
/// rejected explicitly rather than silently failing "repo not found": a
/// PAT can never scope to "every repo", same rule federation's own
/// fine-grained tokens follow.
fn parse_scopes_field(entry_list: &str) -> Result<Vec<(String, Permission)>, String> {
    let mut entries = Vec::new();
    for entry in entry_list.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (repo, perm) = entry.split_once(':').ok_or_else(|| format!("invalid entry: {entry}"))?;
        let repo = repo.trim();
        if repo == "*" {
            return Err("a personal access token can't scope to all repos — list them explicitly".to_string());
        }
        let perm = match perm.trim() {
            "guest" => Permission::Guest,
            "reporter" => Permission::Reporter,
            "developer" => Permission::Developer,
            "maintainer" => Permission::Maintainer,
            "owner" => Permission::Owner,
            other => return Err(format!("invalid permission: {other}")),
        };
        entries.push((repo.to_string(), perm));
    }
    Ok(entries)
}

async fn resolve_scopes(
    ctx: &gitquarry_core::app::AppContext,
    entry_list: &str,
) -> Result<BTreeMap<Uid, Permission>, String> {
    let entries = parse_scopes_field(entry_list)?;
    if entries.is_empty() {
        return Err("a token needs at least one repo scope".to_string());
    }
    let mut scopes = BTreeMap::new();
    for (repo, perm) in entries {
        let uid = settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, &repo)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("repo not found: {repo}"))?;
        scopes.insert(uid, perm);
    }
    Ok(scopes)
}

#[derive(serde::Deserialize)]
pub(crate) struct PatForm {
    action: String,
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    uid: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    scopes: String,
    #[serde(default)]
    expires_days: String,
}

pub(crate) async fn handler_pat_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<PatForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/pat").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;

    let mut pending_secret = None;
    let message = match form.action.as_str() {
        "create" => match create_token(&ctx, &me, &form).await {
            Ok(secret) => {
                pending_secret = Some(secret);
                "token created.".to_string()
            }
            Err(e) => e,
        },
        "revoke" => {
            domain::pat::revoke(&ctx, &me.uid, &form.uid, me.is_admin).await.ok();
            "revoked.".to_string()
        }
        other => gitquarry_i18n::shared::unknown_action(locale, other),
    };

    let tokens = access::list_tokens_for_owner(ctx.backend().as_ref(), &ctx.cipher, &me.uid).await?;
    let repo_names = repo_uid_to_name_map(&ctx, &tokens).await;
    let content = render_pat_page(
        &tokens,
        &repo_names,
        &csrf_field(&headers),
        Some(Flash::Ok(&message)),
        pending_secret.as_deref(),
        locale,
    );
    // Async (`data-async-target="#pat-body"`, see `crate::webui::ASYNC_HEADER`'s
    // own doc comment) — a created token's one-time secret still shows up
    // fine in the swapped fragment, it just never triggers a real page
    // navigation `create`/`revoke` can leave an F5 warning about.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        return Ok(axum::response::Html(content.into_string()).into_response());
    }
    let topnav = login::topnav(&ctx, &headers).await;
    let body = account_settings_page("pat", content, locale).into_string();
    Ok(axum::response::Html(layout(
        "pat",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

async fn create_token(ctx: &gitquarry_core::app::AppContext, me: &User, form: &PatForm) -> Result<String, String> {
    let scopes = resolve_scopes(ctx, &form.scopes).await?;
    let expires_in_days = form.expires_days.trim().parse::<u32>().ok();
    match domain::pat::create(ctx, &me.uid, form.label.trim(), scopes, expires_in_days)
        .await
        .map_err(|e| e.to_string())?
    {
        Some((_token, secret)) => Ok(secret),
        None => Err("try again.".to_string()),
    }
}

/// Both forms below (revoke, create) target this one `#pat-body` wrapper:
/// revoking changes the table, creating adds a row *and* shows the
/// one-time secret banner above it — either way the whole thing has to
/// refresh, not just the form that submitted, same reasoning
/// `render_appearance_page`'s own doc comment gives.
#[cfg(any(test, feature = "test-util"))]
pub fn render_pat_page(
    tokens: &[PersonalAccessToken],
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    message: Option<Flash>,
    pending_secret: Option<&str>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    render_pat_page_impl(tokens, repo_names, csrf, message, pending_secret, locale)
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_pat_page(
    tokens: &[PersonalAccessToken],
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    message: Option<Flash>,
    pending_secret: Option<&str>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    render_pat_page_impl(tokens, repo_names, csrf, message, pending_secret, locale)
}
fn render_pat_page_impl(
    tokens: &[PersonalAccessToken],
    repo_names: &HashMap<Uid, String>,
    csrf: &str,
    message: Option<Flash>,
    pending_secret: Option<&str>,
    locale: gitquarry_i18n::Locale,
) -> Markup {
    use gitquarry_i18n::web::auth::pat as t;
    let action_url = "/_account/pat";
    html! {
      div id="pat-body" {
        (PreEscaped(render_flash(message, locale)))

        @if let Some(secret) = pending_secret {
            div class="alert alert-warn" { span class="alert-body" { (t::secret_reveal_warning(locale)) } }
            p { pre { (secret) } " " (copy_button(secret, locale)) }
        }

        (settings_section(&t::tokens_heading(locale), Some(&t::page_intro(locale)), html! {
            table class="list" {
                tr {
                    th { (t::label_column_header(locale)) }
                    th { (t::scopes_column_header(locale)) }
                    th { (t::expires_column_header(locale)) }
                    th { (t::last_used_column_header(locale)) }
                    th {}
                }
                @if tokens.is_empty() {
                    tr { td colspan="5" { em { (t::no_tokens_yet_hint(locale)) } } }
                }
                @for token in tokens {
                    tr {
                        td { (token.label) }
                        td {
                            @for (i, (repo_uid, perm)) in token.scopes.iter().enumerate() {
                                @if i > 0 { ", " }
                                code { (repo_names.get(repo_uid).map(String::as_str).unwrap_or("?")) ":" (format!("{perm:?}").to_lowercase()) }
                            }
                        }
                        td { (token.expires_at.as_deref().unwrap_or("—")) }
                        td { (token.last_used_at.as_deref().unwrap_or("—")) }
                        td {
                            form method="post" action=(&action_url) style="display:inline" data-async data-async-target="#pat-body" data-confirm=(t::revoke_confirm(locale, &token.label)) {
                                (PreEscaped(csrf))
                                input type="hidden" name="action" value="revoke";
                                input type="hidden" name="uid" value=(token.uid.as_str());
                                button type="submit" { (t::revoke_button(locale)) }
                            }
                        }
                    }
                }
            }
        }))

        (settings_section(&t::create_heading(locale), None, html! {
            form method="post" action=(&action_url) data-async data-async-target="#pat-body" {
                (PreEscaped(csrf))
                input type="hidden" name="action" value="create";
                (field(&t::label_column_header(locale), None, html! {
                    input name="label" size="30" placeholder=(t::label_placeholder(locale));
                }))
                (field(&t::scopes_column_header(locale), None, html! {
                    input name="scopes" size="60" placeholder=(t::scopes_placeholder(locale));
                }))
                (field(&t::expires_column_header(locale), None, html! {
                    input name="expires_days" type="number" min="1" size="6" placeholder=(t::expires_days_placeholder(locale));
                }))
                button type="submit" { (t::create_button(locale)) }
            }
        }))
      }
    }
}
