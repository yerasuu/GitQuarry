//! Struct definitions only — [`ServerSettings`], [`Visibility`], [`OwnerRef`],
//! [`RepoConfig`] — plus [`RepoConfig`]'s own `#[derive(Entity)]` storage
//! shape (collection prefix, key field). Business logic (CRUD wrappers,
//! repo-name/owner/visible indexes) lives in [`super::settings`], which
//! re-exports everything here so existing `crate::storage::settings::RepoConfig`-
//! style paths keep working.
//!
//! `RepoConfig`'s own repo-name index stays hand-rolled in `settings.rs`
//! rather than a `#[entity(index)]` on `full_name`: the generated
//! `find_by_full_name` would fetch the whole `RepoConfig` on every resolve,
//! but the hottest path here (`resolve_repo_name`, called on every repo
//! lookup) only ever needs the uid — the derive's one-size index shape
//! would double the read cost of that path.

use super::uid::Uid;
use s3db::Entity;
use serde::{Deserialize, Serialize};

/// Server config (binds, mirrors dir) editable from the web UI and persisted
/// in the bucket itself. All fields are optional: if not set, the server
/// uses its hardcoded defaults or whatever was passed via CLI.
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    #[serde(default)]
    pub web_bind: Option<String>,
    #[serde(default)]
    pub ssh_bind: Option<String>,
    #[serde(default)]
    pub mirrors_dir: Option<String>,
    /// `crate::mirror::gc::parse_age` syntax (`"3d"`, `"12h"`, ...).
    #[serde(default)]
    pub mirrors_max_age: Option<String>,
    /// `crate::mirror::gc::parse_size` syntax (`"5G"`, `"512M"`, ...).
    #[serde(default)]
    pub mirrors_max_size: Option<String>,
    /// OIDC login for the web UI (admin pages + reading `private` repos as a
    /// matching [`crate::auth::entities::User`]). All four fields are required
    /// together — see [`gitquarry_auth_crypto::oidc::discover`] — or OIDC stays off and
    /// admin pages stay exactly as open as they are without this configured.
    #[serde(default)]
    pub oidc_issuer_url: Option<String>,
    #[serde(default)]
    pub oidc_client_id: Option<String>,
    #[serde(default)]
    pub oidc_client_secret: Option<String>,
    /// Public base URL the browser/IdP redirect back to (can differ from
    /// `--web-bind` behind a reverse proxy). The actual `redirect_uri`
    /// registered with the IdP is `{this}/_callback`.
    #[serde(default)]
    pub oidc_redirect_base_url: Option<String>,
    /// Extra hosts an external mirror (`gitquarry-mirror-external`) may
    /// point at, beyond the ones every build already allows — exact names
    /// or `*.` patterns.
    ///
    /// *Additional*, never a replacement: the public host of each forge
    /// this build has a client for is always allowed
    /// (`gitquarry_forge_clients::PUBLIC_FORGE_HOSTS`, unioned in by
    /// `gitquarry_mirror_external::validate::effective_allowed_hosts`), so a
    /// stock instance mirrors to GitHub/GitLab/Codeberg with nothing to
    /// configure and this field only ever names a self-hosted forge the
    /// instance also trusts.
    ///
    /// What it's for: mirroring makes *the server* open connections to a
    /// host chosen by whoever configures the mirror, from the server's own
    /// network position. Everything outside the built-in set plus this list
    /// is refused — which is what keeps that from reaching an internal
    /// address. Matching is on the name, never on a resolved address: the
    /// ref sync shells out to `git`, which resolves inside its own process,
    /// so an address check would be a TOCTOU with DNS rebinding behind it.
    #[serde(default, alias = "mirror_allowed_hosts")]
    pub mirror_extra_hosts: Vec<String>,
    /// Where scheduled backups go — `bucket:<name>` (a section of the local
    /// `gitquarry.toml`), `path:/abs/dir`, or `self`. Parsed by
    /// [`crate::backup::dest::Dest::parse`]; never credentials, since this
    /// object lives inside the very bucket being backed up (see that
    /// module's own doc comment).
    #[serde(default)]
    pub backup_dest: Option<String>,
    /// Whether the periodic backup job runs at all. Off by default: a
    /// backup writes to somewhere an operator hasn't necessarily chosen yet,
    /// so it starts only once someone says so.
    #[serde(default)]
    pub backup_enabled: bool,
    /// `crate::mirror::gc::parse_age` syntax (`"24h"`, `"7d"`, ...), same as
    /// [`Self::mirrors_max_age`]. `None` uses
    /// `crate::backup::schedule::DEFAULT_INTERVAL`.
    #[serde(default)]
    pub backup_interval: Option<String>,
    /// Per-class opt-outs. `None` means "whatever
    /// [`crate::backup::inventory::Selection::default`] says", which is
    /// everything except the derived classes — so an instance that never
    /// touches these gets a sensible backup, and a `false` here is always a
    /// deliberate choice someone made.
    #[serde(default)]
    pub backup_include_git: Option<bool>,
    #[serde(default)]
    pub backup_include_lfs: Option<bool>,
    #[serde(default)]
    pub backup_include_releases: Option<bool>,
    #[serde(default)]
    pub backup_include_avatars: Option<bool>,
    #[serde(default)]
    pub backup_include_entities: Option<bool>,
    #[serde(default)]
    pub backup_include_audit: Option<bool>,
    /// Caches and rebuildable indexes (`search-index/`, `commit-sig/`, the
    /// four indexes `backfill-repo-index` repopulates). Off unless set: a
    /// restore pairs with that command plus a re-index instead, which is
    /// much cheaper than carrying one cache object per commit on every run.
    #[serde(default)]
    pub backup_include_derived: Option<bool>,
    /// Whether each run also takes a re-sealed snapshot
    /// (`crate::backup::snapshot`) of the mutable config set. `None` means
    /// yes: it's the only half of the backup that keeps *history*, and it's
    /// small.
    #[serde(default)]
    pub backup_snapshot: Option<bool>,
    /// How many snapshots to keep. `None` keeps every one — deleting a
    /// snapshot destroys the only copy of what the live data used to be, so
    /// it doesn't happen unless an operator asks for it.
    #[serde(default)]
    pub backup_keep: Option<u32>,
    /// How many months of `.conf/audit/` to keep at the destination. `None`
    /// keeps all of them. The audit trail is the one class that grows without
    /// bound, which is why it gets a retention knob the other classes don't.
    #[serde(default)]
    pub backup_audit_keep_months: Option<u32>,
    /// How long a key deleted at the source survives at the replica before
    /// the (opt-in) prune pass removes it. `None` disables that pass
    /// entirely, which is the default: never propagating a deletion is what
    /// makes an accidental purge recoverable from the backup at all.
    #[serde(default)]
    pub backup_delete_grace_days: Option<u32>,
    /// Concurrent object copies per run. `None` uses
    /// `crate::backup::replica::DEFAULT_CONCURRENCY`.
    #[serde(default)]
    pub backup_concurrency: Option<u32>,
    /// The footer's "source" link — either an absolute `http(s)://` URL, or
    /// a `namespace/name` repo already hosted on this instance (rendered as
    /// `/namespace/name`). `None` hides the link entirely rather than
    /// guessing at one: this project's own repo layout (a fork's included)
    /// varies too much per deployment to default to anything real.
    #[serde(default)]
    pub source_link: Option<String>,
}

