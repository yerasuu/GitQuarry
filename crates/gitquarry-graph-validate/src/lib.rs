//! Structural validation for a `JobGraph`: duplicate ids, dangling `needs`,
//! cycles, empty `image`, job-count bounds. Consumed by `gitquarry-ci-sdk`
//! (Rust SDK) and by `gitquarry-ci`'s own server-side graph handling —
//! Rust-to-Rust sharing only. The Python SDK reimplements the same
//! algorithm independently in `validate.py`; the contract test in
//! `gitquarry-ci-sdk-contract-tests` is what keeps the two in sync, not
//! shared code (see `docs/gitquarry-ci-sdk-design.md`).

use std::collections::HashMap;
use std::fmt;

/// Default cap on jobs in a single graph — mirrored numerically (not
/// shared) by Python's `MAX_JOBS` in `validate.py`.
pub const DEFAULT_MAX_JOBS: usize = 500;

/// The minimal shape this crate needs out of a job to validate it. Callers
/// (the SDK's generated wire type, the server's hand-written `JobSpec`)
/// each build a slice of these rather than sharing a concrete `JobSpec`
/// type — keeps this crate decoupled from either side's own type.
pub struct JobNode<'a> {
    pub id: &'a str,
    pub image: &'a str,
    pub needs: &'a [String],
}

#[derive(Debug, PartialEq, Eq)]
pub enum GraphValidationError {
    DuplicateId(String),
    UnknownNeed { job: String, needs: String },
    Cycle(Vec<String>),
    EmptyImage(String),
    TooManyJobs { count: usize, max: usize },
}

impl fmt::Display for GraphValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(f, "duplicate job id: {id}"),
            Self::UnknownNeed { job, needs } => {
                write!(f, "job {job} needs unknown job {needs}")
            }
            Self::Cycle(path) => write!(f, "cycle in needs graph: {}", path.join(" -> ")),
            Self::EmptyImage(id) => write!(f, "job {id} has an empty image"),
            Self::TooManyJobs { count, max } => {
                write!(f, "graph has {count} jobs, exceeding the max of {max}")
            }
        }
    }
}

impl std::error::Error for GraphValidationError {}

pub fn check_unique_ids(nodes: &[JobNode]) -> Vec<GraphValidationError> {
    let mut seen = HashMap::new();
    let mut errors = Vec::new();
    for node in nodes {
        if seen.insert(node.id, ()).is_some() {
            errors.push(GraphValidationError::DuplicateId(node.id.to_string()));
        }
    }
    errors
}

pub fn check_needs_exist(nodes: &[JobNode]) -> Vec<GraphValidationError> {
    let ids: std::collections::HashSet<&str> = nodes.iter().map(|n| n.id).collect();
    let mut errors = Vec::new();
    for node in nodes {
        for need in node.needs {
            if !ids.contains(need.as_str()) {
                errors.push(GraphValidationError::UnknownNeed {
                    job: node.id.to_string(),
                    needs: need.clone(),
                });
            }
        }
    }
    errors
}

pub fn check_nonempty_image(nodes: &[JobNode]) -> Vec<GraphValidationError> {
    nodes
        .iter()
        .filter(|n| n.image.trim().is_empty())
        .map(|n| GraphValidationError::EmptyImage(n.id.to_string()))
        .collect()
}

pub fn check_resource_bounds(nodes: &[JobNode], max_jobs: usize) -> Vec<GraphValidationError> {
    if nodes.len() > max_jobs {
        vec![GraphValidationError::TooManyJobs {
            count: nodes.len(),
            max: max_jobs,
        }]
    } else {
        Vec::new()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

/// DFS with white/gray/black coloring. Only runs against ids that are
/// actually present (dangling `needs` are `check_needs_exist`'s job, not
/// this one's) so a bad `needs` doesn't also spuriously trigger a cycle.
pub fn check_no_cycles(nodes: &[JobNode]) -> Vec<GraphValidationError> {
    let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    let mut color = vec![Color::White; nodes.len()];
    let mut errors = Vec::new();

    for start in 0..nodes.len() {
        if color[start] == Color::White {
            let mut path = Vec::new();
            visit(start, nodes, &index, &mut color, &mut path, &mut errors);
        }
    }
    errors
}

fn visit(
    at: usize,
    nodes: &[JobNode],
    index: &HashMap<&str, usize>,
    color: &mut [Color],
    path: &mut Vec<String>,
    errors: &mut Vec<GraphValidationError>,
) {
    color[at] = Color::Gray;
    path.push(nodes[at].id.to_string());

    for need in nodes[at].needs {
        let Some(&next) = index.get(need.as_str()) else {
            continue;
        };
        match color[next] {
            Color::White => visit(next, nodes, index, color, path, errors),
            Color::Gray => {
                let mut cycle = path.clone();
                cycle.push(nodes[next].id.to_string());
                errors.push(GraphValidationError::Cycle(cycle));
            }
            Color::Black => {}
        }
    }

    path.pop();
    color[at] = Color::Black;
}

/// Runs every check and collects all failures rather than stopping at the
/// first one — a pipeline author fixing graph errors one at a time instead
/// of one-per-run is worth the extra bookkeeping.
pub fn validate_graph(nodes: &[JobNode], max_jobs: usize) -> Result<(), Vec<GraphValidationError>> {
    let mut errors = Vec::new();
    errors.extend(check_unique_ids(nodes));
    errors.extend(check_needs_exist(nodes));
    errors.extend(check_no_cycles(nodes));
    errors.extend(check_nonempty_image(nodes));
    errors.extend(check_resource_bounds(nodes, max_jobs));

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

#[cfg(test)]
mod tests;
