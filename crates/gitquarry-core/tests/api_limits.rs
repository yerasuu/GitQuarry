//! `/_api/v1`'s limits as a client meets them: the per-token rate and its
//! `RateLimit-*` fields, and the per-address limit on failed credentials.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::api;
use gitquarry_core::api::limits::ApiLimits;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain;
use gitquarry_core::auth::entities::{ApiGrant, ApiScope, Permission};
use gitquarry_core::config::ApiSection;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::uid::Uid;
use gitquarry_test_support::{Page, account, send};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

fn limited(root: &Path, section: ApiSection) -> Arc<AppContext> {
    Arc::new(AppContext {
        api_limits: ApiLimits::new(&section),
        ..gitquarry_test_support::ctx(root)
    })
}

/// alice and her repo `alice/proj`, which [`token`]s read.
struct Owner {
    alice: Uid,
    proj: Uid,
}

async fn owner(ctx: &AppContext) -> Owner {
    let alice = account(ctx, "alice", None).await.uid;
    let proj = gitquarry_core::domain::repo::create(ctx, "alice/proj", None, None, None, Visibility::Private)
        .await
        .unwrap()
        .unwrap()
        .uid;
    Owner { alice, proj }
}

/// A new API token reading `alice/proj`, returning its secret.
async fn token(ctx: &AppContext, owner: &Owner) -> String {
    let grant = ApiGrant {
        scopes: ApiScope::ALL.into_iter().collect(),
        paths: BTreeMap::new(),
    };
    let scopes = BTreeMap::from([(owner.proj.clone(), Permission::Reporter)]);
    domain::pat::create_api(ctx, &owner.alice, "agent", scopes, grant, None)
        .await
        .unwrap()
        .unwrap()
        .1
}

struct Call<'a> {
    path: &'a str,
    authorization: Option<&'a str>,
    peer: [u8; 4],
    forwarded_for: Option<&'a str>,
}

impl<'a> Call<'a> {
    fn get(path: &'a str, authorization: Option<&'a str>) -> Self {
        Call {
            path,
            authorization,
            peer: [192, 0, 2, 1],
            forwarded_for: None,
        }
    }

    async fn send(&self, ctx: &Arc<AppContext>) -> Page {
        let mut request = Request::get(self.path);
        if let Some(value) = self.authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        if let Some(value) = self.forwarded_for {
            request = request.header("x-forwarded-for", value);
        }
        let mut request = request.body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((self.peer, 4242))));
        let app = axum::Router::new().merge(api::router(ctx)).with_state(Arc::clone(ctx));
        send(&app, request).await
    }
}

