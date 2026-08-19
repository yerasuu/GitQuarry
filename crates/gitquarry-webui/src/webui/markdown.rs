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
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};

pub fn render(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let parser = Parser::new_ext(markdown, options);
    let events = highlight_code_blocks(parser);
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
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
