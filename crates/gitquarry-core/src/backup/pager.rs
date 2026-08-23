//! A paged key listing, handed out one `(key, size)` at a time.
//!
//! Hoisted out of [`super::replica`] once [`super::snapshot`] needed the same
//! thing: every pass in this module walks a whole bucket, and doing that
//! through `Backend::list_prefix` would hold every key in memory at once —
//! the one part of a backup that stops working as an instance grows, and
//! precisely the part that has to keep working when it's biggest.
//!
//! Every driver that matters overrides `Backend::list_page` natively (S3,
//! local files, redb, plus the resilience and cache wrappers); the ones that
//! don't fall back to `s3db`'s default, which re-lists and slices per page —
//! correct, just not cheap, which is worth knowing before pointing a very
//! large bucket's backup at one of those drivers.

use crate::storage::Backend;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::sync::Arc;

/// Keys per listing page. Big enough that a large bucket doesn't spend its
/// life in LIST round trips, small enough that a page in memory is nothing.
/// Matches S3's own maximum page size, which it clamps to anyway.
pub(crate) const PAGE_SIZE: usize = 1000;

pub(crate) struct KeyPages {
    backend: Arc<dyn Backend>,
    prefix: String,
    buf: VecDeque<(String, u64)>,
    after: Option<String>,
    exhausted: bool,
}

impl KeyPages {
    /// An empty `prefix` walks the whole bucket — which is what a backup
    /// wants: a prefix nobody taught [`super::inventory`] about is exactly
    /// the data `Class::Unknown` exists to carry, and it can only be seen by
    /// looking everywhere.
    pub(crate) fn new(backend: Arc<dyn Backend>, prefix: &str) -> Self {
        KeyPages {
            backend,
            prefix: prefix.to_string(),
            buf: VecDeque::new(),
            after: None,
            exhausted: false,
        }
    }

    pub(crate) async fn next(&mut self) -> Result<Option<(String, u64)>> {
        while self.buf.is_empty() && !self.exhausted {
            let page = self
                .backend
                .list_page(&self.prefix, self.after.as_deref(), PAGE_SIZE)
                .await
                .with_context(|| format!("listing '{}'", self.prefix))?;
            self.after = page.next_after.clone();
            self.exhausted = page.next_after.is_none();
            self.buf.extend(page.entries);
        }
        Ok(self.buf.pop_front())
    }
}
