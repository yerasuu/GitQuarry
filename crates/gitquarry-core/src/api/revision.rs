//! The commit every `/_api/v1` repo read answers at (decision #144), so an
//! agent can repeat a read and get the same answer.
//!
//! A repo read takes `?rev=` as a [`RevisionRequest`], resolves it once
//! with [`RevisionRequest::resolve`], reads at [`Revision::sha`] (never at
//! `rev` again, or a ref that moves mid-request would mix two commits into
//! one answer), and answers with the sha in a top-level `revision` field,
//! [`Pinned`] being that shape. Sending that sha back as `?rev=` gives the
//! same answer for as long as the commit exists.

use super::error::{ApiError, ErrorCode};
use crate::git::browse;
use crate::validate;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::Serialize;
use std::path::Path;

/// `?rev=`: a branch, tag, sha or revision expression `validate_rev`
/// accepts. Absent or empty means the repo's default branch.
#[derive(Debug)]
pub struct RevisionRequest {
    rev: Option<String>,
}

impl RevisionRequest {
    pub fn rev(&self) -> Option<&str> {
        self.rev.as_deref()
    }

    /// The full sha of the commit this request names in the repo at
    /// `git_dir`, an annotated tag dereferenced to its commit.
    ///
    /// # Errors
    ///
    /// `not_found` for a rev the repo doesn't have, or a repo with no
    /// commits yet when no rev was given; `internal` if git can't be run.
    pub async fn resolve(&self, git_dir: &Path) -> Result<Revision, ApiError> {
        let rev = match self.rev.as_deref() {
            Some(rev) => rev.to_string(),
            None => browse::resolve_head(git_dir)
                .await
                .map_err(|e| ApiError::internal(&e))?
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::NotFound, "the repo has no commits yet").with_hint("push a commit first")
                })?,
        };
        let sha = browse::resolve_commit(git_dir, &rev)
            .await
            .map_err(|e| ApiError::internal(&e))?
            .ok_or_else(|| {
                ApiError::new(ErrorCode::NotFound, "no such revision in this repo")
                    .with_hint("pass a branch, tag or commit sha the repo has; omit `rev` for the default branch")
            })?;
        Ok(Revision(sha))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RevisionRequest {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let mut rev = None;
        for (key, value) in form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes()) {
            if key == "rev" {
                rev = (!value.is_empty()).then(|| value.into_owned());
            }
        }
        if let Some(rev) = rev.as_deref() {
            validate::validate_rev(rev).map_err(|e| {
                ApiError::new(ErrorCode::InvalidParameter, e.to_string())
                    .with_hint("pass a branch, tag or commit sha; ranges are not accepted")
            })?;
        }
        Ok(RevisionRequest { rev })
    }
}

/// A full commit sha, as [`RevisionRequest::resolve`] found it. Serialized
/// as the bare sha string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Revision(String);

impl Revision {
    pub fn sha(&self) -> &str {
        &self.0
    }
}

/// A repo read's answer: `body`'s fields with `revision` beside them.
/// `body` must serialize as a JSON object, and must not have a field of
/// its own named `revision`.
#[derive(Debug, Serialize)]
pub struct Pinned<'a, T> {
    pub revision: &'a Revision,
    #[serde(flatten)]
    pub body: T,
}
