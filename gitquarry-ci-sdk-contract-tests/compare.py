#!/usr/bin/env python3
"""Contract-test comparator invoked by `run.sh`.

Two modes:

    compare.py validate <json-file>
    compare.py compare <fixture-name> <rust-json-file> <python-json-file>

`validate` checks a single `JobGraph` blob against
`gitquarry-wire/schema/pipeline/v1/job-graph.schema.json` (real JSON-Schema
validation via the `jsonschema` package when it is importable; otherwise a
hand-rolled structural check -- required keys present, no keys beyond the
schema's `additionalProperties: false` set -- printing a note that it fell
back).

`compare` checks two blobs (one produced by the Rust SDK, one by the Python
SDK) for logical equality:

- `jobs` array order never matters -- both are sorted by `id` first.
- `commands` array order **within** a job matters -- compared as-is.
- `needs`/`env`/`secrets`/`artifacts` are normalized to the schema's empty
  default when absent before comparing: Rust's serde output skips these
  keys entirely when empty (`skip_serializing_if`), while the Python SDK's
  dataclass-based `_to_dict` always includes them. Both are equally valid
  encodings of "no needs" / "no env" / etc., so a bare key-presence diff
  here is not a real mismatch.
- every other field (`schema_version`, `id`, `image`, `commands`, `kind`,
  and each `needs`/`env`/`secrets`/`artifacts` value) compares directly.

Exits non-zero with a message naming the fixture and the exact diff on any
mismatch.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

SCHEMA_DIR = Path(__file__).resolve().parent.parent / "crates/gitquarry-wire/schema/pipeline/v1"

# Schema defaults for JobSpec's optional fields -- see job-spec.schema.json.
_JOB_SPEC_DEFAULTS: dict[str, Any] = {
    "needs": [],
    "env": {},
    "secrets": [],
    "artifacts": [],
    "kind": "build",
}
_JOB_SPEC_ALLOWED_KEYS = {"id", "image", "commands", *_JOB_SPEC_DEFAULTS}
_JOB_SPEC_REQUIRED_KEYS = {"id", "image", "commands"}
_JOB_GRAPH_ALLOWED_KEYS = {"schema_version", "jobs"}
_JOB_GRAPH_REQUIRED_KEYS = {"schema_version", "jobs"}
_SECRET_REF_ALLOWED_KEYS = {"env_key", "secret_name"}
_SECRET_REF_REQUIRED_KEYS = {"env_key", "secret_name"}


def fail(message: str) -> None:
    print(f"gitquarry-ci-sdk-contract-tests: {message}", file=sys.stderr)
    sys.exit(1)


def load_json(path: Path, label: str) -> Any:
    text = path.read_text()
    try:
        return json.loads(text)
    except json.JSONDecodeError as e:
        fail(f"{label}: not valid JSON: {e}\n--- raw output ---\n{text}")
        raise  # unreachable, keeps type-checkers happy


def structural_validate(graph: Any, label: str) -> None:
    """Hand-rolled fallback: required keys present, no keys beyond what the
    schema's `additionalProperties: false` permits, at every nesting level.
    """
    if not isinstance(graph, dict):
        fail(f"{label}: top-level JobGraph must be an object, got {type(graph).__name__}")
    missing = _JOB_GRAPH_REQUIRED_KEYS - graph.keys()
    if missing:
        fail(f"{label}: JobGraph missing required key(s): {sorted(missing)}")
    extra = graph.keys() - _JOB_GRAPH_ALLOWED_KEYS
    if extra:
        fail(f"{label}: JobGraph has unexpected key(s) not in schema: {sorted(extra)}")
    if graph["schema_version"] != 1:
        fail(f"{label}: schema_version must be the constant 1, got {graph['schema_version']!r}")
    if not isinstance(graph["jobs"], list):
        fail(f"{label}: jobs must be an array")

    for job in graph["jobs"]:
        if not isinstance(job, dict):
            fail(f"{label}: each job must be an object, got {type(job).__name__}")
        job_missing = _JOB_SPEC_REQUIRED_KEYS - job.keys()
        if job_missing:
            fail(f"{label}: job {job.get('id', '<no id>')!r} missing required key(s): {sorted(job_missing)}")
        job_extra = job.keys() - _JOB_SPEC_ALLOWED_KEYS
        if job_extra:
            fail(f"{label}: job {job.get('id', '<no id>')!r} has unexpected key(s) not in schema: {sorted(job_extra)}")
        if "kind" in job and job["kind"] not in ("build", "test", "deploy"):
            fail(f"{label}: job {job['id']!r} has invalid kind {job['kind']!r}")
        for secret in job.get("secrets", []):
            if not isinstance(secret, dict):
                fail(f"{label}: job {job['id']!r} has a non-object secret entry")
            secret_missing = _SECRET_REF_REQUIRED_KEYS - secret.keys()
            if secret_missing:
                fail(f"{label}: job {job['id']!r} secret missing required key(s): {sorted(secret_missing)}")
            secret_extra = secret.keys() - _SECRET_REF_ALLOWED_KEYS
            if secret_extra:
                fail(f"{label}: job {job['id']!r} secret has unexpected key(s): {sorted(secret_extra)}")


def schema_validate(graph: Any, label: str) -> None:
    try:
        import jsonschema
        from referencing import Registry, Resource
    except ImportError:
        print(
            f"gitquarry-ci-sdk-contract-tests: note: 'jsonschema' package not installed -- "
            f"falling back to hand-rolled structural validation for {label}",
            file=sys.stderr,
        )
        structural_validate(graph, label)
        return

    schemas = {}
    for name in ["job-graph", "job-spec", "job-kind", "secret-ref", "pipeline-trigger"]:
        path = SCHEMA_DIR / f"{name}.schema.json"
        schema = json.loads(path.read_text())
        schemas[schema["$id"]] = schema

    registry = Registry().with_resources((uri, Resource.from_contents(s)) for uri, s in schemas.items())
    validator_cls = jsonschema.validators.validator_for(schemas["https://gitquarry/schema/pipeline/v1/job-graph.schema.json"])
    validator = validator_cls(
        schemas["https://gitquarry/schema/pipeline/v1/job-graph.schema.json"],
        registry=registry,
    )
    errors = sorted(validator.iter_errors(graph), key=lambda e: list(e.path))
    if errors:
        details = "\n".join(f"  - {'/'.join(str(p) for p in e.path)}: {e.message}" for e in errors)
        fail(f"{label}: failed job-graph.schema.json validation:\n{details}")


def normalize_job(job: dict[str, Any]) -> dict[str, Any]:
    """Fills in schema defaults for optional keys the Rust SDK omits when
    empty, so presence-vs-absence of an empty collection isn't reported as
    a mismatch (see module docstring).
    """
    normalized = dict(_JOB_SPEC_DEFAULTS)
    normalized.update(job)
    return normalized


def normalize_graph(graph: dict[str, Any]) -> dict[str, Any]:
    jobs = sorted((normalize_job(j) for j in graph["jobs"]), key=lambda j: j["id"])
    return {"schema_version": graph["schema_version"], "jobs": jobs}


def diff_summary(rust_graph: dict[str, Any], python_graph: dict[str, Any]) -> str:
    rust_pretty = json.dumps(rust_graph, indent=2, sort_keys=True)
    python_pretty = json.dumps(python_graph, indent=2, sort_keys=True)
    return f"--- rust (normalized) ---\n{rust_pretty}\n--- python (normalized) ---\n{python_pretty}"


def cmd_validate(argv: list[str]) -> None:
    if len(argv) != 1:
        fail("usage: compare.py validate <json-file>")
    path = Path(argv[0])
    graph = load_json(path, str(path))
    schema_validate(graph, str(path))


def cmd_compare(argv: list[str]) -> None:
    if len(argv) != 3:
        fail("usage: compare.py compare <fixture-name> <rust-json-file> <python-json-file>")
    fixture_name, rust_path, python_path = argv
    rust_graph = load_json(Path(rust_path), f"{fixture_name} (rust)")
    python_graph = load_json(Path(python_path), f"{fixture_name} (python)")

    rust_norm = normalize_graph(rust_graph)
    python_norm = normalize_graph(python_graph)

    if rust_norm != python_norm:
        summary = diff_summary(rust_norm, python_norm)
        fail(f"fixture '{fixture_name}': rust and python JobGraph outputs differ\n{summary}")

    print(f"gitquarry-ci-sdk-contract-tests: fixture '{fixture_name}' OK -- rust and python agree")


def main() -> None:
    if len(sys.argv) < 2:
        fail("usage: compare.py <validate|compare> ...")
    mode, rest = sys.argv[1], sys.argv[2:]
    if mode == "validate":
        cmd_validate(rest)
    elif mode == "compare":
        cmd_compare(rest)
    else:
        fail(f"unknown mode {mode!r}, expected 'validate' or 'compare'")


if __name__ == "__main__":
    main()
