//! Local password login for the web UI (`User::password_hash`). Unrelated to
//! `gitquarry_core::crypto::Cipher`: that derives symmetric key material from the
//! bucket passphrase with fixed params and a caller-supplied salt; this
//! produces a self-contained Argon2id PHC string (its own random salt
//! embedded) meant only for [`verify_password`], never for deriving key
//! material.

use anyhow::Result;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use std::sync::LazyLock;

/// Argon2id at 64 MiB / 3 passes / 1 lane, rather than `Argon2::default()`'s
/// 19 MiB / 2 / 1 — that default is the OWASP *floor*, and login is not a
/// hot path here: a handful of hashes a day, against an attacker who would
/// be running these in parallel offline if the store ever leaked. The cost
/// of being wrong is asymmetric, so this sits well above the floor.
///
/// Only ever used to produce *new* hashes: verification reads its
/// parameters out of the stored PHC string, so every hash written under the
/// old cost keeps verifying untouched and is rewritten at the new cost the
/// next time that password is set.
fn hasher() -> Argon2<'static> {
    let params = Params::new(64 * 1024, 3, 1, None).expect("argon2 params are in range");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    hasher()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hasheando password: {e}"))
}

/// `false` for a malformed `hash` too, same as a mismatched password — a
/// syntactically broken stored hash shouldn't behave differently from a
/// wrong password. Timing is handled by the `password-hash` crate itself.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    // `Argon2::default()`, deliberately, not [`hasher`]: what governs the
    // check is the cost recorded in `parsed`, and a hash written under any
    // earlier setting has to keep verifying.
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

/// A real Argon2id hash nothing will ever match, computed once per process.
/// Verify against this when the submitted username resolves to no account
/// (or to one with no password set): a nonexistent name then costs the same
/// Argon2 work as a wrong password for a real one, so response time stops
/// answering "does this account exist". Both login surfaces — the web form
/// and git-smart-HTTP's Basic auth — share this one value.
pub static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password("dummy-password-for-constant-time-login-checks").expect("hashing the dummy login password")
});
