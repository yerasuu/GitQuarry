use gitquarry_core::mirror::hook::*;
use std::path::Path;

#[test]
fn detects_zero_shas_of_both_widths() {
    assert!(is_zero_sha(&"0".repeat(40)));
    assert!(is_zero_sha(&"0".repeat(64)));
    assert!(!is_zero_sha(&"0".repeat(39)));
    assert!(!is_zero_sha("abc"));
}

#[test]
fn zero_sha_matches_repo_hash_width() {
    assert_eq!(zero_sha_like(&"a".repeat(64)).len(), 64);
    assert_eq!(zero_sha_like(&"a".repeat(40)).len(), 40);
}

#[test]
fn parses_hook_lines() {
    let updates = parse_hook_input("aaa bbb refs/heads/main\n\nccc ddd refs/tags/v1\n");
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].refname, "refs/heads/main");
    assert_eq!(updates[1].new, "ddd");
}

#[test]
fn touched_refs_filters_by_prefix_and_protected_set() {
    let updates = parse_hook_input("aaa bbb refs/heads/main\nccc ddd refs/heads/dev\neee fff refs/tags/v1\n");
    let mut protected = std::collections::BTreeSet::new();
    protected.insert("main".to_string());
    let touched = touched_refs(&updates, "refs/heads/", &protected);
    assert_eq!(touched.len(), 1);
    assert_eq!(touched[0].refname, "refs/heads/main");
}

#[tokio::test]
async fn check_protected_denies_non_owner_before_anything_else() {
    let updates = parse_hook_input(&format!("{} {} refs/heads/main\n", "a".repeat(40), "b".repeat(40)));
    let touched: Vec<&RefUpdate> = updates.iter().collect();
    let msg = check_protected(Path::new("."), &touched, false, true, "protected branch")
        .await
        .unwrap();
    assert!(msg.unwrap().contains("only an owner"));
}

#[tokio::test]
async fn check_protected_rejects_deleting_a_protected_ref() {
    let zero = "0".repeat(40);
    let updates = parse_hook_input(&format!("{} {zero} refs/tags/v1\n", "a".repeat(40)));
    let touched: Vec<&RefUpdate> = updates.iter().collect();
    let msg = check_protected(Path::new("."), &touched, true, false, "protected tag")
        .await
        .unwrap();
    assert!(msg.unwrap().contains("can't be deleted"));
}

#[tokio::test]
async fn check_protected_rejects_any_move_of_a_protected_tag() {
    let updates = parse_hook_input(&format!("{} {} refs/tags/v1\n", "a".repeat(40), "b".repeat(40)));
    let touched: Vec<&RefUpdate> = updates.iter().collect();
    // allow_fast_forward = false: a tag never moves once protected, even
    // to a plain descendant commit.
    let msg = check_protected(Path::new("."), &touched, true, false, "protected tag")
        .await
        .unwrap();
    assert!(msg.unwrap().contains("can't be moved"));
}

#[tokio::test]
async fn check_protected_allows_creating_a_new_protected_ref() {
    let zero = "0".repeat(40);
    let updates = parse_hook_input(&format!("{zero} {} refs/tags/v1\n", "a".repeat(40)));
    let touched: Vec<&RefUpdate> = updates.iter().collect();
    let msg = check_protected(Path::new("."), &touched, true, false, "protected tag")
        .await
        .unwrap();
    assert!(msg.is_none());
}

fn test_user(name: &str, is_admin: bool) -> gitquarry_core::auth::entities::User {
    gitquarry_core::auth::entities::User {
        uid: gitquarry_core::storage::uid::Uid::new(),
        username: name.to_string(),
        email: None,
        pubkeys: Vec::new(),
        gpg_keys: Vec::new(),
        repos: Default::default(),
        default_permission: None,
        password_hash: None,
        is_admin,
        session_epoch: 0,
        is_federation_moderator: false,
        starred: Default::default(),
        watched: Default::default(),
        text_size_default: String::new(),
        theme_default: String::new(),
        density_default: String::new(),
        favorites_public: false,
        totp_secret: None,
        totp_enabled: false,
        totp_recovery_codes: Default::default(),
        totp_last_step: 0,
        locale_default: String::new(),
        storage_quota_bytes: None,
        avatar_content_type: None,
        secondary_emails: Default::default(),
    }
}

fn admin_user(name: &str) -> gitquarry_core::auth::entities::User {
    test_user(name, true)
}

fn group_owned_by(group: &str, owner: &gitquarry_core::auth::entities::User) -> gitquarry_core::auth::entities::Group {
    gitquarry_core::auth::entities::Group {
        name: group.to_string(),
        members: std::collections::BTreeMap::from([(
            owner.uid.clone(),
            gitquarry_core::auth::entities::Permission::Owner,
        )]),
        ..Default::default()
    }
}

#[test]
fn admin_can_always_push_protected() {
    let root = admin_user("root");
    let someone_else_uid = gitquarry_core::storage::uid::Uid::new();
    let owner = gitquarry_core::storage::entities::OwnerRef::User(someone_else_uid);
    assert!(can_push_protected_decision(Some(&root), Some(&owner), None));
}

#[test]
fn user_repo_owner_can_push_protected() {
    let alice = test_user("alice", false);
    let bob = test_user("bob", false);
    let owner = gitquarry_core::storage::entities::OwnerRef::User(alice.uid.clone());
    assert!(can_push_protected_decision(Some(&alice), Some(&owner), None));
    assert!(!can_push_protected_decision(Some(&bob), Some(&owner), None));
}

#[test]
fn group_owner_can_push_protected_but_other_members_cant() {
    let alice = test_user("alice", false);
    let bob = test_user("bob", false);
    let acme = group_owned_by("acme", &alice);
    let owner = gitquarry_core::storage::entities::OwnerRef::Group(acme.uid.clone());
    assert!(can_push_protected_decision(Some(&alice), Some(&owner), Some(&acme)));
    assert!(!can_push_protected_decision(Some(&bob), Some(&owner), Some(&acme)));
}

#[test]
fn no_owner_or_no_actor_denies_by_default() {
    let alice = test_user("alice", false);
    assert!(!can_push_protected_decision(Some(&alice), None, None));
}
