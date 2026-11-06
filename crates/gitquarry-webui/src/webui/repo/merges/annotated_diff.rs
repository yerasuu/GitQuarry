//! The MR view's unified diff, rendered with line numbers, its
//! line-anchored comments woven in under the lines they mark, and a
//! per-line link that reopens the page with the "comment on this line"
//! form expanded there.
//!
//! Only the unified view is annotated. `super::components::side_by_side_diff`
//! pairs removals against additions positionally, so a row there doesn't
//! correspond to one anchorable line — the split view keeps rendering as it
//! did, and its anchored comments fall back to the flat thread.

use crate::webui::{csrf_field, repo_link, url_encode};
use axum::http::HeaderMap;
use gitquarry_core::storage::merge_requests::{CommentAnchor, DiffSide, MrComment};
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::merges as t;
use maud::{Markup, PreEscaped, html};
use std::collections::HashSet;

enum Row<'a> {
    /// `diff --git`, `index`, mode, `---`/`+++` headers — nothing to anchor.
    Meta(&'a str),
    Hunk(&'a str),
    /// A content line, with whichever side's line numbers it occupies.
    Line {
        old: Option<u32>,
        new: Option<u32>,
        class: &'static str,
        text: &'a str,
        anchor: CommentAnchor,
    },
}

/// Everything [`render`] produced: the markup, plus which of the caller's
/// comments it actually placed. Anything not placed (an anchor whose line
/// no longer appears in the current diff, or a plain thread comment) is the
/// caller's to render in the flat thread instead.
pub(super) struct Annotated {
    pub(super) markup: Markup,
    pub(super) placed: HashSet<usize>,
}

/// The path a `+++ b/<path>` / `--- a/<path>` header names, or `None` for
/// `/dev/null` (a pure add or delete has one of the two). Delegates to
/// `super::super::diff::header_path` — the same parsing `color_diff`/
/// `side_by_side_diff` need, kept in one place.
fn header_path(line: &str) -> Option<&str> {
    super::super::diff::header_path(line)
}

/// The `-old_start` / `+new_start` of an `@@ -1,2 +3,4 @@` header. `None`
/// if the header is malformed — the caller then leaves the counters alone
/// rather than numbering the following lines wrongly.
fn hunk_starts(line: &str) -> Option<(u32, u32)> {
    let body = line.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = body.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |spec: &str| spec.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

fn parse(diff: &str) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let mut path = "";
    let (mut old_line, mut new_line) = (0u32, 0u32);

    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            if let Some(found) = header_path(line) {
                path = found;
            }
            rows.push(Row::Meta(line));
        } else if line.starts_with("@@") {
            if let Some((old, new)) = hunk_starts(line) {
                old_line = old;
                new_line = new;
            }
            rows.push(Row::Hunk(line));
        } else if let Some(text) = line.strip_prefix('+') {
            rows.push(Row::Line {
                old: None,
                new: Some(new_line),
                class: "diff-add",
                text,
                anchor: CommentAnchor {
                    path: path.to_string(),
                    side: DiffSide::New,
                    line: new_line,
                },
            });
            new_line += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            rows.push(Row::Line {
                old: Some(old_line),
                new: None,
                class: "diff-del",
                text,
                anchor: CommentAnchor {
                    path: path.to_string(),
                    side: DiffSide::Old,
                    line: old_line,
                },
            });
            old_line += 1;
        } else if let Some(text) = line.strip_prefix(' ') {
            rows.push(Row::Line {
                old: Some(old_line),
                new: Some(new_line),
                class: "",
                // A context line exists on both sides, so it anchors to the
                // new one — that's the side that survives the merge.
                text,
                anchor: CommentAnchor {
                    path: path.to_string(),
                    side: DiffSide::New,
                    line: new_line,
                },
            });
            old_line += 1;
            new_line += 1;
        } else {
            rows.push(Row::Meta(line));
        }
    }
    rows
}

fn side_param(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Old => "old",
        DiffSide::New => "new",
    }
}

