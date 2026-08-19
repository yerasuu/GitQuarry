use gitquarry_webui::webui::escaping::*;

#[test]
fn escapes_html_metacharacters() {
    assert_eq!(esc("<script>&\"'"), "&lt;script&gt;&amp;&quot;&#39;");
}

#[test]
fn url_encode_keeps_path_separators() {
    assert_eq!(url_encode("team/project"), "team/project");
    assert_eq!(url_encode("a b?c"), "a%20b%3Fc");
}
