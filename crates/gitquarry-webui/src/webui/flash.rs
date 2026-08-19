//! A one-line status message after a form submit (or the error page).

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
