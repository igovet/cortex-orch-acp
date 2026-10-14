"""Per-connection engine process launch, isolation, and termination.

Launches only from ``ConnectionRecord.launch_command`` -- never a
discovered/guessed executable: a missing or relative ``launch_command`` is
refused before any subprocess is started. The process environment comes
from ``connections.environment.build_process_env`` with
``channel_credentials=None`` (the trusted-channel secret is delivered to
the *MCP server* subprocess the engine itself spawns, per
``mcp_bridge.build_mcp_servers`` -- never placed in this adapter process's
own environment).
"""
from __future__ import annotations

import contextlib
import os
import hashlib
import shutil
import sys
import signal
import concurrent.futures
import subprocess
import tempfile
import threading
import uuid
from pathlib import Path

from ..connections.environment import build_process_env
from ..connections import registry
from ..portable.errors import PortableError, PortableErrorCategory
from . import claude_install, codex_install, opencode_install
from .logs import EngineLogs
from .broker import McpBroker

# Per-engine (verify_install, launch_command_for) pair -- codex-acp's own
# verified install additionally pins CODEX_PATH/NO_BROWSER below; a
# claude-agent-acp install carries no equivalent second binary to pin (see
# claude_install's own module docstring).
_INSTALL_MODULE_BY_ENGINE = {
    "codex-acp": codex_install,
    "claude-agent-acp": claude_install,
    "opencode": opencode_install,
}


def _require_opencode_isolation(jail, install) -> None:
    """OpenCode runs only verified, only in the engine jail, and only where
    that jail isolates its processes (see ``engine_jail(isolate_processes=)``).

    Each ``opencode acp`` starts a private control server whose password is
    in its environment. The kernel keeps a sibling jail from reading that
    environment only because every bubblewrap sandbox gets its own user
    namespace -- which a setuid bwrap does not create, so a setuid bwrap is
    refused rather than trusted.
    """
    import stat as stat_module

    if jail is None:
        raise _validation_error(
            "opencode_requires_engine_jail",
            "OpenCode is only launched inside Cortex's engine jail.", field="jail",
        )
    if install is None:
        raise _validation_error(
            "opencode_install_unverified",
            "OpenCode is only launched from a verified install (engine.install_resolution).", field="install",
        )
    bwrap = shutil.which("bwrap")
    try:
        setuid = bwrap is None or bool(os.stat(bwrap).st_mode & stat_module.S_ISUID)
    except OSError:
        setuid = True
    if setuid:
        raise _validation_error(
            "opencode_process_isolation_unavailable",
            "OpenCode needs an unprivileged (non-setuid) bubblewrap, whose per-sandbox user namespace keeps "
            "other jails from reading its control-server credential.",
        )

TERMINATE_GRACE_SECONDS = 5.0

# The verified codex-acp 1.12.0 bundle chooses an internal auto reviewer for
# its `agent` mode, so a dangerous command can be approved without an ACP
# permission request. Keep a private, exact-source-checked variant whose
# reviewer is the host user. This does not turn engine-native tools that never
# request approval into mediated operations.
_CODEX_AGENT_REVIEWER_SOURCE = (
    b'    "agent",\n    "Approve for me",\n'
    b'    "Only ask for actions detected as potentially unsafe",\n'
    b'    "auto_review",\n    "on-request",\n    "auto_review",\n'
)
_CODEX_AGENT_REVIEWER_MANUAL = _CODEX_AGENT_REVIEWER_SOURCE.replace(
    b'    "on-request",\n    "auto_review",\n',
    b'    "on-request",\n    "user",\n',
)

# Only these pinned upstream call sites are transformed. The ACP _meta field is
# Cortex-owned; upstream 1.12.0 otherwise ignores systemPrompt. App Server's
# thread/start developerInstructions is a request-level override of native
# config, so read and retain the effective native developer instructions.
# Two further Cortex-owned _meta fields, both absent by default: a string
# cortexBaseInstructions becomes thread/start baseInstructions and replaces
# Codex's model base prompt ("" removes it); cortexNativeContext=false turns
# off the permissions, apps, collaboration-mode, skills-catalog and
# environment-context sections through that thread's config. Both were
# observed against Codex CLI 0.155.1's own App Server schema and requests.
_CODEX_NEW_SESSION_SOURCE = b'''    const response = await this.codexClient.threadStart({
      config: await this.createSessionConfig(request.cwd, additionalDirectories, request.mcpServers),
      modelProvider: this.getModelProvider(),
      cwd: request.cwd
    });'''
_CODEX_NEW_SESSION_PATCHED = b'''    const sessionConfig = await this.createSessionConfig(request.cwd, additionalDirectories, request.mcpServers);
    const cortexRole = request._meta?.cortexRole;
    if (cortexRole !== undefined && cortexRole !== "coordinator" && cortexRole !== "worker") {
      throw RequestError.invalidParams({}, "Unsupported Cortex role");
    }
    const cortexInstructions = request._meta?.systemPrompt;
    let developerInstructions;
    if (cortexInstructions !== void 0) {
      if (typeof cortexInstructions !== "string" || cortexInstructions.trim().length === 0) {
        throw RequestError.invalidParams({}, "Cortex systemPrompt must be a non-empty string");
      }
      const nativeResponse = await this.codexClient.configRead({ includeLayers: false, cwd: request.cwd });
      if (!nativeResponse || !nativeResponse.config) {
        throw new Error("Could not verify native Codex developer instructions");
      }
      const nativeInstructions = sessionConfig.developer_instructions ?? nativeResponse.config.developer_instructions;
      if (nativeInstructions != null && typeof nativeInstructions !== "string") {
        throw new Error("Native Codex developer instructions have an unsupported shape");
      }
      developerInstructions = nativeInstructions ? nativeInstructions + "\\n\\n" + cortexInstructions : cortexInstructions;
    }
    const cortexBaseInstructions = request._meta?.cortexBaseInstructions;
    if (cortexBaseInstructions !== void 0 && typeof cortexBaseInstructions !== "string") {
      throw RequestError.invalidParams({}, "Cortex base instructions must be a string");
    }
    const cortexNativeContext = request._meta?.cortexNativeContext;
    if (cortexNativeContext !== void 0 && typeof cortexNativeContext !== "boolean") {
      throw RequestError.invalidParams({}, "Cortex native context flag must be a boolean");
    }
    if (cortexNativeContext === false) {
      Object.assign(sessionConfig, {
        include_permissions_instructions: false,
        include_apps_instructions: false,
        include_collaboration_mode_instructions: false,
        include_environment_context: false,
        skills: { ...(sessionConfig.skills ?? {}), include_instructions: false }
      });
    }
    const response = await this.codexClient.threadStart({
      config: sessionConfig,
      modelProvider: this.getModelProvider(),
      cwd: request.cwd,
      ...(developerInstructions === void 0 ? {} : { developerInstructions }),
      ...(cortexBaseInstructions === void 0 ? {} : { baseInstructions: cortexBaseInstructions })
    });
    this.cortexSessionRoles ??= new Map();
    this.cortexSessionRoles.set(response.thread.id, cortexRole);'''

