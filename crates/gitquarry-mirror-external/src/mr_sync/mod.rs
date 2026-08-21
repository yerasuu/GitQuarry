//! Track B of `docs/external-mirror-concept.md`: bidirectional merge-request
//! sync against a linked [`crate::access::ExternalMirror`] with
//! `sync_merge_requests` on. Called from [`crate::engine::sync_repo_mirrors`]
//! right after that mirror's own ref sync — MR discovery assumes Track A
//! already pulled the head/base branches locally, so an MR's branches
//! always exist by the time this runs.
//!
//! [`ExternalMirror::direction`] governs this track exactly as it governs
//! Track A's refs: a `PullOnly` mirror never creates, patches, merges or
//! comments on the external side, and a `PushOnly` one never imports
//! anything from it.
//!
//! Anti-loop bookkeeping (the riskiest part, per the design doc) lives
//! entirely in [`sync_one_linked_mr`]: outbound comments are filtered by
//! `origin.is_none()` *and* `ExternalPrLink::synced_local_comment_keys` (the
//! second set isn't in the original design — `origin` alone only says
//! "mine to export", it never stops the same export from repeating every
//! tick), inbound comments by `synced_remote_comment_ids`.

use crate::access::{self, ExternalMirror, ExternalMirrorProvider, ExternalPrLink};
use anyhow::{Result, bail};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::domain::merge_request::{self, NewMergeRequest};
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::merge_requests::{self, MrComment, MrStatus};
use gitquarry_forge_clients::{self as forge, ForgeApi, MrPatch, NewRemoteMr, RemoteMr, RemoteMrState};
use std::collections::{BTreeSet, HashMap};

/// What a local comment's body looks like on the external side. A plain
/// thread comment goes out verbatim; a line comment gets its `path:line`
/// prepended, since [`ForgeApi::add_comment`] posts an issue-style comment
/// with nowhere to put a diff position. Prefixing is lossy on the way back
/// (a re-imported copy is a plain comment whose body happens to start with
/// a path) but that round trip can't happen: an imported comment carries
/// `origin`, and `origin.is_some()` is exactly what the export filter
/// skips.
///
/// Not `#[cfg]`-gated: called unconditionally from [`sync_one_linked_mr`]'s
/// production comment-export path, not just from tests.
pub fn export_body(comment: &MrComment) -> String {
    match &comment.anchor {
        Some(anchor) => format!("`{}:{}`\n\n{}", anchor.path, anchor.line, comment.body),
        None => comment.body.clone(),
    }
}

/// `Other` is the only provider tag with no real [`ForgeApi`] impl — it's a
/// UI label for "some other git host", not a specific REST API this trait
/// could target, so enabling `sync_merge_requests` on one fails loudly here
/// rather than silently doing nothing.
fn build_forge(mirror: &ExternalMirror) -> Result<Box<dyn ForgeApi>> {
    match mirror.provider {
        ExternalMirrorProvider::GitHub => Ok(Box::new(forge::github::GitHubForge::new(
            &mirror.remote_url,
            &mirror.token,
        )?)),
        ExternalMirrorProvider::GitLab => Ok(Box::new(forge::gitlab::GitLabForge::new(
            &mirror.remote_url,
            &mirror.token,
        )?)),
        ExternalMirrorProvider::Codeberg => Ok(Box::new(forge::codeberg::CodebergForge::new(
            &mirror.remote_url,
            &mirror.token,
        )?)),
        other => bail!("MR sync isn't implemented for provider {other:?}"),
    }
}

/// One MR-sync pass for `mirror`: discovers new MRs in both directions,
/// links them, then syncs every linked MR's fields/merge/comments. Errors
/// on one MR (a forge call failing, a CAS conflict) are logged and never
/// abort the rest — same log-and-continue discipline
/// [`crate::engine::sync_repo_mirrors`] already applies one level up for
/// mirrors themselves.
pub async fn sync_merge_requests_once(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &ExternalMirror,
) -> Result<()> {
    let forge = build_forge(mirror)?;
    sync_merge_requests_once_with(ctx, resolved, mirror, forge.as_ref()).await
}

