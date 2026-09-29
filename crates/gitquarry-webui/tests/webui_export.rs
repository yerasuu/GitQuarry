//! The headers `/_raw/*` answers with for real pushed files: which types
//! render inline, which are forced to a download, and that a filename can
//! never break out of its `Content-Disposition` quoted-string.

use axum::Router;
use axum::http::{StatusCode, header};
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Page, Session, Work, ctx};
use std::sync::Arc;

struct Seeded {
    _root: tempfile::TempDir,
    app: Router,
    sha: String,
}

/// Public `team/proj` with one commit holding one file of every kind the
/// raw view treats differently, pushed the way `git push` leaves a repo.
async fn seeded() -> Seeded {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let work = Work::new().await;
    let w = work.path();
    std::fs::create_dir_all(w.join("assets")).unwrap();
    std::fs::create_dir_all(w.join("docs")).unwrap();
    // Written by hand rather than through `Work::commit`, whose contents are
    // `&str`: the PNG signature isn't valid UTF-8.
    for (name, content) in [
        ("README.md", &b"# Hello\n"[..]),
        ("page.html", b"<script>alert(1)</script>\n"),
        ("logo.png", b"\x89PNG\r\n\x1a\n"),
        ("binary.bin", b"\x00\x01\x02"),
        (
            "assets/logo.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
        ),
        ("docs/spec.pdf", b"%PDF-1.4\n"),
        ("weird\"name.svg", b"<svg/>"),
        ("back\\slash.svg", b"<svg/>"),
    ] {
        std::fs::write(w.join(name), content).unwrap();
    }
    let sha = work.commit(&[], "every kind of file").await;
    work.push(&repo, "refs/heads/main").await;

    Seeded {
        _root: root,
        app: gitquarry_webui::webui::router(ctx),
        sha,
    }
}

async fn get(app: &Router, uri: &str) -> Page {
    Session::get_as(app, uri, None).await
}

/// `(Content-Type, Content-Disposition)` of the raw view of `path`, which
/// must already be percent-encoded for the query string.
async fn raw_headers(app: &Router, path: &str) -> (String, Option<String>) {
    let page = get(app, &format!("/_raw/plain/team/proj?path={path}")).await;
    assert_eq!(page.status, StatusCode::OK, "{path}: {}", page.body);
    let content_type = page.headers[header::CONTENT_TYPE].to_str().unwrap().to_string();
    let disposition = page
        .headers
        .get(header::CONTENT_DISPOSITION)
        .map(|v| v.to_str().unwrap().to_string());
    (content_type, disposition)
}

#[tokio::test]
async fn common_extensions_get_their_content_type_and_render_inline() {
    let s = seeded().await;

    assert_eq!(raw_headers(&s.app, "logo.png").await, ("image/png".to_string(), None));
    assert_eq!(
        raw_headers(&s.app, "README.md").await,
        ("text/plain; charset=utf-8".to_string(), None)
    );
    assert_eq!(
        raw_headers(&s.app, "binary.bin").await,
        ("application/octet-stream".to_string(), None)
    );
    // Served as text, never as `text/html`: the raw view is same-origin, so
    // a pushed page would otherwise run its script against this app.
    assert_eq!(
        raw_headers(&s.app, "page.html").await,
        ("text/plain; charset=utf-8".to_string(), None)
    );
}

#[tokio::test]
async fn blob_view_embeds_images_and_prints_code() {
    let s = seeded().await;

    let svg = get(&s.app, "/team/proj?view=blob&path=assets/logo.svg").await;
    assert_eq!(svg.status, StatusCode::OK, "{}", svg.body);
    let svg = svg.body;
    assert!(svg.contains(r#"<img src="/_raw/plain/"#), "{svg}");
    assert!(!svg.contains(r#"<pre class="blob">"#), "{svg}");

    let readme = get(&s.app, "/team/proj?view=blob&path=README.md").await;
    assert_eq!(readme.status, StatusCode::OK, "{}", readme.body);
    let readme = readme.body;
    assert!(!readme.contains(r#"<img src="/_raw/plain/"#), "{readme}");
    assert!(readme.contains(r#"<pre class="blob">"#), "{readme}");
}

#[tokio::test]
async fn svg_is_forced_to_a_download_to_block_inline_script_execution() {
    let s = seeded().await;

    assert_eq!(
        raw_headers(&s.app, "assets/logo.svg").await,
        (
            "image/svg+xml".to_string(),
            Some("attachment; filename=\"logo.svg\"".to_string())
        )
    );
}

/// PDF keeps `application/pdf` (an `<embed>` needs it) but must not render
/// inline from the raw URL: a PDF can carry JavaScript, and the viewer that
/// runs it sits largely outside what this app's CSP can reach.
#[tokio::test]
async fn raw_pdf_is_handed_over_as_a_download() {
    let s = seeded().await;

    assert_eq!(
        raw_headers(&s.app, "docs/spec.pdf").await,
        (
            "application/pdf".to_string(),
            Some("attachment; filename=\"spec.pdf\"".to_string())
        )
    );
}

/// A quote in a tree path used to close the quoted-string early and let the
/// rest of the name read as header parameters.
#[tokio::test]
async fn disposition_filename_stays_inside_its_quoted_string() {
    let s = seeded().await;

    let (_, disposition) = raw_headers(&s.app, "weird%22name.svg").await;
    assert_eq!(disposition.as_deref(), Some("attachment; filename=\"weird'name.svg\""));
    let (_, disposition) = raw_headers(&s.app, "back%5Cslash.svg").await;
    assert_eq!(disposition.as_deref(), Some("attachment; filename=\"back'slash.svg\""));
}

#[tokio::test]
async fn a_control_character_in_the_path_never_reaches_a_header() {
    let s = seeded().await;

    let page = get(&s.app, "/_raw/plain/team/proj?path=bad%0D%0Aname.svg").await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST);
    assert!(
        page.headers.get(header::CONTENT_DISPOSITION).is_none(),
        "{:?}",
        page.headers
    );
}

#[tokio::test]
async fn patch_and_snapshot_downloads_name_their_file() {
    let s = seeded().await;

    let patch = get(&s.app, &format!("/_raw/patch/team/proj?sha={}", s.sha)).await;
    assert_eq!(patch.status, StatusCode::OK);
    let disposition = patch.headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.starts_with("attachment; filename=\""), "{disposition}");
    assert!(disposition.contains(&s.sha[..7]), "{disposition}");
    assert!(disposition.ends_with(".patch\""), "{disposition}");

    let snapshot = get(&s.app, "/_raw/snapshot/team/proj?fmt=zip").await;
    assert_eq!(snapshot.status, StatusCode::OK);
    assert_eq!(
        snapshot.headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"proj-main.zip\""
    );
}
