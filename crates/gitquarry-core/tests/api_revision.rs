//! `api::revision` as a repo read uses it: `?rev=` checked at the edge,
//! resolved once to a full commit sha, and answered as `revision`. No
//! `/_api/v1` route reads a repo yet, so these mount one over a real git
//! repo in a temp dir.

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use gitquarry_core::api::revision::{Pinned, RevisionRequest};
use gitquarry_test_support::send;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "tag.gpgSign=false",
        ])
        .args(["-c", "commit.gpgSign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

async fn read(Extension(git_dir): Extension<Arc<PathBuf>>, rev: RevisionRequest) -> Response {
    match rev.resolve(&git_dir).await {
        Ok(revision) => Json(Pinned {
            revision: &revision,
            body: json!({ "asked": rev.rev() }),
        })
        .into_response(),
        Err(e) => e.into_response(),
    }
}

fn app(git_dir: &Path) -> Router {
    Router::new()
        .route("/read", get(read))
        .layer(Extension(Arc::new(git_dir.to_path_buf())))
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let page = send(app, Request::get(uri).body(Body::empty()).unwrap()).await;
    (page.status, serde_json::from_str(&page.body).unwrap())
}

/// A repo on `main` with two commits and an annotated tag `v1` on the first.
fn repo(dir: &Path) -> (PathBuf, String, String) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
    let first = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["tag", "-a", "v1", "-m", "v1"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let second = git(dir, &["rev-parse", "HEAD"]);
    (dir.join(".git"), first, second)
}

#[tokio::test]
async fn a_read_answers_with_the_full_sha_it_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let (git_dir, first, second) = repo(dir.path());
    let app = app(&git_dir);

    let short = &first[..12];
    for (uri, sha) in [
        ("/read".to_string(), &second),
        ("/read?rev=".to_string(), &second),
        ("/read?rev=main".to_string(), &second),
        ("/read?rev=v1".to_string(), &first),
        ("/read?rev=main~1".to_string(), &first),
        (format!("/read?rev={short}"), &first),
    ] {
        let (status, body) = get_json(&app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        assert_eq!(body["revision"], sha.as_str(), "{uri}");
    }

    // Repeating the answer's sha reads the same commit after the branch moves.
    let (_, body) = get_json(&app, "/read").await;
    let pinned = body["revision"].as_str().unwrap().to_string();
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "three"]);
    let (_, again) = get_json(&app, &format!("/read?rev={pinned}")).await;
    assert_eq!(again["revision"], pinned);
    assert_eq!(again["asked"], pinned);
}

#[tokio::test]
async fn an_unknown_rev_is_not_found_and_a_bad_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (git_dir, _, _) = repo(dir.path());
    let app = app(&git_dir);

    let (status, body) = get_json(&app, "/read?rev=nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    for rev in ["-x", "main..v1", "a%20b"] {
        let (status, body) = get_json(&app, &format!("/read?rev={rev}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rev}");
        assert_eq!(body["code"], "invalid_parameter", "{rev}");
    }
}

#[tokio::test]
async fn an_empty_repo_has_no_revision_to_read() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let app = app(&dir.path().join(".git"));

    let (status, body) = get_json(&app, "/read").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}