/// [`sync_merge_requests_once`]'s real logic, taking `forge` directly rather
/// than building it from `mirror.provider` — split out so tests can drive
/// it with an in-memory fake forge instead of a real GitHub/GitLab/Codeberg
/// account, per `docs/external-mirror-concept.md`'s own verification
/// section ("el trait `ForgeApi` permite mockear GitHub... suficientemente
/// pura para testear sin llamada real a la API").
///
/// Not `#[cfg]`-gated behind `test-util` like a true test-only item: this
/// is also [`sync_merge_requests_once`]'s real production entry point, so it
/// must stay compiled in every build — only its visibility is widened here,
/// for the moved integration tests in `tests/mr_sync.rs`.
pub async fn sync_merge_requests_once_with(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &ExternalMirror,
    forge: &dyn ForgeApi,
) -> Result<()> {
    let uid = &resolved.uid;

    let links = access::list_mr_links(ctx.backend().as_ref(), &ctx.cipher, uid, &mirror.id).await?;
    let cursor = links
        .iter()
        .map(|l| l.remote_updated_cursor.as_str())
        .filter(|c| !c.is_empty())
        .reduce(crate::stamp::max)
        .map(str::to_string);

    let mut linked_by_external: HashMap<u64, u64> = links.iter().map(|l| (l.external_number, l.local_number)).collect();
    // `BTreeSet`, not `HashSet`: the per-MR pass below walks this, and a
    // sync whose order changes run to run is that much harder to read a
    // log of.
    let mut linked_locals: BTreeSet<u64> = links.iter().map(|l| l.local_number).collect();

    let remote_mrs = forge.list_merge_requests(cursor.as_deref()).await?;
    // An MR's head/base have to exist here before a local one can be
    // created for it (`domain::merge_request::create` refuses otherwise) —
    // and for an MR opened from a *fork* on the external side, the head
    // branch lives in that fork and Track A never mirrors it, so it can
    // never exist. Checked up front so those MRs are skipped once with a
    // count, instead of each failing `create` again on every single tick.
    let (manifest, _etag) = resolved.remote.get_manifest().await?;
    let mirrored_branch = |branch: &str| manifest.refs.contains_key(&format!("refs/heads/{branch}"));
    let mut unmirrored_branches = 0usize;
    for remote_mr in &remote_mrs {
        if linked_by_external.contains_key(&remote_mr.number) || !mirror.direction.allows_pull() {
            continue;
        }
        if !mirrored_branch(&remote_mr.source_branch) || !mirrored_branch(&remote_mr.target_branch) {
            unmirrored_branches += 1;
            continue;
        }
        match create_local_mr_from_remote(ctx, resolved, &mirror.id, remote_mr).await {
            Ok(local_number) => {
                linked_by_external.insert(remote_mr.number, local_number);
                linked_locals.insert(local_number);
            }
            Err(e) => eprintln!(
                "gitquarry: warning: couldn't mirror external MR #{} (mirror '{}') locally: {e:#}",
                remote_mr.number, mirror.label
            ),
        }
    }

    if unmirrored_branches > 0 {
        eprintln!(
            "gitquarry: {unmirrored_branches} merge request(s) on mirror '{}' aren't importable: their branches aren't mirrored here (an MR opened from a fork can't be)",
            mirror.label
        );
    }

    let local_mrs = merge_requests::list_merge_requests(&ctx.backend(), &ctx.cipher, uid).await?;
    for mr in &local_mrs {
        if linked_locals.contains(&mr.number) || !mirror.direction.allows_push() {
            continue;
        }
        match forge
            .create_merge_request(NewRemoteMr {
                title: mr.title.clone(),
                body: mr.description.clone(),
                head: mr.source_branch.clone(),
                base: mr.target_branch.clone(),
            })
            .await
        {
            Ok(created) => {
                let link = ExternalPrLink {
                    local_number: mr.number,
                    external_number: created.number,
                    remote_updated_cursor: created.updated_at,
                    ..Default::default()
                };
                // A `Cas::Conflict` matters as much as an `Err` here and
                // used to be dropped on the floor: with no link stored,
                // the next pass sees this local MR as unlinked and creates
                // a *second* one on the external side.
                match access::put_mr_link(ctx.backend().as_ref(), &ctx.cipher, uid, &mirror.id, &link, None).await {
                    Ok(Cas::Ok) => {}
                    Ok(Cas::Conflict) => eprintln!(
                        "gitquarry: warning: created MR #{} on mirror '{}' but a link for it already existed — \
                         it won't be synced, remove the stale link to recover",
                        mr.number, mirror.label
                    ),
                    Err(e) => eprintln!(
                        "gitquarry: warning: created MR #{} on mirror '{}' but couldn't persist its link: {e:#}",
                        mr.number, mirror.label
                    ),
                }
                linked_by_external.insert(created.number, mr.number);
                linked_locals.insert(mr.number);
            }
            Err(e) => eprintln!(
                "gitquarry: warning: couldn't create MR #{} (mirror '{}') on the external side: {e:#}",
                mr.number, mirror.label
            ),
        }
    }

    for local_number in linked_locals {
        if let Err(e) = sync_one_linked_mr(ctx, resolved, mirror, forge, local_number).await {
            eprintln!(
                "gitquarry: warning: MR #{local_number} sync failed (mirror '{}'): {e:#}",
                mirror.label
            );
        }
    }
    Ok(())
}

