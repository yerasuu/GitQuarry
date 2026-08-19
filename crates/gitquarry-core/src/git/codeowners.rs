//! `CODEOWNERS` (GitHub's own file, same location convention: repo root)
//! parsing and path matching — a deliberate *subset* of gitignore-style
//! glob syntax, not a full implementation: exact paths, a `dir/` prefix
//! (everything under that directory), a bare `*`-pattern with no `/`
//! (matched against any path's final segment, repo-wide — `*.rs` matches
//! `src/lib.rs` just as much as `lib.rs`), and per-segment `*`/`**` inside a
//! `/`-containing pattern. Real CODEOWNERS files lean on a handful of
//! common shapes in practice; this covers those without pulling in a full
//! gitignore-matching crate for the long tail of edge cases nothing in
//! this codebase's own usage needs.
//!
//! Same "last matching rule wins, never a union of every match" semantics
//! GitHub's own CODEOWNERS has: a later, more specific rule overriding an
//! earlier, broader one is the whole point of listing them top-to-bottom.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub pattern: String,
    pub owners: Vec<String>,
}

/// Parses `CODEOWNERS` content: one rule per non-empty, non-comment line,
/// `<pattern> <owner> [<owner>...]`. Owners keep their leading `@` stripped
/// (GitHub's convention is `@username`, this codebase's usernames never
/// have one) — a line naming zero owners is skipped, same as a line that's
/// just a comment.
pub fn parse(content: &str) -> Vec<Rule> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pattern = parts.next()?.to_string();
            let owners: Vec<String> = parts.map(|o| o.trim_start_matches('@').to_string()).collect();
            (!owners.is_empty()).then_some(Rule { pattern, owners })
        })
        .collect()
}

/// The owners of the *last* rule in `rules` whose pattern matches `path`, or
/// an empty slice if nothing matches — same precedence CODEOWNERS itself
/// documents (bottom-most matching entry wins, entirely, not merged with
/// earlier matches).
pub fn owners_for_path<'a>(rules: &'a [Rule], path: &str) -> &'a [String] {
    rules
        .iter()
        .rev()
        .find(|rule| matches(&rule.pattern, path))
        .map(|rule| rule.owners.as_slice())
        .unwrap_or(&[])
}

fn matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
    if let Some(dir) = pattern.strip_suffix('/') {
        return path == dir || path.starts_with(&format!("{dir}/"));
    }
    if !pattern.contains('/') {
        let basename = path.rsplit('/').next().unwrap_or(path);
        return segment_glob(pattern, basename);
    }
    let pattern_segs: Vec<&str> = pattern.split('/').collect();
    let path_segs: Vec<&str> = path.split('/').collect();
    match_segments(&pattern_segs, &path_segs)
}

/// Segment-by-segment match, `**` (its own path segment) allowed to
/// consume zero or more path segments — the standard recursive shape a
/// small gitignore-style matcher uses.
fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some(&"**") => match_segments(&pattern[1..], path) || (!path.is_empty() && match_segments(pattern, &path[1..])),
        Some(seg) => !path.is_empty() && segment_glob(seg, path[0]) && match_segments(&pattern[1..], &path[1..]),
    }
}

/// `*` within one path segment (never crosses `/`, this is only ever called
/// with a single segment on each side) — splits `pattern` on `*` into
/// literal parts and checks they occur in `text` in order, anchored at the
/// start/end unless `pattern` itself starts/ends with `*`.
fn segment_glob(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let parts: Vec<&str> = pattern.split('*').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return true; // pattern was just "*" (or "**", but that never reaches here)
    }

    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        let is_first = i == 0;
        let is_last = i == parts.len() - 1;
        if is_first && anchored_start {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if is_last && anchored_end {
            return rest.ends_with(part);
        } else {
            let Some(idx) = rest.find(part) else { return false };
            rest = &rest[idx + part.len()..];
        }
    }
    true
}
