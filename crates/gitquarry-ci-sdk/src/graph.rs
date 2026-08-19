use std::collections::BTreeSet;
use std::process::exit;

use gitquarry_graph_validate::{DEFAULT_MAX_JOBS, JobNode, validate_graph};

use crate::job::JobBuilder;
use crate::wire_generated::{JobGraph, JobSpec};

/// Builds a [`JobGraph`]. Mutable, chained, same reasoning as [`JobBuilder`].
#[must_use = "a GraphBuilder does nothing until dump()ed"]
#[derive(Default)]
pub struct GraphBuilder {
    jobs: Vec<JobSpec>,
}

pub fn graph() -> GraphBuilder {
    GraphBuilder::default()
}

impl GraphBuilder {
    pub fn add_job(mut self, job: JobBuilder) -> Self {
        self.jobs.push(job.into_spec());
        self
    }

    /// Sugar for monorepos: calls `f` once per distinct top-level directory
    /// among `paths` (typically [`PipelineContext::changed_paths`]),
    /// adding whatever job it returns.
    pub fn for_each_dir<F>(mut self, paths: &[String], mut f: F) -> Self
    where
        F: FnMut(&str) -> JobBuilder,
    {
        let mut seen = BTreeSet::new();
        for path in paths {
            if let Some(dir) = path.split('/').next()
                && seen.insert(dir.to_string())
            {
                self.jobs.push(f(dir).into_spec());
            }
        }
        self
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let nodes: Vec<JobNode> = self
            .jobs
            .iter()
            .map(|j| JobNode {
                id: &j.id,
                image: &j.image,
                needs: &j.needs,
            })
            .collect();
        validate_graph(&nodes, DEFAULT_MAX_JOBS).map_err(|errors| errors.iter().map(ToString::to_string).collect())
    }

    pub(crate) fn into_graph(self) -> JobGraph {
        JobGraph {
            schema_version: 1,
            jobs: self.jobs,
        }
    }

    /// Validates then serializes the graph to stdout. Never returns — this
    /// is the SDK's sole output; `gitquarry-ci` is the only executor of
    /// what gets dumped here, regardless of which SDK produced it.
    pub fn dump(self) -> ! {
        if let Err(errors) = self.validate() {
            for e in &errors {
                eprintln!("gitquarry-ci-sdk: {e}");
            }
            exit(1);
        }

        let graph = self.into_graph();
        match serde_json::to_writer(std::io::stdout(), &graph) {
            Ok(()) => exit(0),
            Err(e) => {
                eprintln!("gitquarry-ci-sdk: failed to serialize job graph: {e}");
                exit(1);
            }
        }
    }
}