/// Creates the local MR, then immediately mirrors an already-`Closed`
/// remote state (`merge_request::create` has no status param — every new MR
/// starts `Open`). An already-`Merged` remote MR is deliberately left
/// `Open` here instead: `sync_one_linked_mr`'s merge-propagation check right
/// after this returns is unconditional on current status, not gated by the
/// link's cursor the way the closed-state catch-up would be, so it fires on
/// its own very next pass and drives the real `mark_merged_externally` path
/// (with its `merged_at`/`merge_commit_sha` bookkeeping) instead of this
/// function half-doing that job with less information.
async fn create_local_mr_from_remote(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror_id: &str,
    remote_mr: &RemoteMr,
) -> Result<u64> {
    let mr = merge_request::create(
        ctx,
        resolved,
        NewMergeRequest {
            source_repo: &resolved.uid,
            source_branch: &remote_mr.source_branch,
            target_branch: &remote_mr.target_branch,
            title: &remote_mr.title,
            description: &remote_mr.body,
            author: &remote_mr.author,
        },
        gitquarry_i18n::Locale::DEFAULT,
    )
    .await?;
    if remote_mr.state == RemoteMrState::Closed {
        merge_request::close(ctx, resolved, mr.number, gitquarry_i18n::Locale::DEFAULT).await?;
    }
    let link = ExternalPrLink {
        local_number: mr.number,
        external_number: remote_mr.number,
        remote_updated_cursor: remote_mr.updated_at.clone(),
        ..Default::default()
    };
    // The `Cas` is checked, not discarded: a conflict means the local MR
    // exists but nothing records which external one it mirrors, and the
    // next pass would import that same external MR all over again. Loud
    // beats silent — the caller logs it and the operator sees one stray
    // local MR instead of a new one every tick.
    if access::put_mr_link(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &resolved.uid,
        mirror_id,
        &link,
        None,
    )
    .await?
        == Cas::Conflict
    {
        bail!(
            "local MR #{} was created for external #{} but a link for it already existed",
            mr.number,
            remote_mr.number
        );
    }
    Ok(mr.number)
}

/// Writes one comment's anti-loop bookkeeping through immediately, instead
/// of waiting for the end-of-pass merge. Best-effort: the comment itself
/// already landed either way, and the end-of-pass merge is still there to
/// catch what this couldn't write — this exists to shrink the window where
/// a dropped future costs a duplicate, not to be the only writer.
async fn record_synced_comment(
    ctx: &AppContext,
    uid: &gitquarry_core::storage::uid::Uid,
    mirror: &ExternalMirror,
    local_number: u64,
    local_key: Option<&str>,
    remote_id: u64,
) {
    let outcome = access::merge_mr_link(
        ctx.backend().as_ref(),
        &ctx.cipher,
        uid,
        &mirror.id,
        local_number,
        |stored| {
            if let Some(key) = local_key {
                stored.synced_local_comment_keys.insert(key.to_string());
            }
            stored.synced_remote_comment_ids.insert(remote_id);
        },
    )
    .await;
    if let Err(e) = outcome {
        eprintln!(
            "gitquarry: warning: a comment on MR #{local_number} synced with mirror '{}' but recording it failed, it may be synced twice: {e:#}",
            mirror.label
        );
    }
}

