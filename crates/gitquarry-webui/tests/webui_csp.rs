//! The Content-Security-Policy every response carries, checked against what
//! the views actually emit.
//!
//! `style-src 'self'` with no `'unsafe-inline'` only holds while no view
//! writes a `style="..."` attribute — and a CSP nonce is no way back, since
//! nonces cover `<style>` elements and never attributes. Tightening the
//! directive is a one-word edit; finding out a view still needs it takes a
//! browser and the right page. This file keeps the two facts together.

use gitquarry_webui::webui::CONTENT_SECURITY_POLICY;

/// Every module that renders markup. A file added here that still holds an
/// inline style fails the first test below.
const VIEW_SOURCES: &[(&str, &str)] = &[
    ("page_chrome.rs", include_str!("../src/webui/page_chrome.rs")),
    ("repo/mod.rs", include_str!("../src/webui/repo/mod.rs")),
    ("repo/components.rs", include_str!("../src/webui/repo/components.rs")),
    ("repo/summary.rs", include_str!("../src/webui/repo/summary.rs")),
    ("repo/stats.rs", include_str!("../src/webui/repo/stats.rs")),
    ("repo/refs.rs", include_str!("../src/webui/repo/refs.rs")),
    ("repo/releases.rs", include_str!("../src/webui/repo/releases.rs")),
    ("repo/tags.rs", include_str!("../src/webui/repo/tags.rs")),
    ("repo/branches.rs", include_str!("../src/webui/repo/branches.rs")),
    ("repo/merges/mod.rs", include_str!("../src/webui/repo/merges/mod.rs")),
    (
        "repo/merges/annotated_diff.rs",
        include_str!("../src/webui/repo/merges/annotated_diff.rs"),
    ),
    ("actions.rs", include_str!("../src/webui/actions.rs")),
    ("fork.rs", include_str!("../src/webui/fork.rs")),
    ("auth/web/groups.rs", include_str!("../src/auth/web/groups.rs")),
    ("auth/web/users.rs", include_str!("../src/auth/web/users.rs")),
];

const UTILITIES_CSS: &str = include_str!("../src/webui/style/utilities.css");
const SUMMARY_SOURCE: &str = include_str!("../src/webui/repo/summary.rs");

#[test]
fn no_view_writes_an_inline_style_attribute() {
    let offenders: Vec<&str> = VIEW_SOURCES
        .iter()
        .filter(|(_, source)| {
            source
                .lines()
                // A `//`/`//!`/`///` line naming the old shape is prose, not markup.
                .filter(|line| !line.trim_start().starts_with("//"))
                .any(|line| line.contains("style=\"") || line.contains("style=("))
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(
        offenders.is_empty(),
        "{offenders:?} set a style attribute, which `style-src 'self'` blocks and no nonce can \
         cover — add a class to style/utilities.css instead"
    );
}

#[test]
fn the_policy_allows_no_inline_style_or_script() {
    assert!(
        CONTENT_SECURITY_POLICY.contains("style-src 'self';"),
        "{CONTENT_SECURITY_POLICY}"
    );
    assert!(
        !CONTENT_SECURITY_POLICY.contains("'unsafe-inline'"),
        "{CONTENT_SECURITY_POLICY}"
    );
    assert!(
        CONTENT_SECURITY_POLICY.contains("script-src 'self';"),
        "{CONTENT_SECURITY_POLICY}"
    );
}

/// The names live in Rust and the colours in CSS, so a language added to
/// one and not the other renders with no colour at all.
#[test]
fn every_language_slug_has_a_colour_rule() {
    let table = &SUMMARY_SOURCE[SUMMARY_SOURCE.find("fn known_language_slug").expect("the slug table")..];
    let table = &table[..table.find("\n}\n").expect("the table's end")];
    let mut checked = 0;
    for line in table.lines() {
        let Some((_, slug)) = line.rsplit_once("=> \"") else {
            continue;
        };
        let Some(slug) = slug.split('"').next() else { continue };
        assert!(
            UTILITIES_CSS.contains(&format!(".lang-c-{slug} ")),
            "no .lang-c-{slug} rule in style/utilities.css"
        );
        checked += 1;
    }
    assert!(
        checked > 30,
        "only matched {checked} slugs — did the table change shape?"
    );
}

/// Same for the two generated class families a render can pick by number.
#[test]
fn every_width_and_fallback_hue_class_exists() {
    for percent in 0..=100 {
        assert!(
            UTILITIES_CSS.contains(&format!(".w-{percent} ")),
            "no .w-{percent} rule in style/utilities.css"
        );
    }
    // Read from the source rather than hardcoded here: raising the fallback
    // palette without adding the rules is exactly the drift this catches.
    let hues: u32 = SUMMARY_SOURCE
        .split("const FALLBACK_HUES: u32 = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .and_then(|n| n.trim().parse().ok())
        .expect("FALLBACK_HUES");
    for hue in 0..hues {
        assert!(
            UTILITIES_CSS.contains(&format!(".lang-c-h{hue} ")),
            "no .lang-c-h{hue} rule in style/utilities.css"
        );
    }
}
