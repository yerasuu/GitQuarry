use std::env;

/// Populated from env vars `gitquarry-ci` injects into every pipeline
/// resolution container. No network I/O of any kind — everything a
/// pipeline needs to decide what to build is handed to it up front.
pub struct PipelineContext {
    sha: String,
    refname: String,
    pusher: String,
    repo_uid: String,
    changed_paths: Vec<String>,
    is_fork_pr: bool,
}

pub fn context() -> PipelineContext {
    PipelineContext::from_env()
}

impl PipelineContext {
    fn from_env() -> Self {
        Self {
            sha: required_env("GITQUARRY_CI_SHA"),
            refname: required_env("GITQUARRY_CI_REFNAME"),
            pusher: required_env("GITQUARRY_CI_PUSHER"),
            repo_uid: required_env("GITQUARRY_CI_REPO_UID"),
            changed_paths: parse_changed_paths(&required_env("GITQUARRY_CI_CHANGED_PATHS_JSON")),
            is_fork_pr: required_env("GITQUARRY_CI_IS_FORK_PR") == "true",
        }
    }

    pub fn sha(&self) -> &str {
        &self.sha
    }

    pub fn refname(&self) -> &str {
        &self.refname
    }

    pub fn pusher(&self) -> &str {
        &self.pusher
    }

    pub fn repo_uid(&self) -> &str {
        &self.repo_uid
    }

    pub fn changed_paths(&self) -> &[String] {
        &self.changed_paths
    }

    pub fn is_fork_pr(&self) -> bool {
        self.is_fork_pr
    }
}

/// A missing env var here is a bug in `gitquarry-ci`'s own container
/// injection, not a recoverable condition a pipeline author can work
/// around — hence panic with an explicit message rather than a `Result`.
fn required_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| {
        panic!(
            "{key} is not set — gitquarry-ci is expected to inject this into every pipeline \
             resolution container; a missing value here is a bug in gitquarry-ci itself"
        )
    })
}

fn parse_changed_paths(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("GITQUARRY_CI_CHANGED_PATHS_JSON is not valid JSON: {e}"))
}
