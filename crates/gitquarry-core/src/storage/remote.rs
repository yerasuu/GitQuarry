use super::{Backend, Cas, get_json, list_all_keys, put_json};
use crate::crypto::Cipher;
use anyhow::{Context, Result};
use futures::stream::{self, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

/// Name of the object that acts as a repo's source of truth: refs + HEAD.
/// Always updated via compare-and-swap on its etag.
pub const MANIFEST_SUFFIX: &str = "refs.json";
const PACK_PREFIX: &str = "objects/pack";
const LFS_PREFIX: &str = "lfs";
const RELEASES_PREFIX: &str = "releases";
/// Metadata and assets both live under `releases/<tag>/` (not
/// `releases/<tag>.json` as a sibling) specifically so a whole release can
/// be deleted with one prefix scan without risking a different tag whose
/// name happens to be a string-prefix of this one (`"v1"` vs `"v1.0"`) —
/// the trailing `/` this prefix always has once a tag is appended makes
/// that collision impossible, unlike a bare `<tag>` prefix would.
const RELEASE_META_SUFFIX: &str = "meta.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetMeta {
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    pub uploaded_at: String,
    pub uploaded_by: String,
    /// Ascii-armored detached OpenPGP signature over this asset's bytes,
    /// if the uploader attached one — `None` for an unsigned asset
    /// (including both source archives `domain::release::generate_source_archives`
    /// always creates, which are never signed). Verified exactly once,
    /// at upload time (`domain::release::add_asset`), against whichever of
    /// `uploaded_by`'s stored `User::gpg_keys` can verify it — never
    /// re-verified later, so a page render never has to shell out to `gpg`
    /// or re-download the asset just to show the badge.
    #[serde(default)]
    pub signature: Option<String>,
    /// `false` unless `signature` verified against one of `uploaded_by`'s
    /// stored keys at upload time — meaningless (always `false`) when
    /// `signature` is `None`.
    #[serde(default)]
    pub signature_verified: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseMeta {
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub title: String,
    /// Markdown, rendered the same way a repo's README is.
    #[serde(default)]
    pub notes: String,
    /// Upload order, not sorted — the only thing that gives a stable,
    /// meaningful order across the 5 different `Backend` drivers (a plain
    /// `list_prefix` scan gives no ordering guarantee at all).
    #[serde(default)]
    pub assets: Vec<AssetMeta>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub created_by: String,
    /// A cosmetic badge only — same "just a flag, no visibility logic"
    /// scope as GitHub's own prerelease checkbox (unlike a draft, a
    /// prerelease is still fully public/listed).
    #[serde(default)]
    pub is_prerelease: bool,
    /// Unlike [`Self::is_prerelease`], this one *does* gate visibility:
    /// `webui::repo::releases` hides a draft from the list and 404s its
    /// detail page for anyone without `Developer`+ on the repo — same
    /// "exists, but only to whoever could publish it" shape GitHub's own
    /// draft releases have.
    #[serde(default)]
    pub is_draft: bool,
}
/// How many objects [`Remote::copy_all_to`] re-encrypts at once. Bounded,
/// not all-at-once: a repo with thousands of small objects (many old packs)
/// shouldn't open that many simultaneous connections to the backend.
const COPY_CONCURRENCY: usize = 8;

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Manifest {
    #[serde(default)]
    pub refs: BTreeMap<String, String>,
    #[serde(default)]
    pub head: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Remote {
    backend: Arc<dyn Backend>,
    /// Key prefix, no leading or trailing `/`. Empty if the repo lives at
    /// the bucket root.
    prefix: String,
    cipher: Arc<Cipher>,
}

impl Remote {
    pub fn new(backend: Arc<dyn Backend>, prefix: String, cipher: Arc<Cipher>) -> Self {
        Remote {
            backend,
            prefix,
            cipher,
        }
    }

    fn key(&self, suffix: &str) -> String {
        if self.prefix.is_empty() {
            suffix.to_string()
        } else {
            format!("{}/{suffix}", self.prefix)
        }
    }

    /// Current manifest and its etag. The etag is `None` if the repo
    /// doesn't exist in the bucket yet.
    pub async fn get_manifest(&self) -> Result<(Manifest, Option<String>)> {
        get_json(self.backend.as_ref(), &self.key(MANIFEST_SUFFIX), &self.cipher).await
    }

    /// Compare-and-swap of the manifest. `expected_etag = None` means "must
    /// not exist yet".
    pub async fn put_manifest(&self, manifest: &Manifest, expected_etag: Option<&str>) -> Result<Cas> {
        put_json(
            self.backend.as_ref(),
            &self.key(MANIFEST_SUFFIX),
            manifest,
            expected_etag,
            &self.cipher,
        )
        .await
    }

    /// Current repo settings (description/owner/visibility/archived) and
    /// their etag. `None` config on a repo that has no `config.json` yet.
    pub async fn get_repo_config(&self) -> Result<(super::entities::RepoConfig, Option<String>)> {
        // `self.prefix` is already the full S3 key prefix (`repos/<uid>` —
        // see `AppContext::remote_for_uid`/`mirror::hook::build_remote`),
        // but `settings::get_repo_config` expects the bare uid and prepends
        // `REPOS_PREFIX` itself. Passing the already-prefixed string through
        // unstripped double-prefixes the key
        // (`repos/repos/<uid>/.conf/config.json`), which never exists —
        // `get_json`'s default-on-404 then silently hands back
        // `RepoConfig::default()`, so `archived`/`protected_branches`/
        // `protected_tags` enforcement in the hook never actually fired.
        let repo = self
            .prefix
            .strip_prefix(&format!("{}/", super::REPOS_PREFIX))
            .unwrap_or(&self.prefix);
        super::settings::get_repo_config(self.backend.as_ref(), repo, &self.cipher).await
    }

    /// true if the repo already exists in the bucket (has a manifest). Only
    /// checks the etag `get` already returns (`None` = missing), rather than
    /// a separate HEAD-style primitive: not worth its own `Backend` method
    /// for the one caller that needs "exists, don't care about the body",
    /// and `refs.json` is small (a list of refs, not a pack).
    pub async fn exists(&self) -> Result<bool> {
        let (_bytes, etag) = self.backend.get(&self.key(MANIFEST_SUFFIX)).await?;
        Ok(etag.is_some())
    }

    /// Copies every object under this repo's prefix to `dest`'s prefix,
    /// preserving each object's suffix (`refs.json` stays `refs.json`,
    /// `objects/pack/x.pack` stays `objects/pack/x.pack`, just under the new
    /// prefix). Downloads, decrypts, re-encrypts, and re-uploads each one —
    /// NOT a server-side copy: [`crate::crypto::Cipher::seal`]/
    /// [`crate::crypto::Cipher::seal_stream`] both bind each object's
    /// ciphertext to its own key as AAD (specifically so
    /// ciphertext can't be replayed under a different key), so a raw
    /// same-bytes copy under a new prefix would leave every object
    /// undecryptable at its new key forever — `open` failing with "data
    /// was modified/moved from a different key" is that mismatch, not a
    /// corrupt bucket. Doesn't touch the source: pair with
    /// [`Self::delete_all`] on `self` once the caller has actually
    /// confirmed the copy succeeded (see `webui::repo_settings`'s move
    /// action and `webui::fork`), so a failure partway through never loses
    /// data — worst case leaves a partial duplicate under `dest`, not a
    /// missing repo.
    pub async fn copy_all_to(&self, dest: &Remote) -> Result<()> {
        let src_prefix = self.key("");
        let keys = list_all_keys(self.backend.as_ref(), &src_prefix).await?;
        let pairs: Vec<(&str, String)> = keys
            .iter()
            .filter_map(|src_key| {
                let suffix = src_key.strip_prefix(&src_prefix)?;
                if suffix.is_empty() || suffix.ends_with('/') {
                    // A zero-byte "folder" placeholder some S3 consoles create
                    // when browsing/creating a path (key ends in `/`, at any
                    // depth — `gitquarry/`, `gitquarry/.conf/`, ...) — gitquarry
                    // never writes one of these, and at least one provider
                    // rejects a `PutObject` whose key ends in `/` outright. Not
                    // real repo data either way, so it's dropped rather than
                    // carried over; `delete_all` still cleans it up from the
                    // source once the rest of the copy succeeds.
                    return None;
                }
                Some((src_key.as_str(), dest.key(suffix)))
            })
            .collect();
        stream::iter(pairs.iter().map(Ok::<_, anyhow::Error>))
            .try_for_each_concurrent(Some(COPY_CONCURRENCY), |(src_key, dest_key)| async move {
                self.reencrypt_object(src_key, dest, dest_key).await
            })
            .await
    }

    /// Downloads `src_key` (this repo's own — decrypted with `self.cipher`,
    /// `src_key` as AAD) and re-uploads it to `dest_key` on `dest`
    /// (re-encrypted with `dest.cipher`, `dest_key` as AAD — in practice
    /// always the same [`crate::crypto::Cipher`] instance, since one
    /// process only ever has one, but going through `dest`'s own field
    /// rather than `self`'s keeps this correct if that ever stops being
    /// true).
    ///
    /// Every object under a repo's prefix goes through here — not just
    /// packs/LFS/release-assets (sealed with [`Cipher::seal_stream`]), but
    /// also `refs.json`/`config.json`/every small JSON entity (sealed with
    /// the plain, single-shot [`Cipher::seal`]) — and the two formats
    /// aren't interchangeable: `open_stream`ing a plain-`seal`ed object (or
    /// vice versa) fails outright ("not a gitquarry stream"), it doesn't
    /// silently produce garbage. [`Cipher::is_stream_sealed`] picks the
    /// matching pair per-object from its own first few bytes, so this one
    /// method still handles anything under the prefix without the caller
    /// (`copy_all_to`/`delete_all` — this is the only one that actually
    /// decrypts) needing to know each key's format ahead of time. The
    /// initial download still goes through a scratch tempfile either way
    /// (via `Backend::get_stream`), so a multi-gigabyte pack never gets
    /// fully buffered just to peek its header.
    async fn reencrypt_object(&self, src_key: &str, dest: &Remote, dest_key: &str) -> Result<()> {
        let ciphertext_tmp = tempfile::Builder::new()
            .prefix(".gitquarry-tmp-")
            .tempfile()
            .context("creating temp file for re-encryption")?
            .into_temp_path();
        self.backend.get_stream(src_key, &ciphertext_tmp).await?;

        let mut header = [0u8; 8];
        let header_len = {
            use tokio::io::AsyncReadExt;
            let mut f = tokio::fs::File::open(&ciphertext_tmp)
                .await
                .with_context(|| format!("reopening downloaded {src_key}"))?;
            f.read(&mut header)
                .await
                .with_context(|| format!("peeking downloaded {src_key}"))?
        };

        if Cipher::is_stream_sealed(&header[..header_len]) {
            let plaintext_tmp = tempfile::Builder::new()
                .prefix(".gitquarry-tmp-")
                .tempfile()
                .context("creating temp file for re-encryption")?
                .into_temp_path();
            self.cipher
                .open_stream(src_key, &ciphertext_tmp, &plaintext_tmp)
                .await?;
            seal_and_upload(dest.backend.as_ref(), dest_key, &dest.cipher, &plaintext_tmp).await?;
        } else {
            let sealed = tokio::fs::read(&ciphertext_tmp)
                .await
                .with_context(|| format!("reading downloaded {src_key}"))?;
            let plaintext = self.cipher.open(src_key, &sealed)?;
            let resealed = dest
                .cipher
                .seal(dest_key, &plaintext, crate::crypto::CompressionStrategy::None);
            dest.backend.put_cas(dest_key, resealed, None).await?;
        }
        Ok(())
    }

    /// Deletes every object under this repo's prefix — manifest, config,
    /// every pack, all in one shot: unlike [`Self::upload_pack`] and
    /// friends this doesn't need to name each piece, they all live under
    /// `self.key("")`. Irreversible: there's no undo once the backend
    /// confirms the batch delete — the caller is the one that has to make
    /// sure this is really what an operator wanted (see
    /// `webui::repo_settings`'s confirmation-by-typing-the-name).
    pub async fn delete_all(&self) -> Result<()> {
        let prefix = self.key("");
        let keys = list_all_keys(self.backend.as_ref(), &prefix).await?;
        self.backend.delete_many(&keys).await.map(|_| ())
    }

    /// Base names (`pack-<sha>`) of the packs stored on the remote.
    pub async fn list_pack_names(&self) -> Result<Vec<String>> {
        let prefix = self.key(&format!("{PACK_PREFIX}/"));
        let names: BTreeSet<String> = list_all_keys(self.backend.as_ref(), &prefix)
            .await?
            .into_iter()
            .filter_map(|key| {
                let file_name = key.rsplit('/').next()?;
                file_name.strip_suffix(".pack").map(str::to_string)
            })
            .collect();
        Ok(names.into_iter().collect())
    }

    /// Uploads `.pack` then `.idx`. That order matters: git only considers a
    /// pack valid once it sees the `.idx`, so a concurrent reader that sees
    /// the `.idx` already has the `.pack` guaranteed complete.
    pub async fn upload_pack(&self, base_name: &str, pack_path: &Path, idx_path: &Path) -> Result<()> {
        self.upload_file(&format!("{PACK_PREFIX}/{base_name}.pack"), pack_path)
            .await?;
        self.upload_file(&format!("{PACK_PREFIX}/{base_name}.idx"), idx_path)
            .await?;
        Ok(())
    }

    /// Deletes one pack's `.pack` and `.idx` — used by `crate::repack` to
    /// remove the packs a consolidation made redundant, once the new
    /// consolidated pack is confirmed uploaded. Unlike [`Self::delete_all`]
    /// this doesn't touch anything else under the repo's prefix (`refs.json`,
    /// `.conf/config.json`, any other pack).
    pub async fn delete_pack(&self, base_name: &str) -> Result<()> {
        let keys = [
            self.key(&format!("{PACK_PREFIX}/{base_name}.pack")),
            self.key(&format!("{PACK_PREFIX}/{base_name}.idx")),
        ];
        self.backend.delete_many(&keys).await.map(|_| ())
    }

    /// Encrypts the contents of `path` and uploads it to `suffix`, streaming
    /// (see [`crate::crypto::Cipher::seal_stream`]) — bounded memory
    /// regardless of `path`'s size, unlike a plain [`crate::crypto::Cipher::seal`].
    async fn upload_file(&self, suffix: &str, path: &Path) -> Result<()> {
        let key = self.key(suffix);
        seal_and_upload(self.backend.as_ref(), &key, &self.cipher, path).await
    }

    /// Downloads `.idx` and `.pack` to `dest_dir`. `.pack` goes second for
    /// the same reason as [`Self::upload_pack`], but in reverse: both files
    /// land under their final name only once complete.
    pub async fn download_pack(&self, base_name: &str, dest_dir: &Path) -> Result<()> {
        self.download_file(
            &format!("{PACK_PREFIX}/{base_name}.idx"),
            &dest_dir.join(format!("{base_name}.idx")),
        )
        .await?;
        self.download_file(
            &format!("{PACK_PREFIX}/{base_name}.pack"),
            &dest_dir.join(format!("{base_name}.pack")),
        )
        .await?;
        Ok(())
    }

    /// Downloads, decrypts (streaming, see
    /// [`crate::crypto::Cipher::open_stream`] — bounded memory regardless of
    /// `suffix`'s size), and writes to a temp file it renames at the end: a
    /// cut halfway through must not leave a partial file under the final
    /// name, which git would read as corrupt.
    async fn download_file(&self, suffix: &str, dest: &Path) -> Result<()> {
        let key = self.key(suffix);

        let dir = dest.parent().unwrap_or_else(|| Path::new("."));
        let tmp = tempfile::Builder::new()
            .prefix(".gitquarry-dl-")
            .tempfile_in(dir)
            .with_context(|| format!("creating temp file in {}", dir.display()))?;
        let tmp_path = tmp.into_temp_path();

        fetch_decrypted(self.backend.as_ref(), &key, &self.cipher, &tmp_path).await?;

        // fsync before the rename: if the machine cuts out, we don't want a
        // file with a valid name and half-written content.
        let file = tokio::fs::File::open(&tmp_path)
            .await
            .with_context(|| format!("reopening {}", tmp_path.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("syncing {}", tmp_path.display()))?;
        drop(file);

        tmp_path
            .persist(dest)
            .with_context(|| format!("moving temp file -> {}", dest.display()))?;
        Ok(())
    }

    /// `true` if this LFS object has already been uploaded — checked via
    /// [`Backend::list_prefix`] (metadata only, S3 `ListObjects` never
    /// transfers a body) rather than [`Backend::get`], since an LFS object
    /// can be arbitrarily large and this is called once per object on every
    /// batch request. Deliberately doesn't report a stored *size*: the
    /// backend only knows the ciphertext's length (nonce + per-chunk AEAD
    /// tag overhead on top of the plaintext, see
    /// [`crate::crypto::Cipher::seal_stream`]), which isn't the real object
    /// size a Git LFS client expects — callers that need the real size get
    /// it for free once they've actually decrypted (e.g. the download
    /// handler measuring its own decrypted tempfile), and the batch API
    /// doesn't need it at all: `upload`/`download` requests already carry
    /// the client's own claimed `size` (from its LFS pointer file), which
    /// the response just echoes back.
    pub async fn lfs_object_exists(&self, oid: &str) -> Result<bool> {
        let key = self.key(&format!("{LFS_PREFIX}/{oid}"));
        Ok(self.backend.list_prefix(&key).await?.into_iter().any(|(k, _)| k == key))
    }

    /// Uploads an LFS object (content-addressed by `oid`, already verified by
    /// the caller to actually hash to it) — same streaming primitive as
    /// [`Self::upload_pack`].
    pub async fn upload_lfs_object(&self, oid: &str, path: &Path) -> Result<()> {
        self.upload_file(&format!("{LFS_PREFIX}/{oid}"), path).await
    }

    /// Downloads an LFS object to `dest` — same streaming primitive as
    /// [`Self::download_pack`].
    pub async fn download_lfs_object(&self, oid: &str, dest: &Path) -> Result<()> {
        self.download_file(&format!("{LFS_PREFIX}/{oid}"), dest).await
    }

    fn release_key(&self, tag: &str) -> String {
        self.key(&format!("{RELEASES_PREFIX}/{tag}/{RELEASE_META_SUFFIX}"))
    }

    /// Current release metadata for `tag` and its etag; `None` etag if this
    /// tag has no release yet.
    pub async fn get_release(&self, tag: &str) -> Result<(ReleaseMeta, Option<String>)> {
        get_json(self.backend.as_ref(), &self.release_key(tag), &self.cipher).await
    }

    /// Compare-and-swap write, keyed by `release.tag`. `expected_etag = None`
    /// both creates ("must not exist yet" — this is what actually enforces
    /// one release per tag, no separate existence check needed) and lets a
    /// caller CAS an edit of an existing release.
    pub async fn put_release(&self, release: &ReleaseMeta, expected_etag: Option<&str>) -> Result<Cas> {
        put_json(
            self.backend.as_ref(),
            &self.release_key(&release.tag),
            release,
            expected_etag,
            &self.cipher,
        )
        .await
    }

    /// Deletes a release's metadata and every asset under it in one batch —
    /// safe as a single prefix scan (see [`RELEASE_META_SUFFIX`]'s doc
    /// comment for why). Irreversible, no CAS: same "caller already decided
    /// this for sure" contract as [`Self::delete_all`].
    pub async fn delete_release(&self, tag: &str) -> Result<()> {
        let prefix = self.key(&format!("{RELEASES_PREFIX}/{tag}/"));
        let keys = list_all_keys(self.backend.as_ref(), &prefix).await?;
        self.backend.delete_many(&keys).await.map(|_| ())
    }

    /// Every tag that currently has a release — a `list_prefix` scan and
    /// strip, same convention `auth::access::list_names` already uses for
    /// users/groups, not a separate maintained index.
    pub async fn list_release_tags(&self) -> Result<Vec<String>> {
        let prefix = self.key(&format!("{RELEASES_PREFIX}/"));
        let suffix = format!("/{RELEASE_META_SUFFIX}");
        let tags: BTreeSet<String> = list_all_keys(self.backend.as_ref(), &prefix)
            .await?
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix)?.strip_suffix(&suffix).map(str::to_string))
            .collect();
        Ok(tags.into_iter().collect())
    }

    /// Uploads one release asset (streaming — same primitive
    /// [`Self::upload_pack`]/[`Self::upload_lfs_object`] use).
    pub async fn upload_asset(&self, tag: &str, filename: &str, source: &Path) -> Result<()> {
        self.upload_file(&format!("{RELEASES_PREFIX}/{tag}/assets/{filename}"), source)
            .await
    }

    /// Downloads one release asset to `dest` (streaming).
    pub async fn download_asset(&self, tag: &str, filename: &str, dest: &Path) -> Result<()> {
        self.download_file(&format!("{RELEASES_PREFIX}/{tag}/assets/{filename}"), dest)
            .await
    }

    /// Best-effort, like [`Self::delete_pack`] — call only after the
    /// release's own metadata has already been CAS-updated to no longer
    /// list this asset, so a lost race here never deletes bytes a
    /// concurrently-still-live release version still references.
    pub async fn delete_asset(&self, tag: &str, filename: &str) {
        let key = self.key(&format!("{RELEASES_PREFIX}/{tag}/assets/{filename}"));
        let _ = self.backend.delete(&key).await;
    }

    /// Uploads `username`'s avatar image (already sniffed/validated by the
    /// caller — see `validate::sniff_avatar_content_type`). Only meaningful
    /// on the [`crate::app::AppContext::avatar_remote`] instance, whose
    /// `prefix` is already the top-level `avatars/` (an avatar belongs to
    /// the account, not any one repo, so it isn't nested under
    /// [`crate::app::AppContext::remote_for_uid`]'s per-repo prefix the way
    /// LFS objects/release assets are). Re-upload overwrites in place — no
    /// CAS, no history, same contract [`Self::delete_asset`] has.
    pub async fn upload_avatar(&self, username: &str, source: &Path) -> Result<()> {
        self.upload_file(username, source).await
    }

    /// Downloads `username`'s avatar to `dest`. `Err` if none was ever
    /// uploaded — callers only reach this once `User::avatar_content_type`
    /// is `Some`, so a missing blob at that point is a genuine inconsistency
    /// worth surfacing, not a routine "no avatar" case to swallow.
    pub async fn download_avatar(&self, username: &str, dest: &Path) -> Result<()> {
        self.download_file(username, dest).await
    }

    /// Best-effort, like [`Self::delete_asset`] — call only after
    /// `User::avatar_content_type` has already been cleared, same
    /// metadata-first ordering.
    pub async fn delete_avatar(&self, username: &str) {
        let key = self.key(username);
        let _ = self.backend.delete(&key).await;
    }
}

/// How many times [`fetch_decrypted`]/[`seal_and_upload`] retry after a
/// local-I/O failure (their tempfile writes/reads coming up short of what
/// `Cipher::seal_stream`/`open_stream` themselves confirm they wrote — see
/// those functions' own self-checks) before giving up. Observed in practice
/// under real host load: each occurrence is transient, and a fresh attempt
/// with a fresh tempfile normally succeeds — retrying here means the user
/// doesn't have to manually re-run `git push`/reload a repo view every time
/// one draw comes up short.
const LOCAL_IO_RETRIES: u32 = 4;

/// Downloads `key` from `backend` into a decrypted file at `plaintext_path`
/// (the caller creates/owns that path — this only writes to it). The
/// ciphertext itself only ever touches a scratch tempfile in the system
/// temp dir, cleaned up once this returns — there's no local "final
/// destination" for it to be renamed onto, unlike `plaintext_path`.
async fn fetch_decrypted(backend: &dyn Backend, key: &str, cipher: &Cipher, plaintext_path: &Path) -> Result<()> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let ciphertext_tmp = tempfile::Builder::new()
            .prefix(".gitquarry-tmp-")
            .tempfile()
            .context("creating temp file for downloaded ciphertext")?
            .into_temp_path();
        let result: Result<()> = async {
            backend.get_stream(key, &ciphertext_tmp).await?;
            cipher.open_stream(key, &ciphertext_tmp, plaintext_path).await
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(e) if attempt < LOCAL_IO_RETRIES => eprintln!(
                "warning: attempt {attempt}/{LOCAL_IO_RETRIES} to download {key} \
                 failed, retrying: {e:#}"
            ),
            Err(e) => return Err(e),
        }
    }
}

