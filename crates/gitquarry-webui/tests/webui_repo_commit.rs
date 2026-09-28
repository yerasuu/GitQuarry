//! The commit page's signature badge, as `GET /{repo}?view=commit`
//! renders it for a commit pushed into the bucket.

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user;
use gitquarry_core::storage::Cas;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_core::storage::remote::Manifest;
use gitquarry_i18n::Locale;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const SALT_BYTE: u8 = 41;

fn test_ctx(root: &Path) -> AppContext {
    let backend: Arc<dyn gitquarry_core::storage::Backend> =
        Arc::new(s3db::LocalBackend::new(&gitquarry_core::config::LocalParams {
            bucket: root.to_string_lossy().to_string(),
            engine: gitquarry_core::config::LocalEngine::Redb,
        }));
    let cipher = Arc::new(
        gitquarry_core::crypto::Cipher::derive("test passphrase", &[SALT_BYTE; s3db_crypto::SALT_LEN]).unwrap(),
    );
    AppContext {
        bucket: "test".to_string(),
        ssh_bind: "127.0.0.1:2222".to_string(),
        mirrors_root: root.join("mirrors"),
        config_path: root.join("gitquarry.toml"),
        exe_path: root.join("gitquarry"),
        backend,
        locks: Default::default(),
        cipher,
        mirror_max_age: std::time::Duration::from_secs(1),
        mirror_max_size_bytes: 0,
        mirror_large_repo_threshold_bytes: u64::MAX,
        oidc: None,
        login_attempts: Default::default(),
        git_rate_limiter: Default::default(),
        federation: None,
        allow_push_create: false,
        trust_forwarded_for: false,
        events: Default::default(),
        mirror_sync: None,
        visible_repo_uids_cache: Default::default(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Pushes `work`'s whole history and every ref it has into a fresh public
/// `team/proj` as a real pack, so the server's own mirror serves it.
async fn publish(root: &Path, work: &Path) -> (Arc<AppContext>, Router) {
    let ctx = Arc::new(test_ctx(root));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    let packs = tempfile::tempdir().unwrap();
    let pack = gitquarry_core::git::pack_objects_for_push(
        &work.join(".git"),
        &git(work, &["rev-parse", "HEAD"]),
        &[],
        packs.path(),
    )
    .await
    .unwrap()
    .unwrap();
    repo.remote
        .upload_pack(&pack.base_name, &pack.pack_path, &pack.idx_path)
        .await
        .unwrap();
    let refs = git(work, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let manifest = Manifest {
        refs: refs
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(name, sha)| (name.to_string(), sha.to_string()))
            .collect(),
        head: Some("refs/heads/main".to_string()),
    };
    let (_, etag) = repo.remote.get_manifest().await.unwrap();
    assert_eq!(
        repo.remote.put_manifest(&manifest, etag.as_deref()).await.unwrap(),
        Cas::Ok
    );
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
    (ctx, app)
}

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> String {
    let mut builder = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    // `run_server` serves with connect info; a bare `oneshot` doesn't, and
    // the git-smart-HTTP layer 500s without it.
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(status, StatusCode::OK, "{uri}: {html}");
    html
}

fn gpg_available() -> bool {
    std::process::Command::new("gpg").arg("--version").output().is_ok()
}

/// A throwaway ed25519 key for `email` in its own `GNUPGHOME`, plus its
/// armored public half.
fn gpg_key(email: &str) -> (tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let batch = home.path().join("batch");
    std::fs::write(
        &batch,
        format!(
            "%no-protection\nKey-Type: eddsa\nKey-Curve: ed25519\nName-Real: Test Author\nName-Email: {email}\nExpire-Date: 0\n%commit\n"
        ),
    )
    .unwrap();
    let gpg = |args: &[&str]| {
        let out = std::process::Command::new("gpg")
            .arg("--homedir")
            .arg(home.path())
            .args(["--batch", "--yes", "--no-tty"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "gpg {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    gpg(&["--gen-key", &batch.to_string_lossy()]);
    let armored = gpg(&["--armor", "--export", email]);
    (home, armored)
}

/// A repo whose only commit is authored by `email`, signed with `signer`'s
/// key when given, published as `team/proj`.
async fn publish_commit(dir: &Path, email: &str, signer: Option<&Path>) -> (Arc<AppContext>, Router, String) {
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(work.join("a.txt"), "hello\n").unwrap();
    git(&work, &["add", "."]);
    let mut commit = std::process::Command::new("git");
    commit
        .current_dir(&work)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", email)
        .args([
            "-c",
            &format!("user.signingkey={email}"),
            "commit",
            "--quiet",
            "-m",
            "the commit",
        ]);
    match signer {
        Some(home) => commit.env("GNUPGHOME", home).arg("-S"),
        None => commit.arg("--no-gpg-sign"),
    };
    assert!(commit.status().unwrap().success());
    let sha = git(&work, &["rev-parse", "HEAD"]);
    let (ctx, app) = publish(dir, &work).await;
    (ctx, app, sha)
}

/// The page's `<h3>` title, where the badge sits.
fn title(html: &str) -> &str {
    let start = html.find("<h3>commit ").expect("no commit title");
    let len = html[start..].find("</h3>").unwrap();
    &html[start..start + len]
}

#[tokio::test]
async fn commit_by_a_registered_gpg_key_shows_the_gitquarry_verified_badge() {
    if !gpg_available() {
        return;
    }
    let (home, armored) = gpg_key("alice@example.com");
    let dir = tempfile::tempdir().unwrap();
    let (ctx, app, sha) = publish_commit(dir.path(), "alice@example.com", Some(home.path())).await;
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
    user::add_gpg_key(&ctx, "alice", &armored, Locale::En).await.unwrap();

    let html = get(&app, &format!("/team/proj?view=commit&sha={sha}"), None).await;
    assert!(
        title(&html).contains(r#"<span class="badge ok">verified signature (alice)</span>"#),
        "{html}"
    );
}

/// The fallback badge, reached when gitquarry itself couldn't attribute the
/// signature to an account or to this instance — a present-but-unattributed
/// signature must never read as valid, however the signature got there.
#[tokio::test]
async fn a_signature_gitquarry_cant_attribute_gets_a_muted_hint_never_a_good_badge() {
    if !gpg_available() {
        return;
    }
    let (home, _armored) = gpg_key("nobody@example.com");
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, sha) = publish_commit(dir.path(), "nobody@example.com", Some(home.path())).await;

    let html = get(&app, &format!("/team/proj?view=commit&sha={sha}"), None).await;
    let title = title(&html);
    assert!(!title.contains("badge ok"), "{title}");
    assert!(
        title.contains(r#"<span class="muted">unverified signature (missing key)</span>"#),
        "{title}"
    );
}

#[tokio::test]
async fn unsigned_gets_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (_ctx, app, sha) = publish_commit(dir.path(), "test@example.com", None).await;
    let html = get(&app, &format!("/team/proj?view=commit&sha={sha}"), None).await;
    let title = title(&html);
    assert!(!title.contains("signature"), "{title}");
    assert!(!title.contains("<span"), "{title}");
}
