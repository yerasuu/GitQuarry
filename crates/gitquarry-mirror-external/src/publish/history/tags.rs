//! Annotated tags on the public side. A published annotated tag is a new tag
//! object: its `object` names the rewritten commit, its tagger goes through
//! the identity map like an author does, and its signature is dropped. The
//! signature covered the private object id, so it would never verify on the
//! public tag; publishing it would only claim a verification that fails.
//! The message itself passes through, like a commit message, and is
//! secret-scanned like one.
//!
//! The format is git's tag object (`object`, `type`, `tag`, `tagger`
//! headers, a blank line, the message), see
//! [git-mktag](https://git-scm.com/docs/git-mktag); a SHA-1 repository
//! appends the signature to the message, see
//! [signature format](https://git-scm.com/docs/gitformat-signature).

use crate::publish::objects::{ObjectKind, ObjectReader, Oid, from_hex, hash_object, to_hex};
use crate::publish::rewrite::{IdentRole, Rewriter};
use anyhow::{Context, Result, bail};

/// Lines that open a signature appended to a tag message. git takes the
/// last such line as the start of the signature, and so does this.
const SIGNATURE_STARTS: [&[u8]; 4] = [
    b"-----BEGIN PGP SIGNATURE-----",
    b"-----BEGIN PGP MESSAGE-----",
    b"-----BEGIN SSH SIGNATURE-----",
    b"-----BEGIN SIGNED MESSAGE-----",
];

/// Header lines that sign the private tag (the SHA-256 compatibility form).
const DROPPED_HEADERS: [&[u8]; 2] = [b"gpgsig", b"gpgsig-sha256"];

/// A private annotated tag as published.
pub(crate) struct PublicTag {
    pub oid: Oid,
    pub body: Vec<u8>,
    message_at: usize,
    /// The tagger's email when the identity map didn't cover it.
    pub unmapped: Vec<(IdentRole, String)>,
}

impl PublicTag {
    pub(crate) fn message(&self) -> &[u8] {
        &self.body[self.message_at..]
    }
}

/// The public tag object for private tag `tag`, pointing at `public_commit`.
///
/// # Errors
///
/// Fails when the tag isn't a well-formed tag of a commit: a tag of a tree,
/// a blob or another tag isn't published.
pub(crate) fn rewrite_tag(
    reader: &mut ObjectReader,
    rewriter: &Rewriter,
    tag: &Oid,
    public_commit: Oid,
) -> Result<PublicTag> {
    let body = reader.read_kind(tag, ObjectKind::Tag)?;
    let split = body.windows(2).position(|w| w == b"\n\n").map_or(body.len(), |i| i + 1);
    let (head, message) = body.split_at(split);
    let message = message.strip_prefix(b"\n").unwrap_or(message);
    let mut headers: Vec<Vec<u8>> = Vec::new();
    let mut unmapped = Vec::new();
    let mut dropping = false;
    let mut object = None;
    for line in head.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        if line.starts_with(b" ") {
            if !dropping && let Some(last) = headers.last_mut() {
                last.push(b'\n');
                last.extend_from_slice(line);
            }
            continue;
        }
        dropping = false;
        let (key, value) = line
            .iter()
            .position(|&b| b == b' ')
            .map_or((line, &line[line.len()..]), |i| (&line[..i], &line[i + 1..]));
        match key {
            b"object" => {
                object = Some(from_hex(&String::from_utf8_lossy(value))?);
                headers.push(format!("object {}", to_hex(&public_commit)).into_bytes());
            }
            b"type" if value != b"commit" => {
                bail!(
                    "tag {} points at a {}, only tags of commits are published",
                    to_hex(tag),
                    String::from_utf8_lossy(value)
                );
            }
            b"tagger" => {
                let mut line = b"tagger ".to_vec();
                line.extend(rewriter.map_ident(value, IdentRole::Tagger, &mut unmapped));
                headers.push(line);
            }
            k if DROPPED_HEADERS.contains(&k) => dropping = true,
            _ => headers.push(line.to_vec()),
        }
    }
    object.with_context(|| format!("tag {} has no object line", to_hex(tag)))?;
    let message = strip_signature(message);
    let mut out = Vec::with_capacity(body.len());
    for header in &headers {
        out.extend_from_slice(header);
        out.push(b'\n');
    }
    out.push(b'\n');
    let message_at = out.len();
    out.extend_from_slice(message);
    Ok(PublicTag {
        oid: hash_object(ObjectKind::Tag, &out),
        body: out,
        message_at,
        unmapped,
    })
}

fn strip_signature(message: &[u8]) -> &[u8] {
    let mut start = None;
    let mut offset = 0;
    for line in message.split_inclusive(|&b| b == b'\n') {
        if SIGNATURE_STARTS.iter().any(|s| line.starts_with(s)) {
            start = Some(offset);
        }
        offset += line.len();
    }
    start.map_or(message, |at| &message[..at])
}
