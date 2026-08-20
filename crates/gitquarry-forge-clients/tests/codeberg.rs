use gitquarry_forge_clients::codeberg::*;
use gitquarry_forge_clients::{RemotePr, RemotePrState};

#[test]
fn parses_owner_repo_and_derives_the_api_base_from_the_host() {
    let (base, owner, repo) = parse_api_base_and_repo("https://codeberg.org/acme/demo.git").unwrap();
    assert_eq!(base, "https://codeberg.org/api/v1");
    assert_eq!(owner, "acme");
    assert_eq!(repo, "demo");
}

#[test]
fn works_against_a_self_hosted_gitea_instance() {
    let (base, owner, repo) = parse_api_base_and_repo("https://git.example.com/team/proj").unwrap();
    assert_eq!(base, "https://git.example.com/api/v1");
    assert_eq!(owner, "team");
    assert_eq!(repo, "proj");
}

#[test]
fn rejects_a_url_missing_the_repo_segment() {
    assert!(parse_api_base_and_repo("https://codeberg.org/acme").is_err());
    assert!(parse_api_base_and_repo("https://codeberg.org/").is_err());
}

#[test]
fn maps_merged_flag_over_open_closed_state() {
    let merged = GtPullRequest {
        number: 1,
        title: "t".to_string(),
        body: None,
        state: "closed".to_string(),
        merged: true,
        user: GtUser {
            login: "alice".to_string(),
        },
        head: GtBranchRef {
            branch: "feature".to_string(),
        },
        base: GtBranchRef {
            branch: "main".to_string(),
        },
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-02T00:00:00Z".to_string(),
        merged_at: Some("2024-01-02T00:00:00Z".to_string()),
        merged_by: Some(GtUser {
            login: "bob".to_string(),
        }),
        merge_commit_sha: Some("deadbeef".to_string()),
    };
    let pr: RemotePr = merged.into();
    assert_eq!(pr.state, RemotePrState::Merged);
    assert_eq!(pr.merged_by.as_deref(), Some("bob"));
}
