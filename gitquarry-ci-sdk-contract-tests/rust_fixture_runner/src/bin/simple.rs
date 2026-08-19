//! Fixture: two jobs, `test` then `build` (`.needs("test")`), both default
//! `kind`. Rust-side twin of `python_fixture_runner/simple.py` — the
//! contract test compares their dumped `JobGraph` JSON for equality.

use gitquarry_ci_sdk::{graph, job};

fn main() {
    let g = graph()
        .add_job(job("test").image("rust:1.80").run("cargo test"))
        .add_job(job("build").image("rust:1.80").run("cargo build").needs("test"));
    g.dump();
}
