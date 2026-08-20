//! What a PR body — a description, a comment, a review, an inline note —
//! survives on its way to the page.
//!
//! Two things had to hold at once, and they pull against each other: the
//! body is markdown written by a person, and `#123` in it should reach a
//! pull request. Substituting the link into the *source text* satisfies the
//! second and breaks the first, because a `#123` inside a code fence is not
//! a mention. So the substitution happens on the parsed event stream, and
//! these pin the cases that distinguishes.

use gitquarry_webui::webui::markdown::{render, render_with_mentions};
use std::collections::HashMap;

fn mentions() -> HashMap<u64, String> {
    HashMap::from([(7, "/yerasuu/gitquarry/_pulls/7".to_string())])
}

#[test]
fn a_mention_in_prose_becomes_a_link() {
    let html = render_with_mentions("see #7 for the fix", &mentions());

    assert!(
        html.contains(r#"<a href="/yerasuu/gitquarry/_pulls/7">#7</a>"#),
        "{html}"
    );
}

/// The reason this moved out of the source text: a fence is code, and the
/// author wrote those characters on purpose.
#[test]
fn a_mention_inside_a_code_fence_stays_literal() {
    let html = render_with_mentions("```\ngit show #7\n```", &mentions());

    assert!(!html.contains("<a href"), "{html}");
    assert!(html.contains("#7"), "{html}");
}

/// Same for a backtick span, which the parser hands over as its own event
/// rather than as text.
#[test]
fn a_mention_inside_inline_code_stays_literal() {
    let html = render_with_mentions("run `deploy --pr #7` first", &mentions());

    assert!(!html.contains("<a href"), "{html}");
}

/// An `<a>` inside an `<a>` isn't valid HTML, and the author already said
/// where that text should point.
#[test]
fn a_mention_inside_an_existing_link_is_left_alone() {
    let html = render_with_mentions("[see #7](https://example.com/x)", &mentions());

    assert_eq!(html.matches("<a href").count(), 1, "{html}");
    assert!(html.contains("https://example.com/x"), "{html}");
}

/// A number nobody resolved is not a dead link.
#[test]
fn an_unresolved_number_stays_plain_text() {
    let html = render_with_mentions("see #999 and #7", &mentions());

    assert_eq!(html.matches("<a href").count(), 1, "{html}");
    assert!(html.contains("#999"), "{html}");
}

/// The regression that started this: a body written as a list used to reach
/// the page as one flat paragraph with its bullets showing as literal `-`.
#[test]
fn a_body_written_as_markdown_renders_as_markdown() {
    let html = render("Highlights:\n\n- first\n- second\n\n`inline code` and **bold**.");

    assert!(html.contains("<ul>"), "{html}");
    assert_eq!(html.matches("<li>").count(), 2, "{html}");
    assert!(html.contains("<code>inline code</code>"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
}

/// Moving the escaping into the renderer must not have moved it out of the
/// way: a body is still untrusted text from anyone who can comment.
#[test]
fn raw_html_in_a_body_is_still_stripped() {
    let html = render("hello <script>alert(1)</script> world");

    assert!(!html.contains("<script"), "{html}");
}
