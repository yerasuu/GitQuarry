//! The publish filter on its own: `.gitignore` glob semantics, the two
//! modes, and the union of the mirror's config with the in-repo file — which
//! may add exclusions and must never be able to remove one.

use gitquarry_mirror_external::publish::filter::{
    Decision, EntryKind, ExcludedBy, Filter, FilterConfig, FilterMode, RepoFilterFile,
};

fn exclude_mode(exclude: &[&str]) -> FilterConfig {
    FilterConfig {
        mode: FilterMode::Exclude,
        exclude: exclude.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn excluded(filter: &Filter, path: &str, kind: EntryKind) -> Option<ExcludedBy> {
    match filter.decide(path, kind, false) {
        Decision::Exclude(by) => Some(by),
        Decision::Keep { .. } => None,
    }
}

#[test]
fn gitignore_semantics_against_the_full_path() {
    let filter = Filter::new(
        &exclude_mode(&["*.env", "/build", "docs/internal/", "fixtures/**/secret-*", "deploy/**"]),
        None,
    );
    // No slash: the basename, at any depth.
    assert!(excluded(&filter, "prod.env", EntryKind::File).is_some());
    assert!(excluded(&filter, "a/b/prod.env", EntryKind::File).is_some());
    assert!(excluded(&filter, "a/prod.env.example", EntryKind::File).is_none());
    // A leading slash anchors to the root.
    assert!(excluded(&filter, "build", EntryKind::Dir).is_some());
    assert!(excluded(&filter, "src/build", EntryKind::Dir).is_none());
    // A trailing slash matches directories only.
    assert!(excluded(&filter, "docs/internal", EntryKind::Dir).is_some());
    assert!(excluded(&filter, "docs/internal", EntryKind::File).is_none());
    // `**` spans any number of directories, including none.
    assert!(excluded(&filter, "fixtures/secret-a", EntryKind::File).is_some());
    assert!(excluded(&filter, "fixtures/x/y/secret-a", EntryKind::File).is_some());
    // `dir/**` takes the directory whole, so it is pruned before its
    // children are read.
    assert_eq!(
        excluded(&filter, "deploy", EntryKind::Dir),
        Some(ExcludedBy::Config("deploy/**".to_string()))
    );
    assert!(excluded(&filter, "src/deploy", EntryKind::Dir).is_none());
}

#[test]
fn character_classes_and_single_character_wildcards() {
    let filter = Filter::new(&exclude_mode(&["key[0-9].pem", "?.tmp", "[!a]x"]), None);
    assert!(excluded(&filter, "key7.pem", EntryKind::File).is_some());
    assert!(excluded(&filter, "keyx.pem", EntryKind::File).is_none());
    assert!(excluded(&filter, "a.tmp", EntryKind::File).is_some());
    assert!(excluded(&filter, "ab.tmp", EntryKind::File).is_none());
    assert!(excluded(&filter, "bx", EntryKind::File).is_some());
    assert!(excluded(&filter, "ax", EntryKind::File).is_none());
}

#[test]
fn a_file_excluded_by_either_source_is_excluded_by_the_union() {
    let config = exclude_mode(&["deploy/**"]);
    let file = RepoFilterFile::parse(b"exclude = [\"*.env\"]\n").unwrap();
    let filter = Filter::new(&config, Some(&file));
    assert_eq!(
        excluded(&filter, "deploy", EntryKind::Dir),
        Some(ExcludedBy::Config("deploy/**".to_string()))
    );
    assert_eq!(
        excluded(&filter, "src/prod.env", EntryKind::File),
        Some(ExcludedBy::RepoFile("*.env".to_string()))
    );
    assert!(excluded(&filter, "src/main.rs", EntryKind::File).is_none());
}

#[test]
fn the_in_repo_file_cannot_remove_an_exclusion() {
    // Negation, an include list, a mode switch: every way the file could
    // try to widen what is published is refused outright.
    for attempt in [
        "exclude = [\"!deploy/**\"]\n",
        "include = [\"deploy/**\"]\n",
        "mode = \"exclude\"\n",
        "publish_branches = [\"*\"]\n",
    ] {
        assert!(RepoFilterFile::parse(attempt.as_bytes()).is_err(), "{attempt} accepted");
    }
    // And a `false` in the file can't switch off a rule the config turned on.
    let file = RepoFilterFile::parse(b"exclude_submodules = false\nstrip_empty_dirs = false\n").unwrap();
    let filter = Filter::new(&exclude_mode(&[]), Some(&file));
    assert_eq!(
        excluded(&filter, "vendor/lib", EntryKind::Gitlink),
        Some(ExcludedBy::Submodule)
    );
    assert!(filter.strip_empty_dirs());
}

#[test]
fn the_filter_file_itself_is_never_published() {
    let filter = Filter::new(&exclude_mode(&[]), None);
    assert_eq!(
        excluded(&filter, ".gitquarry/publish.toml", EntryKind::File),
        Some(ExcludedBy::FilterFile)
    );
}

#[test]
fn allowlist_keeps_a_new_top_level_dir_private() {
    let config = FilterConfig {
        mode: FilterMode::Allowlist,
        // Anchored: like `.gitignore`, a bare `LICENSE` would also name
        // `customers/LICENSE`, so the top level couldn't be pruned.
        include: vec!["/src/".to_string(), "/LICENSE".to_string(), "docs/*.md".to_string()],
        exclude: vec!["*.env".to_string()],
        ..Default::default()
    };
    let filter = Filter::new(&config, None);
    assert_eq!(
        excluded(&filter, "customers", EntryKind::Dir),
        Some(ExcludedBy::NotInAllowlist),
        "an unlisted directory is pruned, not walked"
    );
    assert_eq!(
        filter.decide("src", EntryKind::Dir, false),
        Decision::Keep { included: true }
    );
    assert_eq!(
        filter.decide("src/a.rs", EntryKind::File, true),
        Decision::Keep { included: true }
    );
    // Exclusions still win inside an allowlisted directory.
    assert!(matches!(
        filter.decide("src/prod.env", EntryKind::File, true),
        Decision::Exclude(ExcludedBy::Config(_))
    ));
    // `docs` itself isn't listed, but something under it could be.
    assert_eq!(
        filter.decide("docs", EntryKind::Dir, false),
        Decision::Keep { included: false }
    );
    assert!(excluded(&filter, "docs/guide.md", EntryKind::File).is_none());
    assert!(excluded(&filter, "docs/notes.txt", EntryKind::File).is_some());
    assert!(excluded(&filter, "README.md", EntryKind::File).is_some());
}
