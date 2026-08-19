//! Read-only web UI (plus a settings page) over the same bucket.
//!
//! Everything coming in here is untrusted: the repo path, `?ref=`, `?path=`,
//! `?sha=`, `?q=` and `?fmt=` end up as filesystem paths, S3 keys, and `git`
//! arguments. They're validated in [`gitquarry_core::validate`] before use, and
//! everything printed goes through [`esc`].
//!
//! Markup: every page (including this file's own shared chrome — `layout`,
//! `header_bar`, `breadcrumbs`, ...) builds HTML with [`maud`]'s `html!`
//! macro: compile-time-checked well-formed markup, and every interpolated
//! value auto-escaped, so there's no `esc()` call to remember (or forget)
//! per field. [`esc`] itself still exists for the few spots that
//! deliberately stay outside `html!` — [`export`]'s atom feed is XML, not
//! HTML, and `repo::components::color_diff` hand-builds already-escaped
//! `<span>`-wrapped diff lines meant to be spliced into a `<pre>` via
//! `PreEscaped` rather than modeled as a maud element tree.
//!
//! Split by concern:
//! - this file: routing plus the two request-wide middlewares
//!   ([`request_cache_middleware`], [`security_headers`]).
//! - [`page_chrome`]/[`repo_link`]/[`flash`]/[`error`]/[`escaping`]/
//!   [`cookies`]/[`csrf`]/[`guards`]/[`actions`]/[`text_size`]/
//!   [`theme_pref`]/[`locale_pref`]/[`diff_view`]: shared render helpers and
//!   request-wide concerns every page needs, one file per concern (same
//!   layout [`gitquarry_core::git::browse`] uses for its own per-concern split).
//!   Every item any of these declares is re-exported here so every existing
//!   `super::xxx`/`crate::webui::xxx` call site keeps working unchanged.
//! - [`crate::auth::web::login`]: `/_login`, `/_login_oidc`, `/_callback`,
//!   `/_logout` — OIDC and local-password auth, session cookie,
//!   `current_user`/`topnav`.
//! - Every private self-service account page lives under `/_account/*`, one
//!   URL per sidebar section (`crate::webui::account_settings_page`) — no
//!   username in any of them, always the logged-in session's own account:
//!   [`crate::auth::web::security`][] (`/_account/security` — pubkeys/GPG
//!   keys/secondary emails/password/TOTP, the one place that changes how
//!   someone can authenticate as this user; [`crate::auth::web::users`]
//!   below is the admin-only equivalent that can edit anyone's),
//!   [`crate::auth::web::appearance`][] (`/_account/appearance` —
//!   text-size/theme/density), [`crate::auth::web::avatar`][]
//!   (`/_account/profile` — avatar upload/remove; also owns the public
//!   `GET /_avatar/<username>` image serve), [`crate::auth::web::favorites`][]
//!   (`/_account/favorites` — starred repos + visibility toggle; also owns
//!   the *public* `/_profile/<user>/favorites` read-only view, the one
//!   section reachable by someone other than the account owner),
//!   [`crate::auth::web::pat`][] (`/_account/pat` — local personal access
//!   tokens for git-smart-HTTP clone/push, scoped by explicit repo +
//!   [`gitquarry_core::auth::entities::Permission`] ceiling; separate from
//!   federation's own fine-grained tokens at `/_account/tokens` below, this
//!   one lives entirely in [`gitquarry_core::auth::domain::pat`], no
//!   `[federation]` config needed). All of these (plus their
//!   [`gitquarry_core::auth::domain`] CRUD and [`crate::auth::i18n`] copy)
//!   live under [`crate::auth`], not here, but are routed from this same
//!   table.
//! - `/_peers`, `/_account/tokens`, `/_remote_pulls/comment`, `/_api/*`,
//!   `/_federation/gossip/*`: federation layers 1/4 (linked-instance
//!   management, fine-grained tokens, PR-proxy, discovery) — all proxied to
//!   the separate `gitquarry-federation` process via [`federation_proxy`]
//!   (`docs/federation-binary-split-concept.md`), never handled in-process
//!   here. Federation is an optional, separately deployed component;
//!   without `[federation]` configured, every one of these routes just
//!   answers 503.
//! - [`fork`]: `POST /_fork`, copies a whole repo to a new one under the
//!   caller's own namespace or a group they own — open to any logged-in
//!   user, unlike [`repo_new`] which is admin-only.
//! - [`watch`]: `POST /_watch`, toggles the caller's watch on a repo — same
//!   shape as `render_star_button`'s star toggle, its own tiny route
//!   instead of a page of its own (watching has no list page yet).
//! - [`gitquarry_core::git::http`]: git smart-HTTP (`info/refs`, `git-upload-pack`,
//!   `git-receive-pack`) — the other publicly supported transport alongside
//!   `ssh://`. Lives under [`gitquarry_core::git`], not here (this module is
//!   browsing-UI only), but has to layer onto this router rather than own
//!   its own listener, since both share one HTTP port — see its own module
//!   doc for why.
//! - [`repo_new`]: `/_new`, creates an empty repo (manifest + initial
//!   config) so `git push` has somewhere to land with description/owner/
//!   visibility already set, instead of defaulting on first push.
//! - [`crate::auth::web::profile`]: `/_profile/<username>`, public list of
//!   that user's repos — the only `/_*` page with no login gate at all; what
//!   differs by viewer is which repos show up, not whether the page loads.
//! - [`crate::settings`]: server-wide settings — `.conf/global.json` — plus
//!   [`crate::settings::web::require_admin`], the gate every admin page uses.
//! - [`repo_settings`]: a single repo's description/owner/visibility —
//!   `.conf/<repo>/config.json`.
//! - [`crate::auth::web::users`]: the user/role admin page — SSH pubkeys,
//!   per-repo permissions, web password, `is_admin` — `.conf/access.json`.
//! - [`crate::auth::web::groups`]: shared namespaces (`<group>/<repo>`)
//!   whose members inherit a permission on every repo under the group
//!   without a per-repo grant — same file, `.conf/access.json`.
//! - [`audit`]: `/_audit`, read-only feed of [`gitquarry_core::storage::audit`]'s recent
//!   entries — who did what admin/repo action, and when.
//! - [`crate::search`]: `/_search`, cross-repo filename and commit-message search
//!   over every repo the request can see — same visibility rule as the
//!   index, not admin-gated.
//! - [`repo`]: the repo index and the per-repo view dispatcher — one file
//!   per view (summary, refs, tree, blob, log, commit, stats, blame) plus
//!   `repo::components` for render helpers 2+ views share.
//! - [`export`]: views that return something other than an HTML page (plain
//!   blob bytes, patch, rawdiff, snapshot archives, the atom feed).
//! - [`highlight`] / [`markdown`]: `syntect` and `pulldown-cmark` wrappers
//!   used by the blob and summary views respectively.
//! - [`icons`]: small hand-authored inline-SVG icon set, sized/colored by
//!   CSS (`1em`, `currentColor`) rather than a vendored icon font.
//! - [`identicon`]: deterministic per-username SVG avatar, computed on the
//!   fly — no upload, no third-party fetch (see its own doc for why).

