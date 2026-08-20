//! Every mutation of a [`gitquarry_core::storage::remote::ReleaseMeta`] goes through
//! here — same `domain`/`web` boundary [`gitquarry_core::domain::repo`]'s doc comment
//! describes: transport-agnostic, `anyhow::Result`, no CSRF/session/audit
//! concerns, those stay in `webui::repo::releases`.
//!
//! A release always wraps an *existing* tag (never an arbitrary rev) and
//! there's at most one release per tag — enforced by
//! [`gitquarry_core::storage::remote::Remote::put_release`]'s CAS (`expected_etag =
//! None` means "must not exist yet"), not a separate check here.

use anyhow::{Context, Result, bail};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::git;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::audit::format_unix_secs;
use gitquarry_core::storage::remote::{AssetMeta, ReleaseMeta};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Lowercase hex — same tiny local helper `git::lfs` has, not worth a shared
/// utility module for four lines.
fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_secs(secs)
}

async fn tag_exists(resolved: &ResolvedRepo, tag: &str) -> Result<bool> {
    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    Ok(manifest.refs.contains_key(&format!("refs/tags/{tag}")))
}

/// Caps how many commits a from-scratch generation (no previous tag to
/// diff against — the repo's very first release) walks: unlike a normal
/// `prev..tag` range, "every commit ever, no bound" could be the entire
/// history of a long-lived repo, and this is meant as a rough draft the
/// author edits before publishing, not an exhaustive changelog.
const FIRST_RELEASE_COMMIT_CAP: usize = 200;

/// A markdown bullet list of commit subjects since whichever tag was
/// created right before `tag` (by [`git::browse::list_refs_detailed`]'s own
/// `--sort=-creatordate`, same ordering the releases list already sorts
/// by) — a rough draft meant to be edited before publishing, not a
/// curated changelog. `Err` if `tag` isn't an actual tag on this repo.
/// This walks raw commits rather than merged MRs: a `Merge`/`Squash`
/// [`gitquarry_core::domain::merge_request::merge`] leaves the MR number right in the
/// merge commit's own subject line (`"Merge merge request #{number} from
/// ..."` / `"{title} (#{number})"`), so it shows up here for free without
/// this needing to cross-reference MR metadata at all — a `Rebase` merge
/// doesn't leave that marker, same asymmetry the git history itself has.
pub async fn generate_notes(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    tag: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<String> {
    if !tag_exists(resolved, tag).await? {
        bail!("{}", gitquarry_i18n::shared::not_a_tag(locale, tag));
    }
    let mirror_path = ctx.open_mirror(resolved).await?.path.clone();
    let tags = git::browse::list_refs_detailed(&mirror_path, "refs/tags/").await?;
    let this_index = tags.iter().position(|r| r.name == format!("refs/tags/{tag}"));
    let previous_tag = this_index
        .and_then(|i| tags.get(i + 1))
        .map(|r| r.name.strip_prefix("refs/tags/").unwrap_or(r.name.as_str()).to_string());

    let commits = match &previous_tag {
        Some(prev) => git::browse::log(&mirror_path, &format!("{prev}..{tag}"), usize::MAX, None).await?,
        None => git::browse::log(&mirror_path, tag, FIRST_RELEASE_COMMIT_CAP, None).await?,
    };

    if commits.is_empty() {
        return Ok(gitquarry_i18n::shared::no_commits_since_previous_tag(locale));
    }
    let mut notes = String::new();
    for commit in &commits {
        notes.push_str(&format!(
            "- {} ({})\n",
            commit.subject,
            &commit.sha[..7.min(commit.sha.len())]
        ));
    }
    Ok(notes)
}

/// `Err` if `tag` isn't an actual tag on this repo, or a release already
/// exists for it.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    tag: &str,
    title: &str,
    notes: &str,
    is_prerelease: bool,
    is_draft: bool,
    created_by: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    if !tag_exists(resolved, tag).await? {
        bail!("{}", gitquarry_i18n::shared::not_a_tag(locale, tag));
    }
    let release = ReleaseMeta {
        tag: tag.to_string(),
        title: title.to_string(),
        notes: notes.to_string(),
        assets: Vec::new(),
        created_at: now_stamp(),
        created_by: created_by.to_string(),
        is_prerelease,
        is_draft,
    };
    let cas = resolved.remote.put_release(&release, None).await?;
    if cas == Cas::Ok && !is_draft {
        gitquarry_core::events::emit(
            ctx,
            gitquarry_core::events::RepoEvent::Release {
                repo_uid: resolved.uid.clone(),
                tag: tag.to_string(),
            },
        );
    }
    Ok(cas)
}

