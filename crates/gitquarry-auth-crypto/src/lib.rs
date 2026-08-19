//! Self-contained auth primitives with no dependency on `gitquarry-core`'s
//! own `AppContext`/storage: [`oidc`] builds the OIDC client at startup
//! (discovery against the configured issuer), [`password`] hashes/verifies
//! the local password login, [`totp`] generates/verifies RFC 6238 TOTP codes
//! for opt-in 2FA on the password-login path.

pub mod oidc;
pub mod password;
pub mod totp;
