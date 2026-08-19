//! Building `/{repo}?view=...` URLs.

use super::escaping::url_encode;

/// Builds a `/{repo}?view=...` URL. A builder instead of a long positional
/// argument list: the view surface grew past `ref`/`path`/`sha` into `skip`
/// too, and most call sites only need one or two of them.
pub struct RepoLink<'a> {
    repo: &'a str,
    view: &'a str,
    rev: &'a str,
    path: &'a str,
    sha: Option<&'a str>,
    skip: Option<usize>,
    tag: Option<&'a str>,
    number: Option<u64>,
}

pub fn repo_link<'a>(repo: &'a str, view: &'a str, rev: &'a str) -> RepoLink<'a> {
    RepoLink {
        repo,
        view,
        rev,
        path: "",
        sha: None,
        skip: None,
        tag: None,
        number: None,
    }
}

impl<'a> RepoLink<'a> {
    pub(crate) fn path(mut self, path: &'a str) -> Self {
        self.path = path;
        self
    }

    pub fn sha(mut self, sha: &'a str) -> Self {
        self.sha = Some(sha);
        self
    }

    pub(crate) fn skip(mut self, skip: usize) -> Self {
        self.skip = Some(skip);
        self
    }

    pub fn tag(mut self, tag: &'a str) -> Self {
        self.tag = Some(tag);
        self
    }

    pub(crate) fn number(mut self, number: u64) -> Self {
        self.number = Some(number);
        self
    }
}

impl std::fmt::Display for RepoLink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "/{}?view={}", url_encode(self.repo), self.view)?;
        if !self.rev.is_empty() {
            write!(f, "&ref={}", url_encode(self.rev))?;
        }
        if !self.path.is_empty() {
            write!(f, "&path={}", url_encode(self.path))?;
        }
        if let Some(sha) = self.sha {
            write!(f, "&sha={}", url_encode(sha))?;
        }
        if let Some(skip) = self.skip {
            write!(f, "&skip={skip}")?;
        }
        if let Some(tag) = self.tag {
            write!(f, "&tag={}", url_encode(tag))?;
        }
        if let Some(number) = self.number {
            write!(f, "&number={number}")?;
        }
        Ok(())
    }
}

/// `/{repo}/blob/{branch}/{path}` — the GitHub-style sibling of
/// `repo_link(repo, "blob", rev).path(path)`. `handler_repo` resolves this
/// form by checking real ref names (see `compound_path`), so `branch` may
/// itself contain `/` without breaking the split — the same ambiguity
/// GitHub's own `/blob/` route has once a branch name has a slash in it.
pub fn github_blob_link(repo: &str, branch: &str, path: &str) -> String {
    format!("/{}/blob/{}/{}", url_encode(repo), url_encode(branch), url_encode(path))
}
