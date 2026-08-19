//! `gitquarry fsck`: operator command that scans every repo under
//! [`gitquarry_core::storage::REPOS_PREFIX`] for objects that don't match what gitquarry
//! itself ever writes for a repo (`refs.json`, `<repo>/.conf/config.json`,
//! `objects/pack/pack-*.{pack,idx}`) — the same class of surprise that
//! broke the first `migrate-repos-prefix --apply` run: a provider's S3
//! console had left zero-byte "folder" placeholder objects (keys ending in
//! `/`) sitting inside a repo's own prefix.
//!
//! Two tiers, on purpose:
//! - a trailing-slash placeholder is unambiguously not real data (gitquarry
//!   never writes one), so `--apply` cleans it up automatically;
//! - anything else unrecognized might be someone's real (if unusual) data —
//!   reported only, never deleted automatically. Deleting real bucket data
//!   without asking isn't this command's call to make.

use anyhow::{Context, Result, bail};
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::remote::MANIFEST_SUFFIX;
use gitquarry_core::{config, storage};

const USAGE: &str = "\
usage:
  gitquarry fsck [--bucket NAME] [--apply]

Scans every repo under repos/ looking for objects that gitquarry doesn't
write: empty-folder placeholders (key ending in '/', the same issue that
broke the first migrate-repos-prefix run) are listed and, with --apply,
deleted -- they're 0 bytes, gitquarry never writes one. Anything else
unexpected is only reported, never auto-deleted: it could be real data
even if unusual, this tool doesn't get to decide to delete it.";

#[cfg(any(test, feature = "test-util"))]
pub enum Finding {
    /// Zero-byte, key ends in `/` — safe to auto-delete with `--apply`.
    EmptyFolderMarker,
    /// Doesn't match any known shape — reported only.
    Unexpected,
}

#[cfg(not(any(test, feature = "test-util")))]
enum Finding {
    /// Zero-byte, key ends in `/` — safe to auto-delete with `--apply`.
    EmptyFolderMarker,
    /// Doesn't match any known shape — reported only.
    Unexpected,
}

pub async fn run(rest: &[String]) -> Result<()> {
    let mut bucket_arg: Option<String> = None;
    let mut apply = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bucket" => bucket_arg = Some(it.next().cloned().context("--bucket needs a value")?),
            "--apply" => apply = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }

    let config_path = config::Config::resolve_path()?;
    let conf =
        config::Config::load_from(&config_path).with_context(|| format!("couldn't read {}", config_path.display()))?;
    let bucket = match bucket_arg {
        Some(b) => b,
        None => conf
            .only_bucket_name()
            .map(str::to_string)
            .context("--bucket NAME required (config has more than one bucket)")?,
    };
    let bucket_cfg = conf.bucket(&bucket)?.clone();
    let backend = storage::build_backend(&bucket_cfg);

    let repos = storage::remote::list_repo_paths(backend.as_ref()).await?;
    let mut markers = Vec::new();
    let mut unexpected = Vec::new();
    for repo in &repos {
        let prefix = format!("{}/{repo}/", storage::REPOS_PREFIX);
        for (key, finding) in scan_repo(backend.as_ref(), &prefix).await? {
            match finding {
                Finding::EmptyFolderMarker => markers.push(key),
                Finding::Unexpected => unexpected.push(key),
            }
        }
    }

    if markers.is_empty() && unexpected.is_empty() {
        println!("gitquarry: fsck clean -- {} repo(s) checked, nothing odd.", repos.len());
        return Ok(());
    }

    if !markers.is_empty() {
        println!("empty-folder placeholders (0 bytes, key ending in '/'):");
        for key in &markers {
            println!("  - {key}");
        }
    }
    if !unexpected.is_empty() {
        println!("unexpected objects (NOT auto-deleted, review by hand):");
        for key in &unexpected {
            println!("  - {key}");
        }
    }

    if !apply {
        println!("\n(dry-run -- run again with --apply to delete the placeholders)");
        return Ok(());
    }
    if markers.is_empty() {
        println!("\nnothing --apply can delete (only unexpected objects, untouched).");
        return Ok(());
    }

    backend.delete_many(&markers).await.context("deleting placeholders")?;
    println!("\ngitquarry: {} placeholder(s) deleted.", markers.len());
    Ok(())
}

/// Lists everything under `prefix` (one repo's own `repos/<repo>/`) and
/// classifies whatever isn't one of the shapes gitquarry itself writes.
async fn scan_repo(backend: &dyn Backend, prefix: &str) -> Result<Vec<(String, Finding)>> {
    Ok(storage::list_all_keys(backend, prefix)
        .await?
        .into_iter()
        .filter_map(|key| {
            let suffix = key.strip_prefix(prefix)?;
            let finding = classify(suffix)?;
            Some((key.clone(), finding))
        })
        .collect())
}

/// `None` for anything gitquarry itself would have written for a repo —
/// `refs.json`, `.conf/config.json`, a pack pair under `objects/pack/`.
#[cfg(any(test, feature = "test-util"))]
pub fn classify(suffix: &str) -> Option<Finding> {
    if suffix.is_empty() || suffix.ends_with('/') {
        return Some(Finding::EmptyFolderMarker);
    }
    let is_config = suffix == MANIFEST_SUFFIX || suffix == ".conf/config.json";
    let is_pack = suffix.starts_with("objects/pack/pack-") && (suffix.ends_with(".pack") || suffix.ends_with(".idx"));
    if is_config || is_pack {
        None
    } else {
        Some(Finding::Unexpected)
    }
}

/// `None` for anything gitquarry itself would have written for a repo —
/// `refs.json`, `.conf/config.json`, a pack pair under `objects/pack/`.
#[cfg(not(any(test, feature = "test-util")))]
fn classify(suffix: &str) -> Option<Finding> {
    if suffix.is_empty() || suffix.ends_with('/') {
        return Some(Finding::EmptyFolderMarker);
    }
    let is_config = suffix == MANIFEST_SUFFIX || suffix == ".conf/config.json";
    let is_pack = suffix.starts_with("objects/pack/pack-") && (suffix.ends_with(".pack") || suffix.ends_with(".idx"));
    if is_config || is_pack {
        None
    } else {
        Some(Finding::Unexpected)
    }
}