/// Encrypts `plaintext_path` (streaming) and uploads it to `key` on
/// `backend` — the other half of [`fetch_decrypted`]. Same scratch-tempfile
/// reasoning: the ciphertext is pure intermediate, never renamed anywhere.
async fn seal_and_upload(backend: &dyn Backend, key: &str, cipher: &Cipher, plaintext_path: &Path) -> Result<()> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let sealed_tmp = tempfile::Builder::new()
            .prefix(".gitquarry-tmp-")
            .tempfile()
            .context("creating temp file for ciphertext to upload")?
            .into_temp_path();
        let result: Result<()> = async {
            cipher.seal_stream(key, plaintext_path, &sealed_tmp).await?;
            backend.put_stream(key, &sealed_tmp).await
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(e) if attempt < LOCAL_IO_RETRIES => eprintln!(
                "warning: attempt {attempt}/{LOCAL_IO_RETRIES} to upload {key} \
                 failed, retrying: {e:#}"
            ),
            Err(e) => return Err(e),
        }
    }
}

/// Enumerates every repo's uid (the top-level segment under
/// [`super::REPOS_PREFIX`] of every key ending in `refs.json`) — the raw
/// storage-level catalog, not a display list: a repo's current name lives
/// on its own `RepoConfig::full_name` now, not in this path, so callers
/// that need names fetch each uid's config (which they need anyway, for
/// visibility/archived/topics). Scoped to `REPOS_PREFIX` rather than the
/// whole bucket: cheaper (backend filters server-side), and it can't
/// accidentally pick up `.conf/`'s own objects.
pub async fn list_repo_paths(backend: &dyn Backend) -> Result<Vec<super::uid::Uid>> {
    let scan_prefix = format!("{}/", super::REPOS_PREFIX);
    let mut uids: Vec<super::uid::Uid> = list_all_keys(backend, &scan_prefix)
        .await?
        .into_iter()
        .filter_map(|key| {
            let rest = key.strip_prefix(&scan_prefix)?;
            let uid = rest.strip_suffix(&format!("/{MANIFEST_SUFFIX}"))?;
            Some(super::uid::Uid::from(uid.to_string()))
        })
        .collect();
    uids.sort();
    uids.dedup();
    Ok(uids)
}
