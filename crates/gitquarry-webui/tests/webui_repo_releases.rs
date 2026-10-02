//! The `releases` tab and a release's own page: who sees drafts and the
//! write forms, what the tag field suggests, and how assets are labelled.

use axum::Router;
use axum::http::{StatusCode, header};
use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_core::auth::domain::user;
use gitquarry_core::auth::entities::Permission;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::{AssetMeta, ReleaseMeta};
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Page, Session, Work, ctx, git, post_form};
use std::sync::Arc;

async fn get(app: &Router, uri: &str, cookie: &str) -> Page {
    Session::get_as(app, uri, Some(cookie)).await
}

struct Repo {
    _dirs: (tempfile::TempDir, Work),
    ctx: Arc<AppContext>,
    resolved: ResolvedRepo,
    app: Router,
}

/// Public `team/proj` with one commit on `main`, tagged with each of `tags`.
/// Nothing is requested yet, so releases put on `resolved` afterwards are
/// what the first page load sees.
async fn repo(tags: &[&str]) -> Repo {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let resolved = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let work = Work::new().await;
    work.commit(&[("f.txt", "hi\n")], "init").await;
    for tag in tags {
        git(work.path(), &["tag", tag]).await;
    }
    work.push(&resolved, "refs/heads/main").await;
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    Repo {
        _dirs: (root, work),
        ctx,
        resolved,
        app,
    }
}

/// `signature`: `None` unsigned, `Some(verified)` signed.
fn asset(filename: &str, sha_char: &str, signature: Option<bool>) -> AssetMeta {
    AssetMeta {
        filename: filename.to_string(),
        size: 100,
        sha256: sha_char.repeat(64),
        uploaded_at: "2024-01-01T00:00:00Z".to_string(),
        uploaded_by: "alice".to_string(),
        signature: signature.map(|_| "-----BEGIN PGP SIGNATURE-----...".to_string()),
        signature_verified: signature.unwrap_or(false),
    }
}

async fn put_release(r: &Repo, tag: &str, title: &str, is_draft: bool, assets: Vec<AssetMeta>) {
    let release = ReleaseMeta {
        tag: tag.to_string(),
        title: title.to_string(),
        notes: "**bold** notes".to_string(),
        assets,
        created_at: "2024-01-01T00:00:00Z".to_string(),
        created_by: "alice".to_string(),
        is_prerelease: false,
        is_draft,
    };
    r.resolved.remote.put_release(&release, None).await.unwrap();
}

/// A logged-in `username` holding `perm` on `team/proj`.
async fn session_with(r: &Repo, username: &str, perm: Permission) -> String {
    gitquarry_test_support::user(&r.ctx, username, false).await;
    user::set_repo_perm(&r.ctx, username, "team/proj", Some(perm), Locale::En)
        .await
        .unwrap();
    Session::login(&r.app, username).await.cookie
}

#[tokio::test]
async fn releases_list_shows_hint_when_empty() {
    let r = repo(&[]).await;
    let Page { status, body: html, .. } = get(&r.app, "/team/proj?view=releases", "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("no releases yet"), "{html}");
}

#[tokio::test]
async fn releases_list_shows_each_release_and_no_form_to_an_anonymous_viewer() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "First release", false, Vec::new()).await;
    let Page { body: html, .. } = get(&r.app, "/team/proj?view=releases", "").await;
    assert!(html.contains("First release"), "{html}");
    assert!(!html.contains(r#"action="/_releases/create/"#), "{html}");
    assert!(!html.contains("generate notes from commits"), "{html}");
}

#[tokio::test]
async fn drafts_stay_hidden_from_an_anonymous_viewer() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "Unpublished", true, Vec::new()).await;

    let Page { body: html, .. } = get(&r.app, "/team/proj?view=releases", "").await;
    assert!(!html.contains("Unpublished"), "{html}");
    assert!(html.contains("no releases yet"), "{html}");

    // Same answer as a tag with no release at all: it doesn't even leak that
    // one is pending.
    let Page { status, body: html, .. } = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains("Unpublished"), "{html}");
}