/// Pijul-Nest/Forgejo-style repo visibility. The two git protocol
/// transports (`crate::git::ssh`, `crate::git::http`) are untouched by any
/// of this — both already require an explicit per-repo
/// [`crate::auth::entities::Permission`] grant regardless of tier, so they're
/// already gated in the sense that matters there, with one asymmetry:
/// `ssh://` can't have a fully anonymous caller (opening a connection at
/// all needs a pubkey that resolves to a user), while `crate::git::http`
/// explicitly allows reading a `Public` repo (`info/refs`/upload-pack, never
/// a push) with no credentials at all — the same "public repo clones with
/// no login" behavior a browser gets on the web UI. What differs by tier is
/// only the web browsing UI (which has a login, unlike a bare `ssh://`
/// connection):
/// - `Public`: no gate anywhere.
/// - `Limited`: on the web, visible to any logged-in user regardless of
///   whether they have a specific grant on this repo, hidden from anonymous
///   requests and the anonymous index.
/// - `Private`: on the web, visible only via `effective_permission`/
///   `is_admin`, same as before `Limited` existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Public,
    Limited,
    Private,
}

/// Who a repo belongs to — a [`crate::auth::entities::User`] or
/// [`crate::auth::entities::Group`], named by uid rather than by the
/// namespace segment of the repo's own path (those two aren't guaranteed
/// to agree: an admin can transfer ownership via `webui::repo_settings::save_conf`
/// without moving the repo). Internally tagged (`{"kind":"user","uid":"..."}`)
/// rather than untagged/adjacently-tagged: matches this codebase's plain,
/// explicit JSON style elsewhere (no serde trickery anywhere else in the tree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "uid", rename_all = "lowercase")]
pub enum OwnerRef {
    User(Uid),
    Group(Uid),
}

