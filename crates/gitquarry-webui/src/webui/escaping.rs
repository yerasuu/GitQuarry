//! Generic string-safety helpers: HTML/XML escaping, percent-encoding,
//! redirect-target validation, and blank-to-`None` normalization.

/// HTML escaping for text and for double-quoted attribute values. Also safe
/// to use for XML text content (the atom feed): the four characters escaped
/// here are exactly the ones XML text nodes require.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-encoding of everything that isn't "unreserved" per RFC 3986. `/`
/// is left alone since it separates repo path segments, which already come
/// sanitized.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `true` if `s` is safe to redirect to: a same-origin relative path only.
/// Rejects `//host` (browsers treat a leading `//` as protocol-relative,
/// same as `https://host`) and any backslash — WHATWG URL parsing
/// normalizes `\` to `/` for http(s) URLs, so `/\evil.com` would otherwise
/// slip past a naive `starts_with("//")` check and still resolve to
/// `//evil.com`. Shared by every `return_to`-style redirect target
/// ([`crate::auth::web::login`], [`crate::auth::web::favorites`], and the preference
/// toggles) so the check only needs fixing in one place. Also rejects any
/// control character or whitespace: the URL parser strips tab/CR/LF before
/// resolving, so `/\t/evil.com` would otherwise reach the browser as
/// `//evil.com`, and no legitimate local path carries a raw one.
pub(crate) fn is_local_redirect(s: &str) -> bool {
    s.starts_with('/')
        && !s.starts_with("//")
        && !s.contains('\\')
        && !s.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub fn non_empty(s: String) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
