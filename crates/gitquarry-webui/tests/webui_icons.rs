//! The inline-SVG icons as the pages actually serve them.

use axum::http::StatusCode;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Session, Work, ctx, git, user};
use std::collections::BTreeSet;
use std::sync::Arc;

/// How many icon functions `webui::icons` defines.
const ICON_COUNT: usize = 13;

/// Every page an icon shows up on, seen by a logged-in viewer: a limited
/// (lock badge), archived (archive badge) repo with a README (book),
/// CONTRIBUTING (file), LICENSE (license), a directory (folder), two
/// branches (branch switcher) and a tag.
async fn pages() -> Vec<String> {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Limited)
        .await
        .unwrap()
        .unwrap();
    let (mut config, etag) =
        gitquarry_core::storage::settings::get_repo_config(ctx.backend().as_ref(), repo.uid.as_str(), &ctx.cipher)
            .await
            .unwrap();
    config.archived = true;
    gitquarry_core::storage::settings::put_repo_config(
        ctx.backend().as_ref(),
        repo.uid.as_str(),
        &config,
        etag.as_deref(),
        &ctx.cipher,
    )
    .await
    .unwrap();

    let work = Work::new().await;
    work.commit(
        &[
            ("src/main.rs", "fn main() {}\n"),
            ("README.md", "# Hello\n"),
            ("CONTRIBUTING.md", "Send patches.\n"),
            ("LICENSE", "MIT License\n"),
        ],
        "initial",
    )
    .await;
    git(work.path(), &["tag", "-a", "v1.0", "-m", "first"]).await;
    git(work.path(), &["branch", "feature"]).await;
    work.push(&repo, "refs/heads/main").await;

    user(&ctx, "alice", false).await;
    let app = gitquarry_webui::webui::router(ctx);
    let session = Session::login(&app, "alice").await;

    let mut out = Vec::new();
    for uri in ["/team/proj", "/team/proj?view=tree", "/team/proj?view=refs"] {
        let page = session.get(&app, uri).await;
        assert_eq!(page.status, StatusCode::OK, "GET {uri}: {}", page.body);
        out.push(page.body);
    }
    out
}

/// Every distinct `class="icon"` SVG across `pages`, whole.
fn icons(pages: &[String]) -> BTreeSet<String> {
    pages
        .iter()
        .flat_map(|page| page.split("<svg ").skip(1))
        .map(|rest| format!("<svg {}", &rest[..rest.find("</svg>").unwrap() + "</svg>".len()]))
        .filter(|svg| svg.contains(r#"class="icon""#))
        .collect()
}

/// The property tests below only see what these pages serve, so this pins
/// that they reach the whole set rather than passing on a subset.
#[tokio::test]
async fn every_icon_is_served_somewhere() {
    let icons = icons(&pages().await);
    assert_eq!(icons.len(), ICON_COUNT, "{icons:#?}");
}

/// Every icon must stay decorative (never its own accessible name) and
/// sized/colored by the `.icon` CSS class rather than a hardcoded fill.
#[tokio::test]
async fn every_icon_is_decorative_and_uses_the_icon_class() {
    for svg in icons(&pages().await) {
        assert!(svg.contains(r#"aria-hidden="true""#), "{svg}");
        assert!(svg.contains("currentColor"), "{svg}");
    }
}

/// Every shape element must be explicitly closed (`</circle>`, `</path>`,
/// ...) — a bare `;` in maud leaves non-void SVG elements unclosed, which
/// nests every following sibling inside the previous shape and makes
/// browsers paint only the outermost one.
#[tokio::test]
async fn every_icon_closes_each_shape_element() {
    for svg in icons(&pages().await) {
        for shape in ["path", "circle", "rect", "line", "polygon"] {
            assert_eq!(
                svg.matches(&format!("<{shape} ")).count(),
                svg.matches(&format!("</{shape}>")).count(),
                "{shape} not individually closed in {svg}"
            );
        }
    }
}
