//! GitLab REST API v4 — unlike GitHub, commonly self-hosted, so the API
//! base is derived from `remote_url`'s own scheme+host rather than a fixed
//! `api.github.com`-style constant. Thin adapter over the same
//! [`super::ForgeApi`] shape `github.rs` already validated against a real
//! account.

use super::{ForgeApi, MrPatch, NewRemoteMr, RemoteComment, RemoteMr, RemoteMrState};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// One request's page size, and how many pages a list call will walk
/// before giving up. Unpaginated single-page listings silently truncated
/// every MR thread past the first page — a repo's comment history is
/// exactly the kind of thing that outgrows one page and then quietly
/// stops syncing. The page cap is the backstop against a pathological
/// thread pinning a sync tick forever; hitting it logs rather than
/// pretending the listing was complete.
const PER_PAGE: usize = 100;
const MAX_PAGES: u32 = 20;

pub struct GitLabForge {
    /// `https://<host>/api/v4`.
    api_base: String,
    /// URL-encoded `namespace%2Fsubgroup%2Fproject` — GitLab projects can
    /// nest under subgroups, so this can carry more than one `/`, all
    /// percent-encoded (the API accepts either this or a numeric project
    /// id; a mirror only ever has the path, never the id).
    project_path: String,
    token: String,
    client: reqwest::Client,
}

