use gitquarry_core::git::browse::read_gitmodules;
use gitquarry_test_support::Work;

/// A repo whose only commit carries `gitmodules` (if any) as `.gitmodules`.
async fn repo_with(gitmodules: Option<&str>) -> Work {
    let work = Work::new().await;
    let mut files = vec![("README", "x\n")];
    files.extend(gitmodules.map(|text| (".gitmodules", text)));
    work.commit(&files, "init").await;
    work
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
    ))
    .await;
    let modules = read_gitmodules(&repo.git_dir(), "main").await.unwrap();
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
        let repo = repo_with(gitmodules).await;
        let modules = read_gitmodules(&repo.git_dir(), "main").await.unwrap();
        assert!(modules.is_empty(), "{gitmodules:?}");
    }
}
