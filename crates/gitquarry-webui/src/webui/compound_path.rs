//! Resolving GitHub-style compound paths (`{repo...}/{marker...}/{branch...}/{path...}`,
//! e.g. `.../blob/main/src/lib.rs` or `.../refs/heads/release/v1/README.md`)
//! without assuming a fixed segment count for either the repo name or the
//! branch name — this codebase doesn't guarantee repo names are exactly
//! `namespace/name` (the UI only ever creates them that way, but
//! [`gitquarry_core::validate::sanitize_path`] doesn't enforce it), and a branch name
//! can always contain `/`. GitHub has the exact same ambiguity once a branch
//! name has a slash in it — a fixed marker segment only tells you *that*
//! what follows is a branch, not *where* the branch name ends.
//!
//! Both ambiguities are resolved the same way: by checking what actually
//! exists (a real repo, then a real ref) rather than guessing a depth.

use super::AppError;
use super::repo::can_view_repo;
use axum::http::HeaderMap;
use gitquarry_core::app::{AppContext, Mirror, ResolvedRepo};
use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_core::{git, validate};
use std::path::Path;

pub(crate) struct CompoundPath {
    pub name: String,
    pub resolved: ResolvedRepo,
    pub config: RepoConfig,
    pub mirror: Mirror,
    pub branch: String,
    pub path: String,
}

/// `marker` is the literal segment sequence separating the repo part from
/// the branch+path part — `&["blob"]` for the HTML view, `&["refs",
/// "heads"]` for the raw-content route. `Ok(None)` means "this path doesn't
/// use the compound form at all, or the repo part isn't real" — the caller
/// treats that as "try the plain interpretation instead", not necessarily
/// a 404, since the same wildcard capture also serves plain repo names.
pub(crate) async fn resolve_compound_path(
    ctx: &AppContext,
    headers: &HeaderMap,
    full: &str,
    marker: &[&str],
) -> Result<Option<CompoundPath>, AppError> {
    let segments: Vec<&str> = full.split('/').filter(|s| !s.is_empty()).collect();
    let Some(marker_at) = find_marker(&segments, marker) else {
        return Ok(None);
    };
    if marker_at == 0 {
        return Ok(None);
    }
    let Ok(name) = validate::sanitize_path(&segments[..marker_at].join("/")) else {
        return Ok(None);
    };
    let Some(resolved) = ctx.resolve_repo(&name).await? else {
        return Ok(None);
    };
    let (config, _etag) = resolved.remote.get_repo_config().await?;
    if !can_view_repo(ctx, headers, &name, config.visibility).await? {
        return Ok(None);
    }
    let rest = &segments[marker_at + marker.len()..];
    if rest.is_empty() {
        // The marker is the last segment, nothing after it — most likely a
        // plain repo whose name happens to end in "blob"/"heads", not a
        // compound URL missing its branch. Let the plain-repo-name
        // interpretation handle it instead of erroring here.
        return Ok(None);
    }

    let mirror = ctx.open_mirror(&resolved).await?;
    let (branch, path) = split_branch_and_path(&mirror.path, rest).await?;
    validate::validate_rev(&branch).map_err(AppError::bad_request)?;
    validate::validate_tree_path(&path).map_err(AppError::bad_request)?;
    Ok(Some(CompoundPath {
        name,
        resolved,
        config,
        mirror,
        branch,
        path,
    }))
}

// `pub` (not `pub(crate)`) because the integration test in `tests/` needs to
// reach it, even though this is only ever called from within the crate in
// non-test builds.
pub fn find_marker(segments: &[&str], marker: &[&str]) -> Option<usize> {
    if marker.is_empty() || segments.len() <= marker.len() {
        return None;
    }
    segments.windows(marker.len()).position(|w| w == marker)
}

/// `rest` is "{branch...}/{path...}" combined — ambiguous on its own, same
/// as GitHub's own `/blob/{branch-or-tag}/{path}` is once the branch name
/// has a slash in it. Resolved by trying real ref names shortest-prefix
/// first, so a branch literally named "release" wins over treating it as
/// the start of some longer, unrelated ref.
// `pub` (not `pub(crate)`) because the integration test in `tests/` needs to
// reach it, even though this is only ever called from within the crate in
// non-test builds.
pub async fn split_branch_and_path(git_dir: &Path, rest: &[&str]) -> Result<(String, String), AppError> {
    let refs = git::browse::list_refs_detailed(git_dir, "refs/").await?;
    let short_names: Vec<&str> = refs
        .iter()
        .filter_map(|r| {
            r.name
                .strip_prefix("refs/heads/")
                .or_else(|| r.name.strip_prefix("refs/tags/"))
        })
        .collect();
    for split in 1..=rest.len() {
        let candidate = rest[..split].join("/");
        if short_names.contains(&candidate.as_str()) {
            return Ok((candidate, rest[split..].join("/")));
        }
    }
    // No known branch/tag matches any prefix — most likely a bare SHA,
    // which never shows up in `refs/`: fall back to the first segment as
    // the rev and let git itself be the final judge, exactly like a bad
    // `?ref=` already does downstream.
    Ok((rest[0].to_string(), rest[1..].join("/")))
}
