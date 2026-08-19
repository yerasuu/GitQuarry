import json

from gitquarry_ci import JobKind, graph, job
from gitquarry_ci.graph import _to_dict


def test_kind_defaults_to_build() -> None:
    j = job("a").image("img").run("x")
    assert j._spec.kind == JobKind.BUILD


def test_kind_can_be_overridden() -> None:
    j = job("a").image("img").run("x").kind(JobKind.DEPLOY)
    assert j._spec.kind == JobKind.DEPLOY


def test_secret_never_stores_a_value_only_env_key_and_secret_name() -> None:
    j = job("deploy").image("img").run("x").secret("API_TOKEN", "prod-token")
    assert len(j._spec.secrets) == 1
    assert j._spec.secrets[0].env_key == "API_TOKEN"
    assert j._spec.secrets[0].secret_name == "prod-token"
    # SecretRef has exactly these two fields -- there is no "value" slot at all.
    assert {f for f in vars(j._spec.secrets[0])} == {"env_key", "secret_name"}


def test_for_each_dir_runs_once_per_distinct_top_level_dir_first_seen_order() -> None:
    paths = [
        "crates/a/src/lib.rs",
        "crates/a/src/other.rs",
        "crates/b/src/lib.rs",
        "docs/readme.md",
    ]
    seen = []

    def make(dir: str):
        seen.append(dir)
        return job(f"test-{dir}").image("rust:1.80").run("cargo test")

    graph().for_each_dir(paths, make)
    assert seen == ["crates", "docs"]


def test_valid_graph_dump_produces_expected_json(capsys, monkeypatch) -> None:
    import pytest

    g = graph().add_job(job("test-a").image("rust:1.80").run("cargo test -p a")).add_job(
        job("build-a").image("rust:1.80").run("cargo build -p a").needs("test-a")
    )

    with pytest.raises(SystemExit) as exc:
        g.dump()
    assert exc.value.code == 0

    out = capsys.readouterr().out
    parsed = json.loads(out)
    assert parsed["schema_version"] == 1
    assert len(parsed["jobs"]) == 2
    assert parsed["jobs"][0]["id"] == "test-a"
    assert parsed["jobs"][0]["kind"] == "build"
    assert parsed["jobs"][1]["needs"] == ["test-a"]


def test_invalid_graph_dump_exits_nonzero(capsys) -> None:
    import pytest

    g = graph().add_job(job("a").image("img").run("x").needs("b")).add_job(
        job("b").image("img").run("x").needs("a")
    )

    with pytest.raises(SystemExit) as exc:
        g.dump()
    assert exc.value.code == 1
    assert "cycle" in capsys.readouterr().err


def test_add_job_marks_builder_consumed() -> None:
    j = job("a").image("img").run("x")
    assert j._consumed is False
    graph().add_job(j)
    assert j._consumed is True


def test_to_dict_serializes_enum_as_value() -> None:
    j = job("a").image("img").run("x")
    d = _to_dict(j._mark_consumed())
    assert d["kind"] == "build"
    assert isinstance(d["kind"], str)
