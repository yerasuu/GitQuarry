//! `gitquarry pre-receive-hook` run as the real binary, the way `git
//! receive-pack` runs it on every push to a mirror: env vars naming the
//! bucket/repo/actor, `<old> <new> <ref>` lines on stdin, and the exit code
//! deciding whether git accepts the push. What is asserted is what a pusher
//! gets: which protected-ref updates are refused, and whether `refs.json`
//! in the bucket moved.

use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::{Group, Permission, User};
use gitquarry_core::config::{LocalEngine, LocalParams};
use gitquarry_core::crypto::{Cipher, bootstrap};
use gitquarry_core::storage::Backend;
use gitquarry_core::storage::entities::{OwnerRef, RepoConfig};
use gitquarry_core::storage::remote::{Manifest, Remote};
use gitquarry_core::storage::settings;
use gitquarry_core::storage::uid::Uid;
use s3db::LocalBackend;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tempfile::TempDir;

const PASSPHRASE: &str = "hunter2";

enum Owner {
    Alice,
    /// Group `acme`: alice is an `Owner` member, bob only a `Developer`.
    Acme,
    Nobody,
}

/// A repo whose `main` branch and `v1`/`v2` tags are protected, seeded with
/// `main`, `dev` and `v1` all at `base`. `child` descends from `base`;
/// `unrelated` shares no history with it, so moving a ref there is a
/// force-push.
struct Fixture {
    dir: TempDir,
    git_dir: PathBuf,
    repo_uid: Uid,
    base: String,
    child: String,
    unrelated: String,
}

fn git(git_dir: &Path, args: &[&str], stdin: &str) -> String {
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn backend(bucket: &Path) -> Arc<dyn Backend> {
    Arc::new(LocalBackend::new(&LocalParams {
        bucket: bucket.to_string_lossy().to_string(),
        engine: LocalEngine::Redb,
    }))
}

async fn put_user(backend: &dyn Backend, cipher: &Cipher, name: &str, is_admin: bool) -> User {
    let user = User {
        uid: Uid::new(),
        username: name.to_string(),
        is_admin,
        ..Default::default()
    };
    access::put_user(backend, cipher, &user, None).await.unwrap();
    user
}

impl Fixture {
    async fn new(object_format: &str, owner: Owner) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join("mirror.git");
        let out = Command::new("git")
            .args(["init", "--quiet", "--bare", &format!("--object-format={object_format}")])
            .arg(&git_dir)
            .status()
            .unwrap();
        assert!(out.success());
        let tree = git(&git_dir, &["mktree"], "");
        let base = git(&git_dir, &["commit-tree", "--no-gpg-sign", "-m", "base", &tree], "");
        let child = git(
            &git_dir,
            &["commit-tree", "--no-gpg-sign", "-p", &base, "-m", "child", &tree],
            "",
        );
        let unrelated = git(
            &git_dir,
            &["commit-tree", "--no-gpg-sign", "-m", "unrelated", &tree],
            "",
        );

        let bucket = dir.path().join("bucket");
        std::fs::create_dir(&bucket).unwrap();
        std::fs::write(
            dir.path().join("gitquarry.toml"),
            format!(
                "[buckets.main]\ndriver = \"local\"\nengine = \"redb\"\nbucket = {:?}\npassphrase = {PASSPHRASE:?}\n",
                bucket.to_string_lossy()
            ),
        )
        .unwrap();

        let repo_uid = Uid::new();
        {
            // Dropped before the binary runs: redb holds an exclusive lock.
            let backend = backend(&bucket);
            let cipher = Arc::new(
                bootstrap::get_or_create_cipher(backend.as_ref(), PASSPHRASE)
                    .await
                    .unwrap(),
            );
            let alice = put_user(backend.as_ref(), &cipher, "alice", false).await;
            let bob = put_user(backend.as_ref(), &cipher, "bob", false).await;
            put_user(backend.as_ref(), &cipher, "root", true).await;
            let acme = Group {
                uid: Uid::new(),
                name: "acme".to_string(),
                members: BTreeMap::from([(alice.uid.clone(), Permission::Owner), (bob.uid, Permission::Developer)]),
                ..Default::default()
            };
            access::put_group(backend.as_ref(), &cipher, &acme, None).await.unwrap();
            // The hook finds the owning group by uid, the index
            // `domain::group::create` claims right after the group itself.
            assert!(acme.claim_uid(backend.as_ref(), &cipher).await.unwrap());

            let config = RepoConfig {
                uid: repo_uid.clone(),
                full_name: "acme/widgets".to_string(),
                owner: match owner {
                    Owner::Alice => Some(OwnerRef::User(alice.uid)),
                    Owner::Acme => Some(OwnerRef::Group(acme.uid)),
                    Owner::Nobody => None,
                },
                protected_branches: BTreeSet::from(["main".to_string()]),
                protected_tags: BTreeSet::from(["v1".to_string(), "v2".to_string()]),
                ..Default::default()
            };
            settings::put_repo_config(backend.as_ref(), repo_uid.as_str(), &config, None, &cipher)
                .await
                .unwrap();
            let manifest = Manifest {
                refs: ["refs/heads/main", "refs/heads/dev", "refs/tags/v1"]
                    .into_iter()
                    .map(|r| (r.to_string(), base.clone()))
                    .collect(),
                head: Some("refs/heads/main".to_string()),
            };
            Remote::new(backend, format!("repos/{repo_uid}"), cipher)
                .put_manifest(&manifest, None)
                .await
                .unwrap();
        }

        Fixture {
            dir,
            git_dir,
            repo_uid,
            base,
            child,
            unrelated,
        }
    }

    fn zero(&self) -> String {
        "0".repeat(self.base.len())
    }

    /// Runs the hook the way `receive-pack` would; `Err` carries its stderr
    /// (where the rejection reason is logged) when it refuses the push.
    fn push(&self, actor: Option<&str>, input: &str) -> Result<(), String> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gitquarry"));
        cmd.arg("pre-receive-hook")
            .env("GITQUARRY_BUCKET", "main")
            .env("GITQUARRY_REPO_UID", self.repo_uid.as_str())
            .env("GITQUARRY_CONFIG", self.dir.path().join("gitquarry.toml"))
            .env("GIT_DIR", &self.git_dir)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .env_remove("GITQUARRY_ACTOR")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(actor) = actor {
            cmd.env("GITQUARRY_ACTOR", actor);
        }
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if out.status.success() { Ok(()) } else { Err(stderr) }
    }

    async fn refs(&self) -> BTreeMap<String, String> {
        let backend = backend(&self.dir.path().join("bucket"));
        let cipher = Arc::new(bootstrap::open_cipher(backend.as_ref(), PASSPHRASE).await.unwrap());
        Remote::new(backend, format!("repos/{}", self.repo_uid), cipher)
            .get_manifest()
            .await
            .unwrap()
            .0
            .refs
    }

    fn initial_refs(&self) -> BTreeMap<String, String> {
        ["refs/heads/main", "refs/heads/dev", "refs/tags/v1"]
            .into_iter()
            .map(|r| (r.to_string(), self.base.clone()))
            .collect()
    }

    async fn assert_rejected(&self, actor: Option<&str>, input: &str, reason: &str) {
        let stderr = self.push(actor, input).expect_err("push should be rejected");
        assert!(stderr.contains(reason), "expected {reason:?} in:\n{stderr}");
        assert_eq!(
            self.refs().await,
            self.initial_refs(),
            "a rejected push leaves refs.json alone"
        );
    }
}

