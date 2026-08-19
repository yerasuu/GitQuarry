"""Builds a single `JobSpec`.

Mutable, chained (methods mutate self and return self) -- the process that
uses this SDK runs, dumps its JobGraph to stdout, and dies in seconds, so
immutability would add ceremony without real benefit here.
"""

from __future__ import annotations

import sys

from gitquarry_ci.wire_generated import JobKind, JobSpec, SecretRef


class JobBuilder:
    """Builder for one `JobSpec`. Must be passed to `GraphBuilder.add_job` to
    take effect -- this is the closest runtime approximation Python has to
    Rust's `#[must_use]` compile-time check on the same type.
    """

    def __init__(self, id: str) -> None:
        self._spec = JobSpec(id=id, image="", commands=[])
        self._consumed = False

    def image(self, image: str) -> JobBuilder:
        self._spec.image = image
        return self

    def run(self, command: str) -> JobBuilder:
        self._spec.commands.append(command)
        return self

    def needs(self, id: str) -> JobBuilder:
        self._spec.needs.append(id)
        return self

    def env(self, key: str, value: str) -> JobBuilder:
        self._spec.env[key] = value
        return self

    def secret(self, env_key: str, secret_name: str) -> JobBuilder:
        """The only entry point for secret data anywhere in this SDK -- a
        reference by name, never a value. There is no method that accepts a
        secret's actual value; the value never passes through this SDK.
        """
        self._spec.secrets.append(SecretRef(env_key=env_key, secret_name=secret_name))
        return self

    def artifact(self, path: str) -> JobBuilder:
        self._spec.artifacts.append(path)
        return self

    def kind(self, kind: JobKind) -> JobBuilder:
        self._spec.kind = kind
        return self

    def _mark_consumed(self) -> JobSpec:
        """Called by `GraphBuilder.add_job`. Returns the underlying spec and
        marks this builder as consumed so `__del__` won't warn about it.
        """
        self._consumed = True
        return self._spec

    def __del__(self) -> None:
        if not getattr(self, "_consumed", True):
            print(
                f"gitquarry-ci-sdk: warning: JobBuilder({self._spec.id!r}) was never "
                "passed to GraphBuilder.add_job() -- it has no effect on the dumped graph",
                file=sys.stderr,
            )


def job(id: str) -> JobBuilder:
    return JobBuilder(id)
