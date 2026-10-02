//! Where a backup goes, expressed as one short string an admin can type
//! into the settings page.
//!
//! Never credentials. The instance's own settings live *in the bucket*
//! (`.conf/global.json`), so an S3 access key stored there would be an S3
//! access key inside the data being backed up — readable to anyone who ever
//! gets the backup. A destination therefore only ever *names* something:
//! a `[buckets.<name>]` section of the local `gitquarry.toml` (where
//! credentials already live, on disk, mode 0600), a directory, or this same
//! bucket.

use crate::config::{Config, LocalEngine, LocalParams};
use crate::crypto::Cipher;
use crate::storage::Backend;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A parsed destination. The string form round-trips through
/// [`Dest::parse`]/[`Dest::to_string`] so what's stored in settings is what
/// an admin typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    /// `bucket:<name>` — the only form that survives losing the source
    /// bucket's provider account, and so the only one that's a backup in the
    /// disaster sense rather than the oops sense.
    Bucket(String),
    /// `path:/abs/dir` — a plain directory, opened as `s3db`'s local
    /// (`redb`) driver. A mounted volume, a NAS, an attached disk.
    Path(PathBuf),
    /// `self` — this same bucket. Snapshot only: a replica writes each
    /// object back at its own key, which against the source itself is a
    /// no-op by definition, so [`Dest::supports_replica`] says no and the
    /// run refuses rather than appearing to work.
    SelfBucket,
}

impl std::fmt::Display for Dest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dest::Bucket(name) => write!(f, "bucket:{name}"),
            Dest::Path(path) => write!(f, "path:{}", path.display()),
            Dest::SelfBucket => f.write_str("self"),
        }
    }
}

impl Dest {
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw == "self" {
            return Ok(Dest::SelfBucket);
        }
        if let Some(name) = raw.strip_prefix("bucket:") {
            let name = name.trim();
            if name.is_empty() {
                bail!("'bucket:' needs a bucket name from gitquarry.toml, e.g. bucket:offsite");
            }
            return Ok(Dest::Bucket(name.to_string()));
        }
        if let Some(path) = raw.strip_prefix("path:") {
            let path = PathBuf::from(path.trim());
            if !path.is_absolute() {
                bail!("'path:' needs an absolute directory, e.g. path:/srv/gitquarry-backup");
            }
            return Ok(Dest::Path(path));
        }
        bail!("unrecognized backup destination '{raw}' (expected bucket:<name>, path:/abs/dir, or self)")
    }

    /// Whether a byte-for-byte replica can be written here. See
    /// [`Dest::SelfBucket`].
    pub fn supports_replica(&self) -> bool {
        !matches!(self, Dest::SelfBucket)
    }

    /// Everything that can be checked without touching the network — what
    /// the settings form runs on save, so a typo is an error next to the
    /// field instead of a schedule that silently never works.
    ///
    /// `source_bucket` is the bucket this instance is serving: naming it as
    /// the destination is rejected outright, since `bucket:<itself>` would
    /// read as an off-site backup while being nothing of the kind.
    pub fn validate_against(&self, config: &Config, source_bucket: &str) -> Result<()> {
        match self {
            Dest::Bucket(name) => {
                if name == source_bucket {
                    bail!(
                        "bucket:{name} is the bucket this instance is already serving — use 'self' \
                         if a snapshot alongside the live data is really what's wanted"
                    );
                }
                config.bucket(name).map(|_| ())
            }
            Dest::Path(path) => {
                if !path.is_dir() {
                    bail!("{} isn't a directory (create it first)", path.display());
                }
                Ok(())
            }
            Dest::SelfBucket => Ok(()),
        }
    }
}

/// The live side of a backup — what's being backed up, and what a
/// destination needs to know about it.
pub struct Source<'a> {
    pub bucket_name: &'a str,
    pub backend: &'a Arc<dyn Backend>,
    pub cipher: &'a Arc<Cipher>,
    /// Absolute path to `gitquarry.toml`, read fresh per run rather than
    /// captured at startup: an operator adding the destination bucket's
    /// section shouldn't have to restart the server for the next backup to
    /// find it.
    pub config_path: &'a Path,
}

