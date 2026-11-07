use super::auth::ApiPrincipal;
use super::cursor::{self, Page, PageRequest};
use super::error::ApiError;
use crate::app::AppContext;
use crate::auth::entities::Permission;
use crate::storage::settings;
use crate::storage::uid::Uid;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::ops::Bound;
use std::sync::Arc;

#[derive(Serialize)]
struct Whoami<'a> {
    user: UserInfo<'a>,
    token: TokenInfo<'a>,
    scopes: Page<ScopeInfo>,
}

#[derive(Serialize)]
struct UserInfo<'a> {
    uid: &'a Uid,
    username: &'a str,
    is_admin: bool,
}

#[derive(Serialize)]
struct TokenInfo<'a> {
    uid: &'a Uid,
    label: &'a str,
    created_at: &'a str,
    expires_at: Option<&'a str>,
    last_used_at: Option<&'a str>,
}

/// `repo` and `permission` are `null` once the repo is gone; the token keeps
/// naming its uid until revoked.
#[derive(Serialize)]
struct ScopeInfo {
    repo_uid: Uid,
    repo: Option<String>,
    ceiling: Permission,
    permission: Option<Permission>,
}

/// `GET /_api/v1/whoami`: the token's owner, the token, and one page of the
/// repos it names with what it may do on each right now.
pub(super) async fn handler(
    State(ctx): State<Arc<AppContext>>,
    principal: ApiPrincipal,
    page: PageRequest,
) -> Result<Response, ApiError> {
    let user = principal.user();
    let token = principal.token();
    // Bound to the token, so one token's cursor never pages another's scopes.
    let scope = format!("whoami.scopes:{}", token.uid);
    let after = page.position(&ctx.cipher, &scope)?.map(Uid::from);
    let lower = after.as_ref().map_or(Bound::Unbounded, Bound::Excluded);
    let mut remaining = token.scopes.range((lower, Bound::Unbounded));

    let backend = ctx.backend();
    let mut items = Vec::with_capacity(page.limit().min(token.scopes.len()));
    for (repo_uid, &ceiling) in remaining.by_ref().take(page.limit()) {
        let (config, _) = settings::get_repo_config(backend.as_ref(), repo_uid.as_str(), &ctx.cipher)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        let (repo, permission) = if config.full_name.is_empty() {
            (None, None)
        } else {
            let permission = principal
                .permission_on(&ctx, repo_uid, &config.full_name)
                .await
                .map_err(|e| ApiError::internal(&e))?;
            (Some(config.full_name), permission)
        };
        items.push(ScopeInfo {
            repo_uid: repo_uid.clone(),
            repo,
            ceiling,
            permission,
        });
    }
    let next_cursor = match (remaining.next(), items.last()) {
        (Some(_), Some(last)) => Some(cursor::seal(&ctx.cipher, &scope, last.repo_uid.as_str())?),
        _ => None,
    };

    let body = Whoami {
        user: UserInfo {
            uid: &user.uid,
            username: &user.username,
            is_admin: user.is_admin,
        },
        token: TokenInfo {
            uid: &token.uid,
            label: &token.label,
            created_at: &token.created_at,
            expires_at: token.expires_at.as_deref(),
            last_used_at: token.last_used_at.as_deref(),
        },
        scopes: Page { items, next_cursor },
    };
    Ok(Json(body).into_response())
}
