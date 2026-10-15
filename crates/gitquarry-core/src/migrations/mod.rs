//! Bucket-wide data migrations, applied automatically at server startup.
//!
//! `s3db::migrate` already versions and rewrites *one collection* — but its
//! transform is synchronous and per-entry, so it can't express a change that
//! spans collections (fold a group's membership *and* write per-repo grants
//! onto the affected users *and* fill an index). That's what lives here:
//! each migration is a plain async pass over whatever it needs, and the
//! bookkeeping is one counter for the whole bucket rather than one per
//! collection.
//!
//! **The counter is a data revision, not a release number.** A stored
//! [`DataVersion::version`] of `N` means "every migration up to and
//! including `N` has been applied to this bucket"; [`CURRENT_DATA_VERSION`]
//! is what this binary knows how to produce. Deriving it from
//! `CARGO_PKG_VERSION` would tie a data shape to a version bump that usually
//! has nothing to do with it, and would have no answer for two releases that
//! both need a migration.
//!
//! **Every migration must be idempotent.** The version is stamped only
//! *after* a pass completes, so a crash (or a second instance starting at
//! the same moment) re-runs it. Each one therefore has to be safe to apply
//! to already-migrated data — in practice: raise, never lower; write index
//! entries that may already exist; skip what's already in the target shape.
//!
//! **A pass that can't finish cleanly does not stamp the version.** A CAS
//! conflict against a concurrent edit leaves the *old* data intact and still
//! working (see [`group_members`]'s own doc comment on why the legacy fields
//! keep granting), so the honest answer is to leave the version alone and
//! let the next startup retry, rather than declare the bucket migrated with
//! entries left behind.

pub mod group_members;
pub mod repo_permission_index;

use crate::crypto::Cipher;
use crate::storage::{Backend, Cas};
use anyhow::{Context, Result};
use s3db::Singleton;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The data revision this binary produces. Bump it in the same commit that
/// adds a migration to [`MIGRATIONS`], never on its own.
pub const CURRENT_DATA_VERSION: u32 = 2;

/// Deliberately not `.schema.json`, which is `s3db::migrate`'s own
/// per-collection bookkeeping key — this is the bucket-wide counter, and two
/// files with the same name meaning different things is exactly the
/// confusion worth spending a hyphen to avoid.
const DATA_VERSION: Singleton = Singleton::new(".conf/data-version.json");

/// One entry per migration this bucket has applied — an audit trail, not
/// something the runner reads back. [`DataVersion::version`] alone decides
/// what still has to run.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppliedMigration {
    pub version: u32,
    pub name: String,
    /// `storage::audit::format_unix_secs`-formatted, same as every other
    /// stored timestamp in this tree.
    pub at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DataVersion {
    /// Every migration up to and including this number has been applied.
    /// `0` — the default a bucket with no such object reads as — means none
    /// have, which is also correct for a bucket predating this mechanism.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub applied: Vec<AppliedMigration>,
}

/// A migration's identity. The pass itself is dispatched by version in
/// [`run_one`] rather than stored as a function pointer here: an `async fn`
/// isn't nameable in a `const` slice, and boxing one per entry buys nothing
/// for a list this short.
#[derive(Debug)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
}

/// In ascending version order — the runner applies them in exactly this
/// sequence and stamps after each one, so a partially-migrated bucket
/// resumes at the first version it hasn't reached.
///
/// - `1` `group-members` (0.5.0): folds a group's pre-`Group::members`
///   `owners`/`teams` into its member map, converts a team scoped to some of
///   the group's repos into explicit per-repo grants, and fills the
///   `user-group` index.
/// - `2` `repo-permission-index` (0.6.0): indexes every grant already held
///   in `User::repos`, which the admin user page and account creation used
///   to write without the index the repo's permission grid reads.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "group-members",
    },
    Migration {
        version: 2,
        name: "repo-permission-index",
    },
];

/// What one pass did (or, in [`Mode::Report`], would do). `incomplete` is the
/// count of items the pass couldn't finish — a CAS conflict against a
/// concurrent edit, an entity that vanished mid-pass — and is what decides
/// whether the version gets stamped.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    /// Human-readable, one line, for the startup log and the CLI report.
    pub summary: String,
    pub changed: usize,
    pub incomplete: usize,
}

