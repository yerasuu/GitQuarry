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
    /// matching [`crate::auth::access::User`]). All four fields are required
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
}

/// Pijul-Nest/Forgejo-style repo visibility. The two git protocol
/// transports (`crate::git::ssh`, `crate::git::http`) are untouched by any
/// of this — both already require an explicit per-repo
/// [`crate::auth::access::Permission`] grant regardless of tier, so they're
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

/// Who a repo belongs to — a [`crate::auth::access::User`] or
/// [`crate::auth::access::Group`], named by uid rather than by the
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
    /// Canonical current display name (`namespace/local-name`). Once the
    /// physical layout is uid-keyed this is the only place a repo's name
    /// lives — renaming/moving becomes rewriting this field plus the
    /// `repo-name` index below, never relocating any git data.
    #[serde(default)]
    pub full_name: String,
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
    /// `ReviewVerdict::Approved`) before `domain::pull_request::merge` lets
    /// a PR land — on top of, never instead of, whatever a `CODEOWNERS`
    /// file at the target branch's root additionally requires for the
    /// specific paths a PR touches (see
    /// `domain::pull_request::missing_required_reviewers`). Empty means no
    /// repo-wide requirement (the default): a PR just needs no pending
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
