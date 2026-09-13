//! A one-line status message after a form submit (or the error page).
//!
//! Two ways in. A handler that renders its own page passes a [`Flash`] to
//! [`render_flash`] directly. A rejected mutation that redirects instead —
//! the shape a mutation should have, so the address bar ends up on a real
//! GET URL rather than on the POST target — goes through
//! [`super::error::AppError::flash_back`], which parks the reason in a
//! short-lived cookie ([`error_cookie`]) that [`super::flash_middleware`]
//! picks up, scopes into [`CURRENT_FLASH`] for
//! [`super::page_chrome::layout`] to render, and clears in that same
//! response so it shows exactly once.

/// Tagged so it renders as unambiguously ok/bad instead of every page
/// inventing its own "is this good or bad news" styling.
#[cfg(any(test, feature = "test-util"))]
pub enum Flash<'a> {
    Ok(&'a str),
    Err(&'a str),
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) enum Flash<'a> {
    Ok(&'a str),
    Err(&'a str),
}

/// `alert` (alongside this project's own `flash`/`flash-ok`/`flash-err`) is
/// there only so `styler`'s `[data-alert-dismiss]` click handler — which
/// looks for a `.closest('.alert')` — finds this element; the class adds no
/// styling of its own here; `flash`/`flash-ok`/`flash-err` (this project's
/// own, loaded after `styler`'s in `page_chrome::STYLE`) still fully own
/// the background/text colors, unchanged from before.
pub(crate) fn render_flash(flash: Option<Flash>, locale: gitquarry_i18n::Locale) -> String {
    let dismiss = gitquarry_i18n::shared::dismiss_button(locale);
    match flash {
        None => String::new(),
        Some(Flash::Ok(m)) => maud::html! {
            p class="alert flash flash-ok" {
                span { (m) }
                span class="alert-dismiss" data-alert-dismiss aria-label=(dismiss) { "×" }
            }
        }
        .into_string(),
        Some(Flash::Err(m)) => maud::html! {
            p class="alert flash flash-err" {
                span { (m) }
                span class="alert-dismiss" data-alert-dismiss aria-label=(dismiss) { "×" }
            }
        }
        .into_string(),
    }
}

/// Where a redirected flash lives between the POST and the GET that
/// displays it. Deliberately not a query parameter: a `?flash=` URL is a
/// link anyone can hand someone else, i.e. a way to put arbitrary text in
/// this instance's own alert styling.
pub(crate) const FLASH_COOKIE: &str = "gq_flash";

/// A message longer than this is truncated rather than dropped. Cookies cap
/// around 4KB in every browser, and a raw error chain (git's stderr, an S3
/// key) can run longer than that; the operator-facing detail is in the log
/// either way.
const MAX_MESSAGE: usize = 512;

tokio::task_local! {
    /// This request's incoming flash, if the previous response set one.
    /// Scoped by [`super::flash_middleware`] so `layout` can render it
    /// without every handler having to thread it through — same reason
    /// [`super::locale_pref::CURRENT_LOCALE`] exists.
    pub(crate) static CURRENT_FLASH: Option<(bool, String)>;
}

/// The `Set-Cookie` value that parks `message` as an error flash — what
/// [`super::error::AppError::flash_back`] attaches to its own redirect,
/// rather than going through [`redirect`] (it has a status and a location
/// of its own to set).
pub(crate) fn error_cookie(headers: &axum::http::HeaderMap, message: &str) -> String {
    let (_, cookie) = super::cookies::set_cookie(headers, FLASH_COOKIE, &encode(false, message), 30);
    cookie
}

/// `o`/`e` tag plus the percent-encoded message — cookie values can't hold
/// `;`, `,` or whitespace raw.
fn encode(ok: bool, message: &str) -> String {
    let truncated: String = message.chars().take(MAX_MESSAGE).collect();
    let encoded: String = form_urlencoded::byte_serialize(truncated.as_bytes()).collect();
    format!("{}{encoded}", if ok { "o" } else { "e" })
}

/// Decodes what [`error_cookie`] set, `true` in the tuple meaning "ok"
/// (nothing sets an ok flash across a redirect yet; the tag is there so
/// one can without a cookie-format change).
pub(crate) fn from_cookie(headers: &axum::http::HeaderMap) -> Option<(bool, String)> {
    let raw = super::cookies::get_cookie(headers, FLASH_COOKIE)?;
    let (tag, encoded) = raw.split_at_checked(1)?;
    let ok = match tag {
        "o" => true,
        "e" => false,
        _ => return None,
    };
    let decoded: String = form_urlencoded::parse(encoded.as_bytes())
        .map(|(k, v)| if v.is_empty() { k } else { v })
        .collect();
    (!decoded.is_empty()).then_some((ok, decoded))
}

/// Renders whatever [`super::flash_middleware`] scoped for this request —
/// what `layout` calls, so a redirected message lands on every page
/// without the handler that renders it knowing anything about flashes.
pub(crate) fn render_current(locale: gitquarry_i18n::Locale) -> String {
    CURRENT_FLASH
        .try_with(|flash| match flash {
            Some((true, m)) => render_flash(Some(Flash::Ok(m)), locale),
            Some((false, m)) => render_flash(Some(Flash::Err(m)), locale),
            None => String::new(),
        })
        .unwrap_or_default()
}
