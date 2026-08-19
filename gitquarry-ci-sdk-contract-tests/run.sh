#!/usr/bin/env bash
# Contract test: proves gitquarry-ci-sdk (Rust) and gitquarry_ci (Python)
# produce equivalent JobGraph JSON for the same logical pipeline, across 3
# fixtures (simple, monorepo-dynamic, ansible-deploy). See
# docs/gitquarry-ci-sdk-design.md, Fase 5.
#
# Real JSON-Schema validation (via the `jsonschema` package) is used when
# the invoking Python has it importable; otherwise compare.py falls back to
# a hand-rolled structural check and says so on stderr. Point PYTHON_BIN at
# a venv with `pip install jsonschema` to get the real validation path.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXTURES_DIR="$SCRIPT_DIR/fixtures"
PYTHON_RUNNER_DIR="$SCRIPT_DIR/python_fixture_runner"
COMPARE_PY="$SCRIPT_DIR/compare.py"
PYTHON_BIN="${PYTHON_BIN:-python3}"
CARGO_PKG="gitquarry-ci-sdk-contract-fixtures"

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

# Same fixed PipelineContext env vars fed to both processes for a given
# fixture -- monorepo-dynamic is the only fixture that actually reads them
# (via context()), but all 3 get them for uniformity.
export GITQUARRY_CI_SHA=deadbeef
export GITQUARRY_CI_REFNAME=refs/heads/main
export GITQUARRY_CI_PUSHER=alice
export GITQUARRY_CI_REPO_UID=repo-1
export GITQUARRY_CI_IS_FORK_PR=false
export GITQUARRY_CI_CHANGED_PATHS_JSON='["crates/foo/src/lib.rs","crates/bar/src/lib.rs","docs/readme.md"]'
export PYTHONPATH="$REPO_ROOT/python/gitquarry-ci"

# bin/script names use underscores (valid Rust/Python identifiers); the
# checked-in fixture files use hyphens per the design doc's directory
# layout. Same order as the design doc's fixture list.
FIXTURES=(simple monorepo_dynamic ansible_deploy)
fixture_file_name() {
    case "$1" in
        simple) echo "simple" ;;
        monorepo_dynamic) echo "monorepo-dynamic" ;;
        ansible_deploy) echo "ansible-deploy" ;;
    esac
}

echo "Building rust fixture runner ($CARGO_PKG)..."
(cd "$REPO_ROOT" && cargo build -q -p "$CARGO_PKG")

overall_status=0

for fixture in "${FIXTURES[@]}"; do
    file_name="$(fixture_file_name "$fixture")"
    echo
    echo "=== fixture: $file_name ==="

    rust_out="$WORK_DIR/$fixture.rust.json"
    python_out="$WORK_DIR/$fixture.python.json"
    fixture_failed=0

    if (cd "$REPO_ROOT" && cargo run -q -p "$CARGO_PKG" --bin "$fixture" -- --dump-graph) > "$rust_out"; then
        echo "rust:   dump() exited 0"
    else
        echo "FAIL: rust fixture '$file_name' exited non-zero (see stderr above)" >&2
        overall_status=1
        continue
    fi

    if "$PYTHON_BIN" "$PYTHON_RUNNER_DIR/$fixture.py" --dump-graph > "$python_out"; then
        echo "python: dump() exited 0"
    else
        echo "FAIL: python fixture '$file_name' exited non-zero (see stderr above)" >&2
        overall_status=1
        continue
    fi

    "$PYTHON_BIN" "$COMPARE_PY" validate "$rust_out" || fixture_failed=1
    "$PYTHON_BIN" "$COMPARE_PY" validate "$python_out" || fixture_failed=1
    "$PYTHON_BIN" "$COMPARE_PY" compare "$file_name" "$rust_out" "$python_out" || fixture_failed=1

    if [[ "$fixture_failed" -ne 0 ]]; then
        overall_status=1
        continue
    fi

    # Rust and Python outputs are proven equal above, so either is an
    # equally valid source for the checked-in reference fixture -- Rust is
    # the canonical pick (see docs/gitquarry-ci-sdk-design.md).
    "$PYTHON_BIN" -c "
import json, sys
with open(sys.argv[1]) as f:
    graph = json.load(f)
with open(sys.argv[2], 'w') as f:
    json.dump(graph, f, indent=2, sort_keys=True)
    f.write('\n')
" "$rust_out" "$FIXTURES_DIR/$file_name.expected.json"
    echo "wrote $FIXTURES_DIR/$file_name.expected.json"
done

echo
if [[ "$overall_status" -eq 0 ]]; then
    echo "gitquarry-ci-sdk-contract-tests: all fixtures passed"
else
    echo "gitquarry-ci-sdk-contract-tests: one or more fixtures FAILED" >&2
fi
exit "$overall_status"
