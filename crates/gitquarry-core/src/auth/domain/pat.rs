//! [`PersonalAccessToken`] create/revoke — mirrors
//! `gitquarry-federation`'s `domain::token` module as closely as the two
//! entities' shapes allow. See [`crate::domain::repo`]'s doc comment for
//! the `domain`/`web` boundary this follows: transport-agnostic,
//! `anyhow::Result`, no CSRF/session/audit concerns. Repo names are never
//! resolved here — the caller (a web handler) resolves them to uids first,
//! same convention federation's own `domain::token::create` follows.

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{Permission, PersonalAccessToken};
use crate::storage::Cas;
use crate::storage::audit::format_unix_secs;
use crate::storage::uid::Uid;
use anyhow::{Result, bail};
use s3db::txn;
use std::collections::BTreeMap;

/// Creates a token owned by `owner_uid`, scoped to `scopes` (repo uid →
/// `Permission` ceiling, already resolved by the caller) with an optional
/// expiry. Returns the stored record and the one-time plaintext secret —
/// the only moment it's ever available; only `PersonalAccessToken::token_hash`
/// survives past this call. `Ok(None)` only on a fresh-uid collision
/// (`Cas::Conflict`, vanishingly rare with 160 bits of randomness). The
/// entity write and its `token_hash` index claim land in one create-only
/// [`txn::run`] batch.
pub async fn create(
    ctx: &AppContext,
    owner_uid: &Uid,
    label: &str,
    scopes: BTreeMap<Uid, Permission>,
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
    PersonalAccessToken::delete(ctx.backend().as_ref(), &ctx.cipher, uid).await;
    token.release_token_hash(ctx.backend().as_ref()).await;
    access::remove_owner_index(ctx.backend().as_ref(), &ctx.cipher, &token.owner_uid, &token.uid).await;
    Ok(())
}
