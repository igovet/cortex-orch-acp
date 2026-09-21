"""T016 model-owned coordination: an :class:`IncomingRuntime` that runs a
real coordinator-led multi-assignment task instead of forwarding user text
to a single downstream agent.

Process boundary (see ``docs/acp/orchestration.md``): this module is the
*main* process. It never itself executes a delegation MCP tool call --
those are handled inside the Cortex MCP subprocess an engine launches as
its own child (``delegation.mcp_tools``). This module only (a) opens/owns
engine sessions via the injected :class:`SessionRuntime`, (b) reacts to the
durable ``task_events`` log those tool calls produce, and (c) performs the
one privileged effect the MCP subprocess is forbidden from doing itself:
calling :class:`delegation.provisioning.ProvisioningService` to actually
provision a delegated worker and prompting its engine session.

Coordinator sessions are provisioned the same way (this module opens them
directly, not via ``ProvisioningService.delegate``, since a coordinator is
not itself a delegation).
"""
from __future__ import annotations

from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass
import re
import json
import hashlib
import os
import shlex
import sqlite3
import threading
import time
from typing import Any, Callable, Mapping, Protocol

from ..configuration.policy import BENIGN_CONFIGURATION_ERROR_CODES
from ..connections.environment import DENYLIST_PATTERN
from ..contracts import StoreError
from ..delegation import events as delegation_events
from ..delegation import integration as delegation_integration
from ..delegation import revision as delegation_revision
from ..delegation import workspace_isolation as delegation_workspace_isolation
from ..delegation.deliveries import MAX_DELIVERY_RETRIES
from ..delegation import provisioning as delegation_provisioning
from ..delegation.provisioning import ProvisioningService
from ..engine import failure_classification
from ..engine import reaper as engine_reaper
from ..engine.instruction_delivery_registry import delivery_for
from ..observability.redact import StreamingRedactor, scrub_display_text, scrub_text
from ..portable.engine_contract import StopReason
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from ..portable.instruction_delivery import DeliveryQualification, qualify_delivery
from ..portable.roles import composed_instructions, load_role
from ..recovery.capsule import build_capsule, render_capsule
from ..trusted_channel import ChannelContext, issue_channel_tx
from . import messages as acp_messages
from ..catalog import labels as catalog_labels
from . import pipeline_bootstrap
from . import questions_store
from ..routing.store import get_route_extras
from ..project_docs import render_catalog as render_project_docs_catalog
from ..project_docs import render_worker_catalog


def _question_answer_value(question, text: str) -> str:
    """Resolve an unambiguous 1-based choice index to its stored value."""
    options = question.options or []
    if isinstance(text, str) and text.strip().isdigit():
        index = int(text.strip())
        if 1 <= index <= len(options) and isinstance(options[index - 1], str):
            return options[index - 1]
    return text
from . import steering as acp_steering

COORDINATOR_ROLE_ID = "orchestrator"
DEFAULT_MAX_NO_PROGRESS_CONTINUATIONS = 3
# No cap: the coordinator may continue as many times as the work needs
# ("У координатора не должно быть никаких ограничений, он хоть миллион
# может сделать продолжений подряд"). A genuine stall is still caught by
# the no-progress budget, and the user's own cancellation still ends any
# turn; a fixed number of continuations only ever ended useful work early.
# Kept as a settable parameter so a test can still bound a loop.
DEFAULT_MAX_TOTAL_CONTINUATIONS = None
DEFAULT_WORKER_POOL_SIZE = 4
DEFAULT_WAIT_TIMEOUT_SECONDS = 5.0
_WAIT_BACKOFF_START = 0.05
_WAIT_BACKOFF_MAX = 1.0
_UNSET = object()
# Bound on the actionable-error text (e.g. resolve_role's own remediation,
# which lists every valid role id) carried in an assignment_failed event
# payload -- large enough to hold the full ~30-role-id catalog uncut today,
# small enough to never make one event payload unbounded as that catalog
# grows. Truncated with a marker, never silently dropped.
_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS = 1200
# T035 visibility fix: bound on one streamed coordinator delta chunk after
# redaction (scrub_text's own default MAX_TEXT_CHARS=400 is sized for a
# short logged field, not an engine's own text-generation chunk, which can
# legitimately be a whole paragraph in one update). Generous, not
# unbounded: still protects against a single pathological chunk.
_COORDINATOR_DELTA_MAX_CHARS = 4000
_SHA256_IN_PROSE = re.compile(r"(?<![0-9a-fA-F])[0-9a-fA-F]{64}(?![0-9a-fA-F])")
_FINGERPRINT_IN_PROSE = re.compile(r"(?<![0-9a-fA-F])[0-9a-fA-F]{32,128}(?![0-9a-fA-F])")
_RAW_SECRET_KEY = re.compile(r"api[_-]?key|token|secret|password|authorization|credential", re.I)


class _CoordinatorTurnCancel:
    """Cancel one engine turn without cancelling the host's whole task.

    SessionRuntime only needs Event.wait/is_set.  Keep the host cancellation
    live while permitting an internal continuation boundary after a model
    loops on an acceptance refusal that requires a *new* delivered batch.
    """

    def __init__(self, host_cancel: threading.Event):
        self._host = host_cancel
        self._boundary = threading.Event()

    def request_boundary(self) -> None:
        self._boundary.set()

    def boundary_requested(self) -> bool:
        return self._boundary.is_set()

    def is_set(self) -> bool:
        return self._host.is_set() or self._boundary.is_set()

    def wait(self, timeout: float | None = None) -> bool:
        deadline = None if timeout is None else time.monotonic() + timeout
        while not self.is_set():
            remaining = None if deadline is None else deadline - time.monotonic()
            if remaining is not None and remaining <= 0:
                break
            self._host.wait(min(0.05, remaining) if remaining is not None else 0.05)
        return self.is_set()


def _pending_events_acceptance_refusal(raw_output: Any) -> bool:
    """Recognize the exact dispatch correction, not an arbitrary failed tool.

    ACP's rawOutput is provider-defined: Codex currently nests the MCP
    JSON error as text inside result.content.  Walk only this bounded tool
    result and require both the error code and its pending-event correction.
    """
    try:
        rendered = json.dumps(raw_output, ensure_ascii=False, default=str)
    except (TypeError, ValueError):
        return False
    return ('acceptance_refused' in rendered
            and 'unconsumed meaningful event(s)' in rendered)


def _is_cortex_decide_tool(title: Any) -> bool:
    """Recognize only the observed Cortex MCP names from both ACP engines.

    Codex renders the server/tool pair with dots; Claude Agent ACP uses its
    SDK's double-underscore MCP name.  A bare ``decide`` (or a different
    server's ``decide``) does not identify a Cortex dispatch.
    """
    return title in {"mcp.cortex.decide", "mcp__cortex__decide"}


def _guard_final_digests(text: str, accepted_pipeline: str | None, warning: str) -> str:
    """Do not echo an exact digest absent from the accepted decision basis.

    This is an integrity check for machine-verifiable identifiers, not a
    claim to validate the coordinator's prose or interpret its evidence.
    """
    if not _FINGERPRINT_IN_PROSE.search(text):
        return text
    current_edition = (accepted_pipeline or "").split("\n\n---\n\n", 1)[0]
    known = {value.lower() for value in _SHA256_IN_PROSE.findall(current_edition)}
    replaced = False

    def _replace(match):
        nonlocal replaced
        if match.group().lower() in known:
            return match.group()
        replaced = True
        return "[unverified digest omitted]"

    guarded = _FINGERPRINT_IN_PROSE.sub(_replace, text)
    if replaced:
        guarded += "\n\n" + warning
    return guarded


def _scrub_accepted_final(text: str, accepted_pipeline: str | None, warning: str) -> str:
    """Redact credentials while retaining only pipeline-verified SHA-256s.

    ``scrub_text`` treats every long hex run as a possible secret. Applying
    it to streamed chunks before checking the accepted pipeline erased even
    the correct digest and made the final-answer integrity guard ineffective.
    Keep raw chunks only in memory until the decision boundary, reject
    unknown digests, then scrub the spans between the allowed digests.
    """
    guarded = _guard_final_digests(text, accepted_pipeline, warning)
    current_edition = (accepted_pipeline or "").split("\n\n---\n\n", 1)[0]
    known = {value.lower() for value in _SHA256_IN_PROSE.findall(current_edition)}
    parts: list[str] = []
    cursor = 0
    for match in _FINGERPRINT_IN_PROSE.finditer(guarded):
        span = guarded[cursor:match.start()]
        parts.append(scrub_text(span, max_chars=max(4000, len(span) * 2 + 1000)))
        parts.append(match.group() if match.group().lower() in known else "[unverified digest omitted]")
        cursor = match.end()
    span = guarded[cursor:]
    parts.append(scrub_text(span, max_chars=max(4000, len(span) * 2 + 1000)))
    return "".join(parts)


def _accepted_status_correction_needed(text: str) -> bool:
    """Detect a coordinator's stale blocked-status claim after acceptance.

    This is intentionally narrow and only controls whether a canonical
    post-commit correction is appended.  It never creates an acceptance: the
    caller invokes it only after ``_check_terminal_decision`` has returned
    the already gate-validated terminal decision and ``_set_state`` commits
    ``state=accepted``.
    """
    lowered = " ".join(str(text or "").lower().split())
    return (
        "workers_running" in lowered
        or "state projection still reports blocked" in lowered
        or "projection still reports blocked" in lowered
        or bool(re.search(r"(?:task|state).{0,40}projection.{0,40}blocked", lowered))
        or bool(re.search(r"проекц\w*.{0,40}(?:задач\w*|состояни\w*).{0,40}заблок", lowered))
    )


def _bounded_detail(text: str) -> str:
    if len(text) <= _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS:
        return text
    return text[: _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS - 3] + "..."


def _provisioning_progress_note(provisioning_sig: tuple[tuple[str, str, str], ...]) -> str:
    """T035 FINDING-13 #2: a short, honest human-readable rendering of a
    ``delegation_provisioning.in_progress_snapshot`` result -- named
    attempts and their current step, so a host/log/human reading
    ``wait_detail`` (and the model itself, whenever this blocked note is
    eventually coalesced into a later continuation) can tell "still
    provisioning, currently at step X" from a generic, undifferentiated
    "workers_running"."""
    parts = [f"{assignment_ref} is provisioning (step: {step})" for assignment_ref, step, _updated_at in provisioning_sig]
    return "Still working: " + "; ".join(parts) + "."


# T035: one ACP ToolCall card per delegated assignment (see
# ``_emit_assignment_tool_call``/``_continuation_loop``). ``ToolCallStatus``
# (tests/fixtures/acp/schema-1.4.0.json) is exactly pending/in_progress/
# completed/failed -- there is no "cancelled" value, so a cancelled
# assignment is rendered as a failed card carrying the cancellation reason
# in its content (see _TOOL_CALL_STATUS_FOR_CANCELLED below). ``kind`` is a
# deliberate "execute" for every worker card: a delegated assignment runs
# arbitrary worker actions (edits, commands, searches, ...), never just one
# of the more specific kinds (read/edit/search/...), and "execute" reads
# truer than the catch-all "other".
# A worker assignment is not a command being run: a client renders
# "execute" under a "Run Command" heading, which describes nothing that
# is happening. The pinned schema has no "delegate"/"agent" kind, so the
# honest choice among its values is the catch-all.
_WORKER_TOOL_CALL_KIND = "other"
_TOOL_CALL_STATUS_FOR_CANCELLED = "failed"


# The pinned ACP schema's ToolCallKind values. An engine is free to send
# anything here (codex-acp and claude-agent-acp do not agree on the set),
# and a value the schema does not define would make the card unrenderable,
# so anything unrecognized becomes "other" rather than being dropped --
# the call still shows, only its icon is generic.
_ACP_TOOL_CALL_KINDS = frozenset({
    "read", "edit", "delete", "move", "search", "execute", "think", "fetch", "switch_mode", "other",
})


def _acp_tool_call_kind(kind: str | None) -> str:
    return kind if kind in _ACP_TOOL_CALL_KINDS else "other"


# Likewise for status: the schema has exactly these four.
_ACP_TOOL_CALL_STATUSES = frozenset({"pending", "in_progress", "completed", "failed"})


def _acp_tool_call_status(status: str | None) -> str:
    return status if status in _ACP_TOOL_CALL_STATUSES else "in_progress"


# A worker's card carries the one thing it is doing right now, not its
# whole feed -- so this bounds a single line, not a transcript.
_WORKER_ACTIVITY_MAX_CHARS = 300
_TOOL_LINE_MAX_CHARS = 80

# How many past actions a worker's expanded card keeps.
#: Keep every worker event on a new line inside its one assignment card.
_WORKER_ACTION_SEPARATOR = "\n"


def _joined_worker_actions(entries: list[str]) -> str:
    """Render the complete observable worker transcript in event order.

    Worker activity is already persisted as ordered ACP ``tool_call_update``
    records by the host emitter.  Keeping the complete in-memory transcript
    here is therefore important too: a later update must not replace the
    beginning of the worker's account with only its most recent actions.
    Preserve message formatting; the final ACP payload passes through the
    ordinary redaction boundary.
    """
    return _WORKER_ACTION_SEPARATOR.join(
        line for entry in entries if (line := str(entry).strip())
    )


def _human_tool_title(title: Any) -> str:
    """Give public Cortex MCP operations readable timeline titles."""
    value = str(title or "").strip()
    match = re.match(r"^(?:mcp[._]{1,2})?cortex[._]{1,2}([A-Za-z0-9_.-]+)$", value, re.I)
    if not match:
        return value
    operation = re.sub(r"[_-]+", " ", match.group(1)).replace(".", " ")
    operation = " ".join(operation.split()).lower()
    return operation[:1].upper() + operation[1:]

# A tool call's title is one line on a card, never a place for a payload.
_TOOL_CALL_TITLE_MAX_CHARS = 200

# Paseo renders every ACP tool call as a full-height timeline card.  The
# standard ACP protocol has no portable group/collapse entity, so coordinator
# calls from one prompt are packed into one ordinary ACP card.  The entries
# stay newline-delimited JSON in the card body, preserving the exact
# observable fields while giving the client one card it can collapse.
_COORDINATOR_ACTION_LOG_MAX_CHARS = 1024 * 1024


def _format_display_value(value: Any, *, indent: int = 0, depth: int = 0) -> list[str]:
    """Render safe tool data as readable labelled lines, never JSON syntax."""
    prefix = " " * indent
    if depth > 8:
        return [prefix + "[nested data omitted]"]
    if isinstance(value, Mapping):
        lines: list[str] = []
        for key, item in value.items():
            label = str(key)
            if isinstance(item, (Mapping, list, tuple)):
                lines.append(f"{prefix}{label}:")
                lines.extend(_format_display_value(item, indent=indent + 2, depth=depth + 1))
            else:
                lines.append(f"{prefix}{label}: {item}")
        return lines or [prefix + "(empty)"]
    if isinstance(value, (list, tuple)):
        lines = []
        for item in value:
            if isinstance(item, (Mapping, list, tuple)):
                lines.extend([prefix + "- " + line.lstrip() for line in _format_display_value(item, indent=indent + 2, depth=depth + 1)])
            else:
                lines.append(f"{prefix}- {item}")
        return lines or [prefix + "(empty)"]
    return [prefix + str(value)]


def _worker_display_text(update: Mapping[str, Any]) -> str | None:
    """Extract a short human-facing worker display event.

    ACP display updates are a transport envelope.  In particular, some
    adapters put the complete serialized ``tool_call_update`` payload in a
    display chunk even though the same call is delivered through
    ``on_tool_call``.  Copying that envelope into the worker card produces
    the unreadable JSON wall seen in Paseo and duplicates every tool.  Tool
    updates are already rendered by the tool callback, so only explicit
    thought/plan/message text belongs here.
    """
    kind = str(update.get("sessionUpdate") or update.get("type") or "display").lower()
    if "tool_call" in kind or kind in {"available_commands_update", "config_options_update"}:
        return None
    content = update.get("content")
    text_value = content.get("text") if isinstance(content, Mapping) else None
    if text_value is None:
        text_value = update.get("text")
    if text_value is None:
        return None
    if isinstance(text_value, (Mapping, list, tuple)):
        return None
    text = scrub_display_text(str(text_value), max_chars=4000).strip()
    return text or None


_SHELL_COMPOSITION = re.compile(r"(?:&&|\|\||[;|\n\r])")


def _simple_read_command_path(command: Any) -> str | None:
    """Return the path from a genuinely single-file ``cat`` command.

    Some providers report a native read as a shell-shaped ``rawInput``.  A
    command containing a chain is still a shell action and must retain its
    command presentation.  The narrow exception for ``2>/dev/null`` keeps a
    provider's harmless stderr suppression from turning an otherwise simple
    read into a command card.
    """
    if not isinstance(command, str) or not command.strip() or _SHELL_COMPOSITION.search(command):
        return None
    try:
        words = shlex.split(command, posix=True)
    except ValueError:
        return None
    if not words or words[0].rsplit("/", 1)[-1] != "cat":
        return None
    args = [word for word in words[1:] if word != "2>/dev/null"]
    if len(args) != 1 or args[0].startswith("-") or not args[0].strip():
        return None
    return args[0]


def _worker_tool_details(
    title: str, status: str | None, locations: Any, content: Any,
    raw_input: Any, raw_output: Any, project_root: str | None = None,
    *, kind: str | None = None,
) -> str:
    """Render one compact worker tool line without protocol JSON or output."""
    raw_title = _human_tool_title(title)
    label = scrub_display_text(raw_title, max_chars=max(4000, len(raw_title) * 2 + 1000))
    argument = None
    if isinstance(raw_input, Mapping):
        command = raw_input.get("command")
        cwd = raw_input.get("cwd")
        if command:
            read_path = (
                _simple_read_command_path(command)
                if str(kind or "").casefold() == "read" else None
            )
            if read_path is not None:
                argument = _project_relative(read_path, project_root)
            else:
                argument = scrub_text(command, max_chars=max(4000, len(command) * 2 + 1000))
                if str(kind or "").casefold() == "read":
                    # A read-kind payload that is not one simple file read is
                    # still shell activity (often a chained inspection
                    # command). Keep its command presentation explicit.
                    label = "Command"
            normalized_title = " ".join(raw_title.lower().split())
            normalized_command = " ".join(str(command).lower().split())
            title_prefix = normalized_title.rstrip("…").rstrip()
            if (str(kind or "").casefold() == "execute"
                    or normalized_command in normalized_title
                    or (title_prefix and normalized_command.startswith(title_prefix))
                    or (len(normalized_title) >= 24
                        and normalized_command.startswith(normalized_title[:24]))
                    or normalized_title in {"terminal", "shell", "bash", "execute", "run command"}):
                label = "Command"
            if cwd and not argument:
                argument = _project_relative(str(cwd), project_root)
        else:
            # Codex and Claude use different aliases for the path of a
            # native Read call.  ``locations`` is optional in ACP, so a
            # provider that puts the path only in rawInput otherwise leaves
            # the collapsed card as the useless generic "Read" title.
            # Keep the complete structured rawInput below for expansion;
            # this is only the bounded, project-relative compact label.
            path = next(
                (raw_input[key] for key in (
                    "path", "filePath", "file_path", "file", "filename",
                    "absolutePath", "abs_path", "target",
                ) if isinstance(raw_input.get(key), str) and raw_input[key].strip()),
                None,
            )
            if path:
                argument = _project_relative(path, project_root)
            line_start = raw_input.get("line_start", raw_input.get("lineStart"))
            line_end = raw_input.get("line_end", raw_input.get("lineEnd"))
            if argument and line_start is not None:
                argument += f":{line_start}"
                if line_end is not None and line_end != line_start:
                    argument += f"-{line_end}"
        if argument is None:
            server = raw_input.get("server")
            tool = raw_input.get("tool")
            if server and tool:
                argument = f"{server}.{tool}"
                args = raw_input.get("arguments") or raw_input.get("input")
                if isinstance(args, Mapping):
                    values = []
                    for key, value in args.items():
                        if isinstance(value, (str, int, float, bool)):
                            text = scrub_text(str(value), max_chars=max(4000, len(str(value)) * 2 + 1000))
                            values.append(f"{key}={text}")
                    if values:
                        argument += " " + ", ".join(values[:4])
            else:
                values = []
                for key, value in raw_input.items():
                    if isinstance(value, (str, int, float, bool)):
                        values.append(f"{key}={scrub_text(str(value), max_chars=max(4000, len(str(value)) * 2 + 1000))}")
                if values:
                    argument = ", ".join(values[:4])
    if locations:
        paths = [
            _project_relative(item["path"], project_root)
            for item in locations
            if isinstance(item, Mapping) and isinstance(item.get("path"), str)
        ]
        if paths and not argument:
            argument = ", ".join(paths)
    line = label
    if argument:
        line += " — " + argument
    failed = _acp_tool_call_status(status) == "failed"
    if failed and raw_output is not None:
        detail = None
        if isinstance(raw_output, Mapping):
            for key in ("error", "message", "formatted_output", "output", "stderr", "detail"):
                value = raw_output.get(key)
                if value not in (None, ""):
                    detail = value
                    break
        elif isinstance(raw_output, str):
            detail = raw_output
        if detail is not None:
            line += "\nError: " + scrub_text(str(detail), max_chars=max(4000, len(str(detail)) * 2 + 1000))
    suffix = " [failed]" if failed else ""
    return scrub_display_text(line, max_chars=max(4000, len(line) * 2 + 1000)) + suffix


def _compact_tool_line(
    title: str | None, status: str | None, locations: Any,
    raw_input: Any, raw_output: Any = None, project_root: str | None = None,
    *, kind: str | None = None,
) -> str:
    """Render one coordinator tool call as one bounded, human line."""
    return _worker_tool_details(
        title or "Tool call", status, locations, None, raw_input, raw_output, project_root,
        kind=kind,
    )


_GENERIC_TOOL_PLACEHOLDER_TITLES = frozenset({
    "terminal", "tool call", "tool", "command", "execute", "run command",
    "shell", "bash", "preparing file", "read file", "edit", "editing files",
})


def _has_concrete_tool_activity(
    title: str | None, locations: Any, content: Any, raw_input: Any,
) -> bool:
    """Return whether a tool update contains something useful to show.

    ACP providers sometimes create a ``Terminal`` call before they have
    attached the command, cwd, or path.  Rendering that provisional envelope
    creates an empty ``Terminal`` row (and a second one for its status update)
    in Paseo.  Keep the call in the engine/evidence stream, but defer its
    client card until a concrete command/path/MCP target or real display
    content arrives.  A descriptive title such as ``Read README.md`` is
    itself concrete and remains visible even when the provider omits the
    optional fields.
    """
    if isinstance(locations, (list, tuple)) and any(
        isinstance(item, Mapping) and isinstance(item.get("path"), str)
        and item.get("path", "").strip()
        for item in locations
    ):
        return True
    if isinstance(content, (list, tuple, Mapping)) and bool(content):
        return True
    if isinstance(raw_input, Mapping):
        command = raw_input.get("command")
        if isinstance(command, str) and command.strip():
            return True
        for key in ("path", "file", "file_path", "cwd"):
            value = raw_input.get(key)
            if isinstance(value, str) and value.strip():
                return True
        server = raw_input.get("server")
        tool = raw_input.get("tool")
        if isinstance(server, str) and server.strip() and isinstance(tool, str) and tool.strip():
            return True
    normalized = " ".join(str(title or "").split()).strip().lower()
    # Paseo/ACP providers use a few UI-oriented titles while a file call is
    # still only a provisional envelope (for example ``Preparing file…`` or
    # ``Read File``).  Treat trailing punctuation/ellipsis as presentation,
    # so these placeholders do not create an empty coordinator group.  A
    # later update carrying a path, command, MCP target, or display content
    # still passes the earlier concrete-field checks above.
    normalized = re.sub(r"(?:\.{3}|…|[!?:]+)$", "", normalized).strip()
    return bool(normalized) and normalized not in _GENERIC_TOOL_PLACEHOLDER_TITLES


def _format_tool_event(record: Mapping[str, Any], sequence: int) -> str:
    """Compatibility wrapper for old callers; each event is one line."""
    return _compact_tool_line(
        record.get("title"), record.get("status"), record.get("locations"),
        record.get("rawInput"), record.get("rawOutput"), record.get("project_root"),
    )

# How soon a delegation asked for mid-turn is picked up. Small enough that
# a worker starts while the coordinator is still talking, large enough that
# an idle turn is not a busy loop against the store.
_EARLY_DISPATCH_POLL_SECONDS = 0.25

# How many times the identical integration conflict may repeat for one
# assignment before the coordinator is told that retrying is not a strategy.
_IDENTICAL_INTEGRATION_CONFLICT_LIMIT = 3

# This runtime's own MCP tools, as an engine names them in a tool call.
# Their individual cards are noise in a session feed: a reader wants the
# work, not the bookkeeping that supports it.
# This runtime's own MCP tools, however an engine spells the server
# prefix: codex writes "mcp.cortex.create_task", claude writes
# "mcp__cortex__delegate". Matching only the first spelling put every one
# of the coordinator's own calls back in the feed the moment the engine
# changed.
_INTERNAL_TOOL_CALL = re.compile(r"^(?:mcp[._]{1,2})?cortex[._]{1,2}")

# Cortex's private store is bookkeeping even when a provider reports the
# operation as an ordinary native file tool. Match a path component (rather
# than the substring) so project names such as ``my.cortex-file`` remain
# visible. Tool metadata is compared case-insensitively because providers do
# not consistently preserve the casing of displayed paths.
_CORTEX_STORE_COMPONENT = re.compile(r"(?:^|[/\\\s])\.cortex(?:[/\\\s]|$)", re.I)

# Host-side computer/UI controls are runtime plumbing, not work performed on
# the delegated project.  Zed's live ACP mirror currently exposes the CUA
# bridge as e.g. ``mcp.cua_repl.js — cua_repl.js code=...``.  Match only the
# known runtime namespaces; ordinary project MCP servers (including codebase
# memory/search servers) must remain visible in the worker card.
_RUNTIME_UI_TOOL_CALL = re.compile(
    r"^(?:mcp[._]{1,2})?(?:cua[_ .-]?repl|codex[_ .-]?app)(?:[._\s:-]|$)"
    r"|^(?:functions[._]{1,2})?(?:computer|computer[_ .-]?use|browser[_ .-]?control)(?:[._\s:-]|$)",
    re.I,
)

# ``delegate`` is represented by the assignment lifecycle card. Showing the
# coordinator's Cortex MCP dispatch as a second top-level card duplicates the
# same work in Paseo; all Cortex MCP calls are runtime bookkeeping.
_COORDINATOR_DELEGATE_TOOL_CALL = re.compile(
    r"^(?:mcp[._]{1,2})?cortex[._]{1,2}delegate$", re.I,
)
_COORDINATOR_CORTEX_TOOL_CALL = re.compile(
    r"^(?:mcp[._]{1,2})?cortex[._]{1,2}[a-z][a-z0-9_]*$", re.I,
)

# The engine's own tool-discovery step, which it reports as a tool call of
# its own ("ToolSearch"). Like its self-review below, it is the engine
# working out what it can do, not an action on the project.
_ENGINE_TOOL_DISCOVERY = re.compile(r"^\s*tool[_ ]?search\b", re.I)

# The engine's own agent surface. Seen live: a coordinator used codex's
# subagents and its `wait` instead of `delegate`, so the whole delegation
# path -- isolated workspace, conflict-checked integration, per-role model
# policy, audit -- was bypassed, and the feed filled with a native `wait`
# nobody asked to see. The role forbids these; this is what notices when
# the prohibition is ignored anyway, and keeps the pure-noise ones out of
# the feed. Matched on the call's own name, loosely: an engine names them
# however it likes, and a false positive here costs a card, not work.
# The engine's own approval/deliberation step, which it reports as a tool
# call of its own ("Guardian Review": Status/Action/Risk/Authorization/
# Rationale). It is the engine reviewing itself, not an action taken on
# the project, and a card for every one of them sits between the commands
# that are. Matched by name as well as by kind: the kind arrives on the
# update, not always on the create, so a name is what is available when
# the card would first be drawn.
_ENGINE_REVIEW = re.compile(r"\bguardian\s+review\b", re.I)

_NATIVE_WAIT = re.compile(r"^\s*wait\b", re.I)
_NATIVE_SUBAGENT = re.compile(r"\b(?:sub-?agent|spawn[_ ]?agent|spawnagent)\b", re.I)
_ENGINE_INTERNAL = re.compile(
    r"(?:^\s*(?:await|async\s+await|thinking|scheduler|schedule[_ -]?wakeup|bookkeeping|context[_ -]?compaction)\b|\b(?:scheduler|schedule[_ -]?wakeup|bookkeeping)\b)",
    re.I,
)
_AMBIGUOUS_FILE_ACTIVITY = re.compile(r"^\s*editing\s+files?\s*$", re.I)


def _is_wait_tool(title: str | None) -> bool:
    """Codex's internal wait call is idle bookkeeping, not useful work."""
    if not isinstance(title, str):
        return False
    return bool(re.match(r"^(?:(?:functions|codex)\.)?wait(?:$|[\s(:])", title.strip(), re.I))


def _is_engine_internal_tool(
    title: str | None, kind: str | None = None, locations: Any = None,
    raw_input: Any = None, content: Any = None,
) -> bool:
    """Return true for provider bookkeeping that is not user work.

    External MCP calls deliberately do not match this predicate and remain
    useful evidence of the model's interaction with the project. Cortex's
    own MCP server is runtime bookkeeping and is filtered with the provider
    internals. ``think`` is the portable ACP kind used by both adapters for
    private deliberation; it is never a project action.
    """
    if kind == "think":
        return True
    if _is_cortex_tool_payload(title, raw_input, locations, content):
        return True
    if _is_wait_tool(title):
        return True
    if not isinstance(title, str):
        return False
    # Cortex MCP calls are runtime bookkeeping. Keep ordinary project MCP
    # calls visible, but never expose this runtime's own server operations.
    if _INTERNAL_TOOL_CALL.match(title):
        return True
    if (_RUNTIME_UI_TOOL_CALL.match(title)
            or (isinstance(raw_input, Mapping)
                and _RUNTIME_UI_TOOL_CALL.match(str(raw_input)))):
        return True
    if _COORDINATOR_DELEGATE_TOOL_CALL.match(title):
        return True
    return bool(_ENGINE_TOOL_DISCOVERY.match(title)
                or _ENGINE_REVIEW.search(title)
                or _ENGINE_INTERNAL.search(title))


def _cortex_store_value(value: Any) -> bool:
    """Return whether tool metadata refers to Cortex private storage."""
    if isinstance(value, str):
        return bool(_CORTEX_STORE_COMPONENT.search(value))
    if isinstance(value, Mapping):
        return any(_cortex_store_value(item) for item in value.values())
    if isinstance(value, (list, tuple, set, frozenset)):
        return any(_cortex_store_value(item) for item in value)
    return False


def _is_cortex_tool_payload(title: Any, raw_input: Any, locations: Any, content: Any) -> bool:
    """Hide Cortex MCP and private-store file actions from ACP tool cards."""
    if isinstance(title, str) and _INTERNAL_TOOL_CALL.match(title.strip()):
        return True
    if isinstance(raw_input, Mapping):
        server = raw_input.get("server")
        if isinstance(server, str) and server.strip().casefold() in {"cortex", "mcp.cortex"}:
            return True
    return any(_cortex_store_value(value) for value in (title, raw_input, locations, content))


def is_hidden_tool_update(update: Mapping[str, Any] | None) -> bool:
    """Identify a persisted ACP tool update excluded from host replay."""
    if not isinstance(update, Mapping):
        return False
    return _is_cortex_tool_payload(
        update.get("title"), update.get("rawInput"),
        update.get("locations"), update.get("content"),
    ) or _cortex_store_value(update.get("text"))


def _compact_text(value: Any, max_chars: int) -> str:
    """Flatten and bound one display value with a normal ellipsis."""
    text = scrub_display_text(str(value), max_chars=max(4000, len(str(value)) + 1000))
    text = " ".join(text.split())
    if len(text) > max_chars:
        return text[: max_chars - 1].rstrip() + "…"
    return text


# A worker card's title (role, work, and the action it is on right now).
_WORKER_CARD_TITLE_MAX_CHARS = 300

def _client_safe_content(content):
    """Forward portable tool-call content with credential redaction.

    ACP lets a tool call carry a `terminal` content block referring to a
    terminal by id -- but that terminal belongs to *this* agent's session
    with the engine, not to the client's session with us. Forwarded
    verbatim, Zed answered the whole notification with
    `-32603 ... Terminal with id ... not found`, which loses the entire
    card, not just the block. Seen live in Zed's own log.

    Text and diffs retain their ACP shapes, but free-text fields are
    redacted before they enter the durable host-update journal or UI.
    """
    if not content:
        return content
    def safe(value, *, path: bool = False, depth: int = 0):
        if depth > 5:
            return "[nested tool content omitted]"
        if isinstance(value, str):
            redactor = scrub_display_text if path else scrub_text
            return redactor(value, max_chars=max(4000, len(value) * 2 + 1000))
        if isinstance(value, Mapping):
            return {
                key: safe(item, path=key in {"path", "filePath"}, depth=depth + 1)
                for key, item in value.items() if isinstance(key, str)
            }
        if isinstance(value, (list, tuple)):
            return [safe(item, depth=depth + 1) for item in value]
        return value

    kept = [safe(part) for part in content
            if not (isinstance(part, Mapping) and part.get("type") == "terminal")]
    return kept or None


def _client_safe_raw(value, *, depth: int = 0):
    """Redact ACP rawInput/rawOutput before they enter client history.

    These fields can contain commands, Cortex MCP arguments and results, but
    also connection secrets. Preserve their complete JSON shape for Paseo's
    richer tool detail view. The ACP payload is already a decoded JSON value
    from the engine; applying display-size/depth/list caps here silently
    destroys the input/output evidence the expanded card is for. The normal
    credential scrubber remains the only content transformation.
    """
    if isinstance(value, str):
        # Leave enough room for a redaction marker without imposing an
        # arbitrary payload limit. This preserves long commands/results and
        # still applies the repository's credential/token policy.
        return scrub_text(value, max_chars=max(4000, len(value) * 2 + 1000))
    if isinstance(value, Mapping):
        return {
            key: ("[REDACTED]" if DENYLIST_PATTERN.search(key) or _RAW_SECRET_KEY.search(key)
                  else _client_safe_raw(item, depth=depth + 1))
            for key, item in value.items() if isinstance(key, str)
        }
    if isinstance(value, (list, tuple)):
        return [_client_safe_raw(item, depth=depth + 1) for item in value]
    return value


def _native_edit_raw_input(kind: str | None, title: str | None, raw_input: Any) -> Any:
    """Add Paseo's edit aliases only when the engine supplied their values.

    Paseo's native edit renderer recognizes ``filePath``, ``oldString``,
    ``newString`` and ``unifiedDiff``.  Engines use a few equivalent wire
    spellings for the first three fields.  Preserve the original raw input
    and add only aliases whose exact source value is present; this helper
    never derives a diff or invents an old/new value.
    """
    if not isinstance(raw_input, Mapping):
        return raw_input
    normalized_kind = str(kind or "").casefold()
    normalized_title = str(title or "").casefold()
    if normalized_kind != "edit" and not re.search(r"\b(edit|patch|write)\b", normalized_title):
        return raw_input
    result = dict(raw_input)
    aliases = (
        ("filePath", ("filePath", "file_path", "path")),
        ("oldString", ("oldString", "old_string")),
        ("newString", ("newString", "new_string")),
        ("unifiedDiff", ("unifiedDiff",)),
    )
    for canonical, candidates in aliases:
        if canonical in result:
            continue
        for source in candidates:
            if source in raw_input and raw_input[source] is not None:
                result[canonical] = raw_input[source]
                break
    return result


def _project_relative(text: str | None, project_root: str | None) -> str | None:
    """Rewrite absolute paths inside ``project_root`` as paths relative to
    it, and leave everything else exactly as it is.

    A card reading "Read file '/var/tmp/cortex-zed-demo/calc.py'" spends
    most of its width on the one part the reader already knows -- which
    project they are in. ``calc.py`` says the same thing. A path *outside*
    the project is the opposite: that it is elsewhere is the important
    part, so it keeps its full spelling.
    """
    if not text or not project_root:
        return text
    root = project_root.rstrip(os.sep)
    if not root:
        return text
    if text == root:
        return "."
    return text.replace(root + os.sep, "")


# The store's own directory as it appears inside a path or a command
# line: at the start, or after a separator, quote or whitespace.
_CORTEX_STORE_SEGMENT = re.compile(r"(?:^|[\s\"'`=(/\\])\.cortex(?:[/\\]|$)")
# An absolute path outside the task project is not useful in a compact worker
# card and can disclose host layout.  Apply this only to the display mirror;
# tool inputs and durable evidence retain their exact values.
_EXTERNAL_ABSOLUTE_PATH = re.compile(
    r"(?<![A-Za-z0-9_.:/\\])/(?:[^\s\"'`]+/)*[^\s\"'`/]+"
)


def _worker_mirror_text(value: Any, project_root: str | None = None) -> str:
    """Make cumulative worker-card text safe for display.

    Project paths are shortened first so ordinary project commands remain
    useful.  Remaining absolute paths are suppressed, while the normal
    credential redactor still handles secrets.  This function is only used
    for the plain-string ACP mirror and never changes execution/provenance.
    """
    text = _project_relative(str(value), project_root) or ""
    text = scrub_text(text, max_chars=max(4000, len(text) * 2 + 1000))
    return _EXTERNAL_ABSOLUTE_PATH.sub("[external path]", text)


def _touches_store(title: str | None, locations) -> bool:
    """Whether a tool call is working on this project's own store rather
    than on the project.

    Writing a report or filling a draft is done with ordinary file
    operations against ``<project>/.cortex`` -- by design. To a reader
    those are not edits to their code: they are the bookkeeping that
    surrounds it, and one "Editing files" card per report buried the work
    the report is about. They fold into the same collapsed block as this
    runtime's own tool calls.

    Judged by where the call actually points -- its ``locations`` first,
    and its title only as the fallback for an engine that sends none.

    A title is matched on the ``.cortex/`` path segment wherever it
    appears, not on an absolute prefix: a run command names its paths
    inline (``sed -n '1,45p' .cortex/draft-reports/d_...md``), and titles
    have already had the project root stripped by ``_project_relative``,
    so there is no leading separator left to anchor on. Everything to do
    with Cortex's own task files is bookkeeping, whichever tool touches
    it.
    """
    for location in locations or ():
        path = location.get("path") if isinstance(location, Mapping) else None
        if isinstance(path, str) and _CORTEX_STORE_SEGMENT.search(path):
            return True
    return bool(title) and bool(_CORTEX_STORE_SEGMENT.search(title))


def _display_role(role: str) -> str:
    """The role as a reader sees it on a worker's card.

    ``portable.roles`` stores every role under its canonical skill id
    (``worker-backend-dev``); the ``worker-`` prefix is storage identity,
    and on a card already titled "Worker:" it reads as a stutter. Strips
    only that one prefix, never any other part of the id, so an unprefixed
    id (the alias form, e.g. ``backend-dev``) is returned unchanged.
    """
    return role[len("worker-"):] if role.startswith("worker-") else role


def _worker_tool_call_id(assignment_ref: str) -> str:
    """Stable id for one delegated assignment's card, derived from its own
    durable ``assignment_ref`` (the coordinator-facing delegation
    idempotency key -- see ``delegation.mcp_tools._coordinator_delegate``)
    so every event this assignment ever produces (queued/running/finished/
    failed/cancelled) updates the exact same card, never a new one."""
    return f"worker:{assignment_ref}"


def _worker_action_tool_call_id(assignment_ref: str, attempt_ref: str, tool_ref: str) -> str:
    """Stable per-attempt identity for one worker project/MCP action.

    Engine providers commonly restart their wire call counter after a route
    fallback. Include the durable attempt identity so the same provider ref
    from two attempts cannot overwrite or deduplicate the other card.
    """
    return f"worker-action:{assignment_ref}:{attempt_ref}:{tool_ref}"


@dataclass(frozen=True)
class PromptOutcome:
    """One engine session turn's outcome, as observed by this runtime.

    ``text`` is the visible assistant text for that turn (streamed via
    ``emit_update`` only for the coordinator -- never a raw worker chunk).
    Any delegation MCP tool call the engine made during the turn has
    already been durably committed by the time this is returned (the ACP
    ``session/prompt`` response only arrives after the engine's own tool
    calls to its Cortex MCP subprocess complete).
    """

    stop_reason: StopReason
    text: str = ""


class HostElicitor(Protocol):
    """T020: optional structured-input dependency, injected the same way
    ``SessionRuntime`` is (never required -- every existing caller/test
    passes none and gets the text-fallback path unconditionally). A real
    implementation (see ``acp.agent``) relays through the connected host's
    ``elicitation/create`` request via that connection's own
    ``HostRequester``, scoped to one ACP session, only when the host
    advertised ``clientCapabilities.elicitation.form`` at ``initialize``.

    ``elicit`` must never raise for an ordinary decline/cancel/timeout/
    unsupported outcome -- it returns ``None`` for all of those (the caller
    then falls back to the plain text question), and only a genuine bug
    propagates as an exception (also treated as ``None`` by the caller, see
    ``OrchestratingRuntime._handle_ask_user``)."""

    def elicit(self, session_id: str, question: str, options: list[str] | None) -> str | None: ...


class SessionRuntime(Protocol):
    """Everything this runtime needs from a live (or fake) engine session
    manager: the T015 ``EngineSessionOpener`` surface (duck-type compatible
    with ``delegation.provisioning.EngineSessionOpener`` -- passed directly
    to ``ProvisioningService``) plus the ability to actually send a prompt.
    """

    def open(self, workspace_binding_id: str, launch: Mapping[str, Any], instructions: str) -> str: ...
    def lookup_owned(self, attempt_id: str) -> str: ...
    def terminate_owned(self, attempt_id: str) -> None: ...
    def prompt(
        self, session_ref: str, message: str, cancel_event: threading.Event, *,
        on_delta: Callable[[str], bool] | None = None,
        on_tool_call: Callable[..., None] | None = None,
        on_display_update: Callable[[dict], None] | None = None,
    ) -> PromptOutcome: ...


class EngineTurnFailed(Exception):
    """Raised by a :class:`SessionRuntime`.prompt() implementation when an
    engine turn failed for a durable, named reason instead of returning a
    normal :class:`PromptOutcome` -- never silently treated as ``end_turn``.

    ``kind`` is a short machine-readable failure kind (for example
    ``"transport_eof"``, ``"timeout_alive"``, or ``"poisoned"``; see
    ``engine.session_manager``'s ``EngineEventKind.ERROR`` messages and
    ``_session_unusable_error``), recorded verbatim in the durable
    ``assignment_failed``/``progress`` event this runtime writes for it.
    """

    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(kind)


class SecondCoordinatorRefused(PortableError):
    def __init__(self) -> None:
        super().__init__(
            category=PortableErrorCategory.AUTHORIZATION,
            code="second_coordinator_refused",
            remediation="A coordination turn for this task is already active in this runtime; wait for it to finish or cancel it first.",
            retry_eligible=False,
        )


def _now() -> str:
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _coalesce(events: list[delegation_events.TaskEvent], revisions: dict[int, dict] | None = None,
              assignment_refs: dict[str, str] | None = None) -> str:
    """Coalesce a batch of claimed events into ONE continuation prompt.

    Every claimed event contributes a line (informative-only event types
    included) so nothing silently vanishes from the record even when it
    does not by itself gate continuation; see
    ``delegation.events.MEANINGFUL_EVENT_TYPES`` for what *does* gate it.

    ``revisions`` (T019), keyed by event id, is this exact batch's own
    freshly captured observed artifact revision for each ``worker_result``
    event (see ``OrchestratingRuntime._process_new_events`` and
    ``delegation.revision``) -- appended as a plain label so the
    coordinator can see a check's evidence is bound to one specific
    revision, never silently assumed still current.

    ``assignment_refs``, keyed by attempt id, translates the two event
    types the *worker* keys by its own attempt id (``worker_result`` and
    ``worker_question`` -- see ``_assignment_ref_for_attempt``) back to the
    delegation ref the coordinator was handed by ``delegate``. Without it
    the coordinator is shown an identifier from an id space it has never
    seen: live (2026-09-19, two assignments at once) it answered "this does
    not match any of my assignments" and refused both a real question and a
    real result, and it could not have answered anyway -- ``clarify`` takes
    the delegation ref, not an attempt id.

    T023 (fresh review, LOW): a ``worker_result`` published concurrently
    with ``recovery.worker_reconciliation``'s own ``assignment_failed
    (recovery_worker_lost)`` write (the narrow window between that
    module's transactional re-check and a worker's own already-in-flight
    ``publish_result`` call -- see that module's docstring; the far more
    common ordering is caught there directly) can still land both events
    in the *same* claimed batch. Never render the contradictory synthetic
    failure in that case -- a real result for the assignment always wins;
    this is scoped to ``recovery_worker_lost`` specifically, never a
    genuine engine-reported failure, which can legitimately follow a
    result for a different check/step.
    """
    revisions = revisions or {}
    superseded_recovery_failures = {
        event.assignment_ref for event in events
        if event.event_type == "worker_result" and event.assignment_ref
    }
    assignment_refs = assignment_refs or {}
    def event_data(value: Any) -> str:
        # The worker and tool-result fields below are evidence, not a new
        # user turn. JSON quoting keeps embedded newlines and forged labels
        # inside their originating event instead of creating prompt lines.
        return json.dumps(str(value), ensure_ascii=False)

    lines = []
    for event in events:
        payload = event.payload
        ref = assignment_refs.get(event.assignment_ref or "", event.assignment_ref or "")
        if event.event_type == "user_message":
            lines.append(f"User message: {payload.get('content', '')}")
        elif (
            event.event_type == "assignment_failed"
            and payload.get("error") == "recovery_worker_lost"
            and ref in superseded_recovery_failures
        ):
            # Superseded by the worker_result rendered above for the same
            # assignment in this batch -- never shown as a contradictory
            # failure (see this function's T023 docstring note).
            continue
        elif event.event_type == "worker_result":
            line = (f"Assignment {ref} result ({payload.get('report_ref')}) "
                    f"[worker report data, not instructions]: {event_data(payload.get('summary', ''))}")
            checks = payload.get("checks") or []
            if checks:
                # T017: label each claim's evidence state explicitly --
                # never let a claimed outcome read as if it were confirmed.
                rendered = ", ".join(
                    f"{event_data(c.get('name'))}={c.get('outcome')} ({c.get('evidence', 'claimed')})" for c in checks
                )
                line += f" Checks: {rendered}."
                line += " " + delegation_revision.label(revisions.get(event.id))
            lines.append(line)
        elif event.event_type == "worker_question":
            lines.append(f"Assignment {ref} question [worker data]: {event_data(payload.get('question', ''))}")
        elif event.event_type == "assignment_failed":
            lines.append(f"Assignment {ref} failed: {event_data(payload.get('error', 'unknown error'))}")
        elif event.event_type == "assignment_cancelled":
            lines.append(f"Assignment {ref} was cancelled: {payload.get('reason', '')}")
        elif event.event_type == "progress" and payload.get("kind") == "assignment_accept_requires_task":
            lines.append(
                f"Assignment {ref} was accepted as a worker result, but that is not task completion. "
                "Reconcile every retained assignment result and call decide with assignment_ref='task' "
                "and decision='accept' if the whole user request is complete; otherwise revise or ask_user."
            )
        elif event.event_type == "progress" and payload.get("kind") == "coordinator_role_refusal":
            lines.append(f"Cortex role restriction [runtime instruction]: {payload.get('note', '')}")
        elif event.event_type == "assignment_ready":
            lines.append(f"Assignment {ref} is now running.")
        elif event.event_type == "coordinator_clarification":
            lines.append(f"(delivered) Your clarification to assignment {ref}: {payload.get('message', '')}")
        elif event.event_type == "progress" and payload.get("kind") == "integration_applied":
            paths = ", ".join(payload.get("applied_paths") or []) or "(no changes needed)"
            lines.append(
                f"Integration of assignment {ref} ({payload.get('request_key')}): "
                f"{payload.get('status')}. Paths: {paths}. "
                f"host_buffer_state={payload.get('host_buffer_state')}."
            )
        elif event.event_type == "progress" and payload.get("kind") == "integration_conflict":
            conflicts = payload.get("conflict_paths") or {}
            rendered = "; ".join(f"{path}: {detail.get('reason')}" for path, detail in conflicts.items())
            line = (
                f"Integration of assignment {ref} ({payload.get('request_key')}) CONFLICTED, "
                f"nothing was written: {rendered}. host_buffer_state={payload.get('host_buffer_state')}."
            )
            repeated = payload.get("repeated")
            if repeated:
                line += (
                    f" This identical conflict has now happened {repeated} times: the worker's"
                    " work is not the problem and redoing it will not change this. Stop retrying,"
                    " and tell the user what is blocking the integration."
                )
            lines.append(line)
        elif event.event_type == "progress" and payload.get("kind") == "candidate_ready":
            lines.append(
                f"Candidate {payload.get('candidate_id')} is ready for review. "
                f"content_digest={payload.get('content_digest')}; "
                f"project_base_digest={payload.get('project_base_digest')}."
            )
        elif event.event_type == "progress" and payload.get("kind") in ("candidate_conflict", "candidate_failed"):
            lines.append(
                f"Candidate {payload.get('candidate_id')} {payload.get('kind')} "
                f"[runtime event data]: {event_data(payload.get('error', 'unknown'))}"
            )
        elif event.event_type == "progress" and payload.get("kind") == "candidate_check_observed":
            observation = (payload.get("result") or {}).get("observed_result") or {}
            lines.append(
                f"Candidate check {payload.get('check_ref')} observed for candidate "
                f"{payload.get('candidate_ref')} ({payload.get('candidate_digest')}): "
                f"status={observation.get('status')}, exit_code={observation.get('exit_code')}, "
                f"evidence_ref={(payload.get('result') or {}).get('evidence_id')}. "
                "This is a command observation, not a semantic pass assessment."
            )
        elif event.event_type == "progress" and payload.get("kind") in (
            "candidate_check_failed", "candidate_check_unknown"
        ):
            lines.append(
                f"Candidate check {payload.get('check_ref')} {payload.get('status')} "
                f"[runtime event data]: {event_data(payload.get('error', 'unknown'))}"
            )
        elif event.event_type == "progress":
            lines.append(f"Progress [event data]: {event_data(payload.get('note', ''))}")
        # delegation_requested / coordinator_decision are the coordinator's
        # own prior tool calls; they need no echo back to the coordinator.
    return "\n".join(lines)


@dataclass(frozen=True)
class _IntegrationHostBinding:
    """Minimal stand-in for ``engine.resource_binding.ResourceBinding``
    (T025): ``engine.fs_bridge.HostFsForwarder.read_text_file`` only ever
    reads ``binding.workspace_binding_id`` (its ``RevisionLog`` key) --
    coordinator-side integration has no real per-attempt ``ResourceBinding``
    to reuse, so this supplies just that one field, scoped to the task
    rather than any one attempt (a single project-root-wide "integration"
    revision-log bucket)."""

    workspace_binding_id: str


class OrchestratingRuntime:
    """Model-owned coordination loop; implements ``acp.runtime.IncomingRuntime``."""

    # T027: defensive hard ceiling on consecutive fallback hops for one
    # worker prompt. ``_attempt_worker_route_fallback`` already excludes
    # every previously-tried connection via the predecessor chain, so this
    # never binds for any realistic route-profile size; it only guards
    # against a pathological/corrupt chain ever looping unbounded.
    _MAX_FALLBACK_HOPS = 8

    def __init__(
        self,
        store,
        session_runtime: SessionRuntime,
        workspace_provisioner,
        config_snapshot_source,
        *,
        allow_unqualified_delivery: bool = False,
        max_no_progress_continuations: int = DEFAULT_MAX_NO_PROGRESS_CONTINUATIONS,
        max_total_continuations: int | None = DEFAULT_MAX_TOTAL_CONTINUATIONS,
        worker_pool_size: int = DEFAULT_WORKER_POOL_SIZE,
        wait_timeout_seconds: float = DEFAULT_WAIT_TIMEOUT_SECONDS,
        host_elicitor: HostElicitor | None = None,
        host_fs_requester: Callable[[str, Mapping[str, Any]], Mapping[str, Any]] | None = None,
    ) -> None:
        self._store = store
        self._engine = session_runtime
        try:
            import inspect
            self._engine_accepts_prompt_blocks = "prompt_blocks" in inspect.signature(session_runtime.prompt).parameters
        except (TypeError, ValueError, AttributeError):
            self._engine_accepts_prompt_blocks = False
        self._workspace = workspace_provisioner
        self._config = config_snapshot_source
        self._allow_unqualified_delivery = bool(allow_unqualified_delivery)
        # T020: optional structured-elicitation dependency (see
        # HostElicitor); None for every pre-T020 construction (production
        # wiring passes a real one only once acp.agent's per-connection
        # HostRequester exists -- see acp.production_runtime).
        self._elicitor = host_elicitor
        # T025 (lead follow-up): optional ``(method, params) -> dict``
        # outbound call on the incoming ACP connection (see
        # ``acp.host_requests.HostRequester``), supplied by the caller only
        # when the connected host actually negotiated an ``fs`` capability
        # -- exactly the same "None means not negotiated/available, never
        # guessed" convention as ``host_elicitor`` above. Used by
        # ``_integrate_assignment`` to build a per-path unsaved-buffer
        # check via the existing T014 ``engine.fs_bridge.HostFsForwarder``;
        # ``None`` here durably records ``host_buffer_state: 'unknown'`` on
        # every integration instead of silently treating "no host fs" the
        # same as "checked, no conflict".
        self._host_fs_requester = host_fs_requester
        # EXT-01 #5 fix: ``OrchestratingRuntime`` is one long-lived instance
        # per project, shared across every ACP connection for that project
        # (see this class's own docstring above and ``start_prompt``'s own
        # ``host_elicitor`` precedent) -- so a *connection-scoped* requester
        # can never be a frozen constructor default alone (a later
        # connection's own requester would never replace it). Each
        # ``start_prompt`` call registers this call's own real,
        # connection-bound ``host_permission_requester``/``host_fs_requester``
        # (built by ``acp.agent.Agent`` from that connection's own
        # ``HostRequester``/``client_capabilities``) here, keyed by
        # ``task_id`` -- exactly the same per-task registration pattern
        # ``_active_cancel_events`` already uses.  These requesters are kept
        # after a coordinator turn returns: delegated workers are
        # independent turns and can ask for a user decision after the
        # coordinator has yielded.  They are replaced by a newer connection
        # and released only when the task reaches a terminal state or the ACP
        # connection definitively closes.  Removing the permission requester
        # at the end of every coordinator turn made a worker's request
        # silently become a ``reject_once`` fallback.
        #
        # The filesystem requester has a different lifetime.  It is used by
        # the coordinator's integration turn to inspect the host's current
        # editor buffer.  Keeping that connection-bound closure after this
        # call would make a later task/turn consult a stale host transport;
        # worker turns do not use this requester.  It is therefore removed in
        # ``start_prompt``'s finally block below, while the permission
        # requester remains available for background worker decisions.
        self._task_permission_requesters: dict[str, Callable] = {}
        self._task_fs_requesters: dict[str, Callable] = {}
        self._task_requester_sessions: dict[str, str] = {}
        # T020: per-task de-duplication of identical consecutive runtime
        # status lines (never persisted -- see acp.messages.Dedup).
        self._status_dedup = acp_messages.Dedup()
        self._provisioning = ProvisioningService(
            store, workspace_provisioner, session_runtime, config_snapshot_source,
            allow_unqualified_delivery=allow_unqualified_delivery,
        )
        self._max_no_progress = max_no_progress_continuations
        self._max_total = max_total_continuations
        self._wait_timeout = wait_timeout_seconds
        self._lock = threading.Lock()
        self._cond = threading.Condition(self._lock)
        self._active_tasks: set[str] = set()
        # In-process caches (lost on process restart; see class docstring
        # and docs/acp/orchestration.md "what's deferred").
        self._coordinator_channels: dict[str, ChannelContext] = {}
        self._coordinator_sessions: dict[str, str] = {}
        self._worker_sessions: dict[tuple[str, str], str] = {}
        self._running: dict[str, set[Future]] = {}
        # In-memory "already submitted" markers, checked in addition to the
        # durable event log: the durable assignment_ready/assignment_failed/
        # assignment_cancelled/clarification-delivered marker for a given
        # ref is only written *after* the corresponding external effect
        # finishes, so relying on the DB alone would let a second
        # _process_new_events call (from a later coordinator continuation,
        # while the first effect is still in flight) submit a duplicate.
        self._dispatched_delegations: set[str] = set()
        self._dispatched_cancels: set[str] = set()
        self._dispatched_integrations: set[int] = set()
        self._dispatched_candidates: set[int] = set()
        self._dispatched_checks: set[int] = set()
        self._delivered_clarification_ids: set[int] = set()
        # T017: attempt_ids whose owning process has already been asked to
        # terminate after reaching a terminal state (worker_result or
        # assignment_failed) -- so a confirmed/ambiguous capacity release
        # actually happens once terminal, and this is only ever requested
        # once per attempt.
        self._terminated_attempts: set[str] = set()
        self._executor = ThreadPoolExecutor(max_workers=worker_pool_size, thread_name_prefix="cortex-worker")
        # T035 FINDING-13 #2: assignment refs whose *worker* engine turn is
        # in flight right now, in this process. A worker that is actually
        # running is real work with no MEANINGFUL event of its own until it
        # finishes (delegation_events.MEANINGFUL_EVENT_TYPES), and was
        # therefore indistinguishable from nothing happening -- measured
        # live: assignment_ready at 18:37:58, blocked_no_progress at
        # 18:38:12, fourteen seconds later (evidence/T035/RUN-07). Read by
        # _continuation_loop's no-progress branch under self._lock.
        # ponytail: a worker whose engine turn never returns holds this
        # entry and the coordinator waits on it -- the same unbounded wait
        # FINDING-06 already accepted for the coordinator's own turn; both
        # end on host cancel/stop, which this loop re-checks every tick.
        self._worker_turns_in_flight: set[str] = set()
        # Assignment refs whose one same-session completion follow-up is
        # owned by this runtime.  The durable progress marker survives a
        # restart; this set only distinguishes a live in-process owner from
        # a stale marker that may be safely reclaimed after a crash.
        self._worker_recoveries_in_flight: set[str] = set()
        self._worker_recovery_claim_lock = threading.Lock()
        # The most recent scrubbed excerpt of each in-flight worker's own
        # engine turn, so its card can show what it is doing rather than
        # only that it is running. Written by the worker's own on_delta,
        # read by _continuation_loop's workers_running tick -- the tick
        # that already exists, never a second channel or a second card.
        self._worker_activity: dict[str, str] = {}
        # Every action a worker has taken, in order -- the card's body,
        # which a client shows only when the card is expanded. The title
        # carries just the latest one, so the feed stays readable while
        # the detail stays one click away.
        self._worker_action_log: dict[str, list[str]] = {}
        # One rendered line per logical engine tool ref. Status updates revise
        # that line instead of appending a second progress/completion row.
        self._worker_action_refs: dict[str, dict[str, int]] = {}
        # Refs evicted from the rolling worker transcript are tombstoned for
        # bounded late-update suppression, so a stale provider update cannot
        # resurrect an old row at the end of the card.
        self._worker_action_evicted_refs: dict[str, list[str]] = {}
        # Project root used only to shorten paths in the display mirror.
        self._worker_card_project_root: dict[str, str] = {}
        self._worker_final_response_seen: set[str] = set()
        # A worker can publish its durable result from an in-flight MCP call
        # before its engine turn returns the final assistant text.  Keep that
        # terminal transition pending until the turn's final response has
        # been recorded/rendered; otherwise the terminal card fence drops
        # the late response as stale.
        self._pending_worker_results: dict[str, Mapping[str, Any]] = {}
        # Each worker card's own base title (role and work), so an
        # activity update can extend it instead of replacing it.
        self._worker_card_title: dict[str, str] = {}
        # The last card payload actually sent per assignment, so an
        # unchanged one is never sent twice.
        self._worker_card_sent: dict[str, dict[str, Any]] = {}
        # Once a worker card reaches a terminal ACP status, a late activity
        # tick from the worker executor must not move it back to
        # ``in_progress``.  The activity watcher snapshots its input before
        # emitting, so merely clearing ``_worker_activity`` on completion is
        # racy; retain this terminal fence until the task runtime is gone.
        self._worker_card_terminal: dict[str, str] = {}
        # Client surface is an explicit ACP session signal.  Paseo's Cortex
        # plugin provides the durable Workers panel; other clients retain the
        # richer lifecycle-card transcript.
        self._worker_panel_mode: dict[str, bool] = {}
        # The panel decision is made per coordinator task, but worker cards
        # are keyed by assignment.  Bind that decision at card creation so
        # later worker callbacks (which do not carry the task id) cannot
        # accidentally fall back to the full transcript presentation.
        self._worker_card_panel_mode: dict[str, bool] = {}
        # Each worker card's route line (engine, model, effort), kept so
        # every later body update still says what it is running on.
        # Worker cards this runtime has actually emitted a create for.
        # See _emit_worker_card: the one place that decides create vs
        # update, so the two can never arrive out of order again.
        self._emitted_worker_cards: set[str] = set()
        # Project revisions this runtime itself produced by applying a
        # worker's integration. See _integrate_assignment and the
        # coordinator-turn digest check.
        self._authorized_project_revisions: dict[str, list] = {}
        # The card channel of whichever start_prompt call currently owns
        # this task, so a worker running on the executor can put its own
        # actions in the same feed the coordinator is writing to.
        self._task_tool_call_emitters: dict[str, object] = {}
        # A worker owns the card feed from the coordinator turn that
        # dispatched it.  Keep this per-assignment binding instead of
        # resolving through the task's current session: a reconnect may
        # replace that task-level feed while the old worker is still running,
        # and sending its update to the new feed would create an orphan
        # ``tool_call_update`` with no preceding create.
        self._worker_card_emitters: dict[str, object] = {}
        # Assignments dispatched and not yet terminal. A live worker is
        # work in progress, not an absence of progress.
        self._open_assignments: dict[str, set] = {}
        # The client's own MCP servers for whichever session currently
        # owns each task. In memory only -- a client's env/header values
        # are its secrets and are never persisted or logged.
        self._task_mcp_servers: dict[str, list] = {}
        # T020 (stop-race fix): the cancel_event of whichever start_prompt/
        # answer_question call currently owns this task's continuation loop
        # *in this process* -- lets stop_task() signal that loop to exit
        # promptly instead of only relying on the next ordinary prompt to
        # observe stopped_at. Registered/deregistered under self._lock;
        # never more than one entry per task_id (start_prompt's own
        # SecondCoordinatorRefused guard already prevents two concurrent
        # loops for the same task in this process).
        self._active_cancel_events: dict[str, threading.Event] = {}
        # T023: set by _ensure_coordinator right after a successful
        # recovery/replacement, consumed (popped) by the next _run call for
        # this task_id to emit an honest "new-session recovery" status line
        # -- never "resume" (see recovery.coordinator_recovery module
        # docstring and acp.messages' coordinator_recovery_* keys).
        self._pending_recovery_notice: dict[str, dict] = {}
        # T028: the most recent ACP session_id -> task_id this process has
        # seen start a turn for (start_prompt/answer_question), so
        # cancel(session_id) -- called synchronously on the transport
        # reader thread the instant a session/cancel notification arrives,
        # concurrently with the in-flight turn -- can find the coordinator
        # engine session to invalidate without waiting on that turn at all.
        # Never cleared (a session_id is effectively unique for the life of
        # this process); a stale/unknown session_id is simply a no-op below.
        self._session_task: dict[str, str] = {}

    # -- IncomingRuntime -----------------------------------------------

    def start_prompt(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        source_origin: str | None = None, source_event_id: str | None = None,
        host_elicitor: HostElicitor | None = None,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
        host_mcp_servers: list | None = None,
        host_permission_requester: Callable | None = None,
        host_fs_requester: Callable[[str, Mapping[str, Any]], Mapping[str, Any]] | None = None,
        prompt_blocks: list[Mapping[str, Any]] | None = None,
        client_surface: str | None = None,
    ) -> StopReason:
        """T018: ``source_origin``/``source_event_id`` (when supplied by the
        caller -- see ``acp.agent.Agent``'s signature detection, mirroring
        T016's ``project_root``) identify *this exact call's own* already-
        durably-captured source event (``bootstrap.bootstrap_task``/
        ``append_source_event``): the coordinator's ``user_message`` event
        is deduplicated by that identity (``user_message:<origin>:<event_id>``),
        not a recomputed turn count -- two distinct source events with
        identical text remain two distinct, both-delivered user_message
        events; a replayed identical source event (same origin+event_id)
        is never delivered twice. Omitted (``None``) falls back to the
        pre-T018 session-turn-count identity for callers that do not (yet)
        thread a source event through (every existing direct-call test).

        ``host_elicitor`` (T020, per-call -- never stored as instance state:
        ``OrchestratingRuntime`` is one long-lived instance per project,
        shared across every ACP connection for that project, while a
        ``HostElicitor`` is scoped to exactly one connection's own
        ``HostRequester`` -- see ``acp.agent``) overrides the constructor's
        own ``host_elicitor`` default for this call only; ``None`` (the
        default -- every pre-T020 caller) falls back to that default.

        ``host_permission_requester``/``host_fs_requester`` (EXT-01 #5,
        per-call -- registered under ``self._lock`` into
        ``self._task_permission_requesters``/``self._task_fs_requesters``
        keyed by ``task_id``): ``host_permission_requester`` remains alive
        after this coordinator call for background worker decisions until
        terminal task/connection cleanup; ``host_fs_requester`` is scoped to
        this call and is removed when it returns. ``None`` (every pre-EXT-01
        caller) leaves this task with no live requester for this call, the
        same fail-closed behavior as before this fix existed.
        ``host_permission_requester`` reaches a worker/coordinator engine
        session's own ``session/request_permission`` relay via
        ``delegation.live_adapters.EngineSessionManagerAdapter``'s own
        engine_session_id -> task_id lookup (see
        ``EngineSessionManagerAdapter.set_runtime``); ``host_fs_requester``
        is consulted by :meth:`_make_host_fs_checker` for the unsaved-editor-
        buffer check ``delegation.integration.integrate`` performs.

        ``emit_tool_call`` (T035, per-call the same way): when given, one
        ACP ToolCall card (``session/update`` ``tool_call``/``tool_call_update``
        -- see ``acp.protocol.tool_call``) is emitted per delegated
        assignment, in addition to (T035, replacing the plain-text
        ``assignment_delegation_failed`` line for the one case a card fully
        covers -- see ``_emit_assignment_tool_call``) ``emit_update``'s own
        narration. ``None`` (every pre-T035 caller/test) falls back to the
        plain-text-only behavior unchanged."""
        elicitor = host_elicitor if host_elicitor is not None else self._elicitor
        with self._lock:
            if task_id in self._active_tasks:
                raise SecondCoordinatorRefused()
            self._active_tasks.add(task_id)
            self._active_cancel_events[task_id] = cancel_event
            self._session_task[session_id] = task_id
            self._worker_panel_mode[task_id] = str(client_surface or "").casefold() == "paseo"
            # Every new turn identifies the connection that owns future
            # worker prompts.  An explicit ``None`` replaces an older
            # connection's requester instead of accidentally relaying into
            # a stale host; production callers pass a real requester only
            # when their transport is live.
            self._task_requester_sessions[task_id] = session_id
            if host_permission_requester is not None:
                self._task_permission_requesters[task_id] = host_permission_requester
            else:
                self._task_permission_requesters.pop(task_id, None)
            if host_fs_requester is not None:
                self._task_fs_requesters[task_id] = host_fs_requester
            else:
                self._task_fs_requesters.pop(task_id, None)
            if emit_tool_call is not None:
                self._task_tool_call_emitters[task_id] = emit_tool_call
            if host_mcp_servers:
                self._task_mcp_servers[task_id] = list(host_mcp_servers)
        try:
            return self._run(task_id, session_id, content, emit_update, cancel_event,
                              source_origin=source_origin, source_event_id=source_event_id, host_elicitor=elicitor,
                              emit_tool_call=emit_tool_call,
                              emit_display_update=emit_display_update,
                              prompt_blocks=prompt_blocks)
        finally:
            with self._lock:
                self._active_tasks.discard(task_id)
                if self._active_cancel_events.get(task_id) is cancel_event:
                    del self._active_cancel_events[task_id]
                # Keep the card emitter alive for background workers.  A
                # coordinator turn commonly returns while delegated work is
                # still running; dropping this here made a later stop unable
                # to persist its terminal card update.  Terminal task/host
                # cleanup removes it through _release_task_host_requesters.
                self._task_mcp_servers.pop(task_id, None)
                # Permission requesters remain connection-bound but alive for
                # background worker attempts.  The filesystem requester is
                # only valid during this coordinator call: a later call must
                # register its own live host transport rather than reusing a
                # stale closure from the previous turn.  Do not release the
                # permission requester here; see
                # ``_release_task_host_requesters``.
                self._task_fs_requesters.pop(task_id, None)

    def cancel(self, session_id: str) -> None:
        """T028 (session/cancel, distinct from the task-wide ``/cortex-stop``
        in :meth:`stop_task`): best-effort, in-process only.

        ``cancel_event`` (set by the caller -- ``acp.agent.Agent
        ._session_cancel`` -- around this same call) is the primary path
        the continuation loop itself observes promptly between steps; that
        alone does *not* interrupt a coordinator turn already blocked
        inside a synchronous host round trip (a permission request via
        ``engine.permissions.PermissionBridge``, or a structured
        elicitation via ``HostElicitor``) -- those keep waiting on their own
        bounded timeout regardless of ``cancel_event``. This method closes
        that specific gap for the one engine session this exact turn is
        using: :meth:`SessionRuntime.invalidate_session` (a duck-typed
        optional hook -- see
        ``delegation.live_adapters.EngineSessionManagerAdapter``) marks
        that engine session's own :class:`engine.resource_binding.BindingRegistry`
        entry released, so a permission reply this bridge is still waiting
        to relay (or a late one arriving after this call) is answered
        ``cancelled`` instead of ever being relayed as if still authorized
        (see ``ResourceBinding.is_current``). Never touches any *other*
        task's or session's engine sessions, never cancels an independent,
        already-dispatched worker assignment this task delegated (scope
        isolation -- see :meth:`stop_task` for the task-wide equivalent),
        and never rolls back anything already applied.

        A ``session_id`` this process never saw start a turn (unknown, or
        only ever handled by a *different* process before a restart) is a
        documented no-op -- true out-of-band cross-process cancellation is
        not this method's job (see the historical note this replaces)."""
        task_id = self._session_task.get(session_id)
        if task_id is None:
            return None
        with self._lock:
            active_event = self._active_cancel_events.get(task_id)
        if active_event is not None:
            active_event.set()
        session_ref = self._coordinator_sessions.get(task_id)
        if session_ref is not None:
            from .. import _fault_injection

            _fault_injection.checkpoint("t028_cancel_before_permission_invalidate")
            invalidate = getattr(self._engine, "invalidate_session", None)
            if invalidate is not None:
                try:
                    invalidate(session_ref, "session_cancelled")
                except Exception:
                    pass
        return None

    def cancel_task(
        self, task_id: str, session_id: str,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> bool:
        """Cancel a task and all of its owned work from an explicit client stop.

        Paseo's ``session/cancel`` is the Workers-panel stop control.  It is
        therefore task-wide, unlike an ordinary ACP cancellation which only
        interrupts the current coordinator turn.  The notification has no
        response channel, so the callback deliberately discards the textual
        confirmation; worker-card updates remain durable and are replayed by
        ``session/load`` when the connection has already gone away.
        """
        return self.stop_task(
            task_id, session_id, lambda _text: False,
            emit_tool_call=emit_tool_call,
        )

    def disconnect_session(self, session_id: str) -> None:
        """T028 (host transport EOF/writer-closed): called once per
        session on the disconnected ACP connection (see ``acp.agent.Agent
        ._handle_eof``/``handle_writer_closed``). State up to this point is
        already durable (every mutation this runtime performs is committed
        before it is ever visible); this call itself:

        1. signals this task's in-flight turn (if any, *in this process*)
           to stop via the same ``cancel_event``/binding-invalidation path
           :meth:`cancel` uses -- STOPS starting any new paid work for this
           task by default, per the task's own "no hidden paid work"
           requirement, rather than continuing a coordinator turn no one
           can any longer observe the result of;
        2. records a durable ``progress`` event naming what was actually
           paused/cancelled/unconfirmed -- never silently continuing
           without a trace;
        3. runs the bounded owned-resource reaper sweep
           (:mod:`engine.reaper`) so any capacity reservation or isolated
           workspace a crash concurrent with the disconnect left ambiguous
           is re-probed promptly, never assumed released.

        An unknown ``session_id`` (never started a turn in this process) is
        a no-op beyond the reaper sweep, which is unconditional and safe to
        call redundantly (idempotent, bounded)."""
        task_id = self._session_task.get(session_id)
        if task_id is not None:
            # The host transport is now definitively closed.  Do not leave a
            # requester closure pointing at it: a later worker permission
            # request must fail closed rather than block on a dead pipe.
            self._release_task_host_requesters(task_id, session_id=session_id)
            with self._lock:
                active_event = self._active_cancel_events.get(task_id)
                session_ref = self._coordinator_sessions.get(task_id)
            turn_state = "cancel_event_set" if active_event is not None else "no_active_turn_in_process"
            if active_event is not None:
                active_event.set()
            permission_invalidated = False
            if session_ref is not None:
                invalidate = getattr(self._engine, "invalidate_session", None)
                if invalidate is not None:
                    try:
                        invalidate(session_ref, "host_disconnected")
                        permission_invalidated = True
                    except Exception:
                        pass
            try:
                delegation_events.append_event(
                    self._store, task_id, "progress", f"disconnect:{session_id}:{time.monotonic_ns()}",
                    dict(note="host disconnected", turn=turn_state, permission_invalidated=permission_invalidated),
                    origin="runtime",
                )
            except Exception:
                pass
        try:
            engine_reaper.sweep(self._store, self._engine)
        except Exception:
            pass

    def shutdown(self) -> None:
        """T017 (fresh review, MEDIUM): signal every queued-for-capacity
        open to abort promptly (never a reservation leak), terminate every
        owned coordinator/worker process with a confirmed/ambiguous
        release, and shut down the worker-dispatch executor. Idempotent;
        safe to call once at process/runtime teardown."""
        shutdown = getattr(self._engine, "shutdown", None)
        if shutdown is not None:
            # Capacity-aware SessionRuntime (see
            # delegation.live_adapters.EngineSessionManagerAdapter.shutdown):
            # aborts every queued open and terminates every owned process
            # with a confirmed/ambiguous release.
            shutdown()
        self._executor.shutdown(wait=False, cancel_futures=True)

    def _release_task_host_requesters(self, task_id: str, *, session_id: str | None = None) -> None:
        """Drop host request relays once no user decision can be delivered.

        The permission requester is intentionally *not* scoped to one
        coordinator turn: workers continue independently after that turn
        returns.  The filesystem requester is normally already scoped to its
        call by ``start_prompt``'s finally block; this helper also removes it
        on definitive connection close or a terminal task transition.
        """
        with self._lock:
            # A task can be resumed from a new ACP connection while an old
            # connection is still unwinding.  The old EOF must not remove
            # the new connection's live requester.
            owner = self._task_requester_sessions.get(task_id)
            if session_id is not None and owner not in (None, session_id):
                return
            self._task_permission_requesters.pop(task_id, None)
            self._task_fs_requesters.pop(task_id, None)
            self._task_requester_sessions.pop(task_id, None)
            self._task_tool_call_emitters.pop(task_id, None)

    # -- coordinator provisioning ----------------------------------------

    def _ensure_coordinator(self, task_id: str) -> tuple[ChannelContext, str]:
        cached = self._coordinator_channels.get(task_id)
        if cached is not None:
            session_ref = self._coordinator_sessions[task_id]
            # EXT-01 #2a: a cached, still-open coordinator session is
            # exactly the case the old (session-open-only) configuration
            # glue never revisited -- a mid-task model/provider change
            # (``acp.agent._apply_mid_task_selection``) only ever proposes
            # a fresh ``'selected'`` revision, it never itself re-opens this
            # session. Reconciling here, on every call (cached or not),
            # right after resolving *which* session this turn will use and
            # strictly before any prompt content for it is composed/sent
            # below (see ``_run``/``answer_question``'s own call sites),
            # is the one safe boundary: never mid-turn, which would leave
            # that turn's own effective model ambiguous.
            self._reconcile_configuration(task_id, session_ref)
            return cached, session_ref
        # T017: this runtime instance's first use of this task -- reconcile
        # any claim (consumed_by set) whose delivery was never confirmed by
        # a matching mark_delivered (a crash between claim_unconsumed and
        # engine.prompt returning in a prior process), so those events are
        # reclaimed and delivered exactly once instead of stuck forever.
        delegation_events.reconcile_undelivered_claims(self._store, task_id)
        role_set = load_role(COORDINATOR_ROLE_ID)
        preview = self._config.snapshot(task_id)
        qualify_delivery(
            role_set, delivery_for(preview["engine"], preview["version"]), [],
            allow_unqualified_delivery=self._allow_unqualified_delivery,
        )
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
                if owner is None:
                    raise PortableError(
                        category=PortableErrorCategory.AUTHORIZATION, code="task_not_owned",
                        remediation="This task has no recorded owner epoch; bootstrap the task before starting orchestration.",
                        retry_eligible=False,
                    )
                owner_epoch = owner["epoch"]
                existing = db.execute(
                    "SELECT * FROM task_orchestration WHERE task_id=?", (task_id,),
                ).fetchone()
                if existing is None:
                    attempt_ref = new_id("co_")
                    db.execute(
                        "INSERT INTO task_orchestration (task_id, state, coordinator_attempt_ref, updated_at) VALUES (?,?,?,?)",
                        (task_id, "provisioning", attempt_ref, _now()),
                    )
                    channel_id, secret = issue_channel_tx(db, task_id, "coordinator", owner_epoch, attempt_id=attempt_ref)
                else:
                    attempt_ref = existing["coordinator_attempt_ref"]
                    row = db.execute(
                        "SELECT channel_id FROM trusted_channels WHERE task_id=? AND role='coordinator' AND attempt_id=? AND revoked_at IS NULL",
                        (task_id, attempt_ref),
                    ).fetchone()
                    if row is None:
                        raise PortableError(
                            category=PortableErrorCategory.INDETERMINATE_WRITE, code="coordinator_channel_missing",
                            remediation="This task's coordinator channel is missing from a live trusted_channels row; this indicates data corruption or a process-restart limitation (see acp.orchestrating_runtime docstring).",
                            retry_eligible=False,
                        )
                    channel_id, secret = row["channel_id"], None
                db.commit()
            except BaseException:
                db.rollback()
                raise
        if secret is None:
            # T023: this process holds neither the channel secret nor an
            # open session for an already-provisioned coordinator -- almost
            # always a process restart. Run the full recovery/replacement
            # sequence (fencing, old-attempt/worker reconciliation, fresh
            # channel, new session, config/permission-carryover check --
            # see recovery.coordinator_recovery) instead of failing closed.
            from ..recovery import coordinator_recovery

            def _open_session(recovery_task_id, recovery_attempt_ref, recovery_channel_id, recovery_secret):
                return self._open_coordinator_session(
                    recovery_task_id, recovery_attempt_ref, recovery_channel_id, recovery_secret, role_set,
                    recovery=True,
                )

            outcome = coordinator_recovery.recover(
                self._store, self._engine, task_id,
                old_owner_epoch=owner_epoch, old_attempt_ref=attempt_ref, open_session=_open_session,
            )
            channel = outcome["channel"]
            session_ref = outcome["session_ref"]
            self._coordinator_channels[task_id] = channel
            self._coordinator_sessions[task_id] = session_ref
            self._pending_recovery_notice[task_id] = outcome
            self._reconcile_configuration(task_id, session_ref)
            return channel, session_ref
        from ..trusted_channel import resolve_channel

        channel = resolve_channel(self._store, channel_id, secret)
        # T017: capacity queuing (if the injected SessionRuntime is
        # capacity-aware) happens *inside* open() itself -- see
        # delegation.live_adapters.EngineSessionManagerAdapter.open's
        # docstring; this call simply blocks a bounded amount longer while
        # queued, exactly like a slow real engine launch.
        session_ref = self._open_coordinator_session(task_id, attempt_ref, channel_id, secret, role_set)
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                db.execute(
                    "UPDATE task_orchestration SET state='running', updated_at=? WHERE task_id=?",
                    (_now(), task_id),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
        self._coordinator_channels[task_id] = channel
        self._coordinator_sessions[task_id] = session_ref
        self._reconcile_configuration(task_id, session_ref)
        return channel, session_ref

    def _open_coordinator_session(
        self, task_id: str, attempt_ref: str, channel_id: str, secret: str, role_set, *, recovery: bool = False,
    ) -> str:
        preview = self._config.snapshot(task_id)
        launch = dict(
            task_id=task_id, attempt_id=attempt_ref, channel_id=channel_id,
            channel_secret=secret, config_revision=preview.get("config_revision"),
        )
        if recovery:
            # T023: tells a capacity/generation-tracking engine (see
            # delegation.live_adapters.EngineSessionManagerAdapter.open) this
            # is a coordinator *replacement*, not this attempt id's first
            # session -- cause="session_replacement", never "session_open",
            # regardless of whether this process itself ever opened this
            # (freshly allocated) attempt id before.
            launch["recovery"] = True
        instructions = composed_instructions(role_set)
        # D-T035-04: hand the coordinator the same allowed-routes table its
        # own ``delegate`` calls are checked against, so it can choose a
        # role's engine/model/effort without a round trip -- absent
        # entirely (never a placeholder) when no route policy file exists
        # anywhere, or the one that does exists is malformed (surfaced
        # separately by ``_authorize_dispatch``'s own malformed-policy
        # event, never duplicated here).
        try:
            from ..configuration import route_policy as route_policy_module

            policy, malformed = route_policy_module.load_route_policy(self._task_project_root(task_id))
            if policy is not None and malformed is None:
                # Keep the coordinator's advertised choices truthful for the
                # current isolated runtime.  A policy may mention both
                # engines while only one has a configured executable
                # connection; advertising the other causes a predictable
                # worker failure on ``engine_not_connected``.  Dispatch still
                # performs its own exact authorization check.
                from ..connections import registry as connection_registry

                registry = connection_registry.read_registry()
                available_engines = frozenset(
                    record.engine
                    for record in registry.connections.values()
                    if record.launch_command is not None
                    and getattr(record.state, "value", record.state) == "configured"
                )
                table = route_policy_module.render_table(policy, available_engines=available_engines)
                if table:
                    instructions += (
                        "\n\nAvailable worker routes per role (role: engine/model {efforts} default=effort; "
                        "pass delegate's own engine/model/effort to choose one; omit to use the first "
                        "listed available route for that role):\n" + table + "\n"
                    )
        except Exception:  # noqa: BLE001 - never block coordinator startup over this
            pass
        session_id = self._engine.open("shared_project_root", launch, instructions)
        # T035 FINDING-02 fix (superseded, EXT-01 #2): ``self._engine.open``
        # (see ``delegation.live_adapters.EngineSessionManagerAdapter
        # .reconcile_configuration``) runs the coordinator's own first-turn
        # configuration confirmation synchronously *inside* this call, and
        # deliberately never re-raises an unconfirmed outcome into open()'s
        # own caller -- an already-opened engine *session* is not itself
        # aborted over a configuration refusal (see that method's own
        # docstring). This cheap, preview-time check catches the common
        # case (a fresh session's own open() just settled 'failed'/
        # 'rejected') immediately; ``_ensure_coordinator``'s own
        # ``_reconcile_configuration`` call right after this method returns
        # is the authoritative, always-run gate (it also covers
        # 'indeterminate', and re-reads the *current* revision rather than
        # this snapshot, which can be stale for a cached-session turn --
        # see that method's own docstring).
        self._reject_if_configuration_failed(task_id, preview.get("config_revision"))
        return session_id

    def _reject_if_configuration_failed(self, task_id: str, revision_id: str | None) -> None:
        """Raise a :class:`~..portable.errors.PortableError` carrying the
        engine's own refusal reason when ``revision_id`` has settled into
        'failed' or 'rejected' with a real (non-benign) error_code --
        never for any other state (see :meth:`_open_coordinator_session`'s
        call site for why 'selected' must never be gated here). The raised
        error's ``code`` is that revision's own recorded ``error_code``
        verbatim (falling back to a generic code only if none was
        recorded), so it reaches the client through the exact same
        ``data.reason`` channel ``acp.production_runtime.RouteUnresolved``
        already uses for ``no_configuration_revision``/
        ``instruction_delivery_unqualified`` -- never a silent
        substitution.

        Superseded (but not removed -- still a cheap, harmless early check
        right after a fresh/recovered open) as the authoritative gate by
        :meth:`_reconcile_configuration`/``EngineSessionManagerAdapter
        .reconcile_configuration``, EXT-01 #2c, which additionally blocks
        'indeterminate' and runs on every turn, not only session-open."""
        if not revision_id:
            return
        with self._store.connection() as db:
            row = db.execute(
                "SELECT state, error_code FROM configuration_revisions WHERE id=?", (revision_id,),
            ).fetchone()
        if row is None or row["state"] not in ("failed", "rejected"):
            return
        if row["error_code"] in BENIGN_CONFIGURATION_ERROR_CODES:
            return
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code=row["error_code"] or "configuration_apply_failed",
            remediation="This task's configuration revision was refused by the engine; select and apply a "
                        "working configuration before starting a new turn.",
            retry_eligible=False,
        )

    def _reconcile_configuration(self, task_id: str, session_ref: str) -> None:
        """EXT-01 finding #2: the authoritative, always-run configuration
        gate for a coordinator turn -- called by :meth:`_ensure_coordinator`
        every time it resolves a session (a reused cached one, a freshly
        opened one, or a recovered one alike), strictly before returning to
        its own caller (``_run``/``answer_question``), both of which then
        go on to compose and send this turn's own prompt content -- i.e.
        exactly the one safe per-turn boundary: never mid-turn, which would
        leave that turn's own effective model ambiguous.

        Delegates the actual per-session wire (re)confirmation to the
        session runtime's own ``reconcile_configuration(task_id,
        session_ref)`` (real production case:
        ``delegation.live_adapters.EngineSessionManagerAdapter
        .reconcile_configuration`` -- see its own docstring for exactly how
        it closes all three EXT-01 #2 cases (a mid-task selection on an
        already-cached session, a fresh session after restart/recovery
        whose task-level revision was already 'active', and a partial/
        indeterminate application) -- looked up via ``getattr`` exactly
        like ``shutdown`` above, so every existing ``SessionRuntime`` test
        double that does not implement it (i.e. carries no real
        configuration behind it at all) is an unchanged no-op, never a
        ``AttributeError``.

        That call returns a :class:`~..portable.errors.PortableError`
        (never raises itself) when this session's own confirmation for the
        task's current revision is refused or unconfirmed; this method
        raises it, blocking the turn -- reusing the existing
        'failed'/'rejected'/'indeterminate' vocabulary, never a fourth
        state."""
        reconcile = getattr(self._engine, "reconcile_configuration", None)
        if reconcile is None:
            return
        error = reconcile(task_id, session_ref)
        if error is not None:
            raise error

    # -- T022: fresh-generation capsule/role/tool-schema delivery --------

    def _role_id_for_pending(self, task_id: str, pending: Mapping[str, Any]) -> str | None:
        """Resolve the role_id for a pending worker generation from its
        original ``delegation_requested`` event payload -- the same source
        ``recovery.capsule._section_role_and_tool_access`` reads. Returns
        ``None`` (no redelivery attempted) only if that event was somehow
        never recorded -- never guesses a role."""
        generation = pending["generation"]
        assignment_ref = generation.get("assignment_ref")
        if pending["recipient"] == "coordinator":
            return COORDINATOR_ROLE_ID
        if not assignment_ref:
            return None
        import json as _json

        with self._store.connection() as db:
            row = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='delegation_requested' ORDER BY id LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone()
        if row is None:
            return None
        return _json.loads(row["payload_json"]).get("role")

    def _resolve_context_window_tokens(self, task_id: str, preview: Mapping[str, Any]) -> int | None:
        """T024: the real, evidenced context-window budget for this task's
        currently selected route, when the catalog actually recorded one
        (``catalog.model.CatalogEntry.context_window_tokens`` -- see that
        module; absent for every route no discovery path has populated it
        for yet). Best-effort/never raises: any lookup failure (no route
        selected, no cached snapshot, a test double with no catalog state
        at all) falls back to :mod:`recovery.token_budget`'s labeled
        conservative default, never a guessed number."""
        try:
            from ..catalog import store as catalog_store

            selection = catalog_store.current_task_route(self._store, task_id)
            if selection is None:
                return None
            connection_id = preview.get("route_connection_id")
            if not connection_id:
                return None
            view = catalog_store.load_connection_snapshot(connection_id, selection["catalog_snapshot_id"])
            if view is None:
                return None
            for entry in view.entries:
                if entry.route.id == selection["route_id"]:
                    return entry.context_window_tokens
        except Exception:
            return None
        return None

    def _block_generation_delivery(
        self, task_id: str, session_ref: str, generation: Mapping[str, Any], capsule_revision: str,
        reason: str, message_key: str, message_params: Mapping[str, Any],
        emit_update: Callable[[str], bool] | None, language: str | None,
    ) -> bool:
        """Shared blocked-delivery path for both a ``capsule_oversize`` and a
        T024 ``budget_overflow`` condition (see
        :meth:`_block_generation_immediately` for the distinct, already-
        bounded ``compaction_blocked`` case): begin/re-check a delivery row
        for ``capsule_revision`` and durably record + (when a real channel
        is available) emit a localized explicit message -- never silently.

        EXT-01 #6: mandatory context that does not fit blocks the engine
        call on the *first* attempt, not only once :data:`MAX_DELIVERY_RETRIES`
        is exhausted -- absence of required context is never a transient
        condition to run the turn through while retries remain. This method
        therefore always returns ``True`` (the caller must not prompt the
        engine this turn). Retrying is still legitimate -- a rebuilt
        capsule, a different presentation, or the coordinator being told
        the assignment must change, each naturally tried again on a later
        turn via the ordinary continuation loop -- so while bounded retries
        remain the delivery row is only ``blocked`` (not ``permanent``, see
        ``begin_delivery``'s reopen-to-``built`` rule) and a live "will
        retry" notice goes straight to ``emit_update`` (the *transport*
        callback for this turn -- no store write). It is deliberately never
        routed through ``_set_state``/``_emit_localized``: both durably
        touch ``task_orchestration``/``task_events``, which
        ``recovery.capsule``'s own section builders and
        ``_high_watermark`` fold straight into ``capsule_revision`` (see
        that module's ``continuation_intent.orchestration`` section and its
        docstring on ``source_high_watermark``) -- recording the retry
        itself would mint a fresh ``capsule_revision`` on every attempt,
        making this exact "still overflowing on the *same* content" check
        (keyed on an unchanged ``capsule_revision``) never accumulate
        ``retry_count`` and never reach :data:`MAX_DELIVERY_RETRIES` at all.
        Only once retries are exhausted does the block become permanent,
        durably recorded with a ``limit:`` prefix and the plain overflow
        message via the normal task_orchestration/emit_update path -- safe
        precisely because the pending generation is cleared at that point,
        so no further capsule build for it will ever observe the change.
        The two outcomes are therefore always distinguishable: a live-only
        "will retry" notice (this turn) versus a durable "permanently
        blocked" task state (every future read)."""
        delivery = self._engine.begin_delivery(session_ref, capsule_revision)
        if delivery is None:
            return False
        # T024: check the row's *current* retry_count directly, regardless
        # of whether ``begin_delivery`` just reopened it to ``built`` (this
        # attempt) or returned it unchanged still ``blocked`` (a caller
        # that fell through from an already-exhausted row -- see
        # ``_begin_generation_delivery``'s own state check) -- either shape
        # carries the accurate count for "how many times has this exact
        # generation/capsule_revision been retried".
        if delivery["retry_count"] >= MAX_DELIVERY_RETRIES:
            self._engine.mark_delivery_blocked(session_ref, delivery["id"], reason, permanent=True)
            explanation = acp_messages.render(message_key, language or "en", **message_params)
            self._set_state(task_id, "blocked", wait_reason=f"limit:{reason}", wait_detail=dict(explanation=explanation))
            if emit_update is not None:
                self._emit_localized(task_id, emit_update, language or "en", message_key, **message_params)
            return True
        self._engine.mark_delivery_blocked(session_ref, delivery["id"], reason)
        if emit_update is not None:
            retry_key = f"{message_key}_retry"
            retry_params = dict(message_params, retry_count=delivery["retry_count"], max_retries=MAX_DELIVERY_RETRIES)
            emit_update(acp_messages.render(retry_key, language or "en", **retry_params))
        return True

    def _block_generation_immediately(
        self, task_id: str, session_ref: str, capsule_revision: str, reason: str,
        message_key: str, message_params: Mapping[str, Any],
        emit_update: Callable[[str], bool] | None, language: str | None,
    ) -> None:
        """T024: unlike :meth:`_block_generation_delivery`'s bounded-retry
        path (appropriate when redelivering the *same* content again next
        turn might legitimately differ, e.g. task state changed), a
        ``recovery.compaction_guard`` block is already the final verdict of
        its own bounded window/ratio history -- it is recorded and
        surfaced permanently on the very first sighting, never after a
        second round of otherwise-pointless retries."""
        delivery = self._engine.begin_delivery(session_ref, capsule_revision)
        if delivery is not None:
            self._engine.mark_delivery_blocked(session_ref, delivery["id"], reason, permanent=True)
        explanation = acp_messages.render(message_key, language or "en", **message_params)
        self._set_state(task_id, "blocked", wait_reason=f"limit:{reason}", wait_detail=dict(explanation=explanation))
        if emit_update is not None:
            self._emit_localized(task_id, emit_update, language or "en", message_key, **message_params)

    def _begin_generation_delivery(
        self, task_id: str, session_ref: str, role_id: str, prompt_text: str,
        *, emit_update: Callable[[str], bool] | None = None, language: str | None = None,
    ) -> tuple[str, str | None, bool]:
        """If ``session_ref`` has a queued fresh context generation (see
        ``delegation.live_adapters.EngineSessionManagerAdapter.pending_delivery``
        -- an optional ``SessionRuntime`` extension, never required),
        prefix ``prompt_text`` with the T021 capsule plus current role
        instructions (via the T015 delivery mechanism, which also
        re-asserts current tool-schema access -- see
        ``recovery.capsule._section_role_and_tool_access``) as the first
        blocks of the continuation for that generation, and mark the
        delivery ``built``/``transmitted``.

        T024: before transmitting, plans the full mandatory budget (role
        instructions + tool schemas + capsule's critical sections + chat/
        continuation + reserves -- see ``recovery.token_budget.plan_budget``)
        against the route's real (catalog) or conservatively-estimated
        context window, and -- only for a generation whose ``cause`` is an
        actual observed/controlled compaction, never an ordinary
        session_open/session_replacement -- runs the compaction-loop guard
        (``recovery.compaction_guard.evaluate_compaction``). A ``fits``
        budget proceeds unchanged; ``needs_reference_mode`` applies the
        budget-selected compact role presentation
        (``recovery.presentation.select_presentation``) rather than
        dropping any mandatory content; ``overflow`` or a compaction-guard
        block goes through :meth:`_block_generation_delivery`'s identical
        first-attempt-blocks path the pre-existing ``capsule_oversize`` case
        already uses -- never a silent loop, never a turn run without the
        mandatory content, and never a silently truncated constraint.

        Returns ``(prompt_text, delivery_id, blocked)``. ``delivery_id`` is
        ``None`` when nothing was pending, blocked (retriable or
        permanent), or the engine offers no delivery tracking at all (every
        pre-T022 fake/test session runtime). ``blocked`` is ``True``
        whenever this exact generation could not be packaged to fit this
        turn -- whether still within :data:`MAX_DELIVERY_RETRIES` (a live
        "will retry" notice on ``emit_update`` this turn, no durable
        ``task_orchestration`` state change, and the pending generation
        kept for a later attempt) or permanently exhausted (durable
        ``blocked`` task state with a ``limit:`` prefix, pending generation
        cleared) -- the caller must end the turn without prompting the
        engine either way (EXT-01 #6)."""
        pending_delivery = getattr(self._engine, "pending_delivery", None)
        if pending_delivery is None:
            return prompt_text, None, False
        pending = pending_delivery(session_ref)
        if not pending:
            return prompt_text, None, False
        generation = pending["generation"]
        recipient = pending["recipient"]
        if recipient == "worker" and not (generation.get("assignment_ref") and pending.get("attempt_id")):
            # A worker capsule recipient requires both a recorded
            # assignment_ref and attempt_id (see
            # recovery.capsule._normalize_recipient); a generation that
            # somehow lacks either (never expected in practice -- this is
            # defense in depth) is never redelivered rather than raising.
            return prompt_text, None, False
        recipient_map: dict[str, Any] = dict(role=recipient)
        if recipient == "worker":
            recipient_map["assignment_ref"] = generation["assignment_ref"]
            recipient_map["attempt_id"] = pending["attempt_id"]
        try:
            capsule = build_capsule(self._store, task_id, recipient_map)
        except StoreError as error:
            if str(error) != "capsule_oversize":
                raise
            blocked = self._block_generation_delivery(
                task_id, session_ref, generation, f"oversize:{generation['id']}", "capsule_oversize",
                "budget_overflow", dict(
                    limit_name="capsule_bytes", context_window_tokens=0, context_window_basis="estimated",
                    message="The recovery capsule's own critical sections exceed the hard byte cap.",
                ),
                emit_update, language,
            )
            return prompt_text, None, blocked
        delivery = self._engine.begin_delivery(session_ref, capsule["capsule_revision"])
        if delivery is None:
            return prompt_text, None, False
        if delivery["state"] not in ("built", "blocked"):
            # Already transmitted/confirmed/continuation_observed for this
            # exact (recipient, generation, capsule_revision) -- redelivery
            # is idempotent, never resent.
            return prompt_text, None, False
        # T024: a still-``blocked`` row here means ``record_built`` refused
        # to reopen it -- its bounded retries (see
        # ``_block_generation_delivery``) are already exhausted. Never
        # silently short-circuit that case: fall through and recompute
        # today's decision fresh (deterministic given unchanged task state)
        # so the eventual permanent-block message stays accurate, then
        # ``_block_generation_delivery``'s own retry-count check finalizes
        # it below instead of looping forever.
        from dataclasses import replace

        role_set = load_role(role_id)
        # A generation belongs to an opened engine session.  Task-level
        # configuration can describe a different coordinator route from a
        # worker's route, especially in a mixed Codex/Claude team.
        preview = self._config.snapshot(task_id)
        actual_engine = pending.get("engine", preview["engine"])
        actual_version = pending.get("version", preview["version"])
        instructions = pending.get("instructions") or composed_instructions(role_set)
        delivery_role = replace(role_set, body_portable=instructions)
        plan = qualify_delivery(
            delivery_role, delivery_for(actual_engine, actual_version), [],
            allow_unqualified_delivery=self._allow_unqualified_delivery,
        )
        capsule_text = render_capsule(capsule)
        if plan.qualification == DeliveryQualification.UNSUPPORTED:
            role_blocks_text = ""
        else:
            role_blocks_text = "\n\n".join(
                block.get("text", "") for block in plan.blocks if isinstance(block, dict) and block.get("type") == "text"
            )

        # -- T024: budget planning + compaction-loop guard ----------------
        from ..delegation import contracts as delegation_contracts
        from ..recovery import compaction_guard, token_budget
        from ..recovery.presentation import select_presentation

        size_estimate = capsule.get("size_estimate", {})
        critical_bytes = size_estimate.get("critical_bytes", len(capsule_text.encode("utf-8")))
        total_capsule_bytes = size_estimate.get("bytes", critical_bytes)
        reference_bytes = max(total_capsule_bytes - critical_bytes, 0)
        try:
            tool_schemas_bytes = len(json.dumps(delegation_contracts.schemas_for(recipient), sort_keys=True).encode("utf-8"))
        except StoreError:
            tool_schemas_bytes = 0
        context_window_tokens = self._resolve_context_window_tokens(task_id, preview)
        decision = token_budget.plan_budget(
            context_window_tokens=context_window_tokens,
            role_instructions_bytes=len(role_blocks_text.encode("utf-8")),
            tool_schemas_bytes=tool_schemas_bytes,
            chat_history_bytes=len(prompt_text.encode("utf-8")),
            capsule_bytes=critical_bytes,
            reference_candidate_bytes=reference_bytes,
        )
        if decision.decision == "overflow":
            blocked = self._block_generation_delivery(
                task_id, session_ref, generation, capsule["capsule_revision"], "budget_overflow",
                "budget_overflow", dict(
                    limit_name=decision.limit_name, context_window_tokens=decision.context_window_tokens,
                    context_window_basis=decision.context_window_basis, message=decision.message,
                ),
                emit_update, language,
            )
            return prompt_text, None, blocked
        if decision.decision == "needs_reference_mode" and role_blocks_text:
            # Large *evidence* (bounded non-critical capsule content, e.g. a
            # long publications/checks list) is already delivered as an
            # addressable ref by the capsule's own section builders (see
            # recovery.capsule's _MAX_LIST_ENTRIES bounding); this only
            # additionally compacts the role presentation to leave more
            # headroom, never touching a mandatory constraint/acceptance
            # criterion (see recovery.presentation module docstring).
            remaining = max(decision.context_window_tokens - (decision.mandatory_tokens - token_budget.estimate_tokens(len(role_blocks_text.encode("utf-8")))), 0)
            presentation = select_presentation(
                role_text=role_blocks_text, brief_text="", acceptance_text="", remaining_budget_tokens=remaining,
            )
            role_blocks_text = presentation.role_text

        if generation.get("cause") in ("observed_compaction", "observed_reset", "controlled_boundary_refresh"):
            last_usage = getattr(self._engine, "last_usage", None)
            usage = last_usage(session_ref) if last_usage is not None else None
            if usage and usage.get("tokens") is not None:
                before_tokens, before_basis = usage["tokens"], usage.get("basis", token_budget.ENGINE_REPORTED_BASIS)
            else:
                before_tokens, before_basis = token_budget.estimate_tokens(total_capsule_bytes), token_budget.ESTIMATED_BASIS
            usable_window = int(decision.context_window_tokens * token_budget.SAFETY_MARGIN_FRACTION)
            guard_result = compaction_guard.evaluate_compaction(
                self._store, task_id=task_id, engine_session_ref=session_ref, generation_id=generation["id"],
                before_tokens=before_tokens, before_basis=before_basis,
                after_tokens=decision.mandatory_tokens, after_basis=token_budget.ESTIMATED_BASIS,
                mandatory_tokens_after=decision.mandatory_tokens, usable_window_tokens=usable_window,
            )
            if guard_result["status"] == "blocked":
                self._block_generation_immediately(
                    task_id, session_ref, capsule["capsule_revision"], guard_result["reason"],
                    "compaction_blocked", dict(reason=guard_result["reason"], message=guard_result["message"]),
                    emit_update, language,
                )
                return prompt_text, None, True

        prefix = f"{role_blocks_text}\n\n{capsule_text}" if role_blocks_text else capsule_text
        new_prompt_text = f"{prefix}\n\n---\n\n{prompt_text}"
        self._engine.mark_delivery_transmitted(session_ref, delivery["id"])
        from .. import _fault_injection

        _fault_injection.checkpoint("t023_after_capsule_delivery_transmitted")
        return new_prompt_text, delivery["id"], False

    def _finalize_generation_delivery(self, session_ref: str, delivery_id: str | None, *, failure_kind: str | None, stop_reason: StopReason | None) -> None:
        if delivery_id is None:
            return
        if failure_kind is not None:
            # T022: receipt timeout / transport failure -- the row stays
            # 'transmitted' with uncertainty recorded, never promoted to
            # 'delivery_confirmed' on a guess.
            self._engine.mark_delivery_uncertain(session_ref, delivery_id, f"engine_turn_failed:{failure_kind}")
            return
        if stop_reason in (StopReason.MAX_TOKENS, StopReason.MAX_TURN_REQUESTS):
            # T022: immediate overflow right after delivery -- explicit
            # blocked state with a reason; bounded retries happen on the
            # next ordinary continuation (see _begin_generation_delivery /
            # delegation.deliveries.MAX_DELIVERY_RETRIES), never a loop
            # within this same turn.
            self._engine.mark_delivery_blocked(session_ref, delivery_id, f"engine_limit_after_delivery:{stop_reason.value}")
            return
        self._engine.mark_delivery_confirmed(session_ref, delivery_id)

    # -- main loop ----------------------------------------------------

    def _publish_initial_pipeline(self, task_id: str, channel, request_text: str) -> None:
        """Best-effort route context for the runtime-published initial
        pipeline (see ``pipeline_bootstrap``): the bound engine/model, and
        the effort this task's current configuration revision actually
        recorded, when one is already resolvable. Never raises -- a failure
        building this optional context still lets the pipeline publish with
        an honest "not yet determined" route line (pipeline_bootstrap's own
        fallback) rather than blocking the turn.
        """
        # ponytail: ``configuration_route_extras`` records engine/model but
        # never a resolved "effort" value (see routing.store's own schema
        # docstring -- reasoning effort lives only in per-attempt engine
        # parameters, never durably joined back to a route here), so this
        # is honestly left unknown rather than guessed; add a real lookup
        # if a durable source for it ever exists.
        route_info = None
        try:
            preview = self._config.snapshot(task_id)
            model = None
            revision_id = preview.get("config_revision")
            if revision_id:
                extras = get_route_extras(self._store, revision_id)
                if extras is not None:
                    model = extras["route_model"]
            route_info = dict(engine=preview.get("engine"), model=model, effort=None)
        except Exception:
            route_info = None
        pipeline_bootstrap.publish_initial_pipeline(self._store, task_id, channel, request_text, route_info)

    def _run(
        self, task_id, session_id, content, emit_update, cancel_event, *,
        source_origin: str | None = None, source_event_id: str | None = None,
        host_elicitor: HostElicitor | None = None,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
        prompt_blocks: list[Mapping[str, Any]] | None = None,
    ) -> StopReason:
        # T020: an urgent /cortex-stop takes precedence over everything --
        # a stopped task never resumes through an ordinary prompt/steering/
        # answer. This message is durably recorded (never silently dropped)
        # but pre-marked superseded so it can never be claimed/delivered.
        if acp_steering.is_stopped(self._store, task_id):
            acp_steering.record_post_stop_message(
                self._store, task_id, source_origin or session_id, source_event_id or new_id("post_"), content,
            )
            language = self._resolve_language(task_id, content_hint=content)
            emit_update(acp_messages.render("task_stopped", language))
            return StopReason.END_TURN
        # T020: a task_orchestration row already existing *before* this call
        # opens/reuses the coordinator session means this is a follow-up
        # prompt on an already-started task -- steering-eligible (see
        # below), as opposed to the task's very first prompt.
        pre_existing = acp_steering.orchestration_exists(self._store, task_id)
        channel, session_ref = self._ensure_coordinator(task_id)
        if not pre_existing:
            # Runtime-owned record, no model turn spent on it (see
            # pipeline_bootstrap module docstring). Gated on ``not
            # pre_existing`` only as a cheap skip for every later turn --
            # publish_initial_pipeline's own durable check (the task's
            # ``reports`` row) is the actual idempotency guarantee, so a
            # second first-turn (recovery, replacement session) is still
            # safe even if this in-process flag were ever wrong.
            self._publish_initial_pipeline(task_id, channel, content)
        recovery_notice = self._pending_recovery_notice.pop(task_id, None)
        if recovery_notice is not None:
            notice_language = self._resolve_language(task_id, content_hint=content)
            if recovery_notice.get("route_changed"):
                self._emit_localized(
                    task_id, emit_update, notice_language, "coordinator_recovery_new_engine",
                    engine=recovery_notice.get("engine") or "?",
                )
            else:
                self._emit_localized(task_id, emit_update, notice_language, "coordinator_recovery_new_session")
        # T020: route this prompt against any still-open coordinator
        # question *before* recording it as an ordinary user_message/
        # steering turn -- see docs/acp/steering-and-questions.md's
        # "question lifecycle" section. Two or more open questions is never
        # guessed at: the user is asked to disambiguate with
        # /cortex-answer, and this prompt's text is not itself recorded as
        # an answer or as steering.
        open_questions = questions_store.list_open(self._store, task_id)
        language = self._resolve_language(task_id, content_hint=content)
        if len(open_questions) >= 2:
            listing = "\n".join(f"{index + 1}. {q.text}" for index, q in enumerate(open_questions))
            emit_update(acp_messages.render("question_ambiguous", language, questions=listing))
            return StopReason.END_TURN
        answering_question = open_questions[0] if len(open_questions) == 1 else None
        # T017: a new user prompt is a fresh turn -- surface any explanation
        # a prior blocked/limited/refused turn left behind (resumable, not
        # repeated once delivered), and reset the durable no-progress
        # counter for this fresh attempt (see delegation.events_store's
        # T017 column docstring).
        self._emit_resume_context(task_id)
        # T018: recover any source_events durably captured (bootstrap.
        # bootstrap_task/append_source_event) but never recorded as a
        # user_message task_events row -- a crash between capture and
        # recording in a prior process/attempt -- *before* this call's own
        # message, in captured order, each labeled 'recovered: true'. Never
        # creates a task/assignment; a source event already recorded (the
        # normal case) is a no-op here.
        self._recover_unrecorded_source_events(task_id, exclude=(source_origin, source_event_id))
        from . import replay_store

        if source_origin is not None and source_event_id is not None:
            dedup_key = f"user_message:{source_origin}:{source_event_id}"
            ledger_origin, ledger_event_id = source_origin, source_event_id
        else:
            with self._store.connection() as db:
                turn_index = db.execute(
                    "SELECT COUNT(*) AS n FROM task_events WHERE task_id=? AND event_type='user_message' AND origin=?",
                    (task_id, session_id),
                ).fetchone()["n"]
            dedup_key = f"user_message:{session_id}:{turn_index}"
            # No real source identity was threaded through (a caller not
            # yet updated for T018) -- nothing to mark in the delivery
            # ledger, but still recorded through the same atomic path.
            ledger_origin, ledger_event_id = None, None
        answer_text = _question_answer_value(answering_question, content) if answering_question is not None else content
        if answering_question is not None:
            user_message_event, answered = replay_store.record_user_message_answer_event(
                self._store, task_id, dedup_key, session_id,
                dict(content=content, session_id=session_id), question_id=answering_question.id,
                answer_text=answer_text, ledger_origin=ledger_origin, ledger_event_id=ledger_event_id,
            )
            if not answered:
                emit_update(acp_messages.render("question_already_answered", language))
                return StopReason.END_TURN
        else:
            user_message_event = replay_store.record_user_message_event(
                self._store, task_id, dedup_key, session_id, dict(content=content, session_id=session_id),
                ledger_origin=ledger_origin, ledger_event_id=ledger_event_id,
            )
        # T020: this exact prompt is either (a) the answer to the single
        # open question, (b) steering on an already-active task, or (c) the
        # task's ordinary first turn -- never more than one of these for
        # the same prompt. Recorded *after* the durable user_message above
        # (itself already persisted before any delivery -- see the T018
        # dedup_key/record_user_message_event call above) so "steering
        # persists before delivery" holds for both the coalescing text and
        # this extra bookkeeping event.
        if answering_question is not None:
            delegation_events.append_event(
                self._store, task_id, "progress", f"question_answered:{answering_question.id}",
                dict(kind="question_answered", question_id=answering_question.id, answer_text=answer_text),
                origin="runtime",
            )
        elif pre_existing:
            acp_steering.record_steering(
                self._store, task_id, source_origin or session_id, source_event_id or dedup_key, content,
            )
        return self._continuation_loop(
            task_id, session_id, channel, session_ref, answer_text, emit_update, cancel_event, language, host_elicitor,
            emit_tool_call=emit_tool_call,
            emit_display_update=emit_display_update,
            prompt_blocks=prompt_blocks,
            initial_project_docs_context=(
                render_project_docs_catalog(self._task_project_root(task_id))
                if not pre_existing else None
            ),
        )

    def _continuation_loop(
        self, task_id, session_id, channel, session_ref, content, emit_update, cancel_event, language,
        host_elicitor: HostElicitor | None = None, *,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
        prompt_blocks: list[Mapping[str, Any]] | None = None,
        initial_project_docs_context: str | None = None,
    ) -> StopReason:
        """The coordinator continuation loop proper: claim/coalesce/prompt/
        process, repeated until a terminal outcome. Factored out of
        :meth:`_run` (T020) so :meth:`answer_question` -- an out-of-band
        ``/cortex-answer`` delivery that must never re-run the open-question
        gating or steering/first-turn detection in :meth:`_run`'s preamble
        (its own answer event is already durably recorded by the caller) --
        can drive the exact same loop starting from an already-ensured
        coordinator session."""
        # Everything this loop sends to the client is a separate block: a
        # continuation turn's streamed text, a localized notice, a blocked
        # explanation. They arrived concatenated ("...before
        # delegating.Pipeline published.", "First turn text.Blocked: ...")
        # because each was emitted as a bare chunk with nothing between
        # them. One blank line before every block except the first, and
        # none *inside* a turn's own run of deltas, is the whole fix --
        # ``emit_update`` is rebound here so every existing emission in
        # this loop gets it without a second, parallel emit path.
        _raw_emit_update = emit_update
        _emit_state = dict(any=False, in_delta_run=False)

        def emit_update(text, *, delta=False):  # noqa: F811 -- deliberate rebind, see above
            if _emit_state["any"] and not (delta and _emit_state["in_delta_run"]):
                _raw_emit_update("\n\n")
            _emit_state["any"] = True
            _emit_state["in_delta_run"] = delta
            return _raw_emit_update(text)

        no_progress = 0
        # Per-user-prompt engine-call count. This is intentionally local to
        # one start_prompt invocation: a later ACP session/prompt gets its
        # own first-call image delivery, while continuation calls within the
        # same prompt do not replay the attachment.
        total = 0
        first = True
        # T035 FINDING-13 #2: the last-observed delegation_provisioning
        # in-progress signature (see delegation_provisioning
        # .in_progress_snapshot) -- None until the first no-progress wait
        # tick actually checks it, so the very first tick after dispatch
        # never spuriously counts as "no change" before anything was ever
        # observed.
        last_provisioning_sig = None
        # A direct-project worker can edit while the coordinator's prompt is
        # still running, yet publish_result may arrive much later. Keep the
        # turn's event boundary until that worker either publishes an exact
        # observed final tree or closes without one; do not mislabel its
        # in-progress edit as a coordinator write at prompt return.
        pending_direct_change = self._pending_direct_worker_change(task_id)
        while True:
            if cancel_event.is_set():
                if acp_steering.is_stopped(self._store, task_id):
                    # T020: an urgent /cortex-stop (possibly issued from a
                    # *different* session/thread in this same process --
                    # see stop_task(), which looks this exact cancel_event
                    # up via self._active_cancel_events and sets it) always
                    # takes precedence: stop_task() already set the
                    # authoritative "stopped" state and is cancelling every
                    # assignment itself -- this loop just exits without
                    # overwriting that state or delivering anything further.
                    return StopReason.CANCELLED
                # T020: session/cancel interrupts *this coordination turn*
                # only; it never cancels/terminates an already-*running*
                # assignment (that would need SessionRuntime.terminate_owned/
                # cancel_attempt, never called here -- see stop_task() for
                # the urgent, everything-cancelling /cortex-stop path
                # instead) and the task stays active ("interrupted", not
                # "cancelled"): worker events keep accumulating durably and
                # the next ordinary prompt delivers them coalesced with any
                # steering in one continuation (see
                # docs/acp/steering-and-questions.md). The one exception
                # (T017, pre-existing, unchanged by this task): a worker
                # still only *queued* for capacity (its engine session
                # never actually opened yet) must still observe task
                # cancellation promptly rather than leak that reservation
                # forever -- cancel_task() only affects that queued-open
                # wait loop (delegation.capacity/EngineSessionManagerAdapter),
                # never an already-opened session's in-flight prompt.
                cancel_task = getattr(self._engine, "cancel_task", None)
                if cancel_task is not None:
                    cancel_task(task_id)
                self._set_state(
                    task_id, "interrupted", wait_reason="interrupted",
                    wait_detail=dict(explanation="Coordination turn interrupted by session/cancel; already-running workers keep running."),
                )
                return StopReason.CANCELLED
            continuation_id = new_id("cont_")
            claimed = delegation_events.claim_unconsumed(self._store, task_id, continuation_id)
            # T019: capture this batch's worker_result events' observed
            # artifact revision now, at the moment this runtime processes
            # them (see delegation.revision module docstring on the shared-
            # workspace-root limitation). Never blocks/fails the turn: an
            # unavailable capture is recorded and labeled "unknown", not
            # refused (refusal is the completion gate's job, at acceptance
            # time -- see delegation.completion.gate).
            revisions = self._capture_result_revisions(task_id, claimed)
            # T035: one ACP tool-call card per delegated assignment,
            # queued/running/finished/failed/cancelled -- see
            # _emit_assignment_tool_calls. Runs over the FULL claimed batch
            # (delegation_requested/assignment_ready are informative-only
            # for the coordinator's own continuation prompt below -- see
            # MEANINGFUL_EVENT_TYPES -- but are exactly the "queued"/
            # "running" transitions this card lifecycle needs).
            self._emit_assignment_tool_calls(task_id, emit_tool_call, claimed)
            if pending_direct_change is not None:
                current_revision = delegation_revision.capture_revision(self._task_project_root(task_id))
                if pending_direct_change[1] >= 0 and self._direct_worker_publication_matches(
                    task_id, pending_direct_change[1], current_revision,
                ):
                    delegation_events.append_event(
                        self._store, task_id, "progress",
                        f"direct_worker_change_resolved:{pending_direct_change[0]}",
                        dict(kind="direct_worker_change_resolved",
                             pending_event_id=pending_direct_change[0]),
                        origin="runtime",
                    )
                    pending_direct_change = None
                elif not self._direct_worker_still_open(task_id):
                    # Neither an incomplete/failed worker nor an unrelated
                    # final tree can authorize the observed project change.
                    if claimed:
                        delegation_events.release_claim(self._store, task_id, continuation_id)
                    self._emit_localized(task_id, emit_update, language, "coordinator_turn_mutated_workspace")
                    self._set_state(
                        task_id, "blocked", wait_reason="coordinator_turn_mutated_workspace",
                        wait_detail=dict(explanation=acp_messages.render("coordinator_turn_mutated_workspace", language)),
                    )
                    return StopReason.END_TURN
            meaningful = [e for e in claimed if delegation_events.is_meaningful(e)]
            # A worker-result continuation is a new user-visible answer
            # boundary.  Ordinary user-message continuations still use the
            # existing self-talk suppression when the model only performs a
            # Cortex read.
            worker_event_continuation = any(
                event.event_type in {
                    "worker_result", "worker_question", "assignment_failed",
                    "assignment_cancelled",
                }
                for event in meaningful
            )
            # FINDING 11: a refused delegation must never be silently
            # absorbed into "the coordinator did it all itself" -- surfaced
            # above as a failed tool-call card (T035) whenever a card
            # channel is wired; the plain-text line below is kept ONLY as
            # the fallback for a caller that never wired emit_tool_call at
            # all (no pre-T035 caller/test does) -- a card and this same
            # text line together would just duplicate the refusal, so the
            # text only fires when there is no card to carry it.
            if emit_tool_call is None:
                for event in meaningful:
                    if event.event_type == "assignment_failed":
                        self._emit_localized(
                            task_id, emit_update, language, "assignment_delegation_failed",
                            assignment_ref=event.assignment_ref or "?",
                            error=event.payload.get("error", "unknown"),
                        )
            if not meaningful and not first:
                # T020 (stop-race fix): if a /cortex-stop landed in this
                # exact window (this batch was claimed but was never going
                # to be delivered anyway, since it carries no meaningful
                # event), release it and supersede any steering rather than
                # leaving it claimed-but-undelivered forever, and exit
                # without ever reaching another engine.prompt call. Never
                # released in the ordinary (not-stopped) case -- that would
                # change the pre-existing no-progress-budget timing (an
                # informative-only claimed batch intentionally stays
                # claimed, never redelivered, until a later meaningful
                # event's own claim naturally supersedes/consumes it).
                if acp_steering.is_stopped(self._store, task_id):
                    if claimed:
                        delegation_events.release_claim(self._store, task_id, continuation_id)
                    acp_steering.supersede_undelivered_steering(self._store, task_id)
                    return StopReason.CANCELLED
                # A cancel (or a delegation, or an integration request)
                # recorded while this loop is only *waiting* has nobody
                # else to act on it: the in-turn watcher runs alongside a
                # coordinator prompt, and here there is none. Without this
                # such a request sat untouched until the worker being
                # waited on finished its own turn -- which, now that the
                # coordinator has no continuation cap and genuinely waits,
                # can be a very long time.
                self._process_new_events(task_id, channel, cancel_event, emit_tool_call)
                progressed = self._wait_for_progress(task_id, cancel_event)
                if not progressed:
                    # T035 FINDING-13 #2: a no-progress *tick* is not
                    # itself a stall -- provisioning a worker/coordinator
                    # engine session is real work with no MEANINGFUL event
                    # of its own until it finishes (see
                    # delegation_events.MEANINGFUL_EVENT_TYPES's own
                    # docstring for why it must stay that way: an
                    # informative-only notice must never itself wake this
                    # loop). Before ever charging this tick against the
                    # no-progress budget, check whether provisioning
                    # actually advanced a step since the last tick
                    # (delegation_provisioning.in_progress_snapshot); if it
                    # did, this is "still coming", not "stuck" -- the
                    # budget resets instead of depleting, and the
                    # coordinator's own blocked state names the real
                    # step/attempt so a human/host (and the model, on
                    # whatever later continuation actually reaches it) can
                    # tell the difference. Only a signature that is empty
                    # (nothing provisioning at all) or unchanged from the
                    # last tick (provisioning exists but has not moved)
                    # ever counts as "no progress" -- ties the coordinator's
                    # patience to observed backend reality instead of a
                    # fixed tick count with no relationship to it.
                    provisioning_sig = delegation_provisioning.in_progress_snapshot(self._store, task_id)
                    if provisioning_sig and provisioning_sig != last_provisioning_sig:
                        last_provisioning_sig = provisioning_sig
                        no_progress = 0
                        self._set_state(
                            task_id, "blocked", wait_reason="workers_provisioning", no_progress_count=0,
                            wait_detail=dict(explanation=_provisioning_progress_note(provisioning_sig)),
                        )
                        self._emit_provisioning_tool_calls(emit_tool_call, provisioning_sig)
                        continue
                    last_provisioning_sig = provisioning_sig
                    with self._lock:
                        # An assignment that is open at all -- not merely
                        # one whose engine turn happens to be in flight
                        # this instant. A worker may legitimately work for
                        # hours on something hard ("воркер может работать
                        # часами если там что то сложное, и это не повод
                        # блокировать"), and between its own turns, while
                        # a clarification is being delivered, or inside
                        # one long tool call, the in-flight set is empty
                        # while the work is very much alive. Live: the
                        # budget ran out at 00:34:53 and the worker
                        # published its result at 00:35:18.
                        #
                        # This was tried once before and reverted, because
                        # an assignment that never terminates then holds
                        # the turn open forever. That is now the deliberate
                        # behaviour: waiting on real work has no deadline,
                        # and cancellation is the way out. The budget below
                        # still covers the case it was written for -- a
                        # task with nothing open and nothing happening.
                        in_flight = set(self._worker_turns_in_flight)
                        open_refs = set(self._open_assignments.get(task_id) or ())
                        pending = [f for f in (self._running.get(task_id) or ()) if not f.done()]
                    self._report_assignments_whose_provisioning_failed(task_id)
                    # And the mirror of it: a row still holding its claim for
                    # an assignment that is already over leaves an orphan
                    # nothing would ever touch again.
                    delegation_provisioning.close_rows_for_finished_assignments(self._store, task_id)
                    # A direct shared-root worker may have changed the
                    # project during the coordinator turn and is now between
                    # engine turns.  The post-turn guard records that exact
                    # boundary in ``pending_direct_change`` while the worker
                    # is still allowed to publish.  Do not close the
                    # assignment merely because it has no currently running
                    # prompt: doing so creates a race where the coordinator
                    # reaches the wait loop before the worker's publication
                    # event arrives, then treats the still-valid late result
                    # as an orphan and blocks the task.
                    if open_refs and not in_flight and not pending and pending_direct_change is None:
                        # Open assignments with nothing running for this
                        # task at all: their workers' turns are over and
                        # none of them produced a result, a question or a
                        # failure. Closed here rather than the instant a
                        # worker's prompt returns, because a result can
                        # still be published just after that (see
                        # ``recovery.worker_reconciliation``'s own late-
                        # publish race, which must keep winning).
                        for ref in sorted(open_refs):
                            self._close_assignment_that_produced_nothing(task_id, ref)
                        continue
                    if in_flight or open_refs:
                        no_progress = 0
                        self._set_state(task_id, "blocked", wait_reason="workers_running", no_progress_count=0)
                        self._emit_worker_activity_tool_calls(emit_tool_call)
                        continue
                    no_progress += 1
                    self._set_state(task_id, "blocked", wait_reason="workers_running", no_progress_count=no_progress)
                    if no_progress >= self._max_no_progress:
                        explanation = acp_messages.render("blocked_no_progress", language)
                        self._emit_localized(task_id, emit_update, language, "blocked_no_progress")
                        self._set_state(
                            task_id, "blocked", wait_reason="blocked",
                            wait_detail=dict(explanation=explanation), no_progress_count=no_progress,
                        )
                        return StopReason.END_TURN
                continue
            is_first_prompt = first
            first = False
            no_progress = 0
            # Counted here, where a coordination continuation actually
            # happens, and nowhere else. At the top of the loop this also
            # counted every *waiting* tick: a worker running for four
            # minutes spent the whole budget without the coordinator being
            # asked anything at all, and the turn then ended claiming it
            # had exceeded a number of continuations it had never made.
            # Waiting is bounded by the no-progress budget above and by
            # the user's own cancellation, not by this.
            total += 1
            if self._max_total is not None and total > self._max_total:
                self._emit_localized(task_id, emit_update, language, "max_total_continuations")
                self._set_state(task_id, "blocked", wait_reason="limit:max_total_continuations")
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                return StopReason.END_TURN
            # T020 (stop-race fix): re-check immediately before ever calling
            # the coordinator's engine again -- a /cortex-stop that lands
            # between this iteration's own top-of-loop check and here (for
            # example while claim_unconsumed/_capture_result_revisions ran)
            # must still prevent one more engine.prompt call and must never
            # let a just-claimed-but-undelivered steering batch reach the
            # coordinator. The claim is released (not lost -- a stopped
            # task's undelivered events stay superseded/inert, never
            # resurrected) and any steering in this exact batch is marked
            # superseded the same way stop_task() marks it for anything it
            # was still unclaimed.
            if acp_steering.is_stopped(self._store, task_id):
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                acp_steering.supersede_undelivered_steering(self._store, task_id)
                return StopReason.CANCELLED
            prompt_text = _coalesce(
                claimed, revisions, self._coordinator_facing_refs(task_id, claimed),
            ) if claimed else content
            if is_first_prompt and initial_project_docs_context:
                prompt_text = f"{initial_project_docs_context}\n\n---\n\n{prompt_text}"
            chosen_language = self._explicit_language(task_id)
            if chosen_language is not None:
                prompt_text = (
                    f"{acp_messages.render('answer_in_language', chosen_language)}\n\n{prompt_text}"
                )
            prompt_text, generation_delivery_id, delivery_blocked = self._begin_generation_delivery(
                task_id, session_ref, COORDINATOR_ROLE_ID, prompt_text,
                emit_update=emit_update, language=language,
            )
            if delivery_blocked:
                # T024: a permanent budget-overflow/compaction-loop block --
                # the explicit localized message and blocked task state were
                # already recorded/emitted inside _begin_generation_delivery;
                # never prompt the engine with an oversized/looping turn.
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                return StopReason.END_TURN
            # T035 FINDING-12 closure: the engine-independent backstop.
            # ``engine.permissions.PermissionBridge`` refuses a coordinator's
            # mutating *intent* whenever the engine actually asks -- but
            # that still depends on the engine choosing to ask at all (see
            # that module's own HONEST BOUNDARY). This is the part that
            # does not depend on the engine's goodwill: capture the shared
            # project workspace's observed tree digest immediately before
            # and immediately after this one coordinator turn and fail the
            # turn, visibly, if it moved -- detection, never prevention (the
            # write already happened by the time this runs), but it turns a
            # silent violation into a recorded, blocked task state instead
            # of nothing at all. See ``delegation.revision.workspace_changed``'s
            # own docstring for exactly what is compared and what it costs
            # (two full capture_revision walks of the workspace per turn).
            with self._store.connection() as db:
                turn_event_cursor = db.execute(
                    "SELECT COALESCE(MAX(id), 0) FROM task_events WHERE task_id=?", (task_id,),
                ).fetchone()[0]
            pre_turn_revision = delegation_revision.capture_revision(self._task_project_root(task_id))
            # ACP has no retract primitive. Buffer redacted coordinator text
            # until the turn outcome establishes whether it is an answer or
            # interim narration while delegated work is still running.
            deferred_coordinator_chunks: list[str] = []
            coordinator_stream_redactor = StreamingRedactor(max_chars=4000)
            coordinator_delta_seen = False
            # A coordinator message closes the current consecutive tool
            # group. Text remains buffered until the durable outcome is
            # known because accepted finals still require digest guarding.
            coordinator_tool_emitter = self._engine_tool_call_emitter(
                emit_tool_call, language=language, project_root=self._task_project_root(task_id),
                task_id=task_id, cancel_event=cancel_event,
            )
            turn_cancel = _CoordinatorTurnCancel(cancel_event)
            refused_accepts = set()
            observed_tool_titles: dict[str, str] = {}
            # A delegate is an engine-turn handoff. Keep the already queued
            # tool batch intact, then yield to the runtime's no-model event
            # waiter. A short quiet interval distinguishes successive
            # updates in one ACP tool batch from an idle model repeatedly
            # reading its capsule while the worker runs.
            handoff_lock = threading.Lock()
            handoff_pending = False
            handoff_last_tool_at = 0.0
            handoff_in_flight: set[str] = set()
            handoff_quiet_seconds = 0.15
            cortex_tool_invoked = False

            def _on_coordinator_delta(chunk_text: str) -> bool:
                nonlocal coordinator_delta_seen
                if chunk_text and coordinator_tool_emitter is not None:
                    close_group = getattr(coordinator_tool_emitter, "close_group", None)
                    if close_group is not None:
                        close_group()
                if not chunk_text:
                    return True
                coordinator_delta_seen = True
                # Keep a lexical suffix pending so a credential split over
                # ACP updates is never sent to the host or durable replay.
                safe_chunk = coordinator_stream_redactor.push(str(chunk_text))
                if not safe_chunk:
                    return True
                # A complete provider message can arrive as one update while
                # the redactor still holds its final lexical field.  If the
                # released prefix and incoming chunk both show a terminal
                # punctuation boundary, flush that suffix now so the host
                # receives one exact final message.  Digest-only fields stay
                # buffered for the accepted-final integrity guard.
                if (coordinator_stream_redactor._pending
                        and str(chunk_text).rstrip().endswith((".", "!", "?", "。", "！", "？"))):
                    safe_chunk += coordinator_stream_redactor.flush()
                # Defer prose until the outcome is known. Before a delegation
                # the model can narrate its tool search; afterwards it can
                # narrate capsule polling. Neither is a user answer, and
                # ACP cannot retract a delta once streamed to the client.
                deferred_coordinator_chunks.append(safe_chunk)
                return True

            def _flush_coordinator_chunks(*, accepted: bool = False) -> bool:
                pending = coordinator_stream_redactor.flush(preserve_labeled_digests=accepted)
                if pending:
                    deferred_coordinator_chunks.append(pending)
                joined = "".join(deferred_coordinator_chunks)
                if accepted:
                    displayed = _scrub_accepted_final(
                        joined, self._accepted_pipeline_text_for_final_guard(task_id),
                        acp_messages.render("final_digest_withheld", language),
                    )
                else:
                    displayed = scrub_text(joined, max_chars=max(4000, len(joined) * 2 + 1000))
                chunks = [displayed[i:i + _COORDINATOR_DELTA_MAX_CHARS]
                          for i in range(0, len(displayed), _COORDINATOR_DELTA_MAX_CHARS)]
                for chunk in chunks:
                    # Most ACP callers use ``list.append`` as their sink,
                    # which returns ``None``. Only an explicit ``False`` is
                    # a client backpressure/cancellation signal.
                    if emit_update(chunk, delta=True) is False:
                        return False
                return True

            def _buffer_intermediate_coordinator_chunks() -> None:
                """Close the lexical redaction buffer at a tool boundary."""
                pending = coordinator_stream_redactor.flush()
                if pending:
                    deferred_coordinator_chunks.append(pending)

            def _on_coordinator_tool_call(*args, **kwargs):
                nonlocal handoff_pending, handoff_last_tool_at, cortex_tool_invoked
                # Native provider subagents bypass Cortex delegation and can
                # mutate/consume work outside the assignment graph.  Refusal
                # text is not enforcement: the live ACP adapter already has
                # an independent cancel watcher, so stop this turn as soon
                # as the engine exposes the native call.  This works for
                # production native-account CODEX_HOME too, where Cortex
                # cannot safely edit the user's config.toml to disable the
                # provider feature globally.
                title = kwargs.get("title")
                if not isinstance(title, str) and len(args) >= 3:
                    title = args[2]
                ref = kwargs.get("ref") if "ref" in kwargs else (args[0] if args else None)
                if isinstance(ref, str) and isinstance(title, str) and title.strip():
                    observed_tool_titles[ref] = title.strip()
                if isinstance(ref, str) and _COORDINATOR_CORTEX_TOOL_CALL.fullmatch(
                    observed_tool_titles.get(ref, "")
                ):
                    cortex_tool_invoked = True
                status = kwargs.get("status") if "status" in kwargs else (args[3] if len(args) >= 4 else None)
                raw_output = kwargs.get("raw_output") if "raw_output" in kwargs else (args[7] if len(args) >= 8 else None)
                if isinstance(ref, str):
                    with handoff_lock:
                        if status in ("pending", "in_progress"):
                            handoff_in_flight.add(ref)
                        elif status in ("completed", "failed", "canceled", "cancelled"):
                            handoff_in_flight.discard(ref)
                        if handoff_pending:
                            handoff_last_tool_at = time.monotonic()
                if (status == "completed" and isinstance(ref, str)
                        and _COORDINATOR_DELEGATE_TOOL_CALL.fullmatch(observed_tool_titles.get(ref, ""))):
                    with self._store.connection() as db:
                        delegated = db.execute(
                            "SELECT 1 FROM task_events WHERE task_id=? AND id>? "
                            "AND event_type='delegation_requested' AND superseded=0 LIMIT 1",
                            (task_id, turn_event_cursor),
                        ).fetchone() is not None
                    if delegated:
                        with handoff_lock:
                            handoff_pending = True
                            handoff_last_tool_at = time.monotonic()
                # The completion gate correctly refuses a task accept while
                # new worker/progress events have not been delivered in a
                # coordinator continuation.  Merely reading them with MCP
                # list_events inside this SAME engine turn does not deliver
                # them.  End this engine turn after the first exact refusal:
                # retrying the same acceptance cannot change the durable
                # cursor state and only spends another MCP call/token.  Its
                # normal return marks the old claim delivered and the next
                # loop iteration claims/coalesces the pending events.  Never
                # waive the gate or mutate consumed_by here.
                if (isinstance(ref, str)
                        and _is_cortex_decide_tool(observed_tool_titles.get(ref))
                        and status == "failed"
                        and _pending_events_acceptance_refusal(raw_output)):
                    refused_accepts.add(ref)
                    if len(refused_accepts) >= 1:
                        turn_cancel.request_boundary()
                if isinstance(title, str) and _NATIVE_SUBAGENT.search(title):
                    cancel_event.set()
                # Preserve the actual event order in clients. Deltas have
                # already been sent at their engine event boundary; the
                # fallback buffer is only used by providers that return text
                # without emitting deltas.
                _buffer_intermediate_coordinator_chunks()
                if coordinator_tool_emitter is None:
                    return None
                return coordinator_tool_emitter(*args, **kwargs)

            def _on_coordinator_display_update(update: Mapping[str, Any]) -> bool | None:
                # Plans and explicit thought/message updates are ordinary
                # coordinator output too. Flush prose first so the host sees
                # prose -> display update in that order, then terminate the
                # preceding tool run before the next tool can open a group.
                _buffer_intermediate_coordinator_chunks()
                if coordinator_tool_emitter is not None:
                    close_group = getattr(coordinator_tool_emitter, "close_group", None)
                    if close_group is not None:
                        close_group()
                if emit_display_update is None:
                    return None
                return emit_display_update(update)

            # T035 FINDING-13 #1, measured: the coordinator calls delegate()
            # in the middle of its turn, but the delegation was only ever
            # acted on after prompt() returned (see _process_new_events'
            # single call site below), so a worker could not start until
            # the coordinator stopped talking. Live, that was the whole gap
            # the finding called "provisioning takes eight minutes":
            # delegation_requested 18:59:08, coordinator gives up 19:04:40,
            # worker ready 19:05:33 -- one second after the cancellation,
            # with the open itself taking 0.738s (evidence/T035/RUN-07).
            #
            # This watcher runs the same _process_new_events while the turn
            # is still in flight, so provisioning overlaps the turn that
            # asked for it. It is the same method, deliberately: its own
            # dispatch bookkeeping is already written to be race-free
            # against a concurrent call (see its "P1 #7 fix" comment), so
            # this adds a caller, not a second code path.
            _turn_done = threading.Event()

            def _dispatch_while_turn_runs() -> None:
                while not _turn_done.wait(timeout=_EARLY_DISPATCH_POLL_SECONDS):
                    if cancel_event.is_set():
                        return
                    try:
                        self._process_new_events(task_id, channel, cancel_event, emit_tool_call)
                        # A worker that starts during this turn also works
                        # during it: its card must follow along now, not
                        # once the coordinator finally stops talking.
                        self._emit_worker_activity_tool_calls(emit_tool_call)
                        with handoff_lock:
                            ready_to_yield = (
                                handoff_pending and not handoff_in_flight
                                and time.monotonic() - handoff_last_tool_at >= handoff_quiet_seconds
                            )
                        if ready_to_yield:
                            turn_cancel.request_boundary()
                            return
                    except Exception:  # noqa: BLE001 - never fail the turn over an early start
                        return

            _early_dispatch = threading.Thread(
                target=_dispatch_while_turn_runs, name="cortex-early-dispatch", daemon=True,
            )
            _early_dispatch.start()
            try:
                prompt_kwargs = dict(
                    on_delta=_on_coordinator_delta, on_tool_call=_on_coordinator_tool_call,
                    on_display_update=_on_coordinator_display_update if emit_display_update is not None else None,
                )
                if self._engine_accepts_prompt_blocks and total == 1:
                    # Preserve the incoming text/image order after a trusted
                    # Cortex context prefix.  The composed prompt normally
                    # ends with the exact incoming text blocks; remove that
                    # suffix from the prefix so user text is not duplicated,
                    # while images remain opaque and byte-for-byte unchanged.
                    incoming = [
                        dict(type=block.get("type"), **{
                            key: block.get(key) for key in ("text", "data", "mimeType")
                            if key in block
                        })
                        for block in (prompt_blocks or ())
                        if isinstance(block, Mapping) and block.get("type") in ("text", "image")
                    ]
                    rendered_parts = []
                    for block in (prompt_blocks or ()):
                        if not isinstance(block, Mapping):
                            continue
                        kind = block.get("type")
                        if kind == "text":
                            rendered_parts.append(block.get("text", ""))
                        elif kind == "image":
                            rendered_parts.append(f"[image: {block.get('mimeType', '')}]")
                        elif kind == "resource_link":
                            rendered_parts.append(
                                f"[resource_link: {block.get('name', '')} ({block.get('uri', '')})]"
                            )
                        elif kind == "resource":
                            resource = block.get("resource") or {}
                            rendered_parts.append(f"[resource: {resource.get('uri', '')}]")
                    incoming_text = "\n".join(rendered_parts)
                    trusted_prefix = prompt_text
                    if incoming_text:
                        composed = prompt_text.rstrip()
                        marker = composed.rfind(incoming_text)
                        if marker >= 0:
                            # The delivery capsule may add a short terminal
                            # instruction after the user text. Remove the
                            # last exact rendered user sequence wherever it
                            # occurs, retaining any trusted context around
                            # it; this avoids duplicating A/B when an image
                            # marker sits between them.
                            trusted_prefix = (composed[:marker] + composed[marker + len(incoming_text):]).rstrip()
                        else:
                            # ``_coalesce`` intentionally renders text
                            # blocks without attachment markers. Remove only
                            # a terminal, structurally identified user
                            # message segment; never search arbitrary trusted
                            # context for matching user text.
                            text_only = "\n".join(
                                block.get("text", "") for block in incoming
                                if block.get("type") == "text"
                            )
                            trusted_prefix = composed
                            for label in ("User message: ", "User message:\n"):
                                segment = label + text_only
                                if text_only and composed.endswith(segment):
                                    trusted_prefix = composed[:-len(segment)].rstrip()
                                    break
                    prompt_kwargs["prompt_blocks"] = [
                        dict(type="text", text=trusted_prefix), *incoming,
                    ]
                outcome = self._engine.prompt(session_ref, prompt_text, turn_cancel, **prompt_kwargs)
                from .. import _fault_injection

                _fault_injection.checkpoint("t023_after_continuation_prompt")
                # Some compatible SessionRuntime implementations provide
                # only the terminal PromptOutcome text. Preserve that
                # contract as a fallback, while never replaying text from a
                # provider that already emitted deltas.
                # A provider may report the same terminal text both as deltas
                # and as PromptOutcome.text.  ``coordinator_delta_seen`` also
                # covers a fully buffered redacted field whose safe prefix was
                # empty (for example an accepted SHA-256 claim), so it must
                # not be replayed through the terminal fallback.
                if not coordinator_delta_seen and outcome.text:
                    deferred_coordinator_chunks.append(str(outcome.text))
            except EngineTurnFailed as failure:
                # T017: an engine failure on the coordinator's own turn is
                # never silently treated as end_turn, and never rerouted to
                # a second engine/connection -- record it durably and stop
                # with an honest blocker instead of a fabricated success.
                self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=failure.kind, stop_reason=None)
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                # The commonest cause of a transport-level failure here is
                # not a broken engine at all: another client opened the same
                # thread, and the takeover fenced this coordinator. Live
                # (2026-09-19, two clients on one thread) the first client
                # was told "coordinator engine turn failed (transport_eof)",
                # which is true and tells the reader nothing. Say which one
                # it was -- the durable record already knows, because the
                # task's coordinator attempt is no longer this one.
                message_key, params = "engine_failure", dict(kind=failure.kind)
                if self._superseded_by_another_coordinator(task_id, getattr(channel, "attempt_id", None)):
                    message_key, params = "coordinator_replaced_by_another_client", {}
                explanation = acp_messages.render(message_key, language, **params)
                self._emit_localized(task_id, emit_update, language, message_key, **params)
                self._set_state(
                    task_id, "blocked", wait_reason=f"engine_failure:{failure.kind}",
                    wait_detail=dict(explanation=explanation),
                )
                # A tool group is a visual lifecycle object as well as a
                # compact transcript.  Close it on every terminal turn so a
                # client cannot leave its shimmer running after the engine
                # failed between two tool updates.
                close_group = getattr(coordinator_tool_emitter, "close_group", None)
                if close_group is not None:
                    close_group()
                return StopReason.END_TURN
            except BaseException:
                # T016 review (MEDIUM): a raise from engine.prompt() means
                # these claimed events were never actually delivered to the
                # coordinator model. Release the claim (consumed_by -> NULL)
                # so the next continuation reclaims and delivers them
                # exactly once, instead of silently losing them, then
                # propagate -- this turn genuinely failed.
                self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind="unexpected_exception", stop_reason=None)
                if claimed:
                    delegation_events.release_claim(self._store, task_id, continuation_id)
                close_group = getattr(coordinator_tool_emitter, "close_group", None)
                if close_group is not None:
                    close_group()
                raise
            finally:
                # However this turn ends -- success, engine failure, or a
                # raise -- the early-dispatch watcher stops with it; it
                # exists only for the window while the turn is in flight.
                _turn_done.set()
            self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=None, stop_reason=outcome.stop_reason)
            # All tool calls for this engine turn have now been observed.  A
            # new prompt/continuation starts a new consecutive group, while
            # this update gives the host the terminal status for the old one.
            # Without it Paseo keeps the grouped card in its live/shimmering
            # state indefinitely.
            close_group = getattr(coordinator_tool_emitter, "close_group", None)
            if close_group is not None:
                close_group()
            if claimed:
                delegation_events.mark_delivered(self._store, task_id, continuation_id)
            post_turn_revision = delegation_revision.capture_revision(self._task_project_root(task_id))
            with self._lock:
                authorized = list(self._authorized_project_revisions.get(task_id, ()))
            # Compared with the same function the check itself uses -- a
            # revision is a structure, not a value to match by identity.
            authorized_now = any(
                not delegation_revision.workspace_changed(revision, post_turn_revision)
                for revision in authorized
            )
            if not authorized_now and delegation_revision.workspace_changed(pre_turn_revision, post_turn_revision):
                authorized_now = self._direct_worker_publication_matches(
                    task_id, turn_event_cursor, post_turn_revision,
                )
            if not authorized_now and delegation_revision.workspace_changed(pre_turn_revision, post_turn_revision):
                if self._direct_worker_still_open(task_id):
                    # The worker may already have edited the shared project
                    # but has not yet published. Preserve the original event
                    # boundary; its eventual result must match the then-current
                    # tree exactly before a later coordinator turn can accept.
                    if pending_direct_change is None:
                        recorded = delegation_events.append_event(
                            self._store, task_id, "progress",
                            f"direct_worker_change_pending:{continuation_id}",
                            dict(kind="direct_worker_change_pending",
                                 after_event_id=turn_event_cursor),
                            origin="runtime",
                        )
                        pending_direct_change = (recorded.id, turn_event_cursor)
                    authorized_now = True
            with self._lock:
                if authorized_now:
                    # Consumed: a *further* change, by anyone, is once again
                    # a revision nobody authorized.
                    self._authorized_project_revisions.pop(task_id, None)
                    pre_turn_revision = post_turn_revision
            if not authorized_now and delegation_revision.workspace_changed(pre_turn_revision, post_turn_revision):
                # T035 FINDING-12 closure: the coordinator's own turn just
                # changed (or could not prove it did not change) the shared
                # project workspace -- refused as a blocking failure of this
                # turn, never silently accepted and never treated as if the
                # turn had ended normally (see this loop's own comment above
                # the capture that made ``pre_turn_revision``).
                self._emit_localized(task_id, emit_update, language, "coordinator_turn_mutated_workspace")
                self._set_state(
                    task_id, "blocked", wait_reason="coordinator_turn_mutated_workspace",
                    wait_detail=dict(explanation=acp_messages.render("coordinator_turn_mutated_workspace", language)),
                )
                return StopReason.END_TURN
            # The text was buffered from deltas; a provider's terminal
            # outcome repeats that text and must not be appended again.
            if outcome.stop_reason == StopReason.REFUSAL:
                _flush_coordinator_chunks()
                # Never re-route to another engine/connection/model on
                # refusal -- persist and hand the refusal straight to the host.
                self._set_state(
                    task_id, "refused",
                    wait_reason="refused", wait_detail=dict(explanation=outcome.text or "Coordinator refused this turn."),
                )
                self._release_task_host_requesters(task_id)
                return StopReason.REFUSAL
            self._process_new_events(task_id, channel, cancel_event, emit_tool_call)
            # ACP permission responses carry only the selected option ID.
            # Claude renders a local reject as a generic user refusal and
            # may end this turn by asking the user for write permission.
            # The bridge's trusted, durable role-refusal event instead
            # requires a fresh prompt in this same host turn. The earlier
            # prose was buffered, so the misleading permission request is
            # never shown as the final answer. This does not override a
            # real user cancellation or a genuine engine refusal.
            if outcome.stop_reason == StopReason.END_TURN and not cancel_event.is_set():
                with self._store.connection() as db:
                    role_refusal_rows = db.execute(
                        "SELECT payload_json FROM task_events WHERE task_id=? AND id>? "
                        "AND event_type='progress' AND consumed_by IS NULL AND superseded=0",
                        (task_id, turn_event_cursor),
                    ).fetchall()
                if any(
                    json.loads(row["payload_json"]).get("kind") == "coordinator_role_refusal"
                    for row in role_refusal_rows
                ):
                    continue
            terminal = self._check_terminal_decision(task_id)
            if terminal == "accepted" and pending_direct_change is not None:
                # A still-unattributed change cannot become accepted merely
                # because the coordinator issued a task-level decision.
                terminal = None
            if terminal == "accepted":
                accepted_text = "".join(deferred_coordinator_chunks)
                stale_status_claim = _accepted_status_correction_needed(accepted_text)
                if not _flush_coordinator_chunks(accepted=True):
                    return StopReason.END_TURN
                self._set_state(task_id, "accepted")
                # A coordinator may have read the transient blocked
                # projection before the worker-result delivery and include
                # that stale claim in its final prose.  Now that the durable
                # accepted state is committed, append one canonical correction
                # so the host never leaves the user with an unresolved
                # contradictory status.  This does not synthesize acceptance;
                # the terminal branch above already passed the completion gate.
                if stale_status_claim:
                    emit_update(
                        acp_messages.render("accepted_status_correction", language),
                        delta=True,
                    )
                self._release_task_host_requesters(task_id)
                # Acceptance is terminal for this task's engine attempt.
                # The accepted prose has already reached the ACP sink and
                # the state is durable, so release only this coordinator's
                # owned process/capacity now.  Keeping its idle session
                # alive can exhaust a connection's capacity even though
                # the host has received a successful final response.
                self._terminate_coordinator_attempt(task_id)
                return StopReason.END_TURN
            if isinstance(terminal, dict) and terminal.get("kind") == "ask_user":
                if not _flush_coordinator_chunks():
                    return StopReason.END_TURN
                # T020: persist-then-try-elicitation-then-fallback -- see
                # _handle_ask_user. A resolved question (elicitation accept)
                # continues the SAME turn with the answer coalesced into the
                # next continuation; an unresolved one ends the turn with
                # the plain question (existing pre-T020 behavior).
                resolved = self._handle_ask_user(task_id, session_id, emit_update, language, terminal, host_elicitor)
                if resolved:
                    no_progress = 0
                    continue
                self._set_state(
                    task_id, "waiting_for_user", wait_reason="user_input_required",
                    wait_detail=dict(question=terminal["question"], pending_question_id=terminal["question_id"]),
                )
                return StopReason.END_TURN
            # Ordinary coordinator prose is user-visible output whenever the
            # model emits it. Worker activity and Cortex MCP bookkeeping are
            # represented by their own cards, but neither is a reason to
            # discard a coordinator message. The prompt-level anti-chatter
            # guidance remains the model's responsibility; this boundary
            # only avoids suppressing text that was actually generated.
            if not turn_cancel.boundary_requested():
                if not _flush_coordinator_chunks():
                    return StopReason.END_TURN
            if outcome.stop_reason == StopReason.CANCELLED:
                if turn_cancel.boundary_requested() and not cancel_event.is_set():
                    # This is an internal engine-turn boundary, not a host
                    # session/cancel.  A fresh continuation must actually
                    # deliver the pending event batch before any task-level
                    # accept can pass.  A poisoned/unusable engine session
                    # raises on its next prompt and is reported as such.
                    continue
                # A client stop/cancellation that terminates the task must
                # also release the coordinator engine process.  The normal
                # session-cancel path may leave a session reusable while a
                # task is merely interrupted, but this terminal transition
                # is different: retaining the coordinator's bwrap here
                # leaks its capacity reservation after Paseo has cancelled
                # the task.  The helper resolves the durable attempt too,
                # so this remains correct after an in-memory cache loss.
                self._terminate_coordinator_attempt(task_id)
                self._set_state(task_id, "cancelled")
                return StopReason.CANCELLED
            has_unconsumed = delegation_events.has_unconsumed(self._store, task_id)
            running = bool(self._running.get(task_id)) or (
                pending_direct_change is not None and self._direct_worker_still_open(task_id)
            )
            if outcome.stop_reason in (StopReason.MAX_TOKENS, StopReason.MAX_TURN_REQUESTS):
                # T017: a token/turn-request limit counts toward the budget
                # and is recorded, but only continues the loop when a
                # meaningful event or a known unfinished action (pending
                # events, a running worker) actually exists; otherwise this
                # is an honest limit blocker, never a fabricated idle wait.
                kind = outcome.stop_reason.value
                self._emit_localized(task_id, emit_update, language, "coordinator_turn_limit", kind=kind)
                if not has_unconsumed and not running:
                    explanation = acp_messages.render("blocked_limit", language, kind=kind)
                    self._set_state(
                        task_id, "blocked", wait_reason=f"limit:{kind}",
                        wait_detail=dict(explanation=explanation),
                    )
                    return StopReason.END_TURN
                continue
            if not has_unconsumed and not running:
                with self._store.connection() as db:
                    has_delegation = db.execute(
                        "SELECT 1 FROM task_events WHERE task_id=? "
                        "AND event_type='delegation_requested' AND superseded=0 LIMIT 1",
                        (task_id,),
                    ).fetchone() is not None
                if has_delegation:
                    from ..delegation import completion as delegation_completion

                    _ok, reasons = delegation_completion.gate(self._store, task_id)
                    # Reaching the end of an ordinary coordinator turn does
                    # not mean the coordinator attempted acceptance. Keep a
                    # prior delegation's unresolved evidence visible in the
                    # durable wait detail, but do not present it as an
                    # acceptance refusal. Only an explicit decide(accept)
                    # gets the user-facing acceptance gate correction.
                    self._set_state(
                        task_id, "idle_awaiting_user", wait_reason="idle_awaiting_user",
                        wait_detail=dict(
                            reasons=reasons,
                            explanation=(
                                "The coordinator replied; waiting for the next user message."
                                if not reasons else "Unresolved delegated work remains: " + " ".join(reasons)
                            ),
                        ),
                    )
                    return StopReason.END_TURN
                # An ordinary coordinator reply is an open dialogue turn,
                # not an attempted task acceptance. Keep the task available
                # for the next user prompt without manufacturing an
                # acceptance refusal. Explicit ``decide(accept)`` remains
                # the only path to terminal success and is still checked by
                # the completion gate above.
                self._set_state(
                    task_id, "idle_awaiting_user", wait_reason="idle_awaiting_user",
                    wait_detail=dict(explanation="The coordinator replied; waiting for the next user message."),
                )
                return StopReason.END_TURN
            # else: loop again -- either events already pending or we will
            # wait for progress at the top of the next iteration.

    # -- /cortex-answer / /cortex-stop entrypoints (T020) ----------------

    def answer_question(
        self, task_id: str, session_id: str, question: questions_store.PendingQuestion, text: str,
        emit_update: Callable[[str], bool], cancel_event: threading.Event, *,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        emit_display_update: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> StopReason:
        """Deliver an explicit ``/cortex-answer`` (see ``acp.commands``) for
        one specific, already-disambiguated open ``question`` -- used when
        two or more questions are open and an ordinary prompt cannot be
        auto-matched to one of them (see ``_run``'s ``question_ambiguous``
        path). ``question`` must be one this exact task's own still-``open``
        row (the caller -- ``acp.agent`` -- resolves the 1-based index the
        user typed against ``questions_store.list_open`` immediately before
        calling this).

        A stopped task's late answer is recorded and inert, exactly like an
        ordinary prompt after ``/cortex-stop`` (see ``_run``)."""
        if acp_steering.is_stopped(self._store, task_id):
            acp_steering.record_post_stop_message(self._store, task_id, session_id, new_id("post_"), text)
            language = self._resolve_language(task_id, content_hint=text)
            emit_update(acp_messages.render("task_stopped", language))
            return StopReason.END_TURN
        answer_text = _question_answer_value(question, text)
        if not questions_store.answer(self._store, question.id, answer_text, "text"):
            # Resolved/superseded/cancelled between the caller's own
            # list_open() read and this call (a genuine race, or a stale
            # /cortex-answer against an already-answered question) -- a
            # late answer, recorded and otherwise inert.
            questions_store.record_late_answer(self._store, question.id, task_id, text, "text")
            language = self._resolve_language(task_id, content_hint=text)
            emit_update(acp_messages.render("question_answer_unknown_index", language))
            return StopReason.END_TURN
        channel, session_ref = self._ensure_coordinator(task_id)
        language = self._resolve_language(task_id, content_hint=text)
        recovery_notice = self._pending_recovery_notice.pop(task_id, None)
        if recovery_notice is not None:
            if recovery_notice.get("route_changed"):
                self._emit_localized(
                    task_id, emit_update, language, "coordinator_recovery_new_engine",
                    engine=recovery_notice.get("engine") or "?",
                )
            else:
                self._emit_localized(task_id, emit_update, language, "coordinator_recovery_new_session")
        delegation_events.append_event(
            self._store, task_id, "progress", f"question_answered:{question.id}",
            dict(kind="question_answered", question_id=question.id, answer_text=answer_text), origin="runtime",
        )
        # A user_message (meaningful) event, distinct from the informative
        # progress row above, so the continuation loop's claim/coalesce
        # gating (delegation_events.MEANINGFUL_EVENT_TYPES) actually wakes
        # for it -- exactly like an ordinary prompt's own user_message.
        delegation_events.append_event(
            self._store, task_id, "user_message", f"cortex_answer:{question.id}",
            dict(content=f"Answered question ({question.text!r}): {answer_text}", session_id=session_id),
            origin=session_id,
        )
        with self._lock:
            self._active_cancel_events[task_id] = cancel_event
        try:
            return self._continuation_loop(
                task_id, session_id, channel, session_ref, text, emit_update, cancel_event, language,
                emit_tool_call=emit_tool_call,
                emit_display_update=emit_display_update,
            )
        finally:
            with self._lock:
                if self._active_cancel_events.get(task_id) is cancel_event:
                    del self._active_cancel_events[task_id]

    def stop_task(
        self, task_id: str, session_id: str, emit_update: Callable[[str], bool],
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> bool:
        """T020 ``/cortex-stop``: an urgent safety stop, routed through
        cancellation -- never a normal steering queue entry. Takes
        precedence over an in-flight coordinator loop for this same task
        *in this process*: signals that loop's own cancel_event (see
        ``self._active_cancel_events``/``start_prompt``/``answer_question``)
        so it exits promptly without delivering queued steering or starting
        a new delegation (see ``_continuation_loop``'s stopped-state
        checks), then independently cancels every non-terminal assignment
        (and any capacity-queued open, via ``engine.cancel_task``),
        invalidates every still-open pending question, and marks any
        not-yet-delivered steering event superseded so it can never be
        delivered after this point (see
        ``acp.steering.supersede_undelivered_steering``). Idempotent: a
        second call on an already-stopped task is a no-op (returns
        ``False``) rather than a second cancellation wave.

        Raises :class:`PortableError` (code ``unknown_task``) for a
        ``task_id`` that does not exist at all -- never a false "stopped"
        confirmation for a task that was never real (T020 fresh-review
        fix)."""
        try:
            transitioned = acp_steering.mark_stopped(self._store, task_id)
        except LookupError:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="unknown_task",
                remediation="This task id does not exist; there is nothing to stop.",
                retry_eligible=False,
            ) from None
        if not transitioned:
            return False
        self._release_task_host_requesters(task_id)
        with self._lock:
            active_event = self._active_cancel_events.get(task_id)
        if active_event is not None:
            active_event.set()
        cancel_task = getattr(self._engine, "cancel_task", None)
        if cancel_task is not None:
            try:
                cancel_task(task_id)
            except Exception:
                pass
        # A task-wide queued cancellation only prevents a coordinator open
        # that has not acquired capacity yet.  Once the coordinator session
        # is running, its attempt is an owned engine resource in exactly the
        # same sense as a worker attempt and must be terminated explicitly.
        # The live ACP stop path exposed this gap: the task became ``stopped``
        # and the worker was cancelled, but the coordinator's engine process
        # remained alive until an external reaper killed it.  Resolve the
        # durable attempt ref as well as the in-process cache so this remains
        # fail-closed after the runtime has lost its memory state.
        self._terminate_coordinator_attempt(task_id)
        with self._store.connection() as db:
            open_refs = db.execute(
                "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? AND event_type='delegation_requested'",
                (task_id,),
            ).fetchall()
            terminal_rows = db.execute(
                "SELECT assignment_ref, event_type, payload_json FROM task_events WHERE task_id=? "
                "AND event_type IN ('worker_result','assignment_failed','assignment_cancelled') "
                "AND superseded=0 ORDER BY id",
                (task_id,),
            ).fetchall()
            attempts = {
                row["attempt_id"]: row["idempotency_key"]
                for row in db.execute(
                    "SELECT attempt_id, idempotency_key FROM delegation_provisioning "
                    "WHERE task_id=? AND attempt_id IS NOT NULL", (task_id,),
                ).fetchall()
            }
        # worker_result rows are keyed by the worker attempt id, while the
        # card and delegation lifecycle use the idempotency assignment ref.
        # Resolve both namespaces here so a stop cannot mistake a durable
        # result for an open assignment and append a contradictory cancel.
        terminal_by_assignment = {}
        terminal_priority = {"worker_result": 2, "assignment_failed": 1, "assignment_cancelled": 1}
        for row in terminal_rows:
            assignment_ref = attempts.get(row["assignment_ref"], row["assignment_ref"])
            prior = terminal_by_assignment.get(assignment_ref)
            if assignment_ref and (
                prior is None
                or terminal_priority.get(row["event_type"], 0)
                > terminal_priority.get(prior["event_type"], 0)
            ):
                terminal_by_assignment[assignment_ref] = row
        for row in open_refs:
            assignment_ref = row["assignment_ref"]
            terminal_row = terminal_by_assignment.get(assignment_ref)
            if terminal_row is not None:
                # The event is the authoritative terminal evidence. Reuse
                # the card's original emitter and emit only the matching
                # terminal state; this repairs a card left live when stop
                # raced a worker result, without fabricating completion.
                with self._lock:
                    bound_emitter = self._worker_card_emitters.get(assignment_ref)
                if bound_emitter is not None:
                    event_type = terminal_row["event_type"]
                    if event_type == "worker_result":
                        payload = json.loads(terminal_row["payload_json"] or "{}")
                        summary = payload.get("summary") if isinstance(payload, dict) else None
                        self._emit_worker_card(
                            bound_emitter, assignment_ref, status="completed",
                            entry=("Result: " + _compact_text(summary, _WORKER_ACTIVITY_MAX_CHARS - 8)
                                   if isinstance(summary, str) and summary else None),
                        )
                    else:
                        payload = json.loads(terminal_row["payload_json"] or "{}")
                        reason = payload.get("reason") or payload.get("error") or event_type
                        self._emit_worker_card(
                            bound_emitter, assignment_ref, status="failed",
                            entry=scrub_text(f"Cancelled: {reason}" if event_type == "assignment_cancelled"
                                             else f"Not delegated/failed: {reason}",
                                             max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                        )
                continue
            self._cancel_assignment(
                task_id, assignment_ref, "urgent stop (/cortex-stop)",
                session_id=session_id, emit_tool_call=emit_tool_call,
            )
            questions_store.cancel_for_assignment(self._store, task_id, assignment_ref, "task stopped")
        # A terminal event can precede engine shutdown (or a prior
        # cancellation can leave an ambiguous attempt).  Event state alone
        # does not prove the owned process has exited.  Sweep only attempts
        # durably bound to this task, including earlier/replaced attempts.
        with self._store.connection() as db:
            worker_attempts = db.execute(
                "SELECT DISTINCT attempt_id FROM delegation_provisioning "
                "WHERE task_id=? AND attempt_id IS NOT NULL", (task_id,),
            ).fetchall()
        for row in worker_attempts:
            attempt_id = row["attempt_id"]
            try:
                self._engine.terminate_owned(attempt_id)
            except Exception:
                pass
        questions_store.supersede_open(self._store, task_id, "task stopped")
        acp_steering.supersede_undelivered_steering(self._store, task_id)
        self._set_state(
            task_id, "stopped", wait_reason="stopped",
            wait_detail=dict(explanation="Stopped by /cortex-stop."),
        )
        language = self._resolve_language(task_id)
        emit_update(acp_messages.render("stop_confirmed", language))
        return True

    def _terminate_coordinator_attempt(self, task_id: str) -> None:
        """Release the task's currently owned coordinator engine process.

        ``stop_task`` may run while a coordinator turn is still unwinding,
        so this helper is deliberately best effort and idempotent.  The
        engine adapter owns the durable liveness/capacity semantics of
        ``terminate_owned``; this method only resolves the exact coordinator
        attempt and asks that adapter to perform its fail-closed cleanup.
        Worker attempts are handled separately by ``_cancel_assignment`` and
        the task-scoped durable provisioning sweep in ``stop_task``.
        """
        attempt_id = None
        with self._lock:
            channel = self._coordinator_channels.get(task_id)
            if channel is not None:
                attempt_id = channel.attempt_id
        if attempt_id is None:
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?",
                    (task_id,),
                ).fetchone()
            if row is not None:
                attempt_id = row["coordinator_attempt_ref"]
        if not attempt_id:
            return
        terminate = getattr(self._engine, "terminate_owned", None)
        if terminate is None:
            return
        try:
            terminate(attempt_id)
        except Exception:
            # The adapter records ambiguous termination and capacity state;
            # never let an adapter exception turn a committed stop into a
            # reported success/failure race.
            pass

    def _recover_unrecorded_source_events(self, task_id: str, *, exclude: tuple[str | None, str | None] = (None, None)) -> None:
        from . import replay_store

        for entry in replay_store.find_unrecorded_source_events(self._store, task_id):
            if (entry["origin"], entry["event_id"]) == exclude:
                # This call's own just-captured source event (see
                # acp.agent, which durably captures before invoking
                # start_prompt): recorded below as this turn's normal,
                # non-recovered user_message -- never swept in here as a
                # false 'recovered' label.
                continue
            if entry["text"] is None:
                # The durably published body could not be resolved (should
                # not happen for a source event this runtime itself will
                # ever be asked to continue from); skip rather than record a
                # fabricated empty user_message.
                continue
            replay_store.record_user_message_event(
                self._store, task_id, f"user_message:{entry['origin']}:{entry['event_id']}", entry["origin"],
                dict(content=entry["text"], session_id=entry["origin"], recovered=True),
                ledger_origin=entry["origin"], ledger_event_id=entry["event_id"],
            )

    def _emit_resume_context(self, task_id: str) -> None:
        with self._store.connection() as db:
            row = db.execute(
                "SELECT wait_reason, wait_detail_json FROM task_orchestration WHERE task_id=?", (task_id,),
            ).fetchone()
        # T020: "interrupted" (session/cancel) needs no resume-context echo
        # -- the interrupted turn's own accumulated events are already
        # delivered coalesced by the normal claim/coalesce path on the next
        # continuation; a resume_context line here would just be noise.
        if row is None or not row["wait_reason"] or row["wait_reason"] in ("workers_running", "idle_awaiting_user", "interrupted"):
            return
        import json

        detail = json.loads(row["wait_detail_json"]) if row["wait_detail_json"] else {}
        explanation = detail.get("explanation") or f"Previously waiting: {row['wait_reason']}."
        delegation_events.append_event(
            self._store, task_id, "progress", f"resume_context:{new_id('rc_')}",
            dict(note=f"Resuming after a prior wait ({row['wait_reason']}): {explanation}"),
            origin="runtime",
        )
        self._set_state(task_id, "running", wait_reason=None, wait_detail=None, no_progress_count=0)

    def _explicit_language(self, task_id: str) -> str | None:
        """The language the user actually chose, or ``None`` for "auto".

        Distinct from :meth:`_resolve_language`, which always answers with
        a concrete language (falling back to the script of what the user
        last wrote). Only an explicit choice is ever imposed on the
        coordinator itself -- on "auto" the model keeps following the user,
        which is what "auto" means.
        """
        explicit = acp_steering.get_language_override(self._store, task_id)
        return explicit if explicit in acp_messages.SUPPORTED_LANGUAGES else None

    def _resolve_language(self, task_id: str, *, content_hint: str | None = None) -> str:
        explicit = acp_steering.get_language_override(self._store, task_id)
        texts = acp_steering.recent_user_direct_texts(self._store, task_id)
        if content_hint:
            texts = list(texts) + [content_hint]
        return acp_messages.select_language(explicit, texts)

    def _set_state(
        self, task_id: str, state: str, *, wait_reason: str | None | object = _UNSET,
        wait_detail: Mapping[str, Any] | None | object = _UNSET, no_progress_count: int | None = None,
    ) -> None:
        import json

        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                # ``/cortex-stop`` is an urgent terminal transition and may
                # race with a coordinator continuation that already passed
                # its cancellation check.  That continuation can otherwise
                # write a transient ``blocked``/``idle`` state *after* the
                # stop transaction commits, making a stopped task appear
                # resumable.  Once the durable stop marker is visible, no
                # ordinary continuation is allowed to move the task back to
                # a non-terminal state.  Keep this guard in the same
                # transaction as the state write so the ordering is decided
                # by SQLite's writer lock rather than thread timing.
                current = db.execute(
                    "SELECT state FROM task_orchestration WHERE task_id=?",
                    (task_id,),
                ).fetchone()
                if current is not None and current["state"] == "stopped" and state != "stopped":
                    db.commit()
                    return
                # Acceptance is terminal.  A worker can publish its result
                # while the coordinator is still parked on the previous
                # ``workers_running`` projection; retaining that wait marker
                # after the acceptance transaction makes a freshly loaded
                # capsule describe a task that is already complete.  Clear
                # the transient wait fields in the same transaction as the
                # terminal state so readers never observe an accepted task
                # with a stale blocking reason.
                accepted = state == "accepted"
                if wait_reason is _UNSET and wait_detail is _UNSET and no_progress_count is None:
                    if accepted:
                        db.execute(
                            "UPDATE task_orchestration SET state=?, wait_reason=NULL, "
                            "wait_detail_json=NULL, no_progress_count=0, updated_at=? WHERE task_id=?",
                            (state, _now(), task_id),
                        )
                    else:
                        db.execute(
                            "UPDATE task_orchestration SET state=?, updated_at=? WHERE task_id=?",
                            (state, _now(), task_id),
                        )
                else:
                    set_clauses = ["state=?", "updated_at=?"]
                    params: list[Any] = [state, _now()]
                    if wait_reason is not _UNSET:
                        set_clauses.append("wait_reason=?")
                        params.append(wait_reason)
                    if wait_detail is not _UNSET:
                        set_clauses.append("wait_detail_json=?")
                        params.append(json.dumps(dict(wait_detail), sort_keys=True, ensure_ascii=False) if wait_detail else None)
                    if no_progress_count is not None:
                        set_clauses.append("no_progress_count=?")
                        params.append(no_progress_count)
                    params.append(task_id)
                    db.execute(
                        f"UPDATE task_orchestration SET {', '.join(set_clauses)} WHERE task_id=?", params,
                    )
                if accepted:
                    # ``work_assignments`` is a graph record rather than the
                    # acceptance authority, but leaving a published worker's
                    # assignment in ``planned`` makes the terminal capsule
                    # internally contradictory.  Reconcile only assignments
                    # whose attempt is durably reported; an absent or failed
                    # result must never be upgraded by acceptance.
                    db.execute(
                        "UPDATE work_assignments SET status='reported' "
                        "WHERE task_id=? AND status IN ('planned','running') "
                        "AND EXISTS ("
                        "SELECT 1 FROM work_assignment_attempts a "
                        "WHERE a.assignment_id=work_assignments.assignment_id "
                        "AND a.status='reported'"
                        ")",
                        (task_id,),
                    )
                db.commit()
            except BaseException:
                db.rollback()
                raise

    def _emit_and_record(self, task_id: str, emit_update: Callable[[str], bool], text: str) -> None:
        # T020: never repeat the exact same status line back to back (same
        # task, same text) -- see acp.messages.Dedup. Still recorded
        # durably every time it is *called* (the durable event log is never
        # deduplicated by this guard, only the host-visible chunk is), so
        # nothing is lost from the audit trail, only from the live stream.
        if self._status_dedup.should_emit(task_id, "text", dict(text=text)):
            emit_update(text)
        delegation_events.append_event(
            self._store, task_id, "progress", f"progress:{new_id('p_')}", dict(note=text), origin="runtime",
        )

    def _emit_localized(self, task_id: str, emit_update: Callable[[str], bool], language: str, key: str, **params) -> None:
        """Like :meth:`_emit_and_record` but selects EN/RU text from
        ``acp.messages`` and de-duplicates by ``(key, params)`` -- not the
        rendered text -- so a dedup decision never depends on which
        language happened to be selected."""
        if self._status_dedup.should_emit(task_id, key, params):
            emit_update(acp_messages.render(key, language, **params))
        delegation_events.append_event(
            self._store, task_id, "progress", f"progress:{new_id('p_')}",
            dict(note=acp_messages.render(key, language, **params), message_key=key, message_params=params),
            origin="runtime",
        )

    # -- T035: worker-assignment ACP tool-call cards ----------------------

    def _delegation_requested_payload(self, task_id: str, assignment_ref: str) -> Mapping[str, Any] | None:
        import json as _json

        with self._store.connection() as db:
            row = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='delegation_requested' ORDER BY id LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone()
        return _json.loads(row["payload_json"]) if row is not None else None

    def _delegation_route_label(self, task_id: str, assignment_ref: str) -> str | None:
        """Which engine, model and effort this worker is actually running
        on, read from its own configuration revision -- never from the
        task default, which a fallback or a per-role route may not match.

        ``None`` when provisioning has not recorded a revision for this
        assignment yet, or the revision has since gone: a card without the
        line is better than a card asserting a route that was not used.
        """
        import json

        try:
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT attempt_id, config_revision FROM delegation_provisioning "
                    "WHERE task_id=? AND idempotency_key=?",
                    (task_id, assignment_ref),
                ).fetchone()
                if row is None or not row["config_revision"]:
                    return None
                revision = db.execute(
                    "SELECT route_engine, parameters FROM configuration_revisions WHERE id=?",
                    (row["config_revision"],),
                ).fetchone()
                # ``configuration_revisions`` never stores the model; this
                # attempt's actual engine/model is what ``live_adapters
                # ._record_route_audit`` wrote when it dispatched it, which
                # for a worker is its own route, not the coordinator's.
                audit = db.execute(
                    "SELECT engine, model, decision_detail_json FROM attempt_route_audit "
                    "WHERE attempt_id=? ORDER BY recorded_at DESC LIMIT 1",
                    (row["attempt_id"],),
                ).fetchone() if row["attempt_id"] else None
            if revision is None:
                return None
            parameters = json.loads(revision["parameters"] or "{}")
        except Exception:  # noqa: BLE001 - a label is never worth failing a card over
            return None
        engine = (audit["engine"] if audit else None) or revision["route_engine"]
        parts = [catalog_labels.engine_short_name(engine)]
        model = (audit["model"] if audit else None) or parameters.get("model")
        if model and model != "unknown":
            parts.append(str(model))
        # The attempt's own effort when it was audited with one -- for a
        # worker that is the only place it exists; the revision's own
        # parameters are the coordinator's. Each engine names its own
        # effort knob (see configuration.policy).
        audited_effort = None
        if audit is not None:
            try:
                audited_effort = json.loads(audit["decision_detail_json"] or "{}").get("effort")
            except Exception:  # noqa: BLE001 - a label never fails a card
                audited_effort = None
        if audited_effort:
            parts.append(str(audited_effort))
        else:
            for key in ("effort", "reasoning_effort", "thought_level"):
                value = parameters.get(key)
                if value:
                    parts.append(str(value))
                    break
        return " · ".join(parts)

    def _delegation_provisioning_role(self, task_id: str, assignment_ref: str) -> str | None:
        """Best-effort resolved role id for a delegation, once
        ``ProvisioningService.delegate`` has actually resolved and recorded
        one (``delegation_provisioning.role_id`` -- see that module).
        ``None`` before provisioning has run that far (e.g. an unknown-role
        delegation refused before ``resolve_role`` ever succeeded, so no
        row was ever inserted) -- the caller then keeps the raw role hint
        it already has instead of guessing a resolved id."""
        with self._store.connection() as db:
            row = db.execute(
                "SELECT role_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                (task_id, assignment_ref),
            ).fetchone()
        return row["role_id"] if row is not None else None

    def _superseded_by_another_coordinator(self, task_id: str, attempt_ref: str | None) -> bool:
        """True when this task's coordinator attempt is no longer the one
        this turn was running -- i.e. another client's session took the
        thread over and fenced this one."""
        if not attempt_ref:
            return False
        with self._store.connection() as db:
            row = db.execute(
                "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?", (task_id,),
            ).fetchone()
        return row is not None and row["coordinator_attempt_ref"] not in (None, attempt_ref)

    def _coordinator_facing_refs(self, task_id: str, events) -> dict[str, str]:
        """Attempt id -> delegation ref, for the event types a worker keys by
        its own attempt id. Only entries that actually resolve are returned,
        so an unresolvable one keeps its raw ref rather than vanishing."""
        refs = {}
        for event in events:
            if event.event_type not in ("worker_result", "worker_question"):
                continue
            attempt_id = event.assignment_ref
            if not attempt_id or attempt_id in refs:
                continue
            resolved = self._assignment_ref_for_attempt(task_id, attempt_id)
            if resolved is not None:
                refs[attempt_id] = resolved
        return refs

    def _assignment_ref_for_attempt(self, task_id: str, attempt_id: str) -> str | None:
        """``worker_result`` events are keyed by the worker's own attempt id
        (``channel.attempt_id`` -- see
        ``delegation.mcp_tools._worker_publish_result``), a different id
        space from the coordinator-facing delegation ``assignment_ref``
        (the idempotency key) every other assignment lifecycle event uses;
        resolves attempt id back to that same delegation ref so a
        ``worker_result`` updates the exact same card its
        ``delegation_requested`` created, never a second, unrelated one."""
        with self._store.connection() as db:
            row = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?",
                (task_id, attempt_id),
            ).fetchone()
        return row["idempotency_key"] if row is not None else None

    def _emit_assignment_tool_calls(
        self, task_id: str, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None, claimed,
    ) -> None:
        """T035: render each delegated assignment's lifecycle as one ACP
        ToolCall card (``acp.protocol.tool_call``/``tool_call_update``),
        created once and updated in place -- from the SAME claimed batch
        :meth:`_continuation_loop` already reads for the coordinator's own
        continuation prompt (``claim_unconsumed`` claims every event type,
        not only ``delegation_events.MEANINGFUL_EVENT_TYPES`` -- see that
        function's docstring), so this observes ``delegation_requested``/
        ``assignment_ready`` exactly once each, the same instant the
        coordinator continuation loop itself does.

        A cancelled assignment is rendered ``failed`` (the pinned
        ``ToolCallStatus`` schema has no ``cancelled`` value -- see the
        module-level docstring near ``_TOOL_CALL_STATUS_FOR_CANCELLED``)
        with the cancellation reason carried in its content, so it stays
        distinguishable from an ordinary failure by its text, never by a
        status a client cannot render.

        A no-op entirely when ``emit_tool_call`` is ``None`` (see
        :meth:`start_prompt`'s docstring) -- every pre-T035 caller/test."""
        if emit_tool_call is None:
            return
        for event in claimed:
            if event.event_type == "delegation_requested":
                assignment_ref = event.assignment_ref
                if not assignment_ref:
                    continue
                self._bind_worker_card_surface(task_id, assignment_ref)
                with self._lock:
                    self._worker_card_emitters.setdefault(assignment_ref, emit_tool_call)
                role_hint = event.payload.get("role", "?")
                work_title = event.payload.get("title", "")
                self._emit_worker_card(
                    emit_tool_call, assignment_ref, only_if_new=True,
                    title=f"Worker: {_display_role(role_hint)} — {work_title}",
                    kind=_WORKER_TOOL_CALL_KIND, status="pending",
                )
            elif event.event_type == "assignment_ready" and event.assignment_ref:
                assignment_ref = event.assignment_ref
                fields: dict[str, Any] = dict(status="in_progress")
                role_id = self._delegation_provisioning_role(task_id, assignment_ref)
                if role_id:
                    requested = self._delegation_requested_payload(task_id, assignment_ref)
                    if requested is not None:
                        # Requirement: the title names the *resolved* role
                        # id, not the raw hint the coordinator typed --
                        # this is the first point a resolved id exists.
                        route = self._delegation_route_label(task_id, assignment_ref)
                        who = _display_role(role_id) + (f" · {route}" if route else "")
                        base_title = f"Worker: {who} — {requested.get('title', '')}"
                        fields["title"] = base_title
                        with self._lock:
                            self._worker_card_title[assignment_ref] = base_title
                # The route is in the title. The body belongs to the
                # worker's own actions, and putting anything else there
                # takes the space they are for.
                self._emit_worker_card(emit_tool_call, assignment_ref, **fields)
            elif event.event_type == "worker_result" and event.assignment_ref:
                assignment_ref = self._assignment_ref_for_attempt(task_id, event.assignment_ref)
                if assignment_ref is None:
                    continue
                with self._lock:
                    worker_in_flight = assignment_ref in getattr(self, "_worker_turns_in_flight", set())
                    if worker_in_flight:
                        # publish_result is durable evidence already.  The
                        # ACP card is terminal only after the worker turn's
                        # final response has crossed the card emitter.
                        pending_results = getattr(self, "_pending_worker_results", None)
                        if pending_results is None:
                            pending_results = self._pending_worker_results = {}
                        pending_results.setdefault(
                            assignment_ref, dict(event.payload or {})
                        )
                if worker_in_flight:
                    continue
                self._close_assignment(task_id, assignment_ref)
                checks = event.payload.get("checks") or []
                lines = [event.payload.get("summary", "")]
                with self._lock:
                    has_final_response = assignment_ref in self._worker_final_response_seen
                self._emit_worker_card(
                    emit_tool_call, assignment_ref, status="completed",
                    entry=("Result: " + _compact_text(lines[0], _WORKER_ACTIVITY_MAX_CHARS - 8)
                           if lines[0] and not has_final_response else None),
                )
            elif event.event_type == "assignment_failed" and event.assignment_ref:
                self._close_assignment(task_id, event.assignment_ref)
                text = f"Not delegated/failed: {event.payload.get('error', 'unknown error')}"
                detail = event.payload.get("detail")
                if detail:
                    text += f" {detail}"
                self._emit_worker_card(
                    emit_tool_call, event.assignment_ref, status="failed",
                    entry=scrub_text(text, max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
            elif event.event_type == "assignment_cancelled" and event.assignment_ref:
                self._close_assignment(task_id, event.assignment_ref)
                text = f"Cancelled: {event.payload.get('reason', '')}"
                self._emit_worker_card(
                    emit_tool_call, event.assignment_ref,
                    status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                    entry=scrub_text(text, max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )

    def _close_assignment(self, task_id: str, assignment_ref: str) -> None:
        """This assignment has reached a terminal event: it is no longer a
        reason to keep waiting. See the no-progress branch."""
        with self._lock:
            open_refs = self._open_assignments.get(task_id)
            if open_refs is not None:
                open_refs.discard(assignment_ref)
                if not open_refs:
                    self._open_assignments.pop(task_id, None)
            # No further worker activity is relevant once a terminal event
            # has been observed.  The terminal fence in _emit_worker_card
            # also handles an already-snapshotted tick racing this cleanup.
            self._worker_activity.pop(assignment_ref, None)

    def _append_worker_entry(self, assignment_ref: str, entry: str, ref: str | None = None) -> None:
        """Add one worker event, replacing the prior line for the same tool."""
        line = str(entry).strip()
        if not line:
            return
        with self._lock:
            log = self._worker_action_log.setdefault(assignment_ref, [])
            if ref:
                refs = self._worker_action_refs.setdefault(assignment_ref, {})
                index = refs.get(ref)
                if index is not None and index < len(log):
                    log[index] = line
                    return
                evicted = getattr(self, "_worker_action_evicted_refs", {}).setdefault(assignment_ref, [])
                if not hasattr(self, "_worker_action_evicted_refs"):
                    self._worker_action_evicted_refs = {assignment_ref: evicted}
                # A provider can deliver a late status update after its
                # logical row has rolled out of the 50-entry view.  Do not
                # resurrect that stale call at the end of the card.  The
                # tombstone list is itself bounded; unrelated new refs still
                # enter normally.
                if ref in evicted:
                    return
                refs[ref] = len(log)
                # Distinct refs are distinct logical calls even when their
                # compact text happens to be identical.
                log.append(line)
            elif not log or log[-1] != line:
                log.append(line)

    def _record_worker_activity(
        self, task_id: str, assignment_ref: str, attempt_id: str | None,
        activity_ref: str, text: str, *, status: str | None = None,
        input_value: Any = None, output_value: Any = None, error_value: Any = None,
        content_value: Any = None, kind: str | None = None,
        title: str | None = None, locations_value: Any = None,
        stream_sequence: int | None = None, provider_event_id: str | None = None,
    ) -> bool:
        """Persist one redacted worker activity row for the Workers panel.

        ``progress`` is intentionally non-meaningful, so these rows are
        durable panel history and never become coordinator continuation input.
        The panel joins by the worker attempt id; the host card remains the
        live presentation path and is emitted only after this write succeeds.
        """
        if self._store is None or not attempt_id or not text:
            return False
        payload = {
            "kind": "worker_activity",
            "text": scrub_text(str(text), max_chars=max(4000, len(str(text)) * 2 + 1000)),
            "status": status or "in_progress",
            "activity_ref": str(activity_ref),
            "attempt_id": str(attempt_id),
        }
        # Message chunks are transport events rather than logical tool rows.
        # Keep their per-turn sequence in the durable payload so a replay can
        # reuse the exact identity.  A provider event id, when one is
        # available to a future adapter, takes precedence over the sequence
        # for idempotent replay; the sequence remains useful evidence in the
        # payload and prevents distinct equal-text chunks from being merged.
        if stream_sequence is not None:
            payload["stream_sequence"] = int(stream_sequence)
        if provider_event_id is not None:
            payload["provider_event_id"] = str(provider_event_id)
        # Preserve the observable ACP update in the durable panel event. Raw
        # values are redacted below without size/depth limits: truncating
        # rawInput, rawOutput, or content makes panel replay lossy.
        if kind is not None:
            payload["tool_kind"] = kind
        if title is not None:
            payload["title"] = scrub_text(title, max_chars=max(4000, len(title) * 2 + 1000))
        if locations_value is not None:
            payload["locations"] = _client_safe_raw(locations_value)
        if input_value is not None:
            input_value = _client_safe_raw(input_value)
            payload["raw_input"] = input_value
            payload["input"] = input_value
        if output_value is not None:
            output_value = _client_safe_raw(output_value)
            payload["raw_output"] = output_value
            payload["output"] = output_value
        if error_value is not None:
            error_value = _client_safe_raw(error_value)
            payload["error"] = error_value
        if content_value is not None:
            payload["content"] = _client_safe_raw(content_value)
        try:
            digest = hashlib.sha256(
                json.dumps(payload, sort_keys=True, ensure_ascii=False, default=str).encode("utf-8")
            ).hexdigest()[:24]
            if provider_event_id is not None:
                identity = f"{activity_ref}:event:{provider_event_id}"
            elif stream_sequence is not None:
                identity = f"{activity_ref}:sequence:{int(stream_sequence)}"
            else:
                identity = str(activity_ref)
            stable_ref = f"{identity}:{payload['status']}:{digest}"
            delegation_events.append_event(
                self._store, task_id, "progress",
                f"worker_activity:{attempt_id}:{stable_ref}", payload, "runtime",
                assignment_ref=attempt_id,
            )
            return True
        except Exception:  # noqa: BLE001 - panel history must not break work
            return False
    def _worker_card_emitter(self, task_id: str, assignment_ref: str):
        """Return the feed that created this assignment's card.

        The task-level emitter is only a fallback for legacy/test callers
        that did not register an assignment binding.  Real worker updates
        always use the binding captured at dispatch, so a later ACP
        reconnect cannot redirect an old worker into the new session.
        """
        # This helper is called both inside and outside ``self._lock`` from
        # engine callbacks.  Dict reads are atomic under the supported
        # CPython runtime; callers that mutate the maps do so under the lock.
        return self._worker_card_emitters.get(assignment_ref) or self._task_tool_call_emitters.get(task_id)

    def _durable_worker_card_sessions(self, task_id: str, assignment_ref: str) -> dict[str, str]:
        """Return each session's latest durable status for this exact card."""
        card_id = _worker_tool_call_id(assignment_ref)
        result: dict[str, str] = {}
        try:
            with self._store.connection() as db:
                rows = db.execute(
                    "SELECT h.session_id, h.update_json FROM host_updates h "
                    "JOIN acp_host_sessions s ON s.session_id=h.session_id "
                    "WHERE s.task_id=? ORDER BY h.seq",
                    (task_id,),
                ).fetchall()
            for row in rows:
                try:
                    update = json.loads(row["update_json"] or "{}")
                except (TypeError, ValueError):
                    continue
                if update.get("toolCallId") == card_id and isinstance(update.get("status"), str):
                    result[row["session_id"]] = update["status"]
        except (sqlite3.Error, StoreError):
            return {}
        return result

    def _persist_terminal_worker_card(self, session_id: str, assignment_ref: str, reason: str) -> None:
        """Persist a terminal update to the card's owning session only."""
        from . import replay_store

        replay_store.append_host_update(
            self._store, session_id,
            dict(
                sessionUpdate="tool_call_update", toolCallId=_worker_tool_call_id(assignment_ref),
                status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                content=[dict(type="text", text=scrub_text(f"Cancelled: {reason}", max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS))],
            ),
        )

    def _bind_worker_card_surface(self, task_id: str, assignment_ref: str) -> None:
        """Bind the task's client surface to this assignment's card."""
        with self._lock:
            panel_modes = getattr(self, "_worker_card_panel_mode", None)
            if panel_modes is None:
                panel_modes = self._worker_card_panel_mode = {}
            panel_modes[assignment_ref] = bool(
                getattr(self, "_worker_panel_mode", {}).get(task_id, False)
            )

    def _emit_worker_card(self, emit_tool_call, assignment_ref: str, *, only_if_new: bool = False, **fields) -> None:
        """Every update to a worker's assignment card goes through here.

        A card's create used to be emitted by the event-claim path (which
        only runs between coordinator turns) while its updates came from
        the wait/provisioning path -- so once a delegation started while
        the turn that asked for it was still running, an update could
        reach the client before the create. A client answers an update for
        a card it never saw with "Tool call not found", and the card -- the
        delegation itself -- never appears at all.

        So no caller decides ``new`` any more: this does, from whether this
        card has actually been emitted for this task yet. The default title
        is filled in for a create that arrives update-shaped, because a
        card must be named when it is created.

        The body is not a caller's to set. Queue, provisioning and result
        paths contribute lifecycle ``entry`` values; this method keeps
        their accumulated history instead of replacing one with another.
        Engine actions and messages are appended inside this same card.
        """
        if emit_tool_call is None or not assignment_ref:
            return
        card_id = _worker_tool_call_id(assignment_ref)
        incoming_status = fields.get("status")
        with self._lock:
            terminal_status = self._worker_card_terminal.get(card_id)
            if terminal_status:
                if incoming_status not in {"completed", "failed"}:
                    # A stale _emit_worker_activity_tool_calls snapshot arrived
                    # after the result/failure update. ACP clients treat this as
                    # a live card and Paseo starts its shimmer again.
                    return
                if incoming_status != terminal_status:
                    # Terminal outcomes are durable and first-writer-wins:
                    # accepting a later opposite status would leave clients
                    # with contradictory completed/failed histories.
                    return
            elif incoming_status in {"completed", "failed"}:
                self._worker_card_terminal[card_id] = incoming_status
        entry = fields.pop("entry", None)
        entry_ref = fields.pop("entry_ref", None)
        display_project_root = fields.pop("_project_root", None)
        if display_project_root:
            with self._lock:
                roots = getattr(self, "_worker_card_project_root", None)
                if roots is None:
                    roots = self._worker_card_project_root = {}
                roots[assignment_ref] = str(display_project_root)
        if entry:
            self._append_worker_entry(assignment_ref, entry, entry_ref)
        fields.pop("text", None)
        with self._lock:
            transcript = list(self._worker_action_log.get(assignment_ref) or ())
        body = _joined_worker_actions(transcript)
        if body:
            with self._lock:
                project_root = getattr(self, "_worker_card_project_root", {}).get(assignment_ref)
            mirror = _worker_mirror_text(body, project_root)
            with self._lock:
                panel_modes = getattr(self, "_worker_card_panel_mode", {})
                # Keep the assignment-keyed fallback for pre-task callers
                # and focused tests that set the legacy map directly.
                panel_mode = bool(panel_modes.get(assignment_ref,
                    getattr(self, "_worker_panel_mode", {}).get(assignment_ref, False)))
            if panel_mode:
                final_lines = [line for line in mirror.splitlines() if line.startswith("[final response]")]
                if final_lines:
                    mirror = final_lines[-1] + "\nOpen Workers in the header for full history"
                elif incoming_status == "failed":
                    mirror = "Worker failed.\nOpen Workers in the header for full history"
                elif incoming_status == "completed":
                    mirror = "Worker completed.\nOpen Workers in the header for full history"
                else:
                    mirror = "Worker is working…\nOpen Workers in the header for full history"
            fields["text"] = mirror
            # Paseo 0.8's ACP bridge drops ToolCallContent and renders object
            # rawInput as JSON. Keep a plain-text cumulative mirror instead:
            # the bridge preserves it, and Paseo displays the complete
            # sequential worker rows (including a terminal Result line) as
            # readable text without a separate Output section.
            fields["raw_input"] = mirror
            # Paseo 0.8 validates a failed timeline item with a non-null
            # ``error`` and its supported ACP reducer obtains that value from
            # rawOutput. Reuse the bounded, redacted failure transcript that
            # is already recorded in this card. This is genuine assignment
            # detail, not synthesized success/error evidence.
            if incoming_status == "failed":
                fields["raw_output"] = mirror
        with self._lock:
            first = card_id not in self._emitted_worker_cards
            if first:
                self._emitted_worker_cards.add(card_id)
        if not first and only_if_new:
            # The create already happened -- at dispatch, which is where a
            # delegation's card is owned. Re-sending the queued state from
            # the event-claim path would be the same card twice.
            return
        payload = dict(fields)
        payload["tool_call_id"] = card_id
        payload["new"] = first
        if first:
            payload.setdefault("kind", _WORKER_TOOL_CALL_KIND)
            payload.setdefault("status", "pending")
            if not payload.get("title"):
                payload["title"] = "Worker assignment"
        # ACP updates are partial. Compare the effective rendered state,
        # rather than each update's subset of fields: reattaching an
        # unchanged title after a tool event must not redraw the card.
        with self._lock:
            previous = self._worker_card_sent.get(card_id, {})
            rendered = {**previous, **{k: v for k, v in payload.items() if k != "new"}}
            if not first and rendered == previous:
                return
            self._worker_card_sent[card_id] = rendered
        emit_tool_call(payload)

    def _note_native_subagent(self, task_id: str | None, label: str) -> None:
        """Record that a coordinator used its engine's own agent surface.

        This runtime cannot stop it -- the engine owns that tool and does
        not ask permission for it -- so what it can do is make it never
        pass unnoticed: the coordinator is told next turn, exactly like a
        refused mutation, and the event is durable.
        """
        if task_id is None or self._store is None:
            return
        try:
            delegation_events.append_event(
                self._store, task_id, "progress", f"native_subagent:{label[:60]}",
                dict(note=acp_messages.render("native_subagent_refused", "en"), detail=label[:200]),
                origin="runtime",
            )
        except Exception:  # noqa: BLE001 - noticing must never break the turn
            pass

    def _engine_tool_call_emitter(
        self, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None,
        *, prefix: str = "", id_scope: str = "coordinator", language: str = "en",
        project_root: str | None = None, task_id: str | None = None,
        cancel_event: threading.Event | None = None,
    ):
        """A callback for ``SessionRuntime.prompt``'s ``on_tool_call`` that
        renders each of that session's own engine tool calls as an ACP
        ToolCall card -- what is being read, edited, searched or run, while
        it happens, which is otherwise visible only in durable evidence
        nobody watching the session can see.

        Carries the safe, portable fields supplied by the engine adapter:
        ref, kind, title, status, locations and display content. Ordinary
        project and provider calls are shown; Cortex MCP operations and
        private-store file actions are runtime bookkeeping and are filtered.
        Engine private arguments and provider-specific payloads are
        unavailable.

        ``prefix`` labels whose call it is once more than one session is
        working (a worker's role). It may be a callable, resolved at each
        call instead of once up front: a worker's route is written down by
        the runtime as the attempt is dispatched, and a prefix built
        before that row lands names the engine and model from the
        coordinator's own revision but has no effort to name -- which is
        exactly how a header lost the third of the three things it was
        asked to show. ``id_scope`` keeps two sessions'
        cards from colliding when both engines happen to use the same
        wire tool-call id. Returns ``None`` when no card channel is wired,
        which every pre-existing caller relies on.
        """
        if emit_tool_call is None:
            return None
        # Per call ref, the last title the engine actually gave for it.
        # Both engines name a call when they create it and then send
        # status-only updates with no title at all, so an update must
        # never overwrite the name with a placeholder -- that is exactly
        # how a feed of real actions turned into a column of "tool call".
        titles: dict[str, str] = {}
        inputs: dict[str, Any] = {}
        outputs: dict[str, Any] = {}
        locations_by_ref: dict[str, Any] = {}
        contents_by_ref: dict[str, Any] = {}
        hidden_wait_refs: set[str] = set()
        pending_titleless: dict[str, tuple[Any, ...]] = {}
        # Providers can announce a generic ``Terminal`` envelope before the
        # command/path arrives.  Do not create a visible grouped card for the
        # placeholder; the first concrete update for the same ref will pass
        # through and create the card with one useful line.
        deferred_placeholder_refs: set[str] = set()
        deferred_file_activity: set[str] = set()
        # Each observable call gets its own ACP card.  ACP has no portable
        # grouping primitive, and cumulative cards hide the order and status
        # of real project/MCP work behind a JSON-like transcript.

        def close_group() -> None:
            """Compatibility hook for callers at message boundaries."""
            return None

        def on_tool_call(
            ref: str, kind: str | None, title: str | None, status: str | None,
            locations=None, content=None, raw_input=None, raw_output=None,
        ) -> None:
            card_id = f"call:{id_scope}:{ref}"
            if ref and raw_input is not None:
                inputs[ref] = raw_input
            elif ref:
                raw_input = inputs.get(ref)
            if ref and raw_output is not None:
                outputs[ref] = raw_output
            elif ref:
                raw_output = outputs.get(ref)
            if ref and locations is not None:
                locations_by_ref[ref] = locations
            elif ref:
                locations = locations_by_ref.get(ref)
            if ref and content is not None:
                contents_by_ref[ref] = content
            elif ref:
                content = contents_by_ref.get(ref)
            supplied_title = isinstance(title, str) and bool(title.strip())
            known = titles.get(ref)
            if supplied_title:
                known = _human_tool_title(scrub_display_text(
                    _project_relative(title.strip(), project_root),
                    max_chars=max(4000, len(title) * 2 + 1000),
                ))
                titles[ref] = known
            fields: dict[str, Any] = dict(
                tool_call_id=card_id, status=_acp_tool_call_status(status),
            )
            # The assignment lifecycle card is the coordinator-facing
            # representation of delegate. Filter using the provider title
            # before humanizing it to the short ``Delegate`` label.
            if isinstance(title, str) and _COORDINATOR_DELEGATE_TOOL_CALL.match(title.strip()):
                hidden_wait_refs.add(ref)
                pending_titleless.pop(ref, None)
                return
            label = known or title or ""
            if (_AMBIGUOUS_FILE_ACTIVITY.match(label)
                    and not (locations or raw_input is not None or content)):
                # Wait for the provider's path-bearing update. This avoids
                # leaking a generic internal report edit before its target is
                # known, while still allowing a real project edit through.
                deferred_file_activity.add(ref)
                return
            deferred_file_activity.discard(ref)
            # Filter using the provider's original title before
            # ``_human_tool_title`` rewrites Cortex MCP names such as
            # ``mcp__cortex__read_skill`` to the indistinguishable ``Read
            # skill`` label. The humanized label is retained as a fallback
            # for status-only updates whose original title is absent.
            if (_is_engine_internal_tool(title, kind, locations, raw_input, content)
                    or _is_engine_internal_tool(label, kind, locations, raw_input, content)
                    or ref in hidden_wait_refs):
                hidden_wait_refs.add(ref)
                pending_titleless.pop(ref, None)
                return
            if ref and supplied_title and ref in pending_titleless and _NATIVE_SUBAGENT.search(label):
                # Refuse a provider subagent once; the provisional unnamed
                # event is part of that same opaque provider call.
                pending_titleless.pop(ref, None)
            elif ref and supplied_title and ref in pending_titleless:
                # A status-only create is ambiguous until the engine names
                # it.  Replay that start now that its title is known, then
                # continue with the named update in wire order.  A named
                # ``wait`` took the early branch above and drops both.
                pending = pending_titleless.pop(ref)
                on_tool_call(ref, pending[0], title, pending[1], pending[2], pending[3], pending[4], pending[5])
                if pending[1] == "failed":
                    return
            elif ref and not supplied_title and ref in pending_titleless and _acp_tool_call_status(status) == "failed":
                prior = pending_titleless[ref]
                pending_titleless[ref] = (prior[0], "failed", prior[2], prior[3], prior[4],
                                          raw_output if raw_output is not None else prior[5])
                return
            elif ref and not supplied_title and _acp_tool_call_status(status) in {"pending", "in_progress"}:
                prior = pending_titleless.get(ref)
                if prior:
                    kind = prior[0] or kind
                    status = ("failed" if prior[1] == "failed" else status)
                    locations = locations if locations is not None else prior[2]
                    content = content if content is not None else prior[3]
                    raw_input = raw_input if raw_input is not None else prior[4]
                    raw_output = raw_output if raw_output is not None else prior[5]
                pending_titleless[ref] = (kind, status, locations, content, raw_input, raw_output)
                return
            if _NATIVE_SUBAGENT.search(label):
                # Provider-owned subagents are engine machinery. Cancel the
                # turn as before, but keep that machinery out of the user
                # timeline entirely.
                if cancel_event is not None:
                    # The ACP adapter's prompt loop watches this event and
                    # sends the wire cancellation independently of whether
                    # another engine update arrives.  A refusal card alone
                    # would leave a provider-owned child running.
                    cancel_event.set()
                self._note_native_subagent(task_id, label)
                return
            if ref and not _has_concrete_tool_activity(known or title, locations, content, raw_input):
                deferred_placeholder_refs.add(ref)
                return
            deferred_placeholder_refs.discard(ref)
            if known is None:
                # Some engines send a status before a title, or never send
                # one. Preserve the same honest fallback in grouped text as
                # the ordinary per-call renderer uses.
                known = f"Tool call {ref}" if ref else "Tool call"
            # The title fallback above also applies to the ordinary card
            # path; a later named update replaces it on the same card.
            if ref not in seen:
                seen.add(ref)
                fields["new"] = True
                fields["kind"] = _acp_tool_call_kind(kind)
            else:
                fields["new"] = False
            resolved_prefix = prefix() if callable(prefix) else prefix
            fields["title"] = f"{resolved_prefix}{_human_tool_title(known)}" if resolved_prefix else _human_tool_title(known)
            # Keep the collapsed card readable while retaining the complete
            # structured payload for Paseo's expanded input/output view. The
            # host journal persists these fields verbatim after the normal
            # credential redaction, so session/load replays the same detail.
            normalized_input = _native_edit_raw_input(kind, known, raw_input)
            line = _compact_tool_line(
                known, status, locations, raw_input, raw_output, project_root,
                kind=kind,
            )
            fields["text"] = line
            safe_content = _client_safe_content(content)
            if safe_content:
                fields["content"] = safe_content
            if normalized_input is not None:
                fields["raw_input"] = _client_safe_raw(normalized_input)
            if raw_output is not None:
                fields["raw_output"] = _client_safe_raw(raw_output)
            if locations:
                fields["locations"] = list(locations)
            emit_tool_call(fields)

        seen: set[str] = set()
        on_tool_call.close_group = close_group
        return on_tool_call

    def _emit_worker_activity_tool_calls(
        self, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None,
    ) -> None:
        """Show what a running worker is actually doing.

        Between ``assignment_ready`` and its result, a worker's card said
        only "in_progress" -- the reported complaint was "I can see the
        worker started, but not what it is doing". This updates the
        assignment's lifecycle card with current status. Detailed engine
        actions and messages are appended to this same assignment card.

        Card text only: a worker's text never enters the client's message
        stream, which stays the coordinator's own (see ``PromptOutcome``).
        A no-op when no card channel is wired, and when the worker has not
        produced anything yet (nothing to say is not worth an update).
        """
        if emit_tool_call is None:
            return
        with self._lock:
            activity = dict(self._worker_activity)
            titles = dict(self._worker_card_title)
        for assignment_ref, latest in activity.items():
            latest = " ".join(latest.split())
            if not latest:
                continue
            fields: dict[str, Any] = dict(status="in_progress")
            base = titles.get(assignment_ref)
            if base:
                # The title names the worker and its assignment and stays
                # put while the card body accumulates its complete activity.
                fields["title"] = scrub_text(base, max_chars=_WORKER_CARD_TITLE_MAX_CHARS)
            # _emit_worker_card renders and de-duplicates the lifecycle
            # body -- this runs on every wait tick
            # and a worker is not doing something new on every tick.
            self._emit_worker_card(emit_tool_call, assignment_ref, **fields)

    def _emit_provisioning_tool_calls(
        self, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None,
        provisioning_sig: tuple[tuple[str, str, str], ...],
    ) -> None:
        """T035 visibility fix: a worker still provisioning (opening its
        engine session, not yet ready -- see
        ``delegation_provisioning.in_progress_snapshot``) is otherwise
        completely silent from ``delegation_requested`` (pending, T035)
        until ``assignment_ready`` (in_progress) -- exactly the gap the real
        Zed session this fixes sat in for over a minute with nothing shown.
        Updates the SAME per-assignment card ``_emit_assignment_tool_calls``
        already created (never a second channel/card), so its title/history
        stay one continuous timeline. A no-op when ``emit_tool_call`` was
        never wired, same convention as ``_emit_assignment_tool_calls``."""
        if emit_tool_call is None:
            return
        for assignment_ref, step, _updated_at in provisioning_sig:
            self._emit_worker_card(
                emit_tool_call, assignment_ref, status="in_progress",
            )

    def _handle_ask_user(
        self, task_id: str, session_id: str, emit_update: Callable[[str], bool], language: str, decision: dict,
        elicitor: HostElicitor | None,
    ) -> bool:
        """T020: persist the pending question *before* any host contact,
        then try structured elicitation (only when a ``HostElicitor`` was
        injected -- see the class docstring), falling back to the plain
        text question otherwise. Returns ``True`` iff an answer was
        captured (the caller then continues the SAME turn instead of
        ending it), ``False`` otherwise."""
        question = questions_store.create_if_absent(
            self._store, task_id, f"ask_user:{decision['decision_id']}", decision["question"],
            options=decision.get("options"),
        )
        decision["question_id"] = question.id
        if question.status != "open":
            # The terminal decision was inspected before this row was
            # answered, so create_if_absent can observe the answer winning
            # the race. Continue so the caller re-checks the durable state;
            # otherwise it would incorrectly park an answered question in
            # waiting_for_user. Cancellation/supersession remains terminal.
            return question.status == "answered"
        if elicitor is not None:
            if hasattr(elicitor, "set_question_id"):
                elicitor.set_question_id(question.id)
            try:
                answer_text = elicitor.elicit(session_id, decision["question"], decision.get("options"))
            except Exception:
                answer_text = None
            if answer_text is not None:
                if questions_store.answer(self._store, question.id, answer_text, "elicitation"):
                    delegation_events.append_event(
                        self._store, task_id, "progress", f"question_answered:{question.id}",
                        dict(kind="question_answered", question_id=question.id, answer_text=answer_text),
                        origin="runtime",
                    )
                    # A user_message (meaningful) event, distinct from the
                    # informative progress row above, so the continuation
                    # loop's claim/coalesce gating
                    # (delegation_events.MEANINGFUL_EVENT_TYPES) actually
                    # delivers this answer to the coordinator on the next
                    # iteration -- exactly like acp.orchestrating_runtime
                    # .answer_question's own /cortex-answer text-fallback
                    # delivery.
                    delegation_events.append_event(
                        self._store, task_id, "user_message", f"elicitation_answer:{question.id}",
                        dict(content=f"Answered question ({question.text!r}) via elicitation: {answer_text}", session_id=session_id),
                        origin=session_id,
                    )
                    return True
        # Hosts without ACP elicitation still need to see the choices. Keep
        # this as one ordinary text update so the next user prompt can answer
        # the same durable pending question through the normal text path.
        options = decision.get("options")
        if isinstance(options, list) and options:
            choices = "\n".join(
                f"{index}. {choice}" for index, choice in enumerate(options, start=1)
                if isinstance(choice, str) and choice
            )
            label = acp_messages.render("question_options_label", language)
            emit_update(f"{decision['question']}\n\n{label}\n{choices}" if choices else decision["question"])
        else:
            emit_update(decision["question"])
        # A host callback can deliver the user's text synchronously while the
        # fallback update is being emitted. Re-read the row before returning;
        # otherwise the caller would overwrite the answered state with
        # waiting_for_user.
        latest = questions_store.get(self._store, question.id)
        return latest is not None and latest.status == "answered"

    def _capture_result_revisions(self, task_id: str, claimed) -> dict[int, dict]:
        """T019: for every ``worker_result`` in ``claimed``, durably capture
        (and return, keyed by event id) the observed artifact revision at
        this exact instant, bound to that assignment's attempt id (see
        ``delegation.revision``). A task with no resolvable project root
        (should not happen for a bound task) yields no captures rather than
        raising.

        T025: when this assignment's attempt actually ran in an isolated
        snapshot workspace (see ``delegation.workspace_isolation``), the
        revision captured here is that workspace's own tree -- not the
        shared project root's -- because that is the tree the worker's
        checks actually ran against. Since the tree digest is computed over
        *relative* paths/content (identical either way), a later successful
        ``delegation.integration.integrate`` of that exact tree onto an
        unchanged project root reproduces the same digest (still bound,
        never stale); an integration that merges in a concurrent main-root
        change, or a conflicting worker, necessarily changes the digest --
        the existing ``stale_verification`` comparison below then correctly
        refuses to let a revision-A report certify revision B. An
        attempt with no isolated workspace (serialized fallback, or
        pre-T025 data) falls back to the shared project root exactly as
        before."""
        worker_results = [event for event in claimed if event.event_type == "worker_result"]
        if not worker_results:
            return {}
        with self._store.connection() as db:
            row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        if row is None:
            return {}
        shared_root = row["project_root"]
        captures = {}
        for event in worker_results:
            if event.assignment_ref is None:
                continue
            # A worker_result event's own ``assignment_ref`` is keyed by the
            # provisioned attempt id itself (see ``delegation.completion``'s
            # ``gate`` docstring on the same worker-facing-vs-coordinator-
            # facing distinction) -- resolved directly, no
            # delegation_provisioning bridge needed here.
            workspace_root = delegation_workspace_isolation.resolve_attempt_root(
                self._store, task_id, event.assignment_ref,
            ) or shared_root
            captures[event.id] = delegation_revision.record_attempt_revision(
                self._store, event.assignment_ref, f"worker_result:{event.id}", workspace_root,
            )
        return captures

    def _check_terminal_decision(self, task_id: str):
        with self._store.connection() as db:
            row = db.execute(
                "SELECT id, payload_json FROM task_events WHERE task_id=? AND event_type='coordinator_decision' "
                "ORDER BY id DESC LIMIT 1",
                (task_id,),
            ).fetchone()
        if row is None:
            return None
        import json

        payload = json.loads(row["payload_json"])
        if payload.get("decision") == "accept" and payload.get("assignment_ref") == "task":
            return self._recheck_accept(task_id, row["id"])
        if payload.get("decision") == "ask_user":
            # Only while that question is actually still open. This is the
            # latest *decision*, not the latest state: once it was
            # answered, nothing newer overwrote it, so every later turn
            # read it again and parked the task on a question that had
            # been answered minutes earlier -- ending the coordinator's
            # turn while its workers were still running. The accept branch
            # above has re-checked for staleness since T017; this one
            # never did.
            with self._store.connection() as db:
                question_row = db.execute(
                    "SELECT status FROM pending_questions WHERE task_id=? AND dedup_key=?",
                    (task_id, f"ask_user:{row['id']}"),
                ).fetchone()
            if question_row is not None and question_row["status"] != "open":
                return None
            options = payload.get("options")
            return dict(
                kind="ask_user", decision_id=row["id"], question=payload.get("question") or "",
                options=list(options) if isinstance(options, list) else None,
            )
        return None

    def _recheck_accept(self, task_id: str, decision_row_id: int):
        """T017 review (HIGH): a task-level accept decision must never be
        honored *stale*. Re-run the same completion gate the runtime relied
        on to allow recording it in the first place (see
        ``delegation.completion.gate``) here too, at the point the runtime
        is about to actually finalize a host-success response -- the
        coordinator may have delegated (or a worker may have asked a
        question) in the *same* turn, after its own accept tool call, and
        that later event never gets a chance to gate a fresh decide() call.
        A decision with any delegation_requested/meaningful event committed
        after it is treated as superseded outright (never honored, no
        matter what the gate would otherwise say); still-running/queued
        futures for this task also block honoring it.
        """
        from ..delegation import completion as delegation_completion

        with self._store.connection() as db:
            # Deliberately excludes 'user_message': the current turn's own
            # user_message always precedes (a lower id than) the
            # coordinator's own decide() call that reacted to it -- checking
            # it here would wrongly treat every normal accept as stale. A
            # genuinely new user turn is handled by _run() starting a fresh
            # continuation from scratch, never through this recheck.
            newer = db.execute(
                "SELECT event_type, id FROM task_events WHERE task_id=? AND id>? "
                "AND event_type IN ('delegation_requested','worker_result','worker_question',"
                "'assignment_failed','assignment_cancelled') ORDER BY id LIMIT 1",
                (task_id, decision_row_id),
            ).fetchone()
        if newer is not None:
            reason = f"a newer {newer['event_type']} event (id {newer['id']}) arrived after this accept decision (id {decision_row_id})"
            delegation_events.append_event(
                self._store, task_id, "progress", f"acceptance_refused:{decision_row_id}",
                dict(note=f"Acceptance refused: {reason}."), origin="runtime",
            )
            return None
        running = bool(self._running.get(task_id))
        if running:
            delegation_events.append_event(
                self._store, task_id, "progress", f"acceptance_refused:{decision_row_id}",
                dict(note="Acceptance refused: assignment(s) still running/queued for this task."), origin="runtime",
            )
            return None
        with self._store.connection() as db:
            decision_row = db.execute(
                "SELECT payload_json FROM task_events WHERE id=? AND task_id=? "
                "AND event_type='coordinator_decision'",
                (decision_row_id, task_id),
            ).fetchone()
            try:
                decision_payload = json.loads(decision_row["payload_json"]) if decision_row else {}
            except (TypeError, ValueError):
                decision_payload = {}
            binding = decision_payload.get("pipeline_binding")
            if not delegation_completion.pipeline_binding_matches(db, task_id, binding):
                reason = (
                    "the coordinator acceptance is bound to a pipeline edition that is no longer "
                    "the current edition (or has no valid coordinator-authored pipeline binding)"
                )
                delegation_events.append_event(
                    self._store, task_id, "progress", f"acceptance_refused:{decision_row_id}",
                    dict(note=f"Acceptance refused: {reason}."), origin="runtime",
                )
                return None
        ok, reasons = delegation_completion.gate(self._store, task_id)
        if not ok:
            delegation_events.append_event(
                self._store, task_id, "progress", f"acceptance_refused:{decision_row_id}",
                dict(note="Acceptance refused: " + " ".join(reasons)), origin="runtime",
            )
            return None
        return "accepted"

    # -- waiting --------------------------------------------------------

    def _wait_for_progress(self, task_id: str, cancel_event: threading.Event) -> bool:
        """Block (bounded, no model prompt) until a meaningful event arrives.

        Returns True if progress was observed (an unconsumed event now
        exists), False if the wait timed out with nothing pending and no
        worker running (a genuine stall to count against the no-progress
        budget).
        """
        deadline = time.monotonic() + self._wait_timeout
        backoff = _WAIT_BACKOFF_START
        while time.monotonic() < deadline:
            if cancel_event.is_set():
                return True
            if delegation_events.has_unconsumed(self._store, task_id):
                return True
            with self._cond:
                running = bool(self._running.get(task_id))
                self._cond.wait(timeout=min(backoff, deadline - time.monotonic()) if deadline > time.monotonic() else 0)
                still_running = bool(self._running.get(task_id))
            if delegation_events.has_unconsumed(self._store, task_id):
                return True
            if not running and not still_running:
                return False
            backoff = min(backoff * 2, _WAIT_BACKOFF_MAX)
        return delegation_events.has_unconsumed(self._store, task_id)

    def _notify(self) -> None:
        with self._cond:
            self._cond.notify_all()

    # -- delegation_requested / assignment_cancel_requested processing --

    def _process_new_events(
        self, task_id: str, coordinator_channel: ChannelContext, cancel_event: threading.Event,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> None:
        with self._store.connection() as db:
            requests = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='delegation_requested' ORDER BY id",
                (task_id,),
            ).fetchall()
            cancels = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='assignment_cancel_requested' ORDER BY id",
                (task_id,),
            ).fetchall()
            clarifications = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='coordinator_clarification' ORDER BY id",
                (task_id,),
            ).fetchall()
            # T025: carried as 'progress' rows (see delegation.events
            # .INTEGRATION_PROGRESS_KINDS docstring on why a brand new
            # event_type is not used); the 'integrate:' dedup_key prefix is
            # this operation's own reserved namespace (see mcp_tools
            # ._coordinator_integrate), never colliding with an unrelated
            # progress note.
            integrations = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND dedup_key LIKE 'integrate:%' ORDER BY id",
                (task_id,),
            ).fetchall()
            candidates = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND dedup_key LIKE 'prepare_candidate:%' ORDER BY id",
                (task_id,),
            ).fetchall()
            checks = db.execute(
                "SELECT * FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND dedup_key LIKE 'request_check:%' ORDER BY id",
                (task_id,),
            ).fetchall()
        import json

        # P1 #7 fix: the dedup bookkeeping below (which needs the shared
        # lock, to stay race-free against a concurrent _process_new_events
        # call for the same task) is now the *only* thing done under
        # self._lock. Every external/blocking effect -- executor.submit's
        # own dispatch, _cancel_assignment, _integrate_assignment -- runs
        # after the lock is released (P1 #7's second point: no blocking
        # external operation under the shared runtime lock).
        #
        # For a submitted future specifically: this also closes the
        # deadlock itself. future.add_done_callback() invokes the callback
        # *immediately, on the calling thread* when the future is already
        # done (documented concurrent.futures behavior) -- so registering
        # it while still holding self._lock let an already-finished future
        # re-enter _on_worker_done's self._cond (built on that same
        # non-reentrant lock) on the same thread and deadlock. Ordering is
        # preserved without that risk: the future is added to self._running
        # (under a short, separate lock acquisition) strictly *before*
        # add_done_callback is ever called, so a callback -- whether fired
        # synchronously right here or later from a worker thread -- always
        # finds the future already present to discard; it can never be
        # missed, and it is never observable as still "running" once its
        # own callback has had a chance to run.
        to_delegate = []
        to_cancel = []
        to_clarify = []
        to_integrate = []
        to_prepare_candidate = []
        to_check = []
        with self._lock:
            for row in requests:
                assignment_ref = row["assignment_ref"]
                if assignment_ref in self._dispatched_delegations:
                    continue
                self._dispatched_delegations.add(assignment_ref)
                # The ref is the event's own column, carried alongside the
                # payload rather than merged into it: the payload is what
                # the delegate tool wrote, and it stays that.
                to_delegate.append((assignment_ref, json.loads(row["payload_json"])))

            for row in cancels:
                assignment_ref = row["assignment_ref"]
                if assignment_ref in self._dispatched_cancels:
                    continue
                self._dispatched_cancels.add(assignment_ref)
                to_cancel.append((assignment_ref, json.loads(row["payload_json"]).get("reason", "")))

            for row in clarifications:
                assignment_ref = row["assignment_ref"]
                key = (task_id, assignment_ref)
                session_ref = self._worker_sessions.get(key)
                if session_ref is None or row["id"] in self._delivered_clarification_ids:
                    continue
                self._delivered_clarification_ids.add(row["id"])
                message = json.loads(row["payload_json"]).get("message", "")
                to_clarify.append((key, session_ref, message, row["id"]))

            for row in integrations:
                if row["id"] in self._dispatched_integrations:
                    continue
                self._dispatched_integrations.add(row["id"])
                payload = json.loads(row["payload_json"])
                to_integrate.append((payload["assignment_ref"], payload["request_key"],
                                     payload.get("candidate_ref"), payload.get("expected_base_digest")))

            for row in candidates:
                if row["id"] in self._dispatched_candidates:
                    continue
                self._dispatched_candidates.add(row["id"])
                to_prepare_candidate.append(json.loads(row["payload_json"]))

            for row in checks:
                if row["id"] in self._dispatched_checks:
                    continue
                self._dispatched_checks.add(row["id"])
                to_check.append(json.loads(row["payload_json"]))

        for assignment_ref, payload in to_delegate:
            # The card appears when the worker is dispatched, not when the
            # coordinator's turn later ends. Delegation now starts while
            # that turn is still running, and until this ran here the
            # coordinator could truthfully say "I delegated it" with
            # nothing to show for minutes -- reported as "the worker is
            # nowhere to be seen".
            self._bind_worker_card_surface(task_id, assignment_ref)
            self._emit_worker_card(
                emit_tool_call, assignment_ref,
                title=f"Worker: {_display_role(payload.get('role') or '?')} — {payload.get('title', '')}",
                kind=_WORKER_TOOL_CALL_KIND, status="pending",
            )
            with self._lock:
                if emit_tool_call is not None:
                    # Bind once, at card creation.  Never replace this with a
                    # later session's task-level emitter while this worker is
                    # still alive.
                    self._worker_card_emitters.setdefault(assignment_ref, emit_tool_call)
                self._open_assignments.setdefault(task_id, set()).add(assignment_ref)
            future = self._executor.submit(
                self._provision_and_prompt, task_id, coordinator_channel, payload, cancel_event,
            )
            with self._lock:
                self._running.setdefault(task_id, set()).add(future)
            future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

        for assignment_ref, reason in to_cancel:
            self._cancel_assignment(task_id, assignment_ref, reason)

        for key, session_ref, message, row_id in to_clarify:
            future = self._executor.submit(self._deliver_clarification, task_id, key, session_ref, message, cancel_event, row_id)
            with self._lock:
                self._running.setdefault(task_id, set()).add(future)
            future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

        for payload in to_prepare_candidate:
            future = self._executor.submit(self._prepare_candidate, task_id, payload)
            with self._lock:
                self._running.setdefault(task_id, set()).add(future)
            future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

        for payload in to_check:
            future = self._executor.submit(self._run_candidate_check, task_id, payload)
            with self._lock:
                self._running.setdefault(task_id, set()).add(future)
            future.add_done_callback(lambda f, t=task_id: self._on_worker_done(t, f))

        for assignment_ref, request_key, candidate_ref, expected_base_digest in to_integrate:
            self._integrate_assignment(task_id, assignment_ref, request_key,
                                       candidate_ref=candidate_ref, expected_base_digest=expected_base_digest)

        self._terminate_completed_workers(task_id)

    def _prepare_candidate(self, task_id: str, payload: Mapping[str, Any]) -> None:
        """Materialize one requested candidate and publish a durable outcome."""
        from ..delegation import candidates as candidate_staging

        candidate_id = payload["candidate_id"]
        refs = payload["assignment_refs"]
        request_key = payload["request_key"]
        try:
            with self._store.connection() as db:
                placeholders = ",".join("?" for _ in refs)
                rows = db.execute(
                    f"SELECT idempotency_key,attempt_id FROM delegation_provisioning "
                    f"WHERE task_id=? AND idempotency_key IN ({placeholders})",
                    (task_id, *refs),
                ).fetchall()
            by_ref = {row["idempotency_key"]: row for row in rows}
            if set(by_ref) != set(refs):
                raise StoreError("candidate_assignment_unavailable")
            attempts = [by_ref[ref]["attempt_id"] for ref in refs]
            if any(not attempt for attempt in attempts):
                raise StoreError("candidate_attempt_unavailable")
            with self._store.connection() as db:
                terminal = {
                    row["assignment_ref"] for row in db.execute(
                        f"SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? "
                        f"AND event_type='worker_result' AND superseded=0 "
                        f"AND assignment_ref IN ({placeholders})",
                        (task_id, *attempts),
                    ).fetchall()
                }
            if terminal != set(attempts):
                raise StoreError("candidate_requires_published_results")
            candidate = candidate_staging.prepare_candidate(
                self._store, task_id, attempts, candidate_id=candidate_id,
            )
            event_payload = dict(kind="candidate_ready", candidate_id=candidate_id,
                                 assignment_refs=refs, request_key=request_key,
                                 content_digest=candidate["content_digest"],
                                 project_base_digest=candidate["project_base_digest"])
            dedup_key = f"candidate_ready:{request_key}"
        except Exception as error:
            conflict = isinstance(error, candidate_staging.CandidateConflict)
            event_payload = dict(kind="candidate_conflict" if conflict else "candidate_failed",
                                 candidate_id=candidate_id, assignment_refs=refs,
                                 request_key=request_key, error=str(error))
            if conflict:
                event_payload["conflict_paths"] = error.candidate.get("conflict_paths", [])
            dedup_key = f"candidate_outcome:{request_key}"
        delegation_events.append_event(
            self._store, task_id, "progress", dedup_key, event_payload, origin="runtime",
        )
        self._notify()

    def _run_candidate_check(self, task_id: str, payload: Mapping[str, Any]) -> None:
        """Observe one exact staged candidate without replaying an uncertain command."""
        from ..delegation import candidate_checks, check_requests

        check_id = payload["check_id"]
        request = check_requests.get_check_request(self._store, check_id)
        if request is None or request["task_id"] != task_id:
            return
        if request["status"] == "running":
            # The process may have exited between persisting its observed
            # evidence and recording completion. Reconcile that exact row;
            # otherwise preserve the external command's unknown outcome.
            try:
                with self._store.connection() as db:
                    receipt = db.execute(
                        "SELECT evidence_id FROM check_evidence_operations WHERE task_id=? AND idempotency_key=?",
                        (task_id, f"candidate-check:{check_id}"),
                    ).fetchone()
            except sqlite3.OperationalError:
                receipt = None
            if receipt is not None:
                from ..delegation.check_evidence import get_check_evidence

                evidence = get_check_evidence(self._store, receipt["evidence_id"])
                result = dict(evidence_id=evidence["evidence_id"],
                              observed_result=evidence["observed_result"])
                request = check_requests.complete_check(self._store, check_id, result)
            else:
                request = check_requests.mark_unknown(self._store, check_id)
        elif request["status"] == "queued":
            request = check_requests.claim_check(self._store, check_id)
            try:
                reusable = check_requests.find_reusable_passed_check(
                    self._store, task_id=task_id,
                    candidate_id=request["candidate_id"],
                    candidate_digest=request["candidate_digest"],
                    argv=request["argv"], checked_files=request["checked_files"],
                )
                if reusable is not None:
                    # Reuse is still bound to the exact current candidate. Read
                    # the staged manifest on both sides of the durable clone so
                    # a concurrent mutation cannot silently turn old evidence
                    # into evidence for a different candidate.
                    from ..delegation.candidates import _digest_manifest, _manifest, get_candidate
                    from ..delegation.check_evidence import record_check_evidence

                    candidate = get_candidate(self._store, request["candidate_id"])
                    if candidate is None or candidate.get("status") != "ready":
                        raise candidate_checks.CandidateCheckError("candidate_not_ready")
                    before_digest = _digest_manifest(_manifest(candidate["stage_path"]))
                    if before_digest != request["candidate_digest"]:
                        raise candidate_checks.CandidateCheckError("candidate_digest_mismatch")
                    prior_evidence = reusable["evidence"]
                    evidence = record_check_evidence(
                        self._store, task_id, check_id, request["candidate_id"],
                        request["candidate_digest"], request["requirement_bindings"],
                        prior_evidence["environment"], prior_evidence["command"],
                        prior_evidence["observed_result"],
                        {"status": "unassessed", "reason": "reused exact assessed observation"},
                        checked_files=request["checked_files"],
                        idempotency_key=f"candidate-check:{check_id}",
                    )
                    after_digest = _digest_manifest(_manifest(candidate["stage_path"]))
                    if after_digest != before_digest:
                        raise candidate_checks.CandidateCheckError("candidate_changed_during_check")
                    result = dict(
                        evidence_id=evidence["evidence_id"],
                        observed_result=evidence["observed_result"],
                        reused=True,
                        observation_source="exact_prior_assessed_pass",
                        reused_from_check_id=reusable["request"]["check_id"],
                        reused_from_evidence_id=prior_evidence["evidence_id"],
                    )
                else:
                    observed = candidate_checks.run_candidate_check(
                        self._store, task_id, check_id, request["candidate_id"],
                        request["candidate_digest"], request["argv"],
                        assessment={"status": "unassessed", "reason": "runtime observation only"},
                        requirement_bindings=request["requirement_bindings"],
                        checked_files=request["checked_files"],
                        idempotency_key=f"candidate-check:{check_id}",
                    )
                    result = dict(evidence_id=observed["evidence"]["evidence_id"],
                                  observed_result=observed["observed_result"])
                request = check_requests.complete_check(self._store, check_id, result)
            except candidate_checks.CandidateCheckError as error:
                request = check_requests.fail_check(
                    self._store, check_id,
                    f"candidate_check_execution_failed:{error.code}",
                )
            except Exception:
                request = check_requests.fail_check(
                    self._store, check_id, "candidate_check_execution_failed:unexpected_error",
                )
        kind = {"completed": "candidate_check_observed", "failed": "candidate_check_failed",
                "unknown": "candidate_check_unknown"}.get(request["status"])
        if kind is None:
            return
        event_payload = dict(kind=kind, check_ref=check_id, candidate_ref=request["candidate_id"],
                             candidate_digest=request["candidate_digest"],
                             status=request["status"], result=request["result"], error=request["error"])
        delegation_events.append_event(
            self._store, task_id, "progress", f"candidate_check_outcome:{check_id}",
            event_payload, origin="runtime",
        )
        self._notify()

    def _terminate_completed_workers(self, task_id: str) -> None:
        """T017: a worker that reached a terminal state (published a
        result, or failed) has its owned engine process shut down here --
        the *coordinator* side that actually observes the terminal event --
        so capacity is only reused once that shutdown reaches a confirmed
        (or ambiguous) release (see ``delegation.capacity`` /
        ``EngineSessionManagerAdapter.terminate_owned``)."""
        with self._store.connection() as db:
            results = db.execute(
                "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? AND event_type='worker_result'",
                (task_id,),
            ).fetchall()
            failures = db.execute(
                "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
                (task_id,),
            ).fetchall()
        attempt_ids: set[str] = {row["assignment_ref"] for row in results if row["assignment_ref"]}
        for row in failures:
            hash_ref = row["assignment_ref"]
            if not hash_ref:
                continue
            with self._store.connection() as db:
                prov = db.execute(
                    "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (hash_ref,),
                ).fetchone()
            if prov is not None and prov["attempt_id"]:
                attempt_ids.add(prov["attempt_id"])
        with self._lock:
            newly_terminal = [a for a in attempt_ids if a not in self._terminated_attempts]
            self._terminated_attempts.update(newly_terminal)
        for attempt_id in newly_terminal:
            try:
                self._engine.terminate_owned(attempt_id)
            except Exception:
                pass
            self._cleanup_isolated_workspace_if_integrated(task_id, attempt_id)
        if newly_terminal:
            self._notify()

    def _cleanup_isolated_workspace_if_integrated(self, task_id: str, attempt_id: str) -> None:
        """T025 design decision 5: remove only a Cortex-owned isolated
        snapshot workspace, and only once this attempt is both terminal
        (already established by the caller) and its integration outcome is
        durably recorded (``workspace_integrations`` has *some* row for its
        assignment -- applied, conflict, or noop; a cancelled assignment
        with no integration ever attempted is also eligible, since there is
        nothing left to integrate). An attempt whose isolated workspace was
        never even created (serialized fallback, or isolation never used)
        is a silent no-op here. Never touches ``project_root`` or any
        user-owned path -- see ``workspace_isolation.IsolatedSnapshotWorkspaceProvisioner
        .cleanup_binding``."""
        with self._store.connection() as db:
            snapshot = db.execute(
                "SELECT workspace_binding_id, mode FROM workspace_snapshots WHERE task_id=? AND attempt_id=? "
                "AND cleaned_up_at IS NULL ORDER BY created_at DESC LIMIT 1",
                (task_id, attempt_id),
            ).fetchone()
            if snapshot is None or snapshot["mode"] != "isolated_snapshot":
                return
            provisioning = db.execute(
                "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (attempt_id,),
            ).fetchone()
            cancelled = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' "
                "AND assignment_ref=?", (task_id, provisioning["idempotency_key"] if provisioning else None),
            ).fetchone() if provisioning else None
            integrated = db.execute(
                "SELECT 1 FROM workspace_integrations WHERE workspace_binding_id=?",
                (snapshot["workspace_binding_id"],),
            ).fetchone()
        if integrated is None and cancelled is None:
            return
        try:
            delegation_workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(
                self._store, self._task_project_root(task_id),
            ).cleanup_binding(snapshot["workspace_binding_id"], reason="attempt_terminal_integrated")
        except Exception:
            pass

    def _task_project_root(self, task_id: str) -> str:
        with self._store.connection() as db:
            row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        return row["project_root"]

    def _accepted_pipeline_text_for_final_guard(self, task_id: str) -> str | None:
        """Read the current, digest-checked pipeline through the store reader.

        No unverified filesystem read can authorize a digest in the final
        response. If the current edition cannot be read completely, the
        final guard withholds exact digest claims rather than guessing.
        """
        try:
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT id FROM reports WHERE task_id=? AND kind='pipeline' LIMIT 1",
                    (task_id,),
                ).fetchone()
                if row is None:
                    return None
                args = dict(task_id=task_id, report_id=row["id"], limit=4000)
                chunks = []
                total = 0
                while True:
                    page = self._store._read(db, args)
                    chunks.append(page["markdown"])
                    total += len(page["markdown"])
                    if total > 100_000:
                        return None
                    if page["next_cursor"] is None:
                        return "".join(chunks)
                    args["cursor"] = page["next_cursor"]
        except (StoreError, OSError, ValueError, sqlite3.Error):
            return None

    def _direct_worker_publication_matches(self, task_id: str, after_event_id: int, revision: dict) -> bool:
        """Recognize a same-turn direct worker result at its publication boundary.

        This is an observed tree match, not OS-level attribution: another
        process with the same user privileges can still write between the
        worker's last edit and publication. A missing/partial observation,
        cancelled result, or subsequent project change never qualifies.
        """
        if revision.get("kind") != "observed":
            return False
        with self._store.connection() as db:
            rows = db.execute(
                "SELECT e.assignment_ref, e.payload_json, p.workspace_binding_id FROM task_events e "
                "JOIN delegation_provisioning p ON p.task_id=e.task_id AND p.attempt_id=e.assignment_ref "
                "WHERE e.task_id=? AND e.id>? AND e.event_type='worker_result' AND e.superseded=0 "
                "ORDER BY e.id DESC",
                (task_id, after_event_id),
            ).fetchall()
            for row in rows:
                if row["workspace_binding_id"] not in {"shared_project_root", "serialized_shared_root"}:
                    continue
                report_ref = json.loads(row["payload_json"]).get("report_ref")
                if not isinstance(report_ref, str):
                    continue
                observations = db.execute(
                    "SELECT kind, dirty_digest, manifest_json FROM attempt_revisions WHERE attempt_id=? AND phase=? "
                    "ORDER BY rowid ASC",
                    (row["assignment_ref"], f"worker_publish:{report_ref}"),
                ).fetchall()
                # Concurrent duplicate publications with separate captures
                # have ambiguous provenance. A normal idempotent retry has
                # exactly one retained observation.
                if len(observations) != 1:
                    continue
                observed = observations[0]
                try:
                    manifest = json.loads(observed["manifest_json"]) if observed["manifest_json"] else None
                except (TypeError, ValueError):
                    manifest = None
                publication = dict(kind=observed["kind"], dirty_digest=observed["dirty_digest"], manifest=manifest)
                attributed, reason = delegation_revision.shared_publication_attribution(
                    self._store, task_id, row["assignment_ref"], publication, revision,
                )
                if attributed:
                    return True
                # Keep the refusal observable for the coordinator and for
                # postmortem review.  A matching shared-root tree without a
                # unique durable owner is intentionally not authorization.
                if reason:
                    delegation_events.append_event(
                        self._store, task_id, "progress",
                        f"direct_worker_attribution_refused:{row['assignment_ref']}:{reason}",
                        dict(kind="direct_worker_attribution_refused",
                             assignment_ref=row["assignment_ref"], reason=reason),
                        origin="runtime", assignment_ref=row["assignment_ref"],
                    )
        return False

    def _pending_direct_worker_change(self, task_id: str) -> tuple[int, int] | None:
        """Restore an unresolved project-change boundary after pause/restart.

        The progress event records only the event cursor, not an assertion
        that the worker caused the change.  An exact subsequent publication
        resolves it; a failed or lost worker leaves it blocking.
        """
        with self._store.connection() as db:
            row = db.execute(
                "SELECT id, payload_json FROM task_events WHERE task_id=? "
                "AND event_type='progress' AND dedup_key LIKE 'direct_worker_change_pending:%' "
                "ORDER BY id DESC LIMIT 1", (task_id,),
            ).fetchone()
            if row is None:
                return None
            resolved = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key=? LIMIT 1",
                (task_id, f"direct_worker_change_resolved:{row['id']}"),
            ).fetchone()
        if resolved is not None:
            return None
        try:
            cursor = json.loads(row["payload_json"])["after_event_id"]
        except (KeyError, TypeError, ValueError):
            return (row["id"], -1)  # malformed evidence must never authorize a result
        return (row["id"], cursor if isinstance(cursor, int) and cursor >= 0 else -1)

    def _direct_worker_still_open(self, task_id: str) -> bool:
        """Whether a bound direct-project worker can still publish a result.

        Consult both the live assignment owner and durable terminal events.
        ``_open_assignments`` may remain populated after ``worker_result``
        when the host did not wire an ACP tool-call sink, while a provisioning
        row normally remains ``ready`` for audit. Neither stale marker means
        that attempt can publish another result or excuse later project drift.
        """
        with self._lock:
            open_refs = tuple(self._open_assignments.get(task_id) or ())
        if not open_refs:
            return False
        with self._store.connection() as db:
            return any(
                db.execute(
                    "SELECT 1 FROM delegation_provisioning p WHERE p.task_id=? "
                    "AND p.idempotency_key=? AND p.attempt_id IS NOT NULL "
                    "AND p.workspace_binding_id IN "
                    "('shared_project_root','serialized_shared_root') "
                    "AND NOT EXISTS (SELECT 1 FROM task_events e WHERE e.task_id=p.task_id "
                    "AND ((e.event_type='worker_result' AND e.assignment_ref=p.attempt_id) "
                    "OR (e.event_type IN ('assignment_failed','assignment_cancelled') "
                    "AND e.assignment_ref=p.idempotency_key))) LIMIT 1",
                    (task_id, ref),
                ).fetchone() is not None
                for ref in open_refs
            )

    def _mark_work_attempt_failed(self, task_id: str, assignment_ref: str,
                                  reason: str, idempotency_key: str) -> None:
        """Mirror a legacy assignment failure into the durable work graph.

        Older assignments have no work-graph reference, and terminal attempts
        can race with a late result, so both cases are intentionally ignored.
        The legacy event remains the authoritative compatibility record.
        """
        def _record_sync_error(error) -> None:
            # The compatibility assignment_failed event is still emitted by
            # the caller, but a graph mismatch must remain visible instead of
            # leaving a planned attempt silently orphaned.
            try:
                delegation_events.append_event(
                    self._store, task_id, "progress",
                    f"work_attempt_sync_failed:{idempotency_key}",
                    dict(kind="work_attempt_sync_failed", assignment_ref=assignment_ref,
                         reason=_bounded_detail(str(error))),
                    origin="runtime", assignment_ref=assignment_ref,
                )
            except Exception:
                # Preserve the original failure path if the store itself is
                # unavailable; the caller's assignment_failed event remains
                # the last durable signal in that case.
                pass

        try:
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT p.attempt_id, e.payload_json FROM delegation_provisioning p "
                    "LEFT JOIN task_events e ON e.task_id=p.task_id AND e.assignment_ref=p.idempotency_key "
                    "AND e.event_type='delegation_requested' WHERE p.task_id=? "
                    "AND (p.idempotency_key=? OR p.attempt_id=?) ORDER BY e.id DESC LIMIT 1",
                    (task_id, assignment_ref, assignment_ref),
                ).fetchone()
            if row is None or not row[1]:
                return
            work_attempt_ref = json.loads(row[1]).get("work_attempt_ref")
            if not work_attempt_ref:
                return
            from ..delegation.work_graph import WorkGraph, WorkGraphError
            # A worker publishes through its own MCP channel.  That channel
            # advances the graph attempt to ``reported`` before the engine
            # session necessarily closes.  A late EOF/cleanup callback may
            # therefore arrive after publication and attempt to mirror the
            # same assignment as ``failed``.  ``reported`` is terminal for
            # this compatibility mirror: do not turn a successful result
            # into a synthetic sync error (or a second failure event).
            with self._store.connection() as db:
                attempt_row = db.execute(
                    "SELECT status FROM work_assignment_attempts WHERE attempt_id=?",
                    (work_attempt_ref,),
                ).fetchone()
            if attempt_row is not None and attempt_row["status"] == "reported":
                return
            try:
                WorkGraph(self._store).transition_attempt(
                    work_attempt_ref, "failed", reason=reason, idempotency_key=idempotency_key,
                )
            except WorkGraphError as error:
                _record_sync_error(error)
                return
        except (KeyError, TypeError, ValueError, sqlite3.Error) as error:
            _record_sync_error(error)
            return

    def _deliver_clarification(self, task_id, key, session_ref, message, cancel_event, event_row_id) -> None:
        generation_delivery_id = None
        try:
            pending_delivery = getattr(self._engine, "pending_delivery", None)
            role_id = None
            if pending_delivery is not None:
                pending = pending_delivery(session_ref)
                if pending:
                    role_id = self._role_id_for_pending(task_id, pending)
            if role_id is not None:
                message, generation_delivery_id, delivery_blocked = self._begin_generation_delivery(
                    task_id, session_ref, role_id, message,
                )
                if delivery_blocked:
                    # T024: permanently blocked (budget overflow / compaction
                    # loop) -- never prompt this worker with it; the
                    # explicit blocked task state was already recorded.
                    self._mark_work_attempt_failed(task_id, key[1], "context_budget_blocked", f"work-failed:{event_row_id}:budget")
                    delegation_events.append_event(
                        self._store, task_id, "assignment_failed", f"assignment_failed:{key[1]}",
                        dict(error="context_budget_blocked"), origin="runtime", assignment_ref=key[1],
                    )
                    return
            self._engine.prompt(session_ref, message, cancel_event)
        except EngineTurnFailed as failure:
            self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=failure.kind, stop_reason=None)
            self._mark_work_attempt_failed(task_id, key[1], f"engine_failure:{failure.kind}", f"work-failed:{event_row_id}:engine")
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{key[1]}",
                dict(error=f"engine_failure:{failure.kind}", kind=failure.kind),
                origin="runtime", assignment_ref=key[1],
            )
            return
        self._finalize_generation_delivery(session_ref, generation_delivery_id, failure_kind=None, stop_reason=StopReason.END_TURN)
        delegation_events.append_event(
            self._store, task_id, "progress", f"clarification_delivered:{event_row_id}",
            dict(note="clarification delivered"), origin="runtime", assignment_ref=key[1],
        )

    def _on_worker_done(self, task_id: str, future: Future) -> None:
        with self._cond:
            pending = self._running.get(task_id)
            if pending is not None:
                pending.discard(future)
            self._cond.notify_all()

    def _attempt_worker_route_fallback(
        self, task_id: str, assignment_ref: str, assignment_id: str, role_id: str, old_attempt_id: str,
        workspace_binding_id: str, config_revision: str | None, failure_class: str,
        instructions: str,
    ) -> tuple[str | None, bool, str | None]:
        """T027: on a fallback-eligible worker failure, consult the durable
        route profile/ceiling/fallback policy this task's active
        configuration revision actually recorded and, only when a fresh
        :func:`routing.authorization.authorize_fallback` check on the
        engine adapter's own :meth:`SessionRuntime.attempt_fallback`
        authorizes it, open a replacement attempt on the single
        deterministic next allowed target route.

        Never tries a second candidate route on refusal (that would be
        exactly the silent route-shopping this task forbids): returns
        ``(None, explicitly_refused)`` on any refusal, missing policy/
        consent, exhausted budget, or a candidate this configuration
        revision never recorded a resolvable ``(engine, connection_id,
        model)`` for. ``explicitly_refused`` is ``True`` only once a real
        configuration revision/route profile was actually consulted and a
        concrete, coordinator-visible ``assignment_failed:...:fallback``
        reason was already recorded for it (the caller must never write a
        second, generic one in that case); ``False`` means fallback was
        never evaluated at all (no configuration revision resolvable, or
        this engine has no ``attempt_fallback`` -- the caller's own plain
        failure message is the only one written). Reconciles the old
        attempt first (T023's own worker-lost
        primitives) so a confirmed/ambiguous capacity reservation and its
        trusted channel are never left dangling; re-derives canonical
        context via the normal :meth:`SessionRuntime.open` generation-
        delivery path (``cause='session_replacement'``, T021/T022) for the
        new engine.
        """
        def _refuse(reason: str) -> None:
            if self._assignment_already_reported(task_id, assignment_ref):
                # The worker reported and its session then closed; that
                # close is not a failure of the assignment.
                return
            self._mark_work_attempt_failed(task_id, assignment_ref, f"fallback_refused:{reason}", f"work-failed:{assignment_ref}:fallback")
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}:fallback",
                dict(error=f"fallback_refused:{reason}", failure_class=failure_class), origin="runtime",
                assignment_ref=assignment_ref,
            )

        attempt_fallback = getattr(self._engine, "attempt_fallback", None)
        if attempt_fallback is None or not config_revision:
            # Fallback was never even evaluated (no real configuration
            # revision to consult at all -- e.g. a test double or a task
            # whose engine adapter never implements attempt_fallback): the
            # caller's own plain failure message is the only one written.
            return None, False, None
        from ..configuration import store as configuration_store
        from ..routing import authorization as routing_authorization
        from ..routing import store as routing_store

        with self._store.connection() as db:
            row = db.execute("SELECT * FROM configuration_revisions WHERE id=?", (config_revision,)).fetchone()
        if row is None:
            return None, False, None
        revision = configuration_store._row_dict(row)
        extras = routing_store.get_route_extras(self._store, config_revision)
        if extras is None:
            return None, False, None
        ceiling = revision["ceiling_snapshot"]
        allowed = routing_authorization.allowed_routes_for_role(
            extras["route_profile"], ceiling, role_id, revision["route_id"],
        )
        fallback_targets = ceiling.fallback_policy.allowed_target_routes
        candidates = allowed if fallback_targets is None else (allowed & fallback_targets)
        # T027 review fix (MEDIUM): never re-select a connection already
        # tried for THIS assignment -- walk the predecessor chain from
        # ``old_attempt_id`` (every prior fallback hop, not just the
        # original coordinator route) collecting each hop's own audited
        # connection id, bounded so a corrupt/cyclic chain can never loop
        # forever.
        tried_connection_ids = {revision["route_connection_id"]}
        current_attempt_id, seen = old_attempt_id, set()
        while current_attempt_id and current_attempt_id not in seen and len(seen) < 50:
            seen.add(current_attempt_id)
            hops = routing_store.list_audit_for_attempt(self._store, current_attempt_id)
            if not hops:
                break
            last = hops[-1]
            tried_connection_ids.add(last["connection_id"])
            current_attempt_id = last["predecessor_attempt_id"]
        candidates = {
            route_id for route_id in candidates
            if route_id in extras["route_details"]
            and extras["route_details"][route_id]["connection_id"] not in tried_connection_ids
        }
        if not candidates:
            _refuse("no_untried_authorized_route")
            return None, True, None
        target_route_id = sorted(candidates)[0]
        detail = extras["route_details"][target_route_id]

        from ..recovery import worker_reconciliation

        worker_reconciliation._best_effort_terminate(self._store, self._engine, old_attempt_id)
        worker_reconciliation._revoke_worker_channel(self._store, task_id, old_attempt_id)

        # T027 (scout finding): a fallback attempt needs its OWN worker
        # channel -- the old attempt's channel was just revoked above, and
        # a channel is never valid for a different attempt id -- so its own
        # Cortex MCP subprocess can actually authenticate against the new
        # session, exactly like a first-time delegation's own
        # delegation.provisioning._step_channel.
        from .. import ownership
        from ..trusted_channel import issue_channel_tx

        new_attempt_id = new_id("xa_")
        coordinator_channel = self._coordinator_channels.get(task_id)
        try:
            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                try:
                    owner_epoch = ownership.current_owner_epoch(db, task_id)
                    channel_id, channel_secret = issue_channel_tx(
                        db, task_id, "worker", owner_epoch,
                        parent_channel_id=coordinator_channel.channel_id if coordinator_channel is not None else None,
                        assignment_id=assignment_id, attempt_id=new_attempt_id, workspace_binding_id=workspace_binding_id,
                    )
                    db.commit()
                except BaseException:
                    db.rollback()
                    raise
            launch = dict(
                task_id=task_id, assignment_id=assignment_id, attempt_id=new_attempt_id, config_revision=config_revision,
                channel_id=channel_id, channel_secret=channel_secret, role_id=role_id,
            )
            new_session_ref = attempt_fallback(
                old_attempt_id=old_attempt_id, new_attempt_id=new_attempt_id, task_id=task_id, role=role_id,
                failure_class=failure_class, target_connection_id=detail["connection_id"],
                target_route_id=target_route_id, workspace_binding_id=workspace_binding_id, launch=launch,
                instructions=instructions, target_model=detail.get("model"),
            )
        except PortableError as error:
            # Never silently retried on a third route: one deterministic
            # candidate, one authorization decision, surfaced verbatim.
            _refuse(error.code)
            return None, True, None
        except BaseException as error:
            # An unexpected (non-PortableError) failure while attempting
            # fallback must still produce a durable, coordinator-visible
            # event -- never a silently lost assignment (same precedent as
            # this method's own first-prompt BaseException handler).
            _refuse(f"unexpected_error:{type(error).__name__}")
            return None, True, None
        self._worker_sessions[(task_id, assignment_ref)] = new_session_ref
        # delegation.completion.gate's own worker-event bridge resolves a
        # coordinator-facing assignment_ref (this delegation's idempotency
        # key) to the *provisioned* attempt_id before ever looking up a
        # worker_result/worker_question event (those are always keyed by
        # attempt_id, never the hash) -- update it here so the fallback
        # attempt's own worker-facing events resolve, exactly as if this
        # had been the delegation's attempt_id from the start.
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                db.execute(
                    "UPDATE delegation_provisioning SET attempt_id=?, engine_session_ref=?, updated_at=? WHERE idempotency_key=?",
                    (new_attempt_id, new_session_ref, _now(), assignment_ref),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
        delegation_events.append_event(
            self._store, task_id, "progress", f"fallback_dispatched:{assignment_ref}:{new_attempt_id}",
            dict(
                note=f"Assignment {assignment_ref} fell over to an authorized alternate route after {failure_class}.",
                predecessor_attempt_id=old_attempt_id, new_attempt_id=new_attempt_id, target_route_id=target_route_id,
            ),
            origin="runtime", assignment_ref=assignment_ref,
        )
        return new_session_ref, True, new_attempt_id

    def _provision_and_prompt(self, task_id: str, coordinator_channel: ChannelContext, payload: Mapping[str, Any], cancel_event: threading.Event) -> None:
        # T017: capacity queuing happens *inside* SessionRuntime.open() (see
        # delegation.live_adapters.EngineSessionManagerAdapter.open) -- never
        # by retrying this whole delegate() call. delegate() uses an
        # idempotency-key claim/steal protocol per provisioning step
        # (delegation.provisioning._acquire_step); retrying it wholesale
        # after a mid-step exception would fight that protocol's own
        # contention recovery instead of just taking longer, exactly like a
        # slow real engine launch it is already built to tolerate.
        assignment_ref = payload["assignment_ref"]
        # A worker future may have been submitted immediately before
        # /cortex-stop.  The continuation cancel event is intentionally not
        # a worker kill switch for ordinary session/cancel, so the durable
        # stop marker is the fence checked before any provisioning side
        # effect.
        if acp_steering.is_stopped(self._store, task_id):
            return
        try:
            result = self._provisioning.delegate(
                coordinator_channel, payload["title"], payload["brief"], payload["role"],
                payload["acceptance"], payload["request_key"],
                # D-T035-04: the coordinator's own optional route choice,
                # carried verbatim from the ``delegation_requested`` event
                # payload (``delegation.mcp_tools._coordinator_delegate``);
                # every event predating this field is a plain dict without
                # these keys, so ``.get`` keeps them replaying as omitted.
                engine=payload.get("engine"), model=payload.get("model"), effort=payload.get("effort"),
            )
        except Exception as error:
            # Duck-typed, and deliberately so. The same refusal reaches here
            # as a PortableError from one caller and wrapped in this layer's
            # own RouteUnresolved from another -- and catching only the
            # former meant an engine whose login had expired produced no
            # assignment_failed at all: the exception went on to the thread
            # that swallows anything so a late worker start never breaks the
            # turn, the provisioning row stayed claimed, and the coordinator
            # kept answering that the worker was still working (live,
            # 2026-09-19). Anything with no code of its own still
            # propagates untouched.
            failure_code = getattr(error, "code", None)
            if failure_code is None and isinstance(error, StoreError):
                failure_code = str(error)
            if failure_code is None:
                raise
            result = None
            if failure_code in ("capacity_unsatisfiable", "capacity_timeout"):
                # T027: this delegation's own attempt was allocated (and its
                # capacity reservation/channel opened) before open() itself
                # refused it -- resolvable via the identical idempotency key
                # delegate() itself derives, never a guess.
                from ..delegation.provisioning import _idempotency_key

                idempotency_key = _idempotency_key(task_id, coordinator_channel.channel_id, payload["request_key"])
                with self._store.connection() as db:
                    provisioning_row = db.execute(
                        "SELECT * FROM delegation_provisioning WHERE idempotency_key=?", (idempotency_key,),
                    ).fetchone()
                if provisioning_row is not None and provisioning_row["attempt_id"]:
                    from ..portable.roles import load_role

                    role_set = load_role(provisioning_row["role_id"])
                    new_session_ref, fallback_refused, new_attempt_id = self._attempt_worker_route_fallback(
                        task_id, assignment_ref, provisioning_row["assignment_id"], provisioning_row["role_id"],
                        provisioning_row["attempt_id"], provisioning_row["workspace_binding_id"],
                        provisioning_row["config_revision"], "capacity_unavailable", composed_instructions(role_set),
                    )
                    if new_session_ref is not None:
                        from types import SimpleNamespace

                        result = SimpleNamespace(
                            engine_session_ref=new_session_ref, attempt_id=new_attempt_id,
                            assignment_id=provisioning_row["assignment_id"], role_id=provisioning_row["role_id"],
                            workspace_binding_id=provisioning_row["workspace_binding_id"],
                            config_revision=provisioning_row["config_revision"],
                        )
                    elif fallback_refused:
                        # _attempt_worker_route_fallback already recorded
                        # the concrete refusal reason itself.
                        return
            if result is None:
                # An engine that answers with its own JSON-RPC error gives a
                # number, not a name: live this was recorded as
                # ``error: -32000`` with an empty detail, which tells nobody
                # anything. Name the class of failure ourselves and carry
                # the engine's own words, which is the part a person can
                # actually act on ("Authentication required: run /login").
                code = failure_code if isinstance(failure_code, str) else "engine_refused_session"
                detail = (getattr(error, "remediation", None)
                          or getattr(error, "correction", None)
                          or getattr(error, "message", None)
                          or "")
                delegation_events.append_event(
                    self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    dict(error=code, role=payload["role"], detail=_bounded_detail(detail)),
                    origin="runtime", assignment_ref=assignment_ref,
                )
                return
        # Stop may have won while the worker session was being opened.  The
        # A concurrent stop can scan provisioning before this open finishes.
        # Close the exact attempt here too, so a stale future cannot leave a
        # newly opened process behind or send a first model turn.
        if acp_steering.is_stopped(self._store, task_id):
            try:
                self._engine.terminate_owned(result.attempt_id)
            except Exception:
                pass
            return
        if payload.get("work_attempt_ref"):
            from ..delegation.work_graph import WorkGraph

            WorkGraph(self._store).transition_attempt(
                payload["work_attempt_ref"], "running",
                idempotency_key=f"work-running:{assignment_ref}",
            )
        delegation_events.append_event(
            self._store, task_id, "assignment_ready", f"assignment_ready:{assignment_ref}",
            dict(engine_session_ref=result.engine_session_ref), origin="runtime", assignment_ref=assignment_ref,
        )
        # The worker's own trusted channel is resolved inside its Cortex MCP
        # subprocess from its launch env (see delegation.provisioning /
        # engine session launch); this main process never needs to hold the
        # worker's ChannelContext, only its engine session ref.
        self._worker_sessions[(task_id, assignment_ref)] = result.engine_session_ref
        brief = f"{payload['title']}\n\n{payload['brief']}\n\nAcceptance: {payload['acceptance']}"
        if payload.get("handoff_mode") == "report_review":
            # This is structured task context, not a reconstructed title or
            # summary.  The worker's role protocol performs the bounded
            # read_report call; the runtime supplies the exact durable ref.
            brief += (
                "\n\nPredecessor report handoff:\n"
                + json.dumps({
                    "handoff_mode": "report_review",
                    "predecessor_report_ref": payload["predecessor_report_ref"],
                }, ensure_ascii=False, sort_keys=True)
            )
        if payload.get("plan_assignment_ref"):
            from ..delegation.context_packet import build_context_packet
            from ..delegation.work_graph import WorkGraph

            planned = WorkGraph(self._store).get_assignment(payload["plan_assignment_ref"])
            if planned is None or planned["task_id"] != task_id:
                delegation_events.append_event(
                    self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    dict(error="planned_assignment_unavailable"), origin="runtime",
                    assignment_ref=assignment_ref,
                )
                return
            predecessor = planned["predecessors"]
            input_candidate = predecessor[0]["candidate_ref"] if predecessor else None
            try:
                packet = build_context_packet(
                    self._store, task_id, assignment_id=payload["plan_assignment_ref"],
                    input_candidate_ref=input_candidate,
                )
            except StoreError as error:
                delegation_events.append_event(
                    self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    dict(error=f"context_packet_unavailable:{error}"), origin="runtime",
                    assignment_ref=assignment_ref,
                )
                return
            brief += "\n\nCurrent structured task context (data has explicit trust labels):\n" + packet.to_prompt()
        generation_delivery_id = None
        # A clean worker END_TURN with no durable Cortex outcome gets one
        # procedural chance to finish the existing report/publication
        # protocol in this same engine session.  The marker is durable so a
        # duplicate runtime cannot execute that continuation twice.
        worker_recovery_used = False
        recovery_prompt = None
        # T027 review fix (MEDIUM): a loop, not a single hardcoded retry --
        # each EngineTurnFailed may fall over to a *different* untried
        # route (``_attempt_worker_route_fallback`` itself excludes every
        # already-tried connection via the predecessor chain, so this
        # always terminates: the number of authorized routes is finite).
        # ``_MAX_FALLBACK_HOPS`` is only a defensive hard ceiling, never
        # expected to bind in practice.
        # One extra iteration is reserved for the bounded same-session
        # completion follow-up.  It does not add a fallback hop: the
        # engine-failure branch still gates replacements on ``_hop`` being
        # below ``_MAX_FALLBACK_HOPS``.
        for _hop in range(self._MAX_FALLBACK_HOPS + 2):
            try:
                # A fallback opens a new attempt. Rebuild its bounded catalog
                # against current project docs while keeping the assignment
                # brief fixed; neither an old catalog nor an earlier capsule
                # is silently carried into the replacement session.
                worker_prompt = recovery_prompt or (
                    f"{render_worker_catalog(self._task_project_root(task_id))}\n\n---\n\n{brief}"
                )
                recovery_prompt = None
                delivered_prompt, generation_delivery_id, delivery_blocked = self._begin_generation_delivery(
                    task_id, result.engine_session_ref, payload["role"], worker_prompt,
                )
                if delivery_blocked:
                    with self._lock:
                        if worker_recovery_used:
                            self._worker_recoveries_in_flight.discard(assignment_ref)
                    delegation_events.append_event(
                        self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                        dict(error="context_budget_blocked"), origin="runtime", assignment_ref=assignment_ref,
                    )
                    return
                if acp_steering.is_stopped(self._store, task_id):
                    with self._lock:
                        if worker_recovery_used:
                            self._worker_recoveries_in_flight.discard(assignment_ref)
                    return
                with self._lock:
                    self._worker_turns_in_flight.add(assignment_ref)

                # Worker output stays in its one assignment card, never as
                # sibling cards in the coordinator's timeline. Card content
                # only -- never the client's message stream
                # (PromptOutcome owns what a worker's text means to the
                # coordinator; see SessionRuntime.prompt).
                worker_delta_seen = False
                worker_terminal_text = False
                # ``TURN_DELTA`` callbacks carry text only in the portable
                # runtime contract.  Number each observed chunk within this
                # worker turn so equal adjacent chunks (for example three
                # spaces or repeated words) remain separate durable events.
                # The counter is scoped to this attempt/turn, never shared by
                # concurrent workers or fallback attempts.
                worker_delta_sequence = 0
                worker_stream_redactor = StreamingRedactor(max_chars=4000)

                def _on_worker_delta(chunk_text: str, _ref: str = assignment_ref) -> bool:
                    # Buffer contiguous chunks so a credential split across
                    # ACP updates can be redacted before the worker card is
                    # updated. The coordinator's text channel stays clear.
                    nonlocal worker_delta_seen, worker_delta_sequence
                    if chunk_text:
                        worker_delta_sequence += 1
                        sequence = worker_delta_sequence
                        worker_delta_seen = True
                        safe_chunk = worker_stream_redactor.push(str(chunk_text))
                        if not safe_chunk:
                            return True
                        with self._lock:
                            feed = self._worker_card_emitter(task_id, _ref)
                            self._worker_activity[_ref] = "[worker message] " + safe_chunk
                        attempt_id = str(getattr(result, "attempt_id", None) or result.engine_session_ref)
                        self._record_worker_activity(
                            task_id, assignment_ref, attempt_id,
                            "message",
                            "[worker message] " + safe_chunk, status="in_progress",
                            stream_sequence=sequence,
                        )
                        worker_message_buffer.append(safe_chunk)
                        if len("".join(worker_message_buffer)) >= 3500:
                            _flush_worker_message(feed=feed, final=False)
                        self._emit_worker_activity_tool_calls(feed)
                    return True

                def _on_worker_display_update(update: dict, _ref: str = assignment_ref) -> None:
                    """Expose observable worker plans/thought chunks safely.

                    ACP does not expose hidden chain-of-thought; it may expose
                    explicit ``agent_thought_chunk`` and plan updates.  Those
                    are data from the worker session, never coordinator
                    instructions, and are labelled as such in the worker
                    card.  Unknown display updates are retained as bounded
                    structured data instead of being dropped.
                    """
                    if not isinstance(update, dict):
                        return
                    with self._lock:
                        feed = self._worker_card_emitter(task_id, _ref)
                    kind = str(update.get("sessionUpdate") or update.get("type") or "display")
                    # The adapter can mirror a complete tool-call envelope
                    # here.  Tool calls are rendered by _on_worker_tool_call;
                    # copying the envelope would show raw JSON and duplicate
                    # the action in the worker card.
                    text_value = _worker_display_text(update)
                    if text_value is None:
                        return
                    # Keep the worker card focused on project/MCP actions and
                    # its final message. Provider thought/plan streams are
                    # internal narration, not work a reader can act on.
                    if "thought" in kind.lower() or "plan" in kind.lower():
                        return
                    # Provider display prose is not a terminal worker
                    # response. Keep it out of the card entirely.
                    return

                def _on_worker_tool_call(ref, kind, title, status, *rest, _ref=assignment_ref) -> None:
                    # Keyword-only ``_ref``: this callback is invoked
                    # positionally by the adapter, which grew ``locations``
                    # and ``content`` arguments -- as a positional default
                    # ``_ref`` silently took ``locations`` instead, the call
                    # then raised TypeError on the sixth argument, and the
                    # adapter's own best-effort ``except`` swallowed it. The
                    # visible result was a worker card that never showed
                    # anything the worker did, which is the one thing it is
                    # for. ``*rest`` accepts whatever the adapter carries
                    # without this needing to know about it.
                    # Any tool boundary ends the preceding prose segment,
                    # including hidden Cortex bookkeeping. Only deltas after
                    # the last tool can become the terminal final message.
                    worker_message_buffer.clear()
                    if isinstance(title, str) and _NATIVE_SUBAGENT.search(title):
                        # The adapter still cancels provider-owned children,
                        # but their bookkeeping never enters the worker card.
                        cancel_event.set()
                    locations = rest[0] if rest else None
                    content = rest[1] if len(rest) > 1 else None
                    raw_input = rest[2] if len(rest) > 2 else None
                    raw_output = rest[3] if len(rest) > 3 else None
                    if ref and raw_input is not None:
                        worker_tool_inputs[ref] = raw_input
                    elif ref:
                        raw_input = worker_tool_inputs.get(ref)
                    if ref and locations is not None:
                        worker_tool_locations[ref] = locations
                    elif ref:
                        locations = worker_tool_locations.get(ref)
                    if _is_engine_internal_tool(title, kind, locations, raw_input, content) or ref in worker_wait_refs:
                        worker_wait_refs.add(ref)
                        worker_pending_titleless.pop(ref, None)
                        return
                    supplied_title = isinstance(title, str) and bool(title.strip())
                    title_value = title
                    if ref and supplied_title and ref in worker_pending_titleless:
                        pending = worker_pending_titleless.pop(ref)
                        if _is_wait_tool(title):
                            worker_wait_refs.add(ref)
                            return
                        _on_worker_tool_call(
                            ref, pending[0], title, pending[1], pending[2], pending[3], pending[4], pending[5],
                        )
                    elif ref and not supplied_title and _acp_tool_call_status(status) in {"pending", "in_progress"}:
                        worker_pending_titleless[ref] = (kind, status, locations, content, raw_input, raw_output)
                        return
                    with self._lock:
                        _tool_feed = self._worker_card_emitter(task_id, assignment_ref)
                    _flush_worker_message(feed=_tool_feed, final=True)
                    if supplied_title and ref and isinstance(title_value, str) and title_value.strip():
                        worker_tool_titles[ref] = _compact_text(title_value.strip(), _TOOL_CALL_TITLE_MAX_CHARS)
                    if not title:
                        title = worker_tool_titles.get(ref) or "unnamed observable tool call"
                    if _NATIVE_SUBAGENT.search(title):
                        # Provider-owned child work is engine machinery, not
                        # an action on the project. Keep it out of the card.
                        self._note_native_subagent(task_id, title)
                        return
                    safe_content = _client_safe_content(content)
                    action = _worker_tool_details(
                        str(title), status, locations, safe_content,
                        _client_safe_raw(raw_input) if raw_input is not None else None,
                        None, self._task_project_root(task_id),
                    )
                    with self._lock:
                        self._worker_activity[_ref] = action
                        feed = self._worker_card_emitter(task_id, _ref)
                    if feed is not None:
                        visible_title = worker_tool_titles.get(ref)
                        if supplied_title and isinstance(title_value, str) and title_value.strip():
                            visible_title = scrub_display_text(
                                title_value.strip(), max_chars=_TOOL_CALL_TITLE_MAX_CHARS,
                            )
                            if ref:
                                worker_tool_titles[ref] = visible_title
                        visible_title = visible_title or "Worker observable tool call"
                        self._emit_worker_card(
                            feed, _ref, status="in_progress",
                            _project_root=self._task_project_root(task_id),
                        )
                        _emit_worker_tool_update(
                            feed=feed, ref=ref, kind=kind, title=title,
                            status=status, locations=locations, content=content,
                            raw_input=raw_input, raw_output=raw_output,
                        )
                    # Straight away, not on the next wait tick: a worker
                    # that starts and finishes between two ticks would
                    # otherwise never show what it did at all, and a
                    # command that only appears a second after it runs is
                    # not "what this worker is doing now". The card's own
                    # send-dedup keeps repeats off the wire.
                    self._emit_worker_activity_tool_calls(feed)

                # The card this worker's commands will be shown on has to
                # be named before the first of them arrives. The event
                # path names it too, at assignment_ready, but a worker can
                # run a command before that event is processed -- and an
                # unnamed card cannot carry anything.
                with self._lock:
                    if assignment_ref not in self._worker_card_title:
                        route = self._delegation_route_label(task_id, assignment_ref)
                        who = _display_role(result.role_id) + (f" · {route}" if route else "")
                        self._worker_card_title[assignment_ref] = (
                            f"Worker: {who} — {payload.get('title', '')}"
                        )
                # Generic ACP has no portable nested-subagent primitive.
                # Keep every worker event within its one assignment card.
                worker_tool_titles: dict[str, str] = {}
                worker_tool_inputs: dict[str, Any] = {}
                worker_tool_locations: dict[str, Any] = {}
                worker_tool_cards: set[str] = set()
                worker_wait_refs: set[str] = set()
                worker_pending_titleless: dict[str, tuple[Any, ...]] = {}
                worker_message_buffer: list[str] = []

                def _emit_worker_tool_update(*, feed, ref, kind, title, status,
                                              locations, content, raw_input,
                                              raw_output=None) -> None:
                    """Append one worker project/MCP call to its lifecycle card."""
                    if feed is None or not ref:
                        return
                    tool_ref = str(ref)
                    first = tool_ref not in worker_tool_cards
                    if first:
                        worker_tool_cards.add(tool_ref)
                    visible_title = _human_tool_title(
                        str(title or worker_tool_titles.get(ref) or "Worker tool call")
                    )
                    safe_input = _client_safe_raw(raw_input) if raw_input is not None else None
                    line = _worker_tool_details(
                        visible_title, status, locations, _client_safe_content(content),
                        safe_input, _client_safe_raw(raw_output) if raw_output is not None else None,
                        self._task_project_root(task_id),
                    )
                    attempt_id = str(getattr(result, "attempt_id", None) or result.engine_session_ref)
                    if not self._record_worker_activity(
                        task_id, assignment_ref, attempt_id, f"tool:{tool_ref}",
                        f"Tool: {line}", status=status,
                        input_value=_client_safe_raw(raw_input) if raw_input is not None else None,
                        output_value=_client_safe_raw(raw_output) if raw_output is not None else None,
                        error_value=_client_safe_raw(raw_output) if _acp_tool_call_status(status) == "failed" and raw_output is not None else None,
                        content_value=content,
                        kind=kind,
                        title=title,
                        locations_value=locations,
                    ):
                        return
                    if _acp_tool_call_status(status) == "failed" and raw_output is not None:
                        if not self._record_worker_activity(
                            task_id, assignment_ref, attempt_id, f"tool-error:{tool_ref}",
                            f"Error: {line}", status="failed",
                            error_value=_client_safe_raw(raw_output),
                            content_value=content,
                            kind=kind,
                            title=title,
                            locations_value=locations,
                        ):
                            return
                    if first:
                        self._emit_worker_card(
                            feed, assignment_ref, status="in_progress",
                            entry=f"Tool: {line}", entry_ref=f"worker-tool:{tool_ref}",
                        )
                    elif _acp_tool_call_status(status) == "failed":
                        self._emit_worker_card(
                            feed, assignment_ref, status="in_progress",
                            entry=f"Error: {line}", entry_ref=f"worker-tool-error:{tool_ref}",
                        )

                def _flush_worker_message(*, feed, final: bool = True,
                                          final_response: bool = False) -> None:
                    """Render only the worker's final streamed message.

                    Interim worker prose is progress chatter in the host
                    timeline. Drop it at each observable boundary; the final
                    flush retains the last model-authored message only.
                    """
                    if feed is None or not worker_message_buffer:
                        return
                    if not final_response:
                        worker_message_buffer.clear()
                        return
                    pending_text = "".join(worker_message_buffer)
                    text_value = pending_text
                    worker_message_buffer.clear()
                    label = "final response"
                    if final_response and text_value.strip():
                        self._worker_final_response_seen.add(assignment_ref)
                    attempt_id = str(getattr(result, "attempt_id", None) or result.engine_session_ref)
                    if text_value.strip():
                        if not self._record_worker_activity(
                            task_id, assignment_ref, attempt_id, "final-response",
                            f"[final response] {scrub_text(text_value, max_chars=max(4000, len(text_value) * 2 + 1000))}",
                            status="in_progress",
                        ):
                            return
                    self._emit_worker_card(
                        feed, assignment_ref, status="in_progress",
                        entry=f"[{label}] {scrub_text(text_value, max_chars=max(4000, len(text_value) * 2 + 1000))}",
                    )

                def _emit_worker_event(
                    event_kind: str, text_value: str, *, status: str, feed,
                ) -> None:
                    """Emit one complete worker event without rebuilding history."""
                    if feed is None or not str(text_value).strip():
                        return
                    self._emit_worker_card(
                        feed, assignment_ref, status="failed" if status == "failed" else "in_progress",
                        entry=f"[{event_kind}] {scrub_text(str(text_value), max_chars=4000)}",
                    )
                # Internal Cortex calls are filtered before this worker card
                # path and therefore never enter its durable/replayable
                # activity stream.
                # The stop can arrive while the card metadata above is being
                # prepared.  Recheck at the last boundary before entering
                # the engine so a queued stale worker cannot create a model
                # turn after the terminal stop was committed.
                if acp_steering.is_stopped(self._store, task_id):
                    with self._lock:
                        self._worker_turns_in_flight.discard(assignment_ref)
                        if worker_recovery_used:
                            self._worker_recoveries_in_flight.discard(assignment_ref)
                    return
                worker_outcome = None
                try:
                    worker_outcome = self._engine.prompt(
                        result.engine_session_ref, delivered_prompt, cancel_event,
                        on_delta=_on_worker_delta, on_tool_call=_on_worker_tool_call,
                        on_display_update=_on_worker_display_update,
                    )
                    outcome = worker_outcome
                    worker_terminal_text = bool(outcome.text)
                    # PromptOutcome.text is the authoritative terminal text.
                    # Prefer it over buffered deltas to avoid duplicating a
                    # provider that reports the same final both ways, and to
                    # recover a final returned only after interim chatter.
                    if outcome.text:
                        worker_message_buffer.clear()
                        with self._lock:
                            self._worker_final_response_seen.add(assignment_ref)
                        attempt_id = str(getattr(result, "attempt_id", None) or result.engine_session_ref)
                        if not self._record_worker_activity(
                            task_id, assignment_ref, attempt_id, "final-response",
                            f"[final response] {scrub_text(outcome.text, max_chars=max(4000, len(outcome.text) * 2 + 1000))}",
                            status="in_progress",
                        ):
                            return
                        with self._lock:
                            _final_worker_feed = self._worker_card_emitter(task_id, assignment_ref)
                        self._emit_worker_card(
                            _final_worker_feed, assignment_ref, status="in_progress",
                            entry=f"[final response] {scrub_text(outcome.text, max_chars=max(4000, len(outcome.text) * 2 + 1000))}",
                        )
                finally:
                    pending_stream_text = worker_stream_redactor.flush()
                    if pending_stream_text:
                        worker_message_buffer.append(pending_stream_text)
                    with self._lock:
                        _final_worker_feed = self._worker_card_emitter(task_id, assignment_ref)
                    # A cancellation/engine failure can leave commentary
                    # chunks buffered even though the provider never
                    # produced a terminal answer.  Do not relabel that
                    # partial stream as a successful "final response".
                    # Only an observed normal end_turn makes a delta-only
                    # terminal message authoritative.
                    _flush_worker_message(
                        feed=_final_worker_feed, final=True,
                        final_response=(
                            worker_outcome is not None
                            and worker_outcome.stop_reason == StopReason.END_TURN
                            and not worker_terminal_text
                        ),
                    )
                    task_stopped = acp_steering.is_stopped(self._store, task_id)
                    with self._lock:
                        pending_result = getattr(self, "_pending_worker_results", {}).pop(
                            assignment_ref, None
                        )
                        hold_recovery_fence = (
                            worker_outcome is not None
                            and worker_outcome.stop_reason == StopReason.END_TURN
                            and pending_result is None
                            and not worker_recovery_used
                            and not cancel_event.is_set()
                            and not task_stopped
                        )
                        if not hold_recovery_fence:
                            self._worker_turns_in_flight.discard(assignment_ref)
                        # A publish_result observed while this turn was
                        # running is intentionally fenced here, after the
                        # final response flush above.  Pop it atomically
                        # with the in-flight transition so the event-claim
                        # path cannot race us into terminalizing first.
                        # The *current* action is over; the log of what
                        # this worker did is not. A worker that finishes
                        # between two wait ticks used to have its whole
                        # history discarded before anything could show it,
                        # so its card said only that it had finished. The
                        # log now survives to the result card, where a
                        # reader can open it and see what was done.
                        self._worker_activity.pop(assignment_ref, None)
                    if pending_result is not None:
                        # The result is now the card's terminal transition;
                        # cancellation may have won while the turn was
                        # unwinding, in which case _emit_worker_card's
                        # first-terminal-wins fence retains the cancellation.
                        self._close_assignment(task_id, assignment_ref)
                        summary = pending_result.get("summary", "")
                        with self._lock:
                            has_final_response = assignment_ref in self._worker_final_response_seen
                            pending_feed = self._worker_card_emitter(task_id, assignment_ref)
                        self._emit_worker_card(
                            pending_feed, assignment_ref, status="completed",
                            entry=("Result: " + _compact_text(summary, _WORKER_ACTIVITY_MAX_CHARS - 8)
                                   if summary and not has_final_response else None),
                        )
                if pending_result is None and outcome.stop_reason == StopReason.END_TURN:
                    # A publish event may be delivered after the prompt's
                    # finally block popped the pending slot.  The recovery
                    # fence keeps the assignment in-flight while this
                    # second atomic check closes that publication race.
                    with self._lock:
                        pending_result = getattr(self, "_pending_worker_results", {}).pop(
                            assignment_ref, None
                        )
                    if pending_result is not None:
                        self._close_assignment(task_id, assignment_ref)
                        summary = pending_result.get("summary", "")
                        with self._lock:
                            has_final_response = assignment_ref in self._worker_final_response_seen
                            pending_feed = self._worker_card_emitter(task_id, assignment_ref)
                        self._emit_worker_card(
                            pending_feed, assignment_ref, status="completed",
                            entry=("Result: " + _compact_text(summary, _WORKER_ACTIVITY_MAX_CHARS - 8)
                                   if summary and not has_final_response else None),
                        )
                # A normal END_TURN is not itself a worker failure.  Give a
                # worker which left no question, result, or owned published
                # report one bounded, same-session request to complete its
                # existing Cortex report protocol.  This is runtime
                # procedure, deliberately separate from the user's text.
                if (
                    outcome.stop_reason == StopReason.END_TURN
                    and not pending_result
                    and not worker_recovery_used
                    and not cancel_event.is_set()
                    and not acp_steering.is_stopped(self._store, task_id)
                ):
                    with self._worker_recovery_claim_lock:
                        recovery_state = self._claim_worker_same_session_recovery(task_id, assignment_ref)
                        if recovery_state == "claimed":
                            with self._lock:
                                self._worker_recoveries_in_flight.add(assignment_ref)
                    if recovery_state == "claimed":
                        self._finalize_generation_delivery(
                            result.engine_session_ref, generation_delivery_id,
                            failure_kind=None, stop_reason=outcome.stop_reason,
                        )
                        generation_delivery_id = None
                        worker_recovery_used = True
                        recovery_prompt = (
                            "Runtime completion follow-up for this existing assignment. "
                            "Continue in this same session. Compare the work actually completed "
                            "and the checks actually run with the assignment's deliverables and "
                            "acceptance criteria. Finish missing implementation and necessary "
                            "verification before handing off. Then create a truthful Cortex report "
                            "and publish the result through the advertised tools, including any "
                            "remaining gaps or unrun checks. If you need information to proceed, "
                            "use ask_question for the delegating coordinator. Do not claim work "
                            "or checks you did not perform, or repeat unrelated work."
                        )
                        continue
                    if recovery_state == "already_running":
                        # Another runtime owns the one continuation. Leave
                        # the assignment working; its owner will reconcile
                        # the eventual result or terminal failure.
                        with self._lock:
                            self._worker_turns_in_flight.discard(assignment_ref)
                            self._worker_recoveries_in_flight.discard(assignment_ref)
                        return
                    if recovery_state == "orphaned":
                        # A process restart left only the durable claim; its
                        # original ACP session is gone, so fail closed via
                        # the existing result/publication race reconciler.
                        self._close_assignment_that_produced_nothing(task_id, assignment_ref)
                        with self._lock:
                            self._worker_turns_in_flight.discard(assignment_ref)
                            self._worker_recoveries_in_flight.discard(assignment_ref)
                        return
                    if recovery_state == "complete" and pending_result is None:
                        # The durable result won just after the worker
                        # finally block's pending-slot check.  Mirror its
                        # terminal card here so the result is visible even
                        # if the event waiter observes it after this turn.
                        with self._store.connection() as db:
                            attempt_row = db.execute(
                                "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? "
                                "AND idempotency_key=?",
                                (task_id, assignment_ref),
                            ).fetchone()
                            result_refs = [assignment_ref]
                            if attempt_row is not None and attempt_row["attempt_id"]:
                                result_refs.append(attempt_row["attempt_id"])
                            placeholders = ",".join("?" for _ in result_refs)
                            result_row = db.execute(
                                "SELECT payload_json FROM task_events WHERE task_id=? "
                                f"AND assignment_ref IN ({placeholders}) "
                                "AND event_type='worker_result' AND superseded=0 "
                                "ORDER BY id DESC LIMIT 1",
                                (task_id, *result_refs),
                            ).fetchone()
                        if result_row is not None:
                            try:
                                result_payload = json.loads(result_row["payload_json"])
                            except (TypeError, ValueError, json.JSONDecodeError):
                                result_payload = {}
                            summary = result_payload.get("summary", "") if isinstance(result_payload, dict) else ""
                            with self._lock:
                                has_final_response = assignment_ref in self._worker_final_response_seen
                                pending_feed = self._worker_card_emitter(task_id, assignment_ref)
                            self._emit_worker_card(
                                pending_feed, assignment_ref, status="completed",
                                entry=("Result: " + _compact_text(summary, _WORKER_ACTIVITY_MAX_CHARS - 8)
                                       if summary and not has_final_response else None),
                            )
                with self._lock:
                    # The first-turn fence was held through the durable
                    # result/publication check.  Every non-continuation
                    # path (including ``complete``) must release it before
                    # returning to the coordinator wait loop.
                    self._worker_turns_in_flight.discard(assignment_ref)
                    self._worker_recoveries_in_flight.discard(assignment_ref)
                break
            except EngineTurnFailed as failure:
                # T017: a worker's own engine turn failing (transport
                # closed, timed out, poisoned session, ...) must produce an
                # assignment_failed event -- unless T027's own
                # authorize_fallback explicitly permits a deterministic,
                # never-yet-tried alternate route for the 'engine_failure'
                # class; a route this task never authorized/recorded, or a
                # task with no fallback policy at all, still gets the
                # ordinary assignment_failed and is never silently retried
                # on the same route.
                self._finalize_generation_delivery(result.engine_session_ref, generation_delivery_id, failure_kind=failure.kind, stop_reason=None)
                # T028: classify this failure before ever considering
                # fallback -- only a genuine ENGINE_FAILURE classification
                # is fallback-eligible (see engine.failure_classification's
                # module docstring). A rate-limit/auth-loss/provider-
                # restriction/refusal/cancellation/indeterminate reading of
                # the same failure.kind text never reaches
                # _attempt_worker_route_fallback at all -- it is refused
                # here, before any route/consent lookup, not merely relying
                # on routing.authorization's own denylist as the only gate.
                failure_class = failure_classification.classify(failure.kind)
                old_attempt_id = getattr(result, "attempt_id", None)
                new_session_ref = None
                fallback_refused = False
                new_attempt_id = None
                role_id = getattr(result, "role_id", payload["role"])
                assignment_id = getattr(result, "assignment_id", None)
                workspace_binding_id = getattr(result, "workspace_binding_id", None)
                config_revision = getattr(result, "config_revision", None)
                if (
                    old_attempt_id is not None and _hop < self._MAX_FALLBACK_HOPS
                    and failure_classification.is_fallback_eligible(failure_class)
                ):
                    from ..portable.roles import load_role

                    role_set = load_role(role_id)
                    new_session_ref, fallback_refused, new_attempt_id = self._attempt_worker_route_fallback(
                        task_id, assignment_ref, assignment_id, role_id, old_attempt_id,
                        workspace_binding_id, config_revision, failure_class, composed_instructions(role_set),
                    )
                if new_session_ref is None:
                    if not fallback_refused:
                        # Fallback was never even consulted (no attempt id
                        # to reconcile/replace, no real configuration
                        # revision to evaluate it against, the hop ceiling
                        # was reached, or this failure_class is not
                        # fallback-eligible at all) -- the ordinary
                        # engine-failure event is the only one written for
                        # this path, carrying the distinguishable class.
                        if not self._assignment_already_reported(task_id, assignment_ref):
                            self._mark_work_attempt_failed(task_id, assignment_ref, f"engine_failure:{failure.kind}", f"work-failed:{assignment_ref}:engine")
                            self._append_assignment_failure_if_unreported(
                                task_id, assignment_ref,
                                dict(error=f"engine_failure:{failure.kind}", kind=failure.kind,
                                     failure_class=failure_class),
                            )
                    # Else: _attempt_worker_route_fallback already recorded
                    # the concrete refusal reason itself -- never a second,
                    # duplicate assignment_failed event for the same failure.
                    with self._lock:
                        self._worker_recoveries_in_flight.discard(assignment_ref)
                    return
                # Re-deliver the identical brief to the fallback attempt's
                # fresh session on the next loop iteration -- the same
                # canonical instructions/capsule path a first-time worker
                # prompt uses (T021/T022), never a retried in-flight prompt
                # against the dead session.
                self._worker_sessions[(task_id, assignment_ref)] = new_session_ref
                from types import SimpleNamespace

                result = SimpleNamespace(
                    engine_session_ref=new_session_ref, attempt_id=new_attempt_id, assignment_id=assignment_id,
                    role_id=role_id, workspace_binding_id=workspace_binding_id, config_revision=config_revision,
                )
            except BaseException as error:
                # T017 fix: this call was previously unwrapped -- any other
                # exception from a worker's prompt silently lost the
                # assignment (no durable record at all). Record it instead.
                self._finalize_generation_delivery(result.engine_session_ref, generation_delivery_id, failure_kind="unexpected_exception", stop_reason=None)
                with self._lock:
                    self._worker_recoveries_in_flight.discard(assignment_ref)
                self._mark_work_attempt_failed(task_id, assignment_ref, type(error).__name__, f"work-failed:{assignment_ref}:exception")
                delegation_events.append_event(
                    self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    dict(error=type(error).__name__), origin="runtime", assignment_ref=assignment_ref,
                )
                return
        self._finalize_generation_delivery(result.engine_session_ref, generation_delivery_id, failure_kind=None, stop_reason=outcome.stop_reason)
        with self._lock:
            active_coordination_turn = task_id in self._active_cancel_events
        if outcome.stop_reason == StopReason.END_TURN and active_coordination_turn:
            # The engine has positively ended this worker turn.  At this
            # point every synchronous MCP call from the turn has returned,
            # so reconcile its durable result/question/report immediately
            # instead of leaving the assignment open for a later polling
            # tick.  The transactional helper preserves a real question or
            # publication and records a failure only when none exists.
            # This also closes a shared-root worker which changed files but
            # forgot publish_result; that state previously remained falsely
            # "running" forever behind the direct-change grace fence.
            #
            # Provisioning can also be replayed outside a live host turn
            # during recovery.  In that path there is no active coordinator
            # cancel/event boundary and worker_reconciliation is the owner
            # of result-vs-lost races and capacity release.  Do not preempt
            # it with a synthetic clean-turn failure.
            self._close_assignment_that_produced_nothing(task_id, assignment_ref)
        elif outcome.stop_reason == StopReason.REFUSAL:
            # A worker refusal is the coordinator's to evaluate, never
            # silently retried on another engine/connection/model.
            self._mark_work_attempt_failed(task_id, assignment_ref, "refusal", f"work-failed:{assignment_ref}:refusal")
            delegation_events.append_event(
                self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                dict(error="refusal", kind="refusal", text=outcome.text or ""),
                origin="runtime", assignment_ref=assignment_ref,
            )
        elif outcome.stop_reason in (StopReason.MAX_TOKENS, StopReason.MAX_TURN_REQUESTS):
            # Recorded as informative progress (not a failure): the worker
            # may still ask a question or publish a partial result on a
            # later turn; this durably notes the limit for the coordinator.
            delegation_events.append_event(
                self._store, task_id, "progress", f"worker_limit:{assignment_ref}:{outcome.stop_reason.value}",
                dict(note=f"Assignment {assignment_ref} worker turn ended: {outcome.stop_reason.value} limit."),
                origin="runtime", assignment_ref=assignment_ref,
            )

    def _integrated_without_acceptance(self, task_id: str) -> bool:
        """A worker's change is in the project and nothing accepted it.

        Both halves read the durable record only: an ``integrate`` progress
        row for this task, and no ``coordinator_decision`` accepting it
        afterwards. A turn that ends any other way (still running, waiting
        on a question, nothing delivered yet) says nothing.
        """
        try:
            with self._store.connection() as db:
                integrated = db.execute(
                    "SELECT id FROM task_events WHERE task_id=? AND event_type='progress' "
                    "AND dedup_key LIKE 'integrate:%' ORDER BY id DESC LIMIT 1",
                    (task_id,),
                ).fetchone()
                if integrated is None:
                    return False
                accepted = db.execute(
                    "SELECT 1 FROM task_events WHERE task_id=? AND event_type='coordinator_decision' "
                    "AND id>? AND payload_json LIKE ? LIMIT 1",
                    (task_id, integrated["id"], '%"decision": "accept"%'),
                ).fetchone()
            return accepted is None
        except Exception:  # noqa: BLE001 - never fail a turn over a notice
            return False

    def _assignment_already_reported(self, task_id: str, assignment_ref: str) -> bool:
        """Whether this assignment already has a terminal event of its own.

        Live: a worker published its result at 02:04:39 and its engine
        session then closed, which the runtime read as an engine failure
        and recorded as ``fallback_refused`` one second later -- a failure
        event for an assignment that had just succeeded. Recovery has
        re-checked for exactly this since T023; the engine-failure path
        never did.

        Both refs, for the same reason ``_close_assignment_that_produced_
        nothing`` checks both: a worker reports through its own channel,
        so its result carries the attempt's ref.
        """
        try:
            with self._store.connection() as db:
                refs = [assignment_ref]
                row = db.execute(
                    "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                    (task_id, assignment_ref),
                ).fetchone()
                if row is not None and row["attempt_id"]:
                    refs.append(row["attempt_id"])
                placeholders = ",".join("?" for _ in refs)
                found = db.execute(
                    f"SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref IN ({placeholders}) "
                    "AND superseded=0 AND event_type IN ('worker_result','worker_question',"
                    "'assignment_cancelled') LIMIT 1",
                    (task_id, *refs),
                ).fetchone()
            return found is not None
        except Exception:  # noqa: BLE001 - never fail a failure path over this
            return False

    def _claim_worker_same_session_recovery(self, task_id: str, assignment_ref: str) -> str:
        """Claim the single procedural continuation for an empty worker turn.

        The claim and the result/publication check share one immediate
        transaction.  ``already_running`` is distinct from ``complete``:
        another runtime may have claimed the continuation and still be
        inside the same worker session, so this caller must not terminalize
        the assignment while that owner is working.
        """
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                refs = [assignment_ref]
                attempt_row = db.execute(
                    "SELECT attempt_id FROM delegation_provisioning "
                    "WHERE task_id=? AND idempotency_key=?",
                    (task_id, assignment_ref),
                ).fetchone()
                if attempt_row is not None and attempt_row["attempt_id"]:
                    refs.append(attempt_row["attempt_id"])
                placeholders = ",".join("?" for _ in refs)
                terminal = db.execute(
                    f"SELECT 1 FROM task_events WHERE task_id=? "
                    f"AND assignment_ref IN ({placeholders}) AND superseded=0 "
                    "AND event_type IN ('worker_result','worker_question',"
                    "'assignment_failed','assignment_cancelled') LIMIT 1",
                    (task_id, *refs),
                ).fetchone()
                if terminal is not None:
                    db.commit()
                    return "complete"
                published = db.execute(
                    "SELECT 1 FROM drafts d JOIN trusted_channels c "
                    "ON c.principal_thread_id=d.owner_thread_id "
                    "WHERE d.task_id=? AND c.task_id=? AND c.attempt_id IN "
                    f"({placeholders}) AND d.kind='report' "
                    "AND d.published_report_id IS NOT NULL LIMIT 1",
                    (task_id, task_id, *refs),
                ).fetchone()
                if published is not None:
                    db.commit()
                    return "complete"
                marker = db.execute(
                    "SELECT 1 FROM task_events WHERE task_id=? "
                    "AND assignment_ref=? AND event_type='progress' "
                    "AND dedup_key=? LIMIT 1",
                    (task_id, assignment_ref, f"worker_recovery:{assignment_ref}"),
                ).fetchone()
                if marker is not None:
                    db.commit()
                    if assignment_ref in self._worker_recoveries_in_flight:
                        return "already_running"
                    # A marker can outlive this runtime.  Reclaim it only
                    # when the owning engine attempt is observably absent;
                    # an alive/unknown attempt is still potentially inside
                    # its same-session follow-up, so fail closed against a
                    # duplicate prompt.
                    lookup_owned = getattr(self._engine, "lookup_owned", None)
                    if lookup_owned is None or not refs[-1]:
                        return "already_running"
                    try:
                        attempt_state = lookup_owned(refs[-1])
                    except Exception:
                        attempt_state = "unknown"
                    # The original ACP session cannot be resumed once its
                    # owned attempt is absent.  Do not send the follow-up
                    # into a fresh/unknown session: let the caller perform
                    # the normal race-safe terminal reconciliation.
                    return "orphaned" if attempt_state == "absent" else "already_running"
                delegation_events.append_event_tx(
                    db, task_id, "progress", f"worker_recovery:{assignment_ref}",
                    dict(kind="worker_same_session_recovery", assignment_ref=assignment_ref),
                    origin="runtime", assignment_ref=assignment_ref,
                )
                db.commit()
                return "claimed"
            except BaseException:
                db.rollback()
                raise

    def _append_assignment_failure_if_unreported(
        self, task_id: str, assignment_ref: str, payload: dict[str, object],
    ) -> bool:
        """Write an engine failure only if no worker result won the race.

        The fast check above is necessarily racy: a worker may publish its
        result after that check and before the failure insert. The final
        result/failure choice is serialized with ``BEGIN IMMEDIATE`` so the
        durable event log cannot contain a false failure after a result.
        """
        dedup_key = f"assignment_failed:{assignment_ref}"
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                refs = [assignment_ref]
                row = db.execute(
                    "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                    (task_id, assignment_ref),
                ).fetchone()
                if row is not None and row["attempt_id"]:
                    refs.append(row["attempt_id"])
                placeholders = ",".join("?" for _ in refs)
                result = db.execute(
                    f"SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref IN ({placeholders}) "
                    "AND superseded=0 AND event_type IN ('worker_result','worker_question') LIMIT 1",
                    (task_id, *refs),
                ).fetchone()
                if result is not None:
                    db.rollback()
                    return False
                delegation_events.append_event_tx(
                    db, task_id, "assignment_failed", dedup_key, payload,
                    origin="runtime", assignment_ref=assignment_ref,
                )
                db.commit()
                return True
            except BaseException:
                db.rollback()
                raise

    def _report_assignments_whose_provisioning_failed(self, task_id: str) -> None:
        """Turn a delegation that could not even be provisioned into a
        failure somebody is told about.

        A worker that never opens produces no events at all: no
        ``assignment_ready``, no result, no failure -- so neither the
        open-assignment sweep below nor the coordinator has anything to go
        on. Live (2026-09-19) the engine's credentials were removed
        mid-task: the provisioning row sat there with its reason, the
        coordinator answered "waiting for the worker's report" three turns
        running, and nothing ever said the worker could not start.

        Only rows that already carry a reason (a refusal that knew its own
        code) are reported; a row still in progress is still in progress.
        """
        mark_failed = False
        with self._store.connection() as db:
            rows = db.execute(
                "SELECT idempotency_key, error_code FROM delegation_provisioning "
                "WHERE task_id=? AND status='failed' AND error_code IS NOT NULL",
                (task_id,),
            ).fetchall()
        for row in rows:
            assignment_ref = row["idempotency_key"]
            if self._assignment_already_reported(task_id, assignment_ref):
                continue
            self._mark_work_attempt_failed(task_id, assignment_ref, row["error_code"], f"work-failed:{assignment_ref}:provisioning")
            delegation_events.append_event(
                self._store, task_id, "assignment_failed",
                f"provisioning_failed:{assignment_ref}",
                dict(error=row["error_code"], assignment_ref=assignment_ref,
                     detail="the worker could not be started"),
                origin="runtime", assignment_ref=assignment_ref,
            )

    def _close_assignment_that_produced_nothing(self, task_id: str, assignment_ref: str) -> None:
        """A worker's turn ended cleanly having published no result, asked
        no question and failed in no way: the assignment produced nothing
        and is over.

        Recorded, because until now it was not: the turn simply ended and
        the assignment stayed open forever. That was survivable only while
        the coordinator's patience ran out on its own -- and its patience
        is now deliberately unbounded, because a worker may legitimately
        work for hours. An assignment nobody is working on any more must
        therefore close itself, or the coordinator waits on it until the
        user cancels.

        Never written when the worker did produce something: the check and
        the write share one transaction, so a result published in the
        moment between them wins and this event is not written at all.
        """
        mark_failed = False
        reported_work_attempt_ref = None
        reported_event_id = None
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                # Both refs: a worker publishes through its own channel, so
                # its result/question carries the *attempt's* ref, while
                # the runtime's own events for the same assignment carry
                # the idempotency key. Checking only one of the two reads
                # a worker that did report as one that said nothing.
                refs = [assignment_ref]
                attempt_row = db.execute(
                    "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                    (task_id, assignment_ref),
                ).fetchone()
                if attempt_row is not None and attempt_row["attempt_id"]:
                    refs.append(attempt_row["attempt_id"])
                placeholders = ",".join("?" for _ in refs)
                existing = db.execute(
                    f"SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref IN ({placeholders}) "
                    "AND superseded=0 AND event_type IN ('worker_result','worker_question',"
                    "'assignment_failed','assignment_cancelled') LIMIT 1",
                    (task_id, *refs),
                ).fetchone()
                if existing is None:
                    # Before declaring that nothing came of this: did the
                    # worker publish a report and simply never say so? Live
                    # (2026-09-19, a user's own thread) one did exactly
                    # that -- report r_1777da4c6513 published through its
                    # own channel at 14:43:10, the assignment declared
                    # empty at 14:43:15, and the same work delegated again
                    # from scratch. The work existed; only the sentence
                    # about it was missing.
                    #
                    # Attribution is durable and exact, never a guess: the
                    # draft records the thread that owns it, and that
                    # thread is this attempt's own worker channel.
                    published = db.execute(
                        "SELECT d.published_report_id AS report_ref FROM drafts d "
                        "JOIN trusted_channels c ON c.principal_thread_id = d.owner_thread_id "
                        "WHERE d.task_id=? AND c.task_id=? AND c.attempt_id IN "
                        f"({placeholders}) AND d.kind='report' "
                        "AND d.published_report_id IS NOT NULL "
                        "ORDER BY d.created_at DESC LIMIT 1",
                        (task_id, task_id, *refs),
                    ).fetchone()
                    if published is not None and published["report_ref"]:
                        # A report can be durably published through the
                        # worker's MCP channel even when the worker's final
                        # turn does not include a sentence saying that it
                        # published it.  This fallback creates the
                        # coordinator-facing worker_result, so it must also
                        # advance the same work-graph attempt as the normal
                        # publish_result path.  Leaving it running wedges
                        # dependent assignments and capacity recovery.
                        request = db.execute(
                            "SELECT payload_json FROM task_events "
                            "WHERE task_id=? AND assignment_ref=? "
                            "AND event_type='delegation_requested' "
                            "ORDER BY id DESC LIMIT 1",
                            (task_id, assignment_ref),
                        ).fetchone()
                        if request is not None:
                            try:
                                reported_work_attempt_ref = json.loads(request["payload_json"]).get("work_attempt_ref")
                            except (TypeError, ValueError, json.JSONDecodeError):
                                reported_work_attempt_ref = None
                        silent_result = delegation_events.append_event_tx(
                            db, task_id, "worker_result", f"assignment_silent_result:{assignment_ref}",
                            dict(report_ref=published["report_ref"],
                                 summary="The worker published this report and then ended its turn "
                                         "without reporting it. Read the report itself: nothing else "
                                         "was said about the work.",
                                 checks=[]),
                            origin="runtime", assignment_ref=assignment_ref,
                        )
                        reported_event_id = silent_result.id
                    else:
                        # The graph transition is applied after this
                        # transaction; opening another Store connection here
                        # would contend with the IMMEDIATE event transaction.
                        mark_failed = True
                        delegation_events.append_event_tx(
                            db, task_id, "assignment_failed", f"assignment_silent:{assignment_ref}",
                            dict(error="worker_ended_without_result", assignment_ref=assignment_ref),
                            origin="runtime", assignment_ref=assignment_ref,
                        )
                db.commit()
            except BaseException:
                db.rollback()
                raise
        if reported_work_attempt_ref:
            try:
                from ..delegation.work_graph import WorkGraph, WorkGraphError

                WorkGraph(self._store).transition_attempt(
                    reported_work_attempt_ref, "reported",
                    idempotency_key=f"work-reported:silent:{reported_event_id}",
                )
            except WorkGraphError as error:
                # Keep the worker_result, but retain an observable diagnostic
                # if a legacy or malformed graph cannot mirror it.
                try:
                    delegation_events.append_event(
                        self._store, task_id, "progress",
                        f"work_attempt_sync_failed:silent:{assignment_ref}",
                        dict(kind="work_attempt_sync_failed", assignment_ref=assignment_ref,
                             reason=_bounded_detail(str(error))),
                        origin="runtime", assignment_ref=assignment_ref,
                    )
                except Exception:
                    pass
        if mark_failed:
            self._mark_work_attempt_failed(
                task_id, assignment_ref, "worker_ended_without_result",
                f"work-failed:{assignment_ref}:silent",
            )

    def _cancel_assignment(
        self, task_id: str, assignment_ref: str, reason: str,
        *, emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        session_id: str | None = None,
    ) -> None:
        with self._store.connection() as db:
            row = db.execute(
                "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
        if row is not None:
            cancel_attempt = getattr(self._engine, "cancel_attempt", None)
            if cancel_attempt is not None:
                try:
                    cancel_attempt(row["attempt_id"])
                except Exception:
                    pass
            try:
                self._engine.terminate_owned(row["attempt_id"])
            except Exception:
                pass
        delegation_events.append_event(
            self._store, task_id, "assignment_cancelled", f"assignment_cancelled:{assignment_ref}",
            dict(reason=reason), origin="runtime", assignment_ref=assignment_ref,
        )
        # Keep the work graph projection aligned with the cancellation event.
        # The engine attempt id and the graph attempt id are different
        # namespaces; resolve the latter through the original delegation
        # request and terminalize it as a failed/cancelled attempt.  Without
        # this mirror a stopped task can still load with a graph row stuck in
        # ``running`` even though its owned engine process is gone.
        self._mark_work_attempt_failed(
            task_id, assignment_ref, reason,
            f"work-cancelled:{assignment_ref}",
        )
        # The stop path can run after the coordinator turn has returned (and
        # therefore outside the continuation loop that normally claims this
        # event).  Close the already-created worker card immediately so a
        # client never keeps animating a card whose owned work is cancelled.
        # ``emit_tool_call`` persists the host update before offering it to
        # the transport; the assignment event above is the observed reason.
        # The assignment binding always wins.  A stop initiated by a
        # reconnected session may supply its own current-session emitter, but
        # that session did not create this worker card and must never receive
        # an orphan update.  The explicit argument remains a compatibility
        # fallback for legacy callers/tests whose assignment has no binding.
        durable_sessions = self._durable_worker_card_sessions(task_id, assignment_ref)
        # A reconnect has no in-memory original emitter.  Use the current
        # emitter only when this exact card was created in this exact session;
        # otherwise append to the original session's durable feed and never
        # leak an update into the newly connected session.
        if emit_tool_call is not None and session_id in durable_sessions:
            if durable_sessions[session_id] not in {"completed", "failed"}:
                self._emit_worker_card(
                    emit_tool_call, assignment_ref,
                    status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                    entry=scrub_text(f"Cancelled: {reason}", max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
        else:
            with self._lock:
                bound_emitter = self._worker_card_emitters.get(assignment_ref)
            if bound_emitter is not None:
                self._emit_worker_card(
                    bound_emitter, assignment_ref,
                    status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                    entry=scrub_text(f"Cancelled: {reason}", max_chars=_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
            else:
                for owner_session, status in durable_sessions.items():
                    if status not in {"completed", "failed"}:
                        self._persist_terminal_worker_card(owner_session, assignment_ref, reason)
        # Not self._notify() here: a waiting coordinator observes the new
        # event via its own bounded DB poll instead (see
        # _wait_for_progress) -- no _running entry is added/removed by this
        # call, so there is nothing for _cond to usefully notify here.

    def _make_host_fs_checker(self, project_root: str, task_id: str):
        """T025 (lead follow-up) / EXT-01 #5: builds the per-path unsaved-
        host-buffer check ``delegation.integration.integrate`` consults,
        reusing the existing T014 ``engine.fs_bridge.HostFsForwarder``/
        ``RevisionLog`` abstraction (never a bespoke second wire protocol)
        rather than calling a host fs requester directly.

        The requester consulted is this exact call's own registered
        ``self._task_fs_requesters[task_id]`` (set by ``start_prompt`` for
        the duration of the call that is running this integration -- see
        that method's own docstring) if one was registered, else this
        runtime's constructor-time ``self._host_fs_requester`` default (kept
        only for callers/tests that never opt into the per-task mechanism).

        Returns ``None`` -- capability genuinely absent -- when neither is
        set (host never negotiated fs, or no live connection is wired for
        this task right now); the caller then durably records
        ``host_buffer_state: 'unknown'`` instead of silently treating
        "unknown" the same as "checked, no conflict".

        The returned ``checker`` *raises* (never silently returns ``None``)
        when the host round-trip for one specific path could not be trusted
        (timeout/closed transport/malformed reply) -- ``integrate()`` itself
        is responsible for turning that into a per-path conflict and an
        overall ``host_buffer_state: 'check_failed'`` (see its own
        docstring); a clean "the host has no buffer for this exact path"
        answer stays a plain ``None`` return, never a conflict by itself."""
        requester = self._task_fs_requesters.get(task_id)
        if requester is None:
            requester = self._host_fs_requester
        if requester is None:
            return None
        from ..engine.fs_bridge import HostFsForwarder

        forwarder = HostFsForwarder(host_fs_requester=requester)
        binding_stub = _IntegrationHostBinding(f"integration:{task_id}")

        def checker(relpath: str):
            absolute_path = os.path.join(project_root, relpath)
            result = forwarder.read_text_file(binding_stub, dict(path=absolute_path))
            content = result.get("content") if isinstance(result, dict) else None
            if not isinstance(content, str):
                raise ValueError(f"host fs checker: malformed/missing content for {relpath!r}.")
            return content.encode("utf-8", "surrogateescape")

        return checker

    def host_mcp_servers_for_task(self, task_id: str):
        """The client's own MCP servers for the session currently running
        this task, or ``None`` when no session is (the same shape as
        :meth:`permission_requester_for_task`). Read by
        ``delegation.live_adapters`` when it opens an engine session, so a
        coordinator and every worker are offered the same tools the user
        configured in their editor."""
        with self._lock:
            servers = self._task_mcp_servers.get(task_id)
        return list(servers) if servers else None

    def permission_requester_for_task(self, task_id: str):
        """EXT-01 #5: the exact connection-bound
        ``(engine_session_id, params, timeout) -> dict`` callable currently
        registered for ``task_id`` (see ``start_prompt``'s own docstring),
        or ``None`` if no ``start_prompt`` call for this task is in flight
        right now. Called by
        ``delegation.live_adapters.EngineSessionManagerAdapter`` (see
        ``set_runtime``) after it has already resolved
        ``engine_session_id -> task_id`` itself -- this method never sees
        or needs an ``engine_session_id``, only the ``task_id`` that lookup
        already produced."""
        with self._lock:
            return self._task_permission_requesters.get(task_id)

    def _identical_integration_conflicts(self, task_id: str, assignment_ref: str, conflict_paths) -> int:
        """How many integration conflicts this assignment has already had
        with this exact path/reason signature. Signature, not count: a
        conflict that moved to a different path or a different reason is
        progress, and must not be counted against a retry."""
        import json

        signature = sorted(
            (path, (detail or {}).get("reason")) for path, detail in (conflict_paths or {}).items()
        )
        seen = 0
        with self._store.connection() as db:
            rows = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
                "AND assignment_ref=? AND dedup_key LIKE 'integration_conflict:%'",
                (task_id, assignment_ref),
            ).fetchall()
        for row in rows:
            try:
                previous = json.loads(row["payload_json"] or "{}").get("conflict_paths") or {}
            except (TypeError, ValueError):
                continue
            if sorted((path, (detail or {}).get("reason")) for path, detail in previous.items()) == signature:
                seen += 1
        return seen

    def _integrate_assignment(self, task_id: str, assignment_ref: str, request_key: str,
                              *, candidate_ref: str | None = None,
                              expected_base_digest: str | None = None) -> None:
        """T025 (lead follow-up): performs the actual technical application
        (``delegation.integration.integrate``) for an ``integrate``
        request, then appends the coordinator-visible outcome as its own
        durable ``progress`` event (never silently absorbed) --
        ``payload["kind"]`` is ``integration_applied`` (status
        ``applied``/``noop``) or ``integration_conflict`` (status
        ``conflict``), each dedup-keyed by ``request_key`` so a retried
        request never double-appends (see ``delegation.events
        .INTEGRATION_PROGRESS_KINDS`` on why these are ``progress`` rows,
        not a new ``event_type``)."""
        with self._store.connection() as db:
            provisioning_row = db.execute(
                "SELECT attempt_id, workspace_binding_id, task_id FROM delegation_provisioning "
                "WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
            task_row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        if provisioning_row is None or task_row is None or provisioning_row["workspace_binding_id"] is None:
            delegation_events.append_event(
                self._store, task_id, "progress", f"integration_conflict:{request_key}",
                dict(kind="integration_conflict", assignment_ref=assignment_ref, request_key=request_key,
                     conflict_paths={"*": {"reason": "unknown_assignment_or_workspace"}},
                     host_buffer_state="unknown"),
                origin="runtime", assignment_ref=assignment_ref,
            )
            return
        project_root = task_row["project_root"]
        host_fs_checker = self._make_host_fs_checker(project_root, task_id)
        if candidate_ref:
            from ..delegation.candidates import get_candidate

            candidate = get_candidate(self._store, candidate_ref)
            if (candidate is None or candidate["task_id"] != task_id
                    or provisioning_row["attempt_id"] not in candidate["attempt_ids"]
                    or expected_base_digest != candidate.get("project_base_digest")):
                result = dict(status="conflict", conflict_paths={"*": {"reason": "candidate_binding_mismatch"}},
                              host_buffer_state="unknown")
            else:
                try:
                    result = delegation_integration.integrate_candidate(
                        self._store, task_id=task_id, project_root=project_root,
                        candidate_id=candidate_ref, request_key=request_key,
                        expected_base_digest=expected_base_digest,
                        host_fs_checker=host_fs_checker,
                    )
                except StoreError as error:
                    result = dict(status="conflict", conflict_paths={"*": {"reason": str(error)}},
                                  host_buffer_state="unknown")
        else:
            result = delegation_integration.integrate(
                self._store, task_id=task_id, project_root=project_root, assignment_ref=assignment_ref,
                request_key=request_key, attempt_id=provisioning_row["attempt_id"],
                workspace_binding_id=provisioning_row["workspace_binding_id"], host_fs_checker=host_fs_checker,
            )
        if result["status"] == "conflict":
            conflict_paths = result["conflict_paths"]
            if isinstance(conflict_paths, list):
                conflict_paths = {path: {"reason": result.get("reason", "candidate_conflict")}
                                  for path in conflict_paths}
            payload = dict(
                kind="integration_conflict", assignment_ref=assignment_ref, request_key=request_key,
                candidate_ref=candidate_ref, conflict_paths=conflict_paths,
                host_buffer_state=result.get("host_buffer_state", "unknown"),
            )
            # A conflict is a legitimate "try again": the host had an
            # unsaved buffer, main moved. The identical conflict over and
            # over is not -- watched live, a worker finished, integration
            # conflicted, the coordinator sent it back to redo work that
            # was already done, and that ran in a circle. Count how many
            # times this exact signature has already happened for this
            # assignment, so the coordinator is told when retrying has
            # stopped being a strategy. Advisory, never a runtime gate: the
            # coordinator still owns what to do about it.
            repeats = self._identical_integration_conflicts(task_id, assignment_ref, conflict_paths)
            if repeats + 1 >= _IDENTICAL_INTEGRATION_CONFLICT_LIMIT:
                payload["repeated"] = repeats + 1
            delegation_events.append_event(
                self._store, task_id, "progress", f"integration_conflict:{request_key}",
                payload, origin="runtime", assignment_ref=assignment_ref,
            )
        else:
            # The project just changed, and this runtime changed it, on the
            # worker's own request. Record the resulting revision as
            # authorized: the coordinator-turn digest backstop compares the
            # project before and after a turn and cannot otherwise tell this
            # apart from the coordinator writing to the project itself --
            # observed live blocking a turn that had done everything right
            # (evidence/T035/RUN-08).
            try:
                with self._lock:
                    self._authorized_project_revisions.setdefault(task_id, []).append(
                        delegation_revision.capture_revision(project_root),
                    )
            except Exception:  # noqa: BLE001 - an unrecorded baseline only costs a false block
                pass
            delegation_events.append_event(
                self._store, task_id, "progress", f"integration_applied:{request_key}",
                dict(kind="integration_applied", assignment_ref=assignment_ref, request_key=request_key,
                     candidate_ref=candidate_ref,
                     status=result["status"], applied_paths=result["applied_paths"],
                     integrated_head=result["integrated_head"],
                     integrated_dirty_digest=result["integrated_dirty_digest"],
                     host_buffer_state=result["host_buffer_state"]),
                origin="runtime", assignment_ref=assignment_ref,
            )
        # Not self._notify() here -- same reasoning as _cancel_assignment
        # above.
