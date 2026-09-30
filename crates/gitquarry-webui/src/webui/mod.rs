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
//! - [`page_chrome`]/[`mod@repo_link`]/[`flash`]/[`error`]/[`escaping`]/
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
//!   [`crate::auth::web::security`][] (`/_account/security` — password and
//!   2FA, the one place that changes how someone signs in;
//!   [`crate::auth::web::users`] below is the admin-only equivalent that
//!   can edit anyone's), [`crate::auth::web::keys`][] (`/_account/keys` —
//!   SSH keys for pushing, GPG keys for verifying what was pushed),
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
//!   [`gitquarry_core::auth::domain`] CRUD and `gitquarry_i18n::web::auth::*`
//!   copy) live under [`crate::auth`], not here, but are routed from this
//!   same table.
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

pub(crate) mod audit;
pub mod bootstrap;
pub(crate) mod changelog;
pub(crate) mod export;
pub(crate) mod fork;
pub(crate) mod health;
pub(crate) mod icons;
pub(crate) mod identicon;
pub(crate) mod language_tag;
pub(crate) mod license;
pub(crate) mod markdown;
pub(crate) mod remote_pulls;
pub(crate) mod remote_repo;
pub(crate) mod repo;
pub(crate) mod repo_label;
pub(crate) mod repo_new;
pub(crate) mod repo_settings;
pub(crate) mod repos;
pub(crate) mod watch;

mod actions;
mod compound_path;
mod cookies;
mod csrf;
mod density_pref;
mod diff_view;
mod error;
pub(crate) mod escaping;
pub(crate) mod federation_client;
mod federation_proxy;
mod flash;
mod guards;
mod locale_pref;
mod page_chrome;
pub(crate) mod raw;
mod raw_link;
mod repo_link;
mod text_size;
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
// `init_source_link`/`resolve_source_link_href` are `pub`, not `pub(crate)`,
// same cross-crate reason as `layout` above: `gitquarry-server`'s `serve::run`
// calls the former once at startup, right after resolving `ServerSettings`.
pub(crate) use page_chrome::{
    HeaderExtras, account_settings_page, admin_header, breadcrumbs, copy_button, federation_ui_active, field,
    header_bar, permission_badge_class, permission_label, settings_section, tab_links, topic_badges,
};
pub use page_chrome::{init_source_link, resolve_source_link_href};
pub(crate) use raw_link::{github_raw_link, raw_link};
pub(crate) use repo_label::repo_label;
pub(crate) use repo_link::{github_blob_link, repo_link};
pub(crate) use text_size::{TextSize, effective_text_size, handler_toggle_text_size, render_text_size_buttons};
pub(crate) use theme_pref::{effective_theme, handler_toggle_theme};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use gitquarry_core::app::AppContext;
use std::sync::Arc;
use tracing::info;

pub(crate) type State_ = axum::extract::State<Arc<AppContext>>;

/// Set by `style/app.js` on every fetch()-driven `data-async` form submit —
/// a handler that sees this returns just the freshly-rendered fragment for
/// that one form instead of its normal redirect/full-page response. A
/// request without it (a real `<form>` submit, or `curl -X POST`) gets the
/// original behavior unchanged: the async path is additive, never the only
/// way a toggle works.
pub(crate) const ASYNC_HEADER: &str = "x-gq-async";

/// Content baked in at compile time via `include_str!`, so its hash is
/// stable for the lifetime of the process — a strong `ETag` computed once
/// lets `static_asset_response` answer with `304 Not Modified` on every
/// repeat visit instead of re-sending the file, without needing a
/// content-hashed URL to stay cache-safe across deploys.
fn asset_etag(content: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    let digest = Sha256::digest(content.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2 + 2);
    hex.push('"');
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex.push('"');
    hex
}

