//! The one place that turns a file path into the `language-*` class
//! `highlight.js` (vendored, `scripts/vendor-hljs.sh`) tokenizes a
//! `<code>` block by — shared by [`super::repo::blob::render_blob`],
//! every diff renderer, and [`super::markdown`]'s fenced code blocks.
//!
//! No per-language mapping table: `highlight.js`'s own bundled language
//! modules already register the raw file extension as an alias for most
//! languages that matter here (`rust` registers `"rs"`, `python` registers
//! `"py"`, ...), specifically so `class="language-<ext>"` works without one
//! — building a manual table here would just be re-deriving data
//! `hljs.js` already carries, and would silently drift out of sync with it
//! on a version bump. An extension `highlight.js` doesn't recognize is not
//! a panic or an error: it logs one console warning and renders the block
//! as plain unhighlighted text, same graceful fallback as no extension at
//! all (`"plaintext"`, one of `highlight.js`'s own built-in language names).

/// `path`'s extension, lowercased, or `"plaintext"` if it has none — always
/// a valid CSS class fragment (`Path::extension` never returns anything
/// with a space or a quote).
pub fn language_tag(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .filter(|ext| !ext.is_empty())
        .unwrap_or_else(|| "plaintext".to_string())
}

#[cfg(test)]
mod tests {
    use super::language_tag;

    #[test]
    fn picks_the_lowercased_extension() {
        assert_eq!(language_tag("src/main.rs"), "rs");
        assert_eq!(language_tag("README.MD"), "md");
    }

    #[test]
    fn falls_back_to_plaintext_without_an_extension() {
        assert_eq!(language_tag("Dockerfile"), "plaintext");
        assert_eq!(language_tag(""), "plaintext");
    }
}
