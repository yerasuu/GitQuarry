//! Fixture: one job per distinct top-level directory among
//! `PipelineContext::changed_paths`, exercising `GraphBuilder::for_each_dir`.
//! Rust-side twin of `python_fixture_runner/monorepo_dynamic.py`. Requires
//! the `GITQUARRY_CI_*` env vars (see `run.sh`) — `context()` panics without
//! them.

use gitquarry_ci_sdk::{context, graph, job};

fn main() {
    let ctx = context();
    let g = graph().for_each_dir(ctx.changed_paths(), |dir| {
        job(format!("test-{dir}"))
            .image("rust:1.80")
            .run(format!("cargo test -p {dir}"))
    });
    g.dump();
}
