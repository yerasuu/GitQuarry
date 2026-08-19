"""Builds a `JobGraph`.

Mutable, chained, same reasoning as `JobBuilder`: this SDK's process runs,
dumps, and dies in seconds, so a builder pattern with mutation is simpler
than threading immutable state through.
"""

from __future__ import annotations

import json
import sys
from collections.abc import Callable
from dataclasses import fields, is_dataclass
from enum import Enum
from typing import Any

from gitquarry_ci.job import JobBuilder
from gitquarry_ci.validate import MAX_JOBS, GraphError, validate_graph
from gitquarry_ci.wire_generated import JobSpec


class GraphBuilder:
    def __init__(self) -> None:
        self._jobs: list[JobSpec] = []

    def add_job(self, job: JobBuilder) -> GraphBuilder:
        self._jobs.append(job._mark_consumed())
        return self

    def for_each_dir(self, paths: list[str], fn: Callable[[str], JobBuilder]) -> GraphBuilder:
        """Sugar for monorepos: calls `fn` once per distinct top-level
        directory among `paths` (typically `PipelineContext.changed_paths`),
        in first-seen order, adding whatever job it returns.
        """
        seen: set[str] = set()
        for path in paths:
            top_dir = path.split("/", 1)[0]
            if top_dir not in seen:
                seen.add(top_dir)
                self._jobs.append(fn(top_dir)._mark_consumed())
        return self

    def validate(self, max_jobs: int = MAX_JOBS) -> None:
        """Raises `GraphError` on failure; returns None on success."""
        validate_graph(self._jobs, max_jobs)

    def dump(self) -> None:
        """Validates then serializes the graph to stdout. Never returns
        normally -- this is the SDK's sole output; `gitquarry-ci` is the
        only executor of what gets dumped here, regardless of which SDK
        produced it.
        """
        try:
            self.validate()
        except GraphError as e:
            for problem in e.problems:
                print(f"gitquarry-ci-sdk: {problem}", file=sys.stderr)
            sys.exit(1)

        graph = {"schema_version": 1, "jobs": [_to_dict(j) for j in self._jobs]}
        sys.stdout.write(json.dumps(graph))
        sys.exit(0)


def graph() -> GraphBuilder:
    return GraphBuilder()


def _to_dict(value: Any) -> Any:
    """Turns nested dataclasses (`JobSpec`, `SecretRef`), enums (`JobKind`),
    lists, and dicts into a plain JSON-serializable tree. `json.dumps` can't
    handle dataclasses or enums directly.
    """
    if is_dataclass(value) and not isinstance(value, type):
        return {f.name: _to_dict(getattr(value, f.name)) for f in fields(value)}
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, list):
        return [_to_dict(item) for item in value]
    if isinstance(value, dict):
        return {key: _to_dict(item) for key, item in value.items()}
    return value