#[tokio::test]
async fn a_non_owner_cant_push_to_a_protected_branch_even_a_fast_forward() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    let input = format!("{} {} refs/heads/main\n", f.base, f.child);
    f.assert_rejected(
        Some("bob"),
        &input,
        "protected branch 'main': only an owner may push directly",
    )
    .await;
}

#[tokio::test]
async fn the_repo_owner_can_fast_forward_a_protected_branch() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    f.push(Some("alice"), &format!("{} {} refs/heads/main\n", f.base, f.child))
        .unwrap();
    assert_eq!(f.refs().await["refs/heads/main"], f.child);
}

#[tokio::test]
async fn not_even_the_owner_can_force_push_a_protected_branch() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    let input = format!("{} {} refs/heads/main\n", f.base, f.unrelated);
    f.assert_rejected(
        Some("alice"),
        &input,
        "protected branch 'main': can't be moved/force-pushed",
    )
    .await;
}

#[tokio::test]
async fn not_even_the_owner_can_delete_a_protected_branch() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    let input = format!("{} {} refs/heads/main\n", f.base, f.zero());
    f.assert_rejected(Some("alice"), &input, "protected branch 'main': can't be deleted")
        .await;
}

#[tokio::test]
async fn a_protected_tag_never_moves_even_to_a_descendant() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    let input = format!("{} {} refs/tags/v1\n", f.base, f.child);
    f.assert_rejected(Some("alice"), &input, "protected tag 'v1': can't be moved/force-pushed")
        .await;
}

#[tokio::test]
async fn a_protected_tag_cant_be_deleted() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    let input = format!("{} {} refs/tags/v1\n", f.base, f.zero());
    f.assert_rejected(Some("alice"), &input, "protected tag 'v1': can't be deleted")
        .await;
}

