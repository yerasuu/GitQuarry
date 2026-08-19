//! Server-side wire shape for a resolved pipeline job graph — hand-written,
//! independent of `gitquarry-ci-sdk`'s own `wire_generated.rs` (which
//! `typify`-generates the identical shape from the same JSON schema at
//! `gitquarry-wire/schema/pipeline/v1/`). No shared code between the two by
//! design: the schema is the single source of truth, kept in sync by a
//! contract test, not a shared Rust type. `gitquarry-ci` (the server)
//! deserializes a `JobGraph` dumped by either SDK into these types.
//!
//! Only what `gitquarry-ci`'s own graph handling needs crosses this
//! boundary — see `docs/ci-cd-concept.md` (Phase 2) and
//! `docs/gitquarry-ci-sdk-design.md` (`kind` field, Fase 6).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    #[default]
    Build,
    Test,
    Deploy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    pub env_key: String,
    pub secret_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    pub id: String,
    pub image: String,
    pub commands: Vec<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub secrets: Vec<SecretRef>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Every `deploy` job requires maintainer approval before it runs
    /// unless the repo opted out — see `gitquarry_core::storage::entities`'s
    /// `ci_deploy_gate_disabled` and `gitquarry_ci`'s graph handling.
    #[serde(default)]
    pub kind: JobKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobGraph {
    pub schema_version: u32,
    pub jobs: Vec<JobSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineTrigger {
    pub repo_uid: crate::Uid,
    pub sha: String,
    pub refname: String,
    pub pusher: String,
    pub changed_paths: Vec<String>,
    pub is_fork_pr: bool,
}
