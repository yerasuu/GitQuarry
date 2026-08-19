//! Outbound calls to *this process's own* `gitquarry-core`'s loopback
//! `/_internal/*` API — never to another instance. Every call carries the
//! shared `internal_token` secret in the `X-Internal-Token` header,
//! matching the middleware `gitquarry-core`'s `internal_api` module
//! enforces (its own `artifacts`-scoped instance of that server, per
//! `docs/artifacts-concept.md`'s IPC design decision).

use anyhow::{Context, Result};
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct CoreClient {
    /// `gitquarry-artifacts.toml`'s `core_internal_url`, already including
    /// the `/_internal` path segment.
    base_url: String,
    internal_token: String,
    http: reqwest::Client,
}

/// Matches `gitquarry_core::storage::entities::OwnerRef`'s own wire shape
/// (`#[serde(tag = "kind", content = "uid", rename_all = "lowercase")]`) —
/// this crate never depends on `gitquarry-core`, so it re-declares the
/// shape it needs to serialize, same as `gitquarry_wire` types get
/// re-declared rather than shared when only one boundary uses them.
#[derive(Serialize)]
#[serde(tag = "kind", content = "uid", rename_all = "lowercase")]
pub enum OwnerRef<'a> {
    User(&'a str),
    Group(&'a str),
}

impl CoreClient {
    pub fn new(base_url: String, internal_token: String) -> Self {
        Self {
            base_url,
            internal_token,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url.trim_end_matches('/'))
    }

    /// `POST /_internal/artifacts/namespace-permission` — the one place a
    /// real access decision gets made; see
    /// `gitquarry_core::internal_api::handler_artifacts_namespace_permission`'s
    /// own doc comment. `require` is a lowercase string matching core's
    /// `auth::entities::Permission`'s serde representation
    /// (`"guest"`/`"reporter"`/`"developer"`/`"maintainer"`/`"owner"`).
    pub async fn namespace_permission(&self, username: &str, owner: &OwnerRef<'_>, require: &str) -> Result<bool> {
        #[derive(Serialize)]
        struct Req<'a> {
            username: &'a str,
            owner: &'a OwnerRef<'a>,
            require: &'a str,
        }
        #[derive(Deserialize)]
        struct Resp {
            ok: bool,
        }
        let url = self.url("/artifacts/namespace-permission");
        let resp: Resp = self
            .http
            .post(&url)
            .signed(&self.internal_token)
            .json(&Req {
                username,
                owner,
                require,
            })
            .send()
            .await
            .with_context(|| format!("couldn't reach core at {url}"))?
            .json()
            .await
            .with_context(|| format!("core's response from {url} wasn't valid JSON"))?;
        Ok(resp.ok)
    }
}
