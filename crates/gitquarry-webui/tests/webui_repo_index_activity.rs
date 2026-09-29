//! The home page's "recent activity" sidebar: pushes (from each repo's own
//! `last_push*` fields), releases and merged MRs (from the audit log),
//! newest first, capped, and never about a repo the viewer can't see.

use gitquarry_core::app::AppContext;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Session, ctx};
use std::sync::Arc;

const HOUR: u64 = 3600;

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
    let html = Session::get_as(&app, "/", None).await.body;
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
    let ctx = Arc::new(ctx(dir.path()));
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
    let ctx = Arc::new(ctx(dir.path()));
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
    let ctx = Arc::new(ctx(dir.path()));
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