# App Server combines thread/start.config after the process -c overrides.
# codex-acp's top-level features table replaces the preceding -c features
# table, so the process-only check cannot establish a disabled turn. Patch the
# common session config builder used by new, resumed, and forked sessions.
_CODEX_SESSION_CONFIG_SOURCE = b'''function forceGitRootTurnDiffPaths(config2) {
  const features = isJsonObject(config2["features"]) ? config2["features"] : {};
  return {
    ...config2,
    features: {
      ...features,
      cwd_relative_turn_diffs: false
    }
  };
}'''
_CODEX_SESSION_CONFIG_PATCHED = b'''function forceGitRootTurnDiffPaths(config2) {
  const features = isJsonObject(config2["features"]) ? config2["features"] : {};
  const agents = isJsonObject(config2["agents"]) ? config2["agents"] : {};
  const {
    ["agents.enabled"]: ignoredAgentsEnabled,
    ["features.multi_agent"]: ignoredMultiAgent,
    ["features.multi_agent_v2"]: ignoredMultiAgentV2,
    ["features.hooks"]: ignoredHooks,
    ["features.apps"]: ignoredApps,
    ["features.plugins"]: ignoredPlugins,
    ["features.shell_tool"]: ignoredShellTool,
    ["features.view_image"]: ignoredViewImage,
    ...remainingConfig
  } = config2;
  return {
    ...remainingConfig,
    agents: {
      ...agents,
      enabled: false
    },
    features: {
      ...features,
      cwd_relative_turn_diffs: false,
      multi_agent: false,
      multi_agent_v2: false,
      plugins: false,
      hooks: false,
      apps: false,
      shell_tool: false,
      view_image: false
    },
    tools: { experimental_request_user_input: { enabled: false } }
  };
}'''

# Native server names are disabled at child launch. A session-scoped server
# with the same name must still be configured; upstream's deduplication
# otherwise treats the disabled native entry as a reason to drop it.
_CODEX_MCP_DEDUP_SOURCE = b'''    let serversToConfigure = requestedServers;
    if (shouldDeduplicateMcpConflicts()) {
      const existingNames = await this.getConfigMcpServerNames(projectPath);
      serversToConfigure = requestedServers.filter((mcp) => !existingNames.has(mcp.name));
    }'''
_CODEX_MCP_DEDUP_PATCHED = b'''    const serversToConfigure = requestedServers;'''
_CODEX_MCP_ENTRY_SOURCE = b'''"mcp_servers": Object.fromEntries(serversToConfigure.map((mcp) => [mcp.name, this.createMcpSeverConfig(mcp.server)]))'''
# Codex bounds every MCP tools/call by tool_timeout_sec (60 s by default),
# which would cut a long command_run short. One year is effectively
# unbounded and stays inside tokio's timer range (~2 years).
_CODEX_MCP_ENTRY_PATCHED = b'''"mcp_servers": Object.fromEntries(serversToConfigure.map((mcp) => [mcp.name, { ...this.createMcpSeverConfig(mcp.server), enabled: true, default_tools_approval_mode: "approve", tool_timeout_sec: 31536000 }]))'''
# Codex compacts an MCP tool input schema larger than
# tool_input_schema_max_bytes (default 5,000 normalized bytes) and its first
# pass strips every description, so plan_submit and result_submit reached a
# code-mode model as bare types (Codex 0.159.1 json_schema/compaction.rs).
# 32 KiB keeps every Cortex schema whole; the same limit also raises code
# mode's declaration budget. Codex 0.155.1 (codex-acp 1.x) has no such key.
CODEX_MCP_TOOL_SCHEMA_MAX_BYTES = 32_768
_CODEX_MCP_ENTRY_2_PATCHED = _CODEX_MCP_ENTRY_PATCHED.replace(
    b"tool_timeout_sec: 31536000 }",
    b"tool_timeout_sec: 31536000, tool_input_schema_max_bytes: " + str(CODEX_MCP_TOOL_SCHEMA_MAX_BYTES).encode() + b" }",
)

# Codex runs inside Cortex's engine jail (orchestration.sandbox.engine_jail),
# which is the boundary: its own sandbox cannot start in a nested user
# namespace, so every turn is full access. It still asks ("untrusted": all
# but known-safe reads) so each command reaches engine.permissions, which
# allows it at once unless it deletes -- that one the user confirms.
_CODEX_SEND_PROMPT_SOURCE = b'''    return await this.codexClient.runTurn({
      threadId: request.sessionId,
      input,
      approvalPolicy: agentMode.approvalPolicy,
      approvalsReviewer: agentMode.approvalsReviewer,
      sandboxPolicy: addAdditionalDirectoriesToSandboxPolicy(agentMode.sandboxPolicy, additionalDirectories),
      summary: disableSummary ? "none" : "auto",
      effort,
      model: modelId.model,
      serviceTier
    }, onTurnStarted);'''
_CODEX_SEND_PROMPT_PATCHED = b'''    return await this.codexClient.runTurn({
      threadId: request.sessionId,
      input,
      approvalPolicy: "untrusted",
      approvalsReviewer: "user",
      sandboxPolicy: { type: "dangerFullAccess" },
      summary: disableSummary ? "none" : "auto",
      effort,
      model: modelId.model,
      serviceTier
    }, onTurnStarted);'''
