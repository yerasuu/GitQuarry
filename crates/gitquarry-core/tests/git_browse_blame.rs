use gitquarry_core::git::browse::blame::*;

#[test]
fn parses_blame_porcelain() {
    // Minimal fixture: two lines, second reuses the first commit's
    // metadata block (git omits repeated author/etc lines).
    let text = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 1
author Jane Doe
author-mail <jane@example.com>
summary first commit
filename src/x.rs
\tfn main() {}
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 2 2
\t}
";
    let lines = parse_blame_porcelain(text);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].sha, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(lines[0].author, "Jane Doe");
    assert_eq!(lines[0].content, "fn main() {}");
    assert_eq!(lines[1].content, "}");
}
