use anyhow::Result;
use gitquarry_core::app::{AppContext, Locks, ResolvedRepo};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, SALT_LEN};
use gitquarry_core::domain::pull_request::{self, NewPullRequest};
use gitquarry_core::storage::local::LocalBackend;
use gitquarry_core::storage::pull_requests::{self, PrComment, PrStatus};
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_forge_clients::{ForgeApi, NewRemotePr, PrPatch, RemoteComment, RemotePr, RemotePrState};
use gitquarry_mirror_external::access::{self, ExternalMirror, ExternalMirrorProvider, ExternalPrLink};
use gitquarry_mirror_external::pr_sync::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// In-memory [`ForgeApi`] — exactly the "mock GitHub" the design doc's
/// own verification section calls for, pure enough to drive every
/// anti-loop scenario without a real account.
#[derive(Default)]
struct FakeForge {
    prs: Mutex<Vec<RemotePr>>,
    comments: Mutex<HashMap<u64, Vec<RemoteComment>>>,
    next_number: Mutex<u64>,
    next_comment_id: Mutex<u64>,
}

impl FakeForge {
    fn with_pr(self, pr: RemotePr) -> Self {
        self.prs.lock().unwrap().push(pr);
        self
    }
}

#[async_trait::async_trait]
impl ForgeApi for FakeForge {
    async fn list_pull_requests(&self, since: Option<&str>) -> Result<Vec<RemotePr>> {
        Ok(self
            .prs
            .lock()
            .unwrap()
            .iter()
            .filter(|pr| since.is_none_or(|s| pr.updated_at.as_str() > s))
            .cloned()
            .collect())
    }

    async fn get_pull_request(&self, number: u64) -> Result<Option<RemotePr>> {
        Ok(self.prs.lock().unwrap().iter().find(|pr| pr.number == number).cloned())
    }

    async fn create_pull_request(&self, new: NewRemotePr) -> Result<RemotePr> {
        let mut next = self.next_number.lock().unwrap();
        *next = (*next).max(1);
        let number = *next;
        *next += 1;
        let pr = RemotePr {
            number,
            title: new.title,
            body: new.body,
            source_branch: new.head,
            target_branch: new.base,
            state: RemotePrState::Open,
            author: "faketester".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            ..Default::default()
        };
        self.prs.lock().unwrap().push(pr.clone());
        Ok(pr)
    }

    async fn update_pull_request(&self, number: u64, patch: PrPatch) -> Result<()> {
        let mut prs = self.prs.lock().unwrap();
        let pr = prs
            .iter_mut()
            .find(|p| p.number == number)
            .ok_or_else(|| anyhow::anyhow!("no such fake PR #{number}"))?;
        if let Some(title) = patch.title {
            pr.title = title;
        }
        if let Some(body) = patch.body {
            pr.body = body;
        }
        if let Some(state) = patch.state {
            pr.state = state;
        }
        pr.updated_at = "2024-01-01T00:00:01Z".to_string();
        Ok(())
    }

    async fn merge_pull_request(&self, number: u64) -> Result<RemotePr> {
        let mut prs = self.prs.lock().unwrap();
        let pr = prs
            .iter_mut()
            .find(|p| p.number == number)
            .ok_or_else(|| anyhow::anyhow!("no such fake PR #{number}"))?;
        pr.state = RemotePrState::Merged;
        pr.merge_commit_sha = Some("fakemergesha".to_string());
        pr.merged_by = Some("faketester".to_string());
        pr.merged_at = Some("2024-01-01T00:00:01Z".to_string());
        pr.updated_at = "2024-01-01T00:00:01Z".to_string();
        Ok(pr.clone())
    }

    async fn list_comments(&self, number: u64) -> Result<Vec<RemoteComment>> {
        Ok(self.comments.lock().unwrap().get(&number).cloned().unwrap_or_default())
    }

    async fn add_comment(&self, number: u64, body: &str) -> Result<RemoteComment> {
        let mut next = self.next_comment_id.lock().unwrap();
        let id = *next;
        *next += 1;
        let comment = RemoteComment {
            id,
            author: "faketester".to_string(),
            body: body.to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        self.comments
            .lock()
            .unwrap()
            .entry(number)
            .or_default()
            .push(comment.clone());
        Ok(comment)
    }
}

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Locks::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        allow_push_create: false,
        events: Default::default(),
        mirror_sync: None,
    }
}

