"""OpenCode Code Mode tool calls, normalized to the common MCP card shape.

OpenCode 2.x reaches a session's MCP servers -- Cortex's gateway included --
only from inside its ``execute`` (Code Mode) tool, as
``tools.<server>.<tool>(input)`` (docs/features/opencode-engine/index.md). Its ACP
updates therefore describe the wrapper, not the MCP call. The shapes observed
in a real OpenCode session are:

- ``tool_call``: ``title "execute"``, ``kind "other"``, ``status "pending"``,
  ``rawInput {}``;
- ``tool_call_update``: ``status "in_progress"``, ``rawInput {"code": <JS>}``
  and no title;
- ``tool_call_update``: ``status "completed"`` (also when the inner call
  failed), ``rawOutput {"metadata": {"toolCalls": [{"tool": "<server>.<tool>"
  or "<name>", "input": {...}, "status": ...}], "truncated": ..., "error"?}}``
  and the returned text only in ACP ``content``.

Codex and Claude report the same MCP call under its own name and arguments,
and every presenter (``acp.orchestrating_runtime._human_tool_title``,
``_worker_tool_details``, the Paseo plugin's ``presentToolSemantics`` and
``gatewayPresentation``) keys on those shapes. :class:`CodeModeToolCalls`
rewrites the wrapper into Codex's spelling -- title ``mcp.<server>.<tool>``,
rawInput ``{"server", "tool", "arguments", "code"}`` and rawOutput
``{"result": {"content": [...]}, "metadata": ...}`` -- so the rest of the
runtime and every host render one format. The original JavaScript and
OpenCode's own metadata are kept inside those fields.

The inner call list at completion is authoritative. While the call runs,
the first ``tools.<server>.<tool>(`` in the code is a provisional name
(free-form JavaScript, so only a hint). A call whose inner operation is not
known yet is titled ``Code Mode`` rather than ``execute``, which presenters
would otherwise take for a shell command. An inner ``status "error"`` or
``metadata.error: true`` turns the completed wrapper into ``failed``: the
card then matches what the same MCP error looks like from the other engines.
"""
from __future__ import annotations

import collections
import re
import threading
from collections.abc import Mapping
from typing import Any

CODE_MODE_TITLE = "execute"
UNRESOLVED_TITLE = "Code Mode"
MAX_TRACKED_CALLS = 1024

_CODE_CALL = re.compile(r"\btools\s*\.\s*([A-Za-z_$][\w$]*)\s*\.\s*([A-Za-z_$][\w$]*)\s*\(")
_TERMINAL_STATUSES = frozenset({"completed", "failed"})


def _inner_calls(raw_output: Any) -> list[Mapping]:
    metadata = raw_output.get("metadata") if isinstance(raw_output, Mapping) else None
    calls = metadata.get("toolCalls") if isinstance(metadata, Mapping) else None
    if not isinstance(calls, list):
        return []
    return [call for call in calls
            if isinstance(call, Mapping) and isinstance(call.get("tool"), str) and call["tool"].strip()]


def _operation(tool_name: str) -> tuple[str | None, str]:
    server, separator, tool = tool_name.strip().partition(".")
    return (server, tool) if separator and server and tool else (None, tool_name.strip())


def _primary_call(calls: list[Mapping]) -> Mapping | None:
    """The call a card is named after: the first MCP (``server.tool``) call,
    else the first call (a top-level Code Mode helper such as ``search``)."""
    for call in calls:
        if _operation(call["tool"])[0] is not None:
            return call
    return calls[0] if calls else None


def _content_text_blocks(content: Any) -> list[dict]:
    blocks = []
    for item in content if isinstance(content, list) else ():
        inner = item.get("content") if isinstance(item, Mapping) and item.get("type") == "content" else None
        if isinstance(inner, Mapping) and inner.get("type") == "text" and isinstance(inner.get("text"), str):
            blocks.append({"type": "text", "text": inner["text"]})
    return blocks


def _is_code_mode_start(update: Mapping) -> bool:
    title = update.get("title")
    return (isinstance(title, str) and title.strip() == CODE_MODE_TITLE
            and update.get("kind") in (None, "other"))


