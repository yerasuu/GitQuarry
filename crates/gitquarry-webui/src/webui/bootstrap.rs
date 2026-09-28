//! The first-boot gate. Until an instance has a login method configured,
//! `settings::web::require_admin` and `repo_settings::require_repo_authority`
//! have nothing to check a session against — that's the bootstrap window,
//! and it used to leave `/_settings`, `/_users`, `/_audit`, `/_new` and
//! every repo's settings tab open to anyone who could reach the port.
//!
//! Now that window needs [`TOKEN`]: a per-process secret printed to stderr
//! at startup (see [`print_hint`]), exchanged once at `/_bootstrap?token=…`
//! for a short-lived sealed cookie that the two gates accept in place of an
//! admin session. Deliberately not a config value: nothing to leave lying
//! in a file, and a restart invalidates it. Once the first password user
//! exists, `login_enforced` is true and none of this is consulted again.

use super::cookies::{get_cookie, set_cookie};
use super::error::AppError;
use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use gitquarry_core::app::AppContext;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

pub(crate) const BOOTSTRAP_COOKIE: &str = "bootstrap-grant";

/// How long one exchange of [`TOKEN`] stays usable. Long enough to create
/// the first admin account without racing a clock, short enough that a
/// forgotten browser tab isn't a standing key to the instance.
const GRANT_TTL_SECS: i64 = 3600;

/// This process's bootstrap secret — 192 bits of CSPRNG, hex. Generated on
/// first read rather than at startup so a fully bootstrapped instance never
/// mints one at all.
pub static TOKEN: LazyLock<String> = LazyLock::new(|| {
    let raw: [u8; 24] = rand::random();
    raw.iter().fold(String::with_capacity(48), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
});

#[derive(Serialize, Deserialize)]
struct Grant {
    /// The real deadline, same reasoning as `auth::web::login`'s cookies:
    /// `Max-Age` only binds a cooperating browser.
    exp: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Constant-time compare — the token is high-entropy, but the submitted
/// half is attacker-chosen and a byte-at-a-time short circuit costs nothing
/// to avoid (same call `csrf::ct_eq` makes).
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `true` if this request carries a live, correctly sealed bootstrap grant.
/// Only ever consulted while `login::login_enforced` is false — a grant is
/// no substitute for an admin session on a configured instance.
pub(crate) fn holds_grant(ctx: &AppContext, headers: &HeaderMap) -> bool {
    let Some(raw) = get_cookie(headers, BOOTSTRAP_COOKIE) else {
        return false;
    };
    let Ok(sealed) = B64.decode(raw) else { return false };
    let Ok(plain) = ctx.cipher.open(BOOTSTRAP_COOKIE, &sealed) else {
        return false;
    };
    let Ok(grant) = serde_json::from_slice::<Grant>(&plain) else {
        return false;
    };
    grant.exp >= now_secs()
}

/// `GET /_bootstrap?token=…`: trades the printed token for the grant cookie
/// the two admin gates accept. Refuses outright once a login method exists,
/// so a token that leaked into a shell history can't reopen a bootstrapped
/// instance.
pub(crate) async fn handler_bootstrap_get(
    State(ctx): State<std::sync::Arc<AppContext>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let locale = super::effective_locale(&ctx, &headers).await;
    if crate::auth::web::login::login_enforced(&ctx).await {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::settings::bootstrap_already_done(locale)
        )));
    }
    let submitted = params.get("token").map(String::as_str).unwrap_or("");
    if !ct_eq(submitted, &TOKEN) {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::settings::bootstrap_token_invalid(locale)
        )));
    }

    let grant = Grant {
        exp: now_secs().saturating_add(GRANT_TTL_SECS as u64),
    };
    let sealed = ctx.cipher.seal(
        BOOTSTRAP_COOKIE,
        &serde_json::to_vec(&grant)?,
        gitquarry_core::crypto::CompressionStrategy::None,
    )?;
    let (header, value) = set_cookie(&headers, BOOTSTRAP_COOKIE, &B64.encode(sealed), GRANT_TTL_SECS);
    let mut resp = Redirect::to("/_users").into_response();
    resp.headers_mut().insert(
        header,
        value
            .parse()
            .context(gitquarry_i18n::shared::auth::building_set_cookie(locale))?,
    );
    Ok(resp)
}

/// Prints the one-time bootstrap URL when — and only when — the instance
/// still has no login method configured. Called once from
/// `gitquarry-server`'s startup, after settings/OIDC are resolved.
pub async fn print_hint(ctx: &AppContext, web_bind: &str) {
    if crate::auth::web::login::login_enforced(ctx).await {
        return;
    }
    info!(
        "no login method configured yet — the admin pages are closed until you visit \
         http://{web_bind}/_bootstrap?token={} to create the first admin account; \
         this token dies with the process",
        *TOKEN
    );
}
