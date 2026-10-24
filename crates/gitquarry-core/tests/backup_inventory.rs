//! Two kinds of test here.
//!
//! The first kind is ordinary: a representative key per class, asserted to
//! classify the way `backup::inventory`'s doc comments claim.
//!
//! The second kind (`every_declared_storage_prefix_is_classified`) is the
//! one that matters over time. It walks the workspace's own source, extracts
//! every storage prefix the code declares (`#[entity(collection = ...)]`,
//! `Collection::new*`, `Singleton::new`, `SecondaryIndex::new`) and asserts
//! each one classifies as something other than `Class::Unknown`. Without it,
//! adding a collection anywhere in the tree silently produces data no backup
//! carries — a failure nothing else in the suite can see, because the new
//! collection works perfectly well on its own.

use gitquarry_core::backup::inventory::{Class, Selection, classify};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[test]
fn git_data_classifies_as_git() {
    assert_eq!(classify("repos/abc123/refs.json"), Class::Git);
    assert_eq!(classify("repos/abc123/objects/pack/pack-deadbeef.pack"), Class::Git);
    assert_eq!(classify("repos/abc123/objects/pack/pack-deadbeef.idx"), Class::Git);
}

#[test]
fn lfs_objects_and_locks_are_different_classes() {
    assert_eq!(classify("repos/abc123/lfs/0123abcd"), Class::Lfs);
    // `lfs-locks/` is live state, not a content-addressed blob -- it must
    // not ride along with the LFS toggle.
    assert_eq!(classify("repos/abc123/lfs-locks/some-lock.json"), Class::Entities);
    assert_eq!(classify("repos/abc123/lfs-lock-paths/YS5wc2Q.json"), Class::Entities);
}

#[test]
fn releases_and_their_assets_classify_as_releases() {
    assert_eq!(classify("repos/abc123/releases/v1.0.0/meta.json"), Class::Releases);
    assert_eq!(
        classify("repos/abc123/releases/v1.0.0/assets/binary.tar.gz"),
        Class::Releases
    );
}

#[test]
fn per_repo_entities_classify_as_entities() {
    assert_eq!(classify("repos/abc123/.conf/config.json"), Class::Entities);
    assert_eq!(classify("repos/abc123/merges/1/comments/x.json"), Class::Entities);
    assert_eq!(classify("repos/abc123/mirrors/m1/mr-links/x.json"), Class::Entities);
    assert_eq!(classify("repos/abc123/publish-mirrors/m1.json"), Class::Entities);
}

#[test]
fn search_index_classifies_as_derived() {
    assert_eq!(classify("repos/abc123/search-index/x.json"), Class::Derived);
}

#[test]
fn avatars_are_their_own_class() {
    assert_eq!(classify("avatars/alice"), Class::Avatars);
}

#[test]
fn encryption_config_classifies_as_bootstrap() {
    assert_eq!(classify(".conf/encryption.json"), Class::Bootstrap);
}

#[test]
fn instance_secrets_and_settings_classify_as_entities() {
    assert_eq!(classify(".conf/global.json"), Class::Entities);
    assert_eq!(classify(".conf/ssh_host_key.json"), Class::Entities);
    assert_eq!(classify(".conf/signing_key.json"), Class::Entities);
    assert_eq!(classify(".conf/users/alice.json"), Class::Entities);
    assert_eq!(classify(".conf/groups/team.json"), Class::Entities);
    assert_eq!(classify(".conf/pat/tok.json"), Class::Entities);
}

#[test]
fn audit_is_its_own_class() {
    assert_eq!(classify(".conf/audit/2026-08/x.json"), Class::Audit);
}