fn test_target(root: &std::path::Path) -> ResolvedRepo {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(LocalBackend::new(&LocalParams {
        bucket: root.to_string_lossy().to_string(),
        engine: LocalEngine::Files,
    }));
    let cipher = Arc::new(Cipher::derive("test passphrase", &[13u8; SALT_LEN]).unwrap());
    let remote = Remote::new(backend, "repos/team/proj".to_string(), cipher);
    ResolvedRepo {
        uid: gitquarry_core::storage::uid::Uid::from("team/proj".to_string()),
        remote,
    }
}

async fn put_branch(target: &ResolvedRepo, branch: &str, sha: &str) {
    let (mut manifest, etag) = target
        .remote
        .get_manifest()
        .await
        .unwrap_or((Manifest::default(), None));
    manifest.refs.insert(format!("refs/heads/{branch}"), sha.to_string());
    target.remote.put_manifest(&manifest, etag.as_deref()).await.unwrap();
}

fn test_mirror() -> ExternalMirror {
    ExternalMirror {
        id: "m1".to_string(),
        remote_url: "https://github.com/acme/demo.git".to_string(),
        token: "ghp_secret".to_string(),
        provider: ExternalMirrorProvider::GitHub,
        direction: access::SyncDirection::Bidirectional,
        enabled: true,
        sync_pull_requests: true,
        label: "backup".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn discovers_a_new_remote_pr_and_links_it() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = test_mirror();

    let forge = FakeForge::default().with_pr(RemotePr {
        number: 7,
        title: "external title".to_string(),
        body: "external body".to_string(),
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        state: RemotePrState::Open,
        author: "alice".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    });

    sync_pull_requests_once_with(&ctx, &target, &mirror, &forge)
        .await
        .unwrap();

    let local_prs = pull_requests::list_pull_requests(&ctx.backend(), &ctx.cipher, &target.uid)
        .await
        .unwrap();
    assert_eq!(local_prs.len(), 1);
    assert_eq!(local_prs[0].title, "external title");

    let links = access::list_pr_links(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1")
        .await
        .unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].external_number, 7);
    assert_eq!(links[0].local_number, local_prs[0].number);
}

#[tokio::test]
async fn creates_a_linked_remote_pr_for_an_unlinked_local_one() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = test_mirror();

    pull_request::create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "local title",
            description: "local body",
            author: "bob",
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let forge = FakeForge::default();
    sync_pull_requests_once_with(&ctx, &target, &mirror, &forge)
        .await
        .unwrap();

    assert_eq!(forge.prs.lock().unwrap().len(), 1);
    assert_eq!(forge.prs.lock().unwrap()[0].title, "local title");
    let links = access::list_pr_links(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1")
        .await
        .unwrap();
    assert_eq!(links.len(), 1);
}

