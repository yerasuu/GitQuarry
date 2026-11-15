//! Checks on what reaches tracker over its loopback API, applied before
//! anything is read or written.

use anyhow::{Result, bail};
use gitquarry_wire::Uid;

/// Accepts only the shape [`Uid::new`] produces: 32 lowercase hex digits.
/// The uid becomes the prefix of every issue, board and milestone key, and
/// `s3db::Collection::new_owned` takes that prefix as given, so a `/` or a
/// `..` here would address another repo's collections.
pub(crate) fn validate_uid(raw: &str) -> Result<Uid> {
    if raw.len() != 32 || !raw.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        bail!("invalid uid (expected 32 lowercase hex digits)");
    }
    Ok(Uid::from(raw.to_string()))
}