_CODEX_CUSTOM_TOOL_HELPERS_INSERT = b'''function createCustomToolCallUpdate(item) {
  const toolCallId = stringValue(item["call_id"]) ?? stringValue(item["callId"]) ?? stringValue(item["id"]);
  const name = stringValue(item["name"]);
  if (!toolCallId || !name) return null;
  const input = item["input"] ?? item["arguments"] ?? item["parameters"];
  const record = asRecord3(input);
  const command = name === "exec" ? stringValue(record?.cmd) ?? stringValue(record?.command) ?? (typeof input === "string" ? input : null) : null;
  const title = command ? stripShellPrefix(command) : name;
  const kind = name === "exec" ? "execute" : name === "apply_patch" ? "edit" : name === "view_image" ? "read" : "other";
  return {
    sessionUpdate: "tool_call",
    toolCallId,
    name,
    kind,
    title,
    status: item["status"] === "completed" ? "completed" : ["failed", "cancelled", "incomplete"].includes(item["status"]) ? "failed" : "in_progress",
    rawInput: { name, input }
  };
}
function createCustomToolCallOutputUpdate(item) {
  const toolCallId = stringValue(item["call_id"]) ?? stringValue(item["callId"]) ?? stringValue(item["id"]);
  if (!toolCallId) return null;
  const output = item["output"] ?? item["result"] ?? item["content"];
  const outputRecord = asRecord3(output);
  const exitCode = Number.isInteger(outputRecord?.exit_code) ? outputRecord.exit_code : Number.isInteger(outputRecord?.exitCode) ? outputRecord.exitCode : null;
  const itemStatus = stringValue(item["status"]);
  const failed = outputRecord?.error != null || (exitCode !== null && exitCode !== 0);
  const status = itemStatus === "completed" ? "completed" : ["failed", "cancelled", "incomplete"].includes(itemStatus) ? "failed" : ["in_progress", "inProgress"].includes(itemStatus) ? "in_progress" : failed ? "failed" : "completed";
  return {
    sessionUpdate: "tool_call_update",
    toolCallId,
    status,
    rawOutput: { output, ...(exitCode === null ? {} : { exit_code: exitCode }) }
  };
}
'''
_CODEX_RAW_RESPONSE_METHOD_INSERT = b'''  createRawResponseItemEvent(event) {
    const item = event?.item ?? event?.responseItem ?? event?.rawResponseItem ?? event;
    if (!item || typeof item !== "object") return null;
    if (item.type === "custom_tool_call") return this.deduplicateCustomToolUpdate(createCustomToolCallUpdate(item));
    if (item.type === "custom_tool_call_output") return this.deduplicateCustomToolUpdate(createCustomToolCallOutputUpdate(item));
    return null;
  }
  resetCustomToolState() {
    this._cortexCustomToolCalls = /* @__PURE__ */ new Set();
    this._cortexCustomToolOutputs = /* @__PURE__ */ new Map();
    this._cortexCustomToolPendingOutputs = /* @__PURE__ */ new Map();
    this._cortexCustomToolTerminalWithoutOutput = /* @__PURE__ */ new Set();
  }
  deduplicateCustomToolUpdate(update) {
    if (!update || !update.toolCallId) return update;
    if (!this._cortexCustomToolCalls) this.resetCustomToolState();
    const toolCallId = update.toolCallId;
    const isStart = update.sessionUpdate === "tool_call";
    const isTerminal = update.sessionUpdate === "tool_call_update" && ["completed", "failed"].includes(update.status);
    if (isStart) {
      if (this._cortexCustomToolCalls.has(toolCallId)) return null;
      this._cortexCustomToolCalls.add(toolCallId);
      const pending = this._cortexCustomToolPendingOutputs.get(toolCallId);
      if (pending) {
        this._cortexCustomToolPendingOutputs.delete(toolCallId);
        return { ...update, status: pending.status, rawOutput: pending.rawOutput };
      }
      return update;
    }
    if (!isTerminal) return update;
    if (!this._cortexCustomToolCalls.has(toolCallId)) {
      this._cortexCustomToolPendingOutputs.set(toolCallId, update);
      return null;
    }
    const rawOutput = update.rawOutput;
    const hasRichOutput = rawOutput && Object.values(rawOutput).some((value) => value !== undefined && value !== null && value !== "");
    if (!hasRichOutput) {
      if (this._cortexCustomToolTerminalWithoutOutput.has(toolCallId)) return null;
      this._cortexCustomToolTerminalWithoutOutput.add(toolCallId);
      return update;
    }
    const fingerprint = JSON.stringify(rawOutput);
    if (this._cortexCustomToolOutputs.get(toolCallId) === fingerprint) return null;
    this._cortexCustomToolOutputs.set(toolCallId, fingerprint);
    this._cortexCustomToolTerminalWithoutOutput.add(toolCallId);
    return update;
  }
'''
_CODEX_UPDATE_EVENT_SOURCE = b'  async createUpdateEvent(notification) {\n'
_CODEX_FUNCTION_CALL_SOURCE = b'function createFunctionCallUpdate(item) {'
_CODEX_RAW_RESPONSE_CASE_SOURCE = b'''      case "externalAgentConfig/import/completed":
      case "rawResponseItem/completed":
      case "rawResponse/completed":
'''
_CODEX_RAW_RESPONSE_CASE_PATCHED = b'''      case "externalAgentConfig/import/completed":
      case "rawResponseItem/completed":
        return this.createRawResponseItemEvent(notification.params);
      case "rawResponse/completed":
'''
_CODEX_TURN_STARTED_SOURCE = b'''      case "turn/started":
        this.sessionState.currentTurnId = notification.params.turn.id;
'''
_CODEX_TURN_STARTED_PATCHED = b'''      case "turn/started":
        this.resetCustomToolState();
        this.sessionState.currentTurnId = notification.params.turn.id;
'''
_CODEX_ITEM_EVENT_SWITCH_SOURCE = b'''  async createItemEvent(event) {
    switch (event.item.type) {
'''
_CODEX_ITEM_EVENT_SWITCH_PATCHED = b'''  async createItemEvent(event) {
    switch (event.item.type) {
      case "custom_tool_call":
        return this.deduplicateCustomToolUpdate(createCustomToolCallUpdate(event.item));
      case "custom_tool_call_output":
        return this.deduplicateCustomToolUpdate(createCustomToolCallOutputUpdate(event.item));
'''
_CODEX_COMPLETE_ITEM_EVENT_SWITCH_SOURCE = b'''  async completeItemEvent(event) {
    switch (event.item.type) {
'''
_CODEX_COMPLETE_ITEM_EVENT_SWITCH_PATCHED = b'''  async completeItemEvent(event) {
    switch (event.item.type) {
      case "custom_tool_call":
        return this.deduplicateCustomToolUpdate(createCustomToolCallOutputUpdate(event.item) ?? {
          sessionUpdate: "tool_call_update",
          toolCallId: stringValue(event.item["call_id"]) ?? stringValue(event.item["callId"]) ?? stringValue(event.item["id"]),
          status: event.item.status === "completed" ? "completed" : "failed"
        });
      case "custom_tool_call_output":
        return this.deduplicateCustomToolUpdate(createCustomToolCallOutputUpdate(event.item));
'''
_CODEX_RESPONSE_ITEM_TYPES_SOURCE = b"    case \"function_call_output\":\n      return true;"
_CODEX_RESPONSE_ITEM_TYPES_PATCHED = b"    case \"function_call_output\":\n    case \"custom_tool_call\":\n    case \"custom_tool_call_output\":\n      return true;"
_CODEX_HISTORY_CASE_SOURCE = b"      default:\n        break;"
_CODEX_HISTORY_CASE_INSERT = (
    b"      case \"custom_tool_call\": {\n"
    b"        const toolCallId = stringValue(item[\"call_id\"]);\n"
    b"        if (toolCallId && existingToolCallIds.has(toolCallId)) {\n"
    b"          skippedToolCallIds.add(toolCallId);\n"
    b"          break;\n"
    b"        }\n"
    b"        if (toolCallId && emittedToolCallIds.has(toolCallId)) break;\n"
    b"        const update = createCustomToolCallUpdate(item);\n"
    b"        if (update) {\n"
    b"          recoveredFunctionCall = true;\n"
    b"          emittedToolCallIds.add(update.toolCallId);\n"
    b"          pushUpdates([update]);\n"
    b"        }\n"
    b"        break;\n"
    b"      }\n"
    b"      case \"custom_tool_call_output\": {\n"
    b"        const toolCallId = stringValue(item[\"call_id\"]);\n"
    b"        if (toolCallId && skippedToolCallIds.has(toolCallId)) break;\n"
    b"        const update = createCustomToolCallOutputUpdate(item);\n"
    b"        if (update) pushUpdates([update]);\n"
    b"        break;\n"
    b"      }\n"
)
_CODEX_HISTORY_CASE_BLOCK_SOURCE = (
    b"      case \"function_call_output\": {\n"
    b"        const toolCallId = stringValue(item[\"call_id\"]);\n"
    b"        if (toolCallId && skippedToolCallIds.has(toolCallId)) {\n"
    b"          break;\n"
    b"        }\n"
    b"        const update = createFunctionCallOutputUpdate(\n"
    b"          item,\n"
    b"          terminalOutputMode,\n"
    b"          terminalToolCallIds,\n"
    b"          execToolCallIds\n"
    b"        );\n"
    b"        if (update) {\n"
    b"          pushUpdates([update]);\n"
    b"        }\n"
    b"        break;\n"
    b"      }\n"
    b"      default:\n"
    b"        break;"
)
_CODEX_HISTORY_CASE_BLOCK_PATCHED = _CODEX_HISTORY_CASE_BLOCK_SOURCE.replace(
    b"      default:\n        break;",
    _CODEX_HISTORY_CASE_INSERT + b"      default:\n        break;",
)


