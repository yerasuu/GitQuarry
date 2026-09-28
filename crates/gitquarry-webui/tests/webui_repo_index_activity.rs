//! The home page's "recent activity" sidebar: pushes (from each repo's own
//! `last_push*` fields), releases and merged MRs (from the audit log),
//! newest first, capped, and never about a repo the viewer can't see.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const HOUR: u64 = 3600;

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher =
        Arc::new(gitquarry_core::crypto::Cipher::derive("test passphrase", &[11u8; s3db_crypto::SALT_LEN]).unwrap());
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

/// `name`, last pushed `hours_ago` with `commits` commits to `branch`.
async fn pushed_repo(
    ctx: &AppContext,
    name: &str,
    visibility: Visibility,
    hours_ago: u64,
    commits: u32,
    branch: Option<&str>,
) {
    let resolved = gitquarry_core::domain::repo::create(ctx, name, None, None, None, visibility)
        .await
        .unwrap()
        .unwrap();
    let backend = ctx.backend();
    let uid = resolved.uid.as_str();
    let (mut config, etag) = gitquarry_core::storage::settings::get_repo_config(backend.as_ref(), uid, &ctx.cipher)
        .await
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    config.last_push = Some(now - hours_ago * HOUR);
    config.last_push_commits = Some(commits);
    config.last_push_branch = branch.map(str::to_string);
    gitquarry_core::storage::settings::put_repo_config(backend.as_ref(), uid, &config, etag.as_deref(), &ctx.cipher)
        .await
        .unwrap();
}

/// Each sidebar entry, in order, as "<repo> <description>".
async fn sidebar(ctx: Arc<AppContext>) -> Vec<String> {
    let app = gitquarry_webui::webui::router(ctx);
    let mut request = Request::builder().uri("/").body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.oneshot(request).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    let start = html
        .find(r#"<ul class="index-activity">"#)
        .unwrap_or_else(|| panic!("no sidebar: {html}"));
    let list = &html[start..start + html[start..].find("</ul>").unwrap()];
    list.split("<li>")
        .skip(1)
        .map(|item| {
            let repo_start = item.find(r#"">"#).unwrap() + 2;
            let repo = &item[repo_start..repo_start + item[repo_start..].find("</a>").unwrap()];
            let verb_start = item.find(r#"<span class="verb">— "#).unwrap() + r#"<span class="verb">— "#.len();
            let verb = &item[verb_start..verb_start + item[verb_start..].find("</span>").unwrap()];
            format!("{repo} {verb}")
        })
        .collect()
}

#[tokio::test]
async fn sorts_newest_first_and_caps_at_five() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    for (i, name) in ["a/one", "a/two", "a/three", "a/four", "a/five", "a/six"]
        .iter()
        .enumerate()
    {
        // Created out of age order so a list that merely kept creation order
        // would fail too.
        let hours_ago = [3, 1, 2, 6, 4, 5][i];
        pushed_repo(&ctx, name, Visibility::Public, hours_ago, 1, None).await;
    }

    let repos: Vec<String> = sidebar(ctx)
        .await
        .into_iter()
        .map(|entry| entry.split(' ').next().unwrap().to_string())
        .collect();
    assert_eq!(repos, ["a/two", "a/three", "a/one", "a/five", "a/six"]);
}

#[tokio::test]
async fn describes_pushes_releases_and_merged_mrs() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    pushed_repo(&ctx, "acme/one", Visibility::Public, 1, 1, Some("main")).await;
    pushed_repo(&ctx, "acme/two", Visibility::Public, 2, 4, Some("master")).await;
    pushed_repo(&ctx, "acme/three", Visibility::Public, 3, 2, None).await;
    let backend = ctx.backend();
    gitquarry_core::storage::audit::record(
        backend.as_ref(),
        &ctx.cipher,
        "alice",
        "release.create",
        "acme/one@v0.4.3",
        None,
    )
    .await;
    gitquarry_core::storage::audit::record(
        backend.as_ref(),
        &ctx.cipher,
        "alice",
        "merge_request.merge",
        "acme/two#41",
        None,
    )
    .await;

    let entries = sidebar(ctx).await;
    for expected in [
        "acme/one 1 commit pushed to main",
        "acme/two 4 commits pushed to master",
        "acme/three 2 commits pushed",
        "acme/one tagged v0.4.3",
        "acme/two MR #41 merged",
    ] {
        assert!(
            entries.iter().any(|e| e == expected),
            "missing {expected:?} in {entries:?}"
        );
    }
}

#[tokio::test]
async fn never_mentions_a_repo_the_viewer_cannot_see() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Arc::new(test_ctx(dir.path()));
    pushed_repo(&ctx, "acme/open", Visibility::Public, 2, 1, Some("main")).await;
    pushed_repo(&ctx, "acme/secret", Visibility::Private, 1, 1, Some("main")).await;
    let backend = ctx.backend();
    gitquarry_core::storage::audit::record(
        backend.as_ref(),
        &ctx.cipher,
        "alice",
        "release.create",
        "acme/secret@v9",
        None,
    )
    .await;
    gitquarry_core::storage::audit::record(
        backend.as_ref(),
        &ctx.cipher,
        "alice",
        "merge_request.merge",
        "acme/secret#7",
        None,
    )
    .await;

    assert_eq!(sidebar(ctx).await, ["acme/open 1 commit pushed to main"]);
}
