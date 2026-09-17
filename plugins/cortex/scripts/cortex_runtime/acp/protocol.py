"""Pure ACP v1 message shaping and validation (no I/O, no threading).

Only ``protocolVersion`` 1 is supported. Advertised capabilities are
intentionally minimal and accurate: ``loadSession`` false, no image/audio/
embedded-context prompt content, no HTTP/SSE MCP transport, no session
lifecycle features (load/resume/fork/close/list/set_mode/
set_config_option), and no client-request code path (no ``fs/*``,
``terminal/*``, or ``request_permission`` is ever emitted -- see
``agent.Agent``, which never constructs one).
"""
from __future__ import annotations

PROTOCOL_VERSION = 1

# Methods this agent recognizes as valid ACP method names but does not
# support in T009 (no invented "standard" methods; every one of these is a
# real ACP v1 method this agent simply doesn't advertise/implement yet).
UNSUPPORTED_METHODS = frozenset({
    "session/load", "session/resume", "session/set_mode",
    "session/set_config_option", "session/list", "session/fork", "session/close",
    "authenticate",
})


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
    """Accurate, minimal capability negotiation result for ``initialize``."""
    return dict(
        protocolVersion=PROTOCOL_VERSION,
        agentCapabilities=dict(
            loadSession=False,
            promptCapabilities=dict(image=False, audio=False, embeddedContext=False),
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
    return params["sessionId"], content_blocks_to_text(params.get("prompt"))


def validate_session_cancel_params(params):
    if not isinstance(params, dict) or not isinstance(params.get("sessionId"), str) or not params["sessionId"]:
        raise RpcError(-32602, "session/cancel requires a non-empty string sessionId.")
    return params["sessionId"]


def content_blocks_to_text(blocks):
    """Concatenate only the two baseline content block types into text.

    Every agent MUST accept ``text`` and ``resource_link`` blocks even with
    no optional prompt capability advertised; any other block type is
    rejected with ``-32602`` since this agent's ``promptCapabilities`` are
    all false (see ``initialize_result``).
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
        else:
            raise RpcError(
                -32602,
                f"Content block type '{kind}' is not accepted; only text and "
                "resource_link are supported (no image/audio/embeddedContext "
                "prompt capability is advertised).",
            )
    return "\n".join(parts)


def agent_message_chunk(session_id, text):
    return notification("session/update", dict(
        sessionId=session_id,
        update=dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=text)),
    ))


def prompt_result(stop_reason):
    return dict(stopReason=stop_reason)