# The adapter versions whose exact bundle shape this patch was verified
# against. 1.13.0 differs from 1.12.0 at exactly one patched call site --
# ``runTurn``'s input argument was renamed (``input`` -> ``input: input2``)
# -- and is byte-identical at every other call site checked below; the
# variant is derived from the 1.12.0 literal rather than duplicated, so the
# two can never drift apart silently. Every other version-keyed
# registration for this engine (instruction delivery, compaction tier) must
# name a version before it may appear here.
_CODEX_SEND_PROMPT_1_13_RENAME = (b"      input,\n", b"      input: input2,\n")
_CODEX_SEND_PROMPT_BY_VERSION = {
    "1.12.0": (_CODEX_SEND_PROMPT_SOURCE, _CODEX_SEND_PROMPT_PATCHED),
    "1.13.0": (
        _CODEX_SEND_PROMPT_SOURCE.replace(*_CODEX_SEND_PROMPT_1_13_RENAME),
        _CODEX_SEND_PROMPT_PATCHED.replace(*_CODEX_SEND_PROMPT_1_13_RENAME),
    ),
    "2.1.1": (
        _CODEX_SEND_PROMPT_SOURCE.replace(*_CODEX_SEND_PROMPT_1_13_RENAME),
        _CODEX_SEND_PROMPT_PATCHED.replace(*_CODEX_SEND_PROMPT_1_13_RENAME),
    ),
    "2.0.1": (
        _CODEX_SEND_PROMPT_SOURCE.replace(*_CODEX_SEND_PROMPT_1_13_RENAME),
        _CODEX_SEND_PROMPT_PATCHED.replace(*_CODEX_SEND_PROMPT_1_13_RENAME),
    ),
}

# 2.0.1 renamed the mode label and replaced the response-item/history helpers
# with typed reporters. 2.1.1 has byte-identical patched sites (npm bundle
# comparison, 2026-10-07); only these exact versions use the v2 patch.
_CODEX_AGENT_REVIEWER_2_SOURCE = _CODEX_AGENT_REVIEWER_SOURCE.replace(b'"Approve for me"', b'"Auto review"')
_CODEX_AGENT_REVIEWER_2_MANUAL = _CODEX_AGENT_REVIEWER_MANUAL.replace(b'"Approve for me"', b'"Auto review"')
_CODEX_CUSTOM_TOOL_HELPERS_2_INSERT = (
    b'function cortexStringValue(value) { return typeof value === "string" ? value : null; }\n'
    b'function cortexAsRecord(value) { return value && typeof value === "object" && !Array.isArray(value) ? value : null; }\n'
    + _CODEX_CUSTOM_TOOL_HELPERS_INSERT.replace(b'stringValue(', b'cortexStringValue(').replace(b'asRecord3(', b'cortexAsRecord(')
)
_CODEX_HISTORY_2_SOURCE = b'''  async createHistoryUpdates(item, sessionState) {
    const renderer = new AcpToolCallRenderer(sessionState.clientCapabilities);
    switch (item.type) {
'''
_CODEX_HISTORY_2_PATCHED = _CODEX_HISTORY_2_SOURCE + b'''      case "custom_tool_call": {
        const update = createCustomToolCallUpdate(item);
        return update ? [update] : [];
      }
      case "custom_tool_call_output": {
        const update = createCustomToolCallOutputUpdate(item);
        return update ? [update] : [];
      }
'''
_CODEX_COMPACTION_2_SOURCE = b'''    const fields = {
      ...rendered,
      ...Object.keys(meta3).length > 0 ? { _meta: meta3 } : {}
    };'''
_CODEX_COMPACTION_2_PATCHED = b'''    const fields = {
      ...rendered,
      ...(Object.keys(meta3).length > 0 || facts4.contextCompaction !== void 0) ? {
        _meta: { ...meta3, ...(facts4.contextCompaction === void 0 ? {} : { contextCompaction: facts4.contextCompaction }) }
      } : {}
    };'''
_CODEX_HELPERS_2_SOURCE = b'function stripShellPrefix(command) {'


def _private_verified_file(path: Path, content: bytes, *, mode: int = 0o600) -> None:
    if path.is_symlink():
        raise _validation_error("codex_manual_review_adapter_drifted", "The private adapter file is a symlink.")
    if not path.exists() or path.read_bytes() != content:
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=path.parent, prefix=path.name + ".", suffix=".tmp", delete=False,
        ) as temporary:
            temporary.write(content)
            temporary.flush()
            os.fsync(temporary.fileno())
            temporary_path = Path(temporary.name)
        temporary_path.chmod(mode)
        os.replace(temporary_path, path)
    path.chmod(mode)
    if path.is_symlink() or not path.is_file() or path.read_bytes() != content:
        raise _validation_error(
            "codex_manual_review_adapter_drifted",
            "The private Codex ACP adapter could not be verified.",
        )


def _resolved(path: Path) -> Path | None:
    try:
        return path.resolve()
    except OSError:
        return None


def _symlink_target(link: Path) -> Path | None:
    """The link's resolved target, or ``None`` when it cannot be resolved
    (a broken or cyclic link) -- never raises, so a stale link is always
    comparable against the verified install's own tree."""
    return _resolved(link)