impl Outcome {
    /// Nothing to do — lets a migration answer "already applied" without
    /// inventing a summary string every caller then has to special-case.
    pub fn clean() -> Self {
        Outcome::default()
    }
}

/// Whether a pass writes or only counts. `Report` is what `gitquarry migrate`
/// prints without `--apply`; the startup runner always uses `Apply`.
#[derive(Debug, PartialEq)]
pub enum Mode {
    Report,
    Apply,
}

pub async fn stored_version(backend: &dyn Backend, cipher: &Cipher) -> Result<DataVersion> {
    Ok(DATA_VERSION.get::<DataVersion>(backend, cipher).await?.0)
}

/// The migrations this bucket hasn't reached yet, in the order they must run.
pub async fn pending(backend: &dyn Backend, cipher: &Cipher) -> Result<Vec<&'static Migration>> {
    let stored = stored_version(backend, cipher).await?.version;
    Ok(MIGRATIONS.iter().filter(|m| m.version > stored).collect())
}

/// What [`run_pending`] would do, without writing anything — each pending
/// migration's own report pass.
pub async fn plan(backend: &Arc<dyn Backend>, cipher: &Arc<Cipher>) -> Result<Vec<(&'static Migration, Outcome)>> {
    let mut out = Vec::new();
    for migration in pending(backend.as_ref(), cipher).await? {
        out.push((migration, run_one(backend, cipher, migration, Mode::Report).await?));
    }
    Ok(out)
}

/// Applies every pending migration in order, stamping the version after each
/// one that finishes cleanly.
///
/// `Err` only for a real failure (backend unreachable, decrypt failure) —
/// that's a startup-stopping condition, same as a wrong passphrase. A pass
/// that merely couldn't finish every item reports `incomplete > 0`, leaves
/// the version alone and is retried on the next run; the returned outcomes
/// carry that so the caller can say so out loud instead of the operator
/// finding out from a half-migrated page.
pub async fn run_pending(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
) -> Result<Vec<(&'static Migration, Outcome)>> {
    let mut applied = Vec::new();
    for migration in pending(backend.as_ref(), cipher).await? {
        let outcome = run_one(backend, cipher, migration, Mode::Apply)
            .await
            .with_context(|| format!("data migration {} ({})", migration.version, migration.name))?;
        let incomplete = outcome.incomplete;
        applied.push((migration, outcome));
        if incomplete > 0 {
            // Stopping rather than continuing: migrations are ordered, and
            // running the next one over data the previous one didn't finish
            // reshaping is how a half-migration turns into a wrong one.
            break;
        }
        stamp(backend.as_ref(), cipher, migration).await?;
    }
    Ok(applied)
}

async fn run_one(
    backend: &Arc<dyn Backend>,
    cipher: &Arc<Cipher>,
    migration: &Migration,
    mode: Mode,
) -> Result<Outcome> {
    match migration.version {
        1 => group_members::run(backend, cipher, mode).await,
        2 => repo_permission_index::run(backend, cipher, mode).await,
        // Unreachable through `MIGRATIONS`, which is the only source of the
        // versions passed here — but a new entry added without a matching
        // arm would otherwise silently stamp itself as applied.
        other => anyhow::bail!("no such data migration: {other}"),
    }
}

/// Records `migration` as applied. A CAS conflict means another instance
/// stamped it (or an unrelated concurrent write landed): re-read, and treat
/// "already at or past this version" as success rather than retrying a write
/// whose effect is already there.
async fn stamp(backend: &dyn Backend, cipher: &Cipher, migration: &Migration) -> Result<()> {
    let (mut state, etag) = DATA_VERSION.get::<DataVersion>(backend, cipher).await?;
    if state.version >= migration.version {
        return Ok(());
    }
    state.version = migration.version;
    state.applied.push(AppliedMigration {
        version: migration.version,
        name: migration.name.to_string(),
        at: crate::storage::audit::format_unix_secs(now_unix()),
    });
    match DATA_VERSION.put(backend, cipher, &state, etag.as_deref()).await? {
        Cas::Ok => Ok(()),
        Cas::Conflict => {
            let stored = stored_version(backend, cipher).await?.version;
            if stored >= migration.version {
                return Ok(());
            }
            anyhow::bail!(
                "couldn't record data migration {} ({}): {} was written concurrently and is still at version {stored}",
                migration.version,
                migration.name,
                ".conf/data-version.json",
            )
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