/// `Err` if no release exists for `tag` yet.
#[allow(clippy::too_many_arguments)]
pub async fn edit(
    resolved: &ResolvedRepo,
    tag: &str,
    title: &str,
    notes: &str,
    is_prerelease: bool,
    is_draft: bool,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut release, etag) = resolved.remote.get_release(tag).await?;
    if etag.is_none() {
        bail!("{}", gitquarry_i18n::shared::release_not_found(locale, tag));
    }
    release.title = title.to_string();
    release.notes = notes.to_string();
    release.is_prerelease = is_prerelease;
    release.is_draft = is_draft;
    resolved.remote.put_release(&release, etag.as_deref()).await
}

/// Best-effort, irreversible — same convention as [`gitquarry_core::domain::repo::delete`].
pub async fn delete(resolved: &ResolvedRepo, tag: &str) -> Result<()> {
    resolved.remote.delete_release(tag).await
}

/// Uploads `source`'s bytes (streaming, via [`gitquarry_core::storage::remote::Remote::upload_asset`])
/// then registers the metadata via CAS. `Err` if no release exists for
/// `tag`, or an asset with this filename is already attached — forces an
/// explicit [`remove_asset`]-then-re-upload to replace one, so there's never
/// an ambiguous "did this overwrite or orphan the old bytes" question.
///
/// Accepted risk: if the upload succeeds but the metadata CAS then conflicts
/// (a concurrent edit of the same release), the just-uploaded blob becomes
/// unreferenced — same class of risk `upload_pack`-then-manifest-CAS already
/// accepts elsewhere in this codebase. The caller surfaces a CAS conflict as
/// "reload and retry" like every other form here; the orphaned blob is
/// harmless (never listed, never served).
/// Bundled so [`add_asset`] doesn't cross clippy's `too_many_arguments` --
/// adding `locale` (needed for the "no existe"/"ya hay un asset" checks)
/// was the 8th positional param.
pub struct NewAsset<'a> {
    pub tag: &'a str,
    pub filename: &'a str,
    pub source: &'a Path,
    pub size: u64,
    pub sha256: &'a str,
    pub uploaded_by: &'a str,
    /// Ascii-armored detached OpenPGP signature over `source`'s bytes, if
    /// the uploader attached one — see [`gitquarry_core::storage::remote::AssetMeta::signature`]'s
    /// own doc comment for why this is checked once, here, rather than at
    /// render time.
    pub signature: Option<&'a str>,
    pub locale: gitquarry_i18n::Locale,
}

pub async fn add_asset(ctx: &AppContext, resolved: &ResolvedRepo, asset: NewAsset<'_>) -> Result<Cas> {
    let NewAsset {
        tag,
        filename,
        source,
        size,
        sha256,
        uploaded_by,
        signature,
        locale,
    } = asset;
    let (mut release, etag) = resolved.remote.get_release(tag).await?;
    if etag.is_none() {
        bail!("{}", gitquarry_i18n::shared::release_not_found(locale, tag));
    }
    if release.assets.iter().any(|a| a.filename == filename) {
        bail!("{}", gitquarry_i18n::shared::asset_already_exists(locale, filename));
    }
    let signature_verified = match signature {
        Some(sig) => {
            let (uploader, _etag) =
                gitquarry_core::auth::access::get_user(ctx.backend().as_ref(), &ctx.cipher, uploaded_by).await?;
            let keys: Vec<String> = uploader
                .map(|u| u.gpg_keys.into_iter().map(|k| k.armored).collect())
                .unwrap_or_default();
            let data = tokio::fs::read(source)
                .await
                .context("reading asset bytes to verify its signature")?;
            gitquarry_gpg::verify_detached_signature(&data, sig, &keys).await?
        }
        None => false,
    };
    resolved.remote.upload_asset(tag, filename, source).await?;
    release.assets.push(AssetMeta {
        filename: filename.to_string(),
        size,
        sha256: sha256.to_string(),
        uploaded_at: now_stamp(),
        uploaded_by: uploaded_by.to_string(),
        signature: signature.map(str::to_string),
        signature_verified,
    });
    resolved.remote.put_release(&release, etag.as_deref()).await
}