#[test]
fn only_backfillable_indexes_classify_as_derived() {
    // These four are exactly what `gitquarry backfill-repo-index` rebuilds.
    assert_eq!(classify(".conf/index/repo-name/x.json"), Class::Derived);
    assert_eq!(classify(".conf/index/repo-owner/user/uid/x.json"), Class::Derived);
    assert_eq!(classify(".conf/index/repo-visible/x.json"), Class::Derived);
    assert_eq!(classify(".conf/index/repo-permission/uid/x.json"), Class::Derived);
    // These have no rebuild path at all: an instance restored without them
    // can't authenticate anyone, so they're carried regardless of the
    // derived toggle.
    assert_eq!(classify(".conf/index/pubkey/x.json"), Class::Entities);
    assert_eq!(classify(".conf/index/namespace/alice.json"), Class::Entities);
    assert_eq!(classify(".conf/index/secondary-email/x.json"), Class::Entities);
    assert_eq!(classify(".conf/index/pat-owner/uid/x.json"), Class::Entities);
    assert_eq!(classify(".conf/index/mirrors-enabled/x.json"), Class::Entities);
    assert_eq!(classify(".conf/index/repo-deleted/x.json"), Class::Entities);
    assert_eq!(classify(".conf/index/password-login-count.json"), Class::Entities);
    assert_eq!(classify(".conf/index/signing-epoch.json"), Class::Entities);
}

#[test]
fn derive_minted_index_siblings_classify_as_entities() {
    assert_eq!(classify(".conf/users.index/uid/abc.json"), Class::Entities);
}

#[test]
fn commit_signature_cache_classifies_as_derived() {
    assert_eq!(classify(".conf/commit-sig/deadbeef.json"), Class::Derived);
}

#[test]
fn transient_and_self_referential_keys_are_never_copied() {
    assert_eq!(classify(".conf/txn/t1.json"), Class::Never);
    assert_eq!(classify(".conf/wal/w1.json"), Class::Never);
    assert_eq!(classify(".conf/snapshots/s1.json"), Class::Never);
    assert_eq!(classify(".conf/backups/run1/repos/x.json"), Class::Never);
    assert_eq!(classify(".conf/backup-runs/r1.json"), Class::Never);
    // Console-created folder placeholder.
    assert_eq!(classify("repos/abc123/objects/pack/"), Class::Never);
}

#[test]
fn unrecognized_keys_classify_as_unknown_and_are_still_copied() {
    assert_eq!(classify("something-new/x.json"), Class::Unknown);
    assert_eq!(classify(".conf/brand-new-feature/x.json"), Class::Unknown);
    assert_eq!(classify("repos/abc123/brand-new-feature/x.json"), Class::Unknown);
    assert!(Selection::default().includes(Class::Unknown));
}

#[test]
fn bootstrap_is_included_regardless_of_selection() {
    let nothing = Selection {
        git: false,
        lfs: false,
        releases: false,
        avatars: false,
        entities: false,
        audit: false,
        derived: false,
    };
    assert!(nothing.includes(Class::Bootstrap));
    assert!(!nothing.includes(Class::Never));
}

#[test]
fn default_selection_skips_only_derived() {
    let s = Selection::default();
    assert!(s.git && s.lfs && s.releases && s.avatars && s.entities && s.audit);
    assert!(!s.derived);
}

#[test]
fn minimal_selection_also_skips_audit() {
    let s = Selection::minimal();
    assert!(s.git && s.lfs && s.releases && s.avatars && s.entities);
    assert!(!s.audit && !s.derived);
}

// ---------------------------------------------------------------------------
// The drift guard.
// ---------------------------------------------------------------------------

/// Placeholders that appear inside a `format!`-built prefix, mapped to a
/// sample value of the right shape. A placeholder that isn't in here fails
/// the test by name, which is the point: it means someone introduced a new
/// runtime-scoped prefix and this file is where they say what class it is.
fn placeholder_samples() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("REPOS_PREFIX", "repos"),
        ("INDEX_PREFIX", ".conf/index"),
        ("AUDIT_ROOT", ".conf/audit"),
        ("uid", "sampleuid"),
        ("repo_uid", "sampleuid"),
        ("owner_uid", "sampleuid"),
        ("owner", "sampleowner"),
        ("parent_uid", "sampleuid"),
        ("user_uid", "sampleuid"),
        ("mirror_id", "samplemirror"),
        ("number", "1"),
        ("month", "2026-08"),
        ("kind", "user"),
    ])
}