def _prepare_codex_manual_review_adapter(install, connection_id: str, environ=None) -> str:
    """Return a private adapter whose agent reviewer is the user, never auto_review.

    The caller has already re-verified the pinned original bundle. A changed
    source shape, version, or copy fails before engine launch. The production
    package and native account remain untouched.
    """
    send_prompt = _CODEX_SEND_PROMPT_BY_VERSION.get(install.package_version)
    if send_prompt is None:
        raise _validation_error(
            "codex_manual_review_adapter_unsupported",
            "Only a verified codex-acp adapter is supported: "
            + ", ".join(sorted(_CODEX_SEND_PROMPT_BY_VERSION)) + ".",
        )
    send_prompt_source, send_prompt_patched = send_prompt
    reporter_v2 = install.package_version in ("2.0.1", "2.1.1")
    reviewer_source = _CODEX_AGENT_REVIEWER_2_SOURCE if reporter_v2 else _CODEX_AGENT_REVIEWER_SOURCE
    reviewer_manual = _CODEX_AGENT_REVIEWER_2_MANUAL if reporter_v2 else _CODEX_AGENT_REVIEWER_MANUAL
    package_dir = Path(install.package_json).parent
    if (
        Path(install.adapter_entry) != package_dir / "dist" / "index.js"
        or package_dir.name != "codex-acp"
        or package_dir.parent.name != "@agentclientprotocol"
        or package_dir.parents[1].name != "node_modules"
    ):
        raise _validation_error("codex_manual_review_adapter_unsupported", "The verified Codex ACP install layout is unsupported.")
    source = Path(install.adapter_entry).read_bytes()
    if (
        hashlib.sha256(source).hexdigest() != install.adapter_entry_sha256
        or source.count(reviewer_source) != 1
        or source.count(_CODEX_NEW_SESSION_SOURCE) != 1
        or source.count(_CODEX_SESSION_CONFIG_SOURCE) != 1
        or source.count(_CODEX_MCP_DEDUP_SOURCE) != 1
        or source.count(_CODEX_MCP_ENTRY_SOURCE) != 1
        or source.count(send_prompt_source) != 1
        or source.count(_CODEX_UPDATE_EVENT_SOURCE) != 1
        or source.count(_CODEX_RAW_RESPONSE_CASE_SOURCE) != 1
        or source.count(_CODEX_TURN_STARTED_SOURCE) != 1
        or source.count(_CODEX_ITEM_EVENT_SWITCH_SOURCE) != 1
        or source.count(_CODEX_COMPLETE_ITEM_EVENT_SWITCH_SOURCE) != 1
        or (reporter_v2 and (
            source.count(_CODEX_HISTORY_2_SOURCE) != 1
            or source.count(_CODEX_COMPACTION_2_SOURCE) != 1
            or source.count(_CODEX_HELPERS_2_SOURCE) != 1
        ))
        or (not reporter_v2 and (
            source.count(_CODEX_FUNCTION_CALL_SOURCE) != 1
            or source.count(_CODEX_RESPONSE_ITEM_TYPES_SOURCE) != 1
            or source.count(_CODEX_HISTORY_CASE_BLOCK_SOURCE) != 1
        ))
    ):
        raise _validation_error(
            "codex_manual_review_adapter_unsupported",
            "The verified codex-acp adapter call sites do not match the supported shape.",
        )
    patched = source.replace(reviewer_source, reviewer_manual, 1)
    patched = patched.replace(_CODEX_NEW_SESSION_SOURCE, _CODEX_NEW_SESSION_PATCHED, 1)
    patched = patched.replace(_CODEX_SESSION_CONFIG_SOURCE, _CODEX_SESSION_CONFIG_PATCHED, 1)
    patched = patched.replace(_CODEX_MCP_DEDUP_SOURCE, _CODEX_MCP_DEDUP_PATCHED, 1)
    patched = patched.replace(_CODEX_MCP_ENTRY_SOURCE, _CODEX_MCP_ENTRY_2_PATCHED if reporter_v2 else _CODEX_MCP_ENTRY_PATCHED, 1)
    patched = patched.replace(send_prompt_source, send_prompt_patched, 1)
    if reporter_v2:
        patched = patched.replace(_CODEX_HELPERS_2_SOURCE, _CODEX_CUSTOM_TOOL_HELPERS_2_INSERT + _CODEX_HELPERS_2_SOURCE, 1)
        patched = patched.replace(_CODEX_HISTORY_2_SOURCE, _CODEX_HISTORY_2_PATCHED, 1)
        patched = patched.replace(_CODEX_COMPACTION_2_SOURCE, _CODEX_COMPACTION_2_PATCHED, 1)
    else:
        patched = patched.replace(_CODEX_FUNCTION_CALL_SOURCE, _CODEX_CUSTOM_TOOL_HELPERS_INSERT + _CODEX_FUNCTION_CALL_SOURCE, 1)
    patched = patched.replace(_CODEX_UPDATE_EVENT_SOURCE, _CODEX_RAW_RESPONSE_METHOD_INSERT + _CODEX_UPDATE_EVENT_SOURCE, 1)
    patched = patched.replace(_CODEX_RAW_RESPONSE_CASE_SOURCE, _CODEX_RAW_RESPONSE_CASE_PATCHED, 1)
    patched = patched.replace(_CODEX_TURN_STARTED_SOURCE, _CODEX_TURN_STARTED_PATCHED, 1)
    patched = patched.replace(_CODEX_ITEM_EVENT_SWITCH_SOURCE, _CODEX_ITEM_EVENT_SWITCH_PATCHED, 1)
    patched = patched.replace(_CODEX_COMPLETE_ITEM_EVENT_SWITCH_SOURCE, _CODEX_COMPLETE_ITEM_EVENT_SWITCH_PATCHED, 1)
    if not reporter_v2:
        patched = patched.replace(_CODEX_RESPONSE_ITEM_TYPES_SOURCE, _CODEX_RESPONSE_ITEM_TYPES_PATCHED, 1)
        patched = patched.replace(_CODEX_HISTORY_CASE_BLOCK_SOURCE, _CODEX_HISTORY_CASE_BLOCK_PATCHED, 1)
    state_dir = registry.connection_state_dir(connection_id, "codex-acp", environ)
    private_package = state_dir / "manual-review-adapter"
    private_dist = private_package / "dist"
    if private_package.is_symlink() or private_dist.is_symlink():
        raise _validation_error("codex_manual_review_adapter_drifted", "The private adapter directory is a symlink.")
    private_dist.mkdir(mode=0o700, parents=True, exist_ok=True)
    private_dist.chmod(0o700)
    private_package.chmod(0o700)
    entry = private_dist / Path(install.adapter_entry).name
    _private_verified_file(entry, patched)
    _private_verified_file(private_package / "package.json", Path(install.package_json).read_bytes())
    # The bundled ESM's createRequire(import.meta.url) resolves the vendored
    # Codex module relative to the package. Pin that lookup to the original,
    # already verified install's node_modules tree without copying credentials.
    original_modules = package_dir.parents[1]
    private_modules = private_package / "node_modules"
    if private_modules.is_symlink() and _symlink_target(private_modules) != _resolved(original_modules):
        # A link left by an earlier install is repointed at the install
        # that just passed verification above, never treated as drift: it
        # names a tree this connection no longer launches from, and
        # refusing instead of repointing wedged the engine permanently.
        # Live (2026-09-22): this link still named the editor's own
        # registry copy from before Cortex ran its own installs, so every
        # Codex session and every model-catalog refresh failed closed with
        # ``codex_manual_review_adapter_drifted``.
        private_modules.unlink()
    if not private_modules.exists() and not private_modules.is_symlink():
        private_modules.symlink_to(original_modules, target_is_directory=True)
    if not private_modules.is_symlink() or private_modules.resolve() != original_modules.resolve():
        raise _validation_error("codex_manual_review_adapter_drifted", "The private adapter dependency path has changed.")
    original_xdg_open = Path(install.adapter_entry).parent / "xdg-open"
    if original_xdg_open.is_file():
        _private_verified_file(private_dist / "xdg-open", original_xdg_open.read_bytes(), mode=0o700)
    if hashlib.sha256(entry.read_bytes()).digest() != hashlib.sha256(patched).digest():
        raise _validation_error("codex_manual_review_adapter_drifted", "The private adapter content has changed.")
    return str(entry)


