use axum::http::HeaderMap;
use gitquarry_core::storage::merge_requests::{CommentAnchor, DiffSide, MrComment};
use gitquarry_core::storage::uid::Uid;
use gitquarry_i18n::Locale;
use gitquarry_webui::webui::repo::merges::annotated_diff::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::OnceLock;

/// One fixture uid, shared by every `view`/`view_as` call — its value is
/// arbitrary, only its identity as *the same* uid across a test matters.
fn fixture_repo_uid() -> &'static Uid {
    static UID: OnceLock<Uid> = OnceLock::new();
    UID.get_or_init(|| Uid::from("test-repo-uid".to_string()))
}

/// Written as a `concat!` rather than a `\`-continued literal: the leading
/// space on a context line is significant, and a line continuation eats it.
const DIFF: &str = concat!(
    "diff --git a/f.txt b/f.txt\n",
    "--- a/f.txt\n",
    "+++ b/f.txt\n",
    "@@ -3,3 +3,3 @@\n",
    " context\n",
    "-gone\n",
    "+added\n",
);

/// A `View` with the fixture's repo/number, varying only what each test
/// actually exercises.
fn view<'a>(
    bodies: &'a [String],
    can_comment: bool,
    active: Option<&'a CommentAnchor>,
    headers: &'a HeaderMap,
) -> View<'a> {
    view_as(bodies, can_comment, active, None, headers)
}

/// Comment bodies rendered the way the handler renders them, in the same
/// order as the comments they belong to -- `render` indexes into this by
/// position, so a test that got the order wrong would silently show the
/// wrong body under a line.
fn bodies(comments: &[(String, MrComment)]) -> Vec<String> {
    comments
        .iter()
        .map(|(_, comment)| gitquarry_webui::webui::markdown::render(&comment.body))
        .collect()
}

fn view_as<'a>(
    bodies: &'a [String],
    can_comment: bool,
    active: Option<&'a CommentAnchor>,
    viewer: Option<&'a str>,
    headers: &'a HeaderMap,
) -> View<'a> {
    View {
        comment_bodies: bodies,
        repo_name: "team/proj",
        repo_uid: fixture_repo_uid(),
        number: 1,
        can_comment,
        active,
        viewer,
        can_moderate: false,
        headers,
        locale: Locale::En,
    }
}

fn view_as_moderator<'a>(bodies: &'a [String], viewer: Option<&'a str>, headers: &'a HeaderMap) -> View<'a> {
    View {
        can_moderate: true,
        ..view_as(bodies, false, None, viewer, headers)
    }
}

fn anchored(id: &str, path: &str, side: DiffSide, line: u32, body: &str) -> (String, MrComment) {
    (
        id.to_string(),
        MrComment {
            author: "bob".to_string(),
            body: body.to_string(),
            anchor: Some(CommentAnchor {
                path: path.to_string(),
                side,
                line,
            }),
            ..Default::default()
        },
    )
}

#[test]
fn header_path_strips_the_git_prefix_and_ignores_dev_null() {
    assert_eq!(header_path("+++ b/src/main.rs"), Some("src/main.rs"));
    assert_eq!(header_path("--- a/src/main.rs"), Some("src/main.rs"));
    assert_eq!(header_path("+++ b/src/main.rs\t2024-01-01"), Some("src/main.rs"));
    assert_eq!(header_path("+++ /dev/null"), None);
    assert_eq!(header_path("@@ -1 +1 @@"), None);
}

#[test]
fn hunk_starts_reads_both_sides_and_rejects_a_malformed_header() {
    assert_eq!(hunk_starts("@@ -12,7 +40,8 @@ fn main()"), Some((12, 40)));
    // A one-line hunk omits the count.
    assert_eq!(hunk_starts("@@ -1 +1 @@"), Some((1, 1)));
    assert_eq!(hunk_starts("@@ nonsense @@"), None);
    assert_eq!(hunk_starts("+not a hunk"), None);
}

#[test]
fn each_side_gets_its_own_line_numbering() {
    let rows = parse(DIFF);
    let lines: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            Row::Line { old, new, text, .. } => Some((*old, *new, *text)),
            _ => None,
        })
        .collect();
    assert_eq!(
        lines,
        vec![
            (Some(3), Some(3), "context"),
            // The removal only advances the old side, the addition only the new.
            (Some(4), None, "gone"),
            (None, Some(4), "added"),
        ]
    );
}

#[test]
fn a_context_line_anchors_to_the_new_side() {
    let rows = parse(DIFF);
    let anchors: Vec<_> = rows
        .iter()
        .filter_map(|row| match row {
            Row::Line { anchor, .. } => Some((anchor.path.as_str(), anchor.side, anchor.line)),
            _ => None,
        })
        .collect();
    assert_eq!(
        anchors,
        vec![
            ("f.txt", DiffSide::New, 3),
            ("f.txt", DiffSide::Old, 4),
            ("f.txt", DiffSide::New, 4),
        ]
    );
}

#[test]
fn a_comment_lands_under_the_line_it_anchors_to() {
    let comments = vec![anchored("c1", "f.txt", DiffSide::Old, 4, "why remove this?")];
    let bodies = bodies(&comments);
    let out = render(DIFF, &comments, &view(&bodies, false, None, &HeaderMap::new()));
    assert_eq!(out.placed, HashSet::from([0]));

    let html = out.markup.into_string();
    let comment_at = html.find("why remove this?").unwrap();
    let removed_at = html.find(">gone<").unwrap();
    let added_at = html.find(">added<").unwrap();
    assert!(removed_at < comment_at && comment_at < added_at, "{html}");
}

