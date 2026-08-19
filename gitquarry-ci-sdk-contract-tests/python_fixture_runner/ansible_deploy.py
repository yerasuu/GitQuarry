"""Fixture: single `deploy`-kind job built through the `ansible_playbook`
builder. Python-side twin of `rust_fixture_runner/src/bin/ansible_deploy.rs`.
"""

from gitquarry_ci import JobKind, graph, job
from gitquarry_ci.ansible import ansible_playbook


def main() -> None:
    g = graph().add_job(
        ansible_playbook(job("deploy").image("gitquarry-ci-ansible:latest").kind(JobKind.DEPLOY), "site.yml")
        .inventory("inventory/prod")
        .vault_password_secret("ansible-vault-pass")
        .finish()
    )
    g.dump()


if __name__ == "__main__":
    main()
