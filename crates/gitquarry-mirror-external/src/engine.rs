//! External mirror sync — push/pull against a non-gitquarry forge
//! (GitHub/GitLab/Codeberg), see `docs/external-mirror-concept.md`. Moved
//! out of `gitquarry-core::mirror::external` into its own crate
//! (`docs/mirror-external-extraction-concept.md`) purely for code
//! organization — `gitquarry-server` calls straight into this module
//! in-process (via `gitquarry_core::mirror::MirrorSyncHook`, since core
//! itself can't depend on this crate), the same process and bare-repo
//! filesystem `gitquarry_core::app::AppContext::open_mirror` already owns
//! the locking for.

use crate::access::{self, RefSyncStatus};
use anyhow::{Result, bail};
use futures::stream::{self, TryStreamExt};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::uid::Uid;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// How many CAS-conflict retries [`persist_pulled_ref_to_s3`] attempts
/// before giving up — same bound `storage::pull_requests::next_pr_number`
/// uses for its own CAS retry loop.
const MANIFEST_CAS_MAX_RETRIES: u32 = 8;

/// How often [`run_periodic_sync`] sweeps every repo with at least one
/// enabled mirror — same granularity as `mirror::gc::SWEEP_INTERVAL`; no
/// reason for this to tick any faster than that, and no config knob for it
/// yet (Track A doesn't call for one — see `docs/external-mirror-concept.md`).
const SYNC_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Runs [`sync_all_repos_once`] forever on [`SYNC_INTERVAL`] —
/// `gitquarry-server` spawns this in-process at startup
/// (`tokio::spawn`), same as `mirror::gc::run_periodic_sweep`'s own tick.
/// Never exits on a tick failure, same discipline that sweep already
/// establishes.
pub async fn run_periodic_sync(ctx: Arc<AppContext>) {
    let mut ticker = tokio::time::interval(SYNC_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(e) = sync_all_repos_once(&ctx).await {
            eprintln!("gitquarry-mirror-external: warning: sync tick failed: {e:#}");
        }
    }
}

/// How many *different* repos sync at once. Bounded rather than sequential:
/// different repos share nothing — separate locks, separate mirrors,
/// usually separate forges — so one repo whose forge is slow (a git call
/// here can legitimately take minutes) must not push everybody else's
/// sweep behind it. Bounded rather than unbounded because the far side is
/// somebody's rate limit.
const REPO_SYNC_CONCURRENCY: usize = 4;

/// One sweep: every repo indexed as having an enabled mirror (see
/// `gitquarry_core::storage::settings::list_repos_with_enabled_mirrors`'s
/// "hint, not a fact" doctrine). A repo failing (its mirrors collection
/// itself unreadable, say) only logs and doesn't affect the others —
/// [`sync_repo_mirrors`] already applies the same per-mirror tolerance one
/// level down, so this is the outermost layer of the same discipline, not a
/// new one.
async fn sync_all_repos_once(ctx: &AppContext) -> Result<()> {
    let uids = gitquarry_core::storage::settings::list_repos_with_enabled_mirrors(ctx.backend().as_ref()).await?;
    stream::iter(uids.into_iter().map(Ok::<_, anyhow::Error>))
        .try_for_each_concurrent(Some(REPO_SYNC_CONCURRENCY), |uid| async move {
            let resolved = ResolvedRepo {
                remote: ctx.remote_for_uid(&uid),
                uid: uid.clone(),
            };
            if let Err(e) = sync_repo_mirrors(ctx, &resolved).await {
                eprintln!("gitquarry-mirror-external: warning: sync failed for repo {uid}: {e:#}");
            }
            Ok(())
        })
        .await
}

