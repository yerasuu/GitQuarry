//! API token scopes and path lists (decision #144): what a token may do is
//! the lower of its owner's live permission and its own ceiling, and git and
//! API tokens are each refused by the other surface.

use axum::extract::FromRequestParts;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::api::auth::{ApiPrincipal, Area};
use gitquarry_core::api::error::{ApiError, ErrorCode};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{ApiGrant, ApiScope, Permission, User};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::uid::Uid;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

async fn repo(ctx: &AppContext, full_name: &str) -> Uid {
    gitquarry_core::domain::repo::create(ctx, full_name, None, None, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap()
        .uid
}

/// Rewrites `username`'s stored record through `edit`.
async fn edit_user(ctx: &AppContext, username: &str, edit: impl FnOnce(&mut User)) {
    let (user, etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, username)
        .await
        .unwrap();
    let mut user = user.unwrap();
    edit(&mut user);
    access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref())
        .await
        .unwrap();
}

fn grant(scopes: &[ApiScope]) -> ApiGrant {
    ApiGrant {
        scopes: scopes.iter().copied().collect(),
        paths: BTreeMap::new(),
    }
}

async fn api_token(ctx: &AppContext, owner: &Uid, repo: &Uid, ceiling: Permission, grant: ApiGrant) -> String {
    domain::pat::create_api(
        ctx,
        owner,
        "agent",
        BTreeMap::from([(repo.clone(), ceiling)]),
        grant,
        None,
    )
    .await
    .unwrap()
    .unwrap()
    .1
}

/// The principal `/_api/v1` would act as for `secret`, through the public
/// extractor every endpoint takes.
async fn principal(ctx: &Arc<AppContext>, secret: &str) -> Result<ApiPrincipal, ApiError> {
    let (mut parts, ()) = Request::get("/_api/v1/whoami")
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .body(())
        .unwrap()
        .into_parts();
    ApiPrincipal::from_request_parts(&mut parts, ctx).await
}

#[tokio::test]
async fn no_scope_raises_the_owners_permission() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    account(&ctx, "alice").await;
    let bob = account(&ctx, "bob").await;
    let proj = repo(&ctx, "alice/proj").await;
    let granted = proj.clone();
    edit_user(&ctx, "bob", |bob| {
        bob.repos.insert(granted, Permission::Reporter);
    })
    .await;

    let mut sets: Vec<Vec<ApiScope>> = ApiScope::ALL.iter().map(|&scope| vec![scope]).collect();
    sets.push(ApiScope::ALL.to_vec());
    for scopes in sets {
        let secret = api_token(&ctx, &bob.uid, &proj, Permission::Owner, grant(&scopes)).await;
        let bob = principal(&ctx, &secret).await.unwrap();
        for area in [Area::Repo, Area::Tracker] {
            let permission = bob.permission_on(&ctx, area, &proj, "alice/proj").await.unwrap();
            assert!(
                permission <= Some(Permission::Reporter),
                "{scopes:?} {area:?}: {permission:?}"
            );
            let admin = bob
                .require(&ctx, area, &proj, "alice/proj", Permission::Maintainer)
                .await
                .unwrap_err();
            assert_ne!(admin.code(), ErrorCode::Internal, "{scopes:?} {area:?}");
        }
    }
}

#[tokio::test]
async fn an_admins_token_gets_no_admin_bypass() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    account(&ctx, "alice").await;
    let root = account(&ctx, "root").await;
    edit_user(&ctx, "root", |root| root.is_admin = true).await;
    let proj = repo(&ctx, "alice/proj").await;

    let secret = api_token(&ctx, &root.uid, &proj, Permission::Owner, grant(&ApiScope::ALL)).await;
    let root = principal(&ctx, &secret).await.unwrap();

    assert_eq!(
        root.permission_on(&ctx, Area::Repo, &proj, "alice/proj").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_token_without_repo_admin_cannot_administer() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice").await;
    let proj = repo(&ctx, "alice/proj").await;

    let write = api_token(
        &ctx,
        &alice.uid,
        &proj,
        Permission::Owner,
        grant(&[ApiScope::RepoRead, ApiScope::RepoWrite, ApiScope::TrackerWrite]),
    )
    .await;
    let write = principal(&ctx, &write).await.unwrap();
    assert_eq!(
        write
            .require(&ctx, Area::Repo, &proj, "alice/proj", Permission::Developer)
            .await
            .unwrap(),
        Permission::Developer
    );
    let denied = write
        .require(&ctx, Area::Repo, &proj, "alice/proj", Permission::Maintainer)
        .await
        .unwrap_err();
    assert_eq!(denied.code(), ErrorCode::Forbidden);
    assert_eq!(denied.code().status(), StatusCode::FORBIDDEN);

    let admin = api_token(
        &ctx,
        &alice.uid,
        &proj,
        Permission::Owner,
        grant(&[ApiScope::RepoAdmin]),
    )
    .await;
    let admin = principal(&ctx, &admin).await.unwrap();
    assert_eq!(
        admin
            .require(&ctx, Area::Repo, &proj, "alice/proj", Permission::Maintainer)
            .await
            .unwrap(),
        Permission::Owner
    );
    // `repo:admin` alone gives nothing on the tracker.
    assert_eq!(admin.ceiling(Area::Tracker, &proj), None);
}

