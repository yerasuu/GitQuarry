//! The phase-0 dry run over real git history pushed into a real repo: what
//! each publishable ref would publish, and what every gate says. The cases
//! are the ones that leak in production when the filter is wrong.

use gitquarry_core::app::{AppContext, ResolvedRepo};
use gitquarry_mirror_external::publish::audit::{Finding, Gate, GateReport};
use gitquarry_mirror_external::publish::filter::{ExcludedBy, FilterConfig, FilterMode};
use gitquarry_mirror_external::publish::preview::{Preview, RefPreview, preview};
use gitquarry_mirror_external::publish::{IdentityMapping, PublishMirror, SecretAllow, UnmappedPolicy};
use gitquarry_test_support::{Work, ctx, git};

const MIT: &str = "SPDX-License-Identifier: MIT\n\nPermission is hereby granted...\n";

struct Fixture {
    _root: tempfile::TempDir,
    ctx: AppContext,
    repo: ResolvedRepo,
    work: Work,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path());
        let repo = ctx.create_repo("alice/private").await.unwrap().unwrap();
        Self {
            _root: root,
            ctx,
            repo,
            work: Work::new().await,
        }
    }

    async fn preview(&self, mirror: &PublishMirror) -> Preview {
        self.work.push(&self.repo, "refs/heads/main").await;
        preview(&self.ctx, &self.repo, mirror).await.unwrap()
    }

    async fn main(&self, mirror: &PublishMirror) -> RefPreview {
        let mut preview = self.preview(mirror).await;
        assert!(preview.host.passed(), "{:?}", preview.host);
        assert_eq!(preview.refs.len(), 1, "{:?}", preview.refs);
        let main = preview.refs.remove(0);
        assert_eq!(main.ref_name, "refs/heads/main");
        assert!(main.error.is_none(), "{:?}", main.error);
        main
    }
}

fn mirror(filter: FilterConfig) -> PublishMirror {
    PublishMirror {
        label: "public".to_string(),
        remote_url: "https://github.com/acme/public.git".to_string(),
        filter,
        publish_branches: ["main".to_string()].into(),
        unmapped: UnmappedPolicy::Passthrough,
        ..Default::default()
    }
}

