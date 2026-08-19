//! Every mutation of an [`crate::auth::access::User`] goes through here —
//! `auth::web::users` calls this instead of orchestrating
//! `reserve_namespace`/`claim_pubkey`/`claim_email`/`put_user` (and their
//! rollback-on-failure ordering) inline. See [`crate::domain::repo`]'s doc
//! comment for the `domain`/`web` boundary this follows: transport-
//! agnostic, `anyhow::Result`, no CSRF/session/audit concerns.

use crate::app::AppContext;
use crate::auth::access;
use crate::auth::entities::{Permission, User};
use crate::storage::Cas;
use crate::storage::uid::Uid;
use anyhow::{Context, Result, bail};
use gitquarry_auth_crypto::{password, totp};
use s3db::txn::{self, TxnOp};
use std::collections::BTreeMap;

/// Creates a new user: bundles the namespace reservation (shared with
/// [`crate::auth::access::Group`] names), the pubkey/email claims if given,
/// and the user object write itself into one create-only [`txn::run`]
/// transaction — either all of them land, or none do, with no manual
/// rollback-on-failure dance needed in between. `None` if any of those keys
/// is already claimed by something else (a user, a group, or a pre-existing
/// index entry) — the transaction's `Cas::Conflict` doesn't distinguish
/// which one lost, so every such case collapses to the same "already taken"
/// outcome `username`-only conflicts used to have on their own.
/// Bundled so [`create`] doesn't cross clippy's `too_many_arguments` --
/// adding `locale` (needed for [`parse_repos_field`]'s error message) was
/// the 8th positional param, past the threshold.
pub struct NewUser<'a> {
    pub username: &'a str,
    pub email: Option<String>,
    pub pubkey: Option<&'a str>,
    pub password_plain: Option<&'a str>,
    pub is_admin: bool,
    pub repos_field: &'a str,
    pub locale: gitquarry_i18n::Locale,
}

pub async fn create(ctx: &AppContext, new_user: NewUser<'_>) -> Result<Option<Cas>> {
    let NewUser {
        username,
        email,
        pubkey,
        password_plain,
        is_admin,
        repos_field,
        locale,
    } = new_user;
    access::validate_username(username)?;

    // Validated (and, for named repos, resolved) before any txn op is
    // built, same as before: a bad entry here must short-circuit with no
    // side effects at all, not even a namespace reservation to roll back.
    let (repos, default_permission) = if repos_field.trim().is_empty() {
        (BTreeMap::new(), None)
    } else {
        parse_repos_field(ctx, repos_field, locale).await?
    };

    let mut ops = vec![TxnOp::create(
        access::NAMESPACE_INDEX.key(username),
        &access::NamespaceReservation {
            kind: "user".to_string(),
        },
    )?];

    let pubkeys = match pubkey {
        None => Vec::new(),
        Some(pk) => {
            let fingerprint = access::key_fingerprint(pk)?;
            ops.push(TxnOp::create(
                access::PUBKEY_INDEX.key(&fingerprint),
                &access::UsernamePointer {
                    username: username.to_string(),
                },
            )?);
            vec![pk.to_string()]
        }
    };

    let password_hash = password_plain.map(password::hash_password).transpose()?;
    let has_password = password_hash.is_some();
    let user = User {
        uid: Uid::new(),
        username: username.to_string(),
        email,
        pubkeys,
        gpg_keys: Default::default(),
        repos,
        default_permission,
        password_hash,
        session_epoch: 0,
        is_admin,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    };
    if let Some(op) = user.email_claim_op()? {
        ops.push(op);
    }
    ops.push(user.create_op()?);

    let cas = txn::run(ctx.backend().as_ref(), &ctx.cipher, ops).await?;
    if cas == Cas::Conflict {
        return Ok(None);
    }
    let _ = user.claim_uid(ctx.backend().as_ref(), &ctx.cipher).await;
    if has_password {
        access::bump_password_login_count(ctx.backend().as_ref(), &ctx.cipher, 1).await;
    }
    Ok(Some(cas))
}

