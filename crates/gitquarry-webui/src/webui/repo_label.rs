//! The label a repo is shown under, as opposed to the slug it's addressed
//! by: every listing renders `RepoConfig::display_name`, while the href
//! next to it still uses `full_name` (the only string that routes).

/// `full_name` is always `<namespace>/<slug>` (see [`gitquarry_core::validate`]'s
/// own invariant) and only its local segment is a repo name — the namespace
/// is the owner's, so it's kept verbatim and only the slug is swapped for
/// `display_name`. An absent (a repo predating the field, not yet fixed by
/// `domain::repo::backfill_slug`) or empty display name falls back to
/// `full_name` untouched, never to a blank label.
pub fn repo_label(full_name: &str, display_name: Option<&str>) -> String {
    let Some(display_name) = display_name.filter(|d| !d.is_empty()) else {
        return full_name.to_string();
    };
    match full_name.rsplit_once('/') {
        Some((namespace, _)) => format!("{namespace}/{display_name}"),
        None => display_name.to_string(),
    }
}