/// Shared response shape for every static asset: honors `If-None-Match`
/// with a `304` (skipping the body entirely), otherwise serves the file with
/// a year-long `Cache-Control` — safe because a change to the file changes
/// its `ETag`, so a stale cache never serves stale content, it just costs
/// one conditional round-trip to notice.
///
/// `immutable` drops even that round-trip, and is only correct for an asset
/// whose *URL* carries its content hash (every asset below has one —
/// [`style_css_url`]/[`app_js_url`]/[`styler_js_url`]/[`hljs_js_url`]/
/// [`hljs_line_numbers_js_url`] — this used to be true only of the
/// stylesheet, which is why a plain fixed `/_assets/app.js`-style path once
/// needed `immutable: false` here; a script tag pinned to whatever build a
/// browser first saw is exactly the bug a hashed URL exists to avoid, once
/// one is in place there's no reason to still pay the round-trip).
/// [`favicon.svg`](handler_favicon_svg) is the one exception: its `<link
/// rel="icon">` has no query-string convention browsers respect the same
/// way, so it stays a fixed path, `immutable: false`.
fn static_asset_response(
    req_headers: &axum::http::HeaderMap,
    content_type: &'static str,
    content: &'static str,
    etag: &str,
    immutable: bool,
) -> Response {
    use axum::response::IntoResponse;
    if req_headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(etag)
    {
        return (
            axum::http::StatusCode::NOT_MODIFIED,
            [(axum::http::header::ETAG, etag.to_string())],
        )
            .into_response();
    }
    (
        [
            (axum::http::header::CONTENT_TYPE, content_type.to_string()),
            (axum::http::header::ETAG, etag.to_string()),
            (
                axum::http::header::CACHE_CONTROL,
                if immutable {
                    "public, max-age=31536000, immutable".to_string()
                } else {
                    "public, max-age=31536000".to_string()
                },
            ),
        ],
        content,
    )
        .into_response()
}

/// A route's fixed path plus its own content hash in the query string, so
/// one static route can still be served `immutable` (see
/// [`static_asset_response`]'s own doc comment): a changed asset is a
/// changed URL, so a repeat visit needs no conditional request at all,
/// unlike a fixed path that only shortens the round trip to a `304`.
/// Shared by every asset's own `_url()` function below — each just picks
/// its own path and content.
fn versioned_asset_url(path: &str, content: &str) -> String {
    let etag = asset_etag(content);
    let hash = etag.trim_matches('"');
    format!("{path}?v={}", &hash[..16])
}

/// `GET /_assets/style.css` — every page's stylesheet ([`page_chrome::STYLE`]),
/// served as a file rather than inlined into each response.
async fn handler_style_css(headers: axum::http::HeaderMap) -> Response {
    static ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| asset_etag(page_chrome::STYLE));
    static_asset_response(&headers, "text/css; charset=utf-8", page_chrome::STYLE, &ETAG, true)
}

/// The `<link href>` for [`handler_style_css`] — see [`versioned_asset_url`].
pub(crate) fn style_css_url() -> &'static str {
    static URL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| versioned_asset_url("/_assets/style.css", page_chrome::STYLE));
    &URL
}

/// `GET /_assets/favicon.svg` — the tab icon, the same mark
/// [`page_chrome::BRAND_MARK_PATH`] draws in the topbar, wrapped in a
/// standalone SVG document. Built here rather than vendored as its own
/// file so the mark has one definition; served as a file like every other
/// asset, since `img-src 'self'` (see [`security_headers`]) is what a
/// `<link rel="icon">` is checked against.
///
/// The fill is a literal colour, not `currentColor` and not the accent
/// custom property: nothing about the page's stylesheet reaches an icon the
/// browser paints in its own chrome. `--accent-dark-500` rather than the
/// light theme's `#8b0000`, because a tab strip is as often dark as light
/// and the lighter red is the one that stays legible on both.
async fn handler_favicon_svg(headers: axum::http::HeaderMap) -> Response {
    static CONTENT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><path fill=\"#e0574f\" d=\"{}\"/></svg>",
            page_chrome::BRAND_MARK_PATH
        )
    });
    static ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| asset_etag(&CONTENT));
    static_asset_response(&headers, "image/svg+xml", &CONTENT, &ETAG, false)
}

