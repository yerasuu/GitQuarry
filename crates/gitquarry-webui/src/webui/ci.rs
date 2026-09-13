//! `/_ci/<repo>`: pending `gitquarry-ci` deploy-approval list for one repo,
//! and the approve action — proxies to `gitquarry-ci`'s own loopback API
//! via `AppContext.ci` (`docs/gitquarry-ci-sdk-design.md`). Gated the same
//! tier as the repo settings tab (`Maintainer`/`Owner`/admin), since
//! approving a deploy is exactly that kind of authority.
//!
//! `gitquarry-webui` never imports `gitquarry-ci` as a crate dependency —
//! this crosses the loopback IPC boundary like every other core<->ci call,
//! so [`PendingRun`] is a plain local mirror of `gitquarry_ci::graph::JobRun`'s
//! JSON shape, not a shared Rust type.

use super::{AppError, State_, csrf_field, effective_density, effective_locale, effective_theme, layout, require_csrf};
use crate::auth::web::login;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::auth::access;
use gitquarry_core::validate;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::ci as t;
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use maud::{Markup, html};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct PendingRun {
    pub id: String,
    pub job_id: String,
    pub kind: String,
    pub sha: String,
}

fn ci_unavailable(locale: Locale) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, t::ci_not_configured(locale)).into_response()
}

/// `None` if the logged-in caller may manage `repo`'s CI approvals — same
/// tier `webui::repo_settings::require_repo_authority` requires for a
/// non-owner-tier action; `Some(response)` is what to return instead.
async fn require_ci_authority(
    ctx: &gitquarry_core::app::AppContext,
    headers: &HeaderMap,
    repo: &str,
) -> Result<Option<Response>, AppError> {
    let Some(user) = login::current_user(ctx, headers).await else {
        return Ok(Some(
            Redirect::to(&format!("/_login?return_to={}", super::url_encode(repo))).into_response(),
        ));
    };
    if access::can_manage_repo_settings_for(ctx.backend().as_ref(), &ctx.cipher, &user.username, repo).await? {
        return Ok(None);
    }
    let locale = effective_locale(ctx, headers).await;
    Ok(Some(
        AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::insufficient_repo_permission(locale)
        ))
        .into_response(),
    ))
}

pub(crate) async fn handler_ci_pending_get(
    State(ctx): State_,
    Path(raw_repo): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let name = validate::sanitize_path(&raw_repo).map_err(AppError::bad_request)?;
    if let Some(resp) = require_ci_authority(&ctx, &headers, &name).await? {
        return Ok(resp);
    }
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &name)
        )));
    };
    let Some(ci) = &ctx.ci else {
        return Ok(ci_unavailable(locale));
    };

    let url = format!(
        "{}/runs/pending?repo_uid={}",
        ci.base_url.trim_end_matches('/'),
        resolved.uid
    );
    let resp = ci
        .http
        .get(&url)
        .signed(&ci.internal_token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("couldn't reach gitquarry-ci: {e}"))?;
    let runs: Vec<PendingRun> = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("gitquarry-ci's response wasn't valid JSON: {e}"))?;

    let topnav = login::topnav(&ctx, &headers).await;
    let body = render_pending_page(&name, &runs, &csrf_field(&headers), locale).into_string();
    Ok(axum::response::Html(layout(
        &t::pending_approvals_title(locale),
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

pub fn render_pending_page(repo: &str, runs: &[PendingRun], csrf: &str, locale: Locale) -> Markup {
    html! {
        h1 { (t::pending_approvals_title(locale)) }
        p class="muted" { (t::pending_approvals_intro(locale)) }
        @if runs.is_empty() {
            p class="empty-state" { (t::no_pending_runs(locale)) }
        } @else {
            table class="list" {
                tr {
                    th { (t::job_column(locale)) }
                    th { (t::kind_column(locale)) }
                    th { (t::sha_column(locale)) }
                    th {}
                }
                @for run in runs {
                    tr {
                        td { (run.job_id) }
                        td { (run.kind) }
                        td { code { (run.sha) } }
                        td {
                            form method="post" action="/_ci/approve" {
                                input type="hidden" name="repo" value=(repo);
                                input type="hidden" name="run_id" value=(run.id);
                                (maud::PreEscaped(csrf.to_string()))
                                button type="submit" { (t::approve_button(locale)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CiApproveForm {
    repo: String,
    run_id: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_ci_approve_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<CiApproveForm>,
) -> Result<Response, AppError> {
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let name = validate::sanitize_path(&form.repo).map_err(AppError::bad_request)?;
    if let Some(resp) = require_ci_authority(&ctx, &headers, &name).await? {
        return Ok(resp);
    }
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &name)
        )));
    };
    let Some(ci) = &ctx.ci else {
        return Ok(ci_unavailable(locale));
    };

    let url = format!("{}/runs/approve", ci.base_url.trim_end_matches('/'));
    let resp = ci
        .http
        .post(&url)
        .signed(&ci.internal_token)
        .json(&serde_json::json!({ "repo_uid": resolved.uid.to_string(), "run_id": form.run_id }))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("couldn't reach gitquarry-ci: {e}"))?;
    if !resp.status().is_success() {
        let message = resp.text().await.unwrap_or_default();
        return Err(AppError::bad_request(anyhow::anyhow!("gitquarry-ci: {message}")));
    }

    Ok(Redirect::to(&format!("/_ci/{}", super::url_encode(&name))).into_response())
}
