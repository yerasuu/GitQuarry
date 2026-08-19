import pytest

from gitquarry_ci.validate import GraphError, check_no_cycles, validate_graph
from gitquarry_ci.wire_generated import JobSpec


def make_job(id: str, image: str = "img", needs: list[str] | None = None) -> JobSpec:
    return JobSpec(id=id, image=image, commands=["x"], needs=needs or [])


def test_valid_graph_passes() -> None:
    jobs = [make_job("a"), make_job("b", needs=["a"])]
    validate_graph(jobs)  # must not raise


def test_rejects_duplicate_ids() -> None:
    jobs = [make_job("a"), make_job("a")]
    with pytest.raises(GraphError) as exc:
        validate_graph(jobs)
    assert any("duplicate job id: a" in p for p in exc.value.problems)


def test_rejects_dangling_needs() -> None:
    jobs = [make_job("a", needs=["missing"])]
    with pytest.raises(GraphError) as exc:
        validate_graph(jobs)
    assert any("needs unknown job missing" in p for p in exc.value.problems)


def test_dangling_need_is_not_also_reported_as_a_cycle() -> None:
    jobs = [make_job("a", needs=["missing"])]
    assert check_no_cycles(jobs) == []


def test_rejects_a_cycle() -> None:
    jobs = [make_job("a", needs=["b"]), make_job("b", needs=["a"])]
    with pytest.raises(GraphError) as exc:
        validate_graph(jobs)
    assert any("cycle in needs graph" in p for p in exc.value.problems)


def test_rejects_empty_image() -> None:
    jobs = [make_job("a", image="  ")]
    with pytest.raises(GraphError) as exc:
        validate_graph(jobs)
    assert any("has an empty image" in p for p in exc.value.problems)


def test_rejects_too_many_jobs() -> None:
    jobs = [make_job(f"job-{i}") for i in range(5)]
    with pytest.raises(GraphError) as exc:
        validate_graph(jobs, max_jobs=3)
    assert any("exceeding the max of 3" in p for p in exc.value.problems)


def test_reports_all_problems_at_once() -> None:
    jobs = [make_job("a", image=""), make_job("a", needs=["missing"])]
    with pytest.raises(GraphError) as exc:
        validate_graph(jobs)
    # duplicate id, dangling need, and empty image should all show up together
    assert len(exc.value.problems) >= 3
