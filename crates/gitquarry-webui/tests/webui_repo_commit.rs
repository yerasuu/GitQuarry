use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user;
use gitquarry_i18n::Locale;
use gitquarry_webui::webui::repo::commit::*;
use std::sync::Arc;

fn test_ctx(root: &std::path::Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> = Arc::new(
        gitquarry_core::storage::local::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Files,
        }),
    );
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[41u8; gitquarry_core::crypto::SALT_LEN]).unwrap(),
    );
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.to_path_buf(),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
        mirror_max_size_bytes: 0,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        events: Default::default(),
    }
}

fn gpg_available() -> bool {
    std::process::Command::new("gpg").arg("--version").output().is_ok()
}

/// Real bare repo with one commit signed by a throwaway key, the commit
/// author's email set to `email` — enough for [`render_commit`]'s own
/// `registered_key_signer` to have something to resolve against once a
/// matching `User` is registered.
fn setup_signed_commit(email: &str) -> (tempfile::TempDir, std::path::PathBuf, String, String) {
    let gpg_homedir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(gpg_homedir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let batch = format!(
        "%no-protection\nKey-Type: eddsa\nKey-Curve: ed25519\nName-Real: Test Author\nName-Email: {email}\nExpire-Date: 0\n%commit\n"
    );
    let batch_path = gpg_homedir.path().join("batch");
    std::fs::write(&batch_path, &batch).unwrap();
    let status = std::process::Command::new("gpg")
        .arg("--homedir")
        .arg(gpg_homedir.path())
        .args(["--batch", "--yes", "--no-tty", "--gen-key"])
        .arg(&batch_path)
        .status()
        .unwrap();
    assert!(status.success());
    let out = std::process::Command::new("gpg")
        .arg("--homedir")
        .arg(gpg_homedir.path())
        .args(["--batch", "--armor", "--export", email])
        .output()
        .unwrap();
    assert!(out.status.success());
    let armored_public = String::from_utf8(out.stdout).unwrap();

    let work = tempfile::tempdir().unwrap();
    let run = |args: &[&str], gnupghome: bool| {
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(work.path())
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", email)
            .env("GIT_COMMITTER_NAME", "Test Author")
            .env("GIT_COMMITTER_EMAIL", email);
        if gnupghome {
            cmd.env("GNUPGHOME", gpg_homedir.path());
        }
        let status = cmd.args(args).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "--quiet", "--initial-branch=main"], false);
    std::fs::write(work.path().join("a.txt"), "hello\n").unwrap();
    run(&["add", "."], false);
    run(
        &[
            "-c",
            &format!("user.signingkey={email}"),
            "commit",
            "--quiet",
            "-S",
            "-m",
            "signed commit",
        ],
        true,
    );
    let sha = String::from_utf8(
        std::process::Command::new("git")
            .current_dir(work.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let git_dir = work.path().join(".git");
    (work, git_dir, sha, armored_public)
}

#[tokio::test]
async fn commit_by_a_registered_gpg_key_shows_the_gitquarry_verified_badge() {
    if !gpg_available() {
        return;
    }
    let (_work, git_dir, sha, armored_public) = setup_signed_commit("alice@example.com");
    let ctx_dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(ctx_dir.path());
    user::create(
        &ctx,
        user::NewUser {
            username: "alice",
            email: Some("alice@example.com".to_string()),
            pubkey: None,
            password_plain: None,
            is_admin: false,
            repos_field: "",
            locale: Locale::En,
        },
    )
    .await
    .unwrap();
    user::add_gpg_key(&ctx, "alice", &armored_public, Locale::En)
        .await
        .unwrap();

    let (info, diff) = gitquarry_core::git::browse::show_commit(&git_dir, &sha).await.unwrap();
    let html = render_commit(&ctx, &git_dir, "repo", &info, &diff, &axum::http::HeaderMap::new())
        .await
        .unwrap();
    assert!(html.contains("verified signature (alice)"), "{html}");
}

#[tokio::test]
async fn commit_by_an_unregistered_email_falls_back_to_the_system_trust_store_badge() {
    if !gpg_available() {
        return;
    }
    let (_work, git_dir, sha, _armored_public) = setup_signed_commit("nobody@example.com");
    let ctx_dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(ctx_dir.path());

    let (info, diff) = gitquarry_core::git::browse::show_commit(&git_dir, &sha).await.unwrap();
    let html = render_commit(&ctx, &git_dir, "repo", &info, &diff, &axum::http::HeaderMap::new())
        .await
        .unwrap();
    // Falls through to the old `%G?`-based badge instead — this test has
    // no system trust store configured, so that's "unverified" (missing
    // key), never the gitquarry-registered-key wording.
    assert!(!html.contains(r#"class="badge ok""#), "{html}");
    assert!(html.contains("unverified signature (missing key)"), "{html}");
}

#[test]
fn good_signature_gets_the_good_badge() {
    assert!(signature_badge('G').into_string().contains("badge ok"));
    assert!(signature_badge('U').into_string().contains("badge ok"));
    assert!(signature_badge('X').into_string().contains("badge ok"));
}

#[test]
fn bad_signature_gets_the_bad_badge() {
    assert!(signature_badge('B').into_string().contains("badge danger"));
    assert!(signature_badge('R').into_string().contains("badge danger"));
    assert!(signature_badge('Y').into_string().contains("badge danger"));
}

#[test]
fn unchecked_gets_a_muted_hint_unsigned_gets_nothing() {
    assert!(signature_badge('E').into_string().contains("muted"));
    assert_eq!(signature_badge('N').into_string(), "");
}