/// Parses the `anchor_path`/`anchor_side`/`anchor_line` query params the
/// per-line link sets. `None` unless all three are present and usable —
/// a half-filled set just renders the diff with no form expanded.
pub(super) fn active_anchor(params: &std::collections::HashMap<String, String>) -> Option<CommentAnchor> {
    let path = params.get("anchor_path")?;
    let line = params.get("anchor_line")?.parse::<u32>().ok()?;
    if path.is_empty() || line == 0 {
        return None;
    }
    let side = match params.get("anchor_side").map(String::as_str) {
        Some("old") => DiffSide::Old,
        _ => DiffSide::New,
    };
    Some(CommentAnchor {
        path: path.clone(),
        side,
        line,
    })
}

/// Link that reopens this MR with the comment form expanded on `anchor`.
fn comment_link(repo_name: &str, number: u64, anchor: &CommentAnchor) -> String {
    format!(
        "{}&anchor_path={}&anchor_side={}&anchor_line={}",
        repo_link(repo_name, "merge", "").number(number),
        url_encode(&anchor.path),
        side_param(anchor.side),
        anchor.line,
    )
}

/// `body_html` is the comment's body already rendered -- see
/// `View::comment_bodies`.
#[allow(clippy::too_many_arguments)]
fn comment_row(
    comment_id: &str,
    comment: &MrComment,
    body_html: &str,
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    number: u64,
    viewer: Option<&str>,
    can_moderate: bool,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    let is_author = viewer == Some(comment.author.as_str());
    html! {
        tr class="diff-comment" {
            td colspan="3" {
                p class="muted" { strong { (comment.author) } " — " (comment.created_at) }
                div class="readme plain" { (maud::PreEscaped(body_html)) }
                (super::super::components::comment_edit_delete_controls(
                    "/_merges",
                    repo_name,
                    repo_uid,
                    number,
                    comment_id,
                    &comment.body,
                    is_author,
                    is_author || can_moderate,
                    headers,
                    locale,
                ))
            }
        }
    }
}

fn form_row(
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    number: u64,
    anchor: &CommentAnchor,
    headers: &HeaderMap,
    locale: Locale,
) -> Markup {
    html! {
        tr class="diff-comment" {
            td colspan="3" {
                form method="post" action=(super::mutation_url("comment", repo_name)) {
                    (PreEscaped(csrf_field(headers)))
                    input type="hidden" name="repo_uid" value=(repo_uid);
                    input type="hidden" name="number" value=(number);
                    input type="hidden" name="anchor_path" value=(anchor.path);
                    input type="hidden" name="anchor_side" value=(side_param(anchor.side));
                    input type="hidden" name="anchor_line" value=(anchor.line);
                    textarea name="body" rows="3" cols="60" {}
                    p { button type="submit" { (t::comment_button(locale)) } }
                }
            }
        }
    }
}

/// Everything [`render`] needs beyond the diff and the thread itself.
/// Bundled so it doesn't cross clippy's `too_many_arguments` — same shape
/// `domain::release::NewAsset` uses for the same reason.
pub(super) struct View<'a> {
    /// Each comment's body already rendered to HTML, indexed the same way
    /// as `render`'s `comments` argument. Rendering one resolves `#123`
    /// mentions against storage and so is async, which this render path
    /// isn't; an index with no entry falls back to an empty body rather
    /// than panicking on a caller that got the lengths wrong.
    pub(super) comment_bodies: &'a [String],
    pub(super) repo_name: &'a str,
    pub(super) repo_uid: &'a gitquarry_core::storage::uid::Uid,
    pub(super) number: u64,
    /// Whether to offer the per-line "comment here" link at all.
    pub(super) can_comment: bool,
    /// The one line whose comment form is expanded, if any.
    pub(super) active: Option<&'a CommentAnchor>,
    /// The logged-in username, if any — gates each placed comment's own
    /// edit controls to its author, and (together with [`Self::can_moderate`])
    /// its delete control (see
    /// `super::super::components::comment_edit_delete_controls`).
    pub(super) viewer: Option<&'a str>,
    /// Whether the viewer is `Developer`+ on this repo — lets them delete
    /// (never edit) someone else's comment, same override
    /// `domain::merge_request::delete_comment`'s `is_moderator` param grants.
    pub(super) can_moderate: bool,
    pub(super) headers: &'a HeaderMap,
    pub(super) locale: Locale,
}

