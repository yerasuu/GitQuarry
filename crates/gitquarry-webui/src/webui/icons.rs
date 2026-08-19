//! Small inline-SVG icon set — hand-authored minimal shapes, not an
//! external icon font/library, so there's nothing to vendor or license.
//! Every icon is sized by the `.icon` CSS class (`1em`, so it scales with
//! the surrounding text-size toggle) rather than its own fixed pixel size,
//! and uses `currentColor` so it inherits whatever color context it's
//! dropped into — no new CSS variable needed, keeps the existing palette.
//!
//! Every icon here is always paired with visible adjacent text (a repo
//! name, a badge label, a button's own text) — never icon-only — so each
//! one is `aria-hidden="true" focusable="false"` rather than carrying its
//! own accessible name.
//!
//! Every shape element below uses an explicit `{}` body, never a bare `;`:
//! maud only omits the closing tag for `;`-terminated elements, which is
//! correct for genuine HTML void elements (`br`, `img`, ...) but `circle`/
//! `line`/`path`/`rect`/`polygon` aren't void — a `;` leaves them unclosed,
//! and every following sibling nests inside the previous shape instead of
//! becoming its sibling. Browsers then only paint the outermost shape;
//! everything nested inside it is invisible. `{}` makes maud emit the real
//! closing tag.

use maud::{Markup, html};

fn svg(body: Markup) -> Markup {
    html! {
        svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16"
            class="icon" aria-hidden="true" focusable="false" {
            (body)
        }
    }
}

pub fn folder() -> Markup {
    svg(html! {
        path fill="currentColor" d="M1.75 4.25c0-.69.56-1.25 1.25-1.25h3l1.25 1.5h5.75c.69 0 1.25.56 1.25 1.25v6.5c0 .69-.56 1.25-1.25 1.25h-10c-.69 0-1.25-.56-1.25-1.25z" {}
    })
}

pub fn file() -> Markup {
    svg(html! {
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"
            d="M4.25 2.25h4.5l3 3v8.25c0 .28-.22.5-.5.5h-7c-.28 0-.5-.22-.5-.5v-11c0-.28.22-.5.5-.5z" {}
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" d="M8.75 2.25v3h3" {}
    })
}

pub fn branch() -> Markup {
    svg(html! {
        path fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" d="M4 4.3v9.4" {}
        path fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" d="M4 7c0 2.2 2 3.3 4.2 3.3h.8" {}
        circle cx="4" cy="3" r="1.3" fill="currentColor" {}
        circle cx="9" cy="10.3" r="1.3" fill="currentColor" {}
        circle cx="4" cy="13" r="1.3" fill="currentColor" {}
    })
}

pub fn tag() -> Markup {
    svg(html! {
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"
            d="M1.5 3.5h5.2l7.3 7.3-5.2 5.2-7.3-7.3z" {}
        circle cx="5" cy="5.5" r="1" fill="currentColor" {}
    })
}

pub fn lock() -> Markup {
    svg(html! {
        rect x="3.5" y="7" width="9" height="6.5" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" {}
        path fill="none" stroke="currentColor" stroke-width="1.2" d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" {}
    })
}

pub fn fork() -> Markup {
    svg(html! {
        circle cx="4" cy="4" r="1.3" fill="currentColor" {}
        circle cx="12" cy="4" r="1.3" fill="currentColor" {}
        circle cx="8" cy="13" r="1.3" fill="currentColor" {}
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"
            d="M4 5.3v2a2 2 0 0 0 2 2h4a2 2 0 0 0 2-2v-2" {}
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" d="M8 9.3v2.4" {}
    })
}

pub fn star() -> Markup {
    svg(html! {
        polygon fill="currentColor"
            points="8,1.5 10.3,6.2 15.3,6.6 11.4,9.9 12.7,14.8 8,12.1 3.3,14.8 4.6,9.9 0.7,6.6 5.7,6.2" {}
    })
}

pub fn eye() -> Markup {
    svg(html! {
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"
            d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z" {}
        circle cx="8" cy="8" r="2" fill="none" stroke="currentColor" stroke-width="1.2" {}
    })
}

pub fn archive() -> Markup {
    svg(html! {
        rect x="2" y="3" width="12" height="3" rx="0.5" fill="none" stroke="currentColor" stroke-width="1.2" {}
        path fill="none" stroke="currentColor" stroke-width="1.2" d="M3 6.5v6a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-6" {}
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" d="M6.5 9h3" {}
    })
}

pub fn book() -> Markup {
    svg(html! {
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"
            d="M8 3.5c-1-.8-2.5-1.2-4-1.2-1 0-2 .15-2.5.4v9c.5-.25 1.5-.4 2.5-.4 1.5 0 3 .4 4 1.2 1-.8 2.5-1.2 4-1.2 1 0 2 .15 2.5.4v-9c-.5-.25-1.5-.4-2.5-.4-1.5 0-3 .4-4 1.2z" {}
        line x1="8" y1="3.5" x2="8" y2="12.7" stroke="currentColor" stroke-width="1.2" {}
    })
}

pub fn activity() -> Markup {
    svg(html! {
        path fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"
            d="M1.5 8.5h2.7l1.6-4.3 2.4 7.6 1.6-3.3h4.7" {}
    })
}

pub fn license() -> Markup {
    svg(html! {
        circle cx="8" cy="2" r="1" fill="currentColor" {}
        line x1="8" y1="2" x2="8" y2="13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" {}
        line x1="5" y1="13" x2="11" y2="13" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" {}
        line x1="2" y1="4" x2="14" y2="4" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" {}
        line x1="2" y1="4" x2="2" y2="7" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" {}
        line x1="14" y1="4" x2="14" y2="7" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" {}
        path fill="none" stroke="currentColor" stroke-width="1.1" d="M0.3 7c0 1.4 1.6 2.4 3.5 2.4S7.3 8.4 7.3 7" {}
        path fill="none" stroke="currentColor" stroke-width="1.1" d="M8.7 7c0 1.4 1.6 2.4 3.5 2.4s3.5-1 3.5-2.4" {}
    })
}

pub fn menu() -> Markup {
    svg(html! {
        line x1="2" y1="4" x2="14" y2="4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" {}
        line x1="2" y1="8" x2="14" y2="8" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" {}
        line x1="2" y1="12" x2="14" y2="12" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" {}
    })
}
