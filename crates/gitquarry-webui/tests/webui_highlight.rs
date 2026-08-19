use gitquarry_webui::webui::highlight::*;

#[test]
fn recognized_syntax_gets_semantic_token_classes() {
    let lines = highlighted_lines("main.rs", "if x == 1 { return; }");
    let line = &lines[0];
    assert!(line.contains("class=\"tok-kw\""), "line: {line}");
    // Operators (`==`) deliberately stay uncolored — see `TOKEN_CLASSES`'s
    // own comment on why the keyword selector excludes `keyword.operator` —
    // the text is still there, just never wrapped in a `tok-*` span.
    assert!(line.contains("=="), "line: {line}");
    assert!(!line.contains("\">==<"), "== got wrapped in a class span: {line}");
}

#[test]
fn plain_unrecognized_text_gets_no_token_classes() {
    // No extension syntect recognizes -> falls back to plain text, which
    // has no scope rules to match at all, unlike the old theme-baked
    // version where even unstyled text still carried the active theme's
    // own default color.
    let lines = highlighted_lines("README", "just some plain text");
    assert!(!lines[0].contains("class=\"tok-"), "line: {}", lines[0]);
}

#[test]
fn highlighted_lines_by_token_recognizes_a_fence_language_tag() {
    let rust = highlighted_lines_by_token("rust", "fn main() {}");
    let plain = highlighted_lines_by_token("bogus-lang-xyz", "fn main() {}");
    assert_ne!(rust, plain);
    assert!(rust[0].contains("class=\"tok-"), "rust: {}", rust[0]);
    assert!(!plain[0].contains("class=\"tok-"), "plain: {}", plain[0]);
}

#[test]
fn output_is_one_fragment_per_line_and_html_escaped() {
    let lines = highlighted_lines("main.rs", "let a = 1;\nlet b = 2;");
    assert_eq!(lines.len(), 2);

    let escaped = highlighted_lines("data.txt", "a < b && c > d");
    assert_eq!(escaped.len(), 1);
    assert!(!escaped[0].contains("a < b"));
    assert!(escaped[0].contains("&lt;"));
    assert!(escaped[0].contains("&gt;"));
}

#[test]
fn a_function_name_and_a_string_get_their_own_distinct_classes() {
    let lines = highlighted_lines("main.rs", "fn main() { let s = \"hi\"; }");
    let line = &lines[0];
    assert!(line.contains("class=\"tok-fn\""), "line: {line}");
    assert!(line.contains("class=\"tok-str\""), "line: {line}");
}
