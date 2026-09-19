"""Engine discovery tests (D-T035-02): fixture registry trees only, never
the real ``~/.local/share/zed`` or the real ``~/.codex``/``~/.claude``."""
from __future__ import annotations

import json

from cortex_runtime.connections import discovery, onboarding, registry
from cortex_runtime.connections.model import EngineNativeDir


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return home


def _write_package(entry_dir, package_name, *, version="1.12.0", bin_name="index.js", bin_key="codex-acp"):
    package_dir = entry_dir / "node_modules" / package_name
    package_dir.mkdir(parents=True)
    (package_dir / "dist").mkdir()
    (package_dir / "dist" / bin_name).write_text("#!/usr/bin/env node\n", encoding="utf-8")
    (package_dir / "package.json").write_text(
        json.dumps({"name": package_name, "version": version, "bin": {bin_key: f"dist/{bin_name}"}}),
        encoding="utf-8",
    )
    return package_dir


def _registry_root(tmp_path):
    root = tmp_path / "zed" / "external_agents" / "registry" / "npx"
    root.mkdir(parents=True)
    return root


def test_discovers_both_engines_and_reports_real_bin(tmp_path):
    registry_root = _registry_root(tmp_path)
    codex_dir = registry_root / "codex-acp"
    codex_dir.mkdir()
    _write_package(codex_dir, "@agentclientprotocol/codex-acp", version="1.12.0", bin_key="codex-acp")

    claude_dir = registry_root / "claude-acp"
    claude_dir.mkdir()
    _write_package(
        claude_dir, "@agentclientprotocol/claude-agent-acp", version="0.78.0",
        bin_key="claude-agent-acp",
    )

    home = tmp_path / "home"
    home.mkdir()
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node", environ={"HOME": str(home)})

    assert set(found) == {"codex-acp", "claude-agent-acp"}
    codex = found["codex-acp"]
    assert codex.version == "1.12.0"
    assert codex.launch_command[0] == "/usr/bin/node"
    assert codex.launch_command[1] == str(codex_dir / "node_modules" / "@agentclientprotocol/codex-acp" / "dist" / "index.js")
    assert codex.native_credential_dir == str(home / ".codex")

    claude = found["claude-agent-acp"]
    assert claude.version == "0.78.0"
    assert claude.native_credential_dir == str(home / ".claude")


def test_missing_engine_yields_nothing_not_a_guessed_path(tmp_path):
    registry_root = _registry_root(tmp_path)
    # Only codex-acp is installed; claude-agent-acp must never be reported.
    codex_dir = registry_root / "codex-acp"
    codex_dir.mkdir()
    _write_package(codex_dir, "@agentclientprotocol/codex-acp")

    home = tmp_path / "home"
    home.mkdir()
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node", environ={"HOME": str(home)})
    assert set(found) == {"codex-acp"}
    assert discovery.discover_engine("claude-agent-acp", registry_root=registry_root, node_binary="/usr/bin/node") is None


def test_declared_bin_file_missing_on_disk_is_never_guessed(tmp_path):
    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    package_dir = entry / "node_modules" / "@agentclientprotocol/codex-acp"
    package_dir.mkdir(parents=True)
    (package_dir / "package.json").write_text(
        json.dumps({"name": "@agentclientprotocol/codex-acp", "version": "1.12.0", "bin": {"codex-acp": "dist/index.js"}}),
        encoding="utf-8",
    )
    # dist/index.js is never written -- the declared bin target does not exist.
    result = discovery.discover_engine("codex-acp", registry_root=registry_root, node_binary="/usr/bin/node")
    assert result is None


def test_no_node_binary_yields_nothing(tmp_path):
    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    _write_package(entry, "@agentclientprotocol/codex-acp")
    found = discovery.discover_all(registry_root=registry_root, node_binary=None, environ={"HOME": str(tmp_path), "PATH": ""})
    assert found == {}


def test_broken_unreadable_registry_tree_never_raises(tmp_path):
    missing_root = tmp_path / "does" / "not" / "exist"
    assert discovery.discover_all(registry_root=missing_root, node_binary="/usr/bin/node") == {}

    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    package_dir = entry / "node_modules" / "@agentclientprotocol/codex-acp"
    package_dir.mkdir(parents=True)
    (package_dir / "package.json").write_text("{not valid json", encoding="utf-8")
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node")
    assert found == {}


