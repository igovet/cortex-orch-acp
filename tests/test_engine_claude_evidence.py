"""engine.claude_adapter_acp evidence statuses and instruction delivery.

Mirrors tests/test_engine_codex_evidence.py."""
from __future__ import annotations

import pytest
import json
import os
import subprocess
from pathlib import Path

from cortex_runtime.engine.claude_adapter_acp import (
    EXECUTION_VERIFICATION_UNAVAILABLE,
    ClaudeEvidenceStatus,
    build_claude_evidence_status,
    build_claude_session_new_meta,
    build_first_prompt_blocks,
    claude_native_subagent_restriction_meta,
)
from cortex_runtime.portable.errors import PortableError


def test_build_claude_evidence_status_distinct_fields():
    status = build_claude_evidence_status("cr_" + "a" * 12, "eirt_x:sess-1:model=model-a")
    assert status.selected == "cr_" + "a" * 12
    assert status.configuration_applied == "eirt_x:sess-1:model=model-a"
    assert status.execution_verified == EXECUTION_VERIFICATION_UNAVAILABLE
    assert status.selected != status.configuration_applied
    assert status.execution_verified not in (status.selected, status.configuration_applied)


def test_evidence_status_execution_verified_is_never_settable_to_anything_else():
    with pytest.raises(PortableError) as excinfo:
        ClaudeEvidenceStatus(selected="cr_x", configuration_applied="ev_x", execution_verified="confirmed")
    assert excinfo.value.code == "invalid_field"


def test_evidence_status_requires_non_empty_selected_and_applied():
    with pytest.raises(PortableError):
        build_claude_evidence_status("", "ev_x")
    with pytest.raises(PortableError):
        build_claude_evidence_status("cr_x", "")


def test_build_first_prompt_blocks_prepends_leading_text_block():
    blocks = build_first_prompt_blocks("You are operating under Cortex orchestration.", [{"type": "text", "text": "do the thing"}])
    assert blocks[0] == {"type": "text", "text": "You are operating under Cortex orchestration."}
    assert blocks[1] == {"type": "text", "text": "do the thing"}
    assert len(blocks) == 2


def test_build_first_prompt_blocks_rejects_empty_instructions():
    with pytest.raises(PortableError) as excinfo:
        build_first_prompt_blocks("   ", [])
    assert excinfo.value.code == "invalid_field"


def test_build_first_prompt_blocks_rejects_non_list_user_blocks():
    with pytest.raises(PortableError) as excinfo:
        build_first_prompt_blocks("instructions", "not-a-list")
    assert excinfo.value.code == "invalid_field"


