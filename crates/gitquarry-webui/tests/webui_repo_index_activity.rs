use gitquarry_webui::webui::repo::{ActivityItem, ActivityKind, top_recent_activity};

fn item(repo: &str, at: u64, kind: ActivityKind) -> ActivityItem {
    ActivityItem {
        repo: repo.to_string(),
        at,
        kind,
    }
}

#[test]
fn top_recent_activity_sorts_newest_first_and_caps_at_the_limit() {
    let items = vec![
        item(
            "a/one",
            100,
            ActivityKind::Push {
                commits: 1,
                branch: None,
            },
        ),
        item(
            "a/two",
            300,
            ActivityKind::Push {
                commits: 1,
                branch: None,
            },
        ),
        item(
            "a/three",
            200,
            ActivityKind::Push {
                commits: 1,
                branch: None,
            },
        ),
    ];
    let top = top_recent_activity(items, 2);
    let names: Vec<&str> = top.iter().map(|i| i.repo.as_str()).collect();
    assert_eq!(names, vec!["a/two", "a/three"]);
}

#[test]
fn describe_pluralizes_commit_count_and_names_the_branch() {
    assert_eq!(
        ActivityKind::Push {
            commits: 1,
            branch: Some("main".to_string())
        }
        .describe(),
        "1 commit pushed to main"
    );
    assert_eq!(
        ActivityKind::Push {
            commits: 4,
            branch: Some("master".to_string())
        }
        .describe(),
        "4 commits pushed to master"
    );
    assert_eq!(
        ActivityKind::Push {
            commits: 2,
            branch: None
        }
        .describe(),
        "2 commits pushed"
    );
}

#[test]
fn describe_formats_releases_and_merged_mrs() {
    assert_eq!(
        ActivityKind::Release {
            tag: "v0.4.3".to_string()
        }
        .describe(),
        "tagged v0.4.3"
    );
    assert_eq!(ActivityKind::MrMerged { number: 41 }.describe(), "MR #41 merged");
}
