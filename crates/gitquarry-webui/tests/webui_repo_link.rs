use gitquarry_webui::webui::repo_link::*;

#[test]
fn github_blob_link_builds_a_blob_url() {
    let url = github_blob_link("team/proj", "main", "src/lib.rs");
    assert_eq!(url, "/team/proj/blob/main/src/lib.rs");
}

#[test]
fn repo_link_builds_query_string() {
    let url = repo_link("team/proj", "commit", "").sha("abc123").to_string();
    assert_eq!(url, "/team/proj?view=commit&sha=abc123");
}

#[test]
fn repo_link_omits_empty_fields() {
    let url = repo_link("proj", "tree", "main").to_string();
    assert_eq!(url, "/proj?view=tree&ref=main");
}

#[test]
fn repo_link_builds_a_release_url() {
    let url = repo_link("team/proj", "release", "").tag("v1.0").to_string();
    assert_eq!(url, "/team/proj?view=release&tag=v1.0");
}
