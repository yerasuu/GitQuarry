//! GitHub REST API v3 (`api.github.com`) — the first, validated-against-a-
//! real-account [`super::ForgeApi`] implementation. GitLab/Codeberg follow
//! the same shape once this one's anti-loop bookkeeping is proven.

use super::{ForgeApi, NewRemotePr, PrPatch, RemoteComment, RemotePr, RemotePrState};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const API_BASE: &str = "https://api.github.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// GitHub rejects requests with no `User-Agent` outright (403) — unlike
/// every other outbound call this codebase makes (loopback IPC
/// instances, which don't gate on it), this one's mandatory.
const USER_AGENT: &str = "gitquarry";

pub struct GitHubForge {
    owner: String,
    repo: String,
    token: String,
    client: reqwest::Client,
}

impl GitHubForge {
    /// `remote_url` is `https://github.com/<owner>/<repo>[.git]` — already
    /// validated by `validate::validate_mirror_url` at save time (http(s)
    /// scheme, no control/whitespace chars), so this only ever has to parse
    /// the path, not re-validate the whole URL shape.
    pub fn new(remote_url: &str, token: &str) -> Result<Self> {
        let (owner, repo) = parse_owner_repo(remote_url)?;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("building the GitHub HTTP client")?;
        Ok(GitHubForge {
            owner,
            repo,
            token: token.to_string(),
            client,
        })
    }

    fn pulls_url(&self, suffix: &str) -> String {
        format!("{API_BASE}/repos/{}/{}/pulls{suffix}", self.owner, self.repo)
    }

    fn issues_url(&self, suffix: &str) -> String {
        format!("{API_BASE}/repos/{}/{}/issues{suffix}", self.owner, self.repo)
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", USER_AGENT)
    }
}

/// `https://github.com/<owner>/<repo>` or `.../<repo>.git`, `http://` too
/// (a self-hosted GitHub Enterprise instance behind plain http is
/// implausible but not this parser's job to reject — `validate_mirror_url`
/// already gates the scheme).
// `pub` (rather than private) so the extracted `tests/` integration test
// can still reach it -- used unconditionally by `GitHubForge::new` below,
// so this can't be `#[cfg(test)]`-gated away like a true test-only helper.
pub fn parse_owner_repo(remote_url: &str) -> Result<(String, String)> {
    let without_scheme = remote_url.split_once("://").map(|(_, rest)| rest).unwrap_or(remote_url);
    let path = without_scheme.split_once('/').map(|(_, rest)| rest).unwrap_or("");
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.trim_matches('/').splitn(2, '/');
    let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
        bail!("couldn't parse owner/repo out of GitHub mirror url: {remote_url}");
    };
    if owner.is_empty() || repo.is_empty() {
        bail!("couldn't parse owner/repo out of GitHub mirror url: {remote_url}");
    }
    Ok((owner.to_string(), repo.to_string()))
}

// `pub` (struct and fields) so the extracted `tests/` integration test can
// still construct these directly -- used unconditionally by this file's own
// deserialization/mapping code below, so can't be `#[cfg(test)]`-gated away
// like a true test-only helper.
#[derive(Debug, Deserialize)]
pub struct GhUser {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct GhBranchRef {
    #[serde(rename = "ref")]
    pub branch: String,
}

#[derive(Debug, Deserialize)]
pub struct GhPullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub merged: bool,
    pub user: GhUser,
    pub head: GhBranchRef,
    pub base: GhBranchRef,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub merged_by: Option<GhUser>,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
}

