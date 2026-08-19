//! Syntax highlighting for the blob view and README code fences, via
//! `syntect` — theme-agnostic on purpose: this used to bake one of two
//! `syntect::highlighting::Theme`s (light/dark) into literal
//! `style="color:#rrggbb"` spans at render time, which meant a README or
//! blob rendered under one theme stayed wrong-colored (low-contrast against
//! `--code-bg`, in the worst case) until the *next* render, since a
//! client-side theme flip has no way to touch already-baked inline colors.
//! Every token instead gets a semantic `class="tok-*"` (`--syn-*` tokens,
//! `base.css`) — the same "flip an attribute on `<html>`, CSS repaints
//! everything" mechanism the rest of the page's chrome already uses, so a
//! theme toggle repaints highlighted code too, instantly, with zero re-render.
//!
//! Classification reuses `syntect`'s own `Highlighter` instead of walking
//! scope stacks by hand: [`CLASSIFIER`] is a real (but never rendered)
//! `Theme` where each rule we care about (`"keyword"`, `"string"`, ...) is
//! assigned its own unique, otherwise-meaningless marker `Color` — running
//! the normal highlight pass against it and mapping each resulting color
//! back to the class name that produced it *is* the classification, letting
//! `syntect`'s own scope-selector specificity/priority rules (which rule
//! wins when more than one matches) do the actual work.
//!
//! Loaded once (syntax set is a few MB parsed from bundled YAML data — not
//! something to redo per request) and kept behind `LazyLock` rather than
//! built at startup: most repos never render a blob, so paying the cost
//! lazily beats paying it on every server start.

use std::collections::HashMap;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, Style, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::{SyntaxReference, SyntaxSet};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// `(scope selector, css class)` — order matters where scopes overlap
/// (e.g. a function name that's also a keyword in some grammar): later
/// entries win ties at equal specificity, same as a real theme file's own
/// rule order. Kept short on purpose: a handful of well-known scopes covers
/// what most languages' grammars actually use, and an unmatched token
/// falling through to the unstyled default (`--code-fg`, no span at all)
/// is a fine, boring outcome — better than a class list nobody can hold in
/// their head.
const TOKEN_CLASSES: &[(&str, &str)] = &[
    ("comment", "tok-com"),
    ("string, string.quoted", "tok-str"),
    ("constant.numeric", "tok-num"),
    ("constant.language, constant.character, constant.other", "tok-const"),
    ("entity.name.tag", "tok-tag"),
    ("entity.other.attribute-name", "tok-attr"),
    (
        "storage.type, entity.name.type, entity.name.class, support.type, support.class",
        "tok-type",
    ),
    (
        "entity.name.function, support.function, meta.function-call, entity.name.macro, support.macro",
        "tok-fn",
    ),
    // `keyword.control`/`keyword.other`, not the bare `keyword` prefix: that
    // also matches `keyword.operator` (`=`, `+`, `->`, ...), which almost no
    // real theme colors — operators reading as "keyword"-important was more
    // noise than signal.
    ("keyword.control, keyword.other, storage.modifier", "tok-kw"),
];

/// `(r, g, b) -> class` reverse lookup for [`TOKEN_CLASSES`]'s marker
/// colors, paired with the exact [`Theme`] that produced it in
/// [`CLASSIFIER`] — named so that pairing doesn't read as an opaque
/// tuple type at every use site.
type ColorClassLookup = HashMap<(u8, u8, u8), &'static str>;

