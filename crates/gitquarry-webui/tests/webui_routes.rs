//! Properties of the route table itself, not of any one handler.
//!
//! The mutation targets (`/_merges`, `/_tags/delete`, ...) are POST-only by
//! design, and a rejected submit re-renders in the POST response, so that
//! URL is what a browser is left holding. Reloading it must land somewhere
//! real — before `method_not_allowed_fallback`, axum answered a bare 405
//! with an empty body, because the path matched and only the method didn't.
//!
//! The list of paths is read out of `webui/mod.rs` rather than hardcoded:
//! a route added tomorrow has to be covered without anyone remembering this
//! file exists.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use gitquarry_core::app::AppContext;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

const ROUTES_SOURCE: &str = include_str!("../src/webui/mod.rs");

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[7u8; s3db_crypto::SALT_LEN]).unwrap());
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(60),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

/// Every `.route("<path>", ...)` in `webui/mod.rs`, with the methods that
/// registration names. Deliberately a source scan and not a list kept by
/// hand: axum exposes no way to enumerate a built `Router`, and a table
/// this test can't see is a table it can't defend.
fn registered_routes() -> Vec<(String, bool, bool)> {
    let mut out: Vec<(String, bool, bool)> = Vec::new();
    for chunk in ROUTES_SOURCE.split(".route(").skip(1) {
        let Some(start) = chunk.find('"') else { continue };
        let Some(len) = chunk[start + 1..].find('"') else {
            continue;
        };
        let path = &chunk[start + 1..start + 1 + len];
        if !path.starts_with('/') {
            continue;
        }
        // Everything up to the next registration is this one's method list:
        // `get(...)`, `.post(...)`, or both chained.
        let body = &chunk[start + 1 + len..];
        let get = body.contains("get(");
        let post = body.contains("post(");
        if let Some(existing) = out.iter_mut().find(|(p, _, _)| p == path) {
            existing.1 |= get;
            existing.2 |= post;
        } else {
            out.push((path.to_string(), get, post));
        }
    }
    out
}

/// `/{*repo}` and friends can't be requested literally.
fn concrete(path: &str) -> String {
    let mut out = String::new();
    for segment in path.split('/').skip(1) {
        out.push('/');
        if segment.starts_with('{') {
            out.push_str("placeholder");
        } else {
            out.push_str(segment);
        }
    }
    out
}

#[test]
fn the_source_scan_finds_the_route_table() {
    let routes = registered_routes();
    // A parser that silently matched nothing would make every other test
    // here pass vacuously.
    assert!(routes.len() > 50, "only found {} routes", routes.len());
    assert!(
        routes
            .iter()
            .any(|(p, get, post)| p == "/_merges/create/{*repo}" && *get && *post),
        "the merge-request create target should answer both methods"
    );
}

#[tokio::test]
async fn no_post_only_route_answers_a_stray_get_with_405() {
    let dir = tempfile::tempdir().unwrap();
    let app = gitquarry_webui::webui::router(Arc::new(test_ctx(dir.path())));

    let post_only: Vec<String> = registered_routes()
        .into_iter()
        .filter(|(_, get, post)| *post && !*get)
        .map(|(path, _, _)| path)
        .collect();
    assert!(!post_only.is_empty(), "expected the form targets to be POST-only");

    for path in post_only {
        let mut request = Request::builder()
            .method("GET")
            .uri(concrete(&path))
            .body(Body::empty())
            .unwrap();
        // `run_server` serves with `into_make_service_with_connect_info`, so
        // the git-smart-HTTP layer's rate limiter always finds this; a bare
        // `oneshot` doesn't add it, and its absence 500s before routing.
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
        let response = app.clone().oneshot(request).await.unwrap();
        assert_ne!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "GET {path} is a dead end: a reload after a rejected submit lands here"
        );
    }
}
