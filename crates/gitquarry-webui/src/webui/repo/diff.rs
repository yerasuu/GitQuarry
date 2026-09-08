//! Shared unified-diff line parser — "which file, and so which language,
//! does this line belong to" tracked once instead of independently by
//! [`super::components::color_diff`]/[`super::components::side_by_side_diff`]
//! and [`super::merges::annotated_diff`], since all three need it to tag
//! each line's `<code class="language-*">` for `highlight.js` (client-side,
//! vendored — see `app.js`'s own doc comment) to tokenize.

pub(super) enum Row<'a> {
    /// `diff --git`, `index`, mode, `---`/`+++` headers — nothing to anchor,
    /// no language (diff syntax, not source code).
    Meta(&'a str),
    /// An `@@ ...@@` hunk header — same reasoning as [`Row::Meta`].
    Hunk(&'a str),
    /// A content line. `text` has its leading `+`/`-`/` ` diff marker
    /// already stripped — that marker isn't source text, so callers
    /// reattach it themselves from `class` (see [`prefix_char`]).
    Line {
        class: &'static str,
        path: &'a str,
        text: &'a str,
    },
}

/// The path a `+++ b/<path>` / `--- a/<path>` header names, or `None` for
/// `/dev/null` (a pure add or delete has one of the two).
pub(super) fn header_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("+++ ").or_else(|| line.strip_prefix("--- "))?;
    if rest == "/dev/null" {
        return None;
    }
    // git prefixes with `a/`/`b/` unless `--no-prefix`; a trailing tab
    // introduces the timestamp field some diff generators append.
    let rest = rest.split('\t').next().unwrap_or(rest);
    Some(
        rest.strip_prefix("a/")
            .or_else(|| rest.strip_prefix("b/"))
            .unwrap_or(rest),
    )
}

/// Walks a (possibly multi-file) unified diff into [`Row`]s, tracking the
/// current file's path from the last `+++`/`---` header seen.
pub(super) fn parse(diff: &str) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let mut path = "";

    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            if let Some(found) = header_path(line) {
                path = found;
            }
            rows.push(Row::Meta(line));
        } else if line.starts_with("@@") {
            rows.push(Row::Hunk(line));
        } else if let Some(text) = line.strip_prefix('+') {
            rows.push(Row::Line {
                class: "diff-add",
                path,
                text,
            });
        } else if let Some(text) = line.strip_prefix('-') {
            rows.push(Row::Line {
                class: "diff-del",
                path,
                text,
            });
        } else if let Some(text) = line.strip_prefix(' ') {
            rows.push(Row::Line { class: "", path, text });
        } else {
            rows.push(Row::Meta(line));
        }
    }
    rows
}

/// `+`/`-`/` ` — the diff marker [`parse`] stripped off a [`Row::Line`]'s
/// `text`, derived back from `class` for renderers that show it inline
/// (the unified stream) rather than via a separate gutter column.
pub(super) fn prefix_char(class: &str) -> char {
    match class {
        "diff-add" => '+',
        "diff-del" => '-',
        _ => ' ',
    }
}

/// The `<code class="language-*">escaped text</code>` fragment every
/// renderer wraps a [`Row::Line`]'s content in, so `highlight.js` tokenizes
/// it client-side — `path` (via [`super::super::language_tag::language_tag`])
/// picks the language, `text` is escaped here so callers never emit it raw.
pub(super) fn code_fragment(path: &str, text: &str) -> String {
    format!(
        "<code class=\"language-{}\">{}</code>",
        super::super::language_tag::language_tag(path),
        super::super::esc(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_path_strips_the_git_prefix_and_ignores_dev_null() {
        assert_eq!(header_path("+++ b/src/main.rs"), Some("src/main.rs"));
        assert_eq!(header_path("--- a/src/main.rs"), Some("src/main.rs"));
        assert_eq!(header_path("+++ b/src/main.rs\t2024-01-01"), Some("src/main.rs"));
        assert_eq!(header_path("+++ /dev/null"), None);
        assert_eq!(header_path("@@ -1 +1 @@"), None);
    }

    #[test]
    fn parse_tracks_the_current_file_path_across_lines() {
        let diff = concat!(
            "diff --git a/f.rs b/f.rs\n",
            "--- a/f.rs\n",
            "+++ b/f.rs\n",
            "@@ -1,2 +1,2 @@\n",
            " context\n",
            "-gone\n",
            "+added\n",
        );
        let rows = parse(diff);
        let lines: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Line { class, path, text } => Some((*class, *path, *text)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                ("", "f.rs", "context"),
                ("diff-del", "f.rs", "gone"),
                ("diff-add", "f.rs", "added"),
            ]
        );
    }
}