pub mod audit;
pub mod export;
pub mod fork;
pub mod health;
pub mod highlight;
pub mod icons;
pub mod identicon;
pub mod markdown;
pub mod remote_pulls;
pub mod remote_repo;
pub mod repo;
pub mod repo_new;
pub mod repo_settings;
pub mod watch;

mod actions;
#[cfg(any(test, feature = "test-util"))]
pub mod compound_path;
#[cfg(not(any(test, feature = "test-util")))]
mod compound_path;
#[cfg(any(test, feature = "test-util"))]
pub mod cookies;
#[cfg(not(any(test, feature = "test-util")))]
mod cookies;
#[cfg(any(test, feature = "test-util"))]
pub mod csrf;
#[cfg(not(any(test, feature = "test-util")))]
mod csrf;
#[cfg(any(test, feature = "test-util"))]
pub mod density_pref;
#[cfg(not(any(test, feature = "test-util")))]
mod density_pref;
#[cfg(any(test, feature = "test-util"))]
pub mod diff_view;
#[cfg(not(any(test, feature = "test-util")))]
mod diff_view;
#[cfg(any(test, feature = "test-util"))]
pub mod error;
#[cfg(not(any(test, feature = "test-util")))]
mod error;
pub mod escaping;
pub(crate) mod federation_client;
#[cfg(any(test, feature = "test-util"))]
pub mod federation_proxy;
#[cfg(not(any(test, feature = "test-util")))]
mod federation_proxy;
#[cfg(any(test, feature = "test-util"))]
pub mod flash;
#[cfg(not(any(test, feature = "test-util")))]
mod flash;
#[cfg(any(test, feature = "test-util"))]
pub mod guards;
#[cfg(not(any(test, feature = "test-util")))]
mod guards;
#[cfg(any(test, feature = "test-util"))]
pub mod locale_pref;
#[cfg(not(any(test, feature = "test-util")))]
mod locale_pref;
#[cfg(any(test, feature = "test-util"))]
pub mod page_chrome;
#[cfg(not(any(test, feature = "test-util")))]
mod page_chrome;
pub(crate) mod raw;
#[cfg(any(test, feature = "test-util"))]
pub mod raw_link;
#[cfg(not(any(test, feature = "test-util")))]
mod raw_link;
#[cfg(any(test, feature = "test-util"))]
pub mod repo_link;
#[cfg(not(any(test, feature = "test-util")))]
mod repo_link;
#[cfg(any(test, feature = "test-util"))]
pub mod text_size;
#[cfg(not(any(test, feature = "test-util")))]
mod text_size;
#[cfg(any(test, feature = "test-util"))]
pub mod theme_pref;
#[cfg(not(any(test, feature = "test-util")))]
mod theme_pref;

