//! `gitquarry backup`'s command line. The run itself is covered in
//! `gitquarry-core`'s own backup tests; what's specific to this command is
//! how its flags turn into a class selection, and that a contradictory pair
//! is refused instead of one flag silently winning.

use gitquarry_core::backup::inventory::Selection;
use gitquarry_domain::commands::backup::parse_args;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn no_flags_follows_the_instance_settings() {
    let parsed = parse_args(&args(&[])).unwrap().unwrap();
    assert_eq!(parsed.bucket, None);
    assert_eq!(parsed.dest, None);
    // `None`, not `Some(default)`: the run reads the selection out of
    // settings, and passing a default here would silently override it.
    assert_eq!(parsed.selection, None);
}

#[test]
fn bucket_and_dest_take_their_values() {
    let parsed = parse_args(&args(&["--bucket", "live", "--dest", "path:/srv/backup"]))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.bucket.as_deref(), Some("live"));
    assert_eq!(parsed.dest.as_deref(), Some("path:/srv/backup"));
}

#[test]
fn a_flag_missing_its_value_is_an_error() {
    assert!(parse_args(&args(&["--bucket"])).is_err());
    assert!(parse_args(&args(&["--dest"])).is_err());
}

#[test]
fn minimal_drops_the_audit_trail_and_the_caches() {
    let parsed = parse_args(&args(&["--minimal"])).unwrap().unwrap();
    assert_eq!(parsed.selection, Some(Selection::minimal()));
}

#[test]
fn with_derived_adds_the_caches_to_the_default() {
    let parsed = parse_args(&args(&["--with-derived"])).unwrap().unwrap();
    let selection = parsed.selection.expect("a selection override");
    assert!(selection.derived);
    assert!(selection.audit, "--with-derived shouldn't also drop the audit trail");
}

#[test]
fn the_two_selection_flags_cannot_be_combined() {
    // They contradict each other, and picking a winner silently would mean
    // one run backs up something the operator thought they'd excluded.
    let err = parse_args(&args(&["--minimal", "--with-derived"])).expect_err("contradictory flags must be refused");
    assert!(format!("{err:#}").contains("contradict"), "unexpected error: {err:#}");
}

#[test]
fn an_unknown_flag_is_refused() {
    assert!(parse_args(&args(&["--everything"])).is_err());
}

#[test]
fn help_parses_to_nothing_to_do() {
    assert!(parse_args(&args(&["--help"])).unwrap().is_none());
    assert!(parse_args(&args(&["-h"])).unwrap().is_none());
}