/// Builds the credentialed URL the sync path actually pushes/fetches
/// against — `remote_url` never carries a token (see
/// [`access::ExternalMirror`]'s own doc comment), so it's stitched in here,
/// right before use, never stored. `validate_mirror_token` already rejects
/// `@`/`:`/`/` in `token`, so this can't have its authority segment
/// smuggled into a different host.
#[cfg(any(test, feature = "test-util"))]
pub fn credentialed_url(remote_url: &str, token: &str) -> String {
    credentialed_url_impl(remote_url, token)
}
#[cfg(not(any(test, feature = "test-util")))]
fn credentialed_url(remote_url: &str, token: &str) -> String {
    credentialed_url_impl(remote_url, token)
}
fn credentialed_url_impl(remote_url: &str, token: &str) -> String {
    if let Some(rest) = remote_url.strip_prefix("https://") {
        format!("https://x-access-token:{token}@{rest}")
    } else if let Some(rest) = remote_url.strip_prefix("http://") {
        format!("http://x-access-token:{token}@{rest}")
    } else {
        // Unreachable given `validate_mirror_url` already gates on this at
        // save time — falling back to the bare url rather than panicking
        // keeps this function total.
        remote_url.to_string()
    }
}

/// Lock key for a whole sweep of one repo — deliberately *not* the repo's
/// own key (`AppContext::open_mirror`'s), which [`sync_mirror_once`] takes
/// while it runs and which every clone and push needs too.
///
/// Two sweeps of the same repo used to be able to interleave: Track A
/// serialized itself on the repo lock, but [`crate::pr_sync`] runs after
/// that guard is dropped and took no lock at all, so two concurrent
/// triggers (two git requests through `mirror::sync_external_before_serving`,
/// or one of those racing the periodic tick) both discovered the same
/// unlinked PR and both created it — duplicate PRs upstream, duplicate
/// imports locally.
///
/// A sweep always takes this lock *before* the repo's own, never the other
/// way round, so the two can't cycle.
fn sweep_lock_key(uid: &Uid) -> String {
    format!("mirror-sweep/{uid}")
}

/// Syncs every enabled mirror of one repo, one at a time — mirrors are
/// independent of each other, so one failing must never block the others
/// (log-and-continue, same resilience `mirror::gc::sweep_once` and the
/// other periodic sweeps already establish for their own per-item sweeps).
///
/// Sequential on purpose, not a bounded fan-out: every [`sync_mirror_once`]
/// holds this repo's own lock (`AppContext::open_mirror`) for its whole
/// git-side sweep, so concurrent mirrors of the *same* repo can only queue
/// behind each other anyway — a concurrency bound here would buy nothing
/// but the illusion of parallelism.
pub async fn sync_repo_mirrors(ctx: &AppContext, resolved: &ResolvedRepo) -> Result<()> {
    let _sweep_guard = ctx.locks.lock(&sweep_lock_key(&resolved.uid)).await;
    // A soft-deleted repo is unreachable over ssh/http but still sits in
    // the `mirrors-enabled` index, so only this sweep could still reach it
    // — and pushing its refs to a third party after its owner deleted it is
    // the last thing deleting a repo should mean. `mirror::policy` covers
    // the pull direction (it refuses to write into a deleted repo); this is
    // the other half.
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if config.deleted_at.is_some() {
        return Ok(());
    }
    for mirror in crate::domain::list(ctx, &resolved.uid)
        .await?
        .into_iter()
        .filter(|m| m.enabled)
    {
        let mut failure = match sync_mirror_once(ctx, resolved, &mirror).await {
            // A per-ref failure never makes this an `Err` — see
            // [`SyncReport`] — so the report is where "the sweep ran but
            // nothing moved" has to be read off.
            Ok(report) => report.failure_summary(),
            Err(e) => {
                eprintln!(
                    "gitquarry-mirror-external: warning: mirror '{}' ({}) sync failed: {e:#}",
                    mirror.label, mirror.id
                );
                Some(format!("{e:#}"))
            }
        };
        // Track B, opt-in: PR discovery assumes Track A's own ref sync
        // right above already pulled every PR's head/base branch locally,
        // so this always runs after, never in parallel with it.
        if mirror.sync_pull_requests
            && let Err(e) = crate::pr_sync::sync_pull_requests_once(ctx, resolved, &mirror).await
        {
            eprintln!(
                "gitquarry-mirror-external: warning: PR sync for mirror '{}' ({}) failed: {e:#}",
                mirror.label, mirror.id
            );
            // Only if the ref sync above didn't already fail: that one is
            // the more fundamental failure, and it's usually the cause of
            // this one rather than a second, separate problem.
            failure.get_or_insert_with(|| format!("{e:#}"));
        }
        record_sync_outcome(ctx, &resolved.uid, &mirror.id, failure).await;
    }
    Ok(())
}

