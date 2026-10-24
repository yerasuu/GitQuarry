//! The secret scan's detectors: known credential shapes by regex, plus
//! Shannon entropy for values that look assigned to a secret-sounding name
//! and for long random-looking tokens. Line-based; a finding names the rule
//! and the line, never the matched text, so the preview doesn't become a
//! second place the secret is displayed.

use regex_lite::Regex;
use std::sync::LazyLock;

/// A text blob larger than this isn't scanned, and is reported as such
/// rather than passed: an unscanned file is not a clean one.
pub(crate) const MAX_SCAN_BYTES: usize = 8 * 1024 * 1024;

/// git's own "is this binary" heuristic: a NUL in the first 8000 bytes.
const BINARY_PROBE: usize = 8000;

const GENERIC_MIN_ENTROPY: f64 = 3.5;
const TOKEN_MIN_LEN: usize = 40;
const TOKEN_MIN_ENTROPY: f64 = 4.5;

struct Rule {
    name: &'static str,
    regex: Regex,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    [
        ("aws-access-key-id", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        ("private-key", r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----"),
        (
            "github-token",
            r"\b(?:gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{22,255})\b",
        ),
        ("gitlab-token", r"\bglpat-[A-Za-z0-9_\-]{20,}"),
        ("slack-token", r"\bxox[abprs]-[A-Za-z0-9\-]{10,}"),
        ("google-api-key", r"\bAIza[0-9A-Za-z_\-]{35}"),
        ("stripe-key", r"\b[sr]k_live_[0-9A-Za-z]{20,}"),
    ]
    .into_iter()
    .filter_map(|(name, pattern)| compile(name, pattern).map(|regex| Rule { name, regex }))
    .collect()
});

/// The patterns are constants, so a failure here is a bug in this file; it
/// is logged loudly instead of panicking a request thread, and the
/// remaining rules still run.
fn compile(name: &'static str, pattern: &str) -> Option<Regex> {
    match Regex::new(pattern) {
        Ok(regex) => Some(regex),
        Err(error) => {
            tracing::error!(target: "gitquarry::publish", rule = name, %error, "secret-scan rule doesn't compile");
            None
        }
    }
}

static ASSIGNMENT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    compile(
        "secret-assignment",
        r#"(?i)(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|client[_-]?secret|private[_-]?key)["']?\s*[:=]\s*["']?([^\s"'`,;]{12,})"#,
    )
});

static TOKEN: LazyLock<Option<Regex>> = LazyLock::new(|| compile("high-entropy-string", r"[A-Za-z0-9+/=_\-]{40,}"));

/// One detector hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    pub rule: &'static str,
    /// 1-based.
    pub line: usize,
}

pub(crate) enum Scan {
    Hits(Vec<Hit>),
    Binary,
    TooLarge,
}

pub(crate) fn scan_blob(content: &[u8]) -> Scan {
    if content[..content.len().min(BINARY_PROBE)].contains(&0) {
        return Scan::Binary;
    }
    if content.len() > MAX_SCAN_BYTES {
        return Scan::TooLarge;
    }
    Scan::Hits(scan_text(&String::from_utf8_lossy(content)))
}

pub(crate) fn scan_text(text: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if let Some(rule) = line_rule(line) {
            hits.push(Hit { rule, line: index + 1 });
        }
    }
    hits
}

/// The first rule `line` trips, if any — one finding per line is enough to
/// send a human to look at it.
fn line_rule(line: &str) -> Option<&'static str> {
    if let Some(rule) = RULES.iter().find(|rule| rule.regex.is_match(line)) {
        return Some(rule.name);
    }
    if let Some(assignment) = ASSIGNMENT.as_ref()
        && assignment
            .captures_iter(line)
            .filter_map(|c| c.get(1))
            .any(|value| entropy(value.as_str()) >= GENERIC_MIN_ENTROPY)
    {
        return Some("secret-assignment");
    }
    if let Some(token) = TOKEN.as_ref()
        && token.find_iter(line).any(|m| {
            let s = m.as_str();
            s.len() >= TOKEN_MIN_LEN && !s.bytes().all(|b| b.is_ascii_hexdigit()) && entropy(s) >= TOKEN_MIN_ENTROPY
        })
    {
        return Some("high-entropy-string");
    }
    None
}

/// Shannon entropy in bits per byte.
fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[usize::from(b)] += 1;
    }
    let len = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / len;
            -p * p.log2()
        })
        .sum()
}
