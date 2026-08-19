//! Building `/_raw/{kind}/{repo}?...` URLs — the raw-export sibling of
//! [`super::repo_link::RepoLink`]. Kept on its own route tree (`/_raw/...`)
//! rather than a `?view=` query param on the HTML repo route: a raw kind
//! and an HTML tab used to share one dispatch match in `repo::handler_repo`,
//! and a mistake there could return an HTML error page where a client
//! expected raw bytes. Separate routes make that structurally impossible.

use super::escaping::url_encode;

#[cfg(any(test, feature = "test-util"))]
pub struct RawLink<'a> {
    kind: &'a str,
    repo: &'a str,
    rev: &'a str,
    sha: Option<&'a str>,
    fmt: Option<&'a str>,
    tag: Option<&'a str>,
    filename: Option<&'a str>,
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) struct RawLink<'a> {
    kind: &'a str,
    repo: &'a str,
    rev: &'a str,
    sha: Option<&'a str>,
    fmt: Option<&'a str>,
    tag: Option<&'a str>,
    filename: Option<&'a str>,
}

#[cfg(any(test, feature = "test-util"))]
pub fn raw_link<'a>(kind: &'a str, repo: &'a str, rev: &'a str) -> RawLink<'a> {
    RawLink {
        kind,
        repo,
        rev,
        sha: None,
        fmt: None,
        tag: None,
        filename: None,
    }
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn raw_link<'a>(kind: &'a str, repo: &'a str, rev: &'a str) -> RawLink<'a> {
    RawLink {
        kind,
        repo,
        rev,
        sha: None,
        fmt: None,
        tag: None,
        filename: None,
    }
}

impl<'a> RawLink<'a> {
    pub(crate) fn sha(mut self, sha: &'a str) -> Self {
        self.sha = Some(sha);
        self
    }

    pub(crate) fn fmt(mut self, fmt: &'a str) -> Self {
        self.fmt = Some(fmt);
        self
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn tag(mut self, tag: &'a str) -> Self {
        self.tag = Some(tag);
        self
    }
    #[cfg(not(any(test, feature = "test-util")))]
    pub(crate) fn tag(mut self, tag: &'a str) -> Self {
        self.tag = Some(tag);
        self
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn filename(mut self, filename: &'a str) -> Self {
        self.filename = Some(filename);
        self
    }
    #[cfg(not(any(test, feature = "test-util")))]
    pub(crate) fn filename(mut self, filename: &'a str) -> Self {
        self.filename = Some(filename);
        self
    }
}

impl std::fmt::Display for RawLink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "/_raw/{}/{}", self.kind, url_encode(self.repo))?;
        let mut sep = '?';
        let mut field = |f: &mut std::fmt::Formatter<'_>, key: &str, value: &str| -> std::fmt::Result {
            write!(f, "{sep}{key}={}", url_encode(value))?;
            sep = '&';
            Ok(())
        };
        if !self.rev.is_empty() {
            field(f, "ref", self.rev)?;
        }
        if let Some(sha) = self.sha {
            field(f, "sha", sha)?;
        }
        if let Some(fmt) = self.fmt {
            field(f, "fmt", fmt)?;
        }
        if let Some(tag) = self.tag {
            field(f, "tag", tag)?;
        }
        if let Some(filename) = self.filename {
            field(f, "filename", filename)?;
        }
        Ok(())
    }
}

/// `/_raw/plain/{repo}/refs/heads/{branch}/{path}` — the GitHub-raw-style
/// sibling of `raw_link("plain", ...)`. Only "plain" gets this: it's the
/// one raw kind whose inputs (a branch and an in-repo path) are the same
/// shape as GitHub's own raw-content URL; patch/rawdiff/snapshot/atom/
/// release_asset key off a sha/tag/format instead and have no GitHub
/// equivalent to mirror. `handler_raw_plain` resolves this form by
/// checking real ref names (see `compound_path`), so `branch` itself may
/// contain `/` without breaking the split.
#[cfg(any(test, feature = "test-util"))]
pub fn github_raw_link(repo: &str, branch: &str, path: &str) -> String {
    format!(
        "/_raw/plain/{}/refs/heads/{}/{}",
        url_encode(repo),
        url_encode(branch),
        url_encode(path)
    )
}
#[cfg(not(any(test, feature = "test-util")))]
pub(crate) fn github_raw_link(repo: &str, branch: &str, path: &str) -> String {
    format!(
        "/_raw/plain/{}/refs/heads/{}/{}",
        url_encode(repo),
        url_encode(branch),
        url_encode(path)
    )
}