/// The classifier `Theme` plus its [`ColorClassLookup`] — built together
/// since the lookup is only ever meaningful paired with the exact theme
/// that produced it.
static CLASSIFIER: LazyLock<(Theme, ColorClassLookup)> = LazyLock::new(|| {
    let mut scopes = Vec::with_capacity(TOKEN_CLASSES.len());
    let mut lookup = HashMap::with_capacity(TOKEN_CLASSES.len());
    for (i, (selector, class)) in TOKEN_CLASSES.iter().enumerate() {
        // Marker colors start at r=1 (not 0) so they never collide with
        // `settings.foreground` below (0, 0, 0) — that's the "no rule
        // matched" default `syntect` falls back to, which must stay
        // absent from `lookup` for the plain-span fallback in
        // `highlight_with_syntax` to trigger correctly.
        let r = u8::try_from(i + 1).expect("TOKEN_CLASSES has far fewer than 255 entries");
        let color = Color { r, g: 0, b: 0, a: 255 };
        scopes.push(ThemeItem {
            scope: selector
                .parse()
                .expect("TOKEN_CLASSES selectors are valid scope syntax"),
            style: StyleModifier {
                foreground: Some(color),
                background: None,
                font_style: None,
            },
        });
        lookup.insert((r, 0, 0), *class);
    }
    let theme = Theme {
        name: Some("gitquarry-classifier".to_string()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            background: Some(Color {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            ..Default::default()
        },
        scopes,
    };
    (theme, lookup)
});

/// Renders `text` (the contents of `path`) as one HTML fragment per source
/// line, syntax-highlighted by `path`'s extension. Each fragment already has
/// its HTML-unsafe characters escaped — callers must NOT run them through
/// [`super::esc`] again.
///
/// Falls back to plain (unstyled but still escaped) lines when the extension
/// isn't recognized, so callers can always rely on one fragment per line
/// regardless of language — that's what makes line numbering uniform.
pub fn highlighted_lines(path: &str, text: &str) -> Vec<String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let syntax = SYNTAX_SET
        .find_syntax_by_extension(ext)
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    highlight_with_syntax(syntax, text)
}

/// Same as [`highlighted_lines`] but looked up by a fenced-code-block
/// language tag (a README's ```` ```rust ```` fence, not a file path) —
/// `find_syntax_by_token` is syntect's own lookup for exactly this case
/// (matches an extension or a case-insensitive name/alias, which is what a
/// markdown fence's language token actually is). Falls back to plain text
/// the same way an unrecognized file extension does.
pub fn highlighted_lines_by_token(token: &str, text: &str) -> Vec<String> {
    let syntax = SYNTAX_SET
        .find_syntax_by_token(token)
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
    highlight_with_syntax(syntax, text)
}

fn highlight_with_syntax(syntax: &SyntaxReference, text: &str) -> Vec<String> {
    let (theme, lookup) = &*CLASSIFIER;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in text.lines() {
        // `LinesWithEndings`-style input is what syntect's own examples use,
        // but `highlight_line` works fine per-line without the trailing
        // newline as long as we feed it consistently line by line, which is
        // all we need since we render one table row per line anyway.
        let ranges = match highlighter.highlight_line(line, &SYNTAX_SET) {
            Ok(ranges) => ranges,
            // A line that breaks the highlighter (encoding weirdness deep in
            // a language grammar) shouldn't take down the whole blob view.
            Err(_) => vec![(Style::default(), line)],
        };
        out.push(render_classified_line(&ranges, lookup));
    }
    out
}

/// Emits `<span class="tok-*">` around a range only when its marker color
/// resolved to a known class — an unmatched (default-styled) range is just
/// escaped text, inheriting whatever plain code color the page already
/// uses, same as `html_escape_fallback` below for a line the highlighter
/// rejected outright.
fn render_classified_line(ranges: &[(Style, &str)], lookup: &HashMap<(u8, u8, u8), &'static str>) -> String {
    let mut out = String::new();
    for (style, text) in ranges {
        let key = (style.foreground.r, style.foreground.g, style.foreground.b);
        match lookup.get(&key) {
            Some(class) => {
                out.push_str("<span class=\"");
                out.push_str(class);
                out.push_str("\">");
                push_escaped(&mut out, text);
                out.push_str("</span>");
            }
            None => push_escaped(&mut out, text),
        }
    }
    out
}

fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}
