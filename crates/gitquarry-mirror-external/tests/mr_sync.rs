//! Track B (MR sync) through the sweep that runs it in production,
//! [`engine::sync_repo_mirrors`], against a stateful stand-in for the GitLab
//! v4 API. The stand-in answers the git side with 404s, so the ref sync that
//! runs first fails and is logged — the MR pass runs regardless, which is
//! exactly the order the sweep guarantees.

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::domain::merge_request::{self, NewMergeRequest};
use gitquarry_core::storage::merge_requests::{self, CommentAnchor, DiffSide, MrComment, MrStatus};
use gitquarry_mirror_external::access::{self, ExternalMirror, ExternalMirrorProvider, ExternalPrLink, SyncDirection};
use gitquarry_mirror_external::{domain, engine};
use gitquarry_test_support::{Work, ctx, git};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

const T0: &str = "2024-01-01T00:00:00Z";
const T1: &str = "2024-01-01T00:00:01Z";

#[derive(Clone)]
struct FakeMr {
    iid: u64,
    title: String,
    description: String,
    state: &'static str,
    author: String,
    source_branch: String,
    target_branch: String,
    updated_at: String,
    merged_by: Option<String>,
    merge_commit_sha: Option<String>,
}

impl FakeMr {
    fn open(iid: u64, source: &str, title: &str, author: &str, updated_at: &str) -> Self {
        FakeMr {
            iid,
            title: title.to_string(),
            description: String::new(),
            state: "opened",
            author: author.to_string(),
            source_branch: source.to_string(),
            target_branch: "main".to_string(),
            updated_at: updated_at.to_string(),
            merged_by: None,
            merge_commit_sha: None,
        }
    }

    fn json(&self) -> Value {
        json!({
            "iid": self.iid,
            "title": self.title,
            "description": self.description,
            "state": self.state,
            "author": {"username": self.author},
            "source_branch": self.source_branch,
            "target_branch": self.target_branch,
            "created_at": T0,
            "updated_at": self.updated_at,
            "merged_at": self.merged_by.as_ref().map(|_| T1),
            "merged_by": self.merged_by.as_ref().map(|u| json!({"username": u})),
            "merge_commit_sha": self.merge_commit_sha,
        })
    }
}

#[derive(Clone)]
struct FakeNote {
    id: u64,
    author: String,
    body: String,
}

#[derive(Default)]
struct FakeGitLab {
    mrs: Vec<FakeMr>,
    notes: HashMap<u64, Vec<FakeNote>>,
    next_note_id: u64,
    /// Answers every new-MR POST with a 422 and counts them, so a test can
    /// see when the retries stop.
    reject_creates: bool,
    create_attempts: u32,
}

type Shared = Arc<RwLock<FakeGitLab>>;