/// Best-effort: deletes the user object (the one authoritative, fail-closed
/// step — every lookup path stops finding them the instant it happens) then
/// releases every index entry that pointed at them. A no-op if `username`
/// doesn't exist.
pub async fn delete(ctx: &AppContext, username: &str) -> Result<()> {
    let Some(user) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?.0 else {
        return Ok(());
    };
    access::delete_user_object(ctx.backend().as_ref(), &ctx.cipher, username).await;
    user.release_uid(ctx.backend().as_ref()).await;
    for pubkey in &user.pubkeys {
        if let Ok(fp) = access::key_fingerprint(pubkey) {
            access::release_pubkey(ctx.backend().as_ref(), &fp).await;
        }
    }
    user.release_email(ctx.backend().as_ref()).await;
    access::release_namespace(ctx.backend().as_ref(), username).await;
    if user.password_hash.is_some() {
        access::bump_password_login_count(ctx.backend().as_ref(), &ctx.cipher, -1).await;
    }
    // Every verdict this account's keys ever produced now names a user
    // that no longer exists, and its emails are free for someone else to
    // claim — both change what a cached verdict should say.
    invalidate_cached_signatures(ctx).await;
    Ok(())
}

/// Marks every cached commit-signature verdict stale, so the next render
/// recomputes instead of trusting an answer derived from key/email state
/// that just changed — see `storage::commit_sig`'s own doc comment for why
/// this is one global stamp rather than a per-user one.
async fn invalidate_cached_signatures(ctx: &AppContext) {
    crate::storage::commit_sig::bump_signing_epoch(ctx.backend().as_ref(), &ctx.cipher).await;
}

/// Sets (or, with `None`, clears) `username`'s primary `email`.
///
/// `User::email` was immutable in practice until this existed — only
/// [`create`] ever claimed `EMAIL_INDEX`, so an account created without one
/// could never acquire one, which `setup.rs`'s bootstrap admin always is.
/// That was never a deliberate rule, just a missing operation: the reason
/// the field is off-limits to *self*-service is that OIDC resolves a login
/// to a `User` by it, so letting someone assign their own would let them
/// claim another account's identity. An admin action carries no such
/// problem — it is the same authority that picks the address at `add_user`
/// time.
///
/// The new address is claimed *before* the old one is released, and only
/// released once the user record actually points somewhere else: the
/// reverse order would open a window where the account's own address is
/// unclaimed and another account could take it. A claim that loses (the
/// address already belongs to someone) is rolled back rather than left
/// dangling, since nothing else would ever collect it.
///
/// `Err` if `username` doesn't exist, or if `email` is already associated
/// with another account.
pub async fn set_email(
    ctx: &AppContext,
    username: &str,
    email: Option<&str>,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let email = email.map(crate::validate::validate_secondary_email).transpose()?;
    let (user, etag) = require_user(ctx, username, locale).await?;
    if user.email == email {
        return Ok(Cas::Ok);
    }

    let previous = user.clone();
    let mut user = user;
    user.email = email;
    if let Some(op) = user.email_claim_op()?
        && txn::run(ctx.backend().as_ref(), &ctx.cipher, vec![op]).await? == Cas::Conflict
    {
        bail!("{}", gitquarry_i18n::shared::email_already_associated(locale));
    }

    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    match cas {
        // Nothing points at the old address any more.
        Cas::Ok => {
            previous.release_email(ctx.backend().as_ref()).await;
            invalidate_cached_signatures(ctx).await;
        }
        // A concurrent edit won; the claim we just took belongs to nobody.
        Cas::Conflict => user.release_email(ctx.backend().as_ref()).await,
    }
    Ok(cas)
}

/// `password_plain: None` clears the password (SSH/OIDC-only afterward).
/// `Err` if `username` doesn't exist.
pub async fn set_password(
    ctx: &AppContext,
    username: &str,
    password_plain: Option<&str>,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    let password_hash = password_plain.map(password::hash_password).transpose()?;
    let delta = match (user.password_hash.is_some(), password_hash.is_some()) {
        (false, true) => 1,
        (true, false) => -1,
        _ => 0,
    };
    user.password_hash = password_hash;
    // Any session issued under the old password stops being accepted — see
    // `User::session_epoch`. Changing a password is the standard response to
    // "someone else may be in my account", so it has to actually evict them
    // rather than leaving their sealed cookie good for the rest of its TTL.
    user.session_epoch = user.session_epoch.wrapping_add(1);
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok && delta != 0 {
        access::bump_password_login_count(ctx.backend().as_ref(), &ctx.cipher, delta).await;
    }
    Ok(cas)
}

