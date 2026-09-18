"""engine.install_resolution: deriving + verifying a pinned engine install
straight from a ConnectionRecord's own recorded launch_command (T035
follow-up: "codex per-model efforts are never enriched in practice" /
"a connection's launch command is trusted verbatim, never re-verified").

Every install used here is a synthetic, owned fixture built fresh under
``tmp_path`` (see ``engine_fakes.build_codex_npm_install``/
``build_claude_npm_install``) -- never a reference to a real installed
engine.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from cortex_runtime.engine import install_resolution
from cortex_runtime.portable.errors import PortableError

from engine_fakes import (
    build_claude_npm_install,
    build_codex_npm_install,
    default_scenario,
    isolated_environ,
    make_connection_record,
    new_connection_id,
    write_scenario,
)


def _record_with_launch_command(tmp_path, launch_command, engine, environ):
    return make_connection_record(new_connection_id(), launch_command=launch_command, engine=engine, environ=environ)


# --- codex-acp: real resolution ---------------------------------------------


def test_resolves_and_verifies_a_real_codex_install(tmp_path):
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    launch_command, bundled_binary = build_codex_npm_install(tmp_path / "install", scenario_path)
    record = _record_with_launch_command(tmp_path, launch_command, "codex-acp", environ)

    verified = install_resolution.resolve_verified_install(record, environ)

    assert verified.node_binary == launch_command[0]
    assert verified.adapter_entry == launch_command[1]
    assert verified.package_version == "1.12.0"
    assert verified.bundled_codex_version == "0.154.0-fixture"
    assert verified.bundled_codex_binary == str(bundled_binary)


def test_codex_resolution_fails_closed_on_wrong_entry_point(tmp_path):
    """The same "wrong entry point" scenario as the claude-agent-acp test
    below, for codex-acp: launch_command points at a file under the
    package that package.json's own "bin" field never names."""
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    install_root = tmp_path / "install"
    build_codex_npm_install(install_root, scenario_path)
    wrong_entry = install_root / "dist" / "other.js"
    wrong_entry.write_text("// never named by package.json's own bin field\n", encoding="utf-8")
    from engine_fakes import _resolved_node_binary

    launch_command = (_resolved_node_binary(), str(wrong_entry))
    record = _record_with_launch_command(tmp_path, launch_command, "codex-acp", environ)

    with pytest.raises(PortableError) as excinfo:
        install_resolution.resolve_verified_install(record, environ)
    assert excinfo.value.code == "engine_install_drifted"
    assert excinfo.value.field == "adapter_entry"


def test_codex_resolution_fails_closed_on_untrusted_location(tmp_path):
    """A codex install laid out under one of the untrusted Zed-registry-
    shaped roots (see codex_install's own _UNTRUSTED_HOME_RELATIVE_ROOTS)
    is refused even though every file/hash is otherwise perfectly
    consistent -- the location itself is never trusted."""
    environ = isolated_environ(tmp_path)
    home = Path(environ["HOME"])
    untrusted_root = home / ".local" / "share" / "zed" / "external_agents" / "codex-acp"
    scenario_path = write_scenario(tmp_path, default_scenario())
    launch_command, _binary = build_codex_npm_install(untrusted_root, scenario_path)
    record = _record_with_launch_command(tmp_path, launch_command, "codex-acp", environ)

    with pytest.raises(PortableError) as excinfo:
        install_resolution.resolve_verified_install(record, environ)
    assert excinfo.value.code == "engine_install_untrusted_location"


# --- claude-agent-acp: real resolution + wrong-entry-point rejection -------


def test_resolves_and_verifies_a_real_claude_install(tmp_path):
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    launch_command = build_claude_npm_install(tmp_path / "install", scenario_path)
    record = _record_with_launch_command(tmp_path, launch_command, "claude-agent-acp", environ)

    verified = install_resolution.resolve_verified_install(record, environ)

    assert verified.node_binary == launch_command[0]
    assert verified.adapter_entry == launch_command[1]
    assert verified.package_version == "0.78.0"


