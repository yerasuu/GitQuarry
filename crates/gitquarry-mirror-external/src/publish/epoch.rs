//! `filter_epoch` (`docs/publish-mirror-concept.md` §3, "Epochs"): what
//! changes a history publish's output, and the rule that such a change only
//! ever reaches the public side through a confirmed bump.
//!
//! Two edges enforce it. [`check_update`] refuses to store an output change
//! without the bump; a run compares the [`Fingerprint`] it computes with the
//! one recorded for the epoch on the public side and refuses on a mismatch.
//! The second catches what the first can't see: `.gitquarry/publish.toml`
//! changes with a push to the private repo, not with a settings edit.

use super::filter::{FilterConfig, FilterMode, RepoFilterFile};
use super::log::{self, PublishLog};
use super::{PublishMirror, PublishMode};
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::uid::Uid;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tracing::warn;

/// What a history publish's output depends on, hashed: the entity's half
/// (mode, filter, identity map) and the in-repo filter files' half.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub config: String,
    pub repo_files: String,
}

/// An edit that would change what is published, saved without bumping the
/// epoch. Its text is what the settings page shows next to the
/// confirmation, so it names the consequence.
#[derive(Debug)]
pub struct EpochBumpRequired;

impl std::fmt::Display for EpochBumpRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "this change rewrites the public history: it needs a confirmed epoch bump, after which the next publish \
             is a force push, and everyone who cloned or forked the public repo has a history that no longer matches",
        )
    }
}

impl std::error::Error for EpochBumpRequired {}

/// Length-prefixed, so that no two different inputs hash alike.
fn feed(hasher: &mut Sha1, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn hex(hasher: Sha1) -> String {
    super::objects::to_hex(&hasher.finalize().into())
}

/// The entity's half. The secret allowlist, the SPDX id, the unmapped
/// policy and the published ref patterns are left out: they decide whether
/// a run may go ahead or which refs it pushes, never what a commit becomes.
pub(crate) fn config_fingerprint(mirror: &PublishMirror) -> String {
    let mut hasher = Sha1::new();
    feed(
        &mut hasher,
        match mirror.mode {
            PublishMode::Snapshot => b"snapshot",
            PublishMode::History => b"history",
        },
    );
    let FilterConfig {
        mode,
        include,
        exclude,
        exclude_submodules,
        strip_empty_dirs,
    } = &mirror.filter;
    feed(
        &mut hasher,
        match mode {
            FilterMode::Allowlist => b"allowlist",
            FilterMode::Exclude => b"exclude",
        },
    );
    feed(&mut hasher, &(include.len() as u64).to_be_bytes());
    for pattern in include {
        feed(&mut hasher, pattern.as_bytes());
    }
    feed(&mut hasher, &(exclude.len() as u64).to_be_bytes());
    for pattern in exclude {
        feed(&mut hasher, pattern.as_bytes());
    }
    feed(
        &mut hasher,
        &[u8::from(*exclude_submodules), u8::from(*strip_empty_dirs)],
    );
    feed(&mut hasher, &(mirror.identities.len() as u64).to_be_bytes());
    for identity in &mirror.identities {
        feed(&mut hasher, identity.private.to_ascii_lowercase().as_bytes());
        feed(&mut hasher, identity.public.as_bytes());
        feed(&mut hasher, identity.name.as_deref().unwrap_or("\0").as_bytes());
    }
    hex(hasher)
}

/// The in-repo half, from the union of every published ref's file.
pub(crate) fn repo_files_fingerprint(file: Option<&RepoFilterFile>) -> String {
    let mut hasher = Sha1::new();
    if let Some(file) = file {
        feed(&mut hasher, b"file");
        for pattern in &file.exclude {
            feed(&mut hasher, pattern.as_bytes());
        }
        feed(
            &mut hasher,
            &[u8::from(file.exclude_submodules), u8::from(file.strip_empty_dirs)],
        );
    }
    hex(hasher)
}

/// The epoch whose output is on the public side, if this mirror has
/// published anything. A log from before epochs existed that still records
/// heads counts as epoch 0, so that switching such a mirror to history mode
/// can't force over its snapshots without a confirmed bump.
pub(crate) fn published_epoch(log: &PublishLog) -> Option<u32> {
    log.epoch.or((!log.heads.is_empty()).then_some(0))
}

/// The bump rule, at the save edge: `new` replacing `stored`.
///
/// # Errors
///
/// [`EpochBumpRequired`] when `new` changes what a history publish outputs
/// (or switches into or out of history mode) on a mirror that has already
/// published, and keeps the same epoch; a plain error when the epoch moves
/// by anything but zero or one.
pub(crate) fn check_update(stored: &PublishMirror, new: &PublishMirror, log: &PublishLog) -> anyhow::Result<()> {
    if new.filter_epoch < stored.filter_epoch || new.filter_epoch > stored.filter_epoch.saturating_add(1) {
        anyhow::bail!(
            "the epoch can only stay at {} or be bumped to {}",
            stored.filter_epoch,
            stored.filter_epoch.saturating_add(1)
        );
    }
    let history = stored.mode == PublishMode::History || new.mode == PublishMode::History;
    if history
        && published_epoch(log).is_some()
        && new.filter_epoch == stored.filter_epoch
        && config_fingerprint(stored) != config_fingerprint(new)
    {
        return Err(EpochBumpRequired.into());
    }
    Ok(())
}

/// Starts the GC grace of `left`, the epoch a confirmed bump moved away
/// from, unless something already recorded it. An epoch that is still the
/// published one stays protected anyway, and the run that replaces it on
/// the public side records the later moment over this one.
///
/// Never fails the save it follows, which has already landed: a lost
/// record only means the GC sweep records the epoch when it first sees it,
/// which gives it a longer grace, never a shorter one.
pub(crate) async fn record_bump(ctx: &AppContext, uid: &Uid, mirror_id: &str, left: u32) {
    let now = log::unix_now();
    if let Err(e) = log::update(ctx, uid, mirror_id, |log| {
        log.abandoned.entry(left).or_insert(now);
    })
    .await
    {
        warn!(
            target: "publish",
            repo = %uid,
            mirror = %mirror_id,
            epoch = left,
            error = %format!("{e:#}"),
            "couldn't record when the epoch was abandoned; the next GC sweep records it"
        );
    }
}
