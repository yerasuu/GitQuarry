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
//!
//! Also handles federation layer 2's fork-from-peer (`form.peer` non-empty):
//! same destination-namespace rules, but the source is a linked peer's repo.
//! Core resolves the destination owner itself (it owns `User`/`Group`,
//! federation doesn't), then hands off to `gitquarry-federation` via
//! `POST /_internal-out/fork` (`webui::federation_client::request_fork`),
//! which re-validates the peer link (not banned, at least `BrowseAndFork`)
//! before forwarding back to core's own
//! `POST /_internal/repos/fork-from-remote` for the actual git mechanic.

use super::{
    AppError, State_, effective_density, effective_locale, effective_theme, layout, require_csrf, require_login,
    url_encode,
};
use crate::auth::web::login;
use crate::webui::federation_client;
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::domain;
use gitquarry_core::storage::entities::OwnerRef;
use gitquarry_core::validate;
use maud::html;

#[derive(serde::Deserialize)]
pub(crate) struct ForkForm {
    /// Full name (`owner/name`) of the repo being forked *on the peer*
    /// (`peer` non-empty) — only used by the fork-from-peer branch. That
    /// form posts to `/_fork/{repo}` too (the only route there is), but the
    /// path there names no local repo, so this field stays the source of
    /// truth for it. The same-bucket fork takes its source from the URL path
    /// (see `handler_fork_post`'s own `Path(source)` param).
    #[serde(default)]
    source: String,
    /// Linked peer's `Peer::name`, or empty for a same-bucket fork — the
    /// only thing that decides which branch below runs.
    #[serde(default)]
    peer: String,
    /// The caller's own username, or a group they can manage — checked
    /// against `access.json`, never trusted as free text (same reasoning
    /// as `repo_new::NewRepoForm::namespace`).
    namespace: String,
    name: String,
    #[serde(default)]
    csrf: String,
}

/// Resolves `namespace` (the caller's own username, or a group they can
/// manage) to the `OwnerRef` a new repo's `RepoConfig::owner` gets — shared
/// by both the same-bucket and fork-from-peer branches below, since neither
/// cares where the *source* came from.
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

/// The typed name as a display name plus the slug the destination path is
/// built from, or the reason it can't be one. Shared by both branches so a
/// fork from a peer can't produce a repo name the local path would refuse.
fn dest_name(name: &str) -> Result<(String, String), String> {
    let display = validate::validate_repo_display_name(name).map_err(|e| e.to_string())?;
    let slug = domain::repo::slugify_repo_name(&display)
        .ok_or_else(|| "name must contain at least one letter or digit.".to_string())?;
    Ok((display, slug))
}

/// A rejected local fork (bad name, destination already exists, ...)
/// re-renders a small standalone page with this same form — everything
/// the caller typed still filled in, the reason shown above it — instead
/// of losing it to the generic error page. Duplicates (rather than reuses)
/// `webui::repo::summary::render_fork_form`'s markup: that helper is
/// private to `webui::repo`, and this is the one place outside it that
/// needs the same small form. The federation fork-from-peer path (`form.peer`
/// non-empty) doesn't go through this — a separate, more involved UI
/// surface left for its own pass.
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
    // Only the groups `me` actually belongs to (plus their subgroups), read
    // from the `user-group` index — not every group in the bucket, which
    // this used to list just to filter almost all of it back out.
    let groups = crate::auth::web::groups::visible_groups(ctx, me).await?;
    let topnav = login::topnav(ctx, headers).await;
    let body = html! {
        p class="alert alert-err" { (message) }
        form method="post" action=(format!("/_fork/{}", crate::webui::url_encode(source))) class="fork-form" {
            (maud::PreEscaped(crate::webui::csrf_field(headers)))
            select name="namespace" {
                option value=(me.username) selected[namespace == me.username] { (me.username) }
                @for name in crate::auth::web::groups::manageable_group_names(&groups, &me.uid, me.is_admin) {
                    option value=(name) selected[namespace == name] { (name) }
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

/// `GET` on this POST-only mutation target: what a browser asks for when it
/// reloads or backs into the URL a rejected submit left in the address bar.
/// The repo is in the path, so it can land on the repo that was being forked
/// rather than on the router's generic "back to the index" fallback.
pub(crate) async fn handler_mutation_get(Path(repo): Path<String>) -> Response {
    axum::response::Redirect::to(&format!("/{}", url_encode(&repo))).into_response()
}

pub(crate) async fn handler_fork_post(
    State(ctx): State_,
    // The repo being forked: from the URL now, not a hidden field, and
    // still re-validated/re-checked (existence + visibility) below rather
    // than trusted.
    Path(source): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ForkForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/").await {
        Ok(u) => u,
        Err(resp) => return Ok(*resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let namespace = form.namespace.trim();

    let peer_name = form.peer.trim();
    if !peer_name.is_empty() {
        let source_repo = validate::sanitize_path(&form.source).map_err(AppError::bad_request)?;
        let Some(federation) = &ctx.federation else {
            return Err(AppError::not_found(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::repo::repo_not_found(locale, peer_name)
            )));
        };
        let dest_owner = resolve_dest_owner(&ctx, &me, namespace, locale).await?;
        let (_, slug) = dest_name(&form.name).map_err(|msg| AppError::bad_request(anyhow::anyhow!("{msg}")))?;
        let dest = validate::sanitize_path(&format!("{namespace}/{slug}")).map_err(AppError::bad_request)?;
        let (dest_owner_kind, dest_owner_uid) = match &dest_owner {
            OwnerRef::User(uid) => ("user", uid),
            OwnerRef::Group(uid) => ("group", uid),
        };
        // Best-effort, never re-fetched after this — same tolerance the
        // pre-split `fork_from_peer` already had for a stale/missing cache
        // entry: worst case the new repo just starts with no description.
        let description = federation_client::peer_repos(federation)
            .await
            .into_iter()
            .find(|(name, _)| name == peer_name)
            .and_then(|(_, repos)| repos.into_iter().find(|r| r.name == source_repo))
            .and_then(|r| r.description);

        let outcome = federation_client::request_fork(
            federation,
            &federation_client::ForkRequest {
                peer_name,
                source_repo_name: &source_repo,
                dest_full_name: &dest,
                dest_owner_kind,
                dest_owner_uid,
                description,
                acting_username: &me.username,
            },
        )
        .await;
        if !outcome.ok {
            return Err(AppError::bad_request(anyhow::anyhow!(
                "{}",
                outcome.reason.unwrap_or_else(|| "fork failed".to_string())
            )));
        }

        gitquarry_core::storage::audit::record(
            ctx.backend().as_ref(),
            &ctx.cipher,
            &me.username,
            "repo.fork",
            &dest,
            Some(format!("{peer_name}:{source_repo}")),
        )
        .await;

        return Ok(Redirect::to(&format!("/{}", url_encode(&dest))).into_response());
    }

    let source = validate::sanitize_path(&source).map_err(AppError::bad_request)?;
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
    let (dest_display_name, slug) = match dest_name(&form.name) {
        Ok(names) => names,
        Err(msg) => return fork_error_page(&ctx, &headers, &me, &source, namespace, &form.name, &msg).await,
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
