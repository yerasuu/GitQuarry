//! Validation of inputs coming from the network: repo path, rev, sha, path
//! inside the tree, log search query.
//!
//! Everything here is consumed by both the git:// server and the web UI:
//! both receive strings from a remote client and use them as (a) a
//! filesystem path under `mirrors_root`, (b) an S3 key prefix, and (c)
//! arguments to `git`. Without validation that's path traversal and argument
//! injection (`?ref=--output=...` would make `git show` write arbitrary
//! files), so it's validated once, at the edge, before touching disk or
//! spawning git.
//!
//! Per-repo mirror locking ([`crate::app::Locks`]) isn't here: it's
//! concurrency control, not input validation, even though it also takes a
//! repo name — see that module instead.

use anyhow::{Result, bail};

/// Defensive cap for names that end up on the filesystem or in an S3 url.
const MAX_PATH_LEN: usize = 512;
const MAX_REV_LEN: usize = 255;

/// Turns the path requested by the client (`/projectA`, `team/projectB.git`,
/// `%2e%2e/etc`, ...) into a clean S3 prefix: no leading `/`, no `.`/`..`
/// segments (anti path-traversal), no `.git` suffix on the last segment
/// (usual convention of git clients), and each segment restricted to
/// `[A-Za-z0-9._~-]` — a repo name doubles as a URL path segment and an
/// `ssh://` clone-URL component, so anything needing escaping there (a
/// space first among them) is rejected up front instead of surfacing later
/// as a broken clone command or an inconsistently-encoded link.
pub fn sanitize_path(pathname: &str) -> Result<String> {
    if pathname.len() > MAX_PATH_LEN {
        bail!("repo path too long (max {MAX_PATH_LEN} bytes)");
    }
    let mut segments: Vec<&str> = Vec::new();
    for seg in pathname.split('/') {
        match seg {
            "" | "." => continue,
            ".." => bail!("invalid repo path (contains '..'): {pathname}"),
            s if s.starts_with('-') => {
                bail!("invalid repo path (segment starts with '-'): {pathname}")
            }
            // Same charset `url_encode` (gitquarry-webui) leaves unescaped: a
            // name outside it would round-trip through a link as %XX-encoded
            // while the raw clone-URL display (`ssh://host:port/<name>`,
            // `repo::summary::render_clone_urls`) shows it verbatim — a space
            // or other reserved char there breaks the copy-pasted command.
            s if !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')) =>
            {
                bail!("invalid repo path (only letters, digits, '-', '_', '.', '~' allowed): {pathname}")
            }
            s => segments.push(s),
        }
    }
    let Some((last, head)) = segments.split_last() else {
        bail!("empty repo path: {pathname}");
    };
    let last = last.strip_suffix(".git").unwrap_or(last);
    if last.is_empty() {
        bail!("invalid repo path: {pathname}");
    }
    let mut out = head.join("/");
    if !out.is_empty() {
        out.push('/');
    }
    out.push_str(last);
    Ok(out)
}

/// Validates a rev/ref coming from the client before passing it as an
/// argument to `git`. Conservative charset: what git accepts in ref names
/// plus the usual revision suffixes (`^`, `~`, `@{...}`).
pub fn validate_rev(rev: &str) -> Result<()> {
    if rev.is_empty() || rev.len() > MAX_REV_LEN {
        bail!("empty or too-long rev");
    }
    if rev.starts_with('-') {
        bail!("invalid rev (can't start with '-'): {rev}");
    }
    if rev.contains("..") {
        bail!("invalid rev (ranges not supported): {rev}");
    }
    let ok = rev
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '^' | '~' | '@' | '{' | '}'));
    if !ok {
        bail!("invalid rev (disallowed characters): {rev}");
    }
    Ok(())
}

/// Validates a full object sha (sha1 or sha256).
pub fn validate_sha(sha: &str) -> Result<()> {
    if !matches!(sha.len(), 40 | 64) || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid sha: {sha}");
    }
    Ok(())
}

/// Validates a Git LFS object id: sha256 hex, always 64 chars — narrower
/// than [`validate_sha`] (which also accepts a 40-char sha1, meaningless for
/// LFS oids) — before it's used as an S3 key segment.
pub fn validate_lfs_oid(oid: &str) -> Result<()> {
    validate_sha(oid)?;
    if oid.len() != 64 {
        bail!("invalid LFS oid (expected sha256): {oid}");
    }
    Ok(())
}

const MAX_ASSET_FILENAME_LEN: usize = 255;

/// Validates an uploaded release asset's client-supplied filename before it
/// becomes an S3 key segment and (at download time) a `Content-Disposition`
/// value. Unlike [`sanitize_path`], `/` is never treated as a separator
/// here — an asset name is always exactly one flat segment — so `/`/`\` are
/// rejected outright rather than split. Control chars (`\r`/`\n` in
/// particular, and `"`) are rejected here, at upload time, so building
/// `Content-Disposition: attachment; filename="<name>"` later can never
/// fail on a bad byte smuggled in earlier.
pub fn validate_asset_filename(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_ASSET_FILENAME_LEN {
        bail!("empty or too-long filename (max {MAX_ASSET_FILENAME_LEN} bytes)");
    }
    if matches!(name, "." | "..") {
        bail!("invalid filename: {name}");
    }
    if name.contains(['/', '\\', '"']) || name.contains(|c: char| c.is_control()) {
        bail!("invalid filename (disallowed characters): {name}");
    }
    Ok(name.to_string())
}

const MAX_ISSUE_TITLE_LEN: usize = 256;

