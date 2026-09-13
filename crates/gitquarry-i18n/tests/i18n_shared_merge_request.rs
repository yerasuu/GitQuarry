use gitquarry_i18n::Locale;
use gitquarry_i18n::shared::merge_request::*;

#[test]
fn merge_request_not_found_carries_the_number() {
    assert!(merge_request_not_found(Locale::Es, 42).contains("42"));
}

#[test]
fn merge_request_wrong_status_differs_by_locale() {
    assert_ne!(
        merge_request_wrong_status(Locale::Es),
        merge_request_wrong_status(Locale::En)
    );
}

#[test]
fn source_repo_not_a_fork_differs_by_locale() {
    assert_ne!(source_repo_not_a_fork(Locale::Es), source_repo_not_a_fork(Locale::En));
}
