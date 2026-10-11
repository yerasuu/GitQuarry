//! Views that return something other than an HTML page: raw blob bytes,
//! a mailable patch, a raw diff, snapshot archives, and the atom feed.

use super::{AppError, esc, repo_link};
use anyhow::Context;
use axum::body::{Body, Bytes};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::stream;
use gitquarry_core::storage::remote::Remote;
use gitquarry_core::{git, validate};
use std::path::Path;
use tokio::io::AsyncReadExt;

/// Cap for raw blob downloads. Bigger than `repo::MAX_BLOB_BYTES`:
/// unlike the HTML blob view, this doesn't get HTML-escaped or highlighted,
/// so there's no multiplier on the in-memory size — it's just capped so a
/// single request can't buffer an unbounded amount of memory.
const PLAIN_MAX_BYTES: u64 = 10 << 20;

pub fn guess_content_type(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "txt" | "md" | "markdown" | "rs" | "toml" | "yaml" | "yml" | "json" | "js" | "ts" | "css" | "html" | "htm"
        | "sh" | "py" | "go" | "c" | "h" | "cpp" | "hpp" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub fn is_image(path: &str) -> bool {
    guess_content_type(path).starts_with("image/")
}

fn respond(content_type: &str, body: Vec<u8>, disposition: Option<&str>) -> Response {
    let mut builder = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(d) = disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, d);
    }
    builder
        .body(Body::from(body))
        .expect("response with a fixed set of valid headers")
        .into_response()
}

/// Content types a `plain()` response must hand over as a download rather
/// than let the browser render in this origin. Both keep their real MIME
/// type from [`guess_content_type`] (unlike html/htm, downgraded to
/// text/plain there) because both have a legitimate inline use the type is
/// needed for — an `<img src>` for SVG, a PDF viewer embed — and both can
/// carry script:
/// - SVG is a full document format; `<script>` inside one runs.
/// - PDF can carry JavaScript, and the plugin/viewer that runs it is
///   largely outside what this app's CSP can reach, so the header is the
///   control that actually applies.
///
/// Forcing the download only changes what happens when someone opens the
/// raw URL *directly*; `<img src=...>`/`<embed>` still render normally,
/// since `Content-Disposition` doesn't apply to a subresource load.
const FORCE_DOWNLOAD_TYPES: &[&str] = &["image/svg+xml", "application/pdf"];

/// One `Content-Disposition: attachment` value, with the filename made
/// safe to sit inside its quoted-string.
///
/// A repo name only goes through `validate::sanitize_path`, which rejects
/// control characters and `..` but not a `"` — so a repo called `a"b` used
/// to end the quoted string early and let the rest of the name read as
/// header parameters. Quotes and backslashes become `'`, and anything a
/// header value can't carry at all is dropped rather than left to make the
/// whole response fail to build.
pub fn attachment(filename: &str) -> String {
    let safe: String = filename
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '"' || c == '\\' { '\'' } else { c })
        .collect();
    let safe = if safe.trim().is_empty() {
        "file".to_string()
    } else {
        safe
    };
    format!("attachment; filename=\"{safe}\"")
}

/// `Content-Disposition` to force on a `plain()` response, if any — see
/// [`FORCE_DOWNLOAD_TYPES`].
pub fn raw_disposition(path: &str, content_type: &str) -> Option<String> {
    FORCE_DOWNLOAD_TYPES.contains(&content_type).then(|| {
        let filename = Path::new(path).file_name().and_then(|f| f.to_str()).unwrap_or("file");
        attachment(filename)
    })
}

pub(crate) async fn plain(
    bare_repo: &Path,
    rev: &str,
    path: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Response, AppError> {
    let size = git::browse::blob_size(bare_repo, rev, path).await?;
    if size > PLAIN_MAX_BYTES {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::release::file_too_large(locale, size, PLAIN_MAX_BYTES)
        )));
    }
    let bytes = git::browse::read_blob(bare_repo, rev, path).await?;
    let content_type = guess_content_type(path);
    let disposition = raw_disposition(path, content_type);
    Ok(respond(content_type, bytes, disposition.as_deref()))
}

pub(crate) async fn patch(bare_repo: &Path, sha: &str) -> Result<Response, AppError> {
    let text = git::browse::format_patch(bare_repo, sha).await?;
    let filename = format!("{}.patch", validate::short_sha(sha));
    Ok(respond(
        "text/plain; charset=utf-8",
        text.into_bytes(),
        Some(&attachment(&filename)),
    ))
}

pub(crate) async fn rawdiff(bare_repo: &Path, sha: &str) -> Result<Response, AppError> {
    let (_info, diff) = git::browse::show_commit(bare_repo, sha).await?;
    Ok(respond("text/plain; charset=utf-8", diff.into_bytes(), None))
}

