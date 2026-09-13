//! RFC 6238 TOTP (HMAC-SHA1, 30s step, 6 digits) — a thin wrapper over the
//! `hmac`/`sha1`/`data-encoding` crates, all already in this binary's
//! dependency tree transitively (via `russh`/`aws-smithy-checksums`) before
//! this module promoted them to direct dependencies. Deliberately not a
//! whole-purpose TOTP crate: this is ~40 lines of well-specified logic this
//! codebase can own and unit-test directly, same spirit as
//! [`super::password`]'s thin Argon2 wrapper instead of a generic "auth"
//! crate. Unrelated to `gitquarry_core::crypto::Cipher` (bucket-at-rest encryption)
//! — a different concern, same "don't conflate" split `password` already
//! keeps from `crypto`.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

/// 160 bits — RFC 4226's recommended HOTP secret size.
#[cfg(any(test, feature = "test-util"))]
pub const SECRET_BYTES: usize = 20;
#[cfg(not(any(test, feature = "test-util")))]
const SECRET_BYTES: usize = 20;
/// RFC 6238 default step.
#[cfg(any(test, feature = "test-util"))]
pub const STEP_SECS: u64 = 30;
#[cfg(not(any(test, feature = "test-util")))]
const STEP_SECS: u64 = 30;
const DIGITS: u32 = 6;
/// ±30s clock-drift tolerance: a code stays acceptable for at most three
/// steps total (one before, the current one, one after), not an unbounded
/// replay window.
const SKEW_STEPS: i64 = 1;

/// Fresh random secret, base32-encoded — what's shown to the user (and
/// embedded in the `otpauth://` URI): authenticator apps expect base32, not
/// raw bytes.
pub fn generate_secret() -> String {
    let raw: [u8; SECRET_BYTES] = rand::random();
    BASE32_NOPAD.encode(&raw)
}

/// One single-use TOTP recovery code, shown to the user exactly once
/// (enrollment/regeneration) then only ever stored hashed — same "typed by
/// hand" reasoning as [`generate_secret`]'s base32, except hex here: these
/// are meant for a one-time copy-paste or a slip of paper, not scanned, and
/// lowercase hex has no `0`/`O`/`1`/`I`/`l` ambiguity to trip someone up
/// re-typing it from print. 8 random bytes (64 bits) — the same order of
/// per-code guessing difficulty a 6-digit live TOTP code already has once
/// you factor in this app's login rate limiting, not a full independent
/// account-recovery secret.
pub fn generate_recovery_code() -> String {
    let raw: [u8; 8] = rand::random();
    let hex = data_encoding::HEXLOWER.encode(&raw);
    format!("{}-{}-{}-{}", &hex[0..4], &hex[4..8], &hex[8..12], &hex[12..16])
}

#[cfg(any(test, feature = "test-util"))]
pub fn hotp(secret_raw: &[u8], counter: u64) -> Option<u32> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret_raw).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let code = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    Some(code % 10u32.pow(DIGITS))
}
#[cfg(not(any(test, feature = "test-util")))]
fn hotp(secret_raw: &[u8], counter: u64) -> Option<u32> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret_raw).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let code = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    Some(code % 10u32.pow(DIGITS))
}

/// `true` if `code` (as typed by the user) matches `secret_b32` at the
/// current 30s step or one step either side. `false` (never panics or
/// errors) for a malformed code or secret — same "syntactically broken
/// input behaves like a wrong answer" doctrine as
/// [`super::password::verify_password`].
pub fn verify_code(secret_b32: &str, code: &str) -> bool {
    verify_code_at(secret_b32, code, SystemTime::now())
}

/// Which 30s step `code` matched, if any — [`verify_code`] with the answer
/// kept instead of thrown away. A caller that persists the last step it
/// accepted can then refuse to accept the same one twice: a code stays
/// mathematically valid for its whole ±1-step window, so an observed code
/// is replayable for up to 90s unless somebody remembers it was used.
pub fn matching_step(secret_b32: &str, code: &str) -> Option<u64> {
    matching_step_at(secret_b32, code, SystemTime::now())
}

#[cfg(any(test, feature = "test-util"))]
pub fn matching_step_at(secret_b32: &str, code: &str, now: SystemTime) -> Option<u64> {
    matching_step_impl(secret_b32, code, now)
}
#[cfg(not(any(test, feature = "test-util")))]
fn matching_step_at(secret_b32: &str, code: &str, now: SystemTime) -> Option<u64> {
    matching_step_impl(secret_b32, code, now)
}

fn matching_step_impl(secret_b32: &str, code: &str, now: SystemTime) -> Option<u64> {
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secret_raw = BASE32_NOPAD.decode(secret_b32.trim_end_matches('=').as_bytes()).ok()?;
    let elapsed = now.duration_since(UNIX_EPOCH).ok()?;
    let counter = elapsed.as_secs() / STEP_SECS;
    let want = code.parse::<u32>().ok()?;
    (-SKEW_STEPS..=SKEW_STEPS).find_map(|skew| {
        let c = counter.checked_add_signed(skew)?;
        (hotp(&secret_raw, c)? == want).then_some(c)
    })
}

#[cfg(any(test, feature = "test-util"))]
pub fn verify_code_at(secret_b32: &str, code: &str, now: SystemTime) -> bool {
    matching_step_at(secret_b32, code, now).is_some()
}
#[cfg(not(any(test, feature = "test-util")))]
fn verify_code_at(secret_b32: &str, code: &str, now: SystemTime) -> bool {
    matching_step_at(secret_b32, code, now).is_some()
}

/// Minimal percent-encoding for an `otpauth://` URI's path/query — this
/// module can't reach for `crate::webui::url_encode` (auth is a lower layer
/// than web), so this is its own tiny copy of the same "keep alnum and a
/// few safe chars, escape the rest" logic.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `otpauth://totp/<issuer>:<account>?secret=...&issuer=...` — what the
/// enrollment page renders as a clickable link next to the base32 secret a
/// user can also copy in by hand.
pub fn otpauth_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    format!(
        "otpauth://totp/{}:{}?secret={secret_b32}&issuer={}&digits={DIGITS}&period={STEP_SECS}",
        percent_encode(issuer),
        percent_encode(account),
        percent_encode(issuer),
    )
}

/// Renders `uri` (an [`otpauth_uri`] output) as inline SVG XML — scan it
/// with an authenticator app instead of typing the base32 secret by hand,
/// same convenience Forgejo/GitHub's own 2FA enrollment page offers. `None`
/// only if the data is too long for any QR version to hold, which an
/// `otpauth://` URI never comes close to.
pub fn qr_svg(uri: &str) -> Option<String> {
    let code = qrcode::QrCode::new(uri.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .build(),
    )
}
