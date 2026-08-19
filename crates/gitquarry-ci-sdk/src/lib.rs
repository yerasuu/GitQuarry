//! Rust SDK for `gitquarry-ci` pipeline definitions. Never executes
//! anything — builds a `JobGraph` and dumps it as JSON to stdout when run
//! with `--dump-graph`. `gitquarry-ci` is the sole executor of the
//! resulting graph, regardless of which SDK/language produced it.
//! See `docs/gitquarry-ci-sdk-design.md`.

pub mod ansible;
mod context;
mod graph;
mod job;
// The lint fires on typify's own generated Default impl for JobKind, which
// we don't hand-edit.
#[allow(clippy::derivable_impls)]
mod wire_generated;

pub use context::{PipelineContext, context};
pub use graph::{GraphBuilder, graph};
pub use job::{JobBuilder, job};
pub use wire_generated::{JobGraph, JobKind, JobSpec, PipelineTrigger, SecretRef};

#[cfg(test)]
mod tests;