/// Persists a whole-mirror sync outcome to
/// [`access::ExternalMirror::last_error`] — `None` clears it, so a mirror
/// that recovers stops showing a stale failure.
///
/// Best-effort by design, and the only reason this isn't inlined at the two
/// call sites: it runs on the error path of a sync that is itself
/// best-effort, so failing to *record* a failure must not escalate into
/// aborting the sweep over the remaining mirrors.
async fn record_sync_outcome(ctx: &AppContext, uid: &Uid, mirror_id: &str, failure: Option<String>) {
    if let Err(e) = access::set_mirror_error(ctx.backend().as_ref(), &ctx.cipher, uid, mirror_id, failure).await {
        eprintln!("gitquarry-mirror-external: warning: couldn't record sync status for mirror {mirror_id}: {e:#}");
    }
}

/// The interactive "sync now" button's real work — unlike
/// [`sync_repo_mirrors`]'s tick (which must never abort just because one
/// mirror failed, and only logs to stderr where a web click can never see
/// it), this syncs exactly the one mirror a user clicked and surfaces
/// whatever `Err` it hits straight back to them instead of swallowing it.
pub async fn sync_one_mirror_now(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &access::ExternalMirror,
) -> Result<()> {
    // Same exclusion the periodic/pre-serve sweep takes (see
    // [`sweep_lock_key`]): a click landing while one of those runs must
    // queue behind it, not sync the same mirror alongside it.
    let _sweep_guard = ctx.locks.lock(&sweep_lock_key(&resolved.uid)).await;
    let outcome = async {
        // A ref that failed to push is exactly what this click needs to
        // hear about, and `sync_mirror_once` deliberately doesn't `Err` on
        // one (see [`SyncReport`]) — so the report is turned into one here.
        // Without this, a mirror whose every push was rejected still came
        // back as a plain success.
        let report = sync_mirror_once(ctx, resolved, mirror).await?;
        if let Some(failure) = report.failure_summary() {
            bail!("{failure}");
        }
        if mirror.sync_pull_requests {
            crate::pr_sync::sync_pull_requests_once(ctx, resolved, mirror).await?;
        }
        Ok(())
    }
    .await;
    // Recorded here too, not just on the periodic/push-triggered path: this
    // click and that sweep write the same `last_error`, so a manual sync
    // that succeeds has to clear what the last automatic one left behind.
    record_sync_outcome(
        ctx,
        &resolved.uid,
        &mirror.id,
        outcome.as_ref().err().map(|e: &anyhow::Error| format!("{e:#}")),
    )
    .await;
    outcome
}

/// What one mirror's ref sweep actually did.
///
/// [`sync_mirror_once`] can't report this by returning `Err`: refs are
/// independent, so one failing must never abort the rest — every per-ref
/// outcome is recorded and the sweep runs to the end. Handing the outcomes
/// back instead of only writing them to each ref's own
/// [`RefSyncStatus`] is what lets a caller tell "nothing to do" apart from
/// "every push was rejected", which otherwise both came back as a bare
/// `Ok(())` — and made the interactive "sync now" click report success
/// while nothing moved.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub pushed: usize,
    pub pulled: usize,
    /// Refs that moved on both sides with neither an ancestor of the other.
    pub diverged: Vec<String>,
    /// Refs deleted on one side, left alone on the other. Tracked apart
    /// from `diverged` precisely so [`Self::failure_summary`] ignores them:
    /// somebody deleting a branch is a normal thing to do, and counting it
    /// as a failure left the mirror permanently red — and every "sync now"
    /// click reporting an error — until the other side was deleted too.
    pub deleted: Vec<String>,
    /// `(ref name, error)`, in sweep order. Already redacted: every git
    /// call that takes a credentialed URL redacts its own `Err`.
    pub errors: Vec<(String, String)>,
}

