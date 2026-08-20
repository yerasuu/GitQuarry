//! Codeberg/Gitea API v1 — like `gitlab.rs`, commonly self-hosted (Codeberg
//! itself is just one particular Gitea instance), so the API base is
//! derived from `remote_url`'s own scheme+host. Shape-wise closer to
//! `github.rs` than `gitlab.rs`: a boolean `merged` flag alongside
//! `state`/`open`/`closed` rather than a separate `"merged"` state string,
//! MR comments modeled as issue comments.

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
const PER_PAGE: usize = 50;
const MAX_PAGES: u32 = 20;

pub struct CodebergForge {
    /// `https://<host>/api/v1`.
    api_base: String,
    owner: String,
    repo: String,
    token: String,
    client: reqwest::Client,
}

impl CodebergForge {
    /// `remote_url` is `https://<host>/<owner>/<repo>[.git]` — already
    /// validated by `validate::validate_mirror_url` at save time.
    pub fn new(remote_url: &str, token: &str) -> Result<Self> {
        let (api_base, owner, repo) = parse_api_base_and_repo(remote_url)?;
        super::check_path_segment("repository owner", &owner)?;
        super::check_path_segment("repository name", &repo)?;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // No redirect following. The instance's host allowlist is what
            // decides where a mirror may make this server open connections;
            // a 302 from an allowed host would otherwise pick a different
            // destination — including one inside the network the server
            // sits in — after that check already passed.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building the Codeberg HTTP client")?;
        Ok(CodebergForge {
            api_base,
            owner,
            repo,
            token: token.to_string(),
            client,
        })
    }

    fn pulls_url(&self, suffix: &str) -> String {
        format!("{}/repos/{}/{}/pulls{suffix}", self.api_base, self.owner, self.repo)
    }

    fn issues_url(&self, suffix: &str) -> String {
        format!("{}/repos/{}/{}/issues{suffix}", self.api_base, self.owner, self.repo)
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .header("Authorization", format!("token {}", self.token))
    }
}