pub(crate) use actions::{render_repo_action_buttons, star_button_html, watch_button_html};
pub(crate) use cookies::{get_cookie, set_cookie};
pub(crate) use csrf::{csrf_field, csrf_token, require_csrf, require_csrf_for};
pub(crate) use diff_view::{DiffView, effective_diff_view, handler_toggle_diff_view, render_diff_view_buttons};
pub(crate) use error::{AppError, Page};
// `esc`/`non_empty` are `pub`, not `pub(crate)`: `gitquarry-server`'s
// first-run setup wizard (`config::setup`, a different crate) renders its
// own tiny form page with these too.
pub use escaping::{esc, non_empty};
pub(crate) use escaping::{is_local_redirect, url_encode};
pub(crate) use flash::{Flash, render_flash};
pub(crate) use guards::require_login;
pub(crate) use locale_pref::effective_locale;
// `layout` is `pub`, not `pub(crate)`, for the same cross-crate reason as
// `esc`/`non_empty` above.
pub(crate) use density_pref::{effective_density, handler_toggle_density};
pub use page_chrome::layout;
pub(crate) use page_chrome::{
    HeaderExtras, account_settings_page, admin_header, breadcrumbs, copy_button, federation_ui_active, header_bar,
};
pub(crate) use raw_link::{github_raw_link, raw_link};
pub(crate) use repo_link::{github_blob_link, repo_link};
pub(crate) use text_size::{TextSize, effective_text_size, handler_toggle_text_size, render_text_size_buttons};
pub(crate) use theme_pref::{effective_theme, handler_toggle_theme};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use gitquarry_core::app::AppContext;
use std::sync::Arc;

pub(crate) type State_ = axum::extract::State<Arc<AppContext>>;

/// Set by `style/app.js` on every fetch()-driven `data-async` form submit —
/// a handler that sees this returns just the freshly-rendered fragment for
/// that one form instead of its normal redirect/full-page response. A
/// request without it (a real `<form>` submit, or `curl -X POST`) gets the
/// original behavior unchanged: the async path is additive, never the only
/// way a toggle works.
pub(crate) const ASYNC_HEADER: &str = "x-gq-async";

/// `GET /_assets/app.js` — this project's own hand-written client JS (see
/// `style/app.js`'s own doc comment). Served as an actual same-origin file,
/// not inlined into `<head>` the way `STYLE` is: `script-src 'self'` (see
/// [`security_headers`]) allows a same-origin *file*, not inline `<script>`
/// text, so unlike CSS this can't just be spliced into every page.
async fn handler_app_js() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("style/app.js"),
    )
}

/// `GET /_assets/styler.js` — the pinned `styler` build vendored alongside
/// `style/vendor/styler.css` (see [`page_chrome::STYLE`]'s doc comment for
/// the tag/re-vendor story). Same serving shape as [`handler_app_js`]; a
/// second same-origin script file needs no CSP change, `script-src 'self'`
/// already covers it.
async fn handler_styler_js() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("style/vendor/styler.js"),
    )
}

