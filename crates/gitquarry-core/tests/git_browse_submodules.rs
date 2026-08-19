// `submodules` itself is a private submodule of `git::browse` (see that
// module's doc comment); its public items are re-exported at
// `git::browse::*`, which is the path visible from outside the crate.
use gitquarry_core::git::browse::*;

#[test]
fn parses_gitmodules() {
    let text = "\
[submodule \"vendor/lib\"]
\tpath = vendor/lib
\turl = https://example.com/lib.git
[submodule \"other\"]
\tpath = other
\turl = https://example.com/other.git
";
    let modules = parse_gitmodules(text);
    assert_eq!(
        modules.get("vendor/lib"),
        Some(&"https://example.com/lib.git".to_string())
    );
    assert_eq!(modules.get("other"), Some(&"https://example.com/other.git".to_string()));
}

#[test]
fn gitmodules_empty_when_no_sections() {
    assert!(parse_gitmodules("").is_empty());
    assert!(parse_gitmodules("not an ini file\n").is_empty());
}
