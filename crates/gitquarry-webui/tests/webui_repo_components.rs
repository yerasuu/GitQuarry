//! The diff and commit-table renderings shared by the repo views, read off
//! the pages that show them: the commit page's unified and side-by-side
//! diffs, and the tree page's recent-commits table.

use axum::Router;
use axum::http::StatusCode;
use gitquarry_core::storage::entities::Visibility;
use gitquarry_test_support::{Session, Work, ctx, git};
use std::sync::Arc;

const LEAD: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\n";

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> String {
    let page = Session::get_as(app, uri, cookie).await;
    assert_eq!(page.status, StatusCode::OK, "{uri}: {}", page.body);
    page.body
}

/// `main` is `start` -> `before` (tagged `v1.0`) -> `rework`, and `rework`
/// touches one file per diff shape worth telling apart: `notes` (no
/// extension) loses two lines for one, `src/main.rs` changes its last line
/// so its hunk starts mid-file, `gone.go` is deleted (`+++ /dev/null`),
/// `added.py` is new (`--- /dev/null`), and `a b.rs` has a space in its
/// name, which makes git end its `---`/`+++` headers with a tab.
struct Fixture {
    _dir: tempfile::TempDir,
    app: Router,
    rework: String,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let work = Work::new().await;
    work.commit(&[("README", "hi\n")], "start").await;

    let lead_gone = format!("{LEAD}gone\n");
    work.commit(
        &[
            ("notes", "old one\nold two\ncontext\n"),
            ("src/main.rs", &lead_gone),
            ("gone.go", "package gone\n"),
            ("a b.rs", "a\n"),
        ],
        "before",
    )
    .await;
    git(work.path(), &["tag", "v1.0"]).await;

    std::fs::remove_file(work.path().join("gone.go")).unwrap();
    let lead_added = format!("{LEAD}added\n");
    let rework = work
        .commit(
            &[
                ("notes", "new one\ncontext\n"),
                ("src/main.rs", &lead_added),
                ("added.py", "print()\n"),
                ("a b.rs", "a\nb\n"),
            ],
            "rework",
        )
        .await;

    let ctx = Arc::new(ctx(dir.path()));
    let repo = gitquarry_core::domain::repo::create(&ctx, "team/proj", None, None, None, Visibility::Public)
        .await
        .unwrap()
        .unwrap();
    work.push(&repo, "refs/heads/main").await;
    let app = gitquarry_webui::webui::router(ctx);
    Fixture { _dir: dir, app, rework }
}

async fn commit_page(f: &Fixture, cookie: Option<&str>) -> String {
    get(&f.app, &format!("/team/proj?view=commit&sha={}", f.rework), cookie).await
}

#[tokio::test]
async fn unified_diff_headers_win_over_add_del() {
    let f = fixture().await;
    let html = commit_page(&f, None).await;
    // `+++`/`---` start with the add/del markers but are headers, never lines.
    assert!(html.contains(r#"<span class="diff-meta">--- a/notes</span>"#), "{html}");
    assert!(html.contains(r#"<span class="diff-meta">+++ b/notes</span>"#), "{html}");
    assert!(
        html.contains(r#"<span class="diff-hunk">@@ -1,3 +1,2 @@</span>"#),
        "{html}"
    );
    // No extension -> `language-plaintext`; the `+`/`-` marker sits outside
    // the `<code>`, since it isn't source text.
    assert!(
        html.contains(r#"<span class="diff-add-line">+<code class="language-plaintext">new one</code></span>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="diff-del-line">-<code class="language-plaintext">old one</code></span>"#),
        "{html}"
    );
}

/// Each line is tagged with the language of the file it belongs to, which
/// means the path has to be tracked from each file's own headers: the `b/`
/// side normally, the `a/` side when the new one is `/dev/null`, never
/// `/dev/null` itself, and without the tab git appends to a spaced name.
#[tokio::test]
async fn each_diff_line_is_tagged_with_its_own_files_language() {
    let f = fixture().await;
    let html = commit_page(&f, None).await;
    for (marker, language, text) in [
        ('+', "rs", "added"),
        ('-', "go", "package gone"),
        ('+', "py", "print()"),
        ('+', "rs", "b"),
    ] {
        let line = format!(r#"{marker}<code class="language-{language}">{text}</code>"#);
        assert!(html.contains(&line), "{line}: {html}");
    }
    assert!(!html.contains("language-rs\t"), "{html}");
    assert!(!html.contains("language-dev"), "{html}");
}

#[tokio::test]
async fn side_by_side_pairs_dels_against_adds_row_by_row() {
    let f = fixture().await;
    let html = commit_page(&f, Some("diff-view=split")).await;
    // Each side carries its own line number, GitHub-style: "old one"/"new
    // one" land on the same row (positional pairing), "old two" has no
    // partner so the new side's cells are blank instead.
    for row in [
        r#"<td class="lineno">1</td><td class="diff-del-line"><code class="language-plaintext">old one</code></td><td class="lineno">1</td><td class="diff-add-line"><code class="language-plaintext">new one</code></td>"#,
        r#"<td class="lineno">2</td><td class="diff-del-line"><code class="language-plaintext">old two</code></td><td class="lineno"></td><td></td>"#,
        r#"<td class="lineno">3</td><td><code class="language-plaintext">context</code></td><td class="lineno">2</td><td><code class="language-plaintext">context</code></td>"#,
        // Numbered from where this file's hunk header starts, not from 1.
        r#"<td class="lineno">8</td><td class="diff-del-line"><code class="language-rs">gone</code></td><td class="lineno">8</td><td class="diff-add-line"><code class="language-rs">added</code></td>"#,
    ] {
        assert!(html.contains(row), "{row}: {html}");
    }
}

#[tokio::test]
async fn side_by_side_spans_hunk_and_file_headers_across_both_columns() {
    let f = fixture().await;
    let html = commit_page(&f, Some("diff-view=split")).await;
    for row in [
        r#"<td class="diff-meta" colspan="4">diff --git a/notes b/notes</td>"#,
        r#"<td class="diff-meta" colspan="4">--- a/notes</td>"#,
        r#"<td class="diff-meta" colspan="4">+++ b/notes</td>"#,
        r#"<td class="diff-hunk" colspan="4">@@ -1,3 +1,2 @@</td>"#,
    ] {
        assert!(html.contains(row), "{row}: {html}");
    }
}

#[tokio::test]
async fn recent_commits_carry_their_ref_decorations_as_badges() {
    let f = fixture().await;
    let html = get(&f.app, "/team/proj?view=tree", None).await;
    // `%D`'s ", "-separated list is one badge per ref, escaped.
    assert!(
        html.contains(r#"<td>rework <span class="badge accent">HEAD -&gt; main</span></td>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<td>before <span class="badge accent">tag: v1.0</span></td>"#),
        "{html}"
    );
    // No refs, no badge.
    assert!(html.contains("<td>start</td>"), "{html}");
}
