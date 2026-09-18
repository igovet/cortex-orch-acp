"""Pure ACP v1 message shaping and validation (no I/O, no threading).

Only ``protocolVersion`` 1 is supported. Advertised capabilities are
intentionally minimal and accurate: (T018) ``loadSession`` true --
``session/load`` is implemented, *display-only* (see
``acp.replay_store.replay_session``/``acp.agent.Agent._session_load``); no
image/audio prompt content (still unadvertised, still rejected -32602);
``embeddedContext`` true -- an ``EmbeddedResource`` (``resource``) prompt
content block is now accepted and durably stored/rejected with truthful
availability metadata (see ``acp.replay_store.resolve_prompt_blocks``); no
HTTP/SSE MCP transport; no other session lifecycle features
(resume/fork/close/list/set_mode -- see ``docs/acp/input-history-and-replay.md``
for why ``session/resume`` stays unsupported); and no client-request code
path (no ``fs/*``, ``terminal/*``, or ``request_permission`` is ever emitted
-- see ``agent.Agent``, which never constructs one).

``session/set_config_option`` (T010) *is* supported: this module provides
``validate_set_config_option_params``/``set_config_option_response``/
``config_option_update``/``available_commands_update`` for it, and it is
deliberately absent from ``UNSUPPORTED_METHODS`` below -- see
``agent.Agent._session_set_config_option`` and ``acp.config_options``/
``acp.commands`` for the truthful configuration-option/selector logic
itself (this module only shapes/validates the wire messages).
"""
from __future__ import annotations

PROTOCOL_VERSION = 1

# Methods this agent recognizes as valid ACP method names but does not
# support in T009 (no invented "standard" methods; every one of these is a
# real ACP v1 method this agent simply doesn't advertise/implement yet).
UNSUPPORTED_METHODS = frozenset({
    "session/resume", "session/set_mode",
    "session/list", "session/fork", "session/close",
    "authenticate",
})
# session/load (T018) is deliberately absent from UNSUPPORTED_METHODS above
# -- see agent.Agent._session_load / replay_store.replay_session -- exactly
# like session/set_config_option (T010) before it.


