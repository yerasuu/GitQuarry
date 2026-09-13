//! Which class every object in the bucket belongs to — the one place that
//! knows what a whole-instance backup has to carry, so a new collection
//! can't silently stop being backed up somewhere else in the tree.
//!
//! Classification is by key *shape*, not by a hand-maintained list of every
//! `Collection` in the workspace: a list like that goes stale the moment
//! someone adds a per-repo collection and forgets this file, and the failure
//! mode is silent (the object just never gets copied). Matching on the shape
//! means a new key under an already-known prefix lands in the right class
//! for free, and anything genuinely new lands in [`Class::Unknown`] — which
//! is **copied and reported**, never skipped. Fail-safe in the direction
//! that loses no data: an operator sees "N objects of unknown class" in the
//! run report and can come classify them, instead of discovering at restore
//! time that a whole feature's data was never in the backup.

/// What a key is, for backup purposes. Order of the variants is meaningless;
/// what matters is [`classify`]'s ordering of the checks that produce them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `.conf/encryption.json` — the salt plus the wrapped data key
    /// (`s3db::bootstrap`). Not a normal class: it's the *ordered
    /// prerequisite* of a replica copy. It must be the first object written
    /// to a virgin destination, because `s3db::encryption_conf::
    /// ensure_safe_to_initialize` refuses to bootstrap a fresh encryption
    /// config into a bucket that already has objects in it — write it last
    /// and the destination is bricked. Copied verbatim, unlike everything
    /// else: it's the one object that isn't sealed under the data key,
    /// because it's what holds that key.
    Bootstrap,
    /// `refs.json` plus `objects/pack/*` — a repo's git data.
    Git,
    /// `lfs/<oid>` — content-addressed, so never rewritten.
    Lfs,
    /// `releases/<tag>/**` — assets immutable under a fixed tag, `meta.json`
    /// mutable.
    Releases,
    /// Top-level `avatars/` (see `app::AppContext::avatar_remote`) — a
    /// sibling of `repos/`, not nested under any repo.
    Avatars,
    /// Small mutable JSON: server settings, host/signing keys, users,
    /// groups, PATs, repo configs, MR threads, mirror configs, and every
    /// index with no rebuild path (see [`classify_index`]).
    Entities,
    /// `.conf/audit/<month>/*` — append-only and unbounded, which is why it
    /// gets its own retention instead of the replica's never-prune rule.
    Audit,
    /// Caches and indexes that can be regenerated from the classes above:
    /// per-repo search indexes, the commit-signature cache, and the four
    /// indexes `gitquarry backfill-repo-index` repopulates. Skippable by
    /// configuration — the two that actually cost anything at scale are
    /// `search-index/` (per repo, grows with the tree) and `commit-sig/`
    /// (one object per commit ever verified).
    Derived,
    /// Transient or self-referential: s3db's own transaction/WAL keys,
    /// snapshot leases, and the backup system's own output. Copying these
    /// would at best be pointless and at worst make a backup back up its
    /// own previous backups.
    Never,
    /// Matched nothing. Copied anyway and counted in the run report — see
    /// this module's own doc comment for why that's the safe direction.
    Unknown,
}

impl Class {
    /// Every class that can actually appear in a run report, in a stable
    /// order the settings page renders in. [`Class::Never`] is left out:
    /// nothing is ever copied under it, so a report row for it would always
    /// read zero.
    pub const REPORTABLE: [Class; 8] = [
        Class::Bootstrap,
        Class::Git,
        Class::Lfs,
        Class::Releases,
        Class::Avatars,
        Class::Entities,
        Class::Audit,
        Class::Derived,
    ];

    /// Stable machine name — the key a [`crate::backup::report::BackupRun`]'s
    /// per-class counters are stored under, so it must not change once
    /// written: a rename would orphan every stored report's counters.
    pub fn label(self) -> &'static str {
        match self {
            Class::Bootstrap => "bootstrap",
            Class::Git => "git",
            Class::Lfs => "lfs",
            Class::Releases => "releases",
            Class::Avatars => "avatars",
            Class::Entities => "entities",
            Class::Audit => "audit",
            Class::Derived => "derived",
            Class::Never => "never",
            Class::Unknown => "unknown",
        }
    }
}

