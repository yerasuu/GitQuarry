//! The Content-Security-Policy every response carries, checked against what
//! the views actually emit.
//!
//! `style-src 'self'` and `script-src 'self'`, neither with
//! `'unsafe-inline'`, only hold while no view writes a `style="..."`
//! attribute, an inline `<script>`, or an `onclick=`-style handler. A nonce
//! is no way back for two of those three: nonces cover `<style>`/`<script>`
//! *elements* and never attributes. Tightening a directive is a one-word
//! edit; finding out a view still needed it takes a browser and the right
//! page. This file keeps the two facts together.
//!
//! The scan walks `src/` off disk rather than listing modules: a view added
//! next month has to be covered without anyone remembering this file
//! exists.

use gitquarry_webui::webui::CONTENT_SECURITY_POLICY;
use std::path::{Path, PathBuf};

/// Every `.rs` file in the crate, path and contents.
fn crate_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("readable source dir") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.strip_prefix(root).unwrap_or(&path).display().to_string();
                out.push((name, std::fs::read_to_string(&path).expect("readable source")));
            }
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    assert!(out.len() > 30, "only found {} source files", out.len());
    out
}

/// Comment lines are prose — several of them name the very shapes these
/// tests forbid, explaining why they're forbidden.
fn markup_lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    source
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
}

fn offenders(needle: impl Fn(&str) -> bool) -> Vec<String> {
    crate_sources()
        .into_iter()
        .flat_map(|(name, source)| {
            markup_lines(&source)
                .filter(|(_, line)| needle(line))
                .map(|(number, _)| format!("{name}:{number}"))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn no_view_writes_an_inline_style_attribute() {
    let found = offenders(|line| line.contains("style=\"") || line.contains("style=("));
    assert!(
        found.is_empty(),
        "{found:?} set a style attribute, which `style-src 'self'` blocks and no nonce can cover \
         — add a class to style/utilities.css instead"
    );
}

/// An inline `<script>` — `script { ... }` in maud with a body, as opposed
/// to the two `script src=` tags `page_chrome::layout` emits for
/// `/_assets/app.js` and `/_assets/styler.js`.
#[test]
fn no_view_emits_an_inline_script() {
    let found = offenders(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("script") && !trimmed.starts_with("script src=") && !trimmed.starts_with("script_")
    });
    assert!(
        found.is_empty(),
        "{found:?} emit an inline script; `script-src 'self'` serves files only, so it belongs in \
         style/app.js"
    );
}

/// `onclick=`, `onsubmit=`, ... — blocked by `script-src 'self'` just as
/// hard as an inline `<script>`, and not even a nonce reaches an attribute.
/// `app.js` attaches its listeners by `data-*` hook instead.
#[test]
fn no_view_writes_an_inline_event_handler() {
    const EVENTS: &[&str] = &[
        "onclick=",
        "onsubmit=",
        "onchange=",
        "oninput=",
        "onload=",
        "onerror=",
        "onkeydown=",
        "onkeyup=",
        "onfocus=",
        "onblur=",
        "onmouseover=",
        "ontoggle=",
    ];
    let found = offenders(|line| EVENTS.iter().any(|event| line.contains(event)));
    assert!(
        found.is_empty(),
        "{found:?} set an inline event handler; hook it from style/app.js off a `data-*` \
         attribute instead"
    );
}

/// A `javascript:` URL is script too, and `script-src` governs it.
#[test]
fn no_view_links_to_a_javascript_url() {
    let found = offenders(|line| line.contains("href=\"javascript:") || line.contains("\"javascript:"));
    assert!(found.is_empty(), "{found:?} link to a javascript: URL");
}

#[test]
fn the_policy_allows_no_inline_style_or_script() {
    assert!(
        CONTENT_SECURITY_POLICY.contains("style-src 'self';"),
        "{CONTENT_SECURITY_POLICY}"
    );
    assert!(
        CONTENT_SECURITY_POLICY.contains("script-src 'self';"),
        "{CONTENT_SECURITY_POLICY}"
    );
    assert!(
        !CONTENT_SECURITY_POLICY.contains("'unsafe-inline'"),
        "{CONTENT_SECURITY_POLICY}"
    );
    assert!(
        !CONTENT_SECURITY_POLICY.contains("'unsafe-hashes'") && !CONTENT_SECURITY_POLICY.contains("'unsafe-eval'"),
        "{CONTENT_SECURITY_POLICY}"
    );
}

const UTILITIES_CSS: &str = include_str!("../src/webui/style/utilities.css");
const SUMMARY_SOURCE: &str = include_str!("../src/webui/repo/summary.rs");

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