def test_build_first_prompt_blocks_never_writes_a_file_or_returns_env_vars(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    before = sorted(p.name for p in tmp_path.iterdir())
    blocks = build_first_prompt_blocks("instructions", [])
    after = sorted(p.name for p in tmp_path.iterdir())
    assert before == after == []
    assert blocks == [{"type": "text", "text": "instructions"}]


def test_claude_disallows_native_gateway_alternatives_for_every_role():
    for role in ['coordinator','worker']:
        meta=build_claude_session_new_meta('Cortex instructions',role=role)
        assert meta['systemPrompt']=={'append':'Cortex instructions'}
        options=meta['claudeCode']['options']
        assert 'tools' not in options
        assert options['skills']==[]
        assert 'settingSources' not in options
        assert options['plugins']==[]
        assert options['strictMcpConfig'] is True
        assert options['managedSettings']['disableAllHooks'] is True
        assert {'Agent','Task','Workflow','TeamCreate','Bash','Read','Edit','Write',
                'Grep','Glob','Skill','NotebookEdit'}<=set(options['disallowedTools'])
        assert not any(name.startswith('mcp__') for name in options['disallowedTools'])


def test_installed_claude_acp_0_78_accepts_tool_allowlist_and_forwards_it():
    """Static exact-install forwarding check; it does not call Claude."""
    root = Path("/home/igovet/.local/state/cortex/engines/claude-agent-acp")
    adapter = root / "dist/acp-agent.js"
    sdk_types = root / "node_modules/@anthropic-ai/claude-agent-sdk/sdk.d.ts"
    if not adapter.exists() or not sdk_types.exists():
        pytest.skip("Pinned local Claude ACP/SDK copy is unavailable")
    package = json.loads((root / "package.json").read_text(encoding="utf-8"))
    assert package["version"] == "0.78.0"
    adapter_source = adapter.read_text(encoding="utf-8")
    sdk_source = sdk_types.read_text(encoding="utf-8")
    assert "const tools = userProvidedOptions?.tools ??" in adapter_source
    assert "disallowedTools: [...(userProvidedOptions?.disallowedTools || []), ...disallowedTools]" in adapter_source
    assert "tools?: string[] | {" in sdk_source
    assert "disallowedTools?: string[];" in sdk_source
    assert "passing 'Skill' here is deprecated" in sdk_source


def test_installed_claude_acp_0_81_forwards_disallowed_tools_without_a_model_call():
    root = Path.home() / ".local/share/cortex/engines/claude-agent-acp/0.81.2/node_modules"
    adapter = root / "@agentclientprotocol/claude-agent-acp"
    sdk_types = root / "@anthropic-ai/claude-agent-sdk/sdk.d.ts"
    if not (adapter / "package.json").exists() or not sdk_types.exists():
        pytest.skip("Installed Claude ACP 0.81.2/SDK copy is unavailable")
    package = json.loads((adapter / "package.json").read_text(encoding="utf-8"))
    assert package["version"] == "0.81.2"
    source = (adapter / "dist/acp-agent.js").read_text(encoding="utf-8")
    assert "...userProvidedOptions" in source
    assert "disallowedTools: [...(userProvidedOptions?.disallowedTools || []), ...disallowedTools]" in source
    assert "disallowedTools?: string[];" in sdk_types.read_text(encoding="utf-8")


def test_installed_claude_acp_0_78_initialize_is_no_paid_and_reports_pinned_version(tmp_path):
    """The exact installed entrypoint must expose ACP before authentication."""
    root = Path("/home/igovet/.local/state/cortex/engines/claude-agent-acp")
    entry = root / "dist/index.js"
    if not entry.exists():
        pytest.skip("Pinned local Claude ACP copy is unavailable")
    env = os.environ.copy()
    env["CLAUDE_CONFIG_DIR"] = str(tmp_path / "empty-claude-config")
    env["HOME"] = str(tmp_path / "empty-home")
    result = subprocess.run(
        ["node", str(entry)],
        input=(json.dumps({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {"name": "cortex-no-paid-test", "version": "1"},
            },
        }) + "\n").encode(),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env=env, cwd=tmp_path, timeout=10, check=False,
    )
    assert result.returncode == 0, result.stderr.decode(errors="replace")
    messages = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    response = next(message for message in messages if message.get("id") == 1)
    assert response["result"]["agentInfo"]["version"] == "0.78.0"


def test_build_claude_session_new_meta_rejects_empty_instructions():
    with pytest.raises(PortableError) as excinfo:
        build_claude_session_new_meta("   ")
    assert excinfo.value.code == "invalid_field"


def test_installed_claude_acp_0_78_forwards_strict_mcp_to_sdk_cli():
    """No engine call: verify this host's exact pinned adapter and SDK path."""
    root = Path.home() / ".local/share/zed/external_agents/registry/npx/claude-acp/node_modules"
    adapter = root / "@agentclientprotocol/claude-agent-acp"
    sdk = root / "@anthropic-ai/claude-agent-sdk"
    if not (adapter / "package.json").exists() or not (sdk / "sdk.mjs").exists():
        pytest.skip("Installed Claude ACP/SDK copy is unavailable on this host")
    version = json.loads((adapter / "package.json").read_text(encoding="utf-8"))["version"]
    if version != "0.78.0":
        pytest.skip(f"Installed Claude ACP is {version}, not pinned 0.78.0")
    adapter_source = (adapter / "dist/acp-agent.js").read_text(encoding="utf-8")
    sdk_source = (sdk / "sdk.mjs").read_text(encoding="utf-8")
    sdk_types = (sdk / "sdk.d.ts").read_text(encoding="utf-8")
    assert "...userProvidedOptions," in adapter_source
    assert "...(userProvidedOptions?.mcpServers || {})," in adapter_source
    assert "...mcpServers," in adapter_source
    assert "strictMcpConfig?: boolean;" in sdk_types
    assert "Only use MCP servers passed via the `mcpServers` option" in sdk_types
    assert 'if(ee)W.push("--strict-mcp-config")' in sdk_source


