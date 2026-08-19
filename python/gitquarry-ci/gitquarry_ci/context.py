"""Populated from env vars `gitquarry-ci` injects into every pipeline
resolution container. No network I/O of any kind -- everything a pipeline
needs to decide what to build is handed to it up front.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass


@dataclass
class PipelineContext:
    sha: str
    refname: str
    pusher: str
    repo_uid: str
    changed_paths: list[str]
    is_fork_pr: bool


def context() -> PipelineContext:
    return PipelineContext(
        sha=_required_env("GITQUARRY_CI_SHA"),
        refname=_required_env("GITQUARRY_CI_REFNAME"),
        pusher=_required_env("GITQUARRY_CI_PUSHER"),
        repo_uid=_required_env("GITQUARRY_CI_REPO_UID"),
        changed_paths=_parse_changed_paths(_required_env("GITQUARRY_CI_CHANGED_PATHS_JSON")),
        is_fork_pr=_required_env("GITQUARRY_CI_IS_FORK_PR") == "true",
    )


def _required_env(key: str) -> str:
    """A missing env var here is a bug in `gitquarry-ci`'s own container
    injection, not a recoverable condition a pipeline author can work
    around -- hence a hard error with an explicit message rather than a
    silently-defaulted value.
    """
    try:
        return os.environ[key]
    except KeyError:
        raise RuntimeError(
            f"{key} is not set -- gitquarry-ci is expected to inject this into every "
            "pipeline resolution container; a missing value here is a bug in "
            "gitquarry-ci itself, not something the pipeline author can fix"
        ) from None


def _parse_changed_paths(raw: str) -> list[str]:
    try:
        parsed: list[str] = json.loads(raw)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"GITQUARRY_CI_CHANGED_PATHS_JSON is not valid JSON: {e}") from e
    return parsed
