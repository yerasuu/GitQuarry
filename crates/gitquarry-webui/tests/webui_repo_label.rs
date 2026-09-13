use gitquarry_webui::webui::repo_label::repo_label;

#[test]
fn swaps_only_the_local_segment_for_the_display_name() {
    assert_eq!(repo_label("acme/my-proj", Some("My Proj!")), "acme/My Proj!");
}

#[test]
fn keeps_the_full_name_when_there_is_no_display_name() {
    assert_eq!(repo_label("acme/my-proj", None), "acme/my-proj");
    assert_eq!(repo_label("acme/my-proj", Some("")), "acme/my-proj");
}

#[test]
fn a_namespaceless_name_is_replaced_whole() {
    assert_eq!(repo_label("my-proj", Some("My Proj")), "My Proj");
}

#[test]
fn a_display_name_holding_a_slash_stays_inside_its_own_namespace() {
    assert_eq!(repo_label("acme/a-b", Some("a/b")), "acme/a/b");
}
