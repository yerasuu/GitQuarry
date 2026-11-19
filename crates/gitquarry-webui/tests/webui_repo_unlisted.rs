//! An `Unlisted` repo on the web: opens for anyone holding its URL, signed
//! in or not, and stays off the index for everyone but the listings its
//! owner keeps for themselves.

use axum::http::StatusCode;
use gitquarry_core::auth::access;
use gitquarry_core::storage::entities::{OwnerRef, Visibility};
use gitquarry_test_support::{Session, Work, ctx};
use std::sync::Arc;

struct Instance {
    _dir: tempfile::TempDir,
    app: axum::Router,
}

impl Instance {
    /// `alice/hidden` (unlisted, with a README) and `alice/open` (public),
    /// both alice's, plus a `bob` with no grant on either.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Arc::new(ctx(dir.path()));
        gitquarry_test_support::user(&ctx, "alice", false).await;
        gitquarry_test_support::user(&ctx, "bob", false).await;
        let alice = access::get_user(ctx.backend().as_ref(), &ctx.cipher, "alice")
            .await
            .unwrap()
            .0
            .unwrap();
        for (name, visibility) in [
            ("alice/hidden", Visibility::Unlisted),
            ("alice/open", Visibility::Public),
        ] {
            let owner = Some(OwnerRef::User(alice.uid.clone()));
            let resolved = gitquarry_core::domain::repo::create(&ctx, name, None, owner, None, visibility)
                .await
                .unwrap()
                .unwrap();
            let work = Work::new().await;
            work.commit(&[("README.md", "# Readme of the repo\n")], "first").await;
            work.push(&resolved, "refs/heads/main").await;
        }
        let app = gitquarry_webui::webui::router(ctx);
        Self { _dir: dir, app }
    }
}

#[tokio::test]
async fn the_index_lists_an_unlisted_repo_to_nobody() {
    let gq = Instance::new().await;
    let bob = Session::login(&gq.app, "bob").await;
    let alice = Session::login(&gq.app, "alice").await;
    for cookie in [None, Some(bob.cookie.as_str()), Some(alice.cookie.as_str())] {
        let page = Session::get_as(&gq.app, "/", cookie).await;
        assert_eq!(page.status, StatusCode::OK);
        assert!(page.body.contains("alice/open"), "{}", page.body);
        assert!(!page.body.contains("alice/hidden"), "{}", page.body);
    }
}

#[tokio::test]
async fn its_owner_still_finds_it_among_their_own_repos() {
    let gq = Instance::new().await;
    let alice = Session::login(&gq.app, "alice").await;
    let page = alice.get(&gq.app, "/_repos").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(r#"href="/alice/hidden""#), "{}", page.body);
}

#[tokio::test]
async fn anyone_with_the_link_reads_it_without_signing_in() {
    let gq = Instance::new().await;
    let bob = Session::login(&gq.app, "bob").await;
    for cookie in [None, Some(bob.cookie.as_str())] {
        for path in [
            "/alice/hidden",
            "/alice/hidden?view=tree",
            "/alice/hidden?view=log",
            "/alice/hidden/blob/main/README.md",
        ] {
            let page = Session::get_as(&gq.app, path, cookie).await;
            assert_eq!(page.status, StatusCode::OK, "GET {path}: {}", page.body);
        }
        let page = Session::get_as(&gq.app, "/alice/hidden", cookie).await;
        assert!(page.body.contains("Readme of the repo"), "{}", page.body);
        assert!(page.body.contains("unlisted"), "{}", page.body);
    }
}

/// Reading is all the link grants: the settings form still needs an admin.
#[tokio::test]
async fn reading_it_by_link_does_not_open_its_settings() {
    let gq = Instance::new().await;
    let bob = Session::login(&gq.app, "bob").await;
    let settings = bob.get(&gq.app, "/alice/hidden?view=settings").await;
    assert_ne!(settings.status, StatusCode::OK, "{}", settings.body);
    assert!(!settings.body.contains(r#"name="visibility""#), "{}", settings.body);
}