#[tokio::test]
async fn the_per_repo_ceiling_still_caps_repo_admin() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice").await;
    let proj = repo(&ctx, "alice/proj").await;

    let secret = api_token(
        &ctx,
        &alice.uid,
        &proj,
        Permission::Maintainer,
        grant(&[ApiScope::RepoAdmin]),
    )
    .await;
    let alice = principal(&ctx, &secret).await.unwrap();
    assert_eq!(alice.ceiling(Area::Repo, &proj), Some(Permission::Maintainer));
    let other = repo(&ctx, "alice/other").await;
    assert_eq!(alice.ceiling(Area::Repo, &other), None);
    let unnamed = alice
        .require(&ctx, Area::Repo, &other, "alice/other", Permission::Reporter)
        .await
        .unwrap_err();
    assert_eq!(unnamed.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn a_path_outside_the_list_is_a_403() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice").await;
    let mono = repo(&ctx, "alice/mono").await;
    let open = repo(&ctx, "alice/open").await;
    let scopes = BTreeMap::from([
        (mono.clone(), Permission::Developer),
        (open.clone(), Permission::Developer),
    ]);
    let grant = ApiGrant {
        scopes: BTreeSet::from([ApiScope::RepoRead]),
        paths: BTreeMap::from([(mono.clone(), BTreeSet::from(["services/api".to_string()]))]),
    };
    let (_, secret) = domain::pat::create_api(&ctx, &alice.uid, "agent", scopes, grant, None)
        .await
        .unwrap()
        .unwrap();
    let alice = principal(&ctx, &secret).await.unwrap();

    for inside in ["services/api", "services/api/main.rs", "services/api/v1/x.rs"] {
        assert!(alice.require_path(&mono, inside).is_ok(), "{inside}");
    }
    for outside in [
        "",
        "services",
        "services/apis",
        "services/apix/main.rs",
        "web/index.html",
    ] {
        let err = alice.require_path(&mono, outside).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Forbidden, "{outside}");
        assert_eq!(err.code().status(), StatusCode::FORBIDDEN, "{outside}");
    }
    for invalid in [
        "services/api/../web",
        "/services/api",
        "services//api",
        "services/api\n",
    ] {
        let err = alice.require_path(&mono, invalid).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidParameter, "{invalid:?}");
    }
    // A repo without a list is open in full.
    assert!(alice.require_path(&open, "").is_ok());
    assert!(alice.require_path(&open, "web/index.html").is_ok());
}

#[tokio::test]
async fn create_api_validates_scopes_and_paths_at_the_edge() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice").await;
    let proj = repo(&ctx, "alice/proj").await;
    let scopes = || BTreeMap::from([(proj.clone(), Permission::Developer)]);
    let with_paths = |repo: &Uid, paths: &[&str]| ApiGrant {
        scopes: BTreeSet::from([ApiScope::RepoRead]),
        paths: BTreeMap::from([(repo.clone(), paths.iter().map(|p| p.to_string()).collect())]),
    };

    let no_scope = domain::pat::create_api(&ctx, &alice.uid, "agent", scopes(), grant(&[]), None).await;
    assert!(no_scope.is_err());
    for paths in [&["../etc"][..], &["/abs"], &["a\u{7}b"], &["a//b"], &[""], &["-x"], &[]] {
        let refused =
            domain::pat::create_api(&ctx, &alice.uid, "agent", scopes(), with_paths(&proj, paths), None).await;
        assert!(refused.is_err(), "{paths:?}");
    }
    let elsewhere = domain::pat::create_api(
        &ctx,
        &alice.uid,
        "agent",
        scopes(),
        with_paths(&Uid::new(), &["src"]),
        None,
    )
    .await;
    assert!(elsewhere.is_err());
    assert!(
        access::list_tokens_for_owner(ctx.backend().as_ref(), &ctx.cipher, &alice.uid)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_git_token_is_refused_by_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));
    let alice = account(&ctx, "alice").await;
    let proj = repo(&ctx, "alice/proj").await;
    let (_, secret) = domain::pat::create(
        &ctx,
        &alice.uid,
        "laptop",
        BTreeMap::from([(proj, Permission::Owner)]),
        None,
    )
    .await
    .unwrap()
    .unwrap();

    let err = principal(&ctx, &secret).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidToken);
}

async fn account(ctx: &AppContext, username: &str) -> User {
    gitquarry_test_support::account(ctx, username, None).await
}
