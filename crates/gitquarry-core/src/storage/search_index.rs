//! Per-repo search index — file list at HEAD + recent commit subjects,
//! maintained incrementally by `domain::repo::update_search_index`
//! (spawned detached after every successful push, same "doesn't delay the
//! push response" pattern `domain::repo::update_primary_language` already
//! uses) rather than rebuilt on every `/_search` query. Turns query-time
//! work from "spawn `git` against every visible repo's bare mirror" into
//! "read one small stored JSON per repo" — still fans out by repo count
//! for the cross-repo aggregation `webui::search` does, but with no
//! subprocess or tree-walk cost per query.
//!
//! Peer mirrors synced under `_remote/<peer>/<repo>` (federation layer 3)
//! have no local `RepoConfig`/uid to key an index on, so they're never
//! indexed here — `webui::search` still searches those live, same as
//! before this module existed.

use super::uid::Uid;
use super::{Backend, Cas, REPOS_PREFIX};
use crate::crypto::Cipher;
use anyhow::Result;
use s3db::Collection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedCommit {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchIndex {
    /// The HEAD commit this index was last built against — `None` means
    /// never indexed yet. [`crate::domain::repo::update_search_index`] diffs
    /// against this to only walk the *new* commits a push introduced, not
    /// the whole history again.
    #[serde(default)]
    pub last_indexed_sha: Option<String>,
    /// Every file at `last_indexed_sha`'s tree, wholesale-replaced on each
    /// update — relisting a tree is cheap, and there's no comparable
    /// "new files since last time" range the way commits have.
    #[serde(default)]
    pub files: Vec<String>,
    /// Newest-first, capped at [`MAX_INDEXED_COMMITS`].
    #[serde(default)]
    pub commits: Vec<IndexedCommit>,
}

/// Steady-state cap, and also the cap on a first-ever index build (never
/// walks a repo's entire history just because it happened to never get
/// indexed before) — see [`SearchIndex::commits`]'s own doc comment. Same
/// order of magnitude as `webui::repo::STATS_LOG_LIMIT`'s own bounded-history
/// tolerance.
pub const MAX_INDEXED_COMMITS: usize = 2000;

const INDEX_KEY: &str = "index";

fn search_index_collection(uid: &Uid) -> Collection {
    Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/search-index"))
}

pub async fn get_search_index(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
) -> Result<(Option<SearchIndex>, Option<String>)> {
    let collection = search_index_collection(uid);
    collection.get::<SearchIndex>(backend, cipher, INDEX_KEY).await
}

pub async fn put_search_index(
    backend: &dyn Backend,
    cipher: &Cipher,
    uid: &Uid,
    index: &SearchIndex,
    expected_etag: Option<&str>,
) -> Result<Cas> {
    let collection = search_index_collection(uid);
    collection.put(backend, cipher, INDEX_KEY, index, expected_etag).await
}
