//! Re-authentication gates shared by every self-service credential change:
//! prove the session holder is still the account holder before a change
//! that outlives the session itself. Used by [`super::security`] (password,
//! 2FA, recovery codes) and [`super::keys`] (SSH and GPG keys) alike — one
//! copy rather than one per page, since they must never drift: a gate that
//! is stricter on one page than another is only as strong as its weakest
//! copy.

use gitquarry_auth_crypto::{password, totp};
use gitquarry_core::auth::entities::User;

/// Shared re-auth gate for `disable_totp`/`regenerate_recovery_codes`:
/// either the account password or a live TOTP code, whichever the user has
/// handy — checked here, not inside `domain::user`, so a stolen session
/// cookie alone can't turn 2FA off or invalidate someone's unused recovery
/// codes (the whole point of having either).
pub(super) fn require_reauth(me: &User, attempt: &str, locale: gitquarry_i18n::Locale) -> Result<(), String> {
    let password_ok = !attempt.is_empty()
        && me
            .password_hash
            .as_deref()
            .is_some_and(|h| password::verify_password(attempt, h));
    let code_ok = me.totp_secret.as_deref().is_some_and(|s| totp::verify_code(s, attempt));
    if !password_ok && !code_ok {
        return Err(gitquarry_i18n::shared::wrong_password_or_code(locale));
    }
    Ok(())
}

/// [`require_reauth`] plus the same per-username lockout `/_login` applies
/// ([`gitquarry_core::app::LoginAttempts`]). Without it this page is an
/// unmetered guessing oracle for a 6-digit TOTP code: whoever holds a
/// session cookie could grind one out here and use it to turn 2FA off,
/// while the login form's own limiter never sees a single attempt. The
/// same bucket, deliberately, not a second one — guesses made here and at
/// `/_login` are guesses at the same account and should add up.
pub(super) async fn require_reauth_throttled(
    ctx: &gitquarry_core::app::AppContext,
    me: &User,
    attempt: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(), String> {
    // Delay rather than refuse — same reasoning as `LoginAttempts::penalty`,
    // and here the caller is already an authenticated session, so a lockout
    // would only ever hurt the person who owns the account.
    let penalty = ctx.login_attempts.penalty(&me.username);
    if !penalty.is_zero() {
        tokio::time::sleep(penalty).await;
    }
    match require_reauth(me, attempt, locale) {
        Ok(()) => {
            ctx.login_attempts.clear(&me.username);
            Ok(())
        }
        Err(e) => {
            ctx.login_attempts.record_failure(&me.username);
            Err(e)
        }
    }
}

/// Same gate as [`require_reauth_throttled`], in front of the two actions that
/// establish a credential outliving the session that created it: a new
/// password (which then logs in on its own) and a new SSH key (which then
/// pushes on its own). Without this a stolen cookie converts itself into
/// permanent access before its 7-day TTL runs out — the same reasoning
/// `disable_totp` and `regenerate_recovery_codes` already apply, which
/// these two were missing.
///
/// Skipped only when there is genuinely nothing to re-prove: an account
/// with neither a password nor a TOTP secret (an OIDC-only user setting a
/// first password, or adding a first key) has no second factor to ask for,
/// and demanding one would lock them out of ever acquiring one.
pub(super) async fn require_reauth_for_credential_change(
    ctx: &gitquarry_core::app::AppContext,
    me: &User,
    attempt: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<(), String> {
    if me.password_hash.is_none() && me.totp_secret.is_none() {
        return Ok(());
    }
    require_reauth_throttled(ctx, me, attempt, locale).await
}