/// `quota_bytes: None` clears the limit (unlimited) — same "empty means
/// unset" shape [`set_password`]'s `None` already has.
pub async fn set_storage_quota(
    ctx: &AppContext,
    username: &str,
    quota_bytes: Option<u64>,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.storage_quota_bytes = quota_bytes;
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await
}

pub async fn toggle_admin(ctx: &AppContext, username: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.is_admin = !user.is_admin;
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await
}

/// `Err` if `pubkey` doesn't parse or is already claimed by a different
/// user.
pub async fn add_pubkey(ctx: &AppContext, username: &str, pubkey: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let fingerprint = access::key_fingerprint(pubkey)?;
    if !access::claim_pubkey(ctx.backend().as_ref(), &ctx.cipher, &fingerprint, username).await? {
        bail!("{}", gitquarry_i18n::shared::key_already_associated(locale));
    }
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.pubkeys.push(pubkey.to_string());
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await
}

pub async fn remove_pubkey(
    ctx: &AppContext,
    username: &str,
    fingerprint: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.pubkeys
        .retain(|k| access::key_fingerprint(k).map(|fp| fp != fingerprint).unwrap_or(true));
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok {
        access::release_pubkey(ctx.backend().as_ref(), fingerprint).await;
    }
    Ok(cas)
}

/// No global uniqueness claim, unlike [`add_pubkey`] — see [`crate::auth::entities::User::gpg_keys`]'s
/// own doc comment for why. `Err` (via [`gitquarry_gpg::fingerprint`]) if
/// `armored_key` isn't a well-formed OpenPGP public key block; the
/// fingerprint it returns is what gets stored alongside the key, once,
/// rather than re-derived by every future render/removal.
pub async fn add_gpg_key(
    ctx: &AppContext,
    username: &str,
    armored_key: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let fingerprint = gitquarry_gpg::fingerprint(armored_key).await?;
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.gpg_keys.push(crate::auth::entities::GpgKeyEntry {
        armored: armored_key.to_string(),
        fingerprint,
    });
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok {
        invalidate_cached_signatures(ctx).await;
    }
    Ok(cas)
}

pub async fn remove_gpg_key(
    ctx: &AppContext,
    username: &str,
    fingerprint: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.gpg_keys.retain(|k| k.fingerprint != fingerprint);
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok {
        invalidate_cached_signatures(ctx).await;
    }
    Ok(cas)
}

/// Sets (or, `perm: None`, clears) `username`'s per-repo grant for `repo` —
/// `repo == "*"` targets `default_permission` instead of a specific repo,
/// same convention `parse_repos_field` uses for the bulk form. `Err` if
/// `repo` doesn't name an existing repo (and isn't `"*"`) or `username`
/// doesn't exist.
pub async fn set_repo_perm(
    ctx: &AppContext,
    username: &str,
    repo: &str,
    perm: Option<Permission>,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    if repo == "*" {
        user.default_permission = perm;
    } else {
        match perm {
            Some(p) => {
                let repo_uid = crate::storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, repo)
                    .await?
                    .with_context(|| gitquarry_i18n::shared::repo_not_found(locale, repo))?;
                user.repos.insert(repo_uid, p);
            }
            None => {
                // No existence check to clear: if the repo was since
                // renamed/deleted there's no uid left to look up by name,
                // but that's harmless — an orphaned uid in `repos` never
                // matches anyone's request (uids are never reused).
                if let Some(repo_uid) =
                    crate::storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, repo).await?
                {
                    user.repos.remove(&repo_uid);
                }
            }
        }
    }
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await
}

/// Starts (or restarts) TOTP enrollment: generates a fresh secret, stores it
/// unconfirmed (`totp_enabled` stays/becomes `false`), and returns it for the
/// caller to render as a QR/otpauth-URI. `Err` if 2FA is already enabled —
/// `disable_totp` must run first, so a fresh pending secret can never
/// silently replace a live one the user hasn't proven they still control.
pub async fn enable_totp(ctx: &AppContext, username: &str, locale: gitquarry_i18n::Locale) -> Result<(Cas, String)> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    if user.totp_enabled {
        bail!("{}", gitquarry_i18n::shared::totp_already_enabled(locale));
    }
    let secret = totp::generate_secret();
    user.totp_secret = Some(secret.clone());
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    Ok((cas, secret))
}

