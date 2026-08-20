use gitquarry_forge_clients::github::*;
use gitquarry_forge_clients::{RemotePr, RemotePrState};

#[test]
fn parses_owner_repo_with_and_without_git_suffix() {
    assert_eq!(
        parse_owner_repo("https://github.com/acme/demo.git").unwrap(),
        ("acme".to_string(), "demo".to_string())
    );
    assert_eq!(
        parse_owner_repo("https://github.com/acme/demo").unwrap(),
        ("acme".to_string(), "demo".to_string())
    );
}

#[test]
fn rejects_a_url_missing_the_repo_segment() {
    assert!(parse_owner_repo("https://github.com/acme").is_err());
    assert!(parse_owner_repo("https://github.com/").is_err());
}

#[test]
fn maps_merged_flag_over_open_closed_state() {
    let merged = GhPullRequest {
        number: 1,
        title: "t".to_string(),
        body: None,
        state: "closed".to_string(),
        merged: true,
        user: GhUser {
            login: "alice".to_string(),
        },
        head: GhBranchRef {
            branch: "feature".to_string(),
        },
        base: GhBranchRef {
            branch: "main".to_string(),
        },
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-02T00:00:00Z".to_string(),
        merged_at: Some("2024-01-02T00:00:00Z".to_string()),
        merged_by: Some(GhUser {
            login: "bob".to_string(),
        }),
        merge_commit_sha: Some("deadbeef".to_string()),
    };
    let pr: RemotePr = merged.into();
    assert_eq!(pr.state, RemotePrState::Merged);
    assert_eq!(pr.merged_by.as_deref(), Some("bob"));

    let closed_not_merged = GhPullRequest {
        number: 2,
        title: "t".to_string(),
        body: None,
        state: "closed".to_string(),
        merged: false,
        user: GhUser {
            login: "alice".to_string(),
        },
        head: GhBranchRef {
            branch: "feature".to_string(),
        },
        base: GhBranchRef {
            branch: "main".to_string(),
        },
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-02T00:00:00Z".to_string(),
        merged_at: None,
        merged_by: None,
        merge_commit_sha: None,
    };
    let pr: RemotePr = closed_not_merged.into();
    assert_eq!(pr.state, RemotePrState::Closed);
}

#[test]
fn api_base_is_dotcoms_own_host_only_for_github_com() {
    assert_eq!(
        api_base_for("https://github.com/acme/demo.git"),
        "https://api.github.com"
    );
    assert_eq!(
        api_base_for("https://www.github.com/acme/demo"),
        "https://api.github.com"
    );
    // GitHub Enterprise Server: the API sits on the same host as the repos.
    assert_eq!(
        api_base_for("https://ghe.corp.example/acme/demo.git"),
        "https://ghe.corp.example/api/v3"
    );
    assert_eq!(
        api_base_for("http://ghe.internal/acme/demo"),
        "http://ghe.internal/api/v3"
    );
}

#[test]
fn a_url_whose_path_cant_name_a_real_repo_is_refused() {
    // Would otherwise be spliced into the REST path unescaped, turning the
    // request into one against a different endpoint.
    assert!(GitHubForge::new("https://github.com/acme/demo?x=1", "tok").is_err());
    assert!(GitHubForge::new("https://github.com/acme/../../other", "tok").is_err());
    assert!(GitHubForge::new("https://github.com/acme/demo", "tok").is_ok());
}
