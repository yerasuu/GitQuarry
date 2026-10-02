use gitquarry_core::app::ResolvedRepo;
use gitquarry_core::storage::remote::Manifest;
use std::path::{Path, PathBuf};

/// `git args` in `dir`, panicking with git's own stderr on failure. Signing
/// is switched off and the identity fixed so the developer's own global
/// config (a `commit.gpgsign = true`, say) can't leak into a test's history.
pub async fn git(dir: &Path, args: &[&str]) -> String {
    let out = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .await
        .expect("running git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A scratch working repo to build real history in, then push into a
/// gitquarry repo the way a client push lands: pack in the bucket, refs in
/// the manifest.
pub struct Work {
    dir: tempfile::TempDir,
}

impl Work {
    /// An empty repo on branch `main`.
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().expect("creating the work dir");
        git(dir.path(), &["init", "-q", "--initial-branch=main"]).await;
        Work { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git_dir(&self) -> PathBuf {
        self.dir.path().join(".git")
    }

    /// Writes each `(path, content)`, commits them all, returns the sha.
    pub async fn commit(&self, files: &[(&str, &str)], message: &str) -> String {
        for (path, content) in files {
            let file = self.path().join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).expect("creating the file's directory");
            }
            std::fs::write(file, content).expect("writing the file");
        }
        git(self.path(), &["add", "-A"]).await;
        git(self.path(), &["commit", "-q", "--allow-empty", "-m", message]).await;
        git(self.path(), &["rev-parse", "HEAD"]).await
    }

    /// Every object and every ref of this repo, into `repo`. `head` names
    /// the ref the manifest's `HEAD` points at.
    pub async fn push(&self, repo: &ResolvedRepo, head: &str) {
        let packs = tempfile::tempdir().expect("creating the pack dir");
        if let Some(pack) = gitquarry_core::git::pack_all_objects(&self.git_dir(), packs.path())
            .await
            .expect("packing the work repo")
        {
            repo.remote
                .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
                .await
                .expect("uploading the pack");
        }
        let listing = git(self.path(), &["for-each-ref", "--format=%(refname) %(objectname)"]).await;
        let refs = listing
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(name, sha)| (name.to_string(), sha.to_string()))
            .collect();
        let (_, etag) = repo.remote.get_manifest().await.unwrap_or((Manifest::default(), None));
        let manifest = Manifest {
            refs,
            head: Some(head.to_string()),
        };
        repo.remote
            .put_manifest(&manifest, etag.as_deref())
            .await
            .expect("writing the manifest");
    }
}
