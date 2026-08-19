use gitquarry_core::git::codeowners::*;

#[test]
fn parse_skips_comments_and_blank_lines() {
    let rules = parse(
        "\
# top comment
*.rs @alice

docs/ @bob @carol
",
    );
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].pattern, "*.rs");
    assert_eq!(rules[0].owners, vec!["alice".to_string()]);
    assert_eq!(rules[1].pattern, "docs/");
    assert_eq!(rules[1].owners, vec!["bob".to_string(), "carol".to_string()]);
}

#[test]
fn parse_strips_leading_at_from_owners() {
    let rules = parse("*.md @alice");
    assert_eq!(rules[0].owners, vec!["alice".to_string()]);
}

#[test]
fn parse_skips_a_line_with_no_owners() {
    let rules = parse("*.rs\ndocs/ @alice");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].pattern, "docs/");
}

#[test]
fn exact_path_matches_only_itself() {
    let rules = parse("src/lib.rs @alice");
    assert_eq!(owners_for_path(&rules, "src/lib.rs"), ["alice"]);
    assert!(owners_for_path(&rules, "src/main.rs").is_empty());
}

#[test]
fn directory_pattern_matches_everything_under_it() {
    let rules = parse("docs/ @alice");
    assert_eq!(owners_for_path(&rules, "docs/readme.md"), ["alice"]);
    assert_eq!(owners_for_path(&rules, "docs/sub/deep.md"), ["alice"]);
    assert_eq!(owners_for_path(&rules, "docs"), ["alice"]);
    assert!(owners_for_path(&rules, "src/docs/x.md").is_empty());
}

#[test]
fn bare_glob_matches_repo_wide_by_basename() {
    let rules = parse("*.rs @alice");
    assert_eq!(owners_for_path(&rules, "lib.rs"), ["alice"]);
    assert_eq!(owners_for_path(&rules, "src/deep/lib.rs"), ["alice"]);
    assert!(owners_for_path(&rules, "lib.txt").is_empty());
}

#[test]
fn single_star_never_crosses_a_path_segment() {
    let rules = parse("src/*.rs @alice");
    assert_eq!(owners_for_path(&rules, "src/lib.rs"), ["alice"]);
    assert!(owners_for_path(&rules, "src/deep/lib.rs").is_empty());
}

#[test]
fn double_star_matches_any_depth() {
    let rules = parse("**/*.rs @alice");
    assert_eq!(owners_for_path(&rules, "lib.rs"), ["alice"]);
    assert_eq!(owners_for_path(&rules, "src/lib.rs"), ["alice"]);
    assert_eq!(owners_for_path(&rules, "src/deep/lib.rs"), ["alice"]);
    assert!(owners_for_path(&rules, "src/lib.txt").is_empty());
}

#[test]
fn last_matching_rule_wins_not_a_union() {
    let rules = parse(
        "\
* @default-owner
docs/ @docs-owner
docs/secret.md @secret-owner
",
    );
    assert_eq!(owners_for_path(&rules, "src/lib.rs"), ["default-owner"]);
    assert_eq!(owners_for_path(&rules, "docs/readme.md"), ["docs-owner"]);
    assert_eq!(owners_for_path(&rules, "docs/secret.md"), ["secret-owner"]);
}

#[test]
fn no_matching_rule_returns_empty() {
    let rules = parse("docs/ @alice");
    assert!(owners_for_path(&rules, "src/lib.rs").is_empty());
}

#[test]
fn empty_codeowners_matches_nothing() {
    let rules = parse("");
    assert!(rules.is_empty());
    assert!(owners_for_path(&rules, "anything").is_empty());
}