/// How many single-use recovery codes a fresh generation produces — enough
/// to last a good while without a TOTP device, not so many the list becomes
/// unwieldy to store somewhere safe. Same "not worth a config knob" call as
/// `domain::repo::DELETE_GRACE_SECS`.
const RECOVERY_CODE_COUNT: usize = 10;

/// Fresh plaintext codes plus their Argon2id hashes (same treatment as
/// `password::hash_password` gives an account password — these are typed by
/// hand at login exactly like one). The plaintext half is only ever handed
/// back to the caller for a one-time reveal, never stored.
fn fresh_recovery_codes() -> Result<(Vec<String>, Vec<String>)> {
    let mut plaintext = Vec::with_capacity(RECOVERY_CODE_COUNT);
    let mut hashed = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let code = totp::generate_recovery_code();
        hashed.push(password::hash_password(&code).context("hashing a freshly generated recovery code")?);
        plaintext.push(code);
    }
    Ok((plaintext, hashed))
}

/// Confirms a pending enrollment: verifies `code` against the currently
/// stored (unconfirmed) secret and, only on success, flips `totp_enabled`
/// and mints a fresh batch of recovery codes. `Err` if there's no pending
/// secret or the code doesn't verify — either way nothing about the user is
/// changed. The returned `Vec<String>` is the *plaintext* codes, for the
/// caller to show exactly once — [`User::totp_recovery_codes`] only ever
/// holds their hashes from this point on.
pub async fn confirm_totp(
    ctx: &AppContext,
    username: &str,
    code: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(Cas, Vec<String>)> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    let secret = user
        .totp_secret
        .clone()
        .with_context(|| gitquarry_i18n::shared::no_pending_totp_enrollment(locale))?;
    if !totp::verify_code(&secret, code) {
        bail!("{}", gitquarry_i18n::shared::incorrect_code(locale));
    }
    let (plaintext, hashed) = fresh_recovery_codes()?;
    user.totp_enabled = true;
    user.totp_recovery_codes = hashed;
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    Ok((cas, plaintext))
}

/// Discards whatever recovery codes exist and mints a fresh batch — the
/// self-service response to "I've used most of them" or "I think someone
/// saw my list". `Err` if 2FA isn't even enabled (nothing to regenerate
/// codes for). No re-auth check here, same split [`disable_totp`]'s own doc
/// comment describes: the caller (`auth::web::security`'s self-service
/// action) checks the account password or a live code first.
pub async fn regenerate_recovery_codes(
    ctx: &AppContext,
    username: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(Cas, Vec<String>)> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    if !user.totp_enabled {
        bail!("{}", gitquarry_i18n::shared::no_pending_totp_enrollment(locale));
    }
    let (plaintext, hashed) = fresh_recovery_codes()?;
    user.totp_recovery_codes = hashed;
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    Ok((cas, plaintext))
}

