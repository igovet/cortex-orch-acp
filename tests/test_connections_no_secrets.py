"""No secret bytes leak into the registry, project .cortex, or source text.

Combines a dynamic check (walk every byte written by a full onboarding flow
with a fake secret) with a static source scan (connections/*.py never
contains a literal secret-looking fixture or a print/logging call that
could carry key material), mirroring tests/test_portable_no_native_tokens.py
and tests/test_no_secret_in_schemas.py.
"""
from __future__ import annotations

import os
from pathlib import Path
import re

import pytest

from cortex_runtime.connections import environment, onboarding, registry
from cortex_runtime.connections.model import AuthMode

CONNECTIONS_DIR = Path(__file__).resolve().parents[1] / "plugins/cortex/scripts/cortex_runtime/connections"

FAKE_SECRET = "fake-super-secret-value-QWERTY123456"


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return config_home, state_home, home


def _walk_bytes(root: Path):
    for parent, _dirs, files in os.walk(root):
        for name in files:
            path = Path(parent) / name
            if path.is_symlink():
                continue
            yield path, path.read_bytes()


def test_registry_and_state_tree_never_contain_the_secret_bytes(tmp_path, monkeypatch, capsys):
    config_home, state_home, _home = _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(
        name="work", engine="claude-agent-acp", provider="anthropic",
        auth_mode=AuthMode.API_KEY, key_kind="anthropic_api_key",
    )
    onboarding.set_api_key(record.id, lambda: FAKE_SECRET.encode())
    env = environment.build_process_env(record, base_env={})
    assert env["ANTHROPIC_API_KEY"] == FAKE_SECRET

    secret_path = Path(record.credential_ref.path)
    for path, data in _walk_bytes(config_home):
        if path == secret_path:
            assert FAKE_SECRET.encode() in data
            continue
        assert FAKE_SECRET.encode() not in data, f"secret leaked into {path}"
    for path, data in _walk_bytes(state_home):
        assert FAKE_SECRET.encode() not in data, f"secret leaked into {path}"

    registry_bytes = registry.registry_path().read_bytes()
    assert FAKE_SECRET.encode() not in registry_bytes

    captured = capsys.readouterr()
    assert FAKE_SECRET not in captured.out
    assert FAKE_SECRET not in captured.err


def test_nothing_is_written_under_a_project_dot_cortex_directory(tmp_path, monkeypatch):
    project_root = tmp_path / "project"
    project_cortex = project_root / ".cortex"
    project_cortex.mkdir(parents=True)
    before = sorted(p.relative_to(project_cortex) for p in project_cortex.rglob("*"))

    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))

    record = onboarding.create_connection(
        name="work", engine="codex-acp", provider="openai",
        auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
    )
    onboarding.set_api_key(record.id, lambda: FAKE_SECRET.encode())
    environment.build_process_env(record, base_env={})
    onboarding.validate(record.id)

    after = sorted(p.relative_to(project_cortex) for p in project_cortex.rglob("*"))
    assert after == before == []


def test_registry_storage_functions_reject_a_config_root_under_dot_cortex(tmp_path, monkeypatch):
    config_home = tmp_path / "project" / ".cortex" / "config"
    config_home.mkdir(parents=True)
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    from cortex_runtime.portable.errors import PortableError
    with pytest.raises(PortableError) as excinfo:
        registry.registry_path()
    assert excinfo.value.code == "connection_storage_under_project"


def connections_source_files():
    return sorted(CONNECTIONS_DIR.glob("*.py"))


@pytest.mark.parametrize("path", connections_source_files(), ids=lambda path: path.name)
def test_connections_source_has_no_literal_secret_looking_fixture(path):
    text = path.read_text(encoding="utf-8")
    # Literal-looking secret fixtures (e.g. "sk-...", "ghp_...", long
    # hex/base64-looking assigned string constants) must never appear in the
    # implementation itself; only real callers (never this package) ever
    # hold a secret value, and even then only as opaque bytes passed in.
    suspicious = re.findall(r'"(sk-[A-Za-z0-9]{10,}|ghp_[A-Za-z0-9]{10,}|AKIA[A-Z0-9]{10,})"', text)
    assert not suspicious, f"{path.name} contains a literal secret-looking fixture: {suspicious}"


@pytest.mark.parametrize("path", connections_source_files(), ids=lambda path: path.name)
def test_connections_source_never_prints_or_logs_a_secret_value(path):
    text = path.read_text(encoding="utf-8")
    for line_number, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith(("print(", "logging.", "log.")):
            lowered = stripped.lower()
            assert "secret" not in lowered and "api_key" not in lowered and "token" not in lowered and "password" not in lowered, (
                f"{path.name}:{line_number} calls print/logging near a credential-shaped name: {stripped!r}"
            )


def test_connections_directory_is_not_empty():
    assert connections_source_files(), "expected connections/*.py files to scan"