#[tokio::test]
async fn comment_anti_loop_exports_once_and_imports_once() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = test_mirror();

    let pr = pull_request::create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "t",
            description: "d",
            author: "bob",
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let forge = FakeForge::default().with_pr(RemotePr {
        number: 7,
        title: "t".to_string(),
        body: "d".to_string(),
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        state: RemotePrState::Open,
        author: "bob".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    });
    let link = ExternalPrLink {
        local_number: pr.number,
        external_number: 7,
        remote_updated_cursor: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    access::put_pr_link(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1", &link, None)
        .await
        .unwrap();

    // Seed one genuinely-local comment and one already-imported one
    // (which must never be re-exported).
    pull_request::add_comment(
        &ctx,
        &target,
        pr.number,
        "alice",
        "local reply",
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();
    let imported = PrComment {
        author: "carol".to_string(),
        body: "already from github".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        origin: Some("m1".to_string()),
        ..Default::default()
    };
    pull_requests::add_pr_comment(ctx.backend().as_ref(), &ctx.cipher, &target.uid, pr.number, &imported)
        .await
        .unwrap();
    // A genuine inbound comment on the external side, never seen locally yet.
    forge.comments.lock().unwrap().insert(
        7,
        vec![RemoteComment {
            id: 99,
            author: "dave".to_string(),
            body: "from github".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        }],
    );

    sync_one_linked_pr(&ctx, &target, &mirror, &forge, pr.number)
        .await
        .unwrap();

    // Exported exactly the one genuinely-local comment, never the
    // already-imported one.
    let exported = forge.comments.lock().unwrap().get(&7).unwrap().clone();
    assert_eq!(exported.iter().filter(|c| c.body == "local reply").count(), 1);
    assert!(!exported.iter().any(|c| c.body == "already from github"));

    // Imported the one genuine remote comment exactly once.
    let local_comments = pull_requests::list_pr_comments(&ctx.backend(), &ctx.cipher, &target.uid, pr.number)
        .await
        .unwrap();
    assert_eq!(local_comments.iter().filter(|c| c.body == "from github").count(), 1);

    // Second tick: nothing new to export or import.
    sync_one_linked_pr(&ctx, &target, &mirror, &forge, pr.number)
        .await
        .unwrap();
    let exported_again = forge.comments.lock().unwrap().get(&7).unwrap().clone();
    assert_eq!(
        exported_again.len(),
        exported.len(),
        "no duplicate export on a second tick"
    );
    let local_comments_again = pull_requests::list_pr_comments(&ctx.backend(), &ctx.cipher, &target.uid, pr.number)
        .await
        .unwrap();
    assert_eq!(
        local_comments_again.len(),
        local_comments.len(),
        "no duplicate import on a second tick"
    );
}

#[tokio::test]
async fn merge_on_the_external_side_marks_the_local_pr_merged() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = test_mirror();

    let pr = pull_request::create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "t",
            description: "d",
            author: "bob",
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let forge = FakeForge::default().with_pr(RemotePr {
        number: 7,
        title: "t".to_string(),
        body: "d".to_string(),
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        state: RemotePrState::Open,
        author: "bob".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    });
    let link = ExternalPrLink {
        local_number: pr.number,
        external_number: 7,
        remote_updated_cursor: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    access::put_pr_link(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1", &link, None)
        .await
        .unwrap();

    forge.merge_pull_request(7).await.unwrap();
    sync_one_linked_pr(&ctx, &target, &mirror, &forge, pr.number)
        .await
        .unwrap();

    let local = pull_request::get(&ctx, &target, pr.number).await.unwrap().unwrap();
    assert_eq!(local.status, PrStatus::Merged);
    assert_eq!(local.merge_commit_sha.as_deref(), Some("fakemergesha"));
}

#[tokio::test]
async fn remote_title_change_wins_when_newer_than_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = test_mirror();

    let pr = pull_request::create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "old title",
            description: "d",
            author: "bob",
        },
        gitquarry_i18n::Locale::En,
    )
    .await
    .unwrap();

    let forge = FakeForge::default().with_pr(RemotePr {
        number: 7,
        title: "new title from github".to_string(),
        body: "d".to_string(),
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        state: RemotePrState::Open,
        author: "bob".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-02T00:00:00Z".to_string(),
        ..Default::default()
    });
    let link = ExternalPrLink {
        local_number: pr.number,
        external_number: 7,
        remote_updated_cursor: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    access::put_pr_link(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1", &link, None)
        .await
        .unwrap();

    sync_one_linked_pr(&ctx, &target, &mirror, &forge, pr.number)
        .await
        .unwrap();

    let local = pull_request::get(&ctx, &target, pr.number).await.unwrap().unwrap();
    assert_eq!(local.title, "new title from github");
}

#[test]
fn a_line_comment_carries_its_position_into_the_exported_body() {
    let plain = PrComment {
        body: "looks good".to_string(),
        ..Default::default()
    };
    assert_eq!(export_body(&plain), "looks good");

    let anchored = PrComment {
        body: "why this?".to_string(),
        anchor: Some(gitquarry_core::storage::pull_requests::CommentAnchor {
            path: "src/main.rs".to_string(),
            side: gitquarry_core::storage::pull_requests::DiffSide::New,
            line: 42,
        }),
        ..Default::default()
    };
    assert_eq!(export_body(&anchored), "`src/main.rs:42`\n\nwhy this?");
}

#[tokio::test]
async fn a_pull_only_mirror_never_creates_a_pr_on_the_external_side() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = ExternalMirror {
        direction: access::SyncDirection::PullOnly,
        ..test_mirror()
    };

    pull_request::create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "local only",
            description: "stays here",
            author: "alice",
        },
        gitquarry_i18n::Locale::DEFAULT,
    )
    .await
    .unwrap();

    let forge = FakeForge::default();
    sync_pull_requests_once_with(&ctx, &target, &mirror, &forge)
        .await
        .unwrap();

    assert!(forge.prs.lock().unwrap().is_empty(), "a pull-only mirror pushed a PR");
    let links = access::list_pr_links(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1")
        .await
        .unwrap();
    assert!(links.is_empty());
}

