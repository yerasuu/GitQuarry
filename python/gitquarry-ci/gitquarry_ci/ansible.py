"""`AnsibleJobBuilder` -- Python has no extension traits, so this free
function + builder class is the idiomatic equivalent of the Rust SDK's
`AnsibleJobExt` trait on `JobBuilder`.

Ansible is not a third pipeline language or a new execution mode -- this
builder just ends up producing a normal `JobBuilder` via `.finish()`, same
contract, same executor.
"""

from __future__ import annotations

import shlex

from gitquarry_ci.job import JobBuilder

# The vault password's env key -- injected via `.secret()`, never as a plain
# env var value or CLI arg text (`ansible-vault` expects a file).
_VAULT_PASS_ENV = "GITQUARRY_VAULT_PASS"
_VAULT_PASS_FIFO = "$VAULT_PASS_FIFO"


class AnsibleJobBuilder:
    def __init__(self, job_builder: JobBuilder, playbook: str) -> None:
        self._job = job_builder
        self._playbook = playbook
        self._inventory: str | None = None
        self._extra_vars: list[tuple[str, str]] = []
        self._vault_password_secret: str | None = None
        self._tags: list[str] = []

    def inventory(self, path: str) -> AnsibleJobBuilder:
        self._inventory = path
        return self

    def extra_var(self, key: str, value: str) -> AnsibleJobBuilder:
        self._extra_vars.append((key, value))
        return self

    def vault_password_secret(self, secret_name: str) -> AnsibleJobBuilder:
        self._vault_password_secret = secret_name
        return self

    def tags(self, tags: str) -> AnsibleJobBuilder:
        self._tags.append(tags)
        return self

    def finish(self) -> JobBuilder:
        parts = ["ansible-playbook"]

        if self._inventory is not None:
            parts.append("--inventory")
            parts.append(shlex.quote(self._inventory))
        for key, value in self._extra_vars:
            parts.append("--extra-vars")
            parts.append(shlex.quote(f"{key}={value}"))
        if self._tags:
            parts.append("--tags")
            parts.append(shlex.quote(",".join(self._tags)))
        if self._vault_password_secret is not None:
            self._job.secret(_VAULT_PASS_ENV, self._vault_password_secret)
            parts.append("--vault-password-file")
            parts.append(_VAULT_PASS_FIFO)
        parts.append(shlex.quote(self._playbook))

        command = " ".join(parts)
        return self._job.run(command)


def ansible_playbook(job_builder: JobBuilder, playbook: str) -> AnsibleJobBuilder:
    return AnsibleJobBuilder(job_builder, playbook)
