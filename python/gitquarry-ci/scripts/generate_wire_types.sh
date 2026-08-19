#!/usr/bin/env bash
# Regenerates gitquarry_ci/wire_generated.py from
# crates/gitquarry-wire/schema/pipeline/v1/ via datamodel-code-generator.
#
# The 5 schema files cross-reference each other with file-based "$ref"s
# (job-graph -> job-spec -> job-kind/secret-ref), so datamodel-codegen is
# pointed at the whole directory rather than a single file, which makes it
# resolve those refs across files and emit one Python module per schema file
# under an output directory. This script then flattens that output into the
# single `wire_generated.py` file the rest of this package imports from, and
# hand-fixes the two things datamodel-codegen doesn't get exactly right for
# our shape:
#   - JobKind as `class JobKind(str, Enum)` (codegen emits `StrEnum`, which is
#     equivalent on 3.11+ but not what the rest of the codebase spells).
#   - list/dict/enum fields with a JSON-schema default use that default
#     directly (`field(default_factory=list)`, `JobKind.build`), not
#     `T | None = <default>` (codegen defaults to Optional for any field
#     with a default, but none of these fields are actually nullable).
#
# A real CI job would run this, diff against the committed file, and fail if
# they differ (catches schema drift) -- that check lives elsewhere, this
# script just does the regeneration.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
package_dir="$(cd "${script_dir}/.." && pwd)"
repo_root="$(cd "${package_dir}/../.." && pwd)"
schema_dir="${repo_root}/crates/gitquarry-wire/schema/pipeline/v1"
# datamodel-codegen insists the output path not already exist when emitting
# multi-file (modular-reference) output, so reserve a name under a fresh
# temp dir rather than mktemp -d'ing the target itself.
out_dir="$(mktemp -d)/wire_generated"
trap 'rm -rf "${out_dir%/*}"' EXIT

datamodel-codegen \
    --input "${schema_dir}" \
    --input-file-type jsonschema \
    --output "${out_dir}" \
    --target-python-version 3.11 \
    --use-standard-collections \
    --use-schema-description \
    --output-model-type dataclasses.dataclass

echo "Generated modules in ${out_dir}:"
ls "${out_dir}"
echo
echo "Now hand-merge these into ${package_dir}/gitquarry_ci/wire_generated.py:"
echo "  - flatten the per-schema-file modules into one file"
echo "  - change JobKind from StrEnum to 'class JobKind(str, Enum)'"
echo "  - replace 'T | None = <default>' with the non-Optional default"
echo "    (field(default_factory=list/dict) or JobKind.build) for needs/"
echo "    env/secrets/artifacts/kind"