impl SyncReport {
    /// One line naming what didn't sync, or `None` if everything the sweep
    /// planned actually landed. A diverged ref counts: the caller asked for
    /// a sync and that ref didn't get one, whatever the reason.
    pub fn failure_summary(&self) -> Option<String> {
        if let Some((ref_name, error)) = self.errors.first() {
            let more = match self.errors.len() {
                1 => String::new(),
                n => format!(" (and {} more ref(s) failed)", n - 1),
            };
            return Some(format!("{ref_name}: {error}{more}"));
        }
        if self.diverged.is_empty() {
            return None;
        }
        Some(format!(
            "diverged, not synced (neither side is an ancestor of the other): {}",
            self.diverged.join(", ")
        ))
    }
}

/// What one ref should do this tick, decided by [`plan_ref_sync`] before any
/// git subprocess runs — keeps the actual decision table unit-testable
/// without a real repo.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefAction {
    /// Already matches `last_synced` (or both sides already agree and this
    /// is the very first tick that's noticed) — just (re)record it.
    Record(String),
    Push(String),
    Pull(String),
    Diverged,
    /// The ref exists on exactly one side *and* this mirror has a
    /// `last_synced` sha for it, so it once existed on both: somebody
    /// deleted it. Never re-created from the surviving side — that would
    /// silently undo the deletion on the next tick, forever.
    DeletedOneSide,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum RefAction {
    Record(String),
    Push(String),
    Pull(String),
    Diverged,
    DeletedOneSide,
}

/// The three-way comparison itself: `local`/`remote` are this ref's current
/// sha on each side (`None` = ref doesn't exist there), `last_synced` the
/// last sha this mirror recorded for it. Async only because the one case
/// where both sides moved needs a real `gitquarry_core::git::is_ancestor`
/// call — every other branch is a pure lookup.
///
/// - Present on one side only, with a `last_synced` sha: it existed on
///   both once, so this is a deletion — [`RefAction::DeletedOneSide`],
///   never re-created from the surviving side.
/// - Local only, never synced: push (new local ref) if the direction allows it.
/// - Remote only, never synced: pull (new remote ref) if the direction allows it.
/// - Same sha: nothing to move, just make sure it's recorded.
/// - Different shas: fast-forward whichever side is behind, via
///   `is_ancestor` — regardless of which one moved since `last_synced`, so
///   a rewound side is never replayed onto the other. Neither is an
///   ancestor of the other → [`RefAction::Diverged`], never forced.
#[cfg(any(test, feature = "test-util"))]
pub async fn plan_ref_sync(
    bare_repo: &std::path::Path,
    local: Option<&str>,
    remote: Option<&str>,
    last_synced: Option<&str>,
    direction: access::SyncDirection,
) -> Result<Option<RefAction>> {
    plan_ref_sync_impl(bare_repo, local, remote, last_synced, direction).await
}
#[cfg(not(any(test, feature = "test-util")))]
async fn plan_ref_sync(
    bare_repo: &std::path::Path,
    local: Option<&str>,
    remote: Option<&str>,
    last_synced: Option<&str>,
    direction: access::SyncDirection,
) -> Result<Option<RefAction>> {
    plan_ref_sync_impl(bare_repo, local, remote, last_synced, direction).await
}
async fn plan_ref_sync_impl(
    bare_repo: &std::path::Path,
    local: Option<&str>,
    remote: Option<&str>,
    last_synced: Option<&str>,
    direction: access::SyncDirection,
) -> Result<Option<RefAction>> {
    Ok(match (local, remote) {
        (Some(_), None) | (None, Some(_)) if last_synced.is_some() => Some(RefAction::DeletedOneSide),
        (Some(l), None) => direction.allows_push().then(|| RefAction::Push(l.to_string())),
        (None, Some(r)) => direction.allows_pull().then(|| RefAction::Pull(r.to_string())),
        (None, None) => None,
        (Some(l), Some(r)) if l == r => (last_synced != Some(l)).then(|| RefAction::Record(l.to_string())),
        // Ancestry decides, never "which side moved since `last_synced`":
        // a side that moved backwards (a force-push on the external forge,
        // a local history rewrite) is *not* something to replay onto the
        // other side just because the other one sat still. Only a genuine
        // fast-forward syncs; anything else is a divergence.
        (Some(l), Some(r)) => {
            if gitquarry_core::git::is_ancestor(bare_repo, l, r).await? {
                direction.allows_pull().then(|| RefAction::Pull(r.to_string()))
            } else if gitquarry_core::git::is_ancestor(bare_repo, r, l).await? {
                direction.allows_push().then(|| RefAction::Push(l.to_string()))
            } else {
                Some(RefAction::Diverged)
            }
        }
    })
}

