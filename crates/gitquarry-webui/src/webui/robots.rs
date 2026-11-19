//! `noindex` for every page of an `Unlisted` repo: an `X-Robots-Tag` header
//! on the response and a `<meta name="robots">` in its `<head>`. Kept out
//! of the listings isn't enough on its own — a crawler that finds the URL
//! anywhere else (a pasted link, a referrer) would otherwise index it.
//!
//! The repo handlers learn the visibility long after the response shape is
//! fixed and through a dozen different paths, so instead of threading a flag
//! through each of them, the two choke points every repo page passes
//! ([`super::repo::can_view_repo`] and [`super::page_chrome::header_bar`])
//! call [`mark_unlisted`], [`super::page_chrome::layout`] reads
//! [`is_unlisted`] for the meta tag, and [`middleware`] adds the header —
//! same request-scoped `task_local` shape as [`super::flash::CURRENT_FLASH`].
//! A marker set inside a task spawned off the request (a `JoinSet`) is lost,
//! which is why listings, which do fan out, never call it.

use axum::http::{HeaderName, HeaderValue};
use axum::response::Response;
use std::cell::Cell;

/// Both header and meta tag use this one value: `nofollow` too, since an
/// unlisted repo's pages link to its other pages and to whatever its README
/// links to, none of which the owner asked to have crawled.
pub(crate) const NOINDEX: &str = "noindex, nofollow";

tokio::task_local! {
    static UNLISTED: Cell<bool>;
}

/// Marks the current request as showing an `Unlisted` repo. A no-op outside
/// [`middleware`]'s scope (a handler called directly, a spawned task).
pub(crate) fn mark_unlisted() {
    let _ = UNLISTED.try_with(|flag| flag.set(true));
}

pub(crate) fn is_unlisted() -> bool {
    UNLISTED.try_with(Cell::get).unwrap_or(false)
}

pub(crate) async fn middleware(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    UNLISTED
        .scope(Cell::new(false), async move {
            let mut response = next.run(req).await;
            if is_unlisted() {
                response.headers_mut().insert(
                    HeaderName::from_static("x-robots-tag"),
                    HeaderValue::from_static(NOINDEX),
                );
            }
            response
        })
        .await
}