def _prepare_codex_runtime_wrapper(binary_path: str, connection_id: str, environ=None) -> str:
    """Create the Cortex-owned per-process Codex launcher.

    ``codex-acp`` 1.12.0 invokes ``CODEX_PATH app-server`` and exposes no
    argument/config override of its own.  The verified bundled Codex release
    supports ``agents.enabled=false`` alongside the observed
    ``features.multi_agent=false``, ``features.multi_agent_v2=false``,
    ``features.shell_tool=false`` and ``features.view_image=false``
    switches (the last two make Codex closer to Claude's own disallowed
    native Bash/Read -- see ``claude_adapter_acp``'s own
    ``CLAUDE_GATEWAY_ALTERNATIVES`` -- while native ``apply_patch`` stays on,
    since the installed Codex CLI 0.154.0 exposes no supported off switch for
    it: ``apply_patch`` is not even listed by ``features list``). App Server's
    later thread/start config may override these flags;
    the private adapter forces them again in every generated session config.
    A separate ``unified_exec`` feature was checked against the same
    installed Codex CLI 0.154.0 and found to have no effect either via
    ``-c features.unified_exec=false`` or ``--disable unified_exec``: both
    silently leave it enabled (``features list`` still reports it ``true``),
    so it is not set here -- there is currently no supported channel to turn
    it off, the same situation as ``apply_patch``.
    This launcher is an additional process-level guard while leaving the
    user's production ``CODEX_HOME`` and credentials untouched.
    The inner binary has already passed the install hash check before this is
    called.  The wrapper is recreated and byte-checked on every launch.
    """
    state_dir = registry.connection_state_dir(connection_id, "codex-acp", environ)
    state_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    wrapper = state_dir / "codex-acp-runtime"
    # The installed CLI merges an empty mcp_servers table with the native
    # account's config, so discover the effective names and disable each one.
    # Plugin-contributed servers have no config.toml transport and cannot be
    # disabled by a sparse mcp_servers.<name>.enabled override; disabling the
    # supported plugins feature excludes them before discovery. Only names and
    # enabled booleans are inspected; mcp list's credential-bearing JSON and
    # stderr are never printed or persisted.
    content = ('''#!/usr/bin/env python3
import json
import os
import re
import subprocess
import sys

BINARY = __BINARY__
BASE = ["-c", "agents.enabled=false", "-c", "features.multi_agent=false",
        "-c", "features.multi_agent_v2=false", "-c", "features.plugins=false",
        "-c", "features.hooks=false", "-c", "features.apps=false",
        "-c", "features.shell_tool=false", "-c", "features.view_image=false",
        "-c", "tools.experimental_request_user_input.enabled=false"]

def checked(args):
    response = subprocess.run([BINARY, *args], stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, check=False, timeout=20)
    if response.returncode:
        raise ValueError("Codex configuration check failed")
    return response.stdout.decode("utf-8")

def servers(args):
    data = json.loads(checked([*args, "mcp", "list", "--json"]))
    if not isinstance(data, list) or any(not isinstance(item, dict) or
       not isinstance(item.get("name"), str) or not item["name"] or
       not isinstance(item.get("enabled"), bool) for item in data):
        raise ValueError("Codex MCP inventory has an unsupported shape")
    names = [item["name"] for item in data]
    if len(names) != len(set(names)) or any(
        re.fullmatch(r"[A-Za-z0-9_-]+", name) is None for name in names
    ):
        raise ValueError("Codex MCP inventory has duplicate or unsupported names")
    return data

if sys.argv[1:] != ["app-server"]:
    sys.exit(64)
try:
    features = checked([*BASE, "features", "list"]).splitlines()
    observed = {parts[0]: parts[-1] for line in features
                if len(parts := line.split()) >= 3}
    if any(observed.get(name) != "false" for name in
           ("multi_agent", "multi_agent_v2", "plugins", "shell_tool", "view_image")):
        raise ValueError("Codex process features are not disabled")
    discovered = servers(BASE)
    overrides = [arg for item in discovered for arg in
                 ("-c", "mcp_servers." + item["name"] + ".enabled=false")]
    if any(item["enabled"] for item in servers([*BASE, *overrides])):
        raise ValueError("Native MCP servers remain enabled")
except (ValueError, UnicodeError, subprocess.SubprocessError, OSError):
    print("cortex codex native feature/MCP check failed", file=sys.stderr)
    sys.exit(65)
os.execv(BINARY, [BINARY, *BASE, *overrides, "app-server"])
'''.replace("__BINARY__", repr(binary_path))).encode("utf-8")
    if not wrapper.exists() or wrapper.read_bytes() != content:
        # A fixed temporary name would let two recovery/open paths clobber
        # each other's wrapper.  Keep the write private, flush it, and then
        # replace the final leaf atomically.
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=state_dir, prefix="codex-acp-runtime.",
            suffix=".tmp", delete=False,
        ) as temporary:
            temporary.write(content)
            temporary.flush()
            os.fsync(temporary.fileno())
            temporary_path = Path(temporary.name)
        temporary_path.chmod(0o700)
        os.replace(temporary_path, wrapper)
    wrapper.chmod(0o700)
    if (
        wrapper.is_symlink() or not wrapper.is_file()
        or wrapper.read_bytes() != content or not os.access(wrapper, os.X_OK)
    ):
        raise _validation_error(
            "codex_runtime_wrapper_drifted",
            "The Cortex-owned Codex runtime wrapper could not be verified after creation.",
            field="CODEX_PATH",
        )
    return str(wrapper)


def _validation_error(code, remediation, *, field=None):
    return PortableError(
        category=PortableErrorCategory.VALIDATION, code=code, remediation=remediation,
        retry_eligible=False, field=field,
    )


class ProcessObservation:
    STARTING = "starting"
    TRANSPORT_EOF = "transport_eof"
    TIMEOUT_ALIVE = "timeout_alive"
    PROCESS_EXITED = "process_exited"


# Engines are started from this thread only; it lives as long as the
# process (idle pool threads never exit), so a jailed engine dies with Cortex,
# never with the turn that happened to open it. See EngineProcess.start.
_ENGINE_SPAWNER = concurrent.futures.ThreadPoolExecutor(max_workers=1, thread_name_prefix="engine-spawner")