/// Syncs one already-linked MR: merge propagation (its own dedicated forge
/// endpoint, never a plain field patch), then — only if neither side just
/// merged this tick — ordinary title/description/open-closed last-write-wins
/// against the link's `remote_updated_cursor`, then comments both ways.
///
/// Not `#[cfg]`-gated: called unconditionally from
/// [`sync_merge_requests_once_with`]'s production loop over linked MRs, not
/// just from tests.
pub async fn sync_one_linked_mr(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &ExternalMirror,
    forge: &dyn ForgeApi,
    local_number: u64,
) -> Result<()> {
    let uid = &resolved.uid;
    let (Some(mut link), _link_etag) =
        access::get_mr_link(ctx.backend().as_ref(), &ctx.cipher, uid, &mirror.id, local_number).await?
    else {
        bail!("missing MR link for local #{local_number}");
    };
    let Some(remote_mr) = forge.get_merge_request(link.external_number).await? else {
        // Deleted upstream — nothing sane to reconcile against; leave the
        // local MR and link exactly as they are rather than guessing.
        return Ok(());
    };
    let (Some(mut local_mr), local_etag) =
        merge_requests::get_merge_request(ctx.backend().as_ref(), &ctx.cipher, uid, local_number).await?
    else {
        bail!("missing local MR #{local_number}");
    };

    let allows_push = mirror.direction.allows_push();
    let allows_pull = mirror.direction.allows_pull();
    if allows_pull && remote_mr.state == RemoteMrState::Merged && local_mr.status != MrStatus::Merged {
        if let Some(sha) = remote_mr.merge_commit_sha.as_deref() {
            merge_request::mark_merged_externally(ctx, resolved, local_number, sha, remote_mr.merged_by.as_deref())
                .await?;
        }
    } else if allows_push && local_mr.status == MrStatus::Merged && remote_mr.state != RemoteMrState::Merged {
        // Best-effort: the local merge already succeeded and is the source
        // of truth regardless of whether the external side accepts it (same
        // tolerance `domain::merge_request::merge`'s own CAS-conflict warning
        // already applies to its metadata write).
        if let Err(e) = forge.accept_merge_request(link.external_number).await {
            eprintln!(
                "gitquarry: warning: couldn't propagate merge of MR #{local_number} (mirror '{}') externally: {e:#}",
                mirror.label
            );
        }
    } else {
        // Parsed instants, not raw strings: the two sides don't agree on
        // sub-second precision or on how UTC is spelled (see
        // [`crate::stamp`]).
        let remote_is_newer = crate::stamp::later(&remote_mr.updated_at, &link.remote_updated_cursor);
        let local_is_newer = crate::stamp::later(&local_mr.updated_at, &link.remote_updated_cursor);
        if remote_is_newer && allows_pull {
            local_mr.title = remote_mr.title.clone();
            local_mr.description = remote_mr.body.clone();
            match remote_mr.state {
                RemoteMrState::Closed => local_mr.status = MrStatus::Closed,
                RemoteMrState::Open => local_mr.status = MrStatus::Open,
                RemoteMrState::Merged => {}
            }
            local_mr.updated_at = remote_mr.updated_at.clone();
            if merge_requests::put_merge_request(
                ctx.backend().as_ref(),
                &ctx.cipher,
                uid,
                &local_mr,
                local_etag.as_deref(),
            )
            .await?
                == Cas::Conflict
            {
                eprintln!(
                    "gitquarry: warning: MR #{local_number} field sync from mirror '{}' hit a concurrent local edit, retrying next tick",
                    mirror.label
                );
            }
        } else if local_is_newer && allows_push {
            let state = match (local_mr.status, remote_mr.state) {
                (MrStatus::Closed, RemoteMrState::Open) => Some(RemoteMrState::Closed),
                (MrStatus::Open, RemoteMrState::Closed) => Some(RemoteMrState::Open),
                _ => None,
            };
            if let Err(e) = forge
                .update_merge_request(
                    link.external_number,
                    MrPatch {
                        title: Some(local_mr.title.clone()),
                        body: Some(local_mr.description.clone()),
                        state,
                    },
                )
                .await
            {
                eprintln!(
                    "gitquarry: warning: couldn't push MR #{local_number} field edits to mirror '{}': {e:#}",
                    mirror.label
                );
            }
        }
    }
    link.remote_updated_cursor = crate::stamp::max(&link.remote_updated_cursor, &remote_mr.updated_at).to_string();

    // Outbound: local comments never yet exported, and never something
    // imported from this same mirror in the first place. A line comment
    // goes out with its position spelled into the body (see
    // [`export_body`]) — [`ForgeApi::add_comment`] posts issue-style
    // comments, which carry no diff position of their own.
    let local_comments = match allows_push {
        true => merge_requests::list_mr_comments_with_keys(&ctx.backend(), &ctx.cipher, uid, local_number).await?,
        false => Vec::new(),
    };
    for (key, comment) in &local_comments {
        if comment.origin.is_some() || link.synced_local_comment_keys.contains(key) {
            continue;
        }
        match forge.add_comment(link.external_number, &export_body(comment)).await {
            Ok(remote_comment) => {
                link.synced_local_comment_keys.insert(key.clone());
                link.synced_remote_comment_ids.insert(remote_comment.id);
                // Persisted here, not only in the merge at the end of this
                // function: the comment is already on the external forge,
                // and this future can be dropped at any await — the
                // pre-serve sync runs it under a 5s timeout, and a forge
                // call is exactly what that timeout tends to land on.
                // Losing the key would mean posting this same comment
                // again on the next tick.
                record_synced_comment(ctx, uid, mirror, local_number, Some(key), remote_comment.id).await;
            }
            Err(e) => eprintln!(
                "gitquarry: warning: couldn't export a comment on MR #{local_number} to mirror '{}': {e:#}",
                mirror.label
            ),
        }
    }

    // Inbound: remote comments never yet imported (including ones this
    // same pass just exported — pre-seeded into `synced_remote_comment_ids`
    // above, so they never round-trip back in as if new).
    let remote_comments = match allows_pull {
        true => forge.list_comments(link.external_number).await?,
        false => Vec::new(),
    };
    for rc in &remote_comments {
        if link.synced_remote_comment_ids.contains(&rc.id) {
            continue;
        }
        // No anchor: the forge APIs this imports from expose issue-style
        // comments, which carry no diff position of their own.
        let comment = MrComment {
            author: rc.author.clone(),
            body: rc.body.clone(),
            created_at: rc.created_at.clone(),
            origin: Some(mirror.id.clone()),
            anchor: None,
        };
        if let Err(e) =
            merge_requests::add_mr_comment(ctx.backend().as_ref(), &ctx.cipher, uid, local_number, &comment).await
        {
            eprintln!(
                "gitquarry: warning: couldn't import a comment on MR #{local_number} from mirror '{}': {e:#}",
                mirror.label
            );
            continue;
        }
        link.synced_remote_comment_ids.insert(rc.id);
        // Same cancellation window as the export side above, mirrored: the
        // comment is already in local storage, so losing its id would
        // import a second copy next tick.
        record_synced_comment(ctx, uid, mirror, local_number, None, rc.id).await;
    }

    // Merged rather than written over `link_etag`: everything this pass
    // accumulated is set insertions and a monotonic cursor, so re-applying
    // it on top of whatever landed meanwhile is both correct and the only
    // way a lost race doesn't cost the anti-loop bookkeeping (and with it
    // a second copy of every comment on the next tick).
    access::merge_mr_link(
        ctx.backend().as_ref(),
        &ctx.cipher,
        uid,
        &mirror.id,
        local_number,
        |stored| {
            stored
                .synced_local_comment_keys
                .extend(link.synced_local_comment_keys.iter().cloned());
            stored
                .synced_remote_comment_ids
                .extend(link.synced_remote_comment_ids.iter());
            stored.remote_updated_cursor =
                crate::stamp::max(&stored.remote_updated_cursor, &link.remote_updated_cursor).to_string();
        },
    )
    .await?;
    Ok(())
}