class RpcError(Exception):
    """One JSON-RPC error to be sent back to the client, never echoing input."""

    def __init__(self, code, message, data=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data


def error_response(request_id, error: RpcError):
    payload = dict(code=error.code, message=error.message)
    if error.data is not None:
        payload["data"] = error.data
    return dict(jsonrpc="2.0", id=request_id, error=payload)


def result_response(request_id, result):
    return dict(jsonrpc="2.0", id=request_id, result=result)


def notification(method, params):
    return dict(jsonrpc="2.0", method=method, params=params)


def initialize_result(agent_version):
    """Accurate capability negotiation result for ``initialize``.

    T018: ``loadSession``/``embeddedContext`` are now true, matching the
    implemented (display-only) ``session/load`` and ``resource`` content
    block support; ``image``/``audio`` stay false (still unimplemented,
    still rejected -32602)."""
    return dict(
        protocolVersion=PROTOCOL_VERSION,
        agentCapabilities=dict(
            loadSession=True,
            promptCapabilities=dict(image=False, audio=False, embeddedContext=True),
            mcpCapabilities=dict(http=False, sse=False),
        ),
        authMethods=[],
        agentInfo=dict(name="cortex", version=agent_version),
    )


def validate_initialize_params(params):
    if not isinstance(params, dict) or not isinstance(params.get("protocolVersion"), int):
        raise RpcError(-32602, "initialize requires an integer protocolVersion.")
    return params


def validate_session_new_params(params):
    if not isinstance(params, dict) or not isinstance(params.get("cwd"), str) or not params["cwd"]:
        raise RpcError(-32602, "session/new requires a non-empty string cwd.")
    mcp_servers = params.get("mcpServers", [])
    if mcp_servers is None:
        mcp_servers = []
    if not isinstance(mcp_servers, list):
        raise RpcError(-32602, "mcpServers must be an array.")
    for entry in mcp_servers:
        if not isinstance(entry, dict) or not isinstance(entry.get("name"), str) or not entry["name"]:
            raise RpcError(-32602, "Each mcpServers entry needs a non-empty string name.")
        if "command" in entry and not isinstance(entry["command"], str):
            raise RpcError(-32602, "An mcpServers entry's command must be a string.")
    return params["cwd"], mcp_servers


def strip_mcp_servers(servers):
    """Shape-only inert metadata: never a value from ``env``/``headers``."""
    stripped = []
    for entry in servers:
        clean = dict(name=entry.get("name"), type=entry.get("type", "stdio"))
        env = entry.get("env")
        if isinstance(env, list):
            clean["env_names"] = sorted({
                item.get("name") for item in env
                if isinstance(item, dict) and isinstance(item.get("name"), str)
            })
        headers = entry.get("headers")
        if isinstance(headers, list):
            clean["header_names"] = sorted({
                item.get("name") for item in headers
                if isinstance(item, dict) and isinstance(item.get("name"), str)
            })
        stripped.append(clean)
    return stripped


def validate_session_prompt_params(params):
    if not isinstance(params, dict) or not isinstance(params.get("sessionId"), str) or not params["sessionId"]:
        raise RpcError(-32602, "session/prompt requires a non-empty string sessionId.")
    blocks = params.get("prompt")
    text = content_blocks_to_text(blocks)
    return params["sessionId"], text, blocks


def validate_session_cancel_params(params):
    if not isinstance(params, dict) or not isinstance(params.get("sessionId"), str) or not params["sessionId"]:
        raise RpcError(-32602, "session/cancel requires a non-empty string sessionId.")
    return params["sessionId"]


def validate_set_config_option_params(params):
    """Return ``(sessionId, configId, kind, value)``; ``kind`` is ``"boolean"``
    or ``"value_id"`` (the pinned schema's ``SetSessionConfigOptionRequest``
    ``anyOf``; a bare ``value`` with no ``type`` defaults to ``value_id``)."""
    if not isinstance(params, dict) or not isinstance(params.get("sessionId"), str) or not params["sessionId"]:
        raise RpcError(-32602, "session/set_config_option requires a non-empty string sessionId.")
    if not isinstance(params.get("configId"), str) or not params["configId"]:
        raise RpcError(-32602, "session/set_config_option requires a non-empty string configId.")
    kind = params.get("type", "value_id")
    if kind == "boolean":
        value = params.get("value")
        if not isinstance(value, bool):
            raise RpcError(-32602, "A type: 'boolean' set_config_option request requires a boolean value.")
        return params["sessionId"], params["configId"], "boolean", value
    value = params.get("value")
    if not isinstance(value, str) or not value:
        raise RpcError(-32602, "set_config_option requires a non-empty string value (or type: 'boolean').")
    return params["sessionId"], params["configId"], "value_id", value


def set_config_option_response(config_options):
    return dict(configOptions=list(config_options))


def config_option_update(session_id, config_options):
    return notification("session/update", dict(
        sessionId=session_id,
        update=dict(sessionUpdate="config_option_update", configOptions=list(config_options)),
    ))


def available_commands_update(session_id, available_commands):
    return notification("session/update", dict(
        sessionId=session_id,
        update=dict(sessionUpdate="available_commands_update", availableCommands=list(available_commands)),
    ))


def content_blocks_to_text(blocks):
    """Concatenate the accepted content block types into exact user text.

    Every agent MUST accept ``text`` and ``resource_link`` blocks even with
    no optional prompt capability advertised. T018: an ``EmbeddedResource``
    (``resource``) block is now also accepted -- this agent truthfully
    advertises ``promptCapabilities.embeddedContext=true`` (see
    ``initialize_result``) -- but contributes only a placeholder marker to
    ``text`` (never the resource body itself, which is a stored/rejected
    *attachment*, not user-typed text; see
    ``acp.replay_store.resolve_prompt_blocks``, invoked separately over the
    same raw ``blocks`` by ``acp.agent``). ``image``/``audio`` remain
    rejected with ``-32602`` (still unadvertised).
    """
    if not isinstance(blocks, list) or not blocks:
        raise RpcError(-32602, "prompt must be a non-empty array of content blocks.")
    parts = []
    for block in blocks:
        if not isinstance(block, dict) or not isinstance(block.get("type"), str):
            raise RpcError(-32602, "Each prompt content block needs a string type.")
        kind = block["type"]
        if kind == "text":
            text = block.get("text")
            if not isinstance(text, str):
                raise RpcError(-32602, "A text content block needs a string text field.")
            parts.append(text)
        elif kind == "resource_link":
            name, uri = block.get("name"), block.get("uri")
            if not isinstance(name, str) or not isinstance(uri, str):
                raise RpcError(-32602, "A resource_link content block needs string name and uri fields.")
            parts.append(f"[resource_link: {name} ({uri})]")
        elif kind == "resource":
            resource = block.get("resource")
            if not isinstance(resource, dict) or not isinstance(resource.get("uri"), str):
                raise RpcError(-32602, "A resource content block needs an embedded resource object with a string uri field.")
            if not isinstance(resource.get("text"), str) and not isinstance(resource.get("blob"), str):
                raise RpcError(-32602, "A resource content block needs an embedded 'text' or 'blob' field.")
            parts.append(f"[resource: {resource['uri']}]")
        else:
            raise RpcError(
                -32602,
                f"Content block type '{kind}' is not accepted; only text, "
                "resource_link, and resource are supported (no image/audio "
                "prompt capability is advertised).",
            )
    return "\n".join(parts)


def agent_message_chunk(session_id, text):
    return notification("session/update", dict(
        sessionId=session_id,
        update=dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=text)),
    ))