def _installed_0_87():
    from cortex_runtime.connections.discovery import owned_root
    root = owned_root() / "claude-agent-acp/0.87.0/node_modules"
    adapter = root / "@agentclientprotocol/claude-agent-acp"
    sdk = root / "@anthropic-ai/claude-agent-sdk"
    if not (adapter / "package.json").is_file() or not (sdk / "sdk.mjs").is_file():
        pytest.skip("Installed Claude ACP 0.87.0/SDK copy is unavailable")
    assert json.loads((adapter / "package.json").read_text())["version"] == "0.87.0"
    assert json.loads((sdk / "package.json").read_text())["version"] == "0.3.287"
    return adapter, sdk


def test_installed_0_87_system_options_and_compaction_source_without_model_call():
    adapter, sdk = _installed_0_87()
    source = (adapter / "dist/acp-agent.js").read_text()
    sdk_source = (sdk / "sdk.mjs").read_text()
    assert "const customPrompt = params._meta.systemPrompt;" in source
    assert "systemPrompt = customPrompt;" in source
    assert "const options = {\n            systemPrompt," in source
    assert "...userProvidedOptions," in source
    assert "disallowedTools: [...(userProvidedOptions?.disallowedTools || []), ...disallowedTools]" in source
    assert "prompt: input,\n            options," in source
    assert "systemPrompt:p,appendSystemPrompt:f" in sdk_source
    assert "strictMcpConfig?: boolean;" in (sdk / "sdk.d.ts").read_text()
    assert 'claudeCode: { toolName: "compact" }' in (
        adapter / "dist/tool-calls/reporters/compaction.js"
    ).read_text()


def test_installed_0_87_initialize_without_auth_or_prompt(tmp_path):
    adapter, _sdk = _installed_0_87()
    env = {**os.environ, "HOME": str(tmp_path / "home"),
           "CLAUDE_CONFIG_DIR": str(tmp_path / "claude")}
    result = subprocess.run(["node", str(adapter / "dist/index.js")], input=json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": {},
                   "clientInfo": {"name": "cortex-no-paid-test", "version": "1"}},
    }) + "\n", capture_output=True, text=True, env=env, cwd=tmp_path, timeout=10)
    assert result.returncode == 0
    response = next(json.loads(line) for line in result.stdout.splitlines()
                    if json.loads(line).get("id") == 1)
    assert response["result"]["protocolVersion"] == 1
    assert response["result"]["agentInfo"]["version"] == "0.87.0"


def test_installed_0_87_sdk293_refresh_keeps_instruction_and_tool_boundaries():
    from cortex_runtime.connections.discovery import owned_root
    root = owned_root() / "claude-agent-acp/0.87.0+sdk.0.3.293/node_modules"
    adapter = root / "@agentclientprotocol/claude-agent-acp"
    sdk = root / "@anthropic-ai/claude-agent-sdk"
    if not (adapter / "package.json").is_file() or not (sdk / "sdk.mjs").is_file():
        pytest.skip("Installed Claude ACP 0.87.0/SDK 0.3.293 refresh is unavailable")
    assert json.loads((adapter / "package.json").read_text())["version"] == "0.87.0"
    assert json.loads((sdk / "package.json").read_text())["version"] == "0.3.293"
    source = (adapter / "dist/acp-agent.js").read_text()
    sdk_source = (sdk / "sdk.mjs").read_text()
    assert "prompt: input,\n            options," in source
    assert "disallowedTools: [...(userProvidedOptions?.disallowedTools || []), ...disallowedTools]" in source
    assert "systemPrompt:typeof this.initConfig?.systemPrompt" in sdk_source
    assert "appendSystemPrompt:this.initConfig?.appendSystemPrompt" in sdk_source
    assert 'if(R)K.push("--strict-mcp-config")' in sdk_source
    assert 'K.push("--disallowedTools",U.join(","))' in sdk_source
    assert "claude-haiku-5-5" in sdk_source


