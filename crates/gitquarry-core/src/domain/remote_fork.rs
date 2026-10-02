//! Seeds a brand-new repo in this bucket from a *remote* gitquarry
//! instance's repo — the git/storage mechanic half of federation layer 2's
//! fork (`docs/federation-binary-split-concept.md` section 2), split out of
//! the old `federation::domain::fork::fork_from_peer` so it no longer needs
//! a `federation::entities::Peer` in scope: after the core/federation
//! binary split, `gitquarry-federation` is the one that knows about
//! `Peer`/link state, and it calls `POST /_internal/repos/fork-from-remote`
//! with just the two strings this needs to actually do the work
//! (`source_git_base_url`, `source_repo_name`) plus `peer_label` for
//! provenance. Same `domain`/`web` boundary [`crate::domain::repo`]'s doc
//! comment describes: transport-agnostic, `anyhow::Result`, no CSRF/
//! session/audit concerns.

use crate::app::{AppContext, ResolvedRepo};
use crate::storage;
use crate::storage::entities::{OwnerRef, PeerOrigin, RepoConfig, Visibility};
use crate::storage::remote::Manifest;
use anyhow::{Result, bail};

pub struct SeedFromRemote<'a> {
    pub source_git_base_url: &'a str,
    pub source_repo_name: &'a str,
    pub dest_full_name: &'a str,
    pub dest_owner: OwnerRef,
    pub description: Option<String>,
    pub peer_label: &'a str,
}

/// `None` if `dest_full_name` is already claimed by a live repo.
/// `description` is whatever the caller already had cached — best-effort,
/// never re-fetched here; this function only touches git content plus this
/// bucket's own storage, never the peer's HTTP API again. Always
/// `Visibility::Public`: nothing federation layers 1-2 expose is ever
/// anything else, so there's no other source visibility to inherit the way
/// `domain::repo::fork` inherits one.
pub async fn seed_from_remote(
    ctx: &AppContext,
    seed: SeedFromRemote<'_>,
    locale: gitquarry_i18n::Locale,
) -> Result<Option<ResolvedRepo>> {
    let SeedFromRemote {
        source_git_base_url,
        source_repo_name,
        dest_full_name,
        dest_owner,
        description,
        peer_label,
    } = seed;
    let mirror = ctx
        .open_remote_mirror_at(peer_label, source_git_base_url, source_repo_name)
        .await?;
    let Some(dest) = ctx.create_repo(dest_full_name).await? else {
        return Ok(None);
    };

    let refs = crate::git::browse::list_refs_detailed(&mirror.path, "refs/").await?;
    let head = crate::git::browse::resolve_head(&mirror.path)
        .await?
        .filter(|h| refs.iter().any(|r| &r.name == h));
    let manifest = Manifest {
        refs: refs.into_iter().map(|r| (r.name, r.sha)).collect(),
        head,
    };

    if !manifest.refs.is_empty() {
        let pack_dir = crate::git::scratch_dir(&mirror.path)?;
        if let Some(pack) = crate::git::pack_all_objects(&mirror.path, pack_dir.path()).await? {
            dest.remote
                .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
                .await?;
        }
    }
    // Create-only (`expected_etag: None`): unlike `fork`, nothing copied a
    // `config.json`/`refs.json` over first, `dest` has no prior state at
    // all to CAS against.
    dest.remote.put_manifest(&manifest, None).await?;
    if !dest.remote.exists().await? {
        bail!("{}", gitquarry_i18n::shared::repo::copy_seems_to_have_failed(locale));
    }

    let new_config = RepoConfig {
        uid: dest.uid.clone(),
        full_name: dest_full_name.to_string(),
        display_name: None,
        description,
        owner: Some(dest_owner),
        visibility: Visibility::Public,
        archived: false,
        deleted_at: None,
        last_push: None,
        last_push_commits: None,
        last_push_branch: None,
        pack_bytes: 0,
        lfs_bytes: 0,
        required_reviewers: Default::default(),
        is_template: false,
        protected_branches: Default::default(),
        protected_tags: Default::default(),
        require_signed_commits: Default::default(),
        lfs_max_blob_size: Default::default(),
        topics: Default::default(),
        forked_from: None,
        forked_from_peer: Some(PeerOrigin {
            peer: peer_label.to_string(),
            repo: source_repo_name.to_string(),
        }),
        fork_count: 0,
        star_count: 0,
        watcher_count: 0,
        primary_language: None,
        labels: Vec::new(),
    };
    let _ = storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        new_config.uid.as_str(),
        &new_config,
        None,
        &ctx.cipher,
    )
    .await;
    if let Some(owner) = &new_config.owner {
        storage::settings::add_owner_index(ctx.backend().as_ref(), &ctx.cipher, owner, &dest.uid).await;
    }
    storage::settings::set_visible_index(ctx.backend().as_ref(), &ctx.cipher, &dest.uid, true).await;

    Ok(Some(dest))
}
