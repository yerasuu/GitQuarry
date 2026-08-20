//! The repo's own push policy, applied to refs arriving from *outside* a
//! push — today only `gitquarry-mirror-external`'s pull path, which writes
//! packs and a manifest straight into S3 without ever going through
//! `git receive-pack` and therefore without ever reaching
//! [`super::hook::run_pre_receive_hook`], where all of this is normally
//! enforced.
//!
//! Everything checked here is checked by that hook too, and for the same
//! reason: the settings a repo carries (protected branches/tags, required
//! signatures, LFS blob ceiling, the owner's storage quota) are properties
//! of *the repo*, not of one transport. A ref that couldn't have been
//! pushed must not be able to arrive through a mirror instead.
//!
//! One deliberate difference from the hook: there's no actor. A mirror is
//! configured by someone with repo-settings authority and then runs
//! unattended, so "only an owner may push directly to a protected branch"
//! has nobody to check against. Protected refs therefore get the *shape*
//! of the rule that survives without an actor — a protected branch may
//! only fast-forward, a protected tag may only be created, never moved —
//! and never the owner override.

use crate::app::{AppContext, ResolvedRepo};
use crate::git;
use crate::storage::entities::RepoConfig;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;

/// Everything the checks below need that's worth fetching once per mirror
/// sweep rather than once per ref: the repo's config, every ref's sha as
/// S3 currently has it (what bounds "commits this update introduces"), and
/// the owner's quota verdict.
pub struct InboundGate {
    config: RepoConfig,
    have_tips: Vec<String>,
    over_quota: Option<(u64, u64)>,
}

impl InboundGate {
    /// Reads the repo config, manifest and quota state — call once before a
    /// sweep, then [`Self::check`] per ref.
    pub async fn load(ctx: &AppContext, resolved: &ResolvedRepo) -> Result<Self> {
        let (config, _) = resolved.remote.get_repo_config().await?;
        let (manifest, _) = resolved.remote.get_manifest().await?;
        let over_quota =
            crate::auth::access::check_quota(ctx.backend().as_ref(), &ctx.cipher, config.owner.as_ref()).await?;
        Ok(InboundGate {
            config,
            have_tips: manifest.refs.values().cloned().collect(),
            over_quota,
        })
    }

    /// `Some(reason)` if `new_sha` must not become `ref_name`'s sha.
    /// `local_sha` is what the ref points at right now (`None` for a ref
    /// this repo doesn't have yet) — the "old" side of the update, exactly
    /// as the pre-receive hook sees it.
    ///
    /// The objects behind `new_sha` must already be in `git_dir`: the
    /// ancestry, signature and blob-size checks all walk real history.
    pub async fn check(
        &self,
        ctx: &AppContext,
        git_dir: &Path,
        ref_name: &str,
        local_sha: Option<&str>,
        new_sha: &str,
    ) -> Result<Option<String>> {
        if self.config.deleted_at.is_some() {
            return Ok(Some("repo is deleted".to_string()));
        }
        if self.config.archived {
            return Ok(Some("repo is archived".to_string()));
        }
        if let Some((used, limit)) = self.over_quota {
            return Ok(Some(format!(
                "owner is over their storage quota ({used} of {limit} bytes used)"
            )));
        }
        if let Some(reason) = self.check_protected(git_dir, ref_name, local_sha, new_sha).await? {
            return Ok(Some(reason));
        }
        if let Some(reason) = self.check_signed(ctx, git_dir, ref_name, new_sha).await? {
            return Ok(Some(reason));
        }
        self.check_blob_size(git_dir, new_sha).await
    }