fn header_u64(page: &Page, name: &str) -> u64 {
    page.headers
        .get(name)
        .unwrap_or_else(|| panic!("no {name} in {:?}", page.headers))
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn code(page: &Page) -> String {
    let body: Value = serde_json::from_str(&page.body).unwrap();
    assert!(body.get("hint").is_some(), "{body}");
    body["code"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_token_over_its_rate_gets_429_with_the_ratelimit_fields() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = limited(
        dir.path(),
        ApiSection {
            requests_per_minute: 3,
            ..ApiSection::default()
        },
    );
    let owner = owner(&ctx).await;
    let bearer = format!("Bearer {}", token(&ctx, &owner).await);

    for remaining in [2, 1, 0] {
        let page = Call::get("/_api/v1/whoami", Some(&bearer)).send(&ctx).await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.body);
        assert_eq!(header_u64(&page, "ratelimit-limit"), 3);
        assert_eq!(header_u64(&page, "ratelimit-remaining"), remaining);
        assert_eq!(page.headers["ratelimit-policy"], "3;w=60");
        assert!(page.headers.get(header::RETRY_AFTER).is_none());
    }

    let page = Call::get("/_api/v1/whoami", Some(&bearer)).send(&ctx).await;
    assert_eq!(page.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(code(&page), "rate_limited");
    assert_eq!(header_u64(&page, "ratelimit-remaining"), 0);
    let retry_after = header_u64(&page, header::RETRY_AFTER.as_str());
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(retry_after, header_u64(&page, "ratelimit-reset"));
}

#[tokio::test]
async fn rewriting_the_authorization_header_does_not_reset_the_quota() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = limited(
        dir.path(),
        ApiSection {
            requests_per_minute: 3,
            ..ApiSection::default()
        },
    );
    let owner = owner(&ctx).await;
    let secret = token(&ctx, &owner).await;

    for spelling in [
        format!("Bearer {secret}"),
        format!("bearer {secret}"),
        format!("BEARER  {secret} "),
    ] {
        let page = Call::get("/_api/v1/whoami", Some(&spelling)).send(&ctx).await;
        assert_eq!(page.status, StatusCode::OK, "{spelling:?}: {}", page.body);
    }
    let page = Call::get("/_api/v1/whoami", Some(&format!("bEaReR {secret}")))
        .send(&ctx)
        .await;
    assert_eq!(page.status, StatusCode::TOO_MANY_REQUESTS);

    // Another token of the same owner has its own quota.
    let other = format!("Bearer {}", token(&ctx, &owner).await);
    let page = Call::get("/_api/v1/whoami", Some(&other)).send(&ctx).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
}

#[tokio::test]
async fn failed_credentials_are_limited_per_address_before_any_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = limited(
        dir.path(),
        ApiSection {
            failed_auth_per_minute: 2,
            ..ApiSection::default()
        },
    );
    let owner = owner(&ctx).await;
    let valid = format!("Bearer {}", token(&ctx, &owner).await);
    let guess = format!("Bearer gqpat_{}", "0".repeat(40));

    let page = Call::get("/_api/v1/whoami", Some(&guess)).send(&ctx).await;
    assert_eq!(page.status, StatusCode::UNAUTHORIZED);
    assert_eq!(header_u64(&page, "ratelimit-limit"), 2);
    assert_eq!(header_u64(&page, "ratelimit-remaining"), 1);
    let page = Call::get("/_api/v1/whoami", None).send(&ctx).await;
    assert_eq!(page.status, StatusCode::UNAUTHORIZED);
    assert_eq!(header_u64(&page, "ratelimit-remaining"), 0);

    // Past the limit even a valid token is refused from this address, as
    // the server can't tell it from a guess without looking it up.
    for authorization in [Some(guess.as_str()), Some(valid.as_str()), None] {
        let page = Call::get("/_api/v1/whoami", authorization).send(&ctx).await;
        assert_eq!(page.status, StatusCode::TOO_MANY_REQUESTS, "{authorization:?}");
        assert_eq!(code(&page), "rate_limited");
        assert!(header_u64(&page, header::RETRY_AFTER.as_str()) >= 1);
    }

    let elsewhere = Call {
        peer: [198, 51, 100, 7],
        ..Call::get("/_api/v1/whoami", Some(&valid))
    };
    assert_eq!(elsewhere.send(&ctx).await.status, StatusCode::OK);
}

#[tokio::test]
async fn behind_a_trusted_proxy_failures_count_against_the_last_forwarded_hop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(AppContext {
        api_limits: ApiLimits::new(&ApiSection {
            failed_auth_per_minute: 1,
            ..ApiSection::default()
        }),
        trust_forwarded_for: true,
        ..gitquarry_test_support::ctx(dir.path())
    });
    let proxied = |forwarded_for| Call {
        forwarded_for: Some(forwarded_for),
        ..Call::get("/_api/v1/whoami", None)
    };

    assert_eq!(proxied("203.0.113.9").send(&ctx).await.status, StatusCode::UNAUTHORIZED);
    // A client-chosen first hop does not move it to a fresh bucket.
    let spoofed = proxied("10.9.8.7, 203.0.113.9").send(&ctx).await;
    assert_eq!(spoofed.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        proxied("203.0.113.10").send(&ctx).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn every_api_response_carries_the_ratelimit_fields() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(gitquarry_test_support::ctx(dir.path()));

    let page = Call::get("/_api/v1/no/such/endpoint", None).send(&ctx).await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert_eq!(header_u64(&page, "ratelimit-limit"), 30);
    assert_eq!(header_u64(&page, "ratelimit-remaining"), 30);
    assert!(page.headers.get("ratelimit-reset").is_some());
}
