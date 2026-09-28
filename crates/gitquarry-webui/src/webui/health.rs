//! `/_healthz` (liveness: this process is up and answering HTTP) and
//! `/_metrics` (Prometheus text exposition format, see
//! <https://prometheus.io/docs/instrumenting/exposition_formats/>) —
//! deliberately under the app's own `/_` prefix rather than the bare
//! `/healthz`/`/metrics` most tooling defaults to: every other reserved
//! top-level route in this router (`/_new`, `/_settings`, ...) uses that
//! prefix specifically so no real repo name can ever collide with one (see
//! `super::run_server`), and a health/metrics endpoint is no different. An
//! operator's scrape/probe config just needs to point at the `/_`-prefixed
//! path instead of the tool's default.
//!
//! No `prometheus` crate dependency: the exposition format is a handful of
//! plain text lines, same "own the ~20 lines instead of a whole crate" call
//! [`gitquarry_auth_crypto::totp`]'s own doc comment already makes for TOTP.

use super::State_;
use axum::extract::State;
use axum::response::IntoResponse;
use std::sync::LazyLock;
use std::time::Instant;

/// Process start time — one server process, one meaningful "uptime", no
/// need to thread this through `AppContext` for a single read site.
static STARTED_AT: LazyLock<Instant> = LazyLock::new(Instant::now);

pub(crate) async fn handler_healthz() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        render_healthz(),
    )
}

pub fn render_healthz() -> &'static str {
    "ok"
}

pub(crate) async fn handler_metrics(State(ctx): State_) -> impl IntoResponse {
    let uptime = STARTED_AT.elapsed().as_secs_f64();
    let locked_out = ctx.login_attempts.locked_count();
    let git_rate_limited = ctx.git_rate_limiter.limited_count();
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        render_metrics(uptime, locked_out, git_rate_limited),
    )
}

/// Deliberately carries no version or build label: this endpoint is
/// unauthenticated (a scraper has no session to present), and the running
/// version is the one piece here that is purely reconnaissance value to
/// somebody shopping for a known bug. Counters stay — an operator who
/// considers even those sensitive can restrict the path at the proxy.
///
/// The actual exposition-format text, kept apart from [`handler_metrics`] so
/// the handler stays thin — same split [`super::audit`]'s own
/// `render_audit_page` follows.
pub fn render_metrics(uptime_secs: f64, locked_out: usize, git_rate_limited: usize) -> String {
    format!(
        "\
# HELP gitquarry_up Always 1 while the process is answering HTTP requests.
# TYPE gitquarry_up gauge
gitquarry_up 1
# HELP gitquarry_uptime_seconds Seconds since this process started.
# TYPE gitquarry_uptime_seconds counter
gitquarry_uptime_seconds {uptime_secs}
# HELP gitquarry_login_penalised_active Usernames currently serving a password-login delay.
# TYPE gitquarry_login_penalised_active gauge
gitquarry_login_penalised_active {locked_out}
# HELP gitquarry_git_rate_limited_active IPs/usernames currently at the git clone/fetch rate limit ceiling.
# TYPE gitquarry_git_rate_limited_active gauge
gitquarry_git_rate_limited_active {git_rate_limited}
"
    )
}
