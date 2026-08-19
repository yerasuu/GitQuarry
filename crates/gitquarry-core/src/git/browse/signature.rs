//! Extracts a commit's detached signature (if any) from its raw object —
//! the `gpgsig <sha>` header a signed commit carries — so
//! [`gitquarry_gpg::verify_detached_signature`] can check it the same way it
//! checks a release asset's signature: no separate GPG-specific commit-
//! object parser needed there.

use crate::git::run;
use anyhow::Result;
use std::ffi::OsStr;
use std::path::Path;

/// `(payload, signature)` if `sha` carries a `gpgsig` header, `None`
/// otherwise. `payload` is the commit object with that header removed —
/// exactly the bytes the signature was made over, same "strip the
/// signature header, verify what's left" algorithm `git`'s own
/// `verify-commit` uses internally. Works for any signature format
/// (`-----BEGIN PGP SIGNATURE-----` or `-----BEGIN SSH SIGNATURE-----`):
/// this only extracts the header, distinguishing formats is the caller's
/// job by inspecting `signature`'s own banner line.
pub async fn commit_raw_signature(git_dir: &Path, sha: &str) -> Result<Option<(Vec<u8>, String)>> {
    let object = run(
        Some(git_dir),
        &[
            OsStr::new("cat-file"),
            OsStr::new("commit"),
            OsStr::new("--end-of-options"),
            OsStr::new(sha),
        ],
    )
    .await?;
    Ok(strip_signature(&object))
}

/// A commit object's header lines are `\n`-separated, one header per line;
/// a multi-line value (only `gpgsig` ever has one) continues on following
/// lines each prefixed with a single space, dedented here to reconstruct
/// the real signature text.
fn strip_signature(object: &[u8]) -> Option<(Vec<u8>, String)> {
    let mut lines = object.split(|&b| b == b'\n');
    let mut payload_lines: Vec<&[u8]> = Vec::new();
    let mut signature_lines: Vec<String> = Vec::new();
    let mut in_signature = false;

    for line in &mut lines {
        if in_signature {
            if line.starts_with(b" ") {
                signature_lines.push(String::from_utf8_lossy(&line[1..]).into_owned());
                continue;
            }
            in_signature = false;
        }
        if !in_signature && line.starts_with(b"gpgsig ") {
            signature_lines.push(String::from_utf8_lossy(&line[b"gpgsig ".len()..]).into_owned());
            in_signature = true;
            continue;
        }
        payload_lines.push(line);
    }

    if signature_lines.is_empty() {
        return None;
    }
    let mut payload = Vec::new();
    for (i, line) in payload_lines.iter().enumerate() {
        if i > 0 {
            payload.push(b'\n');
        }
        payload.extend_from_slice(line);
    }
    let signature = signature_lines.join("\n");
    Some((payload, signature))
}
