//! Validation of form fields only this crate receives, on top of
//! `gitquarry_core::validate` (repo path, rev, sha, tree path).

use anyhow::{Result, bail};

/// A tracker comment id as `gitquarry-tracker`'s `storage::thread::thread_key`
/// mints it: 32 decimal digits of nanoseconds, `-`, 8 lowercase hex digits.
/// The id becomes a path segment of the loopback call to the tracker, so
/// anything else (a `/`, `?` or `#` first among them) is rejected before it
/// can address another repo's comment.
pub(crate) fn validate_comment_id(id: &str) -> Result<()> {
    let well_formed = id.split_once('-').is_some_and(|(nanos, suffix)| {
        nanos.len() == 32
            && nanos.bytes().all(|b| b.is_ascii_digit())
            && suffix.len() == 8
            && suffix.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    });
    if !well_formed {
        bail!("invalid comment id");
    }
    Ok(())
}
