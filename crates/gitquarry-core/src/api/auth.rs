//! `Authorization: Bearer` for `/_api/v1`.
//!
//! The credential today is the local personal access token (`gqpat_…`,
//! [`PersonalAccessToken`]): the only token core resolves by itself.
//! Federation's own tokens are federation's API, and a session cookie is
//! never accepted, so a browser tab can't be driven into this API by a
//! cross-site request.
//!
//! The seam for scoped agent tokens (#146) is [`ApiPrincipal::ceiling`]:
//! it is the one place a token's own limit on a repo is read, and
//! [`ApiPrincipal::permission_on`] always takes the minimum of it and the
//! owner's live permission, so whatever #146 makes the ceiling it can never
//! raise what the owner has.

use super::error::{ApiError, ErrorCode};
use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{Permission, PersonalAccessToken, User};
use crate::storage::uid::Uid;
use anyhow::Result;
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use std::sync::Arc;

/// `gqpat_` and 40 lowercase hex, as `access::generate_secret` mints them.
fn well_formed_secret(secret: &str) -> bool {
    secret
        .strip_prefix("gqpat_")
        .is_some_and(|hex| hex.len() == 40 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// The bearer secret, or why there is none. The scheme is matched without
/// regard to case (RFC 7235 section 2.1).
fn bearer_secret(parts: &Parts) -> Result<&str, ApiError> {
    let missing = || {
        ApiError::new(ErrorCode::Unauthenticated, "this endpoint needs a bearer token")
            .with_hint("send `Authorization: Bearer <token>` with a personal access token")
    };
    let value = parts
        .headers
        .get(header::AUTHORIZATION)
        .ok_or_else(missing)?
        .to_str()
        .map_err(|_| invalid_token())?;
    let (scheme, secret) = value.split_once(' ').ok_or_else(missing)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(missing());
    }
    Ok(secret.trim())
}

fn invalid_token() -> ApiError {
    ApiError::new(ErrorCode::InvalidToken, "the bearer token is not valid")
        .with_hint("check that the token was copied whole and has not expired or been revoked")
}

/// Who a `/_api/v1` request acts as: a live token and its live owner. Never
/// carries the secret, and nothing here logs it.
#[derive(Debug)]
pub struct ApiPrincipal {
    user: User,
    token: PersonalAccessToken,
}

impl ApiPrincipal {
    pub fn user(&self) -> &User {
        &self.user
    }

    pub fn token(&self) -> &PersonalAccessToken {
        &self.token
    }

    /// The most this token allows on `repo_uid` by itself, ignoring what its
    /// owner can do; `None` if the token does not name the repo.
    pub fn ceiling(&self, repo_uid: &Uid) -> Option<Permission> {
        self.token.scopes.get(repo_uid).copied()
    }

    /// What this request may do on the repo: the owner's current permission
    /// on `full_name`, capped by [`Self::ceiling`] on `repo_uid`. The caller
    /// passes both because a token names repos by uid and permissions are
    /// granted by name; they must be the same repo.
    ///
    /// # Errors
    ///
    /// Storage failures reading the owner's grants.
    pub async fn permission_on(&self, ctx: &AppContext, repo_uid: &Uid, full_name: &str) -> Result<Option<Permission>> {
        let Some(ceiling) = self.ceiling(repo_uid) else {
            return Ok(None);
        };
        let owner =
            access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &self.user, full_name).await?;
        Ok(owner.map(|p| p.min(ceiling)))
    }
}

impl FromRequestParts<Arc<AppContext>> for ApiPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, ctx: &Arc<AppContext>) -> Result<Self, Self::Rejection> {
        let secret = bearer_secret(parts)?;
        // Checked before any lookup, so a malformed header costs no S3 read.
        if !well_formed_secret(secret) {
            return Err(invalid_token());
        }
        let backend = ctx.backend();
        let token = access::resolve_by_secret(backend.as_ref(), &ctx.cipher, secret)
            .await
            .map_err(|e| ApiError::internal(&e))?
            .ok_or_else(invalid_token)?;
        let user = access::resolve_user_uid(backend.as_ref(), &ctx.cipher, &token.owner_uid)
            .await
            .map_err(|e| ApiError::internal(&e))?
            .ok_or_else(invalid_token)?;
        access::touch_pat_last_used(backend.as_ref(), &ctx.cipher, &token).await;
        Ok(ApiPrincipal { user, token })
    }
}
