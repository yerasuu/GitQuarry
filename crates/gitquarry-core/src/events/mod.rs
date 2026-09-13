//! Single internal emission point for domain state changes — `RepoEvent`s
//! fanned out to whatever's registered as a consumer. Exists so webhooks and
//! `gitquarry-ci`'s push trigger share one definition of "what counts as a
//! push" instead of each reimplementing it (`docs/webhooks-concept.md`'s
//! Phase 1, `docs/ci-cd-concept.md`'s Phase 0). No consumers are registered
//! yet — `emit` fans out to an empty list until webhooks' delivery engine
//! (`docs/webhooks-concept.md` Phase 3) and `gitquarry-ci`'s trigger
//! register themselves.

use crate::app::AppContext;
use crate::storage::uid::Uid;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// One ref that moved during a push. `old`/`new` are `None` for a ref that
/// didn't exist before/after — a plain zero-oid sentinel would need to know
/// the repo's hash algorithm (sha1 vs sha256) to render correctly, and
/// nothing here needs that string form, just "did it exist".
#[derive(Debug, Clone)]
pub struct RefUpdate {
    pub refname: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// Compares a manifest's `refs` map before and after a push and returns
/// every ref that changed — the post-`receive-pack`-success emission point
/// in `git::ssh`/`git::http` has no other way to learn which refs a push
/// touched (the `pre-receive` hook that actually parses `<old> <new>
/// <refname>` lines runs in a separate subprocess with no channel back to
/// the long-lived server process).
pub fn diff_refs(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<RefUpdate> {
    let mut refnames: Vec<&String> = before.keys().chain(after.keys()).collect();
    refnames.sort_unstable();
    refnames.dedup();
    refnames
        .into_iter()
        .filter_map(|refname| {
            let old = before.get(refname);
            let new = after.get(refname);
            if old == new {
                return None;
            }
            Some(RefUpdate {
                refname: refname.clone(),
                old: old.cloned(),
                new: new.cloned(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRequestAction {
    Opened,
    Updated,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueAction {
    Opened,
    Closed,
}

/// Carries just the ids/shas needed to identify what changed, not full
/// domain objects — a consumer that needs more (an MR's title, an issue's
/// body, ...) fetches it via the existing `domain::*` functions, same as any
/// other caller would.
#[derive(Debug, Clone)]
pub enum RepoEvent {
    Push {
        repo_uid: Uid,
        refs: Vec<RefUpdate>,
        pusher: Option<String>,
    },
    MergeRequest {
        repo_uid: Uid,
        number: u64,
        action: MergeRequestAction,
    },
    Issue {
        repo_uid: Uid,
        number: u64,
        action: IssueAction,
    },
    Release {
        repo_uid: Uid,
        tag: String,
    },
}

pub type Consumer = Arc<dyn Fn(RepoEvent) + Send + Sync>;

/// Held on [`AppContext`]. Cheaply `Clone` (shares the same consumer list),
/// same as `AppContext`'s other shared-state fields.
#[derive(Default, Clone)]
pub struct EventBus {
    consumers: Arc<Mutex<Vec<Consumer>>>,
}

impl fmt::Debug for EventBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.consumers.lock().expect("event bus mutex poisoned").len();
        f.debug_struct("EventBus").field("consumers", &count).finish()
    }
}

impl EventBus {
    pub fn register(&self, consumer: Consumer) {
        self.consumers.lock().expect("event bus mutex poisoned").push(consumer);
    }
}

pub fn emit(ctx: &AppContext, event: RepoEvent) {
    let consumers = ctx.events.consumers.lock().expect("event bus mutex poisoned");
    for consumer in consumers.iter() {
        consumer(event.clone());
    }
}
