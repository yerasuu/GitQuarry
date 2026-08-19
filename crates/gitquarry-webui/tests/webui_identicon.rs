use gitquarry_webui::webui::identicon::*;

#[test]
fn same_seed_produces_the_same_markup() {
    assert_eq!(
        identicon("alice", 24).into_string(),
        identicon("alice", 24).into_string()
    );
}

#[test]
fn different_seeds_produce_different_markup() {
    assert_ne!(identicon("alice", 24).into_string(), identicon("bob", 24).into_string());
}

#[test]
fn is_decorative_and_sized_by_the_caller() {
    let html = identicon("alice", 40).into_string();
    assert!(html.contains(r#"aria-hidden="true""#), "{html}");
    assert!(html.contains(r#"width="40""#), "{html}");
    assert!(html.contains(r#"height="40""#), "{html}");
}