def _installed_0_88():
    from cortex_runtime.connections.discovery import owned_root
    root = owned_root() / "claude-agent-acp/0.88.0/node_modules"
    adapter = root / "@agentclientprotocol/claude-agent-acp"
    sdk = root / "@anthropic-ai/claude-agent-sdk"
    if not (adapter / "package.json").is_file() or not (sdk / "sdk.mjs").is_file():
        pytest.skip("Installed Claude ACP 0.88.0/SDK copy is unavailable")
    assert json.loads((adapter / "package.json").read_text())["version"] == "0.88.0"
    assert json.loads((sdk / "package.json").read_text())["version"] == "0.3.293"
    return adapter, sdk


def test_installed_0_88_instruction_options_and_compaction_source_without_model_call():
    adapter, sdk = _installed_0_88()
    source = (adapter / "dist/acp-agent.js").read_text()
    sdk_source = (sdk / "sdk.mjs").read_text()
    assert "protocolVersion: 1" in source
    assert "const customPrompt = params._meta.systemPrompt;" in source
    assert "systemPrompt = customPrompt;" in source
    assert '...customPrompt,\n                    type: "preset",\n                    preset: "claude_code",' in source
    assert "const options = {\n            systemPrompt," in source
    assert "...userProvidedOptions," in source
    assert "_meta: params._meta," in source
    assert "prompt: input,\n            options," in source
    assert "disallowedTools: [...(userProvidedOptions?.disallowedTools || []), ...disallowedTools]" in source
    assert "const tools = userProvidedOptions?.tools ??" in source
    assert "            tools," in source
    assert '"_claude/model": lastAssistantModel' in source
    assert source.count("update: attachUsageModel({") == 4
    assert "systemPrompt:p,appendSystemPrompt:f" in sdk_source
    assert "systemPrompt:typeof this.initConfig?.systemPrompt" in sdk_source
    assert "appendSystemPrompt:this.initConfig?.appendSystemPrompt" in sdk_source
    assert 'if(R)K.push("--strict-mcp-config")' in sdk_source
    assert 'K.push("--disallowedTools",U.join(","))' in sdk_source
    assert 'K.push("--tools","default")' in sdk_source
    assert 'claudeCode: { toolName: "compact" }' in (
        adapter / "dist/tool-calls/reporters/compaction.js"
    ).read_text()
    for name, category in [("session-model.js", "model"),
                           ("session-effort.js", "thought_level"), ("session-mode.js", "mode")]:
        config = (adapter / "dist" / name).read_text()
        assert 'type: "select"' in config
        assert f'category: "{category}"' in config


def test_installed_0_88_initialize_without_auth_or_prompt(tmp_path):
    adapter, _sdk = _installed_0_88()
    env = {**os.environ, "HOME": str(tmp_path / "home"),
           "CLAUDE_CONFIG_DIR": str(tmp_path / "claude")}
    result = subprocess.run(["node", str(adapter / "dist/index.js")], input=json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": {},
                   "clientInfo": {"name": "cortex-no-paid-test", "version": "1"}},
    }) + "\n", capture_output=True, text=True, env=env, cwd=tmp_path, timeout=10)
    assert result.returncode == 0
    response = next(json.loads(line) for line in result.stdout.splitlines()
                    if json.loads(line).get("id") == 1)
    assert response["result"]["protocolVersion"] == 1
    assert response["result"]["agentInfo"]["version"] == "0.88.0"