class EngineProcess:
    """One launched engine subprocess: env isolation, stderr capture, termination.

    ``engine_instance_id`` is a fresh random identifier per launch (never
    reused, never shared): it names this process's private log directory
    and is included in ``configure()``'s evidence_ref format.
    """

    def __init__(self, record, cwd, *, environ=None, install=None, broker_mcp=False,
                 project_root=None, channel_credentials=None, jail=None, engine_instructions=None):
        """``install`` -- a :class:`~.codex_install.CodexAcpInstall` for a
        ``codex-acp`` record, a :class:`~.claude_install.ClaudeAcpInstall`
        for a ``claude-agent-acp`` record, or ``None`` -- is re-verified
        fresh (see that module's own ``verify_install``) before anything is
        launched when supplied: ``record.launch_command`` must already
        equal that engine's own ``launch_command_for`` of the *verified*
        install (a mismatch is refused, never silently overridden). A
        codex-acp install's verified ``bundled_codex_binary`` is invoked
        through a Cortex-owned runtime wrapper pinned into this process's
        environment as ``CODEX_PATH`` (with ``NO_BROWSER=1``) via
        ``build_process_env``'s ``pinned_engine_vars``; the wrapper disables
        provider-native subagents per process. Claude-agent-acp pins no
        equivalent second binary. Omitting ``install`` launches exactly as before this
        integrity check existed (no verification) -- callers that need the
        pinned-install guarantee must pass it explicitly (see
        ``engine.install_resolution.resolve_verified_install``, the
        production entry point for both engines).

        ``jail`` -- ``{"scopes": [...], "alive": callable | None}`` -- runs
        the process in ``orchestration.sandbox.engine_jail``: ``cwd`` is
        read-only except ``scopes``, and every native tool the engine offers
        acts inside that box. ``alive`` is polled; once it returns false the
        process is terminated, so a revoked channel stops native effects.

        ``engine_instructions`` -- OpenCode only: the whole prompt its
        ``cortex`` agent runs with (``engine.opencode_adapter_acp``), mounted
        read-only into the jail from memory. OpenCode additionally requires a
        jail, a verified install and process isolation
        (:func:`_require_opencode_isolation`).
        """
        if record.launch_command is None:
            raise _validation_error(
                "engine_launch_command_missing",
                "This connection has no launch_command; an engine process is never "
                "guessed or discovered by any other means.",
                field="launch_command",
            )
        command = record.launch_command
        if not command[0].startswith("/"):
            raise _validation_error(
                "engine_launch_command_relative",
                "launch_command[0] must be an absolute path; a relative command is refused.",
                field="launch_command",
            )
        if not isinstance(cwd, str) or not cwd.startswith("/"):
            raise _validation_error(
                "engine_cwd_required",
                "An explicit absolute workspace directory is required to launch an engine process.",
                field="cwd",
            )
        opencode = record.engine == opencode_install.ENGINE
        if opencode:
            _require_opencode_isolation(jail, install)
        self.mcp_broker = None
        self.external_mcp_brokers = []
        self.engine_instance_id = "eirt_" + uuid.uuid4().hex[:12]
        self.logs = EngineLogs(self.engine_instance_id, environ)
        self._jail_dir = self._socket_dir = None
        self._jail_stack = contextlib.ExitStack()
        if jail is not None:
            self._jail_dir = Path(tempfile.mkdtemp(prefix="cortex-engine-"))
            self._socket_dir = self._jail_dir / "sockets"
            self._socket_dir.mkdir(mode=0o700)
            (self._jail_dir / "tmp").mkdir(mode=0o700)
        if broker_mcp:
            if not project_root or channel_credentials is None:
                raise _validation_error(
                    "broker_mcp_credentials_missing",
                    "Brokered Cortex MCP launch requires channel credentials.",
                )
            server_script = str(Path(__file__).resolve().parents[2] / "cortex.py")
            self.mcp_broker = McpBroker(
                channel_credentials, python_executable=sys.executable, server_script=server_script,
                observe=self.logs.observe, socket_dir=self._socket_dir, stderr_log=self.logs.stderr,
            )
        pinned_engine_vars = None
        adapter_entry_override = None
        if install is not None:
            install_module = _INSTALL_MODULE_BY_ENGINE.get(record.engine)
            if install_module is None:
                raise _validation_error(
                    "engine_install_not_applicable",
                    f"An install record is not meaningful for engine {record.engine!r}.",
                    field="install",
                )
            verified_install = install_module.verify_install(install, environ)
            expected_command = install_module.launch_command_for(verified_install)
            if tuple(command) != tuple(expected_command):
                raise _validation_error(
                    "engine_launch_command_mismatch",
                    "record.launch_command does not match this engine's own launch_command_for() of the "
                    "verified install; a launch command is never silently substituted.",
                    field="launch_command",
                )
            if opencode:
                from .opencode_adapter_acp import (
                    CATALOG_PROBE_PROMPT, PROMPT_FILE_NAME, owner_provider_config, process_environment,
                )

                opencode_prompt_path = str(self._jail_dir / "tmp" / PROMPT_FILE_NAME)
                opencode_prompt = engine_instructions or CATALOG_PROBE_PROMPT
                # Carry only provider definitions from the hidden owner
                # configuration. OpenCode reads its own native logins in place.
                owner_env = dict(os.environ if environ is None else environ)
                owner_providers = owner_provider_config(
                    owner_env.get("HOME"), str(self._jail_dir / "tmp"), owner_env,
                )
                self.logs.observe("opencode_owner_providers", imported=list(owner_providers.imported),
                                  skipped=[list(item) for item in owner_providers.skipped])
                pinned_engine_vars = process_environment(opencode_prompt_path, owner_providers.config)
            if record.engine == "codex-acp":
                adapter_entry_override = _prepare_codex_manual_review_adapter(
                    verified_install, record.id, environ,
                )
                pinned_engine_vars = {
                    # codex-acp 1.12.0 supplies no child-config/argv hook;
                    # route through a Cortex-owned launcher that injects the
                    # agents.enabled and both multi_agent feature overrides while the
                    # real production CODEX_HOME remains untouched.
                    "CODEX_PATH": _prepare_codex_runtime_wrapper(
                        verified_install.bundled_codex_binary, record.id, environ,
                    ),
                    "NO_BROWSER": "1",
                }

        base_env = dict(os.environ if environ is None else environ)
        # Pass this connection's own resolution ``environ`` through
        # explicitly (never rely on ``os.environ`` having been
        # monkeypatched by the time this runs): ``build_process_env``
        # re-verifies the connection registry against exactly this mapping
        # (default ``None`` -> real ``os.environ``, for backward
        # compatibility with every other existing caller).
        process_env = build_process_env(
            record, base_env, channel_credentials=None, environ=environ, pinned_engine_vars=pinned_engine_vars,
        )
        # Native engine tools frequently invoke Python through a terminal
        # request (for example ``python3 -m unittest``).  They run in the
        # user's exact project root, so bytecode writes would look like
        # coordinator mutations and invalidate the candidate revision.  Pin
        # this process-wide Python setting for every ACP engine launch; it is
        # harmless for the Node-based Claude adapter and keeps the launch
        # contract independent of the caller's ambient environment.
        process_env["PYTHONDONTWRITEBYTECODE"] = "1"
        if opencode:
            # OpenCode writes $XDG_STATE_HOME/opencode (recent models, prompt
            # history) and fails to start when it cannot. The owner's HOME is
            # read-only in the jail, so this process gets a private,
            # disposable one in its own scratch directory; its credentials
            # and sessions stay in XDG_DATA_HOME.
            state_home = self._jail_dir / "tmp" / "xdg-state"
            state_home.mkdir(mode=0o700, exist_ok=True)
            process_env["XDG_STATE_HOME"] = str(state_home)
        # ssh in the engine's shell uses the owner's own ssh-agent, like
        # their terminal (see orchestration.ssh_askpass.host_agent_env).
        from ..orchestration.ssh_askpass import host_agent_env
        process_env.update(host_agent_env(base_env))
        if record.engine == "claude-agent-acp":
            # Claude aborts an MCP tools/call after 30 min (stdio) / 5 min
            # (http) of silence and ~28 h overall. A long command_run is
            # silent by nature: disable the idle bound and raise the hard one
            # to Claude's own ceiling (2^31-1 ms, ~24.8 days).
            process_env["CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT"] = "0"
            process_env["MCP_TOOL_TIMEOUT"] = "2147483647"
        from ..observability.wire_trace import get_trace
        trace = get_trace()
        if trace is not None:
            # Credential references have now been resolved for this launch.
            # Teach redaction their literals without ever tracing the environment.
            trace.learn_environment(process_env)
        self.state = ProcessObservation.STARTING
        self._exit_code = None
        self._exited_event = threading.Event()
        try:
            launch_command = list(command)
            if adapter_entry_override is not None:
                launch_command[1] = adapter_entry_override
            self.logs.observe(ProcessObservation.STARTING, command=list(launch_command), cwd=cwd)
            fds = ()
            if jail is not None:
                from ..orchestration import ssh_askpass
                from ..orchestration.sandbox import engine_jail
                from ..connections.environment import engine_state_dir
                state = engine_state_dir(record.engine, process_env)
                isolation = {}
                sealed = []
                if opencode:
                    from .opencode_adapter_acp import USER_CONFIG_RELATIVE

                    home = process_env.get("HOME")
                    isolation = dict(
                        isolate_processes=True,
                        masked=[str(Path(home).joinpath(*USER_CONFIG_RELATIVE))] if home else [],
                        data_files={opencode_prompt_path: opencode_prompt.encode("utf-8"), **owner_providers.files},
                    )
                # Opt-in: ssh in the native shell asks for a password in a
                # desktop dialog (see orchestration.ssh_askpass).
                askpass_dir = self._jail_dir / "askpass"
                askpass_env = ssh_askpass.prepare(base_env, askpass_dir)
                process_env.update(askpass_env)
                self.logs.observe("ssh_askpass", enabled=bool(askpass_env))
                launch_command, fds = self._jail_stack.enter_context(engine_jail(
                    cwd, self._jail_dir / "tmp", launch_command, jail["scopes"], env=process_env,
                    state_dirs={state: record.engine}, writable=[self._socket_dir],
                    readonly=jail.get("readonly") or (), sealed=[*sealed, *([askpass_dir] if askpass_env else ())],
                    **isolation,
                ))
                process_env = {"PATH": "/usr/bin:/bin"}
            # Spawned from one process-lifetime thread: the jail's
            # --die-with-parent is PR_SET_PDEATHSIG, which the kernel fires
            # when the *spawning thread* exits. Spawned from a turn's thread,
            # a coordinator was SIGKILLed the moment its turn ended (cancel,
            # completion) and the next message hit a dead session.
            self.process = _ENGINE_SPAWNER.submit(
                subprocess.Popen,
                launch_command,
                cwd=cwd,
                env=process_env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
                pass_fds=fds,
            ).result()
        except Exception:
            if self.mcp_broker is not None:
                self.mcp_broker.close()
            self._release_jail()
            raise
        if self.mcp_broker is not None:
            # Before any session/new can hand the engine its relay entry:
            # only this engine's own process tree may use the relay token.
            self.mcp_broker.bind_peer_root(self.process.pid)
        from . import process_identity
        self._group_identity = process_identity.capture(self.process.pid, self.process.pid)
        self._termination_lock = threading.Lock()
        self.state = "initialized"

        self._stderr_thread = threading.Thread(target=self._drain_stderr, name="engine-stderr", daemon=True)
        self._stderr_thread.start()
        self._wait_thread = threading.Thread(target=self._wait_for_exit, name="engine-wait", daemon=True)
        self._wait_thread.start()
        if jail is not None and jail.get("alive") is not None:
            threading.Thread(target=self._watch_authority, args=(jail["alive"],),
                             name="engine-authority", daemon=True).start()

    def _watch_authority(self, alive) -> None:
        # Native tools act without a per-call gateway check, so authority is
        # enforced on the process: a revoked channel ends it.
        while not self._exited_event.wait(1.0):
            try:
                current = alive()
            except Exception:
                # An unexpected error probing authority is a transient infra
                # hiccup, never proof of revocation: keep the live engine and
                # re-check next tick rather than killing it (which would
                # surface as a spurious transport_eof). A genuine revocation
                # is reported by alive() returning False, cleanly.
                continue
            if not current:
                self.logs.observe("jail_authority_revoked")
                self.terminate()
                return

    def _release_jail(self) -> None:
        self._jail_stack.close()
        if self._jail_dir is not None:
            shutil.rmtree(self._jail_dir, ignore_errors=True)

    def _drain_stderr(self) -> None:
        stream = self.process.stderr
        try:
            while True:
                line = stream.readline()
                if not line:
                    break
                try:
                    text = line.decode("utf-8", errors="replace")
                except Exception:
                    text = repr(line)
                self.logs.stderr_line(text)
        except (OSError, ValueError):
            pass

    def _wait_for_exit(self) -> None:
        code = self.process.wait()
        self._exit_code = code
        self.state = "process_exited"
        self.logs.observe(ProcessObservation.PROCESS_EXITED, exit_code=code)
        self._exited_event.set()

    def alive(self) -> bool:
        return self.process.poll() is None

    @property
    def exit_code(self):
        return self._exit_code

    def observe_timeout_alive(self) -> None:
        """A call timed out; this does not by itself prove the process is
        dead, so capacity is not released here -- only observed."""
        self.logs.observe(ProcessObservation.TIMEOUT_ALIVE, alive=self.alive())

    def observe_transport_eof(self) -> None:
        self.logs.observe(ProcessObservation.TRANSPORT_EOF, alive=self.alive())

    def wait_exited(self, timeout=None) -> bool:
        """Confirm leader reaping and absence of active owned group members."""
        import time
        from . import process_identity

        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            identity = self._group_identity
            if (self._exited_event.is_set() and identity is not None
                    and process_identity.probe_group(identity) == "absent"):
                return True
            if deadline is not None and time.monotonic() >= deadline:
                return False
            time.sleep(.02)

    def broker_external_mcp(self, credentials, config):
        from .mcp_bridge import relay_launch
        allowed=config.get('allowed_tools')
        if not isinstance(allowed,list) or not allowed:
            raise _validation_error('external_mcp_explicit_tool_grants_required',
                                    'Configure exact allowed_tools in the owner-only Cortex MCP registry.')
        broker=McpBroker(credentials,python_executable=sys.executable,
                         server_script=str(Path(__file__).with_name('external_proxy.py')),
                         server_config=config,observe=self.logs.observe,socket_dir=self._socket_dir,
                         stderr_log=self.logs.stderr,peer_root=self.process.pid)
        self.external_mcp_brokers.append(broker)
        args,env=relay_launch(broker.endpoint)
        return dict(name=config['name'],command=sys.executable,args=args,env=env)

    def terminate(self, *, grace: float = TERMINATE_GRACE_SECONDS) -> None:
        """Bounded graceful shutdown/escalation of the exact launched group."""
        from . import process_identity

        with self._termination_lock:
            for broker in self.external_mcp_brokers:
                broker.close()
            self.external_mcp_brokers.clear()
            try:
                self.process.stdin.close()
            except (OSError, ValueError):
                pass
            for sig in (signal.SIGTERM, signal.SIGKILL):
                identity = self._group_identity
                if identity is not None and process_identity.probe_group(identity) == "alive":
                    try:
                        os.killpg(identity.pgid, sig)
                    except (ProcessLookupError, PermissionError, OSError):
                        pass
                if self.wait_exited(grace):
                    return

    def release(self, *, grace: float = TERMINATE_GRACE_SECONDS) -> None:
        """Terminate and reap; capacity is only freed once exit is observed."""
        self.terminate(grace=grace)
        exited = self.wait_exited(0)
        self.state = "released" if exited else "timeout_alive"
        if self.mcp_broker is not None:
            self.mcp_broker.close()
        if exited:
            self._release_jail()