pub async fn run_server(bind_addr: &str, ctx: Arc<AppContext>) -> Result<()> {
    let app = Router::new()
        .route("/_assets/app.js", get(handler_app_js))
        .route("/_assets/styler.js", get(handler_styler_js))
        .route("/_healthz", get(health::handler_healthz))
        .route("/_metrics", get(health::handler_metrics))
        .route("/", get(repo::handler_index))
        .route(
            "/_settings",
            get(crate::settings::web::handler_settings_get).post(crate::settings::web::handler_settings_post),
        )
        .route(
            "/_users",
            get(crate::auth::web::users::handler_users_get).post(crate::auth::web::users::handler_users_post),
        )
        .route(
            "/_groups",
            get(crate::auth::web::groups::handler_groups_get).post(crate::auth::web::groups::handler_groups_post),
        )
        .route("/_audit", get(audit::handler_audit_get))
        .route("/_search", get(crate::search::web::handler_search_get))
        .route("/_fork", axum::routing::post(fork::handler_fork_post))
        .route("/_watch", axum::routing::post(watch::handler_watch_post))
        .route("/_new", get(repo_new::handler_new_get).post(repo_new::handler_new_post))
        .route(
            "/_profile/{username}",
            get(crate::auth::web::profile::handler_profile_get),
        )
        .route(
            "/_profile/{username}/favorites",
            get(crate::auth::web::favorites::handler_favorites_get),
        )
        .route(
            "/_account/security",
            get(crate::auth::web::security::handler_security_get)
                .post(crate::auth::web::security::handler_security_post),
        )
        .route(
            "/_account/pat",
            get(crate::auth::web::pat::handler_pat_get).post(crate::auth::web::pat::handler_pat_post),
        )
        .route(
            "/_account/favorites",
            get(crate::auth::web::favorites::handler_account_favorites_get)
                .post(crate::auth::web::favorites::handler_account_favorites_post),
        )
        .route(
            "/_account/appearance",
            get(crate::auth::web::appearance::handler_appearance_get)
                .post(crate::auth::web::appearance::handler_appearance_post),
        )
        .route(
            "/_account/profile",
            get(crate::auth::web::avatar::handler_account_profile_get)
                .post(crate::auth::web::avatar::handler_avatar_upload_post),
        )
        .route(
            "/_account/profile/delete",
            axum::routing::post(crate::auth::web::avatar::handler_avatar_delete_post),
        )
        .route("/_avatar/{username}", get(crate::auth::web::avatar::handler_avatar_get))
        .route("/_prefs/text_size", axum::routing::post(handler_toggle_text_size))
        .route("/_prefs/theme", axum::routing::post(handler_toggle_theme))
        .route("/_prefs/density", axum::routing::post(handler_toggle_density))
        .route("/_prefs/diff_view", axum::routing::post(handler_toggle_diff_view))
        .route(
            "/_login",
            get(crate::auth::web::login::handler_login).post(crate::auth::web::login::handler_login_local),
        )
        .route(
            "/_login_totp",
            get(crate::auth::web::login::handler_login_totp_get).post(crate::auth::web::login::handler_login_totp_post),
        )
        .route("/_releases", axum::routing::post(repo::releases::handler_create_post))
        .route(
            "/_releases/generate_notes",
            axum::routing::post(repo::releases::handler_generate_notes_post),
        )
        .route(
            "/_releases/edit",
            axum::routing::post(repo::releases::handler_edit_post),
        )
        .route(
            "/_releases/delete",
            axum::routing::post(repo::releases::handler_delete_post),
        )
        .route(
            "/_releases/assets",
            axum::routing::post(repo::releases::handler_upload_asset_post),
        )
        .route(
            "/_releases/assets/delete",
            axum::routing::post(repo::releases::handler_delete_asset_post),
        )
        .route("/_tags", axum::routing::post(repo::tags::handler_create_post))
        .route("/_tags/delete", axum::routing::post(repo::tags::handler_delete_post))
        .route(
            "/_branches/delete",
            axum::routing::post(repo::branches::handler_delete_post),
        );
    let app = app
        .route("/_pulls", axum::routing::post(repo::pulls::handler_create_post))
        .route("/_pulls/bulk", axum::routing::post(repo::pulls::handler_bulk_post))
        .route("/_pulls/close", axum::routing::post(repo::pulls::handler_close_post))
        .route("/_pulls/reopen", axum::routing::post(repo::pulls::handler_reopen_post))
        .route(
            "/_pulls/comment",
            axum::routing::post(repo::pulls::handler_comment_post),
        )
        .route(
            "/_pulls/comment/edit",
            axum::routing::post(repo::pulls::handler_comment_edit_post),
        )
        .route(
            "/_pulls/comment/delete",
            axum::routing::post(repo::pulls::handler_comment_delete_post),
        )
        .route("/_pulls/review", axum::routing::post(repo::pulls::handler_review_post))
        .route("/_pulls/merge", axum::routing::post(repo::pulls::handler_merge_post))
        .route(
            "/_pulls/edit_metadata",
            axum::routing::post(repo::pulls::handler_edit_metadata_post),
        )
        .route(
            "/_pulls/mark_draft",
            axum::routing::post(repo::pulls::handler_mark_draft_post),
        )
        .route(
            "/_pulls/mark_ready",
            axum::routing::post(repo::pulls::handler_mark_ready_post),
        )
        .route("/_login_oidc", get(crate::auth::web::login::handler_login_oidc))
        .route("/_callback", get(crate::auth::web::login::handler_callback))
        .route("/_logout", axum::routing::post(crate::auth::web::login::handler_logout))
        .route("/_remote/{peer}/{*repo}", get(remote_repo::handler_remote_repo))
        // Raw-export views: their own route tree, deliberately not a
        // `?view=` query param on `/{*repo}` below — see `raw`'s module doc.
        .route("/_raw/plain/{*repo}", get(raw::handler_raw_plain))
        .route("/_raw/patch/{*repo}", get(raw::handler_raw_patch))
        .route("/_raw/rawdiff/{*repo}", get(raw::handler_raw_rawdiff))
        .route("/_raw/snapshot/{*repo}", get(raw::handler_raw_snapshot))
        .route("/_raw/atom/{*repo}", get(raw::handler_raw_atom))
        .route("/_raw/release_asset/{*repo}", get(raw::handler_raw_release_asset))
        .route(
            "/{*repo}",
            get(repo::handler_repo).post(repo_settings::handler_repo_conf_post),
        )
        // Every route below only ever does anything if `[federation]` is
        // configured (`ctx.federation.is_some()`) — each handler checks
        // that itself and answers 503 otherwise (`federation_proxy`'s own
        // `federation_unavailable()`). Federation is an optional, separately
        // deployed component (`docs/federation-binary-split-concept.md`):
        // without it, these routes simply don't do anything, same as any
        // other feature this instance hasn't turned on.
        .route(
            "/_peers",
            get(federation_proxy::proxy_peers).post(federation_proxy::proxy_peers),
        )
        .route(
            "/_account/tokens",
            get(federation_proxy::proxy_tokens).post(federation_proxy::proxy_tokens),
        )
        .route(
            "/_remote_pulls/comment",
            axum::routing::post(federation_proxy::proxy_remote_comment),
        )
        .route("/_api/instance", get(federation_proxy::proxy_public))
        .route("/_api/repos", get(federation_proxy::proxy_public))
        .route("/_api/repos/{*name}", get(federation_proxy::proxy_public))
        .route(
            "/_api/pulls",
            get(federation_proxy::proxy_public).post(federation_proxy::proxy_public),
        )
        .route("/_api/pulls/detail", get(federation_proxy::proxy_public))
        .route(
            "/_api/pulls/comment",
            axum::routing::post(federation_proxy::proxy_public),
        )
        .route("/_api/pulls/merge", axum::routing::post(federation_proxy::proxy_public))
        .route("/_api/banned-peers", get(federation_proxy::proxy_public))
        .route(
            "/_federation/gossip/announce",
            axum::routing::post(federation_proxy::proxy_public),
        )
        .route("/_federation/gossip/peers", get(federation_proxy::proxy_public));

    let app = app
        // Runs before the routes above ever see the request: short-circuits
        // the three git-smart-HTTP paths (see `gitquarry_core::git::http`'s module
        // doc for why this has to be a layer, not a route), passes
        // everything else through unchanged.
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            gitquarry_core::git::http::middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            request_cache_middleware,
        ))
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(Arc::clone(&ctx));

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("couldn't listen on {bind_addr}"))?;
    eprintln!(
        "gitquarry: web UI on http://{bind_addr} -> bucket '{}' (mirrors in {})",
        ctx.bucket,
        ctx.mirrors_root.display()
    );
    // `with_connect_info` so `federation_proxy`'s handlers can extract the
    // real client `SocketAddr` (via `ConnectInfo`) and forward it to
    // federation as `X-Forwarded-For` — federation's own loopback bind
    // otherwise only ever sees *this* process's address, never the actual
    // caller's, since it's never reachable except through this proxy (see
    // `federation_proxy`'s own module doc comment).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .context("error serving the web UI")?;
    Ok(())
}

