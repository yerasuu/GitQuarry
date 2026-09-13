//! `POST /_tags`, `/_tags/delete`: create a tag pointing at an existing
//! commit, or delete one. Top-level routes, not nested under the repo's own
//! `/{*repo}` path — same reason as [`super::releases`]'s routes (see that
//! module's doc comment): the repo name arrives as a hidden form field, not
//! a path segment, since `/{*repo}` is already bound to a different POST
//! handler.

use crate::auth::web::login;
use crate::webui::page_chrome::{HeaderExtras, header_bar};
use crate::webui::{
    AppError, State_, effective_density, effective_locale, effective_theme, layout, repo_link, require_csrf,
    require_login, tracker_ui_active, url_encode,
};
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::entities::{Permission, User};
use gitquarry_core::domain;
use gitquarry_core::git;
use gitquarry_core::storage::Cas;
use gitquarry_core::validate;
use gitquarry_i18n::web::repo::refs as t;

/// Resolves `repo` and checks `me` has at least `Write` on it — same gate
/// and same "the actual mutation stays re-checked here, not just at the
/// form" reasoning as [`super::releases::require_write_target`], duplicated
/// rather than shared since the two live in sibling files with no natural
/// common parent below `webui::repo`.
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

#[derive(serde::Deserialize)]
pub(crate) struct TagForm {
    tag: String,
    source: String,
    #[serde(default)]
    csrf: String,
}

/// `GET` on this POST-only mutation target: what a browser asks for when it
/// reloads or backs into the URL a rejected submit left in the address bar.
/// The repo is in the path, so it can land on the repo's refs page, where the tag forms live
/// rather than on the router's generic "back to the index" fallback.
pub(crate) async fn handler_mutation_get(Path(repo): Path<String>) -> Response {
    axum::response::Redirect::to(&format!("/{}?view=refs", url_encode(&repo))).into_response()
}

pub(crate) async fn handler_create_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<TagForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let tag = form.tag.trim();
    let source = form.source.trim();

    // A rejected create (bad rev, unresolvable source, tag already
    // exists, ...) re-renders the refs tab with the typed tag/source
    // still filled in, instead of losing them to the generic error page.
    macro_rules! fail {
        ($msg:expr) => {{
            let error_mirror = ctx.open_mirror_read(&resolved).await?;
            let body = super::refs::render_refs(
                &ctx,
                &error_mirror.path,
                &name,
                &headers,
                Some((tag, source)),
                Some(&$msg),
                locale,
            )
            .await?;
            drop(error_mirror);
            let (config, _etag) = resolved.remote.get_repo_config().await?;
            let topnav = login::topnav(&ctx, &headers).await;
            let density = effective_density(&ctx, &headers).await;
            let header = header_bar(
                &name,
                "",
                "refs",
                HeaderExtras {
                    description: config.description.as_deref(),
                    display_name: config.display_name.as_deref(),
                    archived: config.archived,
                    visibility: config.visibility,
                    branches: &[],
                    menu_extra: "",
                    below_header: "",
                    sub_tabs: "",
                    tracker_active: tracker_ui_active(&ctx),
                    locale,
                },
            );
            return Ok(Html(layout(
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
    if let Err(e) = validate::validate_rev(source) {
        fail!(e.to_string());
    }

    // Needs the local mirror (not just the manifest) to turn `source` —
    // a branch, tag, or short sha — into the full commit sha the new tag
    // will actually point at, and to reject it up front if it doesn't
    // resolve to anything, instead of writing an unverified string into
    // the manifest.
    let mirror = ctx.open_mirror_read(&resolved).await?;
    let resolved_sha = git::browse::resolve_commit(&mirror.path, source).await?;
    drop(mirror);
    let Some(sha) = resolved_sha else {
        fail!(t::invalid_tag_source(locale, source));
    };

    let cas = match domain::repo::create_tag(&resolved, tag, &sha, locale).await {
        Ok(cas) => cas,
        Err(e) => fail!(e.to_string()),
    };
    if cas == Cas::Conflict {
        fail!(gitquarry_i18n::shared::cas_conflict(locale));
    }

    gitquarry_core::storage::audit::record(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &me.username,
        "tag.create",
        &format!("{name}@{tag}"),
        None,
    )
    .await;
    Ok(Redirect::to(&repo_link(&name, "refs", "").to_string()).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct TagDeleteForm {
    tag: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_delete_post(
    State(ctx): State_,
    Path(repo): Path<String>,
    headers: HeaderMap,
    Form(form): Form<TagDeleteForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, &format!("/{}", url_encode(&repo))).await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;
    let (resolved, name) = require_write_target(&ctx, &me, &repo, locale).await?;

    let tag = form.tag.trim();
    let cas = domain::repo::delete_tag(&resolved, tag, locale)
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
        "tag.delete",
        &format!("{name}@{tag}"),
        None,
    )
    .await;
    // Async (`data-async-target="#tag-table"`, see `crate::webui::ASYNC_HEADER`'s
    // own doc comment): the deleted row has to disappear from a table
    // listing every tag, not just this one button's own label, so the
    // whole table gets re-rendered from a fresh ref list.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        let mirror = ctx.open_mirror_read(&resolved).await?;
        let tags = git::browse::list_refs_detailed(&mirror.path, "refs/tags/").await?;
        return Ok(
            axum::response::Html(super::refs::tag_table(&name, &tags, true, &headers, locale).into_string())
                .into_response(),
        );
    }
    Ok(Redirect::to(&repo_link(&name, "refs", "").to_string()).into_response())
}
