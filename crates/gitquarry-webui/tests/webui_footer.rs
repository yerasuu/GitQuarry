//! The site footer `layout` puts on every page.
//!
//! Cheap to render and easy to break silently: it is built once in the
//! chrome and no page's own tests would notice it disappearing.

use gitquarry_i18n::Locale;

fn page() -> String {
    gitquarry_webui::webui::layout("t", "", "<p>body</p>", "", "auto", "cozy", Locale::En)
}

#[test]
fn the_footer_carries_the_copyright_licence_and_changelog() {
    let html = page();

    assert!(html.contains("2026"), "no copyright year: {html}");
    assert!(html.contains("Yerasuu"), "no copyright holder");
    assert!(html.contains("AGPL-3.0-or-later"), "no licence");
    // The file this binary ships, not gnu.org's copy: the AGPL's terms have
    // to reach a network user from the operator, and an outbound link
    // points at text the project doesn't control.
    assert!(html.contains("href=\"/_license\""), "licence links off-site");
    assert!(html.contains("href=\"/_changelog\""), "no changelog link");
}

/// The running version, next to the changelog link: a reader comparing an
/// instance against a release needs to know which build they are looking at,
/// and the footer is the one place present on every page.
#[test]
fn the_footer_names_the_version_this_binary_was_built_from() {
    assert!(page().contains(env!("CARGO_PKG_VERSION")));
}

/// Outside `main`, so the skip-link target holds only the page's own
/// content and a screen reader walking to the end of an article isn't
/// handed the same three chrome links every time.
#[test]
fn the_footer_sits_outside_the_main_content_landmark() {
    let html = page();
    let main_end = html.find("</main>").expect("no main landmark");
    // The element, not the class name: the stylesheet is inlined into
    // `<head>` and mentions `.site-footer` long before `<main>` opens.
    let footer = html.find("<footer").expect("no footer");

    assert!(footer > main_end, "footer rendered inside <main>");
}

/// Both locales render it — a missing `Locale::Es` arm would fall back to
/// English text on a Spanish page rather than failing loudly.
#[test]
fn the_footer_renders_in_every_locale() {
    for locale in [Locale::En, Locale::Es] {
        let html = gitquarry_webui::webui::layout("t", "", "", "", "auto", "cozy", locale);
        assert!(html.contains("<footer"), "{locale:?}");
        assert!(html.contains("Yerasuu"), "{locale:?}");
    }
}
