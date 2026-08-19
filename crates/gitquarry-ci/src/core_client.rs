//! Outbound calls to *this process's own* `gitquarry-core`'s loopback
//! `/_internal/*` API — never to another instance. Every call carries the
//! shared `internal_token` secret in the `X-Internal-Token` header,
//! matching the middleware `gitquarry-core`'s `internal_api` module
//! enforces (its own `ci`-scoped instance of that server, per
//! `docs/ci-cd-concept.md`'s IPC design decision).

use anyhow::{Context, Result, bail};
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use serde::Deserialize;

#[derive(Clone)]
pub struct CoreClient {
    /// `gitquarry-ci.toml`'s `core_internal_url`, already including the
    /// `/_internal` path segment (e.g. `http://127.0.0.1:8091/_internal`).
    base_url: String,
    internal_token: String,
    http: reqwest::Client,
}

/// What a pipeline run needs to know before it can check anything out —
/// `docs/ci-cd-concept.md`'s Phase 1 scope: enough to identify the repo and
/// its current head, not yet a trigger payload (no consumer registered on
/// core's event bus for this crate yet).
#[derive(Debug, Deserialize)]
pub struct CiRepoInfo {
    pub full_name: String,
    pub default_branch: Option<String>,
    pub head_sha: Option<String>,
    /// `RepoConfig::ci_deploy_gate_disabled` — see `graph::gate_status`.
    pub ci_deploy_gate_disabled: bool,
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

    /// `GET /_internal/ci/repo?uid=` — see `gitquarry_core::internal_api`'s
    /// own `handler_ci_repo_info` doc comment for why this call is
    /// deliberately visibility-unfiltered (unlike federation's equivalent
    /// public-repo lookups).
    pub async fn repo_info(&self, uid: &str) -> Result<Option<CiRepoInfo>> {
        let url = self.url(&format!("/ci/repo?uid={uid}"));
        let resp = self
            .http
            .get(&url)
            .signed(&self.internal_token)
            .send()
            .await
            .with_context(|| format!("couldn't reach core at {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("core at {url} returned {}", resp.status());
        }
        resp.json()
            .await
            .map(Some)
            .with_context(|| format!("core's response from {url} wasn't valid JSON"))
    }
}
