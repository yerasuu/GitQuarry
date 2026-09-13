use gitquarry_core::auth::access::{MAX_GROUP_DEPTH, ancestor_group_names};

#[test]
fn ancestor_group_names_walks_outermost_first_and_includes_the_namespace_itself() {
    assert_eq!(ancestor_group_names("acme"), vec!["acme".to_string()]);
    assert_eq!(
        ancestor_group_names("acme/team/squad"),
        vec![
            "acme".to_string(),
            "acme/team".to_string(),
            "acme/team/squad".to_string()
        ]
    );
}

#[test]
fn ancestor_group_names_is_empty_for_an_empty_namespace() {
    assert!(ancestor_group_names("").is_empty());
}

/// Every permission check on a repo inside a group fetches one `Group` per
/// level, so an unbounded namespace would turn a single crafted request into
/// arbitrarily many GETs. A path deeper than the cap resolves as if it
/// stopped there rather than erroring — it can't name a real group anyway.
#[test]
fn ancestor_group_names_stops_at_the_depth_cap() {
    let deep = (0..MAX_GROUP_DEPTH + 5)
        .map(|i| format!("g{i}"))
        .collect::<Vec<_>>()
        .join("/");
    let names = ancestor_group_names(&deep);
    assert_eq!(names.len(), MAX_GROUP_DEPTH);
    assert_eq!(names[0], "g0");
}
