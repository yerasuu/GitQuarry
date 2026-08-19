from gitquarry_ci import JobKind, job
from gitquarry_ci.ansible import ansible_playbook


def test_ansible_playbook_produces_a_normal_job_with_vault_secret_referenced_not_valued() -> None:
    j = (
        ansible_playbook(job("deploy").kind(JobKind.DEPLOY), "site.yml")
        .inventory("inventory/prod")
        .vault_password_secret("ansible-vault-pass")
        .finish()
    )

    assert j._spec.kind == JobKind.DEPLOY
    assert len(j._spec.secrets) == 1
    assert j._spec.secrets[0].secret_name == "ansible-vault-pass"
    assert j._spec.secrets[0].env_key == "GITQUARRY_VAULT_PASS"

    command = j._spec.commands[-1]
    assert "ansible-playbook" in command
    assert "--vault-password-file $VAULT_PASS_FIFO" in command
    assert "site.yml" in command
    # The secret's name must never appear as a value anywhere in the command
    # -- it's only reachable structurally via secrets[0].secret_name above.
    assert "ansible-vault-pass" not in command


def test_extra_vars_and_tags_are_shell_quoted() -> None:
    j = (
        ansible_playbook(job("deploy"), "site.yml")
        .extra_var("message", "has space")
        .tags("a")
        .tags("b")
        .finish()
    )
    command = j._spec.commands[-1]
    assert "--extra-vars" in command
    assert "'message=has space'" in command
    assert "--tags" in command
    assert "a,b" in command


def test_no_vault_secret_means_no_secret_and_no_vault_flag() -> None:
    j = ansible_playbook(job("deploy"), "site.yml").finish()
    assert j._spec.secrets == []
    assert "--vault-password-file" not in j._spec.commands[-1]