def test_created_connection_references_users_own_directory_not_owned(tmp_path, monkeypatch):
    home = _isolate(tmp_path, monkeypatch)
    record = onboarding.create_referenced_connection(
        name="codex-acp (discovered)", engine="codex-acp", provider="openai",
        launch_command=("/usr/bin/node", "/somewhere/dist/index.js"),
    )
    assert isinstance(record.credential_ref, EngineNativeDir)
    assert record.credential_ref.owned is False
    assert record.credential_ref.path == str(home / ".codex")
    # Never a Cortex-managed copy under state_root().
    assert not record.credential_ref.path.startswith(str(registry.state_root()))
    assert not (home / ".codex").exists()  # never created by Cortex


def test_startup_discovery_sync_is_idempotent_and_never_mutates_existing(tmp_path, monkeypatch):
    home = _isolate(tmp_path, monkeypatch)
    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    _write_package(entry, "@agentclientprotocol/codex-acp")
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node", environ={"HOME": str(home)})

    created = onboarding.sync_discovered_connections(found)
    assert len(created) == 1
    first_id = created[0].id
    first_updated_at = created[0].updated_at

    # Running again must not create a second connection or touch the first.
    created_again = onboarding.sync_discovered_connections(found)
    assert created_again == []
    state = registry.read_registry()
    assert len(state.connections) == 1
    assert state.connections[first_id].updated_at == first_updated_at


def test_a_connection_holding_its_own_credential_copy_is_repointed_at_the_clients(tmp_path, monkeypatch):
    """D-T035-02 changed where an engine and its credentials come from, and
    connections made before it kept their own copy of both.

    That copy expires on its own schedule while the client's install stays
    signed in: live, every new session was refused with "OAuth session
    expired and could not be refreshed" on a machine where the same
    editor's own agent was working fine. Re-authenticating a second copy is
    not the answer -- the engine the user already authorised is. The
    connection keeps its id, so tasks and route history that name it keep
    working.
    """
    from cortex_runtime.connections.model import AuthMode, EngineNativeDir

    home = _isolate(tmp_path, monkeypatch)
    existing = onboarding.create_connection(
        name="my own", engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT,
    )
    assert existing.credential_ref.owned is True
    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    _write_package(entry, "@agentclientprotocol/codex-acp")
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node", environ={"HOME": str(home)})

    created = onboarding.sync_discovered_connections(found)
    assert created == [], "the connection is repointed, never duplicated"
    state = registry.read_registry()
    assert len(state.connections) == 1
    upgraded = state.connections[existing.id]
    assert upgraded.credential_ref == EngineNativeDir(path=str(home / ".codex"), owned=False)
    assert upgraded.launch_command == tuple(found["codex-acp"].launch_command)
    assert upgraded.name == existing.name  # its identity is not rewritten


def test_a_connection_with_its_own_secret_is_left_exactly_as_the_user_made_it(tmp_path, monkeypatch):
    """The migration above is only ever about a Cortex-owned copy of an
    engine's *native* login. A connection the user pointed at their own
    secret file is a deliberate configuration and is never rewritten."""
    from cortex_runtime.connections.model import AuthMode

    home = _isolate(tmp_path, monkeypatch)
    existing = onboarding.create_connection(
        name="my own key", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", credential_kind="secret_file",
    )
    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    _write_package(entry, "@agentclientprotocol/codex-acp")
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node", environ={"HOME": str(home)})

    onboarding.sync_discovered_connections(found)
    state = registry.read_registry()
    assert state.connections[existing.id].credential_ref == existing.credential_ref
    assert state.connections[existing.id].updated_at == existing.updated_at


def test_engine_client_lacks_never_appears_available(tmp_path):
    registry_root = _registry_root(tmp_path)
    entry = registry_root / "claude-acp"
    entry.mkdir()
    _write_package(
        entry, "@agentclientprotocol/claude-agent-acp", version="0.78.0", bin_key="claude-agent-acp",
    )
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node")
    assert "codex-acp" not in found