impl GitLabForge {
    /// `remote_url` is `https://<host>/<namespace...>/<project>[.git]` —
    /// already validated by `validate::validate_mirror_url` at save time.
    pub fn new(remote_url: &str, token: &str) -> Result<Self> {
        let (api_base, project_path) = parse_api_base_and_project(remote_url)?;
        // Same guard `github.rs`/`codeberg.rs` apply to their own
        // owner/repo: the encoder below only escapes `%`, so a `?` or `#`
        // anywhere in the path would still cut the REST URL short and send
        // the request somewhere else entirely. GitLab's own namespace and
        // project names are limited to this alphabet regardless.
        for segment in project_path.split("%2F") {
            super::check_path_segment("project path segment", segment)?;
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("building the GitLab HTTP client")?;
        Ok(GitLabForge {
            api_base,
            project_path,
            token: token.to_string(),
            client,
        })
    }

    fn mrs_url(&self, suffix: &str) -> String {
        format!(
            "{}/projects/{}/merge_requests{suffix}",
            self.api_base, self.project_path
        )
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.client.request(method, url).header("PRIVATE-TOKEN", &self.token)
    }
}

/// `https://gitlab.example.com/group/subgroup/project.git` ->
/// `("https://gitlab.example.com/api/v4", "group%2Fsubgroup%2Fproject")`.
#[cfg(any(test, feature = "test-util"))]
pub fn parse_api_base_and_project(remote_url: &str) -> Result<(String, String)> {
    let (scheme, rest) = remote_url
        .split_once("://")
        .with_context(|| format!("couldn't parse GitLab mirror url: {remote_url}"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() {
        bail!("couldn't parse GitLab mirror url (no host): {remote_url}");
    }
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_matches('/');
    if path.is_empty() {
        bail!("couldn't parse GitLab mirror url (no project path): {remote_url}");
    }
    let encoded = path
        .split('/')
        .map(|segment| segment.replace('%', "%25"))
        .collect::<Vec<_>>()
        .join("%2F");
    Ok((format!("{scheme}://{host}/api/v4"), encoded))
}
#[cfg(not(any(test, feature = "test-util")))]
fn parse_api_base_and_project(remote_url: &str) -> Result<(String, String)> {
    let (scheme, rest) = remote_url
        .split_once("://")
        .with_context(|| format!("couldn't parse GitLab mirror url: {remote_url}"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() {
        bail!("couldn't parse GitLab mirror url (no host): {remote_url}");
    }
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_matches('/');
    if path.is_empty() {
        bail!("couldn't parse GitLab mirror url (no project path): {remote_url}");
    }
    let encoded = path
        .split('/')
        .map(|segment| segment.replace('%', "%25"))
        .collect::<Vec<_>>()
        .join("%2F");
    Ok((format!("{scheme}://{host}/api/v4"), encoded))
}

#[derive(Debug, Deserialize)]
pub struct GlUser {
    #[cfg(any(test, feature = "test-util"))]
    pub username: String,
    #[cfg(not(any(test, feature = "test-util")))]
    username: String,
}

#[derive(Debug, Deserialize)]
pub struct GlMergeRequest {
    #[cfg(any(test, feature = "test-util"))]
    pub iid: u64,
    #[cfg(not(any(test, feature = "test-util")))]
    iid: u64,
    #[cfg(any(test, feature = "test-util"))]
    pub title: String,
    #[cfg(not(any(test, feature = "test-util")))]
    title: String,
    #[serde(default)]
    #[cfg(any(test, feature = "test-util"))]
    pub description: Option<String>,
    #[serde(default)]
    #[cfg(not(any(test, feature = "test-util")))]
    description: Option<String>,
    /// `"opened"` | `"closed"` | `"merged"` | `"locked"` — `"locked"` never
    /// shows up for a plain unprotected project, folded into `Open` here if
    /// it ever does rather than adding a fourth [`RemoteMrState`] variant
    /// only GitLab has.
    #[cfg(any(test, feature = "test-util"))]
    pub state: String,
    #[cfg(not(any(test, feature = "test-util")))]
    state: String,
    #[cfg(any(test, feature = "test-util"))]
    pub author: GlUser,
    #[cfg(not(any(test, feature = "test-util")))]
    author: GlUser,
    #[cfg(any(test, feature = "test-util"))]
    pub source_branch: String,
    #[cfg(not(any(test, feature = "test-util")))]
    source_branch: String,
    #[cfg(any(test, feature = "test-util"))]
    pub target_branch: String,
    #[cfg(not(any(test, feature = "test-util")))]
    target_branch: String,
    #[cfg(any(test, feature = "test-util"))]
    pub created_at: String,
    #[cfg(not(any(test, feature = "test-util")))]
    created_at: String,
    #[cfg(any(test, feature = "test-util"))]
    pub updated_at: String,
    #[cfg(not(any(test, feature = "test-util")))]
    updated_at: String,
    #[serde(default)]
    #[cfg(any(test, feature = "test-util"))]
    pub merged_at: Option<String>,
    #[serde(default)]
    #[cfg(not(any(test, feature = "test-util")))]
    merged_at: Option<String>,
    #[serde(default)]
    #[cfg(any(test, feature = "test-util"))]
    pub merged_by: Option<GlUser>,
    #[serde(default)]
    #[cfg(not(any(test, feature = "test-util")))]
    merged_by: Option<GlUser>,
    #[serde(default)]
    #[cfg(any(test, feature = "test-util"))]
    pub merge_commit_sha: Option<String>,
    #[serde(default)]
    #[cfg(not(any(test, feature = "test-util")))]
    merge_commit_sha: Option<String>,
    #[serde(default)]
    #[cfg(any(test, feature = "test-util"))]
    pub sha: Option<String>,
    #[serde(default)]
    #[cfg(not(any(test, feature = "test-util")))]
    sha: Option<String>,
}

impl From<GlMergeRequest> for RemoteMr {
    fn from(mr: GlMergeRequest) -> Self {
        let state = match mr.state.as_str() {
            "merged" => RemoteMrState::Merged,
            "closed" => RemoteMrState::Closed,
            _ => RemoteMrState::Open,
        };
        // GitLab's dedicated `merge_commit_sha` is only ever populated for a
        // "merge commit" strategy merge; a fast-forward/squash merge leaves
        // it `null` and reports the real resulting sha in `sha` instead —
        // falling back to that keeps `mark_merged_externally` from ever
        // recording a `None` merge sha for an MR GitLab itself considers
        // cleanly merged.
        let merge_commit_sha = mr
            .merge_commit_sha
            .or(if state == RemoteMrState::Merged { mr.sha } else { None });
        RemoteMr {
            number: mr.iid,
            title: mr.title,
            body: mr.description.unwrap_or_default(),
            source_branch: mr.source_branch,
            target_branch: mr.target_branch,
            state,
            author: mr.author.username,
            created_at: mr.created_at,
            updated_at: mr.updated_at,
            merged_at: mr.merged_at,
            merged_by: mr.merged_by.map(|u| u.username),
            merge_commit_sha,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GlNote {
    #[cfg(any(test, feature = "test-util"))]
    pub id: u64,
    #[cfg(not(any(test, feature = "test-util")))]
    id: u64,
    #[cfg(any(test, feature = "test-util"))]
    pub author: GlUser,
    #[cfg(not(any(test, feature = "test-util")))]
    author: GlUser,
    #[cfg(any(test, feature = "test-util"))]
    pub body: String,
    #[cfg(not(any(test, feature = "test-util")))]
    body: String,
    #[cfg(any(test, feature = "test-util"))]
    pub created_at: String,
    #[cfg(not(any(test, feature = "test-util")))]
    created_at: String,
    #[serde(default)]
    #[cfg(any(test, feature = "test-util"))]
    pub system: bool,
    #[serde(default)]
    #[cfg(not(any(test, feature = "test-util")))]
    system: bool,
}

#[derive(Debug, Serialize)]
struct GlNewMr<'a> {
    source_branch: &'a str,
    target_branch: &'a str,
    title: &'a str,
    description: &'a str,
}

#[derive(Debug, Default, Serialize)]
struct GlPatchMr<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_event: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct GlNewNote<'a> {
    body: &'a str,
}

async fn ok_json<T: for<'de> Deserialize<'de>>(resp: reqwest::Response, url: &str) -> Result<T> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("{url} returned {status}: {body}");
    }
    resp.json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))
}

#[async_trait::async_trait]
impl ForgeApi for GitLabForge {
    async fn list_merge_requests(&self, since: Option<&str>) -> Result<Vec<RemoteMr>> {
        // Same "no server-side `since`, sort newest-updated-first and cut
        // client-side" shape as `github.rs` — GitLab's `updated_after`
        // filter exists but its semantics (inclusive/exclusive, timezone
        // handling) aren't worth trusting over a plain string comparison
        // this codebase already controls end to end.
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = self.mrs_url(&format!(
                "?state=all&order_by=updated_at&sort=desc&per_page={PER_PAGE}&page={page}"
            ));
            let resp = self
                .request(reqwest::Method::GET, &url)
                .send()
                .await
                .with_context(|| format!("couldn't reach {url}"))?;
            let mrs: Vec<GlMergeRequest> = ok_json(resp, &url).await?;
            let last_page = mrs.len() < PER_PAGE;
            let before = out.len();
            out.extend(
                mrs.into_iter()
                    .map(RemoteMr::from)
                    .take_while(|mr| since.is_none_or(|since| mr.updated_at.as_str() > since)),
            );
            let crossed_cursor = out.len() - before < PER_PAGE;
            if last_page || crossed_cursor {
                return Ok(out);
            }
        }
        eprintln!("gitquarry: warning: stopped listing GitLab MRs after {MAX_PAGES} pages, older ones not synced");
        Ok(out)
    }

    async fn get_merge_request(&self, number: u64) -> Result<Option<RemoteMr>> {
        let url = self.mrs_url(&format!("/{number}"));
        let resp = self
            .request(reqwest::Method::GET, &url)
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(ok_json::<GlMergeRequest>(resp, &url).await?.into()))
    }