#[tokio::test]
async fn the_owner_can_create_a_protected_tag_that_doesnt_exist_yet() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    f.push(Some("alice"), &format!("{} {} refs/tags/v2\n", f.zero(), f.base))
        .unwrap();
    assert_eq!(f.refs().await["refs/tags/v2"], f.base);
}

#[tokio::test]
async fn a_global_admin_can_push_to_someone_elses_protected_branch() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    f.push(Some("root"), &format!("{} {} refs/heads/main\n", f.base, f.child))
        .unwrap();
    assert_eq!(f.refs().await["refs/heads/main"], f.child);
}

#[tokio::test]
async fn only_an_owner_member_of_the_owning_group_can_push_protected() {
    let f = Fixture::new("sha1", Owner::Acme).await;
    let input = format!("{} {} refs/heads/main\n", f.base, f.child);
    f.assert_rejected(Some("bob"), &input, "only an owner may push directly")
        .await;
    f.push(Some("alice"), &input).unwrap();
    assert_eq!(f.refs().await["refs/heads/main"], f.child);
}

#[tokio::test]
async fn a_repo_without_an_owner_denies_protected_pushes_to_everyone_but_admins() {
    let f = Fixture::new("sha1", Owner::Nobody).await;
    let input = format!("{} {} refs/heads/main\n", f.base, f.child);
    f.assert_rejected(Some("alice"), &input, "only an owner may push directly")
        .await;
}

#[tokio::test]
async fn an_anonymous_push_to_a_protected_branch_is_denied() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    let input = format!("{} {} refs/heads/main\n", f.base, f.child);
    f.assert_rejected(None, &input, "only an owner may push directly").await;
}

#[tokio::test]
async fn protection_matches_the_exact_short_name_under_its_own_prefix() {
    // `main` is a protected *branch*: a tag named `main`, a branch nested
    // under `feature/main`, and the unprotected `dev` (force-pushed here)
    // are all free for a non-owner — protection is keyed by the name under
    // `refs/heads/`/`refs/tags/`, never by the last path segment.
    let f = Fixture::new("sha1", Owner::Alice).await;
    let zero = f.zero();
    let input = format!(
        "{} {} refs/heads/dev\n\n{zero} {} refs/tags/main\n{zero} {} refs/heads/feature/main\n",
        f.base, f.unrelated, f.base, f.child
    );
    f.push(Some("bob"), &input).unwrap();
    let refs = f.refs().await;
    assert_eq!(refs["refs/heads/dev"], f.unrelated);
    assert_eq!(refs["refs/tags/main"], f.base);
    assert_eq!(refs["refs/heads/feature/main"], f.child);
    assert_eq!(refs["refs/heads/main"], f.base);
}

#[tokio::test]
async fn malformed_lines_are_skipped_without_hiding_the_well_formed_ones() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    f.push(Some("bob"), "garbage\naaa bbb\n\n").unwrap();
    assert_eq!(f.refs().await, f.initial_refs(), "no ref update, nothing to do");

    let input = format!("garbage\n{} {} refs/heads/main\naaa bbb\n", f.base, f.child);
    f.assert_rejected(Some("bob"), &input, "only an owner may push directly")
        .await;
}

#[tokio::test]
async fn a_truncated_zero_sha_is_not_taken_for_a_deletion() {
    let f = Fixture::new("sha1", Owner::Alice).await;
    let input = format!("{} {} refs/heads/dev\n", f.base, "0".repeat(39));
    assert!(f.push(Some("alice"), &input).is_err());
    assert_eq!(f.refs().await, f.initial_refs());
}

#[tokio::test]
async fn sha256_zero_shas_mark_deletions_and_creations_too() {
    let f = Fixture::new("sha256", Owner::Alice).await;
    assert_eq!(f.zero().len(), 64);
    let input = format!("{} {} refs/heads/main\n", f.base, f.zero());
    f.assert_rejected(Some("alice"), &input, "protected branch 'main': can't be deleted")
        .await;

    // Accepted only if the hook compares the missing ref against a 64-wide
    // zero sha, not git's 40-wide SHA-1 one.
    f.push(Some("alice"), &format!("{} {} refs/tags/v2\n", f.zero(), f.base))
        .unwrap();
    assert_eq!(f.refs().await["refs/tags/v2"], f.base);
}

#[tokio::test]
async fn deleting_an_unprotected_branch_removes_it_from_refs_json() {
    let f = Fixture::new("sha256", Owner::Alice).await;
    f.push(Some("bob"), &format!("{} {} refs/heads/dev\n", f.base, f.zero()))
        .unwrap();
    assert!(!f.refs().await.contains_key("refs/heads/dev"));
}