def test_claude_resolution_fails_closed_on_wrong_entry_point(tmp_path):
    """The exact live-qualification bug this feature closes: package.json's
    own "bin" names dist/index.js, but launch_command points at a
    different file under the same package (e.g. a library module with no
    stdin listener) -- resolve_verified_install must refuse, not silently
    launch it."""
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    install_root = tmp_path / "install"
    # package.json declares "bin": {"claude-agent-acp": "dist/index.js"}
    # (build_claude_npm_install's default), but we hand back a launch
    # command pointing at a sibling file never named by "bin".
    build_claude_npm_install(install_root, scenario_path)
    wrong_entry = install_root / "dist" / "lib.js"
    wrong_entry.write_text("// a library module, never a runnable entry point\n", encoding="utf-8")
    # node_binary must still be the real, resolved interpreter for
    # verify_install's own is_file()/X_OK/untrusted-location checks -- only
    # adapter_entry is swapped.
    from engine_fakes import _resolved_node_binary

    launch_command = (_resolved_node_binary(), str(wrong_entry))
    record = _record_with_launch_command(tmp_path, launch_command, "claude-agent-acp", environ)

    with pytest.raises(PortableError) as excinfo:
        install_resolution.resolve_verified_install(record, environ)
    assert excinfo.value.code == "engine_install_drifted"
    assert excinfo.value.field == "adapter_entry"


# --- unresolvable: never a verification failure -----------------------------


def test_fake_engine_launch_command_is_unresolvable_not_a_verification_failure(tmp_path):
    """This repository's own fake-engine test doubles (4-element
    launch_command, no real npm package anywhere above them) are reported
    as InstallUnresolvable, never as a PortableError -- a caller (see
    delegation.live_adapters) must be able to tell "not a real npm install"
    apart from "a real npm install that failed verification"."""
    from engine_fakes import fake_launch_command

    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    record = _record_with_launch_command(tmp_path, fake_launch_command(scenario_path), "codex-acp", environ)

    with pytest.raises(install_resolution.InstallUnresolvable):
        install_resolution.resolve_verified_install(record, environ)


def test_missing_launch_command_is_unresolvable(tmp_path):
    environ = isolated_environ(tmp_path)
    record = _record_with_launch_command(tmp_path, None, "codex-acp", environ)
    with pytest.raises(install_resolution.InstallUnresolvable):
        install_resolution.resolve_verified_install(record, environ)


class _FakeRecord:
    """A minimal duck-typed stand-in carrying only the two attributes
    resolve_verified_install actually reads -- a real ConnectionRecord
    refuses to even construct for an engine outside connections.model
    .SUPPORT_MATRIX, so an "unknown engine" case can't be expressed with
    one."""

    def __init__(self, engine, launch_command):
        self.engine = engine
        self.launch_command = launch_command


def test_unknown_engine_is_unresolvable(tmp_path):
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    launch_command, _binary = build_codex_npm_install(tmp_path / "install", scenario_path)
    record = _FakeRecord("some-other-engine", launch_command)
    with pytest.raises(install_resolution.InstallUnresolvable):
        install_resolution.resolve_verified_install(record, environ)


def test_codex_install_missing_bundled_codex_package_is_unresolvable(tmp_path):
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    install_root = tmp_path / "install"
    install_root.mkdir()
    dist = install_root / "dist"
    dist.mkdir()
    from engine_fakes import write_wrapper_adapter_entry

    adapter_entry = dist / "index.js"
    write_wrapper_adapter_entry(adapter_entry, scenario_path)
    (install_root / "package.json").write_text(
        json.dumps({"name": "@agentclientprotocol/codex-acp", "version": "1.12.0"}), encoding="utf-8",
    )
    # No node_modules/@openai/codex at all.
    from engine_fakes import _resolved_node_binary

    record = _record_with_launch_command(tmp_path, (_resolved_node_binary(), str(adapter_entry)), "codex-acp", environ)
    with pytest.raises(install_resolution.InstallUnresolvable):
        install_resolution.resolve_verified_install(record, environ)