pub(crate) async fn snapshot(
    bare_repo: &Path,
    repo_name: &str,
    rev: &str,
    fmt: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Response, AppError> {
    // Same `git::browse::archive_one` a release's generated assets go
    // through (`domain::release::generate_source_archives`'s sibling call is
    // `archive_pair`, for when both formats are wanted at once) — one
    // in-process mechanism either way, not two that could drift apart.
    let content_type = match fmt {
        "tar.gz" => "application/gzip",
        "zip" => "application/zip",
        other => {
            return Err(AppError::bad_request(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::release::invalid_snapshot_format(locale, other)
            )));
        }
    };
    let bytes = git::browse::archive_one(bare_repo, rev, fmt).await?;
    let filename = git::browse::archive_filename(repo_name, rev, fmt);
    Ok(respond(content_type, bytes, Some(&attachment(&filename))))
}

const ATOM_ENTRY_LIMIT: usize = 20;

pub(crate) async fn atom(
    bare_repo: &Path,
    repo_name: &str,
    display_name: Option<&str>,
    rev: &str,
) -> Result<Response, AppError> {
    let commits = git::browse::log(bare_repo, rev, ATOM_ENTRY_LIMIT, None).await?;
    let updated = commits.first().map_or("1970-01-01T00:00:00Z", |c| c.date.as_str());
    let log_link = esc(&repo_link(repo_name, "log", "").to_string());

    let mut xml = String::from(r#"<?xml version="1.0" encoding="utf-8"?>"#);
    xml.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">");
    // Title is the repo's public name; the `<id>` stays keyed on the slug —
    // a feed id has to be stable across a repo being renamed for display.
    xml.push_str(&format!(
        "<title>{}</title>",
        esc(&crate::webui::repo_label(repo_name, display_name))
    ));
    xml.push_str(&format!("<id>urn:gitquarry:{}</id>", esc(repo_name)));
    xml.push_str(&format!("<updated>{}</updated>", esc(updated)));
    xml.push_str(&format!("<link rel=\"alternate\" href=\"{log_link}\"/>"));
    for c in &commits {
        let commit_link = esc(&repo_link(repo_name, "commit", "").sha(&c.sha).to_string());
        xml.push_str("<entry>");
        xml.push_str(&format!("<title>{}</title>", esc(&c.subject)));
        xml.push_str(&format!("<id>urn:gitquarry:{}:{}</id>", esc(repo_name), esc(&c.sha)));
        xml.push_str(&format!("<updated>{}</updated>", esc(&c.date)));
        xml.push_str(&format!(
            "<author><name>{}</name><email>{}</email></author>",
            esc(&c.author_name),
            esc(&c.author_email)
        ));
        xml.push_str(&format!("<link rel=\"alternate\" href=\"{commit_link}\"/>"));
        xml.push_str("</entry>");
    }
    xml.push_str("</feed>");
    Ok(respond("application/atom+xml; charset=utf-8", xml.into_bytes(), None))
}

/// Streams a release asset back — the one export view that can't reuse
/// [`respond`]'s fully-buffered `Vec<u8>` (fine for the small, inherently
/// capped exports above; wrong for an arbitrary release binary). Downloads
/// (decrypts) to a scratch tempfile via [`Remote::download_asset`], then
/// streams that plaintext file back in fixed-size chunks — `Content-Length`
/// comes from measuring the already-decrypted file, not a separate lookup
/// (the backend only knows the ciphertext's length, which includes
/// encryption overhead).
pub(crate) async fn release_asset(
    remote: &Remote,
    tag: &str,
    filename: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Response, AppError> {
    let filename = validate::validate_asset_filename(filename).map_err(AppError::bad_request)?;
    let (release, etag) = remote.get_release(tag).await?;
    let asset = release.assets.iter().find(|a| a.filename == filename);
    let Some(asset) = asset.filter(|_| etag.is_some()) else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::release::no_asset_on_release(locale, &filename, tag)
        )));
    };

    let tmp_dir = tempfile::tempdir().context(gitquarry_i18n::shared::release::creating_temp_dir_for_asset(locale))?;
    let tmp_path = tmp_dir.path().join(&filename);
    remote.download_asset(tag, asset.object_name(), &tmp_path).await?;

    let len = tokio::fs::metadata(&tmp_path).await?.len();
    let file = tokio::fs::File::open(&tmp_path).await?;

    const CHUNK_LEN: usize = 64 * 1024;
    let body_stream = stream::try_unfold(
        (file, tmp_dir, vec![0u8; CHUNK_LEN]),
        |(mut f, guard, mut buf)| async move {
            let n = f.read(&mut buf).await?;
            if n == 0 {
                drop(guard); // deletes the scratch dir only once fully read
                return Ok::<_, std::io::Error>(None);
            }
            Ok(Some((Bytes::copy_from_slice(&buf[..n]), (f, guard, buf))))
        },
    );

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, guess_content_type(&filename).to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CONTENT_DISPOSITION, attachment(&filename)),
        ],
        Body::from_stream(body_stream),
    )
        .into_response())
}
