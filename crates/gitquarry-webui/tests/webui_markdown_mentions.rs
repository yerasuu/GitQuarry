//! What a PR description survives on its way to the page.
//!
//! `link_number_mentions` now hands markdown to `markdown::render` instead
//! of pre-escaped HTML, which only works if the renderer keeps the two
//! things that pipeline depends on: the relative link it synthesizes for a
//! `#123` mention, and ordinary block markdown.

use gitquarry_webui::webui::markdown::render;

/// The exact shape `link_number_mentions` emits. The pointy brackets are
/// the part worth pinning: without them a `)` anywhere in a repo path would
/// end the destination early, and the renderer is also free to reject link
/// targets it doesn't like — a relative one has to be allowed.
#[test]
fn a_mention_link_survives_the_renderer() {
    let html = render("see [#7](</yerasuu/gitquarry/_pulls/7>) for the fix");

    assert!(html.contains(r#"href="/yerasuu/gitquarry/_pulls/7""#), "{html}");
    assert!(html.contains("#7"), "{html}");
}

/// The regression itself: a description written as a list used to reach the
/// page as one flat paragraph with its bullets showing as literal `-`.
#[test]
fn a_description_written_as_markdown_renders_as_markdown() {
    let html = render("Highlights:\n\n- first\n- second\n\n`inline code` and **bold**.");

    assert!(html.contains("<ul>"), "{html}");
    assert_eq!(html.matches("<li>").count(), 2, "{html}");
    assert!(html.contains("<code>inline code</code>"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
}

/// The other half of moving escaping into the renderer: a body is still
/// untrusted, so raw HTML in one must not reach the page.
#[test]
fn raw_html_in_a_body_is_still_stripped() {
    let html = render("hello <script>alert(1)</script> world");

    assert!(!html.contains("<script"), "{html}");
}