/// `GET /_assets/app.js` — this project's own hand-written client JS (see
/// `style/app.js`'s own doc comment). Served as an actual same-origin file,
/// not inlined into `<head>` the way `STYLE` is: `script-src 'self'` (see
/// [`security_headers`]) allows a same-origin *file*, not inline `<script>`
/// text, so unlike CSS this can't just be spliced into every page.
async fn handler_app_js(headers: axum::http::HeaderMap) -> Response {
    static ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| asset_etag(APP_JS));
    static_asset_response(&headers, "text/javascript; charset=utf-8", APP_JS, &ETAG, true)
}
const APP_JS: &str = include_str!("style/app.js");

/// The `<script src>` for [`handler_app_js`] — see [`versioned_asset_url`].
pub(crate) fn app_js_url() -> &'static str {
    static URL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| versioned_asset_url("/_assets/app.js", APP_JS));
    &URL
}

/// `GET /_assets/styler.js` — the pinned `styler` build vendored alongside
/// `style/vendor/styler.css` (see [`page_chrome::STYLE`]'s doc comment for
/// the tag/re-vendor story). Same serving shape as [`handler_app_js`]; a
/// second same-origin script file needs no CSP change, `script-src 'self'`
/// already covers it.
async fn handler_styler_js(headers: axum::http::HeaderMap) -> Response {
    static ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| asset_etag(STYLER_JS));
    static_asset_response(&headers, "text/javascript; charset=utf-8", STYLER_JS, &ETAG, true)
}
const STYLER_JS: &str = include_str!("style/vendor/styler.js");

/// The `<script src>` for [`handler_styler_js`] — see [`versioned_asset_url`].
pub(crate) fn styler_js_url() -> &'static str {
    static URL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| versioned_asset_url("/_assets/styler.js", STYLER_JS));
    &URL
}

/// `GET /_assets/hljs.js` — `highlight.js`, vendored the same way
/// [`handler_styler_js`] is (`scripts/vendor-hljs.sh` re-runs the fetch on a
/// version bump). Client-side syntax highlighting for the blob view and
/// every diff view — see `repo::blob::render_blob`'s and
/// `repo::components::color_diff`'s own doc comments for how each feeds it
/// `<code class="language-*">` fragments.
async fn handler_hljs_js(headers: axum::http::HeaderMap) -> Response {
    static ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| asset_etag(HLJS_JS));
    static_asset_response(&headers, "text/javascript; charset=utf-8", HLJS_JS, &ETAG, true)
}
const HLJS_JS: &str = include_str!("style/vendor/hljs.js");

/// The `<script src>` for [`handler_hljs_js`] — see [`versioned_asset_url`].
pub(crate) fn hljs_js_url() -> &'static str {
    static URL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| versioned_asset_url("/_assets/hljs.js", HLJS_JS));
    &URL
}

/// `GET /_assets/hljs-line-numbers.js` — the official `highlight.js`
/// companion plugin that turns the blob view's single tokenized block into
/// numbered, individually anchorable rows (`app.js`'s own doc comment on
/// where it's invoked) — the piece that lets `#L42` permalinks survive
/// highlighting a whole file as one unit instead of losing per-line
/// anchors the way tokenizing line-by-line would (splitting a multi-line
/// comment/string across separate highlight calls loses its own state).
async fn handler_hljs_line_numbers_js(headers: axum::http::HeaderMap) -> Response {
    static ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| asset_etag(HLJS_LINE_NUMBERS_JS));
    static_asset_response(
        &headers,
        "text/javascript; charset=utf-8",
        HLJS_LINE_NUMBERS_JS,
        &ETAG,
        true,
    )
}
const HLJS_LINE_NUMBERS_JS: &str = include_str!("style/vendor/hljs-line-numbers.js");