#[tokio::test]
async fn a_reader_without_write_sees_no_draft_and_no_form() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "Unpublished", true, Vec::new()).await;
    let bob = session_with(&r, "bob", Permission::Reporter).await;

    let Page { body: html, .. } = get(&r.app, "/team/proj?view=releases", &bob).await;
    assert!(!html.contains("Unpublished"), "{html}");
    assert!(!html.contains(r#"action="/_releases/create/"#), "{html}");
    let Page { status, .. } = get(&r.app, "/team/proj?view=release&tag=v1.0", &bob).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_writer_sees_drafts_and_the_write_forms() {
    let r = repo(&["v1.0"]).await;
    put_release(&r, "v1.0", "Unpublished", true, Vec::new()).await;
    let alice = session_with(&r, "alice", Permission::Developer).await;

    let Page { body: html, .. } = get(&r.app, "/team/proj?view=releases", &alice).await;
    assert!(html.contains("Unpublished"), "{html}");
    assert!(html.contains(r#"action="/_releases/create/team/proj""#), "{html}");
    assert!(html.contains("generate notes from commits"), "{html}");
    let Page { status, body: html, .. } = get(&r.app, "/team/proj?view=release&tag=v1.0", &alice).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(r#"action="/_releases/edit/team/proj""#), "{html}");
}

/// The generated-notes round trip re-renders the create form with whatever
/// was typed filled back in, so it must never render for someone who
/// couldn't have submitted that form.
#[tokio::test]
async fn generate_notes_never_echoes_a_prefill_to_someone_without_write() {
    let r = repo(&["v1.0"]).await;
    let leaked = [
        ("tag", "v1.0"),
        ("title", "leaked title"),
        ("notes", "leaked notes text"),
    ];

    let Page {
        status,
        headers,
        body: html,
    } = post_form(&r.app, "/_releases/generate_notes/team/proj", None, &leaked).await;
    assert!(status.is_redirection(), "{status}");
    assert!(
        headers[header::LOCATION].to_str().unwrap().starts_with("/_login"),
        "{headers:?}"
    );
    assert!(!html.contains("leaked"), "{html}");

    let bob = session_with(&r, "bob", Permission::Reporter).await;
    let csrf = get(&r.app, "/team/proj?view=releases", &bob).await.csrf();
    let mut fields = leaked.to_vec();
    fields.push(("csrf", &csrf));
    let Page { status, body: html, .. } =
        post_form(&r.app, "/_releases/generate_notes/team/proj", Some(&bob), &fields).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!html.contains("leaked"), "{html}");
}

#[tokio::test]
async fn the_tag_field_suggests_only_tags_without_a_release_yet() {
    let r = repo(&["v0.2.0", "v0.1.0", "v0.3.0"]).await;
    put_release(&r, "v0.1.0", "Old", false, Vec::new()).await;
    let alice = session_with(&r, "alice", Permission::Developer).await;

    let Page { body: html, .. } = get(&r.app, "/team/proj?view=releases", &alice).await;
    let start = html.find(r#"<datalist id="release-tags">"#).unwrap();
    let datalist = &html[start..start + html[start..].find("</datalist>").unwrap()];
    // Branches aren't candidates, a tag that already has a release isn't
    // either (it would only earn the "already has a release" rejection),
    // and what's left comes back sorted rather than in manifest order.
    let suggested: Vec<&str> = datalist
        .split(r#"<option value=""#)
        .skip(1)
        .map(|rest| &rest[..rest.find('"').unwrap()])
        .collect();
    assert_eq!(suggested, ["v0.2.0", "v0.3.0"]);
}

#[tokio::test]
async fn the_tag_field_suggests_nothing_when_every_tag_is_released() {
    let r = repo(&["v1.0.0"]).await;
    put_release(&r, "v1.0.0", "Only", false, Vec::new()).await;
    let alice = session_with(&r, "alice", Permission::Developer).await;
    let Page { body: html, .. } = get(&r.app, "/team/proj?view=releases", &alice).await;
    assert!(html.contains(r#"<datalist id="release-tags"></datalist>"#), "{html}");
}

#[tokio::test]
async fn release_detail_404s_without_a_release() {
    let r = repo(&["v1.0"]).await;
    let Page { status, .. } = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn release_detail_labels_the_generated_source_assets_but_not_a_regular_one() {
    let r = repo(&["v1.0"]).await;
    // Release creation generates and uploads these two in the background;
    // pre-seeded here as ordinary assets to exercise the label matching.
    let tar_gz = gitquarry_core::git::browse::archive_filename("team/proj", "v1.0", "tar.gz");
    let zip = gitquarry_core::git::browse::archive_filename("team/proj", "v1.0", "zip");
    put_release(
        &r,
        "v1.0",
        "First release",
        false,
        vec![
            asset(&tar_gz, "a", None),
            asset(&zip, "b", None),
            asset("notes.pdf", "c", None),
        ],
    )
    .await;

    let Page { status, body: html, .. } = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(!html.contains("no longer exists on this repo"), "{html}");
    assert!(html.contains("Source code (tar.gz)"), "{html}");
    assert!(html.contains("Source code (zip)"), "{html}");
    // A regular asset keeps its own filename, not a generic label.
    assert!(html.contains("notes.pdf"), "{html}");
    // Real size/sha256 cells, like any other asset — not blank.
    assert!(html.contains("<td>100</td>"), "{html}");
    assert!(html.contains(&"a".repeat(6)), "{html}");
    // Downloads from stored bytes (the real asset route), not the on-demand
    // snapshot route.
    assert!(html.contains("/_raw/release_asset/"), "{html}");
    assert!(!html.contains("/_raw/snapshot/"), "{html}");
}

#[tokio::test]
async fn release_detail_badges_a_signed_asset_by_whether_it_verified() {
    let r = repo(&["v1.0"]).await;
    put_release(
        &r,
        "v1.0",
        "First release",
        false,
        vec![
            asset("asset-good.bin", "a", Some(true)),
            asset("asset-bad.bin", "b", Some(false)),
            asset("asset-plain.bin", "c", None),
        ],
    )
    .await;

    let Page { body: html, .. } = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    let row = |name: &str| html.split("<tr>").find(|r| r.contains(name)).unwrap().to_string();
    assert!(row("asset-good.bin").contains("signature verified"), "{html}");
    assert!(row("asset-bad.bin").contains("signature not verified"), "{html}");
    assert!(!row("asset-plain.bin").contains("signature"), "{html}");
}

#[tokio::test]
async fn release_detail_notes_a_dangling_tag_without_hiding_the_release() {
    // No `v1.0` tag pushed at all — the "dangling" case.
    let r = repo(&[]).await;
    put_release(&r, "v1.0", "First release", false, vec![asset("a.bin", "a", None)]).await;

    let Page { status, body: html, .. } = get(&r.app, "/team/proj?view=release&tag=v1.0", "").await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("no longer exists on this repo"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
    assert!(html.contains("a.bin"), "{html}");
    // Anonymous viewer: no edit/delete/upload forms.
    assert!(!html.contains(r#"action="/_releases/edit/"#), "{html}");
    assert!(!html.contains(r#"action="/_releases/asset_upload/"#), "{html}");
    assert!(!html.contains(r#"action="/_releases/delete/"#), "{html}");
}
