"""Fixture: one job per distinct top-level directory among
`PipelineContext.changed_paths`, exercising `GraphBuilder.for_each_dir`.
Python-side twin of `rust_fixture_runner/src/bin/monorepo_dynamic.rs`.
Requires the `GITQUARRY_CI_*` env vars (see `run.sh`) -- `context()` raises
without them.
"""

from gitquarry_ci import context, graph, job


def main() -> None:
    ctx = context()
    g = graph().for_each_dir(
        ctx.changed_paths,
        lambda top_dir: job(f"test-{top_dir}").image("rust:1.80").run(f"cargo test -p {top_dir}"),
    )
    g.dump()


if __name__ == "__main__":
    main()
