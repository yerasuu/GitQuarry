"""Structural validation for a `JobGraph`: duplicate ids, dangling `needs`,
cycles, empty `image`, job-count bounds.

This is an independent reimplementation of the same algorithm as the Rust
side's `gitquarry-graph-validate` crate -- no code sharing by design. The
cross-language contract test (built elsewhere, not part of this package) is
what's meant to keep the two in sync, not shared code.
"""

from __future__ import annotations

from enum import Enum, auto

from gitquarry_ci.wire_generated import JobSpec

# Mirrors `DEFAULT_MAX_JOBS` in crates/gitquarry-graph-validate/src/lib.rs.
# Not shared code -- just the same number, kept in sync by the contract test.
MAX_JOBS = 500


class GraphError(Exception):
    """Raised by `validate_graph` listing every problem found, not just the
    first one -- a pipeline author fixing graph errors is better served by
    seeing them all at once.
    """

    def __init__(self, problems: list[str]) -> None:
        self.problems = problems
        super().__init__("\n".join(problems))


def check_unique_ids(jobs: list[JobSpec]) -> list[str]:
    seen: set[str] = set()
    problems = []
    for j in jobs:
        if j.id in seen:
            problems.append(f"duplicate job id: {j.id}")
        seen.add(j.id)
    return problems


def check_needs_exist(jobs: list[JobSpec]) -> list[str]:
    ids = {j.id for j in jobs}
    problems = []
    for j in jobs:
        for need in j.needs:
            if need not in ids:
                problems.append(f"job {j.id} needs unknown job {need}")
    return problems


def check_nonempty_image(jobs: list[JobSpec]) -> list[str]:
    return [f"job {j.id} has an empty image" for j in jobs if not j.image.strip()]


def check_resource_bounds(jobs: list[JobSpec], max_jobs: int = MAX_JOBS) -> list[str]:
    if len(jobs) > max_jobs:
        return [f"graph has {len(jobs)} jobs, exceeding the max of {max_jobs}"]
    return []


class _Color(Enum):
    WHITE = auto()
    GRAY = auto()
    BLACK = auto()


def check_no_cycles(jobs: list[JobSpec]) -> list[str]:
    """DFS with white/gray/black 3-coloring over the `needs` graph.

    Only visits ids that are actually present -- a dangling `need` is
    `check_needs_exist`'s problem, not this function's, so it must not also
    spuriously trigger a cycle report here.
    """
    index = {j.id: i for i, j in enumerate(jobs)}
    color = [_Color.WHITE] * len(jobs)
    problems: list[str] = []

    def visit(at: int, path: list[str]) -> None:
        color[at] = _Color.GRAY
        path.append(jobs[at].id)

        for need in jobs[at].needs:
            next_idx = index.get(need)
            if next_idx is None:
                continue
            if color[next_idx] == _Color.WHITE:
                visit(next_idx, path)
            elif color[next_idx] == _Color.GRAY:
                cycle = [*path, jobs[next_idx].id]
                problems.append(f"cycle in needs graph: {' -> '.join(cycle)}")
            # BLACK: already fully explored, nothing to do.

        path.pop()
        color[at] = _Color.BLACK

    for start in range(len(jobs)):
        if color[start] == _Color.WHITE:
            visit(start, [])

    return problems


def validate_graph(jobs: list[JobSpec], max_jobs: int = MAX_JOBS) -> None:
    """Runs every check and raises `GraphError` listing all failures at once,
    rather than stopping at the first one. Returns None on success.
    """
    problems: list[str] = []
    problems.extend(check_unique_ids(jobs))
    problems.extend(check_needs_exist(jobs))
    problems.extend(check_no_cycles(jobs))
    problems.extend(check_nonempty_image(jobs))
    problems.extend(check_resource_bounds(jobs, max_jobs))

    if problems:
        raise GraphError(problems)