/// Makes a pulled ref real, not just a local cache hit: `RefAction::Pull`'s
/// `gitquarry_core::git::update_ref` only touches `bare_repo` on disk, but
/// S3 (the actual source of truth every other reader — ssh/http serving,
/// the web UI, and critically `AppContext::open_mirror`'s own
/// `sync_bare_repo_from_s3`) has no idea a pull ever happened until this
/// also runs. Without it, the very next `open_mirror` call *anywhere*
/// (including this same mirror's next sync tick) resyncs `bare_repo` from
/// the still-stale manifest and quietly throws the pulled ref away — from
/// every other caller's perspective, the pull never happened. Same
/// pack-then-CAS-manifest shape `mirror::hook::run_pre_receive_hook` uses
/// for a real push, with the same CAS-conflict retry loop for a concurrent
/// write (a real push landing on this ref at the same moment) racing this
/// one.
async fn persist_pulled_ref_to_s3(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    bare_repo: &Path,
    ref_name: &str,
    sha: &str,
) -> Result<()> {
    for attempt in 1..=MANIFEST_CAS_MAX_RETRIES {
        let (manifest, etag) = resolved.remote.get_manifest().await?;
        let have_tips: Vec<String> = manifest.refs.values().cloned().collect();
        let scratch = gitquarry_core::git::scratch_dir(bare_repo)?;
        let mut uploaded_bytes: u64 = 0;
        if let Some(pack) =
            gitquarry_core::git::pack_objects_for_push(bare_repo, sha, &have_tips, scratch.path()).await?
        {
            // Best-effort size read, same tolerance the pre-receive hook's
            // own accounting has.
            if let Ok(meta) = tokio::fs::metadata(&pack.pack_path).await {
                uploaded_bytes = meta.len();
            }
            resolved
                .remote
                .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
                .await?;
        }
        let mut new_manifest = manifest;
        new_manifest.refs.insert(ref_name.to_string(), sha.to_string());
        if new_manifest.head.is_none() {
            new_manifest.head = Some(ref_name.to_string());
        }
        match resolved.remote.put_manifest(&new_manifest, etag.as_deref()).await? {
            Cas::Ok => {
                // Counted exactly like a push's own packs
                // (`mirror::hook::run_pre_receive_hook`): without this the
                // owner's quota never grows from mirrored data, so the
                // quota check `mirror::policy` applies to a pull could
                // never trip no matter how much arrives that way.
                if uploaded_bytes > 0 {
                    gitquarry_core::storage::settings::bump_pack_bytes(
                        ctx.backend().as_ref(),
                        &ctx.cipher,
                        &resolved.uid,
                        uploaded_bytes,
                    )
                    .await;
                }
                return Ok(());
            }
            Cas::Conflict if attempt < MANIFEST_CAS_MAX_RETRIES => {
                gitquarry_core::storage::cas_retry_backoff(attempt).await;
                continue;
            }
            Cas::Conflict => break,
        }
    }
    bail!(
        "too many CAS conflicts persisting pulled ref '{ref_name}' to S3 (giving up after {MANIFEST_CAS_MAX_RETRIES} attempts)"
    )
}