/// Removes the asset from the release's metadata first (CAS); only once
/// that's confirmed does it delete the underlying blob — so a lost CAS race
/// never deletes bytes a concurrently-still-live release version still
/// references (same ordering [`gitquarry_core::storage::remote::Remote::delete_pack`]'s
/// doc comment describes). `Err` if no release exists for `tag`, or it has
/// no asset named `filename`.
pub async fn remove_asset(
    resolved: &ResolvedRepo,
    tag: &str,
    filename: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<Cas> {
    let (mut release, etag) = resolved.remote.get_release(tag).await?;
    if etag.is_none() {
        bail!("{}", gitquarry_i18n::shared::release_not_found(locale, tag));
    }
    let before = release.assets.len();
    release.assets.retain(|a| a.filename != filename);
    if release.assets.len() == before {
        bail!("{}", gitquarry_i18n::shared::asset_not_found(locale, filename));
    }
    let cas = resolved.remote.put_release(&release, etag.as_deref()).await?;
    if cas == Cas::Ok {
        resolved.remote.delete_asset(tag, filename).await;
    }
    Ok(cas)
}

/// The two source archives, generated and uploaded once — every future
/// download serves these exact stored bytes ([`add_asset`] uploads them to
/// S3 like any other asset), never a fresh `git archive` run per request.
/// Meant to be spawned detached from the request that created the release
/// (see `webui::repo::releases::handler_create_post`'s call site), so failures
/// here have no requester left to report to — the caller logs and drops
/// them; the release itself already exists either way, just without these
/// two assets, same as if nobody had ever uploaded them.
///
/// `tar.gz` and `zip` come from one `git archive` decode via
/// [`git::browse::archive_pair`], not two — `git` only ever walks this tree
/// and decompresses its blobs once, with the zip re-derived in-process from
/// what that single decode already produced, instead of paying for a second
/// full tree walk just to get the other format.
///
/// [`AppContext::open_mirror`]'s guard is dropped the instant the sync
/// finishes, *before* archiving starts — every other view of this repo
/// (tree/log/blob/commit/...) takes that same per-repo lock, so holding it
/// for the whole `git archive`+`gzip`/zip-repack run would stall unrelated
/// page loads on this repo for however long that takes, not just this
/// background job. Reading from the mirror after releasing the lock is safe
/// in practice: pushes only ever add packs, never delete them outside an
/// explicit `gitquarry repack`/`fsck` (an operator-run admin command, not
/// something racing an ordinary request), and `open_mirror` just refreshed
/// this mirror's last-access stamp, so `mirror::gc`'s sweep won't consider it
/// an eviction candidate again for a full `mirror_max_age` — comfortably
/// longer than an archive run takes.
pub async fn generate_source_archives(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    repo_name: &str,
    tag: &str,
    uploaded_by: &str,
    locale: gitquarry_i18n::Locale,
) -> Result<()> {
    let mirror_path = ctx.open_mirror(resolved).await?.path.clone();
    let (tar_gz, zip) = git::browse::archive_pair(&mirror_path, tag).await?;

    for (fmt, bytes) in [("tar.gz", tar_gz), ("zip", zip)] {
        let sha256 = encode_hex(&Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        let filename = git::browse::archive_filename(repo_name, tag, fmt);
        let tmp = tempfile::Builder::new()
            .prefix(".gitquarry-release-src-")
            .tempfile()?
            .into_temp_path();
        tokio::fs::write(&tmp, &bytes).await?;
        add_asset(
            ctx,
            resolved,
            NewAsset {
                tag,
                filename: &filename,
                source: &tmp,
                size,
                sha256: &sha256,
                uploaded_by,
                signature: None,
                locale,
            },
        )
        .await?;
    }
    Ok(())
}
