//! The handler error type every fallible route returns.

use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

/// A handler error, with its matching status. Defaults to 500; anything
/// coming from validating client input is marked 400 and anything missing
/// 404, so client mistakes don't get reported as server failures.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    error: anyhow::Error,
}
#[cfg(not(any(test, feature = "test-util")))]
#[derive(Debug)]
pub(crate) struct AppError {
    status: StatusCode,
    error: anyhow::Error,
}

impl AppError {
    pub(crate) fn bad_request(error: anyhow::Error) -> Self {
        AppError {
            status: StatusCode::BAD_REQUEST,
            error,
        }
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn not_found(error: anyhow::Error) -> Self {
        AppError {
            status: StatusCode::NOT_FOUND,
            error,
        }
    }
    #[cfg(not(any(test, feature = "test-util")))]
    pub(crate) fn not_found(error: anyhow::Error) -> Self {
        AppError {
            status: StatusCode::NOT_FOUND,
            error,
        }
    }

    pub(crate) fn forbidden(error: anyhow::Error) -> Self {
        AppError {
            status: StatusCode::FORBIDDEN,
            error,
        }
    }

    /// For the `webui::raw` routes (plain/patch/rawdiff/snapshot/atom/
    /// release_asset): those promise a non-HTML body on success, so an
    /// error must stay plain text too instead of falling back to the
    /// normal HTML error page — otherwise a client parsing raw bytes gets
    /// blindsided by markup on the one path where it can't happen.
    #[cfg(any(test, feature = "test-util"))]
    pub fn into_plain_response(self) -> Response {
        if self.status.is_server_error() {
            eprintln!("gitquarry: UI web: {:#}", self.error);
        }
        let msg = self.client_message(gitquarry_i18n::Locale::DEFAULT);
        (self.status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], msg).into_response()
    }
    #[cfg(not(any(test, feature = "test-util")))]
    pub(crate) fn into_plain_response(self) -> Response {
        if self.status.is_server_error() {
            eprintln!("gitquarry: UI web: {:#}", self.error);
        }
        let msg = self.client_message(gitquarry_i18n::Locale::DEFAULT);
        (self.status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], msg).into_response()
    }
}

impl AppError {
    /// What the client is actually told, as opposed to what gets logged.
    ///
    /// A 4xx/503 message was written by this codebase for the person
    /// reading it — a bad rev, a failed validation, a permission refusal,
    /// "federation isn't configured" — and goes out verbatim; that's the
    /// whole point of marking those statuses explicitly.
    ///
    /// A 5xx is the opposite: the blanket `From<E> for AppError` below
    /// means *any* stray error becomes one, so the chain is whatever
    /// happened to bubble up — git's stderr (which quotes `GIT_DIR` paths),
    /// an S3 key, a bucket name, a backend endpoint URL. The full chain
    /// still reaches the operator through the `eprintln!` next to every
    /// call of this; the client gets a fixed sentence.
    fn client_message(&self, locale: gitquarry_i18n::Locale) -> String {
        if self.status.is_server_error() {
            gitquarry_i18n::shared::internal_error(locale)
        } else {
            format!("error: {:#}", self.error)
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            eprintln!("gitquarry: UI web: {:#}", self.error);
        }
        // No `HeaderMap` reaches this conversion, so the explicit theme/
        // density cookies can't be read here — falls back to
        // `prefers-color-scheme` and the dense default, same as a
        // first-ever visit would. Locale doesn't have that problem:
        // `super::locale_pref::CURRENT_LOCALE` was already resolved (cookie,
        // then account default) by `webui::request_cache_middleware` before
        // this handler ever ran — `try_with` rather than `with` since a test
        // or background path constructing an `AppError` outside that
        // middleware's scope has nothing to read, and falls back to
        // `Locale::DEFAULT` same as before.
        let locale = super::locale_pref::CURRENT_LOCALE
            .try_with(|locale| *locale)
            .unwrap_or(gitquarry_i18n::Locale::DEFAULT);
        let msg = self.client_message(locale);
        let body = super::page_chrome::layout(
            "error",
            "",
            &super::flash::render_flash(Some(super::flash::Flash::Err(&msg)), locale),
            "",
            "",
            "",
            locale,
        );
        (self.status, Html(body)).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: e.into(),
        }
    }
}

pub(crate) type Page = Result<Html<String>, AppError>;