#[test]
fn every_declared_storage_prefix_is_classified() {
    let root = workspace_root();
    let mut sources = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates")).expect("reading crates/") {
        let src = crate_dir.expect("crate dir entry").path().join("src");
        if src.is_dir() {
            collect_rs_files(&src, &mut sources);
        }
    }
    assert!(!sources.is_empty(), "found no source files to scan under crates/*/src");

    let mut unclassified = Vec::new();
    let mut checked = 0usize;
    for path in &sources {
        let text = std::fs::read_to_string(path).expect("reading source file");
        for declared in declared_prefixes(&text, path) {
            checked += 1;
            if declared.samples().iter().all(|key| classify(key) == Class::Unknown) {
                unclassified.push(format!("{}: {}", path.display(), declared.raw));
            }
        }
    }

    assert!(
        checked > 10,
        "prefix extraction found only {checked} prefixes; the scan is probably broken, not the tree"
    );
    assert!(
        unclassified.is_empty(),
        "these declared storage prefixes classify as Class::Unknown — a backup would carry them \
         as unclassified data instead of honoring their real class. Classify them in \
         backup::inventory:\n  {}",
        unclassified.join("\n  ")
    );
}

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<root>/crates/gitquarry-core`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root above crates/gitquarry-core")
        .to_path_buf()
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("reading source dir") {
        let path = entry.expect("source dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// One declaration found in the source, resolved to concrete keys `classify`
/// can be asked about.
struct Declared {
    /// What the source literally said, for the failure message.
    raw: String,
    /// `true` for a [`s3db::Singleton`], whose literal is a whole object key
    /// rather than a prefix to hang names off.
    full_key: bool,
    /// An `#[entity(suffix = "...")]` / `Collection::new_with_suffix` tail
    /// (e.g. `/.conf/config.json`), which decides where under the prefix the
    /// object actually sits.
    suffix: Option<String>,
}

impl Declared {
    /// Keys to try. A plain collection's objects sit directly under its
    /// prefix; a collection declared with a `suffix` (e.g. `RepoConfig`,
    /// whose objects live at `repos/<uid>/.conf/config.json`) puts one more
    /// segment in between, so both shapes are offered and the declaration
    /// counts as classified if *either* lands somewhere real. A genuinely
    /// new prefix fails both, which is the case this guard exists for.
    fn samples(&self) -> Vec<String> {
        if self.full_key {
            return vec![self.raw.clone()];
        }
        if let Some(suffix) = &self.suffix {
            return vec![format!("{}/sampleuid{suffix}", self.raw)];
        }
        vec![
            format!("{}/sample.json", self.raw),
            format!("{}/sampleuid/sample.json", self.raw),
        ]
    }
}

/// Every storage prefix `text` declares, as concrete keys with placeholders
/// substituted. Panics (fails the test) on a construct it can recognize but
/// not resolve — silently skipping one would defeat the whole guard.
fn declared_prefixes(text: &str, path: &Path) -> Vec<Declared> {
    let mut found = Vec::new();

    for needle in [
        "#[entity(collection = ",
        "Collection::new(",
        "Collection::new_with_suffix(",
        "Collection::new_owned(format!(",
    ] {
        for (raw, args) in literals_after(text, needle) {
            let suffix = suffix_from_args(&args);
            found.push((raw, args, false, suffix));
        }
    }
    for (raw, args) in literals_after(text, "Singleton::new(") {
        found.push((raw, args, true, None));
    }
    // `SecondaryIndex::new(PREFIX, "name", Segment::X)` -- the literal that
    // comes first is the index's own name; every existing call passes
    // `INDEX_PREFIX` (in some `super::`-qualified form) as the prefix, so
    // anything else has to be handled explicitly rather than guessed at.
    for call in text.split("SecondaryIndex::new(").skip(1) {
        let head = call.split(')').next().unwrap_or("");
        assert!(
            head.contains("INDEX_PREFIX"),
            "{}: SecondaryIndex::new with an unrecognized prefix argument ({head}); teach \
             backup_inventory's scan what prefix it uses",
            path.display()
        );
        let name = head
            .split('"')
            .nth(1)
            .unwrap_or_else(|| panic!("{}: SecondaryIndex::new with no name literal", path.display()));
        found.push((format!(".conf/index/{name}"), Vec::new(), false, None));
    }

    found
        .into_iter()
        // `"..."` is prose: several module docs mention
        // `#[entity(collection = "...")]` while explaining why they don't
        // use it. Nothing real ever declares an ellipsis prefix.
        .filter(|(raw, _, _, _)| !raw.is_empty() && !raw.contains("..."))
        .map(|(raw, args, full_key, suffix)| Declared {
            raw: resolve_placeholders(&raw, &args, path),
            full_key,
            suffix,
        })
        .collect()
}

