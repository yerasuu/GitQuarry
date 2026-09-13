use gitquarry_core::storage::entities::{RepoConfig, Visibility};
use gitquarry_webui::auth::web::profile::*;

fn repo(name: &str, visibility: Visibility) -> (String, RepoConfig) {
    (
        name.to_string(),
        RepoConfig {
            uid: Default::default(),
            full_name: name.to_string(),
            display_name: None,
            description: None,
            owner: None,
            visibility,
            archived: false,
            deleted_at: None,
            last_push: None,
            last_push_commits: None,
            last_push_branch: None,
            pack_bytes: 0,
            lfs_bytes: 0,
            required_reviewers: Default::default(),
            is_template: false,
            protected_branches: Default::default(),
            protected_tags: Default::default(),
            require_signed_commits: Default::default(),
            lfs_max_blob_size: Default::default(),
            topics: Default::default(),
            forked_from: None,
            fork_count: 0,
            star_count: 0,
            watcher_count: 0,
            primary_language: None,
            labels: Default::default(),
        },
    )
}

#[test]
fn profile_row_shows_the_display_name_and_links_the_slug() {
    let (name, mut config) = repo("alice/my-proj", Visibility::Public);
    config.display_name = Some("My Proj!".to_string());
    let html = render_profile(
        "alice",
        &[(name, config)],
        true,
        false,
        None,
        gitquarry_i18n::Locale::Es,
    )
    .into_string();
    assert!(html.contains(r#"href="/alice/my-proj""#));
    assert!(html.contains("alice/My Proj!"));
}

#[test]
fn profile_marks_private_repos_and_lists_public_ones() {
    let repos = vec![
        repo("alice/public-one", Visibility::Public),
        repo("alice/secret", Visibility::Private),
    ];
    let html = render_profile("alice", &repos, true, false, None, gitquarry_i18n::Locale::Es).into_string();
    assert!(html.contains("public-one"));
    assert!(html.contains("secret"));
    let secret_row = html.split("<tr>").find(|r| r.contains("secret")).unwrap();
    assert!(secret_row.contains("badge"));
    let public_row = html.split("<tr>").find(|r| r.contains("public-one")).unwrap();
    assert!(!public_row.contains("badge"));
}

#[test]
fn profile_empty_hint_without_repos() {
    assert!(
        render_profile("bob", &[], true, false, None, gitquarry_i18n::Locale::Es)
            .into_string()
            .contains("sin repos visibles")
    );
}

#[test]
fn profile_links_to_account_settings_only_for_the_owner() {
    let someone_elses = render_profile("alice", &[], false, false, None, gitquarry_i18n::Locale::Es).into_string();
    assert!(!someone_elses.contains("/_account/profile"));

    let own = render_profile("alice", &[], true, false, None, gitquarry_i18n::Locale::Es).into_string();
    assert!(own.contains(r#"href="/_account/profile""#));
}

#[test]
fn profile_links_to_favorites_only_when_public() {
    let hidden = render_profile("alice", &[], true, false, None, gitquarry_i18n::Locale::Es).into_string();
    assert!(!hidden.contains("/_profile/alice/favorites"));

    let shown = render_profile("alice", &[], true, true, None, gitquarry_i18n::Locale::Es).into_string();
    assert!(shown.contains(r#"href="/_profile/alice/favorites""#));
}

#[test]
fn profile_shows_star_count_only_for_starred_repos() {
    let mut starred = repo("alice/starred", Visibility::Public);
    starred.1.star_count = 3;
    let unstarred = repo("alice/unstarred", Visibility::Public);
    let repos = vec![starred, unstarred];

    let html = render_profile("alice", &repos, true, false, None, gitquarry_i18n::Locale::Es).into_string();
    let starred_row = html.split("<tr>").find(|r| r.contains("alice/starred")).unwrap();
    assert!(starred_row.contains('\u{2605}'));
    let unstarred_row = html.split("<tr>").find(|r| r.contains("alice/unstarred")).unwrap();
    assert!(!unstarred_row.contains('\u{2605}'));
}