impl From<GhPullRequest> for RemotePr {
    fn from(pr: GhPullRequest) -> Self {
        let state = if pr.merged {
            RemotePrState::Merged
        } else if pr.state == "closed" {
            RemotePrState::Closed
        } else {
            RemotePrState::Open
        };
        RemotePr {
            number: pr.number,
            title: pr.title,
            body: pr.body.unwrap_or_default(),
            source_branch: pr.head.branch,
            target_branch: pr.base.branch,
            state,
            author: pr.user.login,
            created_at: pr.created_at,
            updated_at: pr.updated_at,
            merged_at: pr.merged_at,
            merged_by: pr.merged_by.map(|u| u.login),
            merge_commit_sha: pr.merge_commit_sha,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhComment {
    id: u64,
    user: GhUser,
    body: String,
    created_at: String,
}

impl From<GhComment> for RemoteComment {
    fn from(c: GhComment) -> Self {
        RemoteComment {
            id: c.id,
            author: c.user.login,
            body: c.body,
            created_at: c.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct GhNewPr<'a> {
    title: &'a str,
    body: &'a str,
    head: &'a str,
    base: &'a str,
}

#[derive(Debug, Default, Serialize)]
struct GhPatchPr<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct GhNewComment<'a> {
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
impl ForgeApi for GitHubForge {
    async fn list_pull_requests(&self, since: Option<&str>) -> Result<Vec<RemotePr>> {
        // GitHub's `/pulls` list has no `since` filter of its own (unlike
        // `/issues`, which does) — sorted newest-updated-first instead, so a
        // cursor'd caller can stop consuming as soon as it crosses `since`
        // without paging through the whole history every tick.
        let url = self.pulls_url("?state=all&sort=updated&direction=desc&per_page=100");
        let resp = self
            .request(reqwest::Method::GET, &url)
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        let prs: Vec<GhPullRequest> = ok_json(resp, &url).await?;
        Ok(prs
            .into_iter()
            .map(RemotePr::from)
            .take_while(|pr| since.is_none_or(|since| pr.updated_at.as_str() > since))
            .collect())
    }

    async fn get_pull_request(&self, number: u64) -> Result<Option<RemotePr>> {
        let url = self.pulls_url(&format!("/{number}"));
        let resp = self
            .request(reqwest::Method::GET, &url)
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(ok_json::<GhPullRequest>(resp, &url).await?.into()))
    }

    async fn create_pull_request(&self, new: NewRemotePr) -> Result<RemotePr> {
        let url = self.pulls_url("");
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&GhNewPr {
                title: &new.title,
                body: &new.body,
                head: &new.head,
                base: &new.base,
            })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        Ok(ok_json::<GhPullRequest>(resp, &url).await?.into())
    }

    async fn update_pull_request(&self, number: u64, patch: PrPatch) -> Result<()> {
        let url = self.pulls_url(&format!("/{number}"));
        // `Merged` isn't a settable `state` via this endpoint — GitHub only
        // ever transitions a PR to merged through the dedicated merge
        // endpoint (`merge_pull_request`); a caller asking to patch straight
        // to `Merged` is a caller bug, not something to silently reinterpret
        // here, so it's simply never sent.
        let state = match patch.state {
            Some(RemotePrState::Open) => Some("open"),
            Some(RemotePrState::Closed) => Some("closed"),
            Some(RemotePrState::Merged) | None => None,
        };
        let resp = self
            .request(reqwest::Method::PATCH, &url)
            .json(&GhPatchPr {
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

    async fn merge_pull_request(&self, number: u64) -> Result<RemotePr> {
        let url = self.pulls_url(&format!("/{number}/merge"));
        let resp = self
            .request(reqwest::Method::PUT, &url)
            .json(&serde_json::json!({}))
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("{url} returned {status}: {body}");
        }
        // The merge response itself only carries `{sha, merged, message}` —
        // not `merged_at`/`merged_by`, which `mark_merged_externally` also
        // wants — so this re-fetches the now-merged PR instead of mapping
        // the merge response directly.
        self.get_pull_request(number)
            .await?
            .with_context(|| format!("PR #{number} vanished right after merging"))
    }

    async fn list_comments(&self, number: u64) -> Result<Vec<RemoteComment>> {
        // GitHub models PR comments as issue comments — a PR *is* an issue
        // under the hood, same numbering.
        let url = self.issues_url(&format!("/{number}/comments?per_page=100"));
        let resp = self
            .request(reqwest::Method::GET, &url)
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        let comments: Vec<GhComment> = ok_json(resp, &url).await?;
        Ok(comments.into_iter().map(RemoteComment::from).collect())
    }

    async fn add_comment(&self, number: u64, body: &str) -> Result<RemoteComment> {
        let url = self.issues_url(&format!("/{number}/comments"));
        let resp = self
            .request(reqwest::Method::POST, &url)
            .json(&GhNewComment { body })
            .send()
            .await
            .with_context(|| format!("couldn't reach {url}"))?;
        Ok(ok_json::<GhComment>(resp, &url).await?.into())
    }
}