    async fn check_protected(
        &self,
        git_dir: &Path,
        ref_name: &str,
        local_sha: Option<&str>,
        new_sha: &str,
    ) -> Result<Option<String>> {
        // A ref this repo doesn't have yet is a creation, and creation is
        // what protection has nothing to protect against — same carve-out
        // `hook::check_protected` makes for a zero "old" sha.
        let Some(local_sha) = local_sha else {
            return Ok(None);
        };
        if let Some(short) = ref_name.strip_prefix("refs/heads/")
            && self.config.protected_branches.contains(short)
            && !git::is_ancestor(git_dir, local_sha, new_sha).await?
        {
            return Ok(Some(format!(
                "protected branch '{short}': a mirror may only fast-forward it, and this isn't one"
            )));
        }
        if let Some(short) = ref_name.strip_prefix("refs/tags/")
            && self.config.protected_tags.contains(short)
            && local_sha != new_sha
        {
            return Ok(Some(format!("protected tag '{short}': can't be moved")));
        }
        Ok(None)
    }

    async fn check_signed(
        &self,
        ctx: &AppContext,
        git_dir: &Path,
        ref_name: &str,
        new_sha: &str,
    ) -> Result<Option<String>> {
        let Some(short) = ref_name.strip_prefix("refs/heads/") else {
            return Ok(None);
        };
        if !self.config.require_signed_commits.contains(short) {
            return Ok(None);
        }
        let backend = ctx.backend();
        for sha in git::new_commit_shas(git_dir, new_sha, &self.have_tips).await? {
            if !commit_signature_authorized(&backend, &ctx.cipher, git_dir, &self.config.full_name, &sha).await? {
                return Ok(Some(format!(
                    "branch '{short}' requires signed commits: {} isn't signed by a registered key with push access",
                    crate::validate::short_sha(&sha)
                )));
            }
        }
        Ok(None)
    }

    async fn check_blob_size(&self, git_dir: &Path, new_sha: &str) -> Result<Option<String>> {
        let Some(raw_limit) = &self.config.lfs_max_blob_size else {
            return Ok(None);
        };
        let max_size = super::gc::parse_size(raw_limit).context("invalid lfs_max_blob_size in repo config")?;
        let oversized = git::oversized_new_blobs(git_dir, new_sha, &self.have_tips, max_size).await?;
        let Some((path, size)) = oversized.first() else {
            return Ok(None);
        };
        Ok(Some(format!(
            "blob over the repo's {raw_limit} limit without LFS: {path} ({size} bytes)"
        )))
    }
}

/// Whether `sha`'s signature proves it came from an account that currently
/// has at least `Permission::Developer` on this repo — the one question
/// `require_signed_commits` actually asks, shared by the pre-receive hook
/// and by [`InboundGate::check`] so the two can't drift apart.
///
/// A verified-but-unauthorized signer (an ex-contributor whose key is still
/// registered) doesn't satisfy it: the point is proving *this* update came
/// from someone allowed to make it. An instance signature doesn't either —
/// it attests that this server built the commit, not that a person with
/// push access stands behind it.
pub(crate) async fn commit_signature_authorized(
    backend: &Arc<dyn crate::storage::Backend>,
    cipher: &Arc<crate::crypto::Cipher>,
    git_dir: &Path,
    repo_full_name: &str,
    sha: &str,
) -> Result<bool> {
    let Some(info) = git::browse::log(git_dir, sha, 1, None).await?.into_iter().next() else {
        // No commit object to inspect at all — nothing to authorize, same
        // "skip what can't be read" tolerance the hook's own loop has.
        return Ok(true);
    };
    let signer = crate::domain::commit_signature::verified_signer(
        backend,
        cipher,
        git_dir,
        &info.sha,
        info.signature_status,
        &info.author_email,
    )
    .await?;
    match &signer {
        Some(crate::domain::commit_signature::CommitSigner::User(username)) => {
            let perm =
                crate::auth::access::effective_permission_for(backend.as_ref(), cipher, username, repo_full_name)
                    .await?;
            Ok(perm >= Some(crate::auth::entities::Permission::Developer))
        }
        Some(crate::domain::commit_signature::CommitSigner::Instance) | None => Ok(false),
    }
}
