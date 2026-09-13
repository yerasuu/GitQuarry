use gitquarry_webui::webui::identicon::*;

#[test]
fn same_seed_produces_the_same_markup() {
    assert_eq!(identicon("alice").into_string(), identicon("alice").into_string());
}

#[test]
fn different_seeds_produce_different_markup() {
    assert_ne!(identicon("alice").into_string(), identicon("bob").into_string());
}

/// Decorative (the username is always written out beside it, so a screen
/// reader announcing the icon too would just repeat it), and carrying no
/// pixel size of its own: `.avatar` sizes it off the surrounding text, so
/// it keeps its proportion to the name under every density and text size.
#[test]
fn is_decorative_and_sized_by_the_stylesheet() {
    let html = identicon("alice").into_string();
    assert!(html.contains(r#"aria-hidden="true""#), "{html}");
    assert!(html.contains(r#"class="avatar""#), "{html}");
    assert!(!html.contains("width=\"4"), "a hardcoded pixel size came back: {html}");
}