/// Best-effort per-ref status write. Deliberately not `?` at the call
/// sites: this sweep's contract is that one ref's trouble never stops the
/// rest, and a bucket write failing while *recording* trouble is the worst
/// possible reason to abandon every remaining ref.
async fn record_ref_status(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &access::ExternalMirror,
    ref_name: &str,
    status: RefSyncStatus,
) {
    if let Err(e) = access::set_ref_status(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &resolved.uid,
        &mirror.id,
        ref_name,
        status,
    )
    .await
    {
        eprintln!(
            "gitquarry-mirror-external: warning: couldn't record status for '{ref_name}' on mirror {}: {e:#}",
            mirror.id
        );
    }
}

/// Syncs every ref of one mirror against its `last_synced` memory and the
/// external side's own current state — push/pull/fast-forward per
/// [`plan_ref_sync`], never forcing a genuine divergence. One ref failing (a
/// bad credential, a rejected non-fast-forward on the remote side, a git
/// error mid-fetch, ...) only marks that ref [`RefSyncStatus::Error`] and
/// moves on — never aborts the rest of this mirror's refs, same
/// log-and-continue discipline [`sync_repo_mirrors`] applies one level up
/// for mirrors themselves.
pub async fn sync_mirror_once(
    ctx: &AppContext,
    resolved: &ResolvedRepo,
    mirror: &access::ExternalMirror,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let url = credentialed_url(&mirror.remote_url, &mirror.token);
    // Before `open_mirror`, deliberately: this is the one round trip that
    // needs no local repo at all, and it's the one most likely to be slow
    // (an unreachable or throttling forge). Taking the repo's lock first
    // would make every clone and push of this repo wait behind it.
    let remote_refs = crate::git_remote::ls_remote_refs(&url).await?;

    let local_mirror = ctx.open_mirror(resolved).await?;
    let bare_repo = &local_mirror.path;
    let local_refs = crate::git_remote::list_refs_with_sha(bare_repo).await?;
    // A pulled ref lands in S3 without ever passing through
    // `git receive-pack`, so the repo's own push policy (protected refs,
    // required signatures, blob ceiling, owner quota) has to be applied
    // here or not at all. Loaded once per sweep, and only for a mirror
    // that can actually pull: a push-only one never writes anything local.
    // Kept as a `Result`, not `?`-ed: a config or manifest read failing is
    // a reason to refuse *pulls* (they're what needs the policy), never a
    // reason to abandon this mirror's pushes along with them.
    let gate = match mirror.direction.allows_pull() {
        true => Some(
            gitquarry_core::mirror::policy::InboundGate::load(ctx, resolved)
                .await
                .map_err(|e| format!("{e:#}")),
        ),
        false => None,
    };
    let is_branch_or_tag = |name: &&String| name.starts_with("refs/heads/") || name.starts_with("refs/tags/");
    let ref_names: BTreeSet<String> = local_refs
        .keys()
        .chain(remote_refs.keys())
        .filter(is_branch_or_tag)
        .cloned()
        .collect();

    for ref_name in ref_names {
        let local_sha = local_refs.get(&ref_name).map(String::as_str);
        let remote_sha = remote_refs.get(&ref_name).map(String::as_str);
        let last_synced = mirror.last_synced.get(&ref_name).map(String::as_str);

        // `plan_ref_sync` resolves the remote sha locally in every branch
        // where the two sides differ — `is_ancestor` has to walk both, in
        // either direction, so even a push-only mirror needs the remote's
        // objects to tell "local is ahead" from "the two diverged". Only
        // two cases skip the round trip: the remote doesn't have this ref
        // at all (nothing to fetch), or both sides already agree (nothing
        // to resolve). This is also the *only* fetch on the pull path —
        // `RefAction::Pull` below deliberately doesn't repeat it.
        // A ref present on one side only, with a `last_synced` sha, was
        // deleted — `plan_ref_sync` refuses to touch it either way, so
        // fetching its objects would be a round trip per tick, forever.
        let deleted_one_side = last_synced.is_some() && (local_sha.is_none() || remote_sha.is_none());
        let needs_remote_objects = !deleted_one_side && remote_sha.is_some() && local_sha != remote_sha;
        if needs_remote_objects
            && let Err(e) = crate::git_remote::fetch_ref_from_remote_url(bare_repo, &url, &ref_name).await
        {
            record_ref_status(ctx, resolved, mirror, &ref_name, RefSyncStatus::Error(e.to_string())).await;
            report.errors.push((ref_name.clone(), e.to_string()));
            continue;
        }

        let action = match plan_ref_sync(bare_repo, local_sha, remote_sha, last_synced, mirror.direction).await {
            Ok(action) => action,
            Err(e) => {
                record_ref_status(ctx, resolved, mirror, &ref_name, RefSyncStatus::Error(e.to_string())).await;
                report.errors.push((ref_name.clone(), e.to_string()));
                continue;
            }
        };

        let outcome = match action {
            None => continue,
            Some(RefAction::Record(sha)) => Ok(sha),
            Some(RefAction::Push(sha)) => {
                crate::git_remote::push_to_remote_url(bare_repo, &url, &[format!("{ref_name}:{ref_name}")])
                    .await
                    .inspect(|()| report.pushed += 1)
                    .map(|()| sha)
            }
            Some(RefAction::Pull(sha)) => {
                async {
                    match &gate {
                        Some(Ok(gate)) => {
                            if let Some(reason) = gate.check(ctx, bare_repo, &ref_name, local_sha, &sha).await? {
                                bail!("refused by this repo's own policy: {reason}");
                            }
                        }
                        Some(Err(e)) => bail!("couldn't read this repo's own policy, refusing the pull: {e}"),
                        // Unreachable: a mirror that can't pull never plans
                        // one. Refusing rather than assuming keeps that an
                        // invariant instead of a comment.
                        None => bail!("a mirror that can't pull tried to"),
                    }
                    // No fetch here: every path that reaches a `Pull` has
                    // already gone through the prefetch above (a pull means
                    // the two sides differ, which is exactly its trigger).
                    gitquarry_core::git::update_ref(bare_repo, &ref_name, &sha).await?;
                    persist_pulled_ref_to_s3(ctx, resolved, bare_repo, &ref_name, &sha).await?;
                    report.pulled += 1;
                    Ok(sha)
                }
                .await
            }
            // Both get `RefSyncStatus::Diverged`: the two sides disagree
            // and this sweep refuses to force either one. Giving a deletion
            // its own stored variant would mean migrating an enum for no
            // behavioural difference — the *report* is where they part
            // ways, since only one of them is a failure.
            Some(action @ (RefAction::Diverged | RefAction::DeletedOneSide)) => {
                record_ref_status(ctx, resolved, mirror, &ref_name, RefSyncStatus::Diverged).await;
                match action {
                    RefAction::DeletedOneSide => report.deleted.push(ref_name.clone()),
                    _ => report.diverged.push(ref_name.clone()),
                }
                continue;
            }
        };

        match outcome {
            Ok(sha) => {
                // Logged, not propagated: the ref itself did sync. Losing
                // the bookkeeping only costs a redundant `Record` next
                // tick, which is no reason to drop the refs still queued.
                if let Err(e) = access::update_last_synced(
                    ctx.backend().as_ref(),
                    &ctx.cipher,
                    &resolved.uid,
                    &mirror.id,
                    &ref_name,
                    &sha,
                )
                .await
                {
                    eprintln!(
                        "gitquarry-mirror-external: warning: '{ref_name}' synced but recording it on mirror {} failed: {e:#}",
                        mirror.id
                    );
                }
            }
            Err(e) => {
                record_ref_status(ctx, resolved, mirror, &ref_name, RefSyncStatus::Error(e.to_string())).await;
                report.errors.push((ref_name.clone(), e.to_string()));
            }
        }
    }
    Ok(report)
}
