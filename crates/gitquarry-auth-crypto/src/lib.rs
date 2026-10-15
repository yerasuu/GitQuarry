//! Self-contained auth primitives with no dependency on `gitquarry-core`'s
//! own `AppContext`/storage: [`oidc`] builds the OIDC client at startup
//! (discovery against the configured issuer), [`password`] hashes/verifies
//! the local password login, [`totp`] generates/verifies RFC 6238 TOTP codes
//! for opt-in 2FA on the password-login path, [`webauthn`] verifies passkey
//! registrations and sign-ins.

pub mod oidc;
pub mod password;
pub mod totp;
pub mod webauthn;

/// A fresh 192-bit secret from the CSPRNG, as 48 lowercase hex characters.
/// For the one-time tokens an operator copies out of the log (the web UI's
/// bootstrap grant, the setup wizard): long enough that guessing one before
/// it dies with the process is out of reach.
pub fn random_token() -> String {
    let raw: [u8; 24] = rand::random();
    raw.iter().fold(String::with_capacity(48), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}