fn excluding(patterns: &[&str]) -> FilterConfig {
    FilterConfig {
        mode: FilterMode::Exclude,
        exclude: patterns.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn gate(preview: &RefPreview, gate: Gate) -> &GateReport {
    preview.gates.iter().find(|g| g.gate == gate).unwrap()
}

#[tokio::test]
async fn a_secret_once_at_a_public_path_then_moved_under_an_excluded_dir_is_caught() {
    let fx = Fixture::new().await;
    fx.work
        .commit(
            &[
                ("LICENSE", MIT),
                ("src/main.rs", "fn main() {}\n"),
                ("prod.env", "DB=postgres://prod\n"),
            ],
            "start\n",
        )
        .await;
    std::fs::create_dir_all(fx.work.path().join("deploy")).unwrap();
    git(fx.work.path(), &["mv", "prod.env", "deploy/prod.env"]).await;
    fx.work.commit(&[], "move the env under deploy\n").await;

    let main = fx.main(&mirror(excluding(&["deploy/**"]))).await;

    assert_eq!(
        main.published_paths,
        ["LICENSE", "src/main.rs"],
        "the head itself looks clean"
    );
    let blob_set = gate(&main, Gate::BlobSet);
    assert!(
        blob_set.findings.iter().any(|f| matches!(f,
            Finding::BlobAlsoExcluded { published_path, excluded_path, .. }
                if published_path == "prod.env" && excluded_path == "deploy/prod.env")),
        "{blob_set:?}"
    );
    assert!(gate(&main, Gate::Filter).passed());
}

#[tokio::test]
async fn a_file_excluded_by_the_union_of_both_sources_is_not_published() {
    let fx = Fixture::new().await;
    fx.work
        .commit(
            &[
                ("LICENSE", MIT),
                ("src/main.rs", "fn main() {}\n"),
                ("src/local.env", "X=1\n"),
                ("deploy/run.sh", "echo\n"),
                (".gitquarry/publish.toml", "exclude = [\"*.env\"]\n"),
            ],
            "start\n",
        )
        .await;

    let main = fx.main(&mirror(excluding(&["deploy/"]))).await;

    assert_eq!(main.published_paths, ["LICENSE", "src/main.rs"]);
    assert_eq!(main.excluded.get(&ExcludedBy::Config("deploy/".to_string())), Some(&1));
    assert_eq!(main.excluded.get(&ExcludedBy::RepoFile("*.env".to_string())), Some(&1));
    assert_eq!(main.excluded.get(&ExcludedBy::FilterFile), Some(&1));
    assert!(main.gates.iter().all(GateReport::passed), "{:?}", main.gates);
}

#[tokio::test]
async fn an_in_repo_file_trying_to_remove_an_exclusion_blocks_and_changes_nothing() {
    let fx = Fixture::new().await;
    fx.work
        .commit(
            &[
                ("LICENSE", MIT),
                ("deploy/prod.env", "X=1\n"),
                (".gitquarry/publish.toml", "exclude = [\"!deploy/**\"]\n"),
            ],
            "start\n",
        )
        .await;

    let main = fx.main(&mirror(excluding(&["deploy/**"]))).await;

    assert_eq!(main.published_paths, ["LICENSE"]);
    let filter = gate(&main, Gate::Filter);
    assert!(
        filter
            .findings
            .iter()
            .any(|f| matches!(f, Finding::RepoFilterInvalid { .. })),
        "{filter:?}"
    );
}

#[tokio::test]
async fn refs_outside_heads_and_tags_are_never_previewed() {
    let fx = Fixture::new().await;
    let head = fx.work.commit(&[("LICENSE", MIT)], "start\n").await;
    for r in [
        "refs/merge-requests/1/head",
        "refs/notes/commits",
        "refs/gitquarry/internal",
    ] {
        git(fx.work.path(), &["update-ref", r, &head]).await;
    }
    git(fx.work.path(), &["tag", "v1.0", &head]).await;
    git(fx.work.path(), &["branch", "fix/acme-corp-outage", &head]).await;

    let mut m = mirror(excluding(&[]));
    m.publish_branches = ["main".to_string(), "release/*".to_string()].into();
    m.publish_tags = ["v*".to_string()].into();
    let preview = fx.preview(&m).await;

    let names: Vec<&str> = preview.refs.iter().map(|r| r.ref_name.as_str()).collect();
    assert_eq!(names, ["refs/heads/main", "refs/tags/v1.0"]);
}

#[tokio::test]
async fn a_submodule_gitlink_is_excluded_unless_the_mirror_allows_it() {
    let fx = Fixture::new().await;
    let other = fx.work.commit(&[("LICENSE", MIT)], "start\n").await;
    git(
        fx.work.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{other},vendor/lib"),
        ],
    )
    .await;
    git(fx.work.path(), &["commit", "-q", "-m", "add a submodule"]).await;

    let main = fx.main(&mirror(excluding(&[]))).await;
    assert_eq!(main.published_paths, ["LICENSE"]);
    assert_eq!(main.excluded.get(&ExcludedBy::Submodule), Some(&1));
    assert_eq!(main.collapsed_commits, 1, "the commit only added the gitlink");

    let mut keep = mirror(excluding(&[]));
    keep.filter.exclude_submodules = false;
    let main = fx.main(&keep).await;
    assert_eq!(main.published_paths, ["LICENSE", "vendor/lib"]);
}

#[tokio::test]
async fn allowlist_mode_keeps_a_new_top_level_dir_private() {
    let fx = Fixture::new().await;
    fx.work
        .commit(&[("LICENSE", MIT), ("src/lib.rs", "\n")], "start\n")
        .await;
    fx.work
        .commit(&[("customers/acme.csv", "acme,42\n")], "customer data\n")
        .await;

    let m = mirror(FilterConfig {
        mode: FilterMode::Allowlist,
        include: vec!["src/**".to_string(), "LICENSE".to_string()],
        ..Default::default()
    });
    let main = fx.main(&m).await;

    assert_eq!(main.published_paths, ["LICENSE", "src/lib.rs"]);
    assert_eq!(main.excluded.get(&ExcludedBy::NotInAllowlist), Some(&1));
    assert_eq!(main.collapsed_commits, 1);
    assert_eq!(main.published_commits, 1);
}

#[tokio::test]
async fn with_nothing_filtered_the_public_head_is_the_private_head() {
    // The ids are computed, never written; an identity rewrite reproducing
    // git's own ids byte for byte is what shows the encoding is right.
    let fx = Fixture::new().await;
    fx.work.commit(&[("LICENSE", MIT), ("a/b/c.txt", "c\n")], "one\n").await;
    git(fx.work.path(), &["checkout", "-q", "-b", "side"]).await;
    fx.work.commit(&[("side.txt", "s\n")], "side\n").await;
    git(fx.work.path(), &["checkout", "-q", "main"]).await;
    fx.work.commit(&[("main.txt", "m\n")], "main\n").await;
    git(fx.work.path(), &["merge", "-q", "--no-edit", "side"]).await;
    let head = git(fx.work.path(), &["rev-parse", "HEAD"]).await;

    let main = fx.main(&mirror(excluding(&[]))).await;

    assert_eq!(main.private_head, head);
    assert_eq!(main.public_head.as_deref(), Some(head.as_str()));
    assert_eq!(main.collapsed_commits, 0);
}

#[tokio::test]
async fn the_public_head_matches_what_git_would_build_from_the_filtered_tree() {
    let fx = Fixture::new().await;
    fx.work
        .commit(
            &[("LICENSE", MIT), ("src/a.rs", "a\n"), ("secret/key", "k\n")],
            "only\n",
        )
        .await;
    let dir = fx.work.path();
    let license = git(dir, &["rev-parse", "HEAD:LICENSE"]).await;
    let src = git(dir, &["rev-parse", "HEAD:src"]).await;
    let author = format!("@{}", git(dir, &["log", "-1", "--format=%ad", "--date=raw"]).await);
    let tree_input = format!("100644 blob {license}\tLICENSE\n040000 tree {src}\tsrc\n");
    let tree = pipe(dir, &["mktree"], &tree_input, &[]);
    let expected = pipe(
        dir,
        &["commit-tree", &tree],
        "only\n",
        &[("GIT_AUTHOR_DATE", &author), ("GIT_COMMITTER_DATE", &author)],
    );

    let main = fx.main(&mirror(excluding(&["secret/"]))).await;

    assert_eq!(main.public_head.as_deref(), Some(expected.as_str()));
}

#[tokio::test]
async fn unmapped_identities_block_and_a_mapping_rewrites_them() {
    let fx = Fixture::new().await;
    fx.work.commit(&[("LICENSE", MIT)], "start\n").await;
    let mut m = mirror(excluding(&[]));
    m.unmapped = UnmappedPolicy::Block;

    let main = fx.main(&m).await;
    let identity = gate(&main, Gate::Identity);
    assert!(
        identity
            .findings
            .iter()
            .any(|f| matches!(f, Finding::UnmappedIdentity { email, .. } if email == "test@example.com")),
        "{identity:?}"
    );

    m.identities = vec![IdentityMapping {
        private: "TEST@example.com".to_string(),
        public: "test@users.noreply.example.org".to_string(),
        name: Some("Test".to_string()),
    }];
    let mapped = fx.main(&m).await;
    assert!(gate(&mapped, Gate::Identity).passed());
    assert_ne!(mapped.public_head, Some(mapped.private_head.clone()));
}

#[tokio::test]
async fn license_gate_requires_a_license_and_the_declared_spdx_id() {
    let fx = Fixture::new().await;
    fx.work.commit(&[("README.md", "hi\n")], "start\n").await;
    let mut m = mirror(excluding(&[]));
    let main = fx.main(&m).await;
    assert_eq!(gate(&main, Gate::License).findings, [Finding::LicenseMissing]);

    fx.work.commit(&[("LICENSE", MIT)], "license\n").await;
    m.spdx = Some("Apache-2.0".to_string());
    let main = fx.main(&m).await;
    assert!(matches!(
        gate(&main, Gate::License).findings.as_slice(),
        [Finding::SpdxMismatch { found, .. }] if found == "MIT"
    ));

    m.spdx = Some("MIT".to_string());
    assert!(gate(&fx.main(&m).await, Gate::License).passed());
}

#[tokio::test]
async fn secret_scan_covers_content_and_messages_and_honours_the_allowlist() {
    let fx = Fixture::new().await;
    let key = "AKIAABCDEFGHIJKLMNOP";
    fx.work
        .commit(
            &[("LICENSE", MIT), ("config.py", &format!("aws = \"{key}\"\n"))],
            &format!("rotate {key}\n"),
        )
        .await;
    let commit = git(fx.work.path(), &["rev-parse", "HEAD"]).await;
    let blob = git(fx.work.path(), &["rev-parse", "HEAD:config.py"]).await;

    let mut m = mirror(excluding(&[]));
    let main = fx.main(&m).await;
    let secrets = gate(&main, Gate::Secrets);
    assert!(secrets.findings.contains(&Finding::Secret {
        path: "config.py".to_string(),
        sha: blob.clone(),
        rule: "aws-access-key-id",
        line: 1,
    }));
    assert!(secrets.findings.contains(&Finding::Secret {
        path: String::new(),
        sha: commit.clone(),
        rule: "aws-access-key-id",
        line: 1,
    }));

    m.secret_allowlist = [
        SecretAllow {
            path: "config.py".to_string(),
            sha: blob,
        },
        SecretAllow {
            path: String::new(),
            sha: commit,
        },
    ]
    .into();
    assert!(gate(&fx.main(&m).await, Gate::Secrets).passed());
}

#[tokio::test]
async fn the_host_gate_runs_again_at_preview_time() {
    let fx = Fixture::new().await;
    fx.work.commit(&[("LICENSE", MIT)], "start\n").await;
    let m = PublishMirror {
        remote_url: "https://forge.internal/acme/public.git".to_string(),
        ..mirror(excluding(&[]))
    };
    let preview = fx.preview(&m).await;
    assert!(
        matches!(preview.host.findings.as_slice(), [Finding::HostNotAllowed { .. }]),
        "{:?}",
        preview.host
    );
    assert!(!preview.passed());
}

/// `git args` in `dir` with `input` on stdin, for the plumbing the shared
/// `git` helper can't feed.
fn pipe(dir: &std::path::Path, args: &[&str], input: &str, env: &[(&str, &str)]) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}