/// Wraps the whole request in a fresh per-request `s3db::RequestCache`
/// (scoped via [`gitquarry_core::app::REQUEST_BACKEND`]), so [`AppContext::backend`]
/// serves repeat reads of the same key — the logged-in session's own
/// `User`, `ServerSettings`, a repo's `RepoConfig`, ... — from memory
/// instead of hitting S3 again on every one of the many independent
/// `login::current_user`/`effective_locale`/`effective_theme`/... calls a
/// single page render makes. Dropped (and its cached reads with it) the
/// moment this request finishes, same "never outlive one request" doctrine
/// `RequestCache`'s own doc comment lays out — a stale hint surviving past
/// its request could let a revoked permission stay "live" too long, and
/// gitquarry runs multiple instances against the same bucket regardless.
async fn request_cache_middleware(
    State(ctx): State_,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let cached: std::sync::Arc<dyn gitquarry_core::storage::Backend> =
        std::sync::Arc::new(s3db::RequestCache::new(std::sync::Arc::clone(&ctx.backend)));
    gitquarry_core::app::REQUEST_BACKEND.scope(cached, next.run(req)).await
}

/// Response headers applied to every route, hand-rolled (`from_fn`, no
/// `tower-http`) since it's four header inserts, same "small enough to
/// write directly" call as [`cookies::get_cookie`]/[`cookies::set_cookie`].
/// Meaningful specifically because this app ships zero JavaScript (see the
/// module doc comment): `script-src 'none'` means even a hypothetical
/// escaping bug that smuggled a `<script>` tag past every [`esc`]/`maud`
/// auto-escape still couldn't execute it — the browser refuses on policy,
/// not on hope the markup stayed clean. `style-src` needs `'unsafe-inline'`:
/// plenty of views set a `style="..."` attribute directly (widths in
/// `repo::stats`'s bar chart, `display:inline` on same-row forms), not
/// worth threading a nonce through every render function to avoid it.
async fn security_headers(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    // Checked before the request is consumed. A response rendered for a
    // logged-in session can carry things no shared cache should keep — a
    // private repo's tree, `/_users`, `/_settings` — and this app sets no
    // cache headers at all otherwise, which leaves browsers and any
    // intermediary free to heuristically cache them. Keyed on "is there a
    // session cookie" rather than on the route, so a page that becomes
    // sensitive later can't be forgotten here; anonymous responses (the
    // public browse surface this project is largely about) stay cacheable.
    let authenticated = cookies::get_cookie(req.headers(), crate::auth::web::login::SESSION_COOKIE).is_some();
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    if authenticated {
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
    }
    headers.insert(
        axum::http::HeaderName::from_static("x-content-type-options"),
        axum::http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-frame-options"),
        axum::http::HeaderValue::from_static("DENY"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("referrer-policy"),
        axum::http::HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("content-security-policy"),
        axum::http::HeaderValue::from_static(
            // `script-src 'self'`, not `'none'` anymore: `/_assets/app.js`
            // (served same-origin, no inline `<script>`, no `'unsafe-inline'`
            // added here) is the one piece of JS in this app, progressively
            // enhancing a handful of toggle forms — see that file's own doc
            // comment. Deliberately narrower than `'unsafe-inline'` would be:
            // a same-origin-file-only policy still blocks any injected
            // inline `<script>` a hypothetical escaping bug let through.
            // `img-src` needs `data:` too: `vendor/styler.css`'s own select-
            // chevron/checkmark icons are `data:image/svg+xml` background-
            // images baked into that pinned, hand-vetted stylesheet, not
            // anything request- or user-influenced — same trust level as the
            // CSS file itself, which `style-src 'self'` already accepts.
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    resp
}
