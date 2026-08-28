//! `gitquarry repack <repo>`: operator command that consolidates a repo's
//! accumulated small packs (one per push — see the "no remote repack"
//! limitation in the README) into a single new one. Downloads every pack
//! into a throwaway local mirror, runs `git repack -a -d` there
//! ([`gitquarry_core::git::repack`]), uploads whatever new pack(s) that produced,
//! confirms they're on the remote, then deletes the old ones — same
//! "confirm before delete" discipline as `migrate-repos-prefix` and
//! `webui::repo_settings`'s move/delete. Never touches `refs.json`: only the
//! pack storage underneath the same refs changes, so there's nothing here
//! that could make a client see different content, just fewer/bigger files
//! backing it.
//!
//! Not automatic, and not safe to run against a repo mid-push from another
//! process: it only reads packs and refs at the start, so a push that lands
//! after that read (and before the old packs get deleted) has its own new
//! pack survive untouched (not deleted, since it wasn't in the "old" set)
//! but isn't included in the new consolidated one either — not corruption,
//! just a smaller win than a repack run when the repo is quiet.

use anyhow::{Context, Result, bail};
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::remote::Remote;
use gitquarry_core::{config, git, storage, validate};
use std::sync::Arc;
use tracing::info;

const USAGE: &str = "\
usage:
  gitquarry repack <repo> [--bucket NAME] [--apply]

Consolidates a repo's accumulated packs (one per push) into a single new
one. Without --apply it only reports how many packs there are and how
much they weigh, without touching anything.

Downloads every pack to a temporary local mirror, runs 'git repack -a
-d' there, uploads the new pack(s), confirms they landed, and only then
deletes the old ones. Doesn't touch refs.json -- the served content
doesn't change, only how it's packed underneath.

Don't run this against a repo with pushes in progress from another
process: a push that lands during the repack isn't lost (its new pack
isn't deleted, it wasn't in the 'old' set), but it also isn't included in
the consolidated pack -- it resolves itself on the next repack.";

pub async fn run(rest: &[String]) -> Result<()> {
    let mut bucket_arg: Option<String> = None;
    let mut apply = false;
    let mut repo_arg: Option<String> = None;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => bucket_arg = Some(it.next().cloned().context("--bucket needs a value")?),
            "--apply" => apply = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if !other.starts_with('-') && repo_arg.is_none() => repo_arg = Some(other.to_string()),
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    let repo = repo_arg.context(format!("missing repo\n\n{USAGE}"))?;
    let repo = validate::sanitize_path(&repo)?;

    let config_path = config::Config::resolve_path()?;
    let conf =
        config::Config::load_from(&config_path).with_context(|| format!("couldn't read {}", config_path.display()))?;
    let bucket = match bucket_arg {
        Some(b) => b,
        None => conf
            .default_bucket_name()
            .map(str::to_string)
            .context("--bucket NAME required (config has more than one bucket and no default_bucket)")?,
    };
    let bucket_cfg = conf.bucket(&bucket)?.clone();
    let backend = storage::build_backend(&bucket_cfg)?;
    let cipher = Arc::new(
        gitquarry_core::crypto::bootstrap::get_or_create_cipher(backend.as_ref(), &bucket_cfg.passphrase).await?,
    );
    let repo_uid = storage::settings::resolve_repo_name(backend.as_ref(), &cipher, &repo)
        .await?
        .with_context(|| format!("repo '{repo}' doesn't exist in bucket '{bucket}'"))?;
    let remote = Remote::new(
        Arc::clone(&backend),
        format!("{}/{repo_uid}", storage::REPOS_PREFIX),
        Arc::clone(&cipher),
    );

    let old_packs = remote.list_pack_names().await?;
    let (pack_count, total_bytes) = pack_stats(backend.as_ref(), &repo_uid).await?;
    if old_packs.len() <= 1 {
        info!("gitquarry: '{repo}' already has {pack_count} pack(s), nothing to consolidate.");
        return Ok(());
    }
    info!(
        "gitquarry: '{repo}' has {pack_count} packs ({} total).",
        human_bytes(total_bytes)
    );
    if !apply {
        info!("(dry-run -- run again with --apply to consolidate)");
        return Ok(());
    }

    let tmp = tempfile::tempdir().context("creating temp directory")?;
    let mirror_path = tmp.path();
    git::init_bare(mirror_path).await?;
    gitquarry_core::mirror::sync_bare_repo_from_s3(&remote, mirror_path).await?;
    info!("gitquarry: running git repack...");
    git::repack(mirror_path).await?;

    let new_packs = git::local_pack_names(mirror_path).await?;
    let dest_dir = git::objects_pack_dir(mirror_path);
    for base_name in &new_packs {
        if old_packs.contains(base_name) {
            continue;
        }
        remote
            .upload_pack(
                base_name,
                &dest_dir.join(format!("{base_name}.pack")),
                &dest_dir.join(format!("{base_name}.idx")),
            )
            .await?;
    }

    // Confirm every new pack actually landed before deleting anything old —
    // same "verify before delete" care as `migrate-repos-prefix`.
    let uploaded = remote.list_pack_names().await?;
    for base_name in &new_packs {
        if !uploaded.contains(base_name) {
            bail!(
                "new pack '{base_name}' doesn't appear in the bucket after uploading -- nothing old was deleted, retry"
            );
        }
    }

    let mut deleted = 0usize;
    for base_name in &old_packs {
        if new_packs.contains(base_name) {
            continue;
        }
        remote.delete_pack(base_name).await?;
        deleted += 1;
    }

    let (new_count, new_bytes) = pack_stats(backend.as_ref(), &repo_uid).await?;
    info!(
        "gitquarry: done -- {pack_count} packs ({}) -> {new_count} packs ({}), {deleted} old one(s) deleted.",
        human_bytes(total_bytes),
        human_bytes(new_bytes),
    );

    // Corrects any drift in the best-effort `pack_bytes` counter
    // (`mirror::hook` only ever adds to it) with the real, just-measured
    // total — see `RepoConfig::pack_bytes`'s own doc comment.
    let (mut config, etag) = storage::settings::get_repo_config(backend.as_ref(), repo_uid.as_str(), &cipher).await?;
    if config.pack_bytes != new_bytes {
        config.pack_bytes = new_bytes;
        storage::settings::put_repo_config(backend.as_ref(), repo_uid.as_str(), &config, etag.as_deref(), &cipher)
            .await?;
    }
    Ok(())
}

/// Count and total `.pack` bytes under a repo's `objects/pack/` — cheap,
/// straight from the backend's own per-object size, no per-pack extra
/// round trip.
async fn pack_stats(backend: &dyn Backend, repo_uid: &storage::uid::Uid) -> Result<(usize, u64)> {
    let prefix = format!("{}/{repo_uid}/objects/pack/", storage::REPOS_PREFIX);
    let objects = backend.list_prefix(&prefix).await?;
    let packs = objects.iter().filter(|(k, _)| k.ends_with(".pack"));
    let count = packs.clone().count();
    let total = packs.map(|(_, size)| *size).sum();
    Ok((count, total))
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}