/// Checks `code` against `username`'s stored recovery-code hashes and, on a
/// match, removes it (one-time use) and saves — `Ok(true)` only once the
/// removal itself is persisted, so a match that then lost a concurrent CAS
/// race to another login attempt correctly reports failure rather than
/// letting the same code work twice. `Ok(false)` for no match, a user with
/// 2FA off, or a user that doesn't exist — [`crate::auth::web::login`] treats
/// every one of those identically to a wrong TOTP code, never leaking which.
pub async fn consume_recovery_code(ctx: &AppContext, username: &str, code: &str) -> Result<bool> {
    let (Some(mut user), etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await? else {
        return Ok(false);
    };
    if !user.totp_enabled {
        return Ok(false);
    }
    let Some(matched_at) = user
        .totp_recovery_codes
        .iter()
        .position(|hash| password::verify_password(code, hash))
    else {
        return Ok(false);
    };
    user.totp_recovery_codes.remove(matched_at);
    Ok(access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await? == Cas::Ok)
}

/// Disables 2FA and clears the secret plus every recovery code. No re-auth
/// check happens here — that's the caller's job: `auth::web::security`'s
/// self-service action checks the account password or a live code before
/// calling this; `auth::web::users`' admin action calls this directly with
/// no re-auth at all, as the break-glass path for a user who lost their
/// device *and* every recovery code (or never had 2FA confirmed at all).
pub async fn disable_totp(ctx: &AppContext, username: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.totp_secret = None;
    user.totp_enabled = false;
    user.totp_recovery_codes = Vec::new();
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await
}

/// `Err` if `email` doesn't parse or is already claimed by a different
/// user (as either their primary `email` or one of their own
/// `secondary_emails`) — same shape [`add_pubkey`] has for fingerprints.
pub async fn add_secondary_email(
    ctx: &AppContext,
    username: &str,
    email: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let email = crate::validate::validate_secondary_email(email)?;
    if !access::claim_secondary_email(ctx.backend().as_ref(), &ctx.cipher, &email, username).await? {
        bail!("{}", gitquarry_i18n::shared::email_already_associated(locale));
    }
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.secondary_emails.insert(email);
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok {
        invalidate_cached_signatures(ctx).await;
    }
    Ok(cas)
}

pub async fn remove_secondary_email(
    ctx: &AppContext,
    username: &str,
    email: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.secondary_emails.remove(email);
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok {
        access::release_secondary_email(ctx.backend().as_ref(), email).await;
        invalidate_cached_signatures(ctx).await;
    }
    Ok(cas)
}

/// Stores `content_type` (already sniffed/validated by the caller via
/// `validate::sniff_avatar_content_type`) as `username`'s avatar mime and
/// uploads the already-written `source` file to the avatar blob store. The
/// blob upload happens before the metadata write, same ordering
/// `domain::release::add_asset` uses for release assets: a `Cas::Conflict`
/// here just leaves an unreferenced blob overwritten again on retry, never a
/// `User` pointing at a blob that was never actually uploaded.
pub async fn set_avatar(
    ctx: &AppContext,
    username: &str,
    content_type: &str,
    source: &std::path::Path,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    ctx.avatar_remote().upload_avatar(username, source).await?;
    user.avatar_content_type = Some(content_type.to_string());
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await
}

/// Self-service revert to the generated identicon. The metadata flip
/// (`avatar_content_type = None`) is what actually stops the avatar from
/// being served/shown — the blob delete is best-effort afterward, same
/// "metadata first, blob cleanup can't corrupt anything if lost" ordering
/// `Remote::delete_asset` uses.
pub async fn remove_avatar(ctx: &AppContext, username: &str, locale: gitquarry_i18n::Locale) -> Result<Cas> {
    let (mut user, etag) = require_user(ctx, username, locale).await?;
    user.avatar_content_type = None;
    let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await?;
    if cas == Cas::Ok {
        ctx.avatar_remote().delete_avatar(username).await;
    }
    Ok(cas)
}

async fn require_user(
    ctx: &AppContext,
    username: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(User, Option<String>)> {
    let (user, etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username).await?;
    let user = user.with_context(|| gitquarry_i18n::shared::user_not_found(locale, username))?;
    Ok((user, etag))
}

/// Parses `"repo1:write,repo2:read,*:read"` into (explicit per-repo grants,
/// resolved to each repo's uid; the `"*"` default, split out into its own
/// return value since `User::repos` is uid-keyed and has no string value
/// left to safely reserve as a sentinel the way `"*"` was). Every named repo
/// must currently exist — a real tightening from the old scheme, which
/// never validated a repo name at all and just carried it as inert dead
/// text forever if it was wrong.
async fn parse_repos_field(
    ctx: &AppContext,
    s: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(BTreeMap<Uid, Permission>, Option<Permission>)> {
    let mut repos = BTreeMap::new();
    let mut default_permission = None;
    for entry in s.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (repo, perm) = entry
            .split_once(':')
            .with_context(|| gitquarry_i18n::shared::invalid_entry(locale, entry))?;
        let repo = repo.trim();
        let perm = match perm.trim() {
            "guest" => Permission::Guest,
            "reporter" => Permission::Reporter,
            "developer" => Permission::Developer,
            "maintainer" => Permission::Maintainer,
            "owner" => Permission::Owner,
            other => bail!("{}", gitquarry_i18n::shared::invalid_permission(locale, other)),
        };
        if repo == "*" {
            default_permission = Some(perm);
            continue;
        }
        let uid = crate::storage::settings::resolve_repo_name(ctx.backend().as_ref(), &ctx.cipher, repo)
            .await?
            .with_context(|| gitquarry_i18n::shared::repo_not_found(locale, repo))?;
        repos.insert(uid, perm);
    }
    Ok((repos, default_permission))
}
