//! Outbound calls to *this process's own* `gitquarry-core`'s loopback
//! `/_internal/*` API (`docs/federation-binary-split-concept.md` section
//! 3B) — never to another instance (see [`crate::instance_client`] for
//! that). Every call carries the shared `internal_token` secret in the
//! `X-Internal-Token` header, matching the middleware
//! `gitquarry-core`'s `internal_api` module enforces.

use anyhow::{Context, Result, bail};
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use gitquarry_wire::{MergeRequestMeta, MrDetail, RepoSummary, Uid};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct CoreClient {
    /// `gitquarry-federation.toml`'s `core_internal_url`, already including
    /// the `/_internal` path segment (e.g. `http://127.0.0.1:8090/_internal`).
    base_url: String,
    internal_token: String,
    http: reqwest::Client,
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

    async fn post_json<Req: Serialize, Resp: for<'de> Deserialize<'de>>(&self, path: &str, body: &Req) -> Result<Resp> {
        let url = self.url(path);
        let resp = self
            .http
            .post(&url)
            .signed(&self.internal_token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("couldn't reach core at {url}"))?;
        resp.json()
            .await
            .with_context(|| format!("core's response from {url} wasn't valid JSON"))
    }

    /// `path` already includes its own query string, if any.
    async fn get_json<Resp: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<Option<Resp>> {
        let url = self.url(path);
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

    /// `POST /_internal/authorize` — the one place a real access decision
    /// ever gets made; see `entities::AccessToken`'s own doc comment on why
    /// this crate never computes a `Permission` itself. `require` is a
    /// lowercase string matching core's `auth::entities::Permission`'s
    /// serde representation (`"guest"`/`"reporter"`/`"developer"`/
    /// `"maintainer"`/`"owner"`).
    pub async fn authorize(&self, username: &str, repo: &str, require: &str) -> Result<bool> {
        #[derive(Serialize)]
        struct Req<'a> {
            username: &'a str,
            repo: &'a str,
            require: &'a str,
        }
        #[derive(Deserialize)]
        struct Resp {
            ok: bool,
        }
        let resp: Resp = self
            .post_json(
                "/authorize",
                &Req {
                    username,
                    repo,
                    require,
                },
            )
            .await?;
        Ok(resp.ok)
    }

    /// `POST /_internal/repos/authorize-and-summarize` — the token-scoped
    /// equivalent of [`Self::authorize`] that also hands back the repo's
    /// public metadata, for a `private`/`limited` repo a fine-grained
    /// `AccessToken` lists by uid (never by name — see
    /// `entities::AccessToken::repos`'s own doc comment). `None` if `ok`
    /// was false; never returns a summary without also having verified the
    /// caller is entitled to it.
    pub async fn authorize_and_summarize(
        &self,
        username: &str,
        uid: &Uid,
        require: &str,
    ) -> Result<Option<RepoSummary>> {
        #[derive(Serialize)]
        struct Req<'a> {
            username: &'a str,
            uid: &'a Uid,
            require: &'a str,
        }
        #[derive(Deserialize)]
        struct Resp {
            ok: bool,
            repo: Option<RepoSummary>,
        }
        let resp: Resp = self
            .post_json("/repos/authorize-and-summarize", &Req { username, uid, require })
            .await?;
        Ok(if resp.ok { resp.repo } else { None })
    }

    /// `GET /_internal/repos/public` — every repo core currently exposes
    /// publicly.
    pub async fn public_repos(&self) -> Result<Vec<RepoSummary>> {
        Ok(self.get_json("/repos/public").await?.unwrap_or_default())
    }

    /// `GET /_internal/repos/public/{name}` — `None` if `name` doesn't
    /// exist or isn't `Public`.
    pub async fn public_repo(&self, name: &str) -> Result<Option<RepoSummary>> {
        self.get_json(&format!("/repos/public/{name}")).await
    }

    /// `GET /_internal/repos/uid-for-name?name=` — see
    /// `crate::entities::AccessToken::repos`'s own doc comment on why this
    /// is needed at all: a token scopes itself by uid, but every external
    /// PR-proxy call carries a repo *name*.
    pub async fn uid_for_name(&self, name: &str) -> Result<Option<Uid>> {
        #[derive(Deserialize)]
        struct Resp {
            uid: String,
        }
        let resp: Option<Resp> = self
            .get_json(&format!("/repos/uid-for-name?name={}", urlencoding(name)))
            .await?;
        Ok(resp.map(|r| Uid::from(r.uid)))
    }

    /// `GET /_internal/merge-requests/list?repo=` — every MR on `repo`.
    pub async fn list_merge_requests(&self, repo: &str) -> Result<Vec<MergeRequestMeta>> {
        Ok(self
            .get_json(&format!("/merge-requests/list?repo={}", urlencoding(repo)))
            .await?
            .unwrap_or_default())
    }

    /// `GET /_internal/merge-requests?repo=&number=` — one MR plus its
    /// comments.
    pub async fn merge_request_detail(&self, repo: &str, number: u64) -> Result<Option<MrDetail>> {
        self.get_json(&format!("/merge-requests?repo={}&number={number}", urlencoding(repo)))
            .await
    }

    /// `POST /_internal/merge-requests` — caller has already authorized
    /// `author` on `repo` via [`Self::authorize`], same division of labor
    /// `gitquarry-core`'s own `internal_api` module doc comment describes.
    pub async fn create_merge_request(&self, req: &CreateMergeRequest<'_>) -> Result<MergeRequestMeta> {
        self.post_json("/merge-requests", req).await
    }

    pub async fn comment_merge_request(&self, repo: &str, number: u64, author: &str, body: &str) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            repo: &'a str,
            number: u64,
            author: &'a str,
            body: &'a str,
        }
        #[derive(Deserialize)]
        struct Resp {
            #[allow(dead_code)]
            ok: bool,
        }
        let _: Resp = self
            .post_json(
                "/merge-requests/comment",
                &Req {
                    repo,
                    number,
                    author,
                    body,
                },
            )
            .await?;
        Ok(())
    }

    pub async fn merge_merge_request(
        &self,
        repo: &str,
        number: u64,
        merged_by: &str,
        strategy: gitquarry_wire::MergeStrategy,
    ) -> Result<MergeResult> {
        #[derive(Serialize)]
        struct Req<'a> {
            repo: &'a str,
            number: u64,
            merged_by: &'a str,
            strategy: gitquarry_wire::MergeStrategy,
        }
        self.post_json(
            "/merge-requests/merge",
            &Req {
                repo,
                number,
                merged_by,
                strategy,
            },
        )
        .await
    }
}

