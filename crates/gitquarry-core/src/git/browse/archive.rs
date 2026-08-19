//! Downloadable snapshots: `git archive` plus in-process repacking to zip.

use crate::git::run_capped;
use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::Path;

/// `reponame-rev.fmt` — e.g. `proj-v1.0.tar.gz` for repo `team/proj` at
/// `v1.0`. Shared by every place that hands one of this module's archives to
/// a downloader: `webui::export::snapshot`'s own `Content-Disposition` header,
/// `domain::release::generate_source_archives`'s uploaded filenames, and
/// `webui::repo::releases`'s label-matching against those same filenames — so
/// what's displayed always matches what actually lands on disk.
pub fn archive_filename(repo_name: &str, rev: &str, fmt: &str) -> String {
    let base = repo_name.rsplit('/').next().unwrap_or(repo_name);
    // Branch/tag names can contain `/` (`feature/x`); keep the archive
    // filename to one path segment.
    let rev_for_name: String = rev
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{base}-{rev_for_name}.{fmt}")
}

/// Ceiling on how much `git archive` output one call will buffer.
///
/// Unlike every other `git` call in this module, an archive's size scales
/// with the repo rather than with anything the caller already checked, and
/// `webui::export::snapshot` reaches this anonymously with no rate limit in
/// front of it — so "however big the tree is" was also "however much memory
/// one request may claim". Generous enough that no plausible source tree
/// trips it (a snapshot is source, not history or LFS payloads), low enough
/// that hitting it is an error rather than an outage.
pub const MAX_ARCHIVE_BYTES: u64 = 512 << 20;

/// `git archive` output for `rev` in `fmt` (must be `"tar"` or `"zip"`, both
/// natively supported by git — no external compressor needed), capped at
/// [`MAX_ARCHIVE_BYTES`].
pub async fn archive(git_dir: &Path, rev: &str, fmt: &str) -> Result<Vec<u8>> {
    let format_flag = format!("--format={fmt}");
    run_capped(
        Some(git_dir),
        &[
            OsStr::new("archive"),
            OsStr::new(&format_flag),
            OsStr::new("--end-of-options"),
            OsStr::new(rev),
        ],
        MAX_ARCHIVE_BYTES,
    )
    .await
}

/// One archive for `rev` in `fmt` (`"tar.gz"` or `"zip"`) — the single
/// `git archive --format=tar` decode this module always does, then exactly
/// the compression `fmt` asked for and nothing else. Used wherever only one
/// format is wanted at a time (`webui::export::snapshot`'s on-demand
/// download); [`archive_pair`] is the sibling for when both are wanted
/// together (a release's generated assets), so a single-format request here
/// never pays for repackaging a format nobody asked for.
///
/// Shares its actual compression code with [`archive_pair`] (see
/// [`gzip_bytes`]/[`zip_from_tar`]) rather than each keeping its own copy, so
/// a normal branch's download button and a release's generated archives are
/// byte-for-byte the same mechanism, not two implementations that could
/// quietly drift apart.
pub async fn archive_one(git_dir: &Path, rev: &str, fmt: &str) -> Result<Vec<u8>> {
    let tar_bytes = archive(git_dir, rev, "tar").await?;
    let fmt = fmt.to_string();
    tokio::task::spawn_blocking(move || match fmt.as_str() {
        "tar.gz" => gzip_bytes(&tar_bytes),
        "zip" => zip_from_tar(&tar_bytes),
        other => bail!("unsupported archive format: {other}"),
    })
    .await
    .context("archive repacking task panicked")?
}

/// Both `.tar.gz` and `.zip` for `rev`, decoding the tree out of git exactly
/// once — `git archive` (this crate's [`archive`]) has to walk the whole
/// tree and decompress every blob out of the pack format, and doing that
/// twice (once per format, the old approach) doubles the one part of
/// generating a release's source archives that's actually expensive. Here
/// the tar entries `git` already decoded are re-emitted as a zip in-process
/// instead of asking git to decode the same tree over again in a second
/// `--format=zip` run: same content, half the git-side work.
///
/// The re-emit itself (gzip + zip-repack) runs on a blocking-pool thread via
/// [`tokio::task::spawn_blocking`] — it's synchronous CPU work (the `zip`/
/// `flate2` crates have no async API), so it shouldn't run directly on an
/// async worker thread alongside request handling.
pub async fn archive_pair(git_dir: &Path, rev: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let tar_bytes = archive(git_dir, rev, "tar").await?;
    tokio::task::spawn_blocking(move || Ok((gzip_bytes(&tar_bytes)?, zip_from_tar(&tar_bytes)?)))
        .await
        .context("archive repacking task panicked")?
}

/// `tar_bytes` is a full, already-decoded `git archive --format=tar` output —
/// gzip-compressing it needs no reparsing, just a straight compress pass.
fn gzip_bytes(tar_bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(tar_bytes)
        .context("gzip-compressing the tar archive")?;
    encoder.finish().context("finishing the gzip stream")
}

/// Walks an already-decoded `git archive --format=tar` once and rebuilds the
/// same entries as a zip, instead of asking git to decode the same tree over
/// again via `--format=zip` — unix permissions and symlinks carried over so
/// the zip behaves the same way `git archive --format=zip` itself would.
fn zip_from_tar(tar_bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Cursor;

    let mut zip_buf = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut zip_buf);
        let mut tar = tar::Archive::new(tar_bytes);
        for entry in tar.entries().context("reading tar entries")? {
            let mut entry = entry.context("reading a tar entry")?;
            let name = entry
                .path()
                .context("reading a tar entry's path")?
                .to_string_lossy()
                .into_owned();
            let mode = entry.header().mode().unwrap_or(0o644);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default().unix_permissions(mode);
            match entry.header().entry_type() {
                tar::EntryType::Directory => {
                    writer
                        .add_directory(name, options)
                        .context("adding a zip directory entry")?;
                }
                tar::EntryType::Symlink => {
                    let target = entry
                        .link_name()
                        .context("reading a symlink entry's target")?
                        .context("symlink entry with no recorded target")?
                        .to_string_lossy()
                        .into_owned();
                    writer
                        .add_symlink(name, target, options.unix_permissions(0o120777))
                        .context("adding a zip symlink entry")?;
                }
                _ => {
                    writer.start_file(name, options).context("starting a zip file entry")?;
                    std::io::copy(&mut entry, &mut writer).context("writing a zip file entry's contents")?;
                }
            }
        }
        writer.finish().context("finishing the zip archive")?;
    }
    Ok(zip_buf.into_inner())
}
