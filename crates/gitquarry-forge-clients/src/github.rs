//! GitHub REST API v3 (`api.github.com`) — the first, validated-against-a-
//! real-account [`super::ForgeApi`] implementation. GitLab/Codeberg follow
//! the same shape once this one's anti-loop bookkeeping is proven.

use super::{ForgeApi, MrPatch, NewRemoteMr, RemoteComment, RemoteMr, RemoteMrState};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::warn;

/// github.com's API lives on its own host; every GitHub Enterprise Server
/// instance serves it under `/api/v3` on the same host as the repos. Which
/// one applies is decided by the mirror's own URL — a hardcoded
/// `api.github.com` sent an Enterprise mirror's token to github.com and
/// asked it about a repo path that only exists on the private instance.
const DOTCOM_API_BASE: &str = "https://api.github.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// GitHub rejects requests with no `User-Agent` outright (403) — unlike
/// every other outbound call this codebase makes (loopback IPC
/// instances, which don't gate on it), this one's mandatory.
const USER_AGENT: &str = "gitquarry";
/// One request's page size, and how many pages a list call will walk
/// before giving up. Unpaginated single-page listings silently truncated
/// every MR thread past the first page — a repo's comment history is
/// exactly the kind of thing that outgrows one page and then quietly
/// stops syncing. The page cap is the backstop against a pathological
/// thread pinning a sync tick forever; hitting it logs rather than
/// pretending the listing was complete.
const PER_PAGE: usize = 100;
const MAX_PAGES: u32 = 20;

pub struct GitHubForge {
    api_base: String,
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
            .context("building the GitHub HTTP client")?;
        Ok(GitHubForge {
            api_base: api_base_for(remote_url),
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
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", USER_AGENT)
    }
}

/// [`DOTCOM_API_BASE`] for a `github.com` URL, `<scheme>://<host>/api/v3`
/// for anything else (a GitHub Enterprise Server instance).
// `pub` for the same reason `parse_owner_repo` below is.
pub fn api_base_for(remote_url: &str) -> String {
    let rest = remote_url.split_once("://").map_or(remote_url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let scheme = remote_url.split_once("://").map_or("https", |(scheme, _)| scheme);
    match host.trim_start_matches("www.") {
        "github.com" => DOTCOM_API_BASE.to_string(),
        _ => format!("{scheme}://{host}/api/v3"),
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

impl From<GhPullRequest> for RemoteMr {
    fn from(mr: GhPullRequest) -> Self {
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
    async fn list_merge_requests(&self, since: Option<&str>) -> Result<Vec<RemoteMr>> {
        // GitHub's `/pulls` list has no `since` filter of its own (unlike
        // `/issues`, which does) — sorted newest-updated-first instead, so a
        // cursor'd caller stops consuming as soon as it crosses `since`
        // instead of paging through the whole history every tick. Without a
        // cursor (a mirror's first tick) it does page, or the repo's older
        // MRs would never be discovered at all.
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = self.pulls_url(&format!(
                "?state=all&sort=updated&direction=desc&per_page={PER_PAGE}&page={page}"
            ));
            let resp = self
                .request(reqwest::Method::GET, &url)
                .send()
                .await
                .with_context(|| format!("couldn't reach {url}"))?;
            let mrs: Vec<GhPullRequest> = ok_json(resp, &url).await?;
            let last_page = mrs.len() < PER_PAGE;
            let before = out.len();
            out.extend(
                mrs.into_iter()
                    .map(RemoteMr::from)
                    .take_while(|mr| since.is_none_or(|since| !crate::stamp::later(since, &mr.updated_at))),
            );
            let crossed_cursor = out.len() - before < PER_PAGE;
            if last_page || crossed_cursor {
                return Ok(out);
            }
        }
        warn!("stopped listing GitHub MRs after {MAX_PAGES} pages, older ones not synced");
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
        Ok(Some(ok_json::<GhPullRequest>(resp, &url).await?.into()))
    }

    async fn create_merge_request(&self, new: NewRemoteMr) -> Result<RemoteMr> {
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

    async fn update_merge_request(&self, number: u64, patch: MrPatch) -> Result<()> {
        let url = self.pulls_url(&format!("/{number}"));
        // `Merged` isn't a settable `state` via this endpoint — GitHub only
        // ever transitions an MR to merged through the dedicated merge
        // endpoint (`accept_merge_request`); a caller asking to patch straight
        // to `Merged` is a caller bug, not something to silently reinterpret
        // here, so it's simply never sent.
        let state = match patch.state {
            Some(RemoteMrState::Open) => Some("open"),
            Some(RemoteMrState::Closed) => Some("closed"),
            Some(RemoteMrState::Merged) | None => None,
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

    async fn accept_merge_request(&self, number: u64) -> Result<RemoteMr> {
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
        // wants — so this re-fetches the now-merged MR instead of mapping
        // the merge response directly.
        self.get_merge_request(number)
            .await?
            .with_context(|| format!("MR #{number} vanished right after merging"))
    }

    async fn list_comments(&self, number: u64) -> Result<Vec<RemoteComment>> {
        // GitHub models MR comments as issue comments — an MR *is* an issue
        // under the hood, same numbering.
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = self.issues_url(&format!("/{number}/comments?per_page={PER_PAGE}&page={page}"));
            let resp = self
                .request(reqwest::Method::GET, &url)
                .send()
                .await
                .with_context(|| format!("couldn't reach {url}"))?;
            let comments: Vec<GhComment> = ok_json(resp, &url).await?;
            let last_page = comments.len() < PER_PAGE;
            out.extend(comments.into_iter().map(RemoteComment::from));
            if last_page {
                return Ok(out);
            }
        }
        warn!("stopped listing comments on GitHub MR #{number} after {MAX_PAGES} pages, the rest wasn't imported");
        Ok(out)
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