def user_message_chunk(session_id, text):
    return notification("session/update", dict(
        sessionId=session_id,
        update=dict(sessionUpdate="user_message_chunk", content=dict(type="text", text=text)),
    ))


def _tool_call_text_content(text):
    return [dict(type="content", content=dict(type="text", text=text))]


def tool_call(session_id, tool_call_id, title, kind, status, *, text=None):
    """A brand-new ``ToolCall`` (T035 worker-assignment cards): one per
    delegated assignment, keyed by a caller-chosen stable ``tool_call_id``
    (see ``acp.orchestrating_runtime``'s per-assignment id derivation) so
    every subsequent :func:`tool_call_update` for the same assignment
    targets this same card instead of creating a new one. ``status`` is one
    of the pinned schema's four ``ToolCallStatus`` values (``pending``,
    ``in_progress``, ``completed``, ``failed`` -- there is no ``cancelled``;
    see the caller for how a cancelled assignment maps onto ``failed``).
    ``text``, when given, becomes this card's initial bounded content."""
    update = dict(sessionUpdate="tool_call", toolCallId=tool_call_id, title=title, kind=kind, status=status)
    if text:
        update["content"] = _tool_call_text_content(text)
    return notification("session/update", dict(sessionId=session_id, update=update))


def tool_call_update(session_id, tool_call_id, *, status=None, title=None, text=None):
    """A ``ToolCallUpdate`` for an already-created card (see :func:`tool_call`
    above) -- only the fields actually changing are included, per the pinned
    schema's own "all fields except the tool call ID are optional" contract."""
    update = dict(sessionUpdate="tool_call_update", toolCallId=tool_call_id)
    if status is not None:
        update["status"] = status
    if title is not None:
        update["title"] = title
    if text is not None:
        update["content"] = _tool_call_text_content(text)
    return notification("session/update", dict(sessionId=session_id, update=update))


def validate_load_session_params(params):
    """(T018) Validate a ``LoadSessionRequest`` per the pinned schema:
    ``sessionId``, ``cwd`` (absolute existing canonical directory -- checked
    by the caller, see ``agent.Agent._session_load``), and ``mcpServers``
    (an array; stripped/ignored the same as ``session/new``'s -- a loaded
    session's display-only replay never connects to any of them)."""
    if not isinstance(params, dict) or not isinstance(params.get("sessionId"), str) or not params["sessionId"]:
        raise RpcError(-32602, "session/load requires a non-empty string sessionId.")
    if not isinstance(params.get("cwd"), str) or not params["cwd"]:
        raise RpcError(-32602, "session/load requires a non-empty string cwd.")
    mcp_servers = params.get("mcpServers")
    if mcp_servers is None:
        mcp_servers = []
    if not isinstance(mcp_servers, list):
        raise RpcError(-32602, "mcpServers must be an array.")
    return params["sessionId"], params["cwd"], mcp_servers


def load_session_response(config_options=None):
    """T035 FINDING-08 fix: ``configOptions`` mirrors ``session/new``'s
    -- the reloaded session's own selection surface (see
    ``agent.Agent._session_load``/``_config_options_for_session``), not
    always ``None``. ``modes`` stays ``None``: ``session/new`` never
    reports it either (Cortex has no ACP "modes" concept, only
    ``configOptions``), so there is nothing session/load should add."""
    return dict(modes=None, configOptions=config_options if config_options else None)


def prompt_result(stop_reason):
    return dict(stopReason=stop_reason)
