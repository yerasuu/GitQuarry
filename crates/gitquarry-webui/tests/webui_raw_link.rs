use gitquarry_webui::webui::raw_link::*;

#[test]
fn github_raw_link_builds_a_refs_heads_url() {
    let url = github_raw_link("team/proj", "main", "src/lib.rs");
    assert_eq!(url, "/_raw/plain/team/proj/refs/heads/main/src/lib.rs");
}

#[test]
fn raw_link_omits_empty_fields() {
    let url = raw_link("atom", "proj", "").to_string();
    assert_eq!(url, "/_raw/atom/proj");
}

#[test]
fn raw_link_builds_a_release_asset_url() {
    let url = raw_link("release_asset", "team/proj", "")
        .tag("v1.0")
        .filename("proj.zip")
        .to_string();
    assert_eq!(url, "/_raw/release_asset/team/proj?tag=v1.0&filename=proj.zip");
}
