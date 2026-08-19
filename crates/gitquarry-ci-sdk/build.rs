//! Generates `wire_generated.rs` from `gitquarry-wire/schema/pipeline/v1/`
//! via `typify` on every build of this crate — never edit that file by
//! hand, edit the JSON schema instead (see `docs/gitquarry-ci-sdk-design.md`).
//!
//! Typify resolves `$ref` only within a single schema document's own
//! `definitions` map, so the 3 files that reference each other
//! (`job-graph` -> `job-spec` -> `job-kind`/`secret-ref`) are bundled into
//! one in-memory root document here, rewriting each file-based `$ref` into
//! an in-document `#/definitions/<Name>` pointer before handing it to
//! typify. `pipeline-trigger.schema.json` has no refs, so it's added as
//! its own separate root schema.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn schema_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../gitquarry-wire/schema/pipeline/v1")
}

fn read_schema(dir: &Path, name: &str) -> Value {
    let path = dir.join(format!("{name}.schema.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Rewrites every `"$ref": "<file>.schema.json"` in `value` to
/// `"$ref": "#/definitions/<Name>"` per `file_to_def`, recursively.
fn rewrite_refs(value: &mut Value, file_to_def: &HashMap<&str, &str>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref")
                && let Some(def_name) = file_to_def.get(reference.as_str())
            {
                *reference = format!("#/definitions/{def_name}");
            }
            for v in map.values_mut() {
                rewrite_refs(v, file_to_def);
            }
        }
        Value::Array(items) => {
            for v in items {
                rewrite_refs(v, file_to_def);
            }
        }
        _ => {}
    }
}

fn main() {
    let dir = schema_dir();
    println!("cargo:rerun-if-changed={}", dir.display());

    let file_to_def: HashMap<&str, &str> = HashMap::from([
        ("job-kind.schema.json", "JobKind"),
        ("secret-ref.schema.json", "SecretRef"),
        ("job-spec.schema.json", "JobSpec"),
    ]);

    let mut job_kind = read_schema(&dir, "job-kind");
    let mut secret_ref = read_schema(&dir, "secret-ref");
    let mut job_spec = read_schema(&dir, "job-spec");
    let mut job_graph = read_schema(&dir, "job-graph");
    let pipeline_trigger = read_schema(&dir, "pipeline-trigger");

    for schema in [&mut job_kind, &mut secret_ref, &mut job_spec, &mut job_graph] {
        rewrite_refs(schema, &file_to_def);
    }

    job_graph["definitions"] = Value::Object(
        [
            ("JobSpec".to_string(), job_spec),
            ("JobKind".to_string(), job_kind),
            ("SecretRef".to_string(), secret_ref),
        ]
        .into_iter()
        .collect(),
    );

    let mut settings = typify::TypeSpaceSettings::default();
    settings.with_struct_builder(false);
    let mut type_space = typify::TypeSpace::new(&settings);

    for schema in [job_graph, pipeline_trigger] {
        let root: schemars::schema::RootSchema =
            serde_json::from_value(schema).expect("bundled schema matches schemars::schema::RootSchema shape");
        type_space
            .add_root_schema(root)
            .expect("typify can convert the pipeline v1 schema to Rust types");
    }

    let contents = type_space.to_stream().to_string();
    let out_path = Path::new(&env::var("OUT_DIR").unwrap()).join("wire_generated.rs");
    fs::write(&out_path, contents).expect("write wire_generated.rs");
}