/// Bumps `counter` and hands the new value back — used from inside
/// `render`'s `html!` via `@let`, which needs a genuine expression on
/// its right-hand side (a bare `*counter += 1;` statement isn't valid
/// there, and discarding a plain `()`-valued assignment through `@let _ =`
/// trips clippy's unit-binding lint).
fn advance(counter: &mut i32) -> i32 {
    *counter += 1;
    *counter
}

/// `class` ("diff-add"/"diff-del"/"") as stored on a [`Row::Line`], for
/// anchor derivation and comment matching, mapped to the CSS class that
/// actually renders it: added/removed lines get a tinted *background*
/// (`diff-add-line`/`diff-del-line`) — not `components::color_diff`'s
/// `diff-add`/`diff-del` classes, which stay solid-text for the unrelated
/// insertion/deletion count badges in `commit_table` — so `highlight.js`'s
/// token colors show through.
fn line_css_class(class: &str) -> &'static str {
    match class {
        "diff-add" => "diff-add-line",
        "diff-del" => "diff-del-line",
        _ => "",
    }
}

/// `comments` is the MR's whole thread (each paired with its storage key,
/// see `domain::merge_request::list_comments`'s own doc comment), in order;
/// only the anchored ones land in the diff, and each is matched to at most
/// one line (the first occurrence of that path+side+line — a diff can
/// repeat a path only across separate `diff --git` sections, which don't
/// share line numbers).
pub(super) fn render(diff: &str, comments: &[(String, MrComment)], view: &View<'_>) -> Annotated {
    let rows = parse(diff);
    let mut placed = HashSet::new();
    // Bumped on each `Row::Hunk` so every `Row::Line` up to the next one can
    // share a `data-rows-group` — `app.js`'s `[data-toggle-rows]` handler
    // hides/shows all of them together. Starts at -1: `Row::Meta` rows (a
    // file's own header, before its first hunk) never reference this, so
    // its value there is moot.
    let mut hunk_index: i32 = -1;

    let markup = html! {
        table class="diff-annotated" {
            @for row in &rows {
                @match row {
                    Row::Meta(l) => tr { td class="diff-meta" colspan="3" { (l) } },
                    Row::Hunk(l) => {
                        @let hunk_index = advance(&mut hunk_index);
                        tr { td class="diff-hunk" colspan="3" {
                            button type="button" class="hunk-toggle" data-toggle-rows=(format!("hunk-{hunk_index}")) aria-expanded="true" { (l) }
                        } }
                    },
                    Row::Line { old, new, class, text, anchor } => {
                        tr data-rows-group=(format!("hunk-{hunk_index}")) {
                            td class="lineno" { @if let Some(n) = old { (n) } }
                            td class="lineno" { @if let Some(n) = new { (n) } }
                            td class=(line_css_class(class)) {
                                @if view.can_comment {
                                    a class="line-comment-link"
                                      href=(comment_link(view.repo_name, view.number, anchor)) {
                                        (t::comment_on_line_marker(view.locale))
                                    }
                                    " "
                                }
                                (PreEscaped(super::super::diff::code_fragment(&anchor.path, text)))
                            }
                        }
                        @for (index, (comment_id, comment)) in comments.iter().enumerate() {
                            @if comment.anchor.as_ref() == Some(anchor) && placed.insert(index) {
                                (comment_row(comment_id, comment, view.comment_bodies.get(index).map_or("", String::as_str), view.repo_name, view.repo_uid, view.number, view.viewer, view.can_moderate, view.headers, view.locale))
                            }
                        }
                        @if view.active == Some(anchor) {
                            (form_row(view.repo_name, view.repo_uid, view.number, anchor, view.headers, view.locale))
                        }
                    }
                }
            }
        }
    };

    Annotated { markup, placed }
}
