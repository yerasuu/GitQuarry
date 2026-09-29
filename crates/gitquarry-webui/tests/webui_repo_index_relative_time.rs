//! How the home page words a repo's last push: the card's spelled-out
//! "updated 3 hours ago" and the compact list's "3h ago".
//!
//! Both are measured against the server's own clock at render time, so each
//! case stamps `last_push` relative to the test's clock just before the
//! request. That only ever adds a second or two to the age, never subtracts,
//! so every case sits on or just past a bucket's lower edge; the old
//! one-second-below-the-edge cases (`HOUR - 1` reading "59 minutes ago")
//! can't be pinned without controlling the server's clock and were dropped.

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{ctx, send};
use std::sync::Arc;

const MINUTE: u64 = 60;
const HOUR: u64 = 3600;
const DAY: u64 = 86_400;
const WEEK: u64 = 604_800;
const MONTH: u64 = 2_629_800;
const YEAR: u64 = 31_557_600;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

struct Index {
    _root: tempfile::TempDir,
    ctx: Arc<AppContext>,
    resolved: ResolvedRepo,
    app: Router,
}

async fn index() -> Index {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let resolved = gitquarry_core::domain::repo::create(&ctx, "acme/web", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    Index {
        _root: root,
        ctx,
        resolved,
        app,
    }
}

/// The repo list as the home page renders it (its live-search fragment,
/// `x-gq-async`, so the sidebar's own stamps can't be mistaken for the
/// row's) with `acme/web` last pushed at `last_push`.
async fn repo_list_pushed_at(index: &Index, last_push: u64) -> String {
    let backend = index.ctx.backend();
    let uid = index.resolved.uid.as_str();
    let (mut config, etag) =
        gitquarry_core::storage::settings::get_repo_config(backend.as_ref(), uid, &index.ctx.cipher)
            .await
            .unwrap();
    config.last_push = Some(last_push);
    gitquarry_core::storage::settings::put_repo_config(
        backend.as_ref(),
        uid,
        &config,
        etag.as_deref(),
        &index.ctx.cipher,
    )
    .await
    .unwrap();

    let request = Request::get("/").header("x-gq-async", "1").body(Body::empty()).unwrap();
    send(&index.app, request).await.body
}

async fn words_ago(index: &Index, secs_ago: u64) -> String {
    let html = repo_list_pushed_at(index, now() - secs_ago).await;
    let start = html
        .find("<span>updated ")
        .unwrap_or_else(|| panic!("no card stamp: {html}"))
        + "<span>updated ".len();
    let len = html[start..].find("</span>").unwrap();
    html[start..start + len].to_string()
}

async fn short_ago(index: &Index, secs_ago: u64) -> String {
    let html = repo_list_pushed_at(index, now() - secs_ago).await;
    let marker = r#"<span class="muted">"#;
    let start = html.find(marker).unwrap_or_else(|| panic!("no compact stamp: {html}")) + marker.len();
    let len = html[start..].find("</span>").unwrap();
    html[start..start + len].to_string()
}

#[tokio::test]
async fn words_covers_every_bucket_the_mockup_shows() {
    let index = index().await;
    assert_eq!(words_ago(&index, 0).await, "just now");
    assert_eq!(words_ago(&index, 3 * HOUR).await, "3 hours ago");
    assert_eq!(words_ago(&index, 5 * HOUR).await, "5 hours ago");
    assert_eq!(words_ago(&index, DAY).await, "yesterday");
    assert_eq!(words_ago(&index, 6 * DAY).await, "6 days ago");
    assert_eq!(words_ago(&index, 3 * WEEK).await, "3 weeks ago");
    assert_eq!(words_ago(&index, 2 * MONTH).await, "2 months ago");
}

#[tokio::test]
async fn words_singular_forms_have_no_trailing_s() {
    let index = index().await;
    assert_eq!(words_ago(&index, MINUTE).await, "1 minute ago");
    assert_eq!(words_ago(&index, HOUR).await, "1 hour ago");
    assert_eq!(words_ago(&index, WEEK).await, "1 week ago");
    assert_eq!(words_ago(&index, MONTH).await, "1 month ago");
    assert_eq!(words_ago(&index, YEAR).await, "1 year ago");
}

#[tokio::test]
async fn words_plural_forms_have_a_trailing_s() {
    let index = index().await;
    assert_eq!(words_ago(&index, 2 * MINUTE).await, "2 minutes ago");
    assert_eq!(words_ago(&index, 2 * HOUR).await, "2 hours ago");
    assert_eq!(words_ago(&index, 2 * WEEK).await, "2 weeks ago");
    assert_eq!(words_ago(&index, 2 * MONTH).await, "2 months ago");
    assert_eq!(words_ago(&index, 2 * YEAR).await, "2 years ago");
}

/// `yesterday` is its own bucket, not "1 day ago" — it covers the whole
/// [24h, 48h) span, not just exactly-24h.
#[tokio::test]
async fn words_yesterday_covers_the_whole_24_to_48_hour_span() {
    let index = index().await;
    assert_eq!(words_ago(&index, DAY).await, "yesterday");
    assert_eq!(words_ago(&index, DAY + 23 * HOUR).await, "yesterday");
    assert_eq!(words_ago(&index, 2 * DAY).await, "2 days ago");
}

#[tokio::test]
async fn short_covers_every_bucket_with_a_unit_letter() {
    let index = index().await;
    assert_eq!(short_ago(&index, 0).await, "just now");
    assert_eq!(short_ago(&index, MINUTE).await, "1m ago");
    assert_eq!(short_ago(&index, 3 * HOUR).await, "3h ago");
    assert_eq!(short_ago(&index, DAY).await, "1d ago");
    assert_eq!(short_ago(&index, 6 * DAY).await, "6d ago");
    assert_eq!(short_ago(&index, 3 * WEEK).await, "3w ago");
    assert_eq!(short_ago(&index, 2 * MONTH).await, "2mo ago");
    assert_eq!(short_ago(&index, 2 * YEAR).await, "2y ago");
}

/// A `last_push` ahead of the server's clock (clock skew between the
/// pushing process and this one) reads as "just now" instead of
/// underflowing the age.
#[tokio::test]
async fn a_future_timestamp_reads_as_just_now() {
    let index = index().await;
    let ahead = now() + DAY;
    let html = repo_list_pushed_at(&index, ahead).await;
    assert!(html.contains("<span>updated just now</span>"), "{html}");
    assert!(html.contains(r#"<span class="muted">just now</span>"#), "{html}");
}