#[test]
fn an_anchor_that_no_longer_matches_a_line_is_left_unplaced() {
    let comments = vec![
        anchored("c1", "f.txt", DiffSide::New, 999, "stale"),
        anchored("c2", "other.txt", DiffSide::New, 3, "wrong file"),
    ];
    let bodies = bodies(&comments);
    let out = render(DIFF, &comments, &view(&bodies, false, None, &HeaderMap::new()));
    assert!(out.placed.is_empty());
    let html = out.markup.into_string();
    assert!(!html.contains("stale"));
    assert!(!html.contains("wrong file"));
}

#[test]
fn each_hunk_gets_a_matching_toggle_and_row_group() {
    let out = render(DIFF, &[], &view(&[], false, None, &HeaderMap::new()));
    let html = out.markup.into_string();
    assert!(html.contains(r#"data-toggle-rows="hunk-0""#), "{html}");
    assert!(html.contains(r#"aria-expanded="true""#), "{html}");
    assert!(html.contains(r#"data-rows-group="hunk-0""#), "{html}");
}

#[test]
fn the_per_line_link_only_shows_for_a_viewer_who_may_comment() {
    let out = render(DIFF, &[], &view(&[], false, None, &HeaderMap::new()));
    assert!(!out.markup.into_string().contains("anchor_line"));

    let out = render(DIFF, &[], &view(&[], true, None, &HeaderMap::new()));
    let html = out.markup.into_string();
    assert!(html.contains("anchor_path=f.txt"), "{html}");
    assert!(html.contains("anchor_side=old&amp;anchor_line=4"), "{html}");
}

#[test]
fn the_form_expands_only_on_the_active_anchor() {
    let active = CommentAnchor {
        path: "f.txt".to_string(),
        side: DiffSide::New,
        line: 4,
    };
    let out = render(DIFF, &[], &view(&[], true, Some(&active), &HeaderMap::new()));
    let html = out.markup.into_string();
    assert_eq!(html.matches(r#"action="/_merges/comment""#).count(), 1);
    // The form's hidden fields carry the anchor back to the handler.
    assert!(html.contains(r#"name="anchor_side" value="new""#), "{html}");
    assert!(html.contains(r#"name="anchor_line" value="4""#), "{html}");
}

#[test]
fn a_placed_comments_edit_delete_controls_only_show_for_its_own_author() {
    let comments = vec![anchored("c1", "f.txt", DiffSide::Old, 4, "why remove this?")];

    let as_someone_else = render(
        DIFF,
        &comments,
        &view_as(&bodies(&comments), false, None, Some("alice"), &HeaderMap::new()),
    )
    .markup
    .into_string();
    assert!(
        !as_someone_else.contains(r#"action="/_merges/comment/edit""#),
        "{as_someone_else}"
    );
    assert!(
        !as_someone_else.contains(r#"action="/_merges/comment/delete""#),
        "{as_someone_else}"
    );

    let as_the_author = render(
        DIFF,
        &comments,
        &view_as(&bodies(&comments), false, None, Some("bob"), &HeaderMap::new()),
    )
    .markup
    .into_string();
    assert!(
        as_the_author.contains(r#"action="/_merges/comment/edit""#),
        "{as_the_author}"
    );
    assert!(
        as_the_author.contains(r#"action="/_merges/comment/delete""#),
        "{as_the_author}"
    );
}

#[test]
fn a_moderator_can_delete_someone_elses_placed_comment_but_not_edit_it() {
    let comments = vec![anchored("c1", "f.txt", DiffSide::Old, 4, "why remove this?")];

    let as_moderator = render(
        DIFF,
        &comments,
        &view_as_moderator(&bodies(&comments), Some("alice"), &HeaderMap::new()),
    )
    .markup
    .into_string();
    assert!(
        !as_moderator.contains(r#"action="/_merges/comment/edit""#),
        "{as_moderator}"
    );
    assert!(
        as_moderator.contains(r#"action="/_merges/comment/delete""#),
        "{as_moderator}"
    );
}

#[test]
fn active_anchor_needs_a_path_and_a_positive_line() {
    let params = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>()
    };

    assert!(active_anchor(&params(&[])).is_none());
    assert!(active_anchor(&params(&[("anchor_path", "f.txt")])).is_none());
    assert!(active_anchor(&params(&[("anchor_path", ""), ("anchor_line", "3")])).is_none());
    assert!(active_anchor(&params(&[("anchor_path", "f.txt"), ("anchor_line", "0")])).is_none());
    assert!(active_anchor(&params(&[("anchor_path", "f.txt"), ("anchor_line", "x")])).is_none());

    let anchor = active_anchor(&params(&[
        ("anchor_path", "f.txt"),
        ("anchor_line", "3"),
        ("anchor_side", "old"),
    ]))
    .unwrap();
    assert_eq!(
        (anchor.path.as_str(), anchor.side, anchor.line),
        ("f.txt", DiffSide::Old, 3)
    );

    // An unrecognized side falls back to the new one, same as a missing one.
    let anchor = active_anchor(&params(&[
        ("anchor_path", "f.txt"),
        ("anchor_line", "3"),
        ("anchor_side", "sideways"),
    ]))
    .unwrap();
    assert_eq!(anchor.side, DiffSide::New);
}
