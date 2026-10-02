//! `GitLabForge` against a local stand-in for the GitLab v4 API, so what is
//! checked is the request it actually sends and what it makes of the reply.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use gitquarry_forge_clients::gitlab::GitLabForge;
use gitquarry_forge_clients::{ForgeApi, RemoteMrState};
use serde_json::json;
use tokio::sync::mpsc;

struct Seen {
    path_and_query: String,
    token: Option<String>,
}

/// Serves `reply(path)` for every request and reports each one it saw.
async fn fake_gitlab(reply: fn(&str) -> Option<serde_json::Value>) -> (String, mpsc::UnboundedReceiver<Seen>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(move |req: Request| {
        let tx = tx.clone();
        async move {
            let path = req.uri().path().to_string();
            let _ = tx.send(Seen {
                path_and_query: req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default(),
                token: req
                    .headers()
                    .get("PRIVATE-TOKEN")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
            });
            match reply(&path) {
                Some(body) => axum::Json(body).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, rx)
}

fn merged_mr_without_merge_commit_sha() -> serde_json::Value {
    json!({
        "iid": 5,
        "title": "t",
        "state": "merged",
        "author": {"username": "alice"},
        "source_branch": "feature",
        "target_branch": "main",
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-02T00:00:00Z",
        "merged_at": "2024-01-02T00:00:00Z",
        "merged_by": {"username": "bob"},
        "merge_commit_sha": null,
        "sha": "fastforwardsha"
    })
}

#[tokio::test]
async fn derives_the_api_base_from_the_remote_and_url_encodes_a_nested_project_path() {
    let (base, mut seen) = fake_gitlab(|_| None).await;

    for (remote, project) in [
        (format!("{base}/group/sub/project.git"), "group%2Fsub%2Fproject"),
        (format!("{base}/acme/demo"), "acme%2Fdemo"),
    ] {
        let forge = GitLabForge::new(&remote, "tok").unwrap();
        assert!(forge.get_merge_request(5).await.unwrap().is_none());
        let req = seen.recv().await.unwrap();
        assert_eq!(
            req.path_and_query,
            format!("/api/v4/projects/{project}/merge_requests/5")
        );
        assert_eq!(req.token.as_deref(), Some("tok"));
    }
}

#[test]
fn rejects_a_url_with_no_project_path() {
    assert!(GitLabForge::new("https://gitlab.com/", "tok").is_err());
    assert!(GitLabForge::new("https://gitlab.com", "tok").is_err());
}

#[test]
fn a_project_path_that_cant_name_a_real_project_is_refused() {
    // Percent-encoding the separators isn't enough: `?`/`#` would still
    // truncate the REST path into a query or fragment.
    assert!(GitLabForge::new("https://gitlab.example.com/group/proj?x=1", "tok").is_err());
    assert!(GitLabForge::new("https://gitlab.example.com/group/../root", "tok").is_err());
    assert!(GitLabForge::new("https://gitlab.example.com/group/sub/proj.git", "tok").is_ok());
}

#[tokio::test]
async fn a_merged_mr_falls_back_to_sha_when_merge_commit_sha_is_absent() {
    let (base, _seen) = fake_gitlab(|path| {
        path.ends_with("/merge_requests/5")
            .then(merged_mr_without_merge_commit_sha)
    })
    .await;
    let forge = GitLabForge::new(&format!("{base}/acme/demo"), "tok").unwrap();

    let mr = forge.get_merge_request(5).await.unwrap().unwrap();
    assert_eq!(mr.state, RemoteMrState::Merged);
    assert_eq!(mr.merge_commit_sha.as_deref(), Some("fastforwardsha"));
    assert_eq!(mr.merged_by.as_deref(), Some("bob"));
}

#[tokio::test]
async fn list_comments_drops_system_notes_and_keeps_notes_without_the_flag() {
    let (base, _seen) = fake_gitlab(|path| {
        path.ends_with("/merge_requests/5/notes").then(|| {
            json!([
                {"id": 1, "author": {"username": "alice"}, "body": "changed the description",
                 "created_at": "2024-01-01T00:00:00Z", "system": true},
                // An older GitLab omits `system` entirely; that's a real comment.
                {"id": 2, "author": {"username": "bob"}, "body": "looks good",
                 "created_at": "2024-01-01T00:00:01Z"}
            ])
        })
    })
    .await;
    let forge = GitLabForge::new(&format!("{base}/acme/demo"), "tok").unwrap();

    let comments = forge.list_comments(5).await.unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, 2);
    assert_eq!(comments[0].author, "bob");
    assert_eq!(comments[0].body, "looks good");
}