/// Indexes `gitquarry backfill-repo-index` can rebuild from the entities
/// they point at. Deliberately *only* these four: the command's own usage
/// text names exactly them, and every other index under `.conf/index/`
/// (`namespace`, `pubkey`, `secondary-email`, `pat-owner`, `mirrors-enabled`,
/// `repo-deleted`) has no rebuild path at all — an instance restored without
/// `pubkey`/`namespace` can't authenticate anyone, which is a correctness
/// problem, not a cost tradeoff. Those stay [`Class::Entities`], and they're
/// small enough that carrying them always costs nothing worth measuring.
const REBUILDABLE_INDEXES: [&str; 4] = ["repo-name/", "repo-owner/", "repo-visible/", "repo-permission/"];

/// The class of one storage key. Total: every key gets a class, and the
/// fallback is [`Class::Unknown`] (copied) rather than a silent skip.
pub fn classify(key: &str) -> Class {
    // A zero-byte "folder" placeholder some S3 consoles create when
    // browsing a path. gitquarry never writes one, at least one provider
    // rejects a `PutObject` whose key ends in `/`, and it holds nothing —
    // same reasoning `storage::remote::Remote::copy_all_to` already drops
    // them on.
    if key.is_empty() || key.ends_with('/') {
        return Class::Never;
    }
    if key == s3db::bootstrap::ENCRYPTION_META_KEY {
        return Class::Bootstrap;
    }
    if let Some(rest) = key.strip_prefix(".conf/") {
        return classify_conf(rest);
    }
    if let Some(rest) = key.strip_prefix("repos/") {
        return classify_repo(rest);
    }
    if key.strip_prefix("avatars/").is_some_and(|rest| !rest.is_empty()) {
        return Class::Avatars;
    }
    // `gitquarry-ci`'s own top-level namespace (`docs/ci-cd-concept.md`):
    // `ci/<repo_uid>/runs` pipeline-run records. No rebuild path, so stays
    // `Entities` rather than `Derived` — same "never silently dropped"
    // default every other real data class gets.
    if key.strip_prefix("ci/").is_some_and(|rest| !rest.is_empty()) {
        return Class::Entities;
    }
    Class::Unknown
}

/// `rest` is everything after `repos/`, i.e. `<uid>/<tail>`.
fn classify_repo(rest: &str) -> Class {
    // The uid segment itself carries no class information — every repo has
    // the same internal layout, so only the tail matters.
    let Some((_uid, tail)) = rest.split_once('/') else {
        return Class::Unknown;
    };
    if tail == crate::storage::remote::MANIFEST_SUFFIX || tail.starts_with("objects/pack/") {
        return Class::Git;
    }
    // `lfs/` before `lfs-locks/` reads like an ordering trap but isn't:
    // `starts_with("lfs/")` can't match `lfs-locks/...`. Both are listed
    // explicitly anyway, so neither depends on the other's position.
    if tail.starts_with("lfs/") {
        return Class::Lfs;
    }
    if tail.starts_with("releases/") {
        return Class::Releases;
    }
    if tail == ".conf/config.json"
        || tail.starts_with("lfs-locks/")
        || tail.starts_with("merges/")
        || tail.starts_with("mirrors/")
    {
        return Class::Entities;
    }
    if tail.starts_with("search-index/") {
        return Class::Derived;
    }
    Class::Unknown
}

/// `rest` is everything after `.conf/`.
fn classify_conf(rest: &str) -> Class {
    if matches!(
        rest,
        "global.json" | "ssh_host_key.json" | "signing_key.json" | "data-version.json"
    ) {
        return Class::Entities;
    }
    if rest.starts_with("users/") || rest.starts_with("groups/") || rest.starts_with("pat/") {
        return Class::Entities;
    }
    if rest.starts_with("audit/") {
        return Class::Audit;
    }
    if rest.starts_with("commit-sig/") {
        return Class::Derived;
    }
    if let Some(index_rest) = rest.strip_prefix("index/") {
        return classify_index(index_rest);
    }
    if rest.starts_with("backups/")
        || rest.starts_with("backup-runs/")
        // The destination-side orphan ledger (`crate::backup::prune`). It
        // only ever exists at a backup destination, but a replica promoted
        // to a live bucket and backed up onward would otherwise carry it as
        // real data.
        || rest.starts_with("backup-orphans/")
        || rest.starts_with("snapshots/")
        || rest.starts_with("txn/")
        || rest.starts_with("wal/")
    {
        return Class::Never;
    }
    // `<collection>.index/` — the sibling prefix `#[entity(index)]` mints
    // for a derived lookup (e.g. `.conf/users.index/uid/<uid>.json`). No
    // rebuild command exists for any of them, and they're pointer-sized.
    if rest.split('/').next().is_some_and(|seg| seg.ends_with(".index")) {
        return Class::Entities;
    }
    Class::Unknown
}