    async fn create_merge_request(&self, new: NewRemoteMr) -> Result<RemoteMr> {
        let url = self.mrs_url("");
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&GlNewMr {
                source_branch: &new.head,
                target_branch: &new.base,
                title: &new.title,
                description: &new.body,
            })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        Ok(ok_json::<GlMergeRequest>(resp, &url).await?.into())
    }

    async fn update_merge_request(&self, number: u64, patch: MrPatch) -> Result<()> {
        let url = self.mrs_url(&format!("/{number}"));
        // GitLab's PUT never takes `Merged` as a `state_event` either — same
        // "use the dedicated merge endpoint" split as GitHub's PATCH.
        let state_event = match patch.state {
            Some(RemoteMrState::Open) => Some("reopen"),
            Some(RemoteMrState::Closed) => Some("close"),
            Some(RemoteMrState::Merged) | None => None,
        };
        let resp = self
            .request(reqwest::Method::PUT, &url)
            .json(&GlPatchMr {
                title: patch.title.as_deref(),
                description: patch.body.as_deref(),
                state_event,
            })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("{url} returned {status}: {body}");
        }
        Ok(())
    }

    async fn accept_merge_request(&self, number: u64) -> Result<RemoteMr> {
        let url = self.mrs_url(&format!("/{number}/merge"));
        let resp = self
            .request(reqwest::Method::PUT, &url)
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("{url} returned {status}: {body}");
        }
        // Unlike GitHub's merge response (bare `{sha, merged, message}`),
        // GitLab's merge response is the full updated MR object — but
        // re-fetching anyway keeps this path identical to `github.rs` and
        // immune to any field GitLab's merge response happens to omit.
        self.get_merge_request(number)
            .await?
            .with_context(|| format!("MR !{number} vanished right after merging"))
    }

    async fn list_comments(&self, number: u64) -> Result<Vec<RemoteComment>> {
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = format!(
                "{}/projects/{}/merge_requests/{number}/notes?per_page={PER_PAGE}&page={page}",
                self.api_base, self.project_path
            );
            let resp = self
                .request(reqwest::Method::GET, &url)
                .send()
                .await
                .with_context(|| format!("couldn't reach {url}"))?;
            let notes: Vec<GlNote> = ok_json(resp, &url).await?;
            let last_page = notes.len() < PER_PAGE;
            // GitLab mixes real comments with system notes ("changed the
            // description", "marked as ready", ...) in the same list,
            // flagged by `system` — importing those as if they were
            // comments would spam the local thread with noise nobody wrote.
            // Filtered after the page-size check, so a page full of system
            // notes still advances to the next one.
            out.extend(notes.into_iter().filter(|n| !n.system).map(|n| RemoteComment {
                id: n.id,
                author: n.author.username,
                body: n.body,
                created_at: n.created_at,
            }));
            if last_page {
                return Ok(out);
            }
        }
        eprintln!(
            "gitquarry: warning: stopped listing notes on GitLab MR !{number} after {MAX_PAGES} pages, the rest wasn't imported"
        );
        Ok(out)
    }

    async fn add_comment(&self, number: u64, body: &str) -> Result<RemoteComment> {
        let url = format!(
            "{}/projects/{}/merge_requests/{number}/notes",
            self.api_base, self.project_path
        );
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&GlNewNote { body })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        let note: GlNote = ok_json(resp, &url).await?;
        Ok(RemoteComment {
            id: note.id,
            author: note.author.username,
            body: note.body,
            created_at: note.created_at,
        })
    }
}