#[tokio::test]
async fn a_push_only_mirror_never_imports_a_remote_pr() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = ExternalMirror {
        direction: access::SyncDirection::PushOnly,
        ..test_mirror()
    };

    let forge = FakeForge::default().with_pr(RemotePr {
        number: 9,
        title: "external title".to_string(),
        body: "external body".to_string(),
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        state: RemotePrState::Open,
        author: "bob".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    });

    sync_pull_requests_once_with(&ctx, &target, &mirror, &forge)
        .await
        .unwrap();

    let local_prs = pull_requests::list_pull_requests(&ctx.backend(), &ctx.cipher, &target.uid)
        .await
        .unwrap();
    assert!(local_prs.is_empty(), "a push-only mirror imported a remote PR");
}

/// The anti-loop bookkeeping has to survive a lost CAS race on the link:
/// dropping it (what unwrapping the `Cas` away used to do) means every
/// comment gets exported a second time on the next pass.
#[tokio::test]
async fn a_concurrent_link_write_doesnt_cost_the_comment_dedup_state() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    put_branch(&target, "feature", &"b".repeat(40)).await;
    let mirror = test_mirror();

    let pr = pull_request::create(
        &ctx,
        &target,
        NewPullRequest {
            source_repo: &target.uid,
            source_branch: "feature",
            target_branch: "main",
            title: "local",
            description: "body",
            author: "alice",
        },
        gitquarry_i18n::Locale::DEFAULT,
    )
    .await
    .unwrap();
    pull_requests::add_pr_comment(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &target.uid,
        pr.number,
        &PrComment {
            author: "alice".to_string(),
            body: "mine to export".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let forge = FakeForge::default();
    sync_pull_requests_once_with(&ctx, &target, &mirror, &forge)
        .await
        .unwrap();
    assert_eq!(forge.comments.lock().unwrap().values().flatten().count(), 1);

    // Someone else writes the link between the two passes, invalidating the
    // etag the next pass would have used.
    let (mut stored, etag) = access::get_pr_link(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1", pr.number)
        .await
        .unwrap();
    let mut stored = stored.take().unwrap();
    stored.remote_updated_cursor = "2024-01-02T00:00:00Z".to_string();
    access::put_pr_link(
        ctx.backend().as_ref(),
        &ctx.cipher,
        &target.uid,
        "m1",
        &stored,
        etag.as_deref(),
    )
    .await
    .unwrap();

    sync_pull_requests_once_with(&ctx, &target, &mirror, &forge)
        .await
        .unwrap();
    assert_eq!(
        forge.comments.lock().unwrap().values().flatten().count(),
        1,
        "the comment was exported twice"
    );
}

/// A PR opened from a fork carries a head branch that lives in that fork,
/// which Track A never mirrors — so it can never be imported, and must be
/// skipped instead of failing `create` again on every tick.
#[tokio::test]
async fn an_external_pr_whose_branches_arent_mirrored_is_skipped_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(dir.path());
    let target = test_target(dir.path());
    put_branch(&target, "main", &"a".repeat(40)).await;
    let mirror = test_mirror();

    let forge = FakeForge::default().with_pr(RemotePr {
        number: 11,
        title: "from a fork".to_string(),
        body: String::new(),
        // Never mirrored here: it only exists in the contributor's fork.
        source_branch: "contributor-patch".to_string(),
        target_branch: "main".to_string(),
        state: RemotePrState::Open,
        author: "carol".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        ..Default::default()
    });

    sync_pull_requests_once_with(&ctx, &target, &mirror, &forge)
        .await
        .unwrap();

    let local_prs = pull_requests::list_pull_requests(&ctx.backend(), &ctx.cipher, &target.uid)
        .await
        .unwrap();
    assert!(local_prs.is_empty());
    let links = access::list_pr_links(ctx.backend().as_ref(), &ctx.cipher, &target.uid, "m1")
        .await
        .unwrap();
    assert!(links.is_empty(), "a PR that can't be imported shouldn't get a link");
}