def test_a_discovered_engine_actually_resolves_for_launch(tmp_path):
    """The gap the discovery tests did not cover, and the reason this
    feature could not have worked: a discovered connection is only useful
    if `install_resolution.resolve_verified_install` accepts it, and that
    is the check every engine open runs.

    Two separate barriers refused it, both real, neither exercised by a
    discovery test:

    * the client's agent registry was on the untrusted-location list, so
      the location alone was a refusal (D-T035-02 reverses that: the
      engines Cortex runs are the ones the user configured in the client,
      which is where the client installs them);
    * dependencies were looked up only *nested* under the adapter package,
      while npm hoists shared dependencies to the top of the tree -- which
      is exactly the shape a client install has.

    This builds that shape and resolves it.
    """
    from types import SimpleNamespace

    from cortex_runtime.engine import install_resolution

    home = tmp_path / "home"
    modules = home / ".local/share/zed/external_agents/registry/npx/codex-acp/node_modules"
    adapter = modules / "@agentclientprotocol/codex-acp"
    (adapter / "dist").mkdir(parents=True)
    (adapter / "dist/index.js").write_text("// adapter\n", encoding="utf-8")
    (adapter / "package.json").write_text(
        json.dumps({"name": "@agentclientprotocol/codex-acp", "version": "1.12.0",
                    "bin": {"codex-acp": "dist/index.js"}}), encoding="utf-8")
    # Hoisted beside the adapter, not under it -- an ordinary npm layout.
    codex = modules / "@openai/codex"
    codex.mkdir(parents=True)
    (codex / "package.json").write_text(
        json.dumps({"name": "@openai/codex", "version": "0.154.0"}), encoding="utf-8")
    triple = "x86_64-unknown-linux-musl"
    platform_package = modules / "@openai/codex-linux-x64"
    binary = platform_package / "vendor" / triple / "bin"
    binary.mkdir(parents=True)
    (binary / "codex").write_text("#!/bin/sh\n", encoding="utf-8")
    (binary / "codex").chmod(0o755)
    (platform_package / "package.json").write_text(
        json.dumps({"name": "@openai/codex-linux-x64", "version": "0.154.0-linux-x64"}), encoding="utf-8")
    node = home / "node"
    node.write_text("#!/bin/sh\n", encoding="utf-8")
    node.chmod(0o755)

    record = SimpleNamespace(
        engine="codex-acp",
        launch_command=(str(node), str(adapter / "dist/index.js")),
    )
    resolved = install_resolution.resolve_verified_install(record, {"HOME": str(home)})
    assert resolved.package_version == "1.12.0"


def test_the_node_binary_is_reported_canonical_not_as_the_symlink_on_path(tmp_path, monkeypatch):
    """The launch trust check accepts only an already-canonical absolute
    path, and the node on a user's PATH is very often a symlink
    (`~/.local/bin/node` -> a version manager's real binary). Reporting the
    symlink produced a connection that could never launch at all: every
    resolve refused with `engine_install_untrusted_location`, naming the
    node binary, on a machine where node worked perfectly."""
    home = _isolate(tmp_path, monkeypatch)
    real = tmp_path / "real-node"
    real.write_text("#!/bin/sh\n", encoding="utf-8")
    real.chmod(0o755)
    bin_dir = home / ".local" / "bin"
    bin_dir.mkdir(parents=True)
    link = bin_dir / "node"
    link.symlink_to(real)

    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    _write_package(entry, "@agentclientprotocol/codex-acp")
    found = discovery.discover_all(
        registry_root=registry_root, environ={"HOME": str(home), "PATH": str(bin_dir)},
    )

    assert found["codex-acp"].launch_command[0] == str(real.resolve())


def test_a_referencing_connection_follows_the_clients_current_install(tmp_path, monkeypatch):
    """An editor that updates its engine, or a node binary that resolves
    somewhere new, otherwise leaves a connection pointing at something that
    no longer launches."""
    home = _isolate(tmp_path, monkeypatch)
    registry_root = _registry_root(tmp_path)
    entry = registry_root / "codex-acp"
    entry.mkdir()
    _write_package(entry, "@agentclientprotocol/codex-acp")
    found = discovery.discover_all(registry_root=registry_root, node_binary="/usr/bin/node", environ={"HOME": str(home)})
    created = onboarding.sync_discovered_connections(found)
    assert len(created) == 1
    connection_id = created[0].id

    # The client's install moves.
    moved = registry_root / "codex-acp-2"
    moved.mkdir()
    _write_package(moved, "@agentclientprotocol/codex-acp")
    import dataclasses

    found_again = dict(found)
    found_again["codex-acp"] = dataclasses.replace(
        found_again["codex-acp"],
        launch_command=(
            "/usr/bin/node",
            str(moved / "node_modules" / "@agentclientprotocol" / "codex-acp" / "dist" / "index.js"),
        ),
    )
    onboarding.sync_discovered_connections(found_again)

    state = registry.read_registry()
    assert state.connections[connection_id].launch_command == found_again["codex-acp"].launch_command
    assert len(state.connections) == 1, "kept in step, never duplicated"