/// The `<script src>` for [`handler_hljs_line_numbers_js`] — see
/// [`versioned_asset_url`].
pub(crate) fn hljs_line_numbers_js_url() -> &'static str {
    static URL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| versioned_asset_url("/_assets/hljs-line-numbers.js", HLJS_LINE_NUMBERS_JS));
    &URL
}

/// `/_assets/*` alone: the stylesheet, scripts and icon every page's chrome
/// ([`layout`]) links. Stateless, so any router can mount it — the one
/// outside [`router`] being `gitquarry-server`'s setup wizard, a separate
/// short-lived server whose page uses the same chrome.
pub fn asset_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/_assets/style.css", get(handler_style_css))
        .route("/_assets/app.js", get(handler_app_js))
        .route("/_assets/favicon.svg", get(handler_favicon_svg))
        .route("/_assets/styler.js", get(handler_styler_js))
        .route("/_assets/hljs.js", get(handler_hljs_js))
        .route("/_assets/hljs-line-numbers.js", get(handler_hljs_line_numbers_js))
}

/// The whole route table, layers included, with nothing bound yet.
///
/// Split out of [`run_server`] so `tests/webui_routes.rs` can assert
/// properties of the table itself (every mutation target answering a stray
/// GET rather than axum's bare 405) without a listener, a bucket or a
/// storage backend.
pub fn router(ctx: Arc<AppContext>) -> Router {
    let app = Router::new()
        .merge(asset_routes())
        .route("/_bootstrap", get(bootstrap::handler_bootstrap_get))
        .route("/_healthz", get(health::handler_healthz))
        .route("/_metrics", get(health::handler_metrics))
        .route("/", get(repo::handler_index))
        .route(
            "/_settings/footer",
            get(crate::settings::web::handler_settings_footer_get)
                .post(crate::settings::web::handler_settings_footer_post),
        )
        .route(
            "/_settings/network",
            get(crate::settings::web::handler_settings_network_get)
                .post(crate::settings::web::handler_settings_network_post),
        )
        .route(
            "/_settings/mirrors-cache",
            get(crate::settings::web::handler_settings_mirrors_cache_get)
                .post(crate::settings::web::handler_settings_mirrors_cache_post),
        )
        .route(
            "/_settings/external-mirrors",
            get(crate::settings::web::handler_settings_external_mirrors_get)
                .post(crate::settings::web::handler_settings_external_mirrors_post),
        )
        .route(
            "/_settings/oidc",
            get(crate::settings::web::handler_settings_oidc_get).post(crate::settings::web::handler_settings_oidc_post),
        )
        .route(
            "/_settings/backup",
            get(crate::settings::web::handler_settings_backup_get)
                .post(crate::settings::web::handler_settings_backup_post),
        )
        .route(
            "/_settings/backup/run",
            post(crate::settings::web::handler_settings_backup_run_post),
        )
        .route(
            "/_users",
            get(crate::auth::web::users::handler_users_get).post(crate::auth::web::users::handler_users_post),
        )
        .route(
            "/_groups",
            get(crate::auth::web::groups::handler_groups_get).post(crate::auth::web::groups::handler_groups_post),
        )
        .route("/_group/{*path}", get(crate::auth::web::group::handler_group_get))
        .route("/_audit", get(audit::handler_audit_get))
        .route("/_changelog", get(changelog::handler_changelog_get))
        .route("/_license", get(license::handler_license_get))
        .route("/_search", get(crate::search::web::handler_search_get))
        .route(
            "/_fork/{*repo}",
            get(fork::handler_mutation_get).post(fork::handler_fork_post),
        )
        .route(
            "/_watch/{*repo}",
            get(watch::handler_mutation_get).post(watch::handler_watch_post),
        )
        .route("/_new", get(repo_new::handler_new_get).post(repo_new::handler_new_post))
        .route("/_repos", get(repos::handler_repos_get))
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
            "/_account/keys",
            get(crate::auth::web::keys::handler_keys_get).post(crate::auth::web::keys::handler_keys_post),
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
            "/_account/profile/emails",
            axum::routing::post(crate::auth::web::avatar::handler_emails_post),
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
        .route(
            "/_releases/create/{*repo}",
            get(repo::releases::handler_mutation_get).post(repo::releases::handler_create_post),
        )
        .route(
            "/_releases/generate_notes/{*repo}",
            get(repo::releases::handler_mutation_get).post(repo::releases::handler_generate_notes_post),
        )
        .route(
            "/_releases/edit/{*repo}",
            get(repo::releases::handler_mutation_get).post(repo::releases::handler_edit_post),
        )
        .route(
            "/_releases/delete/{*repo}",
            get(repo::releases::handler_mutation_get).post(repo::releases::handler_delete_post),
        )
        .route(
            "/_releases/asset_upload/{*repo}",
            get(repo::releases::handler_mutation_get).post(repo::releases::handler_upload_asset_post),
        )
        .route(
            "/_releases/asset_delete/{*repo}",
            get(repo::releases::handler_mutation_get).post(repo::releases::handler_delete_asset_post),
        )
        .route(
            "/_tags/create/{*repo}",
            get(repo::tags::handler_mutation_get).post(repo::tags::handler_create_post),
        )
        .route(
            "/_tags/delete/{*repo}",
            get(repo::tags::handler_mutation_get).post(repo::tags::handler_delete_post),
        )
        .route(
            "/_branches/delete/{*repo}",
            get(repo::branches::handler_mutation_get).post(repo::branches::handler_delete_post),
        );
    let app = app
        .route(
            "/_merges/create/{*repo}",
            get(repo::merges::handler_create_get).post(repo::merges::handler_create_post),
        )
        .route(
            "/_merges/bulk/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_bulk_post),
        )
        .route(
            "/_merges/close/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_close_post),
        )
        .route(
            "/_merges/reopen/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_reopen_post),
        )
        .route(
            "/_merges/comment/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_comment_post),
        )
        .route(
            "/_merges/comment_edit/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_comment_edit_post),
        )
        .route(
            "/_merges/comment_delete/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_comment_delete_post),
        )
        .route(
            "/_merges/review/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_review_post),
        )
        .route(
            "/_merges/merge/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_merge_post),
        )
        .route(
            "/_merges/edit_metadata/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_edit_metadata_post),
        )
        .route(
            "/_merges/mark_draft/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_mark_draft_post),
        )
        .route(
            "/_merges/mark_ready/{*repo}",
            get(repo::merges::handler_mutation_get).post(repo::merges::handler_mark_ready_post),
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

    app
        // Every mutation target (`/_merges`, `/_tags/delete`, ... 31 of them)
        // answers POST only, and a rejected submit re-renders in the POST
        // response rather than redirecting, so that URL is what stays in the
        // address bar. Reloading or backing into it would otherwise get
        // axum's bare 405 -- empty body, `Allow` header, and the catch-all
        // `GET /{*repo}` never consulted because the path itself did match.
        .method_not_allowed_fallback(|| async { axum::response::Redirect::to("/") })
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
        .layer(axum::middleware::from_fn(flash_middleware))
        .layer(axum::middleware::from_fn(security_headers))
        // Every HTML/CSS/JS response this app sends is compressible text
        // (`page_chrome::STYLE` alone inlines ~5000 lines of CSS into every
        // page) and none of it is pre-compressed, so this is a straight win
        // -- picks whichever of br/gzip/zstd the client's `Accept-Encoding`
        // offers.
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(Arc::clone(&ctx))
}

pub async fn run_server(bind_addr: &str, ctx: Arc<AppContext>) -> Result<()> {
    let app = router(Arc::clone(&ctx));

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("couldn't listen on {bind_addr}"))?;
    info!(
        "web UI on http://{bind_addr} -> bucket '{}' (mirrors in {})",
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
    // Computed once, up front, rather than lazily like `CURRENT_USER_MEMO`:
    // `AppError::into_response` (the one caller that actually needs this
    // task-local, see its own doc comment) has no `HeaderMap` to resolve it
    // from later, so it has to already be sitting in scope by the time any
    // handler further down the stack can fail.
    let locale = locale_pref::effective_locale(&ctx, req.headers()).await;
    // Nested alongside `REQUEST_BACKEND`: `login::CURRENT_USER_MEMO` memoizes
    // `login::current_user`'s decoded result the same "one request, never
    // longer" way, see its own doc comment for why that's safe.
    locale_pref::CURRENT_LOCALE
        .scope(
            locale,
            crate::auth::web::login::CURRENT_USER_MEMO.scope(
                std::cell::RefCell::new(None),
                gitquarry_core::app::REQUEST_BACKEND.scope(cached, next.run(req)),
            ),
        )
        .await
}

/// Carries a redirected flash across the redirect: reads the cookie
/// [`flash::redirect`] set, scopes it into [`flash::CURRENT_FLASH`] so
/// [`page_chrome::layout`] renders it on whatever page the browser landed
/// on, and clears the cookie in that same response so it shows exactly
/// once (a 30s `Max-Age` alone would show it again on the next page the
/// reader opened).
async fn flash_middleware(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let incoming = flash::from_cookie(req.headers());
    let clear = incoming
        .is_some()
        .then(|| cookies::set_cookie(req.headers(), flash::FLASH_COOKIE, "", 0));
    let mut response = flash::CURRENT_FLASH.scope(incoming, next.run(req)).await;
    if let Some((name, cookie)) = clear
        && let Ok(cookie) = axum::http::HeaderValue::from_str(&cookie)
    {
        response.headers_mut().append(name, cookie);
    }
    response
}

/// Response headers applied to every route, hand-rolled (`from_fn`, no
/// `tower-http`) since it's four header inserts, same "small enough to
/// write directly" call as [`cookies::get_cookie`]/[`cookies::set_cookie`].
/// Meaningful specifically because every script and every stylesheet this
/// app serves is a same-origin *file*: `script-src 'self'` means a
/// `<script>` tag smuggled past every [`esc`]/`maud` auto-escape still
/// couldn't execute — the browser refuses on policy, not on hope the markup
/// stayed clean. Same for an `onclick=` an injection might set, which no
/// nonce could ever have covered anyway. `tests/webui_csp.rs` walks `src/`
/// and fails if any view goes back to an inline script, handler or
/// `javascript:` URL. `style-src` is `'self'` alone, with
/// no `'unsafe-inline'`: no view sets a `style="..."` attribute any more —
/// bar widths, language colours and `display:inline` all resolve to classes
/// in `style/utilities.css` (see `components::width_class` and
/// `repo::summary::language_class`). That directive can't be relaxed by a
/// nonce either way, since CSP nonces cover `<style>` elements and never
/// style attributes, so the class file is what keeps it tight —
/// `tests/webui_csp.rs` fails if a view goes back to an attribute.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

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
    // Harmless to send even though this process itself only speaks plain
    // HTTP (`cookies.rs`'s own doc comment) -- a browser only honors HSTS
    // once it's reached this origin over HTTPS in the first place, which
    // means a TLS-terminating proxy is already in front, exactly the
    // deployment this project assumes.
    headers.insert(
        axum::http::HeaderName::from_static("strict-transport-security"),
        axum::http::HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("permissions-policy"),
        axum::http::HeaderValue::from_static("geolocation=(), camera=(), microphone=(), payment=(), usb=()"),
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
            CSP,
        ),
    );
    resp
}
