//! Local password login for the web UI (`User::password_hash`). Unrelated to
//! [`crate::crypto::Cipher`]: that derives symmetric key material from the
//! bucket passphrase with fixed params and a caller-supplied salt; this
//! produces a self-contained Argon2id PHC string (its own random salt
//! embedded) meant only for [`verify_password`], never for deriving key
//! material.

use anyhow::Result;
use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use std::sync::LazyLock;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
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