/// An explicit object-name suffix, either `#[entity(suffix = "...")]` or
/// `Collection::new_with_suffix`'s second argument.
fn suffix_from_args(args: &[String]) -> Option<String> {
    for arg in args {
        if let Some(rest) = arg.strip_prefix("suffix = ") {
            return Some(rest.trim_matches('"').to_string());
        }
        if arg.starts_with('"') && arg.ends_with('"') && arg.len() > 1 {
            return Some(arg.trim_matches('"').to_string());
        }
    }
    None
}

/// The first string literal after each occurrence of `needle`, paired with
/// the identifiers of whatever arguments follow it up to the call's closing
/// paren — a `format!`'s positional `{}` holes are filled from those, in
/// order.
fn literals_after(text: &str, needle: &str) -> Vec<(String, Vec<String>)> {
    text.split(needle)
        .skip(1)
        .filter_map(|rest| {
            let after_quote = rest.strip_prefix('"')?;
            let (literal, tail) = after_quote.split_once('"')?;
            let args = tail
                .split(')')
                .next()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|arg| !arg.is_empty())
                // `super::INDEX_PREFIX` / `crate::storage::REPOS_PREFIX` ->
                // the bare const name, which is what `placeholder_samples`
                // is keyed by.
                .map(|arg| arg.rsplit("::").next().unwrap_or(arg).to_string())
                .collect();
            Some((literal.to_string(), args))
        })
        .collect()
}

fn resolve_placeholders(raw: &str, args: &[String], path: &Path) -> String {
    let samples = placeholder_samples();
    let mut positional = args.iter();
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .unwrap_or_else(|| panic!("{}: unbalanced '{{' in prefix literal {raw}", path.display()));
        // An empty hole (`{}`) takes the next positional argument; a named
        // one (`{uid}`) is its own key.
        let name = match &after[..close] {
            "" => positional.next().map(String::as_str).unwrap_or_else(|| {
                panic!(
                    "{}: prefix literal {raw} has more '{{}}' holes than the call has arguments",
                    path.display()
                )
            }),
            named => named,
        };
        let sample = samples.get(name).unwrap_or_else(|| {
            panic!(
                "{}: prefix literal {raw} interpolates unknown placeholder '{name}'; add a \
                 sample value for it in backup_inventory's placeholder_samples()",
                path.display()
            )
        });
        out.push_str(sample);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out.trim_end_matches('/').to_string()
}

#[test]
fn a_selection_from_untouched_settings_matches_the_default() {
    let settings = gitquarry_core::storage::entities::ServerSettings::default();
    assert_eq!(Selection::from_settings(&settings), Selection::default());
}

#[test]
fn settings_opt_outs_are_honored_one_class_at_a_time() {
    let settings = gitquarry_core::storage::entities::ServerSettings {
        backup_include_lfs: Some(false),
        backup_include_derived: Some(true),
        ..Default::default()
    };
    let selection = Selection::from_settings(&settings);
    assert!(!selection.lfs);
    assert!(selection.derived);
    // Everything untouched still follows the default.
    assert!(selection.git && selection.releases && selection.avatars && selection.entities && selection.audit);
}
