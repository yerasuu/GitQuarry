import pytest

from gitquarry_ci import context


def _set_all_env(monkeypatch, **overrides) -> None:
    values = {
        "GITQUARRY_CI_SHA": "deadbeef",
        "GITQUARRY_CI_REFNAME": "refs/heads/main",
        "GITQUARRY_CI_PUSHER": "alice",
        "GITQUARRY_CI_REPO_UID": "repo-123",
        "GITQUARRY_CI_CHANGED_PATHS_JSON": '["crates/a/lib.rs", "docs/readme.md"]',
        "GITQUARRY_CI_IS_FORK_PR": "false",
    }
    values.update(overrides)
    for key, value in values.items():
        monkeypatch.setenv(key, value)


def test_context_reads_all_env_vars(monkeypatch) -> None:
    _set_all_env(monkeypatch)
    ctx = context()
    assert ctx.sha == "deadbeef"
    assert ctx.refname == "refs/heads/main"
    assert ctx.pusher == "alice"
    assert ctx.repo_uid == "repo-123"
    assert ctx.changed_paths == ["crates/a/lib.rs", "docs/readme.md"]
    assert ctx.is_fork_pr is False


def test_context_is_fork_pr_true(monkeypatch) -> None:
    _set_all_env(monkeypatch, GITQUARRY_CI_IS_FORK_PR="true")
    assert context().is_fork_pr is True


def test_context_missing_env_var_raises_runtime_error(monkeypatch) -> None:
    monkeypatch.delenv("GITQUARRY_CI_SHA", raising=False)
    with pytest.raises(RuntimeError, match="GITQUARRY_CI_SHA"):
        context()
