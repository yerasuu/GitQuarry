use gitquarry_webui::webui::markdown::*;

#[test]
fn renders_basic_markdown() {
    let html = render("# Title\n\nSome **bold** text.");
    assert!(html.contains("<h1>Title</h1>"));
    assert!(html.contains("<strong>bold</strong>"));
}

#[test]
fn strips_raw_html() {
    let html = render("before <script>alert(1)</script> after");
    assert!(!html.contains("<script>"));
}

#[test]
fn neutralizes_javascript_link() {
    let html = render("[click me](javascript:alert(1))");
    assert!(!html.contains("javascript:"));
    assert!(html.contains("href=\"#\""));
}

#[test]
fn keeps_safe_links() {
    let html = render("[repo](https://example.com/x) and [rel](./docs)");
    assert!(html.contains("href=\"https://example.com/x\""));
    assert!(html.contains("href=\"./docs\""));
}

#[test]
fn highlights_a_fenced_code_block_with_a_language_tag() {
    let html = render("```rust\nfn main() {}\n```");
    assert!(html.contains("<pre class=\"blob\">"));
    // The fence's language token becomes the `language-*` class
    // `highlight.js` (client-side, vendored) looks for -- no server-side
    // tokenizing happens here, see `markdown.rs`'s own doc comment.
    assert!(html.contains("class=\"language-rust\""));
    assert!(html.contains("fn main() {}"));
}

#[test]
fn does_not_highlight_a_fence_with_no_language_tag() {
    let html = render("```\nfn main() {}\n```");
    assert!(!html.contains("<pre class=\"blob\">"));
    assert!(html.contains("fn main() {}"));
}

#[test]
fn highlighted_code_block_still_escapes_html_metacharacters() {
    let html = render("```rust\nlet x = a < b;\n```");
    assert!(!html.contains("a < b"));
    assert!(html.contains("&lt;"));
}

#[test]
fn renders_github_style_alert_blockquotes() {
    let html = render("> [!NOTE]\n> Something worth knowing.");
    assert!(html.contains("class=\"markdown-alert-note\""));
    // The marker line itself is parsed away, not left as literal text.
    assert!(!html.contains("[!NOTE]"));
    assert!(html.contains("Something worth knowing."));

    let html = render("> [!WARNING]\n> Be careful.");
    assert!(html.contains("class=\"markdown-alert-warning\""));
}

#[test]
fn plain_blockquote_is_unaffected() {
    let html = render("> just a quote, not an alert");
    assert!(html.contains("<blockquote>"));
    assert!(!html.contains("markdown-alert"));
}
