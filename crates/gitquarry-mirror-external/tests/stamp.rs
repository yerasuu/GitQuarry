use gitquarry_mirror_external::stamp::{later, max, to_unix};

#[test]
fn reads_every_shape_the_three_providers_and_this_codebase_produce() {
    // This codebase's own (`storage::audit::format_unix_secs`).
    assert_eq!(to_unix("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(to_unix("2024-03-01T12:34:56Z"), Some(1_709_296_496));
    // GitLab: milliseconds.
    assert_eq!(to_unix("2024-03-01T12:34:56.789Z"), Some(1_709_296_496));
    // Gitea on a server that isn't on UTC.
    assert_eq!(to_unix("2024-03-01T14:34:56+02:00"), Some(1_709_296_496));
    assert_eq!(to_unix("2024-03-01T10:34:56-0200"), Some(1_709_296_496));
    // Not a stamp at all.
    assert_eq!(to_unix(""), None);
    assert_eq!(to_unix("yesterday"), None);
}

#[test]
fn the_same_instant_in_two_shapes_is_neither_newer_nor_older() {
    // The regression: `'.' < 'Z'`, so a byte comparison called the
    // millisecond form older and let a stale side win.
    assert!(!later("2024-03-01T12:34:56.000Z", "2024-03-01T12:34:56Z"));
    assert!(!later("2024-03-01T12:34:56Z", "2024-03-01T12:34:56.000Z"));
    assert!(!later("2024-03-01T14:34:56+02:00", "2024-03-01T12:34:56Z"));
    assert!(later("2024-03-01T12:34:57Z", "2024-03-01T12:34:56.999Z"));
}

#[test]
fn max_keeps_the_later_instant_across_shapes() {
    assert_eq!(
        max("2024-03-01T12:34:56.999Z", "2024-03-01T12:35:00Z"),
        "2024-03-01T12:35:00Z"
    );
    // An empty cursor (a link that never synced) never wins.
    assert_eq!(max("", "2024-03-01T12:35:00Z"), "2024-03-01T12:35:00Z");
}
