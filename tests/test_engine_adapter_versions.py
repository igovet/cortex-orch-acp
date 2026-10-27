"""Exact SDK-2 adapter qualification; no model calls or native-account access."""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.connections import discovery
from cortex_runtime.delegation.generations import AUTOMATIC, tier_for
from cortex_runtime.engine.instruction_delivery_registry import delivery_for, has_exact_registration
from cortex_runtime.engine.process import _prepare_codex_manual_review_adapter
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.instruction_delivery import DeliveryQualification
from cortex_runtime.portable.roles import load_role
from engine_fakes import isolated_environ, new_connection_id


NEW_VERSIONS = [("codex-acp", "2.3.0", "2.1.1"), ("claude-agent-acp", "0.97.0", "0.88.0")]


def _installed(engine, version):
    package = discovery.owned_root() / engine / version / "node_modules/@agentclientprotocol" / engine
    if not (package / "package.json").is_file():
        pytest.skip(f"Installed {engine} {version} is unavailable")
    assert json.loads((package / "package.json").read_text())["version"] == version
    return package


@pytest.mark.parametrize("engine,version,rollback", NEW_VERSIONS)
def test_exact_new_and_rollback_registrations(engine, version, rollback):
    role = load_role("worker-backend-dev")
    for exact in (version, rollback):
        assert discovery.is_qualified(engine, exact)
        assert discovery.is_pinned(engine, exact)
        assert has_exact_registration(engine, exact)
        plan = delivery_for(engine, exact).plan(role, [])
        assert plan.qualification == DeliveryQualification.QUALIFIED
        assert plan.version == exact and plan.blocks == ()
        assert plan.session_new_meta["systemPrompt"] == (
            role.body_portable if engine == "codex-acp" else {"append": role.body_portable}
        )
        assert tier_for(engine, exact).tier == AUTOMATIC
    assert not discovery.is_qualified(engine, "99.0.0")
    assert not has_exact_registration(engine, "99.0.0")
    assert delivery_for(engine, "99.0.0").plan(role, []).qualification == DeliveryQualification.UNSUPPORTED


@pytest.mark.parametrize("engine,version,rollback", NEW_VERSIONS)
def test_discovery_prefers_new_qualified_but_retains_rollback(tmp_path, engine, version, rollback):
    root = tmp_path / engine
    for exact in (rollback, version, "99.0.0"):
        package = root / exact / "node_modules/@agentclientprotocol" / engine
        (package / "dist").mkdir(parents=True)
        (package / "dist/index.js").write_text("")
        (package / "package.json").write_text(json.dumps({
            "version": exact, "bin": {engine: "dist/index.js"},
        }))
    found = discovery.discover_engine(engine, registry_root=root, node_binary="/usr/bin/node")
    assert found.version == version
    assert discovery.is_pinned(engine, rollback)


def test_installed_codex_2_3_rejects_unregistered_bundle_drift(tmp_path):
    package = _installed("codex-acp", "2.3.0")
    source = (package / "dist/index.js").read_bytes() + b"\n// drift outside patched call sites\n"
    target = tmp_path / "node_modules/@agentclientprotocol/codex-acp"
    (target / "dist").mkdir(parents=True)
    (target / "dist/index.js").write_bytes(source)
    (target / "package.json").write_bytes((package / "package.json").read_bytes())
    install = SimpleNamespace(
        package_version="2.3.0", adapter_entry=str(target / "dist/index.js"),
        package_json=str(target / "package.json"), adapter_entry_sha256=hashlib.sha256(source).hexdigest(),
    )
    with pytest.raises(PortableError) as refused:
        _prepare_codex_manual_review_adapter(install, new_connection_id(), isolated_environ(tmp_path))
    assert refused.value.code == "codex_manual_review_adapter_unsupported"


def test_installed_claude_0_97_instruction_and_policy_contract():
    package = _installed("claude-agent-acp", "0.97.0")
    source = (package / "dist/acp-agent.js").read_text()
    assert hashlib.sha256(source.encode()).hexdigest() == (
        "c569a914b2b9c75127193139a69bca3ca4ae6b2ebd6728de0330a16689fdc268"
    )
    sdk = package.parents[1] / "@anthropic-ai/claude-agent-sdk"
    assert json.loads((sdk / "package.json").read_text())["version"] == "0.3.310"
    sdk_source = (sdk / "sdk.mjs").read_text()
    assert hashlib.sha256(sdk_source.encode()).hexdigest() == (
        "4ee428f7e39da03262a8a3fc547d3ec60801d3af17b2d40e553c0086ffdac1f8"
    )
    for fragment in (
        "protocolVersion: 1", "const customPrompt = params._meta.systemPrompt;",
        "systemPrompt = customPrompt;", 'preset: "claude_code",',
        "const options = {\n            systemPrompt,", "_meta: params._meta,",
        "prompt: input,\n            options,", "...userProvidedOptions,",
        "disallowedTools: [...(userProvidedOptions?.disallowedTools || []), ...disallowedTools]",
        "const tools = userProvidedOptions?.tools ??", "            tools,",
        "await this.cancelTurns(params, { awaitInterrupt: true });",
        "const interrupt = session.query.interrupt();",
    ):
        assert fragment in source
    for fragment in (
        "systemPrompt:f,appendSystemPrompt:m", "systemPrompt:typeof this.initConfig?.systemPrompt",
        "appendSystemPrompt:this.initConfig?.appendSystemPrompt",
        '--strict-mcp-config', '--disallowedTools', '--tools=default',
    ):
        assert fragment in sdk_source
    compaction = (package / "dist/tool-calls/reporters/compaction.js").read_text()
    assert 'claudeCode: { toolName: "compact" }' in compaction
    for name, category in (("session-model.js", "model"), ("session-effort.js", "thought_level"),
                           ("session-mode.js", "mode")):
        options = (package / "dist" / name).read_text()
        assert 'type: "select"' in options
        assert f'category: "{category}"' in options


@pytest.mark.parametrize("engine,version,rollback", NEW_VERSIONS)
def test_installed_adapter_v1_initialize_without_account_or_prompt(tmp_path, engine, version, rollback):
    package = _installed(engine, version)
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is unavailable")
    home = tmp_path / "home"
    home.mkdir()
    (home / "codex").mkdir()
    (home / "claude").mkdir()
    # Only initialize is sent. Account/config directories are empty and isolated.
    env = {"PATH": os.environ.get("PATH", ""), "HOME": str(home),
           "XDG_CONFIG_HOME": str(home / "config"), "XDG_STATE_HOME": str(home / "state"),
           "XDG_CACHE_HOME": str(home / "cache"), "XDG_DATA_HOME": str(home / "data"),
           "CLAUDE_CONFIG_DIR": str(home / "claude"), "CODEX_HOME": str(home / "codex"),
           "NO_BROWSER": "1"}
    from test_codex_developer_instructions import _read_response

    process = subprocess.Popen([node, str(package / "dist/index.js")],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, env=env, cwd=tmp_path)
    try:
        process.stdin.write(json.dumps({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {},
                       "clientInfo": {"name": "cortex-no-paid-test", "version": "1"}},
        }) + "\n")
        process.stdin.flush()
        response = _read_response(process, 1)
        assert "error" not in response, response
        assert response["result"]["protocolVersion"] == 1
        assert response["result"]["agentInfo"]["version"] == version
    finally:
        process.terminate()
        try:
            process.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate(timeout=3)
