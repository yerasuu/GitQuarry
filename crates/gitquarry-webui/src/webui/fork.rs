//! `POST /_fork`: copies a whole repo (every object under its S3 prefix —
//! manifest, config, every pack) to a new one under the caller's own
//! username or a group they own, server-side via
//! [`gitquarry_core::storage::remote::Remote::copy_all_to`] — same primitive
//! `webui::repo_settings`'s "move repo" uses, just to a namespace the caller
//! doesn't have to be an admin for. Unlike `/_new` (admin-gated: repo
//! creation is normally an operator action), forking is deliberately open
//! to any logged-in user — that's the point of a fork, and the destination
//! is always a namespace the caller already controls, never an arbitrary
//! one.

use super::{
    AppError, State_, effective_density, effective_locale, effective_theme, layout, require_csrf, require_login,
    url_encode,
};
use crate::auth::web::login;
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::domain;
use gitquarry_core::storage::entities::OwnerRef;
use gitquarry_core::validate;
use maud::html;

#[derive(serde::Deserialize)]
pub(crate) struct ForkForm {
    /// Full name (`owner/name`) of the repo being forked — a hidden field
    /// on the fork form, not user-typed, but still re-validated/re-checked
    /// (existence + visibility) here rather than trusted from the form.
    source: String,
    /// The caller's own username, or a group they can manage — checked
    /// against `access.json`, never trusted as free text (same reasoning
    /// as `repo_new::NewRepoForm::namespace`).
    namespace: String,
    name: String,
    #[serde(default)]
    csrf: String,
}

/// Resolves `namespace` (the caller's own username, or a group they can
/// manage) to the `OwnerRef` a new repo's `RepoConfig::owner` gets.
async fn resolve_dest_owner(
    ctx: &gitquarry_core::app::AppContext,
    me: &gitquarry_core::auth::entities::User,
    namespace: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<OwnerRef, AppError> {
    if namespace == me.username {
        return Ok(OwnerRef::User(me.uid.clone()));
    }
    let can_manage = gitquarry_core::auth::access::can_manage_group_for(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        namespace,
    )
    .await?;
    let group = if can_manage {
        gitquarry_core::auth::access::get_group(ctx.backend().as_ref(), &ctx.cipher, namespace)
            .await?
            .0
    } else {
        None
    };
    let Some(group) = group else {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::invalid_namespace(locale, namespace)
        )));
    };
    Ok(OwnerRef::Group(group.uid))
}

/// A rejected local fork (bad name, destination already exists, ...)
/// re-renders a small standalone page with this same form — everything
/// the caller typed still filled in, the reason shown above it — instead
/// of losing it to the generic error page. Duplicates (rather than reuses)
/// `webui::repo::summary::render_fork_form`'s markup: that helper is
/// private to `webui::repo`, and this is the one place outside it that
/// needs the same small form.
async fn fork_error_page(
    ctx: &gitquarry_core::app::AppContext,
    headers: &HeaderMap,
    me: &gitquarry_core::auth::entities::User,
    source: &str,
    namespace: &str,
    name: &str,
    message: &str,
) -> Result<Response, AppError> {
    let locale = effective_locale(ctx, headers).await;
    let groups = access::list_all_groups(&ctx.backend(), &ctx.cipher).await?;
    let topnav = login::topnav(ctx, headers).await;
    let body = html! {
        p class="alert alert-err" { (message) }
        form method="post" action="/_fork" class="fork-form" {
            (maud::PreEscaped(crate::webui::csrf_field(headers)))
            input type="hidden" name="source" value=(source);
            select name="namespace" {
                option value=(me.username) selected[namespace == me.username] { (me.username) }
                @for group in groups.iter().filter(|g| me.is_admin || g.role_of(&me.uid) >= Some(Permission::Owner)) {
                    option value=(group.name) selected[namespace == group.name] { (group.name) }
                }
            }
            input name="name" size="20" value=(name);
            button type="submit" class="btn" { (crate::webui::icons::fork()) " " (gitquarry_i18n::web::repo::summary::fork_button(locale)) }
        }
    }
    .into_string();
    Ok(Html(layout(
        "fork",
        "",
        &body,
        &topnav,
        effective_theme(ctx, headers).await,
        effective_density(ctx, headers).await,
        locale,
    ))
    .into_response())
}

pub(crate) async fn handler_fork_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<ForkForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let namespace = form.namespace.trim();

    let source = validate::sanitize_path(&form.source).map_err(AppError::bad_request)?;
    let Some(source_resolved) = ctx.resolve_repo(&source).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &source)
        )));
    };
    let (source_config, _etag) = source_resolved.remote.get_repo_config().await?;
    // Same visibility check the repo page itself applies: forking can't be
    // a side channel to read a private repo you otherwise couldn't see.
    if !super::repo::can_view_repo(&ctx, &headers, &source, source_config.visibility).await? {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &source)
        )));
    }

    let dest_owner = match resolve_dest_owner(&ctx, &me, namespace, locale).await {
        Ok(owner) => owner,
        Err(_) => {
            let msg = gitquarry_i18n::shared::repo::invalid_namespace(locale, namespace);
            return fork_error_page(&ctx, &headers, &me, &source, namespace, &form.name, &msg).await;
        }
    };
    let dest_display_name = match validate::validate_repo_display_name(&form.name) {
        Ok(n) => n,
        Err(e) => return fork_error_page(&ctx, &headers, &me, &source, namespace, &form.name, &e.to_string()).await,
    };
    let Some(slug) = domain::repo::slugify_repo_name(&dest_display_name) else {
        let msg = "name must contain at least one letter or digit.";
        return fork_error_page(&ctx, &headers, &me, &source, namespace, &form.name, msg).await;
    };
    let dest = match validate::sanitize_path(&format!("{namespace}/{slug}")) {
        Ok(dest) => dest,
        Err(e) => return fork_error_page(&ctx, &headers, &me, &source, namespace, &form.name, &e.to_string()).await,
    };
    if dest == source {
        let msg = gitquarry_i18n::shared::repo::destination_is_same_repo(locale);
        return fork_error_page(&ctx, &headers, &me, &source, namespace, &form.name, &msg).await;
    }
    let Some(_dest_resolved) = domain::repo::fork(
        &ctx,
        &source_resolved,
        &dest,
        Some(dest_display_name),
        dest_owner,
        locale,
    )
    .await?
    else {
        let msg = gitquarry_i18n::shared::repo::repo_already_exists_at(locale, &dest);
        return fork_error_page(&ctx, &headers, &me, &source, namespace, &form.name, &msg).await;
    };

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "repo.fork",
        &dest,
        Some(source.clone()),
    )
    .await;

    Ok(Redirect::to(&format!("/{}", url_encode(&dest))).into_response())
}