async fn handle(State(state): State<Shared>, req: Request) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let first_page = req
        .uri()
        .query()
        .is_none_or(|q| !q.split('&').any(|kv| kv.starts_with("page=") && kv != "page=1"));
    let body: Value = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .ok()
        .filter(|b: &Bytes| !b.is_empty())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let Some(rest) = path.strip_prefix("/api/v4/projects/acme%2Fdemo/merge_requests") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let mut gl = state.write().unwrap();
    let str_field = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
    let reply = match (method, segments.as_slice()) {
        (Method::GET, []) => json!(if first_page {
            gl.mrs.iter().map(FakeMr::json).collect()
        } else {
            Vec::new()
        }),
        (Method::POST, []) if gl.reject_creates => {
            gl.create_attempts += 1;
            return StatusCode::UNPROCESSABLE_ENTITY.into_response();
        }
        (Method::POST, []) => {
            let iid = gl.mrs.iter().map(|m| m.iid).max().unwrap_or(0) + 1;
            let mut mr = FakeMr::open(
                iid,
                &str_field("source_branch").unwrap_or_default(),
                "",
                "faketester",
                T0,
            );
            mr.title = str_field("title").unwrap_or_default();
            mr.description = str_field("description").unwrap_or_default();
            mr.target_branch = str_field("target_branch").unwrap_or_default();
            gl.mrs.push(mr.clone());
            mr.json()
        }
        (method, [n, tail @ ..]) => {
            let Ok(n) = n.parse::<u64>() else {
                return StatusCode::NOT_FOUND.into_response();
            };
            match (method, tail) {
                (Method::GET, ["notes"]) => {
                    let notes = if first_page {
                        gl.notes.get(&n).cloned().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    json!(
                        notes
                            .iter()
                            .map(|note| json!({"id": note.id, "author": {"username": note.author},
                        "body": note.body, "created_at": T0, "system": false}))
                            .collect::<Vec<_>>()
                    )
                }
                (Method::POST, ["notes"]) => {
                    gl.next_note_id += 1;
                    let note = FakeNote {
                        id: gl.next_note_id,
                        author: "faketester".to_string(),
                        body: str_field("body").unwrap_or_default(),
                    };
                    gl.notes.entry(n).or_default().push(note.clone());
                    json!({"id": note.id, "author": {"username": note.author}, "body": note.body, "created_at": T0})
                }
                (method, tail) => {
                    let Some(mr) = gl.mrs.iter_mut().find(|m| m.iid == n) else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    match (method, tail) {
                        (Method::GET, []) => {}
                        (Method::PUT, []) => {
                            if let Some(title) = str_field("title") {
                                mr.title = title;
                            }
                            if let Some(description) = str_field("description") {
                                mr.description = description;
                            }
                            match str_field("state_event").as_deref() {
                                Some("close") => mr.state = "closed",
                                Some("reopen") => mr.state = "opened",
                                _ => {}
                            }
                            mr.updated_at = T1.to_string();
                        }
                        (Method::PUT, ["merge"]) => merge(mr),
                        _ => return StatusCode::NOT_FOUND.into_response(),
                    }
                    mr.json()
                }
            }
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    axum::Json(reply).into_response()
}

fn merge(mr: &mut FakeMr) {
    mr.state = "merged";
    mr.merge_commit_sha = Some("fakemergesha".to_string());
    mr.merged_by = Some("faketester".to_string());
    mr.updated_at = T1.to_string();
}

async fn fake_gitlab(initial: FakeGitLab) -> (String, Shared) {
    let state = Arc::new(RwLock::new(initial));
    let app = axum::Router::new().fallback(handle).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, state)
}

/// Real `main` and `feature` (one commit ahead) in the bucket: importing an
/// MR writes its branches into the repo's git mirror, so they have to name
/// objects that exist.
async fn push_main_and_feature(repo: &ResolvedRepo) {
    let work = Work::new().await;
    work.commit(&[("f.txt", "main\n")], "main\n").await;
    git(work.path(), &["checkout", "-q", "-b", "feature"]).await;
    work.commit(&[("f.txt", "feature\n")], "feature\n").await;
    work.push(repo, "refs/heads/main").await;
}

struct World {
    _dir: tempfile::TempDir,
    ctx: AppContext,
    repo: ResolvedRepo,
    mirror: ExternalMirror,
    gitlab: Shared,
}

/// A repo with `main` and `feature` already mirrored, plus one stored GitLab mirror of it pointed at the stand-in.
async fn world(direction: SyncDirection, gitlab: FakeGitLab) -> World {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let repo = ctx.create_repo("team/proj").await.unwrap().unwrap();
    push_main_and_feature(&repo).await;

    let (mut settings, etag) = gitquarry_core::storage::settings::get_settings(ctx.backend().as_ref(), &ctx.cipher)
        .await
        .unwrap();
    settings.mirror_extra_hosts.push("127.0.0.1".to_string());
    gitquarry_core::storage::settings::put_settings(ctx.backend().as_ref(), &settings, etag.as_deref(), &ctx.cipher)
        .await
        .unwrap();

    let (base, gitlab) = fake_gitlab(gitlab).await;
    let mirror = domain::create(
        &ctx,
        &repo.uid,
        ExternalMirror {
            remote_url: format!("{base}/acme/demo"),
            token: "glpat-secret".to_string(),
            provider: ExternalMirrorProvider::GitLab,
            direction,
            enabled: true,
            sync_merge_requests: true,
            label: "backup".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    World {
        _dir: dir,
        ctx,
        repo,
        mirror,
        gitlab,
    }
}

impl World {
    async fn sweep(&self) {
        engine::sync_repo_mirrors(&self.ctx, &self.repo).await.unwrap();
    }

    async fn local_mr(&self, title: &str, author: &str) -> u64 {
        merge_request::create(
            &self.ctx,
            &self.repo,
            NewMergeRequest {
                source_repo: &self.repo.uid,
                source_branch: "feature",
                target_branch: "main",
                title,
                description: "d",
                author,
            },
            gitquarry_i18n::Locale::En,
        )
        .await
        .unwrap()
        .number
    }

    async fn link(&self, local_number: u64, external_number: u64) {
        let link = ExternalPrLink {
            local_number,
            external_number,
            remote_updated_cursor: T0.to_string(),
            ..Default::default()
        };
        access::put_mr_link(
            self.ctx.backend().as_ref(),
            &self.ctx.cipher,
            &self.repo.uid,
            &self.mirror.id,
            &link,
            None,
        )
        .await
        .unwrap();
    }

    async fn links(&self) -> Vec<ExternalPrLink> {
        access::list_mr_links(
            self.ctx.backend().as_ref(),
            &self.ctx.cipher,
            &self.repo.uid,
            &self.mirror.id,
        )
        .await
        .unwrap()
    }

    async fn local_mrs(&self) -> Vec<merge_requests::MergeRequestMeta> {
        merge_requests::list_merge_requests(&self.ctx.backend(), &self.ctx.cipher, &self.repo.uid)
            .await
            .unwrap()
    }

    async fn add_local_comment(&self, number: u64, comment: MrComment) {
        merge_requests::add_mr_comment(
            self.ctx.backend().as_ref(),
            &self.ctx.cipher,
            &self.repo.uid,
            number,
            &comment,
        )
        .await
        .unwrap();
    }

    fn remote_note_bodies(&self, number: u64) -> Vec<String> {
        let gl = self.gitlab.read().unwrap();
        gl.notes
            .get(&number)
            .map(|n| n.iter().map(|n| n.body.clone()).collect())
            .unwrap_or_default()
    }
}

fn with_mr(mr: FakeMr) -> FakeGitLab {
    FakeGitLab {
        mrs: vec![mr],
        ..Default::default()
    }
}

#[tokio::test]
async fn discovers_a_new_remote_mr_and_links_it() {
    let w = world(
        SyncDirection::Bidirectional,
        with_mr(FakeMr::open(7, "feature", "external title", "alice", T0)),
    )
    .await;
    w.sweep().await;

    let local = w.local_mrs().await;
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].title, "external title");
    // Never plain "alice": that would hand a local alice this MR.
    assert_eq!(local[0].author, "127.0.0.1/alice");
    let links = w.links().await;
    assert_eq!(links.len(), 1);
    assert_eq!((links[0].external_number, links[0].local_number), (7, local[0].number));
}

#[tokio::test]
async fn creates_a_linked_remote_mr_for_an_unlinked_local_one() {
    let w = world(SyncDirection::Bidirectional, FakeGitLab::default()).await;
    w.local_mr("local title", "bob").await;
    w.sweep().await;

    let titles: Vec<String> = w.gitlab.read().unwrap().mrs.iter().map(|m| m.title.clone()).collect();
    assert_eq!(titles, ["local title"]);
    assert_eq!(w.links().await.len(), 1);
}

#[tokio::test]
async fn comment_anti_loop_exports_once_and_imports_once() {
    let mut gitlab = with_mr(FakeMr::open(7, "feature", "t", "bob", T0));
    gitlab.next_note_id = 99;
    gitlab.notes.insert(
        7,
        vec![FakeNote {
            id: 99,
            author: "dave".to_string(),
            body: "from gitlab".to_string(),
        }],
    );
    let w = world(SyncDirection::Bidirectional, gitlab).await;
    let mr = w.local_mr("t", "bob").await;
    w.link(mr, 7).await;

    // One genuinely-local comment, and one already imported from this very
    // mirror (which must never be re-exported).
    merge_request::add_comment(&w.ctx, &w.repo, mr, "alice", "local reply", gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    w.add_local_comment(
        mr,
        MrComment {
            author: "carol".to_string(),
            body: "already from gitlab".to_string(),
            created_at: T0.to_string(),
            origin: Some(w.mirror.id.clone()),
            ..Default::default()
        },
    )
    .await;

    w.sweep().await;
    let exported = w.remote_note_bodies(7);
    assert_eq!(exported.iter().filter(|b| *b == "local reply").count(), 1);
    assert!(!exported.iter().any(|b| b == "already from gitlab"));
    let imported = merge_requests::list_mr_comments(&w.ctx.backend(), &w.ctx.cipher, &w.repo.uid, mr)
        .await
        .unwrap();
    assert_eq!(imported.iter().filter(|c| c.body == "from gitlab").count(), 1);
    assert!(
        imported
            .iter()
            .any(|c| c.body == "from gitlab" && c.author == "127.0.0.1/dave")
    );

    w.sweep().await;
    assert_eq!(
        w.remote_note_bodies(7).len(),
        exported.len(),
        "no duplicate export on a second tick"
    );
    let imported_again = merge_requests::list_mr_comments(&w.ctx.backend(), &w.ctx.cipher, &w.repo.uid, mr)
        .await
        .unwrap();
    assert_eq!(
        imported_again.len(),
        imported.len(),
        "no duplicate import on a second tick"
    );
}

#[tokio::test]
async fn a_line_comment_carries_its_position_into_the_exported_body() {
    let w = world(SyncDirection::Bidirectional, FakeGitLab::default()).await;
    let mr = w.local_mr("t", "bob").await;
    w.add_local_comment(
        mr,
        MrComment {
            author: "alice".to_string(),
            body: "looks good".to_string(),
            created_at: T0.to_string(),
            ..Default::default()
        },
    )
    .await;
    w.add_local_comment(
        mr,
        MrComment {
            author: "alice".to_string(),
            body: "why this?".to_string(),
            created_at: T0.to_string(),
            anchor: Some(CommentAnchor {
                path: "src/main.rs".to_string(),
                side: DiffSide::New,
                line: 42,
            }),
            ..Default::default()
        },
    )
    .await;
    w.sweep().await;

    let external = w.links().await[0].external_number;
    let mut bodies = w.remote_note_bodies(external);
    bodies.sort();
    assert_eq!(bodies, ["`src/main.rs:42`\n\nwhy this?", "looks good"]);
}

#[tokio::test]
async fn merge_on_the_external_side_marks_the_local_mr_merged() {
    let mut remote = FakeMr::open(7, "feature", "t", "bob", T0);
    merge(&mut remote);
    let w = world(SyncDirection::Bidirectional, with_mr(remote)).await;
    let mr = w.local_mr("t", "bob").await;
    w.link(mr, 7).await;
    w.sweep().await;

    let local = merge_request::get(&w.ctx, &w.repo, mr).await.unwrap().unwrap();
    assert_eq!(local.status, MrStatus::Merged);
    assert_eq!(local.merge_commit_sha.as_deref(), Some("fakemergesha"));
}

#[tokio::test]
async fn remote_title_change_wins_when_newer_than_the_cursor() {
    let w = world(
        SyncDirection::Bidirectional,
        with_mr(FakeMr::open(
            7,
            "feature",
            "new title from gitlab",
            "bob",
            "2024-01-02T00:00:00Z",
        )),
    )
    .await;
    let mr = w.local_mr("old title", "bob").await;
    w.link(mr, 7).await;
    w.sweep().await;

    let local = merge_request::get(&w.ctx, &w.repo, mr).await.unwrap().unwrap();
    assert_eq!(local.title, "new title from gitlab");
}

#[tokio::test]
async fn a_pull_only_mirror_never_creates_a_mr_on_the_external_side() {
    let w = world(SyncDirection::PullOnly, FakeGitLab::default()).await;
    w.local_mr("local only", "alice").await;
    w.sweep().await;

    assert!(
        w.gitlab.read().unwrap().mrs.is_empty(),
        "a pull-only mirror pushed an MR"
    );
    assert!(w.links().await.is_empty());
}

#[tokio::test]
async fn a_push_only_mirror_never_imports_a_remote_mr() {
    let w = world(
        SyncDirection::PushOnly,
        with_mr(FakeMr::open(9, "feature", "external title", "bob", T0)),
    )
    .await;
    w.sweep().await;

    assert!(
        w.local_mrs().await.is_empty(),
        "a push-only mirror imported a remote MR"
    );
}

/// The anti-loop bookkeeping has to survive a lost CAS race on the link:
/// dropping it (what unwrapping the `Cas` away used to do) means every
/// comment gets exported a second time on the next pass.
#[tokio::test]
async fn a_concurrent_link_write_doesnt_cost_the_comment_dedup_state() {
    let w = world(SyncDirection::Bidirectional, FakeGitLab::default()).await;
    let mr = w.local_mr("local", "alice").await;
    w.add_local_comment(
        mr,
        MrComment {
            author: "alice".to_string(),
            body: "mine to export".to_string(),
            created_at: T0.to_string(),
            ..Default::default()
        },
    )
    .await;
    w.sweep().await;
    let external = w.links().await[0].external_number;
    assert_eq!(w.remote_note_bodies(external).len(), 1);

    // Someone else writes the link between the two passes, invalidating the
    // etag the next pass would have used.
    let (stored, etag) = access::get_mr_link(w.ctx.backend().as_ref(), &w.ctx.cipher, &w.repo.uid, &w.mirror.id, mr)
        .await
        .unwrap();
    let mut stored = stored.unwrap();
    stored.remote_updated_cursor = "2024-01-02T00:00:00Z".to_string();
    access::put_mr_link(
        w.ctx.backend().as_ref(),
        &w.ctx.cipher,
        &w.repo.uid,
        &w.mirror.id,
        &stored,
        etag.as_deref(),
    )
    .await
    .unwrap();

    w.sweep().await;
    assert_eq!(
        w.remote_note_bodies(external).len(),
        1,
        "the comment was exported twice"
    );
}

#[tokio::test]
async fn only_open_same_repo_mrs_are_exported() {
    let w = world(SyncDirection::Bidirectional, FakeGitLab::default()).await;
    let closed = w.local_mr("closed one", "bob").await;
    merge_request::close(&w.ctx, &w.repo, closed, gitquarry_i18n::Locale::En)
        .await
        .unwrap();
    let fork = w.local_mr("from a fork", "bob").await;
    let (mr, etag) = merge_requests::get_merge_request(w.ctx.backend().as_ref(), &w.ctx.cipher, &w.repo.uid, fork)
        .await
        .unwrap();
    let mut mr = mr.unwrap();
    mr.source_repo = gitquarry_core::storage::uid::Uid::from("bob/proj".to_string());
    merge_requests::put_merge_request(
        w.ctx.backend().as_ref(),
        &w.ctx.cipher,
        &w.repo.uid,
        &mr,
        etag.as_deref(),
    )
    .await
    .unwrap();
    w.sweep().await;

    assert!(w.gitlab.read().unwrap().mrs.is_empty());
    assert!(w.links().await.is_empty());
}

#[tokio::test]
async fn a_rejected_export_stops_being_retried() {
    let w = world(
        SyncDirection::Bidirectional,
        FakeGitLab {
            reject_creates: true,
            ..Default::default()
        },
    )
    .await;
    w.local_mr("rejected", "bob").await;
    for _ in 0..access::MAX_MR_EXPORT_ATTEMPTS + 2 {
        w.sweep().await;
    }
    assert_eq!(w.gitlab.read().unwrap().create_attempts, access::MAX_MR_EXPORT_ATTEMPTS);
}

/// An MR opened from a fork carries a head branch that lives in that fork,
/// which Track A never mirrors — so it can never be imported, and must be
/// skipped instead of failing `create` again on every tick.
#[tokio::test]
async fn an_external_mr_whose_branches_arent_mirrored_is_skipped_not_retried() {
    let w = world(
        SyncDirection::Bidirectional,
        with_mr(FakeMr::open(11, "contributor-patch", "from a fork", "carol", T0)),
    )
    .await;
    w.sweep().await;

    assert!(w.local_mrs().await.is_empty());
    assert!(
        w.links().await.is_empty(),
        "an MR that can't be imported shouldn't get a link"
    );
}
