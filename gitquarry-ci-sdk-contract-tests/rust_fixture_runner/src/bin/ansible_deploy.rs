//! Fixture: single `deploy`-kind job built through the Ansible extension
//! trait. Rust-side twin of `python_fixture_runner/ansible_deploy.py`.

use gitquarry_ci_sdk::ansible::AnsibleJobExt;
use gitquarry_ci_sdk::{JobKind, graph, job};

fn main() {
    let g = graph().add_job(
        job("deploy")
            .image("gitquarry-ci-ansible:latest")
            .kind(JobKind::Deploy)
            .ansible_playbook("site.yml")
            .inventory("inventory/prod")
            .vault_password_secret("ansible-vault-pass")
            .finish(),
    );
    g.dump();
}