/// `rest` is everything after `.conf/index/`.
fn classify_index(rest: &str) -> Class {
    if matches!(rest, "password-login-count.json" | "signing-epoch.json") {
        return Class::Entities;
    }
    if REBUILDABLE_INDEXES.iter().any(|prefix| rest.starts_with(prefix)) {
        return Class::Derived;
    }
    // Not in the rebuildable set: carried. An index nobody can regenerate
    // is data, whatever it looks like — see [`REBUILDABLE_INDEXES`].
    Class::Entities
}

/// Whether `key` names an object that, once written, is never rewritten in
/// place — which is what makes "already at the destination, same size" a
/// sound reason to skip it.
///
/// Deliberately per *key*, not per class: `Class::Git` covers both
/// `objects/pack/*` (named after its own content hash, so a same-named pack
/// is the same pack) and `refs.json` (rewritten on every single push).
/// Getting this wrong in the permissive direction is the worst bug this
/// system can have — a mutable object wrongly called immutable stops being
/// copied the moment its size happens to match, and the replica goes
/// silently stale — so anything not positively known to be immutable is
/// treated as mutable and re-copied every run.
pub fn is_immutable(key: &str) -> bool {
    if let Some(rest) = key.strip_prefix("repos/") {
        let Some((_uid, tail)) = rest.split_once('/') else {
            return false;
        };
        // Packs and their indexes are named `pack-<sha>`; LFS objects by
        // their own oid; release assets are frozen under an immutable tag.
        return tail.starts_with("objects/pack/")
            || tail.starts_with("lfs/")
            || (tail.starts_with("releases/") && !tail.ends_with("/meta.json"));
    }
    // Audit entries are create-only, one key per event
    // (`storage::audit::record`) — never read-modify-written like every
    // other `.conf/` object.
    key.starts_with(".conf/audit/")
}

/// Which classes a run copies, derived from `ServerSettings`' own
/// `backup_include_*` fields at the call site rather than read from them
/// here: this crate's backup modules take a `Selection`, so a CLI run
/// (`gitquarry backup`) can pass a selection that never went through
/// settings at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub git: bool,
    pub lfs: bool,
    pub releases: bool,
    pub avatars: bool,
    pub entities: bool,
    pub audit: bool,
    pub derived: bool,
}

impl Default for Selection {
    /// Everything except [`Class::Derived`] — the defaults the settings page
    /// itself starts from, so a bucket that never configured a backup and a
    /// bare `gitquarry backup` agree on what "a backup" means.
    fn default() -> Self {
        Selection {
            git: true,
            lfs: true,
            releases: true,
            avatars: true,
            entities: true,
            audit: true,
            derived: false,
        }
    }
}

impl Selection {
    /// The selection an instance's own settings ask for. Every unset field
    /// falls back to [`Selection::default`]'s answer for that class, so a
    /// bucket whose `global.json` predates these fields backs up exactly
    /// what a freshly-configured one would.
    pub fn from_settings(settings: &crate::storage::entities::ServerSettings) -> Self {
        let d = Selection::default();
        Selection {
            git: settings.backup_include_git.unwrap_or(d.git),
            lfs: settings.backup_include_lfs.unwrap_or(d.lfs),
            releases: settings.backup_include_releases.unwrap_or(d.releases),
            avatars: settings.backup_include_avatars.unwrap_or(d.avatars),
            entities: settings.backup_include_entities.unwrap_or(d.entities),
            audit: settings.backup_include_audit.unwrap_or(d.audit),
            derived: settings.backup_include_derived.unwrap_or(d.derived),
        }
    }

    /// Only the classes needed to bring an instance back up, nothing that
    /// grows without bound — what `--minimal` on the CLI passes.
    pub fn minimal() -> Self {
        Selection {
            git: true,
            lfs: true,
            releases: true,
            avatars: true,
            entities: true,
            audit: false,
            derived: false,
        }
    }

    pub fn includes(&self, class: Class) -> bool {
        match class {
            // Never optional: without it the destination can't be opened at
            // all, and it must be written before anything else.
            Class::Bootstrap => true,
            Class::Git => self.git,
            Class::Lfs => self.lfs,
            Class::Releases => self.releases,
            Class::Avatars => self.avatars,
            Class::Entities => self.entities,
            Class::Audit => self.audit,
            Class::Derived => self.derived,
            Class::Never => false,
            // See the module doc: unclassified data is carried, not dropped.
            Class::Unknown => true,
        }
    }
}
