//! A README as the about tab renders it. A README is untrusted content —
//! anyone with write access controls what every other viewer's browser
//! gets — so the sanitizing properties are pinned on the served page.

use axum::http::StatusCode;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Session, Work, ctx};
use std::sync::Arc;

/// Pushes a public `team/proj` whose only file is a `README.md` holding
/// `markdown`, and returns the rendered README out of its about tab — just
/// that element, since the page around it has its own scripts and links.
async fn readme(markdown: &str) -> String {
    let root = tempfile::tempdir().unwrap();
    let ctx = Arc::new(ctx(root.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();

    let work = Work::new().await;
    work.commit(&[("README.md", markdown)], "readme").await;
    work.push(&repo, "refs/heads/main").await;

    let app = gitquarry_webui::webui::router(ctx);
    let page = Session::get_as(&app, "/team/proj", None).await;
    let (status, page) = (page.status, page.body);
    assert_eq!(status, StatusCode::OK, "{page}");

    let start = page
        .find(r#"<div id="readme" class="readme">"#)
        .unwrap_or_else(|| panic!("no rendered readme: {page}"));
    let len = page[start..].find("</div>").unwrap();
    page[start..start + len].to_string()
}

#[tokio::test]
async fn renders_basic_markdown() {
    let html = readme("# Title\n\nSome **bold** text.").await;
    assert!(html.contains("<h1>Title</h1>"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
}

#[tokio::test]
async fn strips_raw_html() {
    let html = readme("before <script>alert(1)</script> after\n\n<img src=x onerror=alert(1)>\n").await;
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(html.contains("before"), "{html}");
}

#[tokio::test]
async fn neutralizes_javascript_link() {
    let html = readme("[click me](javascript:alert(1))").await;
    assert!(!html.contains("javascript:"), "{html}");
    assert!(html.contains("href=\"#\""), "{html}");
}

#[tokio::test]
async fn keeps_safe_links() {
    let html = readme("[repo](https://example.com/x) and [rel](./docs)").await;
    assert!(html.contains("href=\"https://example.com/x\""), "{html}");
    assert!(html.contains("href=\"./docs\""), "{html}");
}

#[tokio::test]
async fn highlights_a_fenced_code_block_with_a_language_tag() {
    let html = readme("```rust\nfn main() {}\n```").await;
    assert!(html.contains("<pre class=\"blob\">"), "{html}");
    // The fence's language token becomes the `language-*` class
    // `highlight.js` (client-side, vendored) looks for -- no server-side
    // tokenizing happens, see `markdown.rs`'s own doc comment.
    assert!(html.contains("class=\"language-rust\""), "{html}");
    assert!(html.contains("fn main() {}"), "{html}");
}

#[tokio::test]
async fn does_not_highlight_a_fence_with_no_language_tag() {
    let html = readme("```\nfn main() {}\n```").await;
    assert!(!html.contains("<pre class=\"blob\">"), "{html}");
    assert!(html.contains("fn main() {}"), "{html}");
}

#[tokio::test]
async fn highlighted_code_block_still_escapes_html_metacharacters() {
    let html = readme("```rust\nlet x = a < b;\n```").await;
    assert!(!html.contains("a < b"), "{html}");
    assert!(html.contains("a &lt; b"), "{html}");
}

#[tokio::test]
async fn renders_github_style_alert_blockquotes() {
    let html = readme("> [!NOTE]\n> Something worth knowing.\n\n> [!WARNING]\n> Be careful.").await;
    assert!(html.contains("class=\"markdown-alert-note\""), "{html}");
    assert!(html.contains("class=\"markdown-alert-warning\""), "{html}");
    // The marker line itself is parsed away, not left as literal text.
    assert!(!html.contains("[!NOTE]"), "{html}");
    assert!(html.contains("Something worth knowing."), "{html}");
}

#[tokio::test]
async fn plain_blockquote_is_unaffected() {
    let html = readme("> just a quote, not an alert").await;
    assert!(html.contains("<blockquote>"), "{html}");
    assert!(!html.contains("markdown-alert"), "{html}");
}
