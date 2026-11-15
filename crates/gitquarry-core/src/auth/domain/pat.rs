//! [`PersonalAccessToken`] create/revoke — mirrors
//! `gitquarry-federation`'s `domain::token` module as closely as the two
//! entities' shapes allow. See [`crate::domain::repo`]'s doc comment for
//! the `domain`/`web` boundary this follows: transport-agnostic,
//! `anyhow::Result`, no CSRF/session/audit concerns. Repo names are never
//! resolved here — the caller (a web handler) resolves them to uids first,
//! same convention federation's own `domain::token::create` follows.

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{ApiGrant, Permission, PersonalAccessToken};
use crate::storage::Cas;
use crate::storage::audit::format_unix_secs;
use crate::storage::uid::Uid;
use crate::validate;
use anyhow::{Result, bail};
use s3db::txn;
use std::collections::BTreeMap;

/// Keeps one token record small; a monorepo agent needing more subtrees
/// than this on one repo is better served by the whole repo.
const MAX_PATHS_PER_REPO: usize = 64;

/// Creates a git-smart-HTTP token owned by `owner_uid`, scoped to `scopes`
/// (repo uid → `Permission` ceiling, already resolved by the caller) with an
/// optional expiry. Returns the stored record and the one-time plaintext
/// secret — the only moment it's ever available; only
/// `PersonalAccessToken::token_hash` survives past this call. `Ok(None)`
/// only on a fresh-uid collision (`Cas::Conflict`, vanishingly rare with 160
/// bits of randomness). The entity write and its `token_hash` index claim
/// land in one create-only [`txn::run`] batch. The token is refused by
/// `/_api/v1`; see [`create_api`].
pub async fn create(
    ctx: &AppContext,
    owner_uid: &Uid,
    label: &str,
    scopes: BTreeMap<Uid, Permission>,
    expires_in_days: Option<u32>,
) -> Result<Option<(PersonalAccessToken, String)>> {
    insert(ctx, owner_uid, label, scopes, None, expires_in_days).await
}

/// [`create`] for a `/_api/v1` token, which git-smart-HTTP refuses. `grant`
/// needs at least one scope, and each of its path lists has to name a repo
/// in `scopes` and hold only non-root paths `validate::validate_tree_path`
/// accepts: a path is matched against request paths by whole segments, so
/// anything else would either never match or match more than it says.
///
/// # Errors
///
/// An invalid label, no repo, no scope, or a path list breaking the rules
/// above; storage failures.
pub async fn create_api(
    ctx: &AppContext,
    owner_uid: &Uid,
    label: &str,
    scopes: BTreeMap<Uid, Permission>,
    grant: ApiGrant,
    expires_in_days: Option<u32>,
) -> Result<Option<(PersonalAccessToken, String)>> {
    if grant.scopes.is_empty() {
        bail!("an API token needs at least one scope");
    }
    for (repo_uid, paths) in &grant.paths {
        if !scopes.contains_key(repo_uid) {
            bail!("a path list names a repo the token does not");
        }
        if paths.is_empty() || paths.len() > MAX_PATHS_PER_REPO {
            bail!("a repo's path list needs 1 to {MAX_PATHS_PER_REPO} paths");
        }
        for path in paths {
            if path.is_empty() {
                bail!("a path list can't hold the repo root; leave the repo out of it instead");
            }
            validate::validate_tree_path(path)?;
        }
    }
    insert(ctx, owner_uid, label, scopes, Some(grant), expires_in_days).await
}

async fn insert(
    ctx: &AppContext,
    owner_uid: &Uid,
    label: &str,
    scopes: BTreeMap<Uid, Permission>,
    api: Option<ApiGrant>,
    expires_in_days: Option<u32>,
) -> Result<Option<(PersonalAccessToken, String)>> {
    access::validate_pat_label(label)?;
    if scopes.is_empty() {
        bail!("a token needs at least one repo scope");
    }

    let secret = access::generate_secret();
    let now = access::now_secs();
    let expires_at = expires_in_days.map(|days| format_unix_secs(now + u64::from(days) * 86400));
    let token = PersonalAccessToken {
        uid: Uid::new(),
        owner_uid: owner_uid.clone(),
        label: label.to_string(),
        token_hash: access::hash_token(&secret),
        scopes,
        created_at: format_unix_secs(now),
        expires_at,
        last_used_at: None,
        api,
    };

    let ops = vec![
        token.create_op()?,
        token
            .token_hash_claim_op()?
            .expect("token_hash is never empty, always claims"),
    ];
    let cas = txn::run(ctx.backend().as_ref(), &ctx.cipher, ops).await?;
    if cas == Cas::Conflict {
        return Ok(None);
    }
    access::add_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner_uid, &token.uid).await;
    Ok(Some((token, secret)))
}

/// Revokes `uid` — a no-op if it doesn't exist, isn't owned by `owner_uid`,
/// and the caller isn't an admin (`caller_is_admin`, same "admin can manage
/// anything" carve-out `access::can_manage_repo_settings` already applies
/// elsewhere — otherwise there'd be no way to clean up an orphaned token
/// after its owner account is deleted).
pub async fn revoke(ctx: &AppContext, owner_uid: &Uid, uid: &str, caller_is_admin: bool) -> Result<()> {
    let Some(token) = access::get_pat(ctx.backend().as_ref(), &ctx.cipher, uid).await? else {
        return Ok(());
    };
    if &token.owner_uid != owner_uid && !caller_is_admin {
        return Ok(());
    }
    let _ = PersonalAccessToken::delete(ctx.backend().as_ref(), &ctx.cipher, uid).await;
    let _ = token.release_token_hash(ctx.backend().as_ref()).await;
    access::remove_owner_index(ctx.backend().as_ref(), &ctx.cipher, &token.owner_uid, &token.uid).await;
    Ok(())
}
