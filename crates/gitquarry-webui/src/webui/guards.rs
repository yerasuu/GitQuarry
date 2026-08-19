//! Shared auth guard: "is anyone logged in".

use super::escaping::url_encode;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};

/// `Ok(user)` for any logged-in session, admin or not — shared by every
/// `/_account/*` self-service section ([`crate::auth::web::security`],
/// [`crate::auth::web::favorites`], [`crate::auth::web::appearance`], ...)
/// that needs an identity to scope its action to, unlike
/// `crate::settings::web::require_admin` there's no "login not configured ->
/// stay open" exemption: self-service inherently needs a session.
/// `return_to_page` is the page to bounce back to after `/_login` (e.g.
/// `/_account/security`) — each caller's own path, not a client-submitted
/// one. No separate "is this the caller's own profile" guard exists
/// anymore: none of these URLs carry a `{username}` segment to check
/// against in the first place — see `crate::webui::account_settings_page`'s
/// own doc comment for why.
pub(crate) async fn require_login(
    ctx: &gitquarry_core::app::AppContext,
    headers: &HeaderMap,
    return_to_page: &str,
) -> Result<gitquarry_core::auth::entities::User, Response> {
    match crate::auth::web::login::current_user(ctx, headers).await {
        Some(user) => Ok(user),
        None => Err(Redirect::to(&format!("/_login?return_to={}", url_encode(return_to_page))).into_response()),
    }
}