/// A repo's own config (unlike [`ServerSettings`], which is global to the
/// whole bucket). Lives at `<repo>/.conf/config.json`, alongside the rest of
/// that repo's objects.
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq, Entity)]
// Literal duplicates `super::REPOS_PREFIX` -- `#[entity(collection = ...)]`
// needs a string literal, can't reference a const.
#[entity(collection = "repos", suffix = "/.conf/config.json")]
pub struct RepoConfig {
    /// Stable id, minted once at repo creation (`AppContext::create_repo`)
    /// — the S3 storage prefix itself is `repos/<uid>/...`. See
    /// [`crate::storage::uid::Uid`]'s doc comment for why.
    #[serde(default)]
    #[entity(key)]
    pub uid: Uid,
    /// Canonical current path (`namespace/slug`) — the repo-name index key,
    /// the ssh/http clone path segment, and the S3-prefix-adjacent name.
    /// Always [`crate::validate::sanitize_path`]-clean; free-text naming
    /// lives in `display_name` below, derived into this via
    /// `domain::repo::slugify_repo_name`. Once the physical layout is
    /// uid-keyed this is the only place a repo's path lives — renaming/
    /// moving becomes rewriting this field plus the `repo-name` index,
    /// never relocating any git data.
    #[serde(default)]
    pub full_name: String,
    /// Free-text public name shown as the repo's page heading — any
    /// characters, unlike `full_name`. `None` for a repo that predates this
    /// field: `domain::repo::backfill_slug` fills it in (from the current
    /// `full_name`'s local segment) the first time that repo is loaded by a
    /// uid-based listing, since it can't be reached by path if `full_name`
    /// itself has since become invalid under `sanitize_path`'s charset.
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// The user or group this repo belongs to — a uid, not a name, so a
    /// rename of that user/group (were one ever added) couldn't strand
    /// this. Display name is resolved via `auth::access::resolve_owner_display`.
    #[serde(default)]
    pub owner: Option<OwnerRef>,
    /// Defaults to `Public`: existing repos created before this field
    /// existed must stay exactly as visible as they already were, not
    /// suddenly vanish from the index on upgrade.
    #[serde(default)]
    pub visibility: Visibility,
    /// Rejects pushes at the pre-receive hook. Repo stays readable/cloneable.
    #[serde(default)]
    pub archived: bool,
    /// Unix seconds of when `domain::repo::delete` soft-deleted this repo,
    /// `None` while live. A repo with this set is invisible everywhere (git
    /// ssh/http, `.conf/index/repo-visible`) except its own settings page,
    /// which shows a restore action instead of the normal form set —
    /// `domain::repo::restore` clears this back to `None`. Past
    /// `domain::repo::DELETE_GRACE_SECS`, the background sweep
    /// (`mirror::deleted_repo_sweep`) calls `domain::repo::purge`, which does
    /// the actual irreversible wipe this field used to trigger directly.
    #[serde(default)]
    pub deleted_at: Option<u64>,
    /// Unix seconds of the last successful manifest write — set by
    /// `storage::settings::touch_last_push`, called from both
    /// `mirror::hook::run_pre_receive_hook` (a real push) and
    /// `gitquarry-mirror-external`'s sync engine (a pull landing new refs):
    /// either one is "this repo's content changed", not just a human push.
    /// `None` for a repo nothing has ever pushed to. Best-effort like
    /// [`Self::pack_bytes`] — a write that loses a CAS race here just means
    /// the home page's "updated" stamp is briefly stale, never wrong in a
    /// way that matters.
    #[serde(default)]
    pub last_push: Option<u64>,
    /// Commit count for the same push [`Self::last_push`] stamps — how many
    /// new commits `git rev-list --count` found in that push's `old..new`
    /// range (or just `new` for a branch that didn't exist before). Set
    /// alongside `last_push`, same best-effort tolerance, same two callers.
    /// Purely for the home page's "N commits pushed to `<branch>`" activity
    /// line — nothing else reads it, so a stale/missing value there is the
    /// entire blast radius.
    #[serde(default)]
    pub last_push_commits: Option<u32>,
    /// Short name (`refs/heads/` stripped) of the branch [`Self::last_push`]
    /// landed on — always the repo's default branch, since that's the only
    /// branch this ever fires for, but stored rather than re-derived so the
    /// home page's activity line doesn't need its own `Manifest` fetch just
    /// to name it.
    #[serde(default)]
    pub last_push_branch: Option<String>,
    /// Best-effort running total of this repo's own `.pack` bytes —
    /// incremented alongside each successful pack upload
    /// (`mirror::hook::run_pre_receive_hook`), never decremented from there.
    /// Unlike [`Self::lfs_bytes`], this one *does* get corrected exactly:
    /// `gitquarry repack <repo> --apply` already computes the real on-disk
    /// pack total (`commands::repack::pack_stats`) to report its own
    /// before/after, and overwrites this field with that real number —
    /// drift self-heals downward the next time someone repacks. Summed with
    /// `lfs_bytes` per owner by `auth::access::quota::check`; see that
    /// module's own doc comment for why this is a soft "already over"
    /// gate, not a byte-exact ledger.
    #[serde(default)]
    pub pack_bytes: u64,
    /// Best-effort running total of this repo's own LFS object bytes —
    /// incremented once per genuinely *new* oid (`git::http`'s LFS upload
    /// handler checks `Remote::lfs_object_exists` first, so a client
    /// re-uploading a hash it already pushed elsewhere never double-counts
    /// it here). No reconciliation command exists for this one the way
    /// [`Self::pack_bytes`] has `repack`; drift only ever comes from a
    /// crashed/interrupted upload counted before the write actually landed,
    /// same tolerance every other best-effort counter here already accepts.
    #[serde(default)]
    pub lfs_bytes: u64,
    /// Usernames who must have approved (their latest review verdict is
    /// `ReviewVerdict::Approved`) before `domain::merge_request::merge` lets
    /// an MR land — on top of, never instead of, whatever a `CODEOWNERS`
    /// file at the target branch's root additionally requires for the
    /// specific paths an MR touches (see
    /// `domain::merge_request::missing_required_reviewers`). Empty means no
    /// repo-wide requirement (the default): an MR just needs no pending
    /// `ChangesRequested`, same as before this field existed.
    #[serde(default)]
    pub required_reviewers: std::collections::BTreeSet<String>,
    /// Marks this repo as available to create new repos *from* — offered
    /// at `/_new` as a "start from this template" option to anyone who can
    /// view it. Unlike `domain::repo::fork`, `domain::repo::create_from_template`
    /// (the same underlying `Remote::copy_all_to` duplication) sets no
    /// `forked_from` link and doesn't bump `fork_count`: the new repo is
    /// its own independent history from the start, same "generated from
    /// template" distinction GitHub itself draws from a real fork. `false`
    /// by default, and always `false` on the newly-created copy — a
    /// template-derived repo isn't itself a template unless someone
    /// explicitly marks it one afterward.
    #[serde(default)]
    pub is_template: bool,
    /// Branch short names (no `refs/heads/` prefix). Enforced at the
    /// pre-receive hook: no force-push, no delete, and only an owner (repo
    /// owner / group owner / admin) may push directly at all — see
    /// `mirror::hook`.
    #[serde(default)]
    pub protected_branches: std::collections::BTreeSet<String>,
    /// Same idea as `protected_branches` but for tags, and stricter: a tag
    /// is never fast-forwarded, so any move (not just a non-ancestor one)
    /// is rejected once protected, not just a delete.
    #[serde(default)]
    pub protected_tags: std::collections::BTreeSet<String>,
    /// Branch short names (no `refs/heads/` prefix, same shape as
    /// `protected_branches`, an independent set — a branch can be one,
    /// both, or neither) where every newly-pushed commit must carry a GPG
    /// signature that verifies against a registered account's own
    /// `auth::entities::User::gpg_keys` — never merely "any valid
    /// signature" (`%G?` alone can't prove *which* gitquarry account it
    /// belongs to, if any). Enforced at the pre-receive hook via
    /// `domain::commit_signature::verified_signer`, same "resolved once,
    /// cached by sha" path a commit's "verified signature (username)" badge
    /// already uses to render.
    #[serde(default)]
    pub require_signed_commits: std::collections::BTreeSet<String>,
    /// Rejects pushes at the pre-receive hook that introduce a blob over
    /// this size and not already converted to an LFS pointer client-side
    /// (`crate::mirror::gc::parse_size` syntax, e.g. `"50M"`). `None` (the
    /// default) leaves the feature off for this repo — some repos have
    /// legitimate large binaries, others don't, so this is opt-in per repo
    /// rather than a server-wide default. Enforcement itself lives in
    /// `mirror::hook`, not here.
    #[serde(default)]
    pub lfs_max_blob_size: Option<String>,
    /// Free-text labels for discovery/categorization (`rust`, `cli`, ...) —
    /// purely cosmetic, no bearing on permissions/visibility. Normalized to
    /// lowercase `[a-z0-9-]` at the settings form so a topic is always safe
    /// to drop straight into a `?q=` link without extra escaping there.
    #[serde(default)]
    pub topics: std::collections::BTreeSet<String>,
    /// Uid of the repo this one was forked from, set once at fork time (see
    /// `webui::fork`) and never edited afterward — unlike `owner`, there's no
    /// settings-form field for this, a fork's lineage isn't something to
    /// reassign after the fact. Resolving it to a display name (see
    /// `webui::repo::summary::render_summary`) fetches that repo's own live
    /// `RepoConfig`, which both recovers its current name after any number
    /// of renames and detects deletion (the fetch just comes back empty) —
    /// the fix for the dangling "forked from" link a plain name string used
    /// to leave with no existence check at all.
    #[serde(default)]
    pub forked_from: Option<Uid>,
    /// Set instead of `forked_from` when the source repo lives on a linked
    /// peer instance (federation layer 2, `docs/federation-concept.md`) —
    /// there's no local `Uid` for a repo this bucket never created. Additive
    /// field, `#[serde(default)]`: every `RepoConfig` written before this
    /// existed just deserializes with `None` here, same as any other field
    /// added over time on this struct.
    #[serde(default)]
    pub forked_from_peer: Option<PeerOrigin>,
    /// How many repos have this one as their `forked_from`. Maintained
    /// incrementally (best-effort CAS bump on the source repo each time
    /// `webui::fork` creates a new fork — see there) rather than computed by
    /// scanning every repo in the bucket on every summary-page render,
    /// which is what this replaced. Same "doesn't need to stay in sync
    /// with reality" tolerance as `User::starred`/star counts elsewhere: a
    /// lost race under concurrent forks just undercounts by one, self-heals
    /// on the next successful fork.
    #[serde(default)]
    pub fork_count: u32,
    /// How many users have this repo in their `User::starred`. Maintained
    /// incrementally (best-effort CAS bump alongside the star/unstar itself
    /// — see `auth::web::favorites`) rather than computed by scanning every user
    /// on every repo-list/profile/summary render, which is what this
    /// replaced. Same "doesn't need to stay in sync with reality" tolerance
    /// as `fork_count`: a lost race under concurrent star/unstar just drifts
    /// by one, self-heals on the next successful toggle.
    #[serde(default)]
    pub star_count: u32,
    /// How many users have this repo in their `User::watched`. Same
    /// incremental-best-effort maintenance as `star_count` — see
    /// `domain::repo::toggle_watch`.
    #[serde(default)]
    pub watcher_count: u32,
    /// The repo's dominant language by byte count, cached here rather than
    /// computed live on every index-page load (that would mean a full tree
    /// walk — `git::browse::language_stats` — for every visible repo, every
    /// time anyone loads `/`, unlike the summary page's own call which only
    /// ever runs for the one repo being viewed). `None` until the repo's
    /// next push: best-effort, updated detached after a successful
    /// `receive-pack` (`git::ssh`/`git::http`) alongside the mirror re-sync
    /// already happening there, same "never blocks the push response, a
    /// lost race just waits for the next push" tolerance `star_count`/
    /// `watcher_count`/`fork_count` above already have.
    #[serde(default)]
    pub primary_language: Option<String>,
    /// This repo's issue-label vocabulary — a small, whole-repo list edited
    /// via the settings page (`webui::repo_settings::labels`), same
    /// full-replace-on-save UX as `topics`. Unlike `topics` (a bare
    /// `BTreeSet<String>`), a label also carries a color, so it's a `Vec` of
    /// its own small struct rather than reusing that shape.
    #[serde(default)]
    pub labels: Vec<Label>,
}

/// One entry in a repo's issue-label vocabulary. `color` is always
/// `#rrggbb` (validated by `validate::validate_hex_color` at the settings
/// form) so it can be dropped straight into a label pill's
/// `style="background:..."` without further checking there.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    pub color: String,
}

/// Where a peer-forked repo came from — the peer's `Peer::name` (not its
/// `web_base_url`: the peer might get re-linked at a different URL later,
/// the name is the stable label) plus the repo's full name on that peer at
/// fork time. Display-only, same as a plain `forked_from` link: never
/// re-resolved live, never a basis for any permission decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerOrigin {
    pub peer: String,
    pub repo: String,
}
