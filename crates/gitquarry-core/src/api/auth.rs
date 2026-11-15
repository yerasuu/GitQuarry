//! `Authorization: Bearer` for `/_api/v1`.
//!
//! The credential is a local personal access token (`gqpat_…`,
//! [`PersonalAccessToken`]) minted for the API, with `ApiGrant` scopes and
//! optional per-repo path lists (decision #144); a git-smart-HTTP token is
//! refused here as git refuses an API one. Federation's own tokens are
//! federation's API, and a session cookie is never accepted, so a browser
//! tab can't be driven into this API by a cross-site request.
//!
//! [`ApiPrincipal::ceiling`] is the one place a token's own limit on a repo
//! is read, and [`ApiPrincipal::permission_on`] always takes the minimum of
//! it and the owner's live permission, so no scope can raise what the owner
//! has.

use super::error::{ApiError, ErrorCode};
use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{ApiScope, Permission, PersonalAccessToken, User};
use crate::storage::uid::Uid;
use crate::validate;
use anyhow::Result;
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use std::collections::BTreeSet;
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
            .with_hint("send `Authorization: Bearer <token>` with an API access token")
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
        .with_hint("check that it is an API token, not a git one, copied whole and not expired or revoked")
}

/// Which half of the API a permission is asked for: repo scopes and tracker
/// scopes cap independently, so a `tracker:write` token can file issues on
/// a repo it can't push to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Repo,
    Tracker,
}

/// The highest tier `scopes` allow in `area`; a higher scope implies the
/// lower ones. `repo:admin` is `Owner` so that it is the per-repo ceiling in
/// [`PersonalAccessToken::scopes`], not the scope, that decides between
/// settings and delete/transfer.
fn scope_ceiling(scopes: &BTreeSet<ApiScope>, area: Area) -> Option<Permission> {
    let tiers: &[(ApiScope, Permission)] = match area {
        Area::Repo => &[
            (ApiScope::RepoAdmin, Permission::Owner),
            (ApiScope::RepoWrite, Permission::Developer),
            (ApiScope::RepoRead, Permission::Reporter),
        ],
        Area::Tracker => &[
            (ApiScope::TrackerWrite, Permission::Developer),
            (ApiScope::TrackerRead, Permission::Reporter),
        ],
    };
    tiers
        .iter()
        .find(|(scope, _)| scopes.contains(scope))
        .map(|&(_, tier)| tier)
}

/// Who a `/_api/v1` request acts as: a live API token and its live owner.
/// Never carries the secret, and nothing here logs it.
///
/// Nothing here applies the owner's global-admin bypass: a token is held by
/// an agent, and an admin's token gets only what its scopes and repos say.
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

    /// The most this token allows in `area` on `repo_uid` by itself,
    /// ignoring what its owner can do: the lower of its per-repo ceiling and
    /// what its scopes allow there. `None` if the token does not name the
    /// repo or holds no scope in `area`.
    pub fn ceiling(&self, area: Area, repo_uid: &Uid) -> Option<Permission> {
        let repo = self.token.scopes.get(repo_uid).copied()?;
        let scopes = &self.token.api.as_ref()?.scopes;
        scope_ceiling(scopes, area).map(|scope| scope.min(repo))
    }

    /// What this request may do in `area` on the repo: the owner's current
    /// permission on `full_name`, capped by [`Self::ceiling`] on `repo_uid`.
    /// The caller passes both because a token names repos by uid and
    /// permissions are granted by name; they must be the same repo.
    ///
    /// # Errors
    ///
    /// Storage failures reading the owner's grants.
    pub async fn permission_on(
        &self,
        ctx: &AppContext,
        area: Area,
        repo_uid: &Uid,
        full_name: &str,
    ) -> Result<Option<Permission>> {
        let Some(ceiling) = self.ceiling(area, repo_uid) else {
            return Ok(None);
        };
        let owner =
            access::effective_permission_for_user(ctx.backend().as_ref(), &ctx.cipher, &self.user, full_name).await?;
        Ok(owner.map(|p| p.min(ceiling)))
    }

    /// [`Self::permission_on`], as the gate an endpoint puts before acting:
    /// the permission when it reaches `needed`. A repo the request can't
    /// even read is `not_found`, as a private repo is to a stranger, so a
    /// token can't probe which repos exist; one it can read but not act on
    /// this way is `forbidden`.
    ///
    /// # Errors
    ///
    /// `not_found` or `forbidden` as above, `internal` on storage failures.
    pub async fn require(
        &self,
        ctx: &AppContext,
        area: Area,
        repo_uid: &Uid,
        full_name: &str,
        needed: Permission,
    ) -> Result<Permission, ApiError> {
        let permission = self
            .permission_on(ctx, area, repo_uid, full_name)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        match permission {
            Some(p) if p >= needed => Ok(p),
            Some(p) if p >= Permission::Reporter => Err(ApiError::new(
                ErrorCode::Forbidden,
                "this token does not allow that on this repo",
            )
            .with_hint("the token's scopes, its ceiling on the repo, or its owner's permission are too low")),
            _ => Err(ApiError::new(ErrorCode::NotFound, "no such repo for this token")),
        }
    }

    /// Checks that `path` (a tree path in `repo_uid`, `""` for the root) is
    /// inside the token's path list for that repo, if it has one. A listed
    /// path covers itself and everything under it, matched by whole
    /// segments: `src/api` covers `src/api/v1.rs`, never `src/apis`. With a
    /// list, the root is outside it, since listing it would show the names
    /// of the subtrees the token is kept out of.
    ///
    /// # Errors
    ///
    /// `invalid_parameter` for a path `validate::validate_tree_path`
    /// rejects; `forbidden` for a path outside the list.
    pub fn require_path(&self, repo_uid: &Uid, path: &str) -> Result<(), ApiError> {
        validate::validate_tree_path(path).map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
        let inside = match self.token.api.as_ref().map(|grant| grant.paths.get(repo_uid)) {
            Some(None) => true,
            Some(Some(allowed)) => allowed.iter().any(|prefix| {
                path.strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            }),
            None => false,
        };
        if inside {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "this token is confined to other paths in this repo",
            )
            .with_hint("ask for a path under one of the token's paths, listed by whoami"))
        }
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
        // Answered as any other bad secret, so the API never confirms that a
        // secret it refuses opens git.
        if token.api.is_none() {
            return Err(invalid_token());
        }
        let user = access::resolve_user_uid(backend.as_ref(), &ctx.cipher, &token.owner_uid)
            .await
            .map_err(|e| ApiError::internal(&e))?
            .ok_or_else(invalid_token)?;
        access::touch_pat_last_used(backend.as_ref(), &ctx.cipher, &token).await;
        Ok(ApiPrincipal { user, token })
    }
}
