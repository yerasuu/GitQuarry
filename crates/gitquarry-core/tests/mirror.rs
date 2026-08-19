use gitquarry_core::mirror::*;

#[test]
fn quotes_paths_with_spaces_and_quotes() {
    assert_eq!(shell_quote("/usr/bin/x"), "'/usr/bin/x'");
    assert_eq!(shell_quote("/a b/x"), "'/a b/x'");
    assert_eq!(shell_quote("/a'b"), r"'/a'\''b'");
}
