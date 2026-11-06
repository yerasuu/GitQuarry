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
use axum::http::{Request, StatusCode};
use gitquarry_test_support::{ctx, send};
use std::sync::Arc;

const ROUTES_SOURCE: &str = include_str!("../src/webui/mod.rs");

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
    let app = gitquarry_webui::webui::router(Arc::new(ctx(dir.path())));

    let post_only: Vec<String> = registered_routes()
        .into_iter()
        .filter(|(_, get, post)| *post && !*get)
        .map(|(path, _, _)| path)
        .collect();
    assert!(!post_only.is_empty(), "expected the form targets to be POST-only");

    for path in post_only {
        let request = Request::get(concrete(&path)).body(Body::empty()).unwrap();
        assert_ne!(
            send(&app, request).await.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "GET {path} is a dead end: a reload after a rejected submit lands here"
        );
    }
}