/// Minimal, dependency-free percent-encoding for a repo full name
/// (`owner/name`) landing in a query string — the only reserved character
/// that can appear in one is `/`, so this doesn't need a general-purpose
/// URL-encoding crate.
fn urlencoding(repo: &str) -> String {
    repo.replace('%', "%25").replace('/', "%2F").replace('&', "%26")
}

#[derive(Serialize)]
pub struct CreateMergeRequest<'a> {
    pub repo: &'a str,
    pub source_repo: &'a str,
    pub source_branch: &'a str,
    pub target_branch: &'a str,
    pub title: &'a str,
    pub description: &'a str,
    pub author: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MergeResult {
    Merged {
        merged: bool,
        sha: String,
    },
    Conflict {
        merged: bool,
        conflicting_paths: Vec<String>,
    },
}

#[derive(Serialize)]
pub struct ForkFromRemote<'a> {
    pub source_git_base_url: &'a str,
    pub source_repo_name: &'a str,
    pub dest_full_name: &'a str,
    /// `"user"` or `"group"` — matches core's `DestOwnerKind` serde repr.
    pub dest_owner_kind: &'a str,
    pub dest_owner_uid: &'a Uid,
    pub description: Option<String>,
    pub peer_label: &'a str,
    pub acting_username: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ForkOutcome {
    pub ok: bool,
    pub repo_uid: Option<String>,
    pub reason: Option<String>,
}

impl CoreClient {
    /// `POST /_internal/repos/fork-from-remote` — see
    /// `crate::domain::fork::request_fork`, the one caller.
    pub async fn fork_from_remote(&self, req: &ForkFromRemote<'_>) -> Result<ForkOutcome> {
        self.post_json("/repos/fork-from-remote", req).await
    }
}