class CodeModeToolCalls:
    """Per-engine-instance normalizer; state is keyed by the exact ACP
    ``(wire session, toolCallId)`` pair, bounded, and dropped when the call
    ends or its session is cleared."""

    def __init__(self, max_tracked: int = MAX_TRACKED_CALLS):
        self._lock = threading.Lock()
        self._calls: collections.OrderedDict[tuple[str, str], dict] = collections.OrderedDict()
        self._max_tracked = max_tracked

    def clear_session(self, wire_session_id: str | None) -> None:
        if wire_session_id is None:
            return
        with self._lock:
            for key in [key for key in self._calls if key[0] == wire_session_id]:
                self._calls.pop(key, None)

    def normalize(self, wire_session_id: str, update: Mapping) -> Mapping:
        """Return ``update`` unchanged unless it belongs to a Code Mode call;
        otherwise a normalized copy (see the module docstring)."""
        tool_call_id = update.get("toolCallId")
        if not isinstance(tool_call_id, str):
            return update
        key = (wire_session_id, tool_call_id)
        with self._lock:
            state = self._calls.get(key)
            if state is None:
                if not _is_code_mode_start(update):
                    return update
                state = {"code": None, "operation": None, "arguments": None,
                         "sent_title": None, "sent_input": None}
                self._calls[key] = state
                while len(self._calls) > self._max_tracked:
                    self._calls.popitem(last=False)
            elif isinstance(update.get("title"), str) and update["title"].strip() != CODE_MODE_TITLE:
                # The engine named the call itself: nothing left to derive.
                self._calls.pop(key, None)
                return update
            normalized = self._normalize_locked(state, update)
            if normalized.get("status") in _TERMINAL_STATUSES:
                self._calls.pop(key, None)
            return normalized

    @staticmethod
    def _normalize_locked(state: dict, update: Mapping) -> dict:
        normalized = dict(update)
        raw_input = update.get("rawInput")
        if isinstance(raw_input, Mapping) and isinstance(raw_input.get("code"), str):
            state["code"] = raw_input["code"]
            if state["arguments"] is None:
                match = _CODE_CALL.search(state["code"])
                if match:
                    state["operation"] = (match.group(1), match.group(2))
        raw_output = update.get("rawOutput")
        calls = _inner_calls(raw_output)
        primary = _primary_call(calls)
        if primary is not None:
            state["operation"] = _operation(primary["tool"])
            state["arguments"] = dict(primary["input"]) if isinstance(primary.get("input"), Mapping) else {}

        failed = update.get("status") == "failed"
        if update.get("status") == "completed" and isinstance(raw_output, Mapping):
            metadata = raw_output.get("metadata")
            failed = ((isinstance(metadata, Mapping) and metadata.get("error") is True)
                      or any(call.get("status") == "error" for call in calls))
            if failed:
                normalized["status"] = "failed"

        operation = state["operation"]
        if operation is None:
            title = UNRESOLVED_TITLE
        else:
            server, tool = operation
            title = f"mcp.{server}.{tool}" if server else tool
        normalized.pop("title", None)
        if title != state["sent_title"] or normalized.get("status") in _TERMINAL_STATUSES:
            normalized["title"] = title
            state["sent_title"] = title

        if operation is not None:
            envelope: dict[str, Any] = {}
            if operation[0] is not None:
                envelope["server"] = operation[0]
            envelope["tool"] = operation[1]
            if state["arguments"] is not None:
                envelope["arguments"] = state["arguments"]
            if state["code"] is not None:
                envelope["code"] = state["code"]
            if envelope != state["sent_input"]:
                normalized["rawInput"] = envelope
                state["sent_input"] = envelope
            else:
                normalized.pop("rawInput", None)

        text_blocks = _content_text_blocks(update.get("content"))
        if text_blocks and (raw_output is None or isinstance(raw_output, Mapping)):
            output: dict[str, Any] = dict(raw_output) if isinstance(raw_output, Mapping) else {}
            output.setdefault("result", {"content": text_blocks})
            if failed:
                output.setdefault("error", "\n".join(block["text"] for block in text_blocks))
            normalized["rawOutput"] = output
        return normalized
