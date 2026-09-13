//! Outbound calls to *this process's own* `gitquarry-core`'s loopback
//! `/_internal/*` API — never to another instance. Every call carries the
//! shared `internal_token` secret in the `X-Internal-Token` header, same
//! shape `gitquarry-ci`/`gitquarry-federation`'s own `core_client.rs`
//! already establish.

use anyhow::{Context, Result, bail};
use gitquarry_wire::MergeRequestMeta;
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use serde::Deserialize;

#[derive(Clone)]
pub struct CoreClient {
    /// `gitquarry-tracker.toml`'s `core_internal_url`, already including the
    /// `/_internal` path segment (e.g. `http://127.0.0.1:8091/_internal`).
    base_url: String,
    internal_token: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct RepoLabelsResponse {
    labels: Vec<String>,
}

#[derive(Deserialize)]
struct UserExistsResponse {
    exists: bool,
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

    fn get_request(&self, url: &str) -> reqwest::RequestBuilder {
        self.http.get(url).signed(&self.internal_token)
    }

    fn post_request(&self, url: &str) -> reqwest::RequestBuilder {
        self.http.post(url).signed(&self.internal_token)
    }

    /// `GET /_internal/repos/labels?uid=` — the label-name vocabulary
    /// `domain::issue::edit` validates against.
    pub async fn repo_labels(&self, uid: &str) -> Result<Vec<String>> {
        let url = self.url(&format!("/repos/labels?uid={uid}"));
        let resp = self
            .get_request(&url)
            .send()
            .await
            .with_context(|| format!("couldn't reach core at {url}"))?;
        if !resp.status().is_success() {
            bail!("core at {url} returned {}", resp.status());
        }
        let body: RepoLabelsResponse = resp
            .json()
            .await
            .with_context(|| format!("core's response from {url} wasn't valid JSON"))?;
        Ok(body.labels)
    }

    /// `GET /_internal/users/exists?username=` — the assignee-exists check
    /// `domain::issue::edit` needs.
    pub async fn user_exists(&self, username: &str) -> Result<bool> {
        let url = self.url(&format!("/users/exists?username={username}"));
        let resp = self
            .get_request(&url)
            .send()
            .await
            .with_context(|| format!("couldn't reach core at {url}"))?;
        if !resp.status().is_success() {
            bail!("core at {url} returned {}", resp.status());
        }
        let body: UserExistsResponse = resp
            .json()
            .await
            .with_context(|| format!("core's response from {url} wasn't valid JSON"))?;
        Ok(body.exists)
    }

    /// `GET /_internal/tracker/merge-request?uid=&number=` — used both by
    /// [`crate::domain::issue::close_referenced_by_merge_request`]'s
    /// "closes #N" text parse and by
    /// [`crate::domain::board::add_card`]'s MR-card existence check.
    pub async fn mr_summary(&self, uid: &str, number: u64) -> Result<Option<MergeRequestMeta>> {
        let url = self.url(&format!("/tracker/merge-request?uid={uid}&number={number}"));
        let resp = self
            .get_request(&url)
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

    /// `POST /_internal/tracker/event` — forwards one of tracker's own
    /// domain events so core's `EventBus` stays the single source of truth
    /// (webhooks, future automation). Best-effort: logged, never fails the
    /// caller's own request over it.
    pub async fn push_event(&self, event: &gitquarry_wire::tracker::TrackerEvent) {
        let url = self.url("/tracker/event");
        if let Err(e) = self.post_request(&url).json(event).send().await {
            eprintln!("gitquarry-tracker: warning: couldn't push event to core at {url}: {e:#}");
        }
    }
}
