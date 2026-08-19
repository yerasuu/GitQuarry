use crate::ansible::{AnsibleJobExt, shell_escape};
use crate::{JobKind, graph, job};

#[test]
fn builds_a_simple_graph() {
    let g = graph()
        .add_job(job("test-a").image("rust:1.80").run("cargo test -p a"))
        .add_job(
            job("build-a")
                .image("rust:1.80")
                .run("cargo build -p a")
                .needs("test-a"),
        );

    assert!(g.validate().is_ok());
}

#[test]
fn for_each_dir_runs_once_per_distinct_top_level_dir() {
    let paths = vec![
        "crates/a/src/lib.rs".to_string(),
        "crates/a/src/other.rs".to_string(),
        "crates/b/src/lib.rs".to_string(),
        "docs/readme.md".to_string(),
    ];
    let mut seen = Vec::new();
    let _ = graph().for_each_dir(&paths, |dir| {
        seen.push(dir.to_string());
        job(format!("test-{dir}")).image("rust:1.80").run("cargo test")
    });
    assert_eq!(seen, vec!["crates", "docs"]);
}

#[test]
fn rejects_a_graph_with_a_cycle() {
    let g = graph()
        .add_job(job("a").image("img").run("x").needs("b"))
        .add_job(job("b").image("img").run("x").needs("a"));

    assert!(g.validate().is_err());
}

#[test]
fn secret_never_carries_a_value_only_a_reference() {
    let j = job("deploy")
        .image("img")
        .run("x")
        .secret("API_TOKEN", "prod-token")
        .into_spec();
    assert_eq!(j.secrets.len(), 1);
    assert_eq!(j.secrets[0].env_key, "API_TOKEN");
    assert_eq!(j.secrets[0].secret_name, "prod-token");
}

#[test]
fn kind_defaults_to_build() {
    let j = job("a").image("img").run("x").into_spec();
    assert_eq!(j.kind, JobKind::Build);
}

#[test]
fn ansible_playbook_produces_a_normal_job_with_vault_secret_referenced_not_valued() {
    let j = job("deploy")
        .kind(JobKind::Deploy)
        .ansible_playbook("site.yml")
        .inventory("inventory/prod")
        .vault_password_secret("ansible-vault-pass")
        .finish()
        .into_spec();

    assert_eq!(j.kind, JobKind::Deploy);
    assert_eq!(j.secrets.len(), 1);
    assert_eq!(j.secrets[0].secret_name, "ansible-vault-pass");
    let command = j.commands.last().unwrap();
    assert!(command.contains("ansible-playbook"));
    assert!(command.contains("--vault-password-file $VAULT_PASS_FIFO"));
    assert!(command.contains("site.yml"));
    assert!(!command.contains("ansible-vault-pass"));
}

#[test]
fn shell_escape_wraps_values_with_shell_metacharacters() {
    assert_eq!(shell_escape("simple-value_123"), "simple-value_123");
    assert_eq!(shell_escape("has space"), "'has space'");
    assert_eq!(shell_escape("it's"), r"'it'\''s'");
}