/// A destination opened and ready to be written to.
pub struct Resolved {
    pub backend: Arc<dyn Backend>,
    /// What to seal with at the destination.
    ///
    /// For a replica this is the *source's* cipher, and that isn't a
    /// shortcut: a replica keeps every object at its own key, and objects
    /// are sealed with their key as AAD, so the destination has to be opened
    /// under the very same data key or nothing there would ever decrypt.
    /// [`Resolved::verify_openable`] is what checks the operator can also
    /// open it with the passphrase they configured for it.
    pub cipher: Arc<Cipher>,
    /// Whether this run should write a replica at all — false for
    /// [`Dest::SelfBucket`].
    pub replica: bool,
    /// The destination's own configured passphrase, for
    /// [`Resolved::verify_openable`]. `None` when there's no separate
    /// passphrase to check (`self`, and a `path:` destination, which has no
    /// config section of its own).
    passphrase: Option<String>,
}

impl Resolved {
    /// Proves the destination can be opened with the passphrase configured
    /// for it, *after* its `.conf/encryption.json` is in place.
    ///
    /// Worth its own step because of how the data key works: the key is
    /// wrapped under the passphrase, and a replica copies the source's
    /// wrapped key verbatim — so a destination bucket configured with a
    /// *different* passphrase would fill up perfectly happily and then be
    /// impossible for its owner to open. That's the single worst outcome
    /// this whole feature can produce (a backup that exists and can't be
    /// read), so it's checked on every run rather than trusted.
    pub async fn verify_openable(&self) -> Result<()> {
        let Some(passphrase) = &self.passphrase else {
            return Ok(());
        };
        s3db::bootstrap::open_cipher(self.backend.as_ref(), passphrase)
            .await
            .context(
                "the destination bucket's own `passphrase` in gitquarry.toml doesn't open it. A \
                 replica carries the source's wrapped data key, so the two buckets must be \
                 configured with the same passphrase — otherwise the backup would be unreadable \
                 exactly when it's needed",
            )
            .map(|_| ())
    }
}

/// Opens `dest` for writing. Does no copying and writes nothing itself — the
/// run does that, so a destination that can't even be opened fails before
/// anything has been touched.
pub async fn resolve(dest: &Dest, source: &Source<'_>) -> Result<Resolved> {
    match dest {
        Dest::SelfBucket => Ok(Resolved {
            backend: Arc::clone(source.backend),
            cipher: Arc::clone(source.cipher),
            replica: false,
            passphrase: None,
        }),
        Dest::Path(path) => {
            if !path.is_dir() {
                bail!("{} isn't a directory (create it first)", path.display());
            }
            // A destination reused across backup runs (this is a replica,
            // not a one-shot dump) may already hold data from before s3db
            // retired the `files` local engine -- `s3db.redb`'s absence
            // from an otherwise non-empty directory is that exact case, so
            // fail clearly instead of silently starting a second, empty
            // store next to the old one.
            if !path.join("s3db.redb").is_file() && std::fs::read_dir(path)?.next().is_some() {
                bail!(
                    "{} already holds backup data in the retired `files` format -- run \
                     `s3db-local-migrate {}` to move it to `engine = \"redb\"` first",
                    path.display(),
                    path.display()
                );
            }
            let backend = crate::storage::build_backend(&crate::config::BucketConfig {
                backend: crate::config::BackendConfig::Local(LocalParams {
                    bucket: path.to_string_lossy().to_string(),
                    engine: LocalEngine::Redb,
                }),
                // A directory destination has no config section, so there's
                // no separate passphrase to honour: it holds the source's own
                // wrapped data key, copied verbatim, and opens with the
                // source's passphrase. `build_backend` never looks at this
                // field.
                passphrase: String::new(),
                resilience: Default::default(),
            })
            .with_context(|| format!("opening {} as a backup destination", path.display()))?
            .backend;
            Ok(Resolved {
                backend,
                cipher: Arc::clone(source.cipher),
                replica: true,
                passphrase: None,
            })
        }
        Dest::Bucket(name) => {
            let config = Config::load_from(source.config_path)?;
            dest.validate_against(&config, source.bucket_name)?;
            let bucket = config.bucket(name)?;
            let backend = crate::storage::build_backend(bucket)
                .with_context(|| format!("opening bucket '{name}' as a backup destination"))?
                .backend;
            Ok(Resolved {
                backend,
                cipher: Arc::clone(source.cipher),
                replica: true,
                passphrase: Some(bucket.passphrase.clone()),
            })
        }
    }
}