/// `https://codeberg.org/owner/repo.git` -> `("https://codeberg.org/api/v1", "owner", "repo")`.
// `pub` so the external integration test (tests/mirror_external_forge_codeberg.rs)
// can reach it directly; also called unconditionally from `CodebergForge::new`
// below, so it can't be `#[cfg(test)]`-gated away.
pub fn parse_api_base_and_repo(remote_url: &str) -> Result<(String, String, String)> {
    let (scheme, rest) = remote_url
        .split_once("://")
        .with_context(|| format!("couldn't parse Codeberg mirror url: {remote_url}"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() {
        bail!("couldn't parse Codeberg mirror url (no host): {remote_url}");
    }
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.trim_matches('/').splitn(2, '/');
    let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
        bail!("couldn't parse owner/repo out of Codeberg mirror url: {remote_url}");
    };
    if owner.is_empty() || repo.is_empty() {
        bail!("couldn't parse owner/repo out of Codeberg mirror url: {remote_url}");
    }
    Ok((format!("{scheme}://{host}/api/v1"), owner.to_string(), repo.to_string()))
}

// `pub`/pub fields so the external integration test
// (tests/mirror_external_forge_codeberg.rs) can build/inspect these
// deserialization targets directly; also used unconditionally by the
// `ForgeApi` impl below, so they can't be `#[cfg(test)]`-gated away.
#[derive(Debug, Deserialize)]
pub struct GtUser {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct GtBranchRef {
    #[serde(rename = "ref")]
    pub branch: String,
}

#[derive(Debug, Deserialize)]
pub struct GtPullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub merged: bool,
    pub user: GtUser,
    pub head: GtBranchRef,
    pub base: GtBranchRef,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub merged_by: Option<GtUser>,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
}

impl From<GtPullRequest> for RemoteMr {
    fn from(mr: GtPullRequest) -> Self {
        let state = if mr.merged {
            RemoteMrState::Merged
        } else if mr.state == "closed" {
            RemoteMrState::Closed
        } else {
            RemoteMrState::Open
        };
        RemoteMr {
            number: mr.number,
            title: mr.title,
            body: mr.body.unwrap_or_default(),
            source_branch: mr.head.branch,
            target_branch: mr.base.branch,
            state,
            author: mr.user.login,
            created_at: mr.created_at,
            updated_at: mr.updated_at,
            merged_at: mr.merged_at,
            merged_by: mr.merged_by.map(|u| u.login),
            merge_commit_sha: mr.merge_commit_sha,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GtComment {
    id: u64,
    user: GtUser,
    body: String,
    created_at: String,
}

impl From<GtComment> for RemoteComment {
    fn from(c: GtComment) -> Self {
        RemoteComment {
            id: c.id,
            author: c.user.login,
            body: c.body,
            created_at: c.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct GtNewPr<'a> {
    title: &'a str,
    body: &'a str,
    head: &'a str,
    base: &'a str,
}

#[derive(Debug, Default, Serialize)]
struct GtPatchPr<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct GtMergeOptions {
    #[serde(rename = "Do")]
    do_: &'static str,
}

#[derive(Debug, Serialize)]
struct GtNewComment<'a> {
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
impl ForgeApi for CodebergForge {
    async fn list_merge_requests(&self, since: Option<&str>) -> Result<Vec<RemoteMr>> {
        // Same "no true server-side `since`, sort newest-updated-first and
        // cut client-side" shape as `github.rs`/`gitlab.rs`.
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = self.pulls_url(&format!("?state=all&sort=recentupdate&limit={PER_PAGE}&page={page}"));
            let resp = self
                .request(reqwest::Method::GET, &url)
                .send()
                .await
                .with_context(|| format!("couldn't reach {url}"))?;
            let mrs: Vec<GtPullRequest> = ok_json(resp, &url).await?;
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
        eprintln!("gitquarry: warning: stopped listing Gitea MRs after {MAX_PAGES} pages, older ones not synced");
        Ok(out)
    }

    async fn get_merge_request(&self, number: u64) -> Result<Option<RemoteMr>> {
        let url = self.pulls_url(&format!("/{number}"));
        let resp = self
            .request(reqwest::Method::GET, &url)
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(ok_json::<GtPullRequest>(resp, &url).await?.into()))
    }

    async fn create_merge_request(&self, new: NewRemoteMr) -> Result<RemoteMr> {
        let url = self.pulls_url("");
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&GtNewPr {
                title: &new.title,
                body: &new.body,
                head: &new.head,
                base: &new.base,
            })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        Ok(ok_json::<GtPullRequest>(resp, &url).await?.into())
    }

    async fn update_merge_request(&self, number: u64, patch: MrPatch) -> Result<()> {
        let url = self.pulls_url(&format!("/{number}"));
        // Same "Merged isn't a settable `state`, use the dedicated merge
        // endpoint" split as `github.rs`/`gitlab.rs`.
        let state = match patch.state {
            Some(RemoteMrState::Open) => Some("open"),
            Some(RemoteMrState::Closed) => Some("closed"),
            Some(RemoteMrState::Merged) | None => None,
        };
        let resp = self
            .request(reqwest::Method::PATCH, &url)
            .json(&GtPatchPr {
                title: patch.title.as_deref(),
                body: patch.body.as_deref(),
                state,
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
        let url = self.pulls_url(&format!("/{number}/merge"));
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&GtMergeOptions { do_: "merge" })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("{url} returned {status}: {body}");
        }
        // The merge endpoint's own response body isn't the full MR object —
        // re-fetch, same as `github.rs`/`gitlab.rs`.
        self.get_merge_request(number)
            .await?
            .with_context(|| format!("MR #{number} vanished right after merging"))
    }

    async fn list_comments(&self, number: u64) -> Result<Vec<RemoteComment>> {
        // Gitea/Codeberg models MR comments as issue comments too, same as
        // GitHub.
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = self.issues_url(&format!("/{number}/comments?limit={PER_PAGE}&page={page}"));
            let resp = self
                .request(reqwest::Method::GET, &url)
                .send()
                .await
                .with_context(|| format!("couldn't reach {url}"))?;
            let comments: Vec<GtComment> = ok_json(resp, &url).await?;
            let last_page = comments.len() < PER_PAGE;
            out.extend(comments.into_iter().map(RemoteComment::from));
            if last_page {
                return Ok(out);
            }
        }
        eprintln!(
            "gitquarry: warning: stopped listing comments on Gitea MR #{number} after {MAX_PAGES} pages, the rest wasn't imported"
        );
        Ok(out)
    }

    async fn add_comment(&self, number: u64, body: &str) -> Result<RemoteComment> {
        let url = self.issues_url(&format!("/{number}/comments"));
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&GtNewComment { body })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        Ok(ok_json::<GtComment>(resp, &url).await?.into())
    }
}
