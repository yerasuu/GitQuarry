use gitquarry_webui::webui::icons::*;

/// Every icon must stay decorative (never its own accessible name) and
/// sized/colored by the `.icon` CSS class rather than a hardcoded fill.
#[test]
fn every_icon_is_decorative_and_uses_the_icon_class() {
    for icon in [
        folder(),
        file(),
        branch(),
        tag(),
        lock(),
        fork(),
        star(),
        eye(),
        archive(),
        menu(),
        license(),
    ] {
        let html = icon.into_string();
        assert!(html.contains(r#"class="icon""#), "{html}");
        assert!(html.contains(r#"aria-hidden="true""#), "{html}");
        assert!(html.contains("currentColor"), "{html}");
    }
}

/// Every shape element must be explicitly closed (`</circle>`, `</path>`,
/// ...) — a bare `;` in maud leaves non-void SVG elements unclosed, which
/// nests every following sibling inside the previous shape and makes
/// browsers paint only the outermost one.
#[test]
fn every_multi_shape_icon_closes_each_element() {
    for (icon, tags) in [
        (file(), &["path", "path"][..]),
        (branch(), &["path", "path", "circle", "circle", "circle"]),
        (tag(), &["path", "circle"]),
        (lock(), &["rect", "path"]),
        (fork(), &["circle", "circle", "circle", "path", "path"]),
        (eye(), &["path", "circle"]),
        (archive(), &["rect", "path", "path"]),
        (
            license(),
            &["circle", "line", "line", "line", "line", "line", "path", "path"],
        ),
        (menu(), &["line", "line", "line"]),
    ] {
        let html = icon.into_string();
        for tag in tags {
            assert_eq!(
                html.matches(&format!("</{tag}>")).count(),
                tags.iter().filter(|t| *t == tag).count(),
                "{tag} not individually closed in {html}"
            );
        }
    }
}
