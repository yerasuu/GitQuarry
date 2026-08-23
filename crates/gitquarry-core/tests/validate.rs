use gitquarry_core::validate::*;

#[test]
fn strips_leading_slash_and_git_suffix() {
    assert_eq!(sanitize_path("/projectA").unwrap(), "projectA");
    assert_eq!(sanitize_path("team/projectB.git").unwrap(), "team/projectB");
    assert_eq!(sanitize_path("a//b/./c").unwrap(), "a/b/c");
}

#[test]
fn rejects_traversal_and_empty() {
    assert!(sanitize_path("../../etc/passwd").is_err());
    assert!(sanitize_path("a/../../b").is_err());
    assert!(sanitize_path("/").is_err());
    assert!(sanitize_path("").is_err());
    assert!(sanitize_path(".git").is_err());
    assert!(sanitize_path("-x/repo").is_err());
    assert!(sanitize_path("re\npo").is_err());
}

#[test]
fn rejects_unescaped_url_characters() {
    assert!(sanitize_path("my repo").is_err());
    assert!(sanitize_path("team/repo name").is_err());
    assert!(sanitize_path("repo?query").is_err());
    assert!(sanitize_path("repo#frag").is_err());
    assert!(sanitize_path("repo%20name").is_err());
    assert_eq!(sanitize_path("my-repo_v1.0").unwrap(), "my-repo_v1.0");
}

#[test]
fn repo_display_name_accepts_any_charset() {
    assert_eq!(validate_repo_display_name(" My Cool Repo! ").unwrap(), "My Cool Repo!");
    assert_eq!(
        validate_repo_display_name("日本語リポジトリ").unwrap(),
        "日本語リポジトリ"
    );
}

#[test]
fn repo_display_name_rejects_empty_too_long_and_control_chars() {
    assert!(validate_repo_display_name("").is_err());
    assert!(validate_repo_display_name("   ").is_err());
    assert!(validate_repo_display_name(&"x".repeat(300)).is_err());
    assert!(validate_repo_display_name("bad\nname").is_err());
}

#[test]
fn rev_rejects_option_injection() {
    assert!(validate_rev("main").is_ok());
    assert!(validate_rev("refs/heads/main").is_ok());
    assert!(validate_rev("HEAD~2^1").is_ok());
    assert!(validate_rev("--output=/tmp/pwn").is_err());
    assert!(validate_rev("main..dev").is_err());
    assert!(validate_rev("a b").is_err());
    assert!(validate_rev("").is_err());
}

#[test]
fn sha_must_be_full_hex() {
    assert!(validate_sha(&"a".repeat(40)).is_ok());
    assert!(validate_sha(&"0".repeat(64)).is_ok());
    assert!(validate_sha("deadbeef").is_err());
    assert!(validate_sha("--output=x").is_err());
}

#[test]
fn lfs_oid_must_be_sha256_only() {
    assert!(validate_lfs_oid(&"a".repeat(64)).is_ok());
    assert!(validate_lfs_oid(&"a".repeat(40)).is_err()); // sha1-length, rejected for LFS
    assert!(validate_lfs_oid("deadbeef").is_err());
    assert!(validate_lfs_oid("--output=x").is_err());
}

#[test]
fn asset_filename_accepts_a_plain_name_and_trims_whitespace() {
    assert_eq!(validate_asset_filename(" build.tar.gz ").unwrap(), "build.tar.gz");
}

#[test]
fn asset_filename_rejects_separators_dots_and_control_chars() {
    assert!(validate_asset_filename("").is_err());
    assert!(validate_asset_filename(".").is_err());
    assert!(validate_asset_filename("..").is_err());
    assert!(validate_asset_filename("a/b").is_err());
    assert!(validate_asset_filename("a\\b").is_err());
    assert!(validate_asset_filename("a\"b").is_err());
    assert!(validate_asset_filename("a\nb").is_err());
    assert!(validate_asset_filename(&"x".repeat(300)).is_err());
}

#[test]
fn tree_path_rejects_traversal() {
    assert!(validate_tree_path("").is_ok());
    assert!(validate_tree_path("src/main.rs").is_ok());
    assert!(validate_tree_path("../secret").is_err());
    assert!(validate_tree_path("-x").is_err());
    assert!(validate_tree_path("/abs").is_err());
}

#[test]
fn query_rejects_empty_leading_dash_and_control_chars() {
    assert!(validate_query("fix bug").is_ok());
    assert!(validate_query("").is_err());
    assert!(validate_query("-i").is_err());
    assert!(validate_query("bad\nquery").is_err());
}

#[test]
fn short_sha_never_panics_on_a_multibyte_string() {
    // Not a sha at all — but a display helper that panics is a request
    // taken down, so it has to degrade instead.
    assert_eq!(short_sha("abcdef0123456789"), "abcdef012345");
    assert_eq!(short_sha("abc"), "abc");
    assert_eq!(short_sha(""), "");
    // One ASCII byte then three 4-byte characters: byte 12 lands inside
    // the last one, which is the case that used to panic.
    let mixed = "a\u{1F600}\u{1F600}\u{1F600}";
    assert_eq!(short_sha(mixed), mixed);
}
