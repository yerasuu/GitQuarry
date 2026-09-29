//! The commit page's signature badge, as `GET /{repo}?view=commit`
//! renders it for a commit pushed into the bucket.

use axum::Router;
use axum::http::StatusCode;
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::domain::user;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_i18n::Locale;
use gitquarry_test_support::{Session, Work, ctx, git};
use std::path::Path;
use std::sync::Arc;

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> String {
    let page = Session::get_as(app, uri, cookie).await;
    assert_eq!(page.status, StatusCode::OK, "{uri}: {}", page.body);
    page.body
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
    let work = Work::new().await;
    std::fs::write(work.path().join("a.txt"), "hello\n").unwrap();
    git(work.path(), &["add", "."]).await;
    // Not the shared `git`: that one pins the author and switches signing
    // off, and this commit needs `email` as its author and, when given,
    // `signer`'s signature.
    let mut commit = std::process::Command::new("git");
    commit
        .current_dir(work.path())
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
    let sha = git(work.path(), &["rev-parse", "HEAD"]).await;

    let ctx = Arc::new(ctx(dir));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    work.push(&repo, "refs/heads/main").await;
    let app = gitquarry_webui::webui::router(Arc::clone(&ctx));
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
