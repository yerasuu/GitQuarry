use gitquarry_core::git::browse::read_gitmodules;

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A repo whose only commit carries `gitmodules` (if any) as `.gitmodules`.
fn repo_with(gitmodules: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "--initial-branch=main"]);
    std::fs::write(dir.path().join("README"), "x\n").unwrap();
    if let Some(text) = gitmodules {
        std::fs::write(dir.path().join(".gitmodules"), text).unwrap();
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

#[tokio::test]
async fn maps_each_submodule_path_to_its_url() {
    let repo = repo_with(Some(
        "\
[submodule \"vendor/lib\"]
\tpath = vendor/lib
\turl = https://example.com/lib.git
[submodule \"other\"]
\tpath = other
\turl = https://example.com/other.git
",
    ));
    let modules = read_gitmodules(&repo.path().join(".git"), "main").await.unwrap();
    assert_eq!(modules.len(), 2);
    assert_eq!(
        modules.get("vendor/lib").map(String::as_str),
        Some("https://example.com/lib.git")
    );
    assert_eq!(
        modules.get("other").map(String::as_str),
        Some("https://example.com/other.git")
    );
}

#[tokio::test]
async fn no_or_unparseable_gitmodules_means_no_submodules() {
    for gitmodules in [None, Some(""), Some("not an ini file\n")] {
        let repo = repo_with(gitmodules);
        let modules = read_gitmodules(&repo.path().join(".git"), "main").await.unwrap();
        assert!(modules.is_empty(), "{gitmodules:?}");
    }
}
