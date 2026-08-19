"""Fixture: two jobs, `test` then `build` (`.needs("test")`), both default
`kind`. Python-side twin of `rust_fixture_runner/src/bin/simple.rs` -- the
contract test compares their dumped `JobGraph` JSON for equality.
"""

from gitquarry_ci import graph, job


def main() -> None:
    g = graph()
    g.add_job(job("test").image("rust:1.80").run("cargo test"))
    g.add_job(job("build").image("rust:1.80").run("cargo build").needs("test"))
    g.dump()


if __name__ == "__main__":
    main()