/// Validates an issue's title before it's stored — trimmed, non-empty,
/// length-capped, control characters rejected (same reasoning as
/// [`validate_asset_filename`]: a title later renders straight into HTML/
/// page-title text, so a stray `\r`/`\n` shouldn't be smuggled in even
/// though `maud`'s own escaping already handles the XSS side of it).
pub fn validate_issue_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.len() > MAX_ISSUE_TITLE_LEN {
        bail!("empty or too-long issue title (max {MAX_ISSUE_TITLE_LEN} bytes)");
    }
    if title.contains(|c: char| c.is_control()) {
        bail!("invalid issue title (control characters)");
    }
    Ok(title.to_string())
}

const MAX_REPO_DISPLAY_NAME_LEN: usize = 256;

/// Validates a repo's free-text `display_name` before it's stored — unlike
/// [`sanitize_path`], no charset restriction: this is the cosmetic name
/// shown as the repo's page heading, not the URL (`domain::repo::slugify_repo_name`
/// derives that separately). Same shape as [`validate_issue_title`]: trimmed,
/// non-empty, length-capped, control characters rejected (it still renders
/// straight into HTML/page-title text).
pub fn validate_repo_display_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_REPO_DISPLAY_NAME_LEN {
        bail!("empty or too-long name (max {MAX_REPO_DISPLAY_NAME_LEN} bytes)");
    }
    if name.contains(|c: char| c.is_control()) {
        bail!("invalid name (control characters)");
    }
    Ok(name.to_string())
}

/// Validates an issue-label color: `#` followed by exactly 6 hex digits.
/// Unlike `topics`' silent-normalize-or-drop leniency, this hard-rejects a
/// malformed value instead — a broken color isn't cosmetic, it breaks the
/// label pill's `style="background:..."` rendering outright.
pub fn validate_hex_color(color: &str) -> Result<()> {
    let ok = color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        bail!("invalid color (expected #rrggbb): {color}");
    }
    Ok(())
}

/// Validates a path inside the repo tree. Empty = root.
pub fn validate_tree_path(path: &str) -> Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if path.len() > MAX_PATH_LEN {
        bail!("path too long (max {MAX_PATH_LEN} bytes)");
    }
    if path.starts_with('-') || path.starts_with('/') {
        bail!("invalid path: {path}");
    }
    if path.split('/').any(|seg| seg == ".." || seg.is_empty()) {
        bail!("invalid path (empty segment or '..'): {path}");
    }
    if path.contains(|c: char| c.is_control()) {
        bail!("invalid path (control characters)");
    }
    Ok(())
}

const MAX_EMAIL_LEN: usize = 254; // RFC 5321's own limit on a full address.

/// Validates a self-service secondary email
/// ([`crate::auth::entities::User::secondary_emails`]) before it becomes an
/// S3 index key segment. Deliberately not full RFC 5322 — just shape-safe
/// (one `@`, something on both sides, no control/whitespace chars) and
/// length-capped, same "conservative charset over a real parser" call this
/// module's other validators make. Case isn't normalized here: two entries
/// differing only by case claim two different index slots, same tolerance
/// `validate_mirror_label` has for its own free-text field — a commit
/// author email either matches byte-for-byte or it doesn't match at all
/// regardless.
pub fn validate_secondary_email(email: &str) -> Result<String> {
    let email = email.trim();
    if email.is_empty() || email.len() > MAX_EMAIL_LEN {
        bail!("empty or too-long email (max {MAX_EMAIL_LEN} bytes)");
    }
    if email.contains(|c: char| c.is_control() || c.is_whitespace()) {
        bail!("invalid email (control or whitespace characters)");
    }
    let Some((local, domain)) = email.split_once('@') else {
        bail!("invalid email (missing '@'): {email}");
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') || !domain.contains('.') {
        bail!("invalid email: {email}");
    }
    Ok(email.to_string())
}

/// Sniffs an uploaded avatar image's real format from its magic bytes —
/// never the client-supplied multipart `Content-Type` header, which is
/// attacker-controlled: trusting it would let something like an SVG (which
/// can embed `<script>`) slip through mislabeled as `image/png` and later
/// get served back with that same trusted-but-wrong header. Only the three
/// formats a browser avatar needs are accepted; returns the canonical MIME
/// type to store (`User::avatar_content_type`) and later serve back
/// verbatim.
pub fn sniff_avatar_content_type(bytes: &[u8]) -> Result<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Ok("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    bail!("unsupported image format (only PNG/JPEG/WebP avatars are accepted)");
}

/// Short sha for display in the UI, without cutting a char in half.
pub fn short_sha(sha: &str) -> &str {
    // `get`, not a plain byte slice: every caller passes hex today, but a
    // byte index that lands mid-character panics, and a display helper is
    // the last place that should be able to take a request down. Falls back
    // to the whole string when 12 isn't a character boundary.
    sha.get(..sha.len().min(12)).unwrap_or(sha)
}

/// Validates a log search term before it's passed straight into
/// `--grep=<query>`. Fused into a single `--grep=...` token by the caller, so
/// a leading `-` can't smuggle in a separate flag either way — this just
/// keeps the same conservative charset as the rest of this module and turns
/// a bad query into a clean 400 instead of a confusing `git log` error.
pub fn validate_query(q: &str) -> Result<()> {
    if q.is_empty() || q.len() > MAX_REV_LEN {
        bail!("empty or too-long search query");
    }
    if q.starts_with('-') {
        bail!("invalid search query (can't start with '-'): {q}");
    }
    if q.contains(|c: char| c.is_control()) {
        bail!("invalid search query (control characters)");
    }
    Ok(())
}
