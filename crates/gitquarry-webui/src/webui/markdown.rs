//! README rendering for the summary tab.
//!
//! A README is untrusted content: anyone with write access to a repo
//! controls what gets rendered here for every other viewer of the summary
//! page. Two things get stripped before handing events to pulldown-cmark's
//! own (already-escaping) HTML writer:
//! - raw HTML blocks/inline HTML (`Event::Html`/`Event::InlineHtml`) — the
//!   direct stored-XSS vector.
//! - non-http(s)/mailto/relative link and image targets (`javascript:...`
//!   etc) — pulldown-cmark happily emits whatever scheme the source used.
//!
//! A fenced code block with a language tag (` ```rust `) gets routed
//! through [`super::highlight::highlighted_lines_by_token`] instead of
//! pulldown-cmark's own plain `<pre><code>` — same syntax highlighter the
//! blob view uses, just looked up by the fence's language token instead of
//! a file extension. A fence with no language tag (or an indented code
//! block) renders exactly as before: plain, escaped, unhighlighted.
//!
//! GitHub-style alert blockquotes (`> [!NOTE]`/`[!TIP]`/`[!IMPORTANT]`/
//! `[!WARNING]`/`[!CAUTION]`) are pulldown-cmark's own native `ENABLE_GFM`
//! feature — it parses the marker line itself (nothing to strip by hand
//! here) and emits `<blockquote class="markdown-alert-note">` etc.; the
//! bold label line (`::before`) and colors are pure CSS, see
//! `.readme .markdown-alert-*` in `base.css`.

use super::highlight;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd, html};
use std::collections::HashMap;

pub fn render(markdown: &str) -> String {
    render_with_mentions(markdown, &HashMap::new())
}

/// Same, plus `#123` mentions turned into links. `mentions` maps a number
/// to its href; a number missing from it stays plain text rather than
/// becoming a dead link, so the caller decides what exists by what it puts
/// in the map.
///
/// The substitution happens on the parsed event stream, not on the source
/// text, because only the parser knows what is prose: a `#123` inside a
/// fenced block or a backtick span is code and must survive verbatim, and
/// one inside an existing link would nest an `<a>` in an `<a>`.
pub fn render_with_mentions(markdown: &str, mentions: &HashMap<u64, String>) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let parser = Parser::new_ext(markdown, options);
    let events = highlight_code_blocks(parser);
    let events = link_mentions(events, mentions);
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
}

/// Rewrites `#123` inside prose text events into link events. Inline code
/// arrives as `Event::Code` and is never touched; a fenced or indented
/// block's contents arrive as `Event::Text` *between* the code-block tags,
/// which is what `depth` tracks.
fn link_mentions<'a>(events: Vec<Event<'a>>, mentions: &HashMap<u64, String>) -> Vec<Event<'a>> {
    if mentions.is_empty() {
        return events;
    }
    let mut out = Vec::with_capacity(events.len());
    let mut in_code = 0usize;
    let mut in_link = 0usize;
    for event in events {
        match &event {
            Event::Start(Tag::CodeBlock(_)) => in_code += 1,
            Event::End(TagEnd::CodeBlock) => in_code = in_code.saturating_sub(1),
            Event::Start(Tag::Link { .. }) => in_link += 1,
            Event::End(TagEnd::Link) => in_link = in_link.saturating_sub(1),
            _ => {}
        }
        match event {
            Event::Text(text) if in_code == 0 && in_link == 0 => {
                push_linked_text(&mut out, &text, mentions);
            }
            other => out.push(other),
        }
    }
    out
}

fn push_linked_text<'a>(out: &mut Vec<Event<'a>>, text: &str, mentions: &HashMap<u64, String>) {
    let spans = scan_number_mentions(text);
    if spans.is_empty() {
        out.push(Event::Text(CowStr::from(text.to_string())));
        return;
    }
    let mut last = 0;
    for (span, number) in &spans {
        let Some(href) = mentions.get(number) else {
            continue;
        };
        if span.start > last {
            out.push(Event::Text(CowStr::from(text[last..span.start].to_string())));
        }
        out.push(Event::Start(Tag::Link {
            link_type: LinkType::Inline,
            dest_url: CowStr::from(href.clone()),
            title: CowStr::Borrowed(""),
            id: CowStr::Borrowed(""),
        }));
        out.push(Event::Text(CowStr::from(text[span.clone()].to_string())));
        out.push(Event::End(TagEnd::Link));
        last = span.end;
    }
    if last < text.len() {
        out.push(Event::Text(CowStr::from(text[last..].to_string())));
    }
}

/// Byte spans of every `#<digits>` in `text`, with the number parsed. A `#`
/// glued to the end of a word (`abc#1`) is not a mention -- it is far more
/// often part of an identifier or a URL fragment than a reference.
pub(crate) fn scan_number_mentions(text: &str) -> Vec<(std::ops::Range<usize>, u64)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#'
            && i.checked_sub(1)
                .map(|p| bytes[p])
                .is_none_or(|b| !b.is_ascii_alphanumeric())
        {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 {
                if let Ok(number) = text[i + 1..j].parse::<u64>() {
                    spans.push((i..j, number));
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    spans
}

/// Buffers a fenced code block's text between its `Start`/`End` events (they
/// can arrive as more than one `Event::Text` for a single block) and, once
/// closed, emits one pre-rendered `Event::Html` in its place — everything
/// else passes through [`sanitize_event`] unchanged. The synthesized block's
/// HTML comes straight from `highlight::highlighted_lines_by_token`, which
/// (like the blob view's own highlighter) already HTML-escapes its input,
/// so it must NOT be run through `sanitize_event` — that would strip it as
/// if it were untrusted raw HTML from the README itself.
fn highlight_code_blocks<'a>(parser: impl Iterator<Item = Event<'a>>) -> Vec<Event<'a>> {
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None; // (language token, accumulated text)
    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) if !lang.trim().is_empty() => {
                current = Some((lang.trim().to_string(), String::new()));
            }
            Event::Text(text) if current.is_some() => {
                current.as_mut().expect("just checked is_some").1.push_str(&text);
            }
            Event::End(TagEnd::CodeBlock) if current.is_some() => {
                let (lang, code) = current.take().expect("just checked is_some");
                let lines = highlight::highlighted_lines_by_token(&lang, &code);
                let html = format!("<pre class=\"blob\"><code>{}</code></pre>", lines.join("\n"));
                out.push(Event::Html(CowStr::from(html)));
            }
            other => {
                if let Some(sanitized) = sanitize_event(other) {
                    out.push(sanitized);
                }
            }
        }
    }
    out
}

fn sanitize_event(event: Event<'_>) -> Option<Event<'_>> {
    match event {
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Link {
            link_type,
            dest_url: sanitize_url(dest_url),
            title,
            id,
        })),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Image {
            link_type,
            dest_url: sanitize_url(dest_url),
            title,
            id,
        })),
        other => Some(other),
    }
}

fn sanitize_url(url: CowStr<'_>) -> CowStr<'_> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    let safe = lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || !lower.contains(':'); // relative path or #fragment, no scheme at all
    if safe { url } else { CowStr::Borrowed("#") }
}
