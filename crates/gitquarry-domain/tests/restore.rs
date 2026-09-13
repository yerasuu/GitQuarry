//! `gitquarry restore`'s command line. The restore itself is covered by
//! `gitquarry-core`'s snapshot tests; what matters here is that the command
//! can't be talked into overwriting live data by accident.

use gitquarry_domain::commands::restore::parse_args;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_snapshot_id_is_positional() {
    let parsed = parse_args(&args(&["00001-abcd"])).unwrap().unwrap();
    assert_eq!(parsed.snapshot_id.as_deref(), Some("00001-abcd"));
    assert!(!parsed.list);
}

#[test]
fn a_restore_is_a_dry_run_until_asked_twice() {
    let parsed = parse_args(&args(&["00001-abcd"])).unwrap().unwrap();
    assert!(!parsed.apply, "a restore must not overwrite anything by default");

    let parsed = parse_args(&args(&["00001-abcd", "--apply"])).unwrap().unwrap();
    assert!(parsed.apply);
}

#[test]
fn asking_for_nothing_in_particular_is_refused() {
    // Without an id there's nothing to restore, and defaulting to "the
    // newest one" would be a destructive guess.
    let err = parse_args(&args(&[])).expect_err("a restore with no target must be refused");
    assert!(
        format!("{err:#}").contains("which snapshot"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn list_needs_no_snapshot_id() {
    let parsed = parse_args(&args(&["--list"])).unwrap().unwrap();
    assert!(parsed.list);
    assert_eq!(parsed.snapshot_id, None);
}

#[test]
fn the_snapshot_source_can_be_overridden() {
    let parsed = parse_args(&args(&["--from", "bucket:offsite", "00001-abcd"]))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.from.as_deref(), Some("bucket:offsite"));
    assert_eq!(parsed.snapshot_id.as_deref(), Some("00001-abcd"));
}

#[test]
fn a_second_positional_argument_is_refused() {
    // Two ids would mean one of them silently doesn't get restored.
    assert!(parse_args(&args(&["one", "two"])).is_err());
}

#[test]
fn an_unknown_flag_is_refused() {
    assert!(parse_args(&args(&["--force"])).is_err());
}

#[test]
fn a_flag_missing_its_value_is_an_error() {
    assert!(parse_args(&args(&["--from"])).is_err());
    assert!(parse_args(&args(&["--bucket"])).is_err());
}

#[test]
fn help_parses_to_nothing_to_do() {
    assert!(parse_args(&args(&["--help"])).unwrap().is_none());
}
