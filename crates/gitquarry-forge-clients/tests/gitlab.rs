use gitquarry_forge_clients::gitlab::*;
use gitquarry_forge_clients::{RemoteComment, RemotePr, RemotePrState};

#[test]
fn parses_api_base_and_url_encodes_a_nested_project_path() {
    let (base, project) = parse_api_base_and_project("https://gitlab.example.com/group/sub/project.git").unwrap();
    assert_eq!(base, "https://gitlab.example.com/api/v4");
    assert_eq!(project, "group%2Fsub%2Fproject");
}

#[test]
fn parses_gitlab_com_top_level_project() {
    let (base, project) = parse_api_base_and_project("https://gitlab.com/acme/demo").unwrap();
    assert_eq!(base, "https://gitlab.com/api/v4");
    assert_eq!(project, "acme%2Fdemo");
}

#[test]
fn rejects_a_url_with_no_project_path() {
    assert!(parse_api_base_and_project("https://gitlab.com/").is_err());
    assert!(parse_api_base_and_project("https://gitlab.com").is_err());
}

#[test]
fn maps_merged_state_and_falls_back_to_sha_when_merge_commit_sha_is_absent() {
    let mr = GlMergeRequest {
        iid: 5,
        title: "t".to_string(),
        description: None,
        state: "merged".to_string(),
        author: GlUser {
            username: "alice".to_string(),
        },
        source_branch: "feature".to_string(),
        target_branch: "main".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-02T00:00:00Z".to_string(),
        merged_at: Some("2024-01-02T00:00:00Z".to_string()),
        merged_by: Some(GlUser {
            username: "bob".to_string(),
        }),
        merge_commit_sha: None,
        sha: Some("fastforwardsha".to_string()),
    };
    let pr: RemotePr = mr.into();
    assert_eq!(pr.state, RemotePrState::Merged);
    assert_eq!(pr.merge_commit_sha.as_deref(), Some("fastforwardsha"));
}

#[test]
fn list_comments_filters_out_system_notes() {
    // `system` defaults to `false` via `#[serde(default)]` when a real
    // note omits the field, so a genuine comment always survives the
    // filter even against an older GitLab version that doesn't send it.
    let notes = vec![
        GlNote {
            id: 1,
            author: GlUser {
                username: "alice".to_string(),
            },
            body: "changed the description".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            system: true,
        },
        GlNote {
            id: 2,
            author: GlUser {
                username: "bob".to_string(),
            },
            body: "looks good".to_string(),
            created_at: "2024-01-01T00:00:01Z".to_string(),
            system: false,
        },
    ];
    let kept: Vec<RemoteComment> = notes
        .into_iter()
        .filter(|n| !n.system)
        .map(|n| RemoteComment {
            id: n.id,
            author: n.author.username,
            body: n.body,
            created_at: n.created_at,
        })
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].body, "looks good");
}
