"""ACP session ownership, worker process services and unfiltered presentation.

Orchestration decisions are made by orchestration.controller and its durable
state machine. This module supplies engine provisioning and observation services;
models cannot invoke those services through the public MCP proposal interface.
"""
from __future__ import annotations

import collections
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass
import re
import json
import hashlib
import os
from pathlib import Path
import shlex
import sqlite3
import threading
import time
from typing import Any, Callable, Mapping, Protocol

from ..configuration import access_mode as configuration_access_mode
from ..configuration.policy import BENIGN_CONFIGURATION_ERROR_CODES
from ..connections.environment import DENYLIST_PATTERN
from ..connections.model import MAX_CONCURRENT_SESSIONS_LIMIT
from ..contracts import StoreError
from ..delegation import events as delegation_events
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
from . import sessions as acp_sessions
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
from . import wire_budget as acp_wire_budget
from .wire_budget import bound_value, clip_text, encoded_size

COORDINATOR_ROLE_ID = "orchestrator"
# Same-session follow-ups for a worker turn that ended without a result.
MAX_WORKER_CONTINUATIONS = 5
DEFAULT_MAX_NO_PROGRESS_CONTINUATIONS = 3
# No cap: the coordinator may continue as many times as the work needs
# ("У координатора не должно быть никаких ограничений, он хоть миллион
# может сделать продолжений подряд"). A genuine stall is still caught by
# the no-progress budget, and the user's own cancellation still ends any
# turn; a fixed number of continuations only ever ended useful work early.
# Kept as a settable parameter so a test can still bound a loop.
DEFAULT_MAX_TOTAL_CONTINUATIONS = None
# Worker threads per project runtime. Each dispatched worker holds one
# thread for its whole life, including while it waits in the per-connection
# capacity queue, so this pool must never be the binding limit: the real
# limit is each connection's own ``max_concurrent_sessions``
# (``connections.model.ConnectionRecord``), enforced across processes by
# ``delegation.capacity``. The pool is sized to that field's validated
# maximum, so it is never tighter than any single connection's configured
# limit, and stays a fixed, deterministic bound; only when several
# connections' limits together exceed it does it cap the project's total.
# ``ThreadPoolExecutor`` starts threads on demand, so idle headroom is free.
DEFAULT_WORKER_POOL_SIZE = MAX_CONCURRENT_SESSIONS_LIMIT
DEFAULT_WAIT_TIMEOUT_SECONDS = 5.0
_UNSET = object()
# Bound on the actionable-error text (e.g. resolve_role's own remediation,
# which lists every valid role id) carried in an assignment_failed event
# payload -- large enough to hold the full ~30-role-id catalog uncut today,
# small enough to never make one event payload unbounded as that catalog
# grows. Truncated with a marker, never silently dropped.
_ASSIGNMENT_FAILED_DETAIL_MAX_CHARS = 1200
# T035 visibility fix: coordinator prose is redacted before it enters a
# complete message block (scrub_text's own default MAX_TEXT_CHARS=400 is
# sized for a short logged field, not an engine paragraph). The block path
# remains bounded by the redaction helper without exposing token fragments.
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






def _revision_dispatch_receipt(raw_output: Any, depth: int = 0) -> bool:
    """Recognize an acknowledged successor, including ACP MCP wrappers."""
    if depth > 6:
        return False
    if isinstance(raw_output, str):
        try:
            return _revision_dispatch_receipt(json.loads(raw_output), depth + 1)
        except (ValueError, TypeError):
            return False
    if isinstance(raw_output, list):
        return any(_revision_dispatch_receipt(item, depth + 1) for item in raw_output)
    if not isinstance(raw_output, dict):
        return False
    if (raw_output.get("recorded") is True and raw_output.get("decision") == "revise"
            and isinstance(raw_output.get("continuation_ref"), str)
            and raw_output["continuation_ref"]):
        return True
    return any(_revision_dispatch_receipt(raw_output[key], depth + 1)
               for key in ("result", "content", "structuredContent", "text") if key in raw_output)








def _bounded_detail(text: str) -> str:
    if len(text) <= _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS:
        return text
    return text[: _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS - 3] + "..."




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


# The status suffix appended to a worker card's title (see
# worker-display.ts's parseWorkerCardTitle).
_WORKER_STATUS_SUFFIX = re.compile(r"(?:\s+·\s+(?:Completed with issues|Cancelled|Interrupted))+$")


def _acp_tool_call_status(status: str | None) -> str:
    return status if status in _ACP_TOOL_CALL_STATUSES else "in_progress"


def worker_card_outcome(node_status: str | None, result_json: str | None) -> tuple[str, str | None]:
    """Map the authoritative runtime node to an ACP card state and label."""
    # ACP has no ``completed_with_issues`` status. A worker whose result was
    # rejected for failed checks is complete, but must visibly retain that
    # outcome; a failed node without a result is an execution failure.
    if node_status == "failed":
        return ("completed", "Completed with issues") if result_json is not None else ("failed", None)
    return (_acp_tool_call_status(node_status), None)


# The same bounds Paseo's ``pipeline.ts`` reader applies to a worker's
# reported result, kept identical here so the projection embedded in the
# terminal ACP card matches (byte for byte) the RPC-read projection an older
# item still falls back to.  See ``integrations/paseo/server/pipeline.ts``.
_WORKER_RESULT_SUMMARY_MAX_CHARS = 600
_WORKER_RESULT_CHECK_NAME_MAX_CHARS = 300
_WORKER_RESULT_CHECK_BLOCKER_MAX_CHARS = 600
_WORKER_RESULT_MAX_CHECKS = 30


def _clip_worker_result_text(value: Any, max_chars: int) -> str | None:
    if not isinstance(value, str):
        return None
    text = scrub_text(value.strip(), max_chars=max(max_chars, len(value) * 2 + 1000))
    return text if len(text) <= max_chars else text[: max_chars - 1] + "…"


def worker_result_projection(payload: Mapping[str, Any]) -> dict[str, Any]:
    """The bounded, redacted ``{summary, checks}`` shown on a worker's card.

    Mirrors ``integrations/paseo/server/pipeline.ts``'s Python reader
    exactly (same bounds, same ``outcome``/``blocker`` shape) so a client can
    render this embedded projection and the RPC-read fallback identically.
    """
    checks = []
    for check in (payload.get("checks") or [])[:_WORKER_RESULT_MAX_CHECKS]:
        if not isinstance(check, Mapping):
            continue
        entry: dict[str, Any] = {
            "name": _clip_worker_result_text(check.get("name"), _WORKER_RESULT_CHECK_NAME_MAX_CHARS) or "check",
            "outcome": str(check.get("outcome") or "unverified"),
        }
        blocker = _clip_worker_result_text(check.get("blocker"), _WORKER_RESULT_CHECK_BLOCKER_MAX_CHARS)
        if blocker:
            entry["blocker"] = blocker
        # closedBy: an exact later rerun closed this failure (kept as run
        # history, resolved). claimedOutcome/evidenceGap: the runtime kept an
        # insufficiently-evidenced "passed" claim as unverified/failed. Both
        # are already stored by delegation.evidence.accept_checks -- never
        # re-derived here, matching pipeline.ts's identical projection.
        closed_by = check.get("closed_by")
        if isinstance(closed_by, str) and closed_by.strip():
            entry["closedBy"] = closed_by.strip()[:128]
        claimed_outcome = check.get("claimed_outcome")
        if isinstance(claimed_outcome, str) and claimed_outcome.strip():
            entry["claimedOutcome"] = claimed_outcome.strip()
        evidence_gap = check.get("evidence_gap")
        if isinstance(evidence_gap, str) and evidence_gap.strip():
            entry["evidenceGap"] = evidence_gap.strip()
        checks.append(entry)
    return {
        "summary": _clip_worker_result_text(payload.get("summary"), _WORKER_RESULT_SUMMARY_MAX_CHARS),
        "checks": checks,
    }


def _bounded_worker_result(projection: Mapping[str, Any]) -> dict[str, Any]:
    """Cap the embedded result projection at ``_WORKER_CARD_RESULT_MAX_BYTES``.

    Checks are dropped from the tail (``truncatedChecks`` counts them) before
    the summary is touched; the full result stays in the worker's report.
    """
    result = dict(projection)
    checks = list(result.get("checks") or [])
    dropped = 0
    while checks and encoded_size(result) > _WORKER_CARD_RESULT_MAX_BYTES:
        checks.pop()
        dropped += 1
        result["checks"] = checks
        result["truncatedChecks"] = dropped
    if encoded_size(result) > _WORKER_CARD_RESULT_MAX_BYTES:
        result = bound_value(result, _WORKER_CARD_RESULT_MAX_BYTES)
    return result


# A worker's card carries the one thing it is doing right now, not its
# whole feed -- so this bounds a single line, not a transcript.
_WORKER_ACTIVITY_MAX_CHARS = 300

# Worker-card wire budget.  Paseo 0.9.2 retains every provider event of a
# session, so a card that re-sent its whole growing transcript on every tool
# event and streamed word grew quadratically and exhausted the daemon heap.
# The durable Workers-panel history (task events) and the worker's report
# keep the complete account; the streamed card carries only a bounded view:
# no body at all on Paseo, and a short rolling tail on other clients.
#: How many recent worker actions a generic client's card shows.
_WORKER_CARD_TAIL_ENTRIES = 20
#: Upper bound of a generic client's card body.
_WORKER_CARD_BODY_MAX_BYTES = 4 * 1024
#: Per-card event budget.  Bytes per update are capped above, but Paseo keeps
#: every update, so the number of updates must be bounded too.  A status
#: transition (create, start, completed/failed) is always sent at once; any
#: other change (title, body) is a progress update, sent at most once per
#: interval and at most this many times per card.  A held-back change is not
#: lost: the latest state is recomputed on every call and goes out with the
#: card's next permitted update, always with its terminal one.
_WORKER_CARD_PROGRESS_MIN_INTERVAL_SECONDS = 30.0
_WORKER_CARD_PROGRESS_MAX_UPDATES = 8
#: Same-status revisions of a settled card (a later outcome label or result).
_WORKER_CARD_TERMINAL_REVISIONS = 2
#: Lifecycle order: a card moves forward only, so it has at most one create,
#: one start and one terminal transition.  Per card on the wire, at most
#: 3 + _WORKER_CARD_PROGRESS_MAX_UPDATES + _WORKER_CARD_TERMINAL_REVISIONS
#: updates, whatever the worker's tool count, output or duration.
_WORKER_CARD_STATUS_RANK = {"pending": 1, "in_progress": 2, "completed": 3, "failed": 3}
#: A failed card's one-line failure detail (Paseo requires a non-null error).
_WORKER_CARD_FAILURE_MAX_BYTES = 1024
#: The completed card's embedded ``{summary, checks}`` projection.
_WORKER_CARD_RESULT_MAX_BYTES = 16 * 1024
#: Tombstones for refs evicted from the rolling tail, so a late provider
#: update cannot resurrect an old row at the end of the card.
_WORKER_ACTION_EVICTED_REFS_MAX = 256
#: One ``key=value`` argument and one whole tool line on a worker card.
_WORKER_TOOL_ARGUMENT_MAX_CHARS = 120
_WORKER_TOOL_LINE_MAX_CHARS = 300
#: Tool arguments that carry a payload rather than an identifier; the card
#: names their size instead of embedding them (e.g. a whole written file).
_WORKER_TOOL_ELIDED_ARGUMENTS = frozenset({"content", "text"})
#: Coordinator tool cards: cap for each of rawInput, rawOutput and every
#: content block on the wire.
_COORDINATOR_CARD_FIELD_MAX_BYTES = 16 * 1024
#: Recent (tool, input) -> output digests a coordinator emitter remembers to
#: replace a repeated identical output with a reference to its first card.
_COORDINATOR_REPEAT_MEMORY = 64
#: Coordinator/engine tool-card ids whose disposition (own card or folded)
#: and open-card field digests a task remembers.  An evicted id that gets a
#: late update is treated as a new call and spends budget like one.
_COORDINATOR_SEEN_MEMORY = 1024
_WORKER_ACTION_SEPARATOR = "\n"


def _is_panel_surface(client_surface: Any) -> bool:
    """Whether the ACP client is Paseo (same test as ``Agent._combined_model_picker``)."""
    return str(client_surface or "").casefold() == "paseo"


def _field_digest(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False, default=str).encode("utf-8")
    ).hexdigest()


class _CoordinatorCardBudget:
    """One task's coordinator/engine tool-card budget (see ``wire_budget``).

    Shared by every coordinator emitter of the task, so it survives coordinator
    turns, emitter recreation and a reopened coordinator session.  A new
    runtime seeds it from the task's durable host journal, so a restarted
    Cortex does not grant the budget again.  All bookkeeping is bounded:
    ``seen`` and ``sent`` are LRU maps of ``_COORDINATOR_SEEN_MEMORY`` ids and
    ``outputs`` keeps ``_COORDINATOR_REPEAT_MEMORY`` keys.
    """

    def __init__(self, *, shown=0, errors_shown=0, shown_bytes=0, folded=0, folded_failed=0,
                 folded_updates=0, folded_sessions=()):
        self.lock = threading.Lock()
        self.shown = shown
        self.errors_shown = errors_shown
        self.shown_bytes = shown_bytes
        self.folded = folded
        self.folded_failed = folded_failed
        self.folded_sent = (folded, folded_failed)
        self.folded_updates = folded_updates
        self.folded_last_at: float | None = None
        self.folded_sessions: set = set(folded_sessions)
        # card id -> own card {"updates", "terminal", "held"} or folded call
        # {"kind", "title", "failed"}.
        self.seen: "collections.OrderedDict[str, dict]" = collections.OrderedDict()
        # Open own cards: digests of the fields already sent.
        self.sent: "collections.OrderedDict[str, dict[str, str]]" = collections.OrderedDict()
        # (call identity digest) -> (output digest, first card id).
        self.outputs: dict[str, tuple[str, str]] = {}

    def remember(self, table, key, value):
        table[key] = value
        table.move_to_end(key)
        while len(table) > _COORDINATOR_SEEN_MEMORY:
            table.popitem(last=False)

    def admit(self, status: str | None) -> bool:
        """Whether a call first seen now gets its own card; else it is folded."""
        if (self.shown < acp_wire_budget.COORDINATOR_CARD_BUDGET
                and self.shown_bytes < acp_wire_budget.COORDINATOR_CARD_BYTES_BUDGET):
            self.shown += 1
            return True
        if status == "failed" and self.errors_shown < acp_wire_budget.COORDINATOR_ERROR_CARD_BUDGET:
            self.errors_shown += 1
            return True
        self.folded += 1
        return False


def _without_mcp_text_mirror(raw_output: Any) -> Any:
    """Drop an MCP result's text blocks when they only repeat ``structuredContent``.

    An MCP ``tools/call`` result carries the same JSON twice: as
    ``structuredContent`` and serialized in ``content[].text``.  Engines
    forward both in rawOutput, doubling every coordinator tool card.  Paseo
    renders ``structuredContent`` first, so the text copy is removed only when
    it parses to exactly the same value.
    """
    if not isinstance(raw_output, Mapping):
        return raw_output
    result = raw_output.get("result")
    if not isinstance(result, Mapping):
        return raw_output
    structured = result.get("structuredContent")
    blocks = result.get("content")
    if structured is None or not isinstance(blocks, list) or not blocks:
        return raw_output
    if not all(isinstance(block, Mapping) and block.get("type") == "text"
               and isinstance(block.get("text"), str) for block in blocks):
        return raw_output
    try:
        mirrored = json.loads("".join(block["text"] for block in blocks))
    except ValueError:
        return raw_output
    if mirrored != structured:
        return raw_output
    return {**raw_output, "result": {key: value for key, value in result.items() if key != "content"}}


def _clip_chars(text: str, max_chars: int) -> str:
    return text if len(text) <= max_chars else text[: max_chars - 1].rstrip() + "…"


def _worker_card_tail_text(entries: list[str], earlier: int) -> str:
    """Render a generic client's bounded card body: the latest actions.

    Entries are already one scrubbed line each (see ``_append_worker_entry``).
    Older ones are summarized by count; the complete account is the durable
    Workers history and the worker's report.
    """
    lines = [line for line in entries if line]
    omitted = earlier
    while True:
        head = [f"… {omitted} earlier actions in task history"] if omitted else []
        body = _WORKER_ACTION_SEPARATOR.join(head + lines)
        if len(body.encode("utf-8")) <= _WORKER_CARD_BODY_MAX_BYTES or not lines:
            return body
        lines = lines[1:]
        omitted += 1


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
    text = scrub_display_text(str(text_value), max_chars=max(4000,len(str(text_value))*2+1000)).strip()
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


def _tool_argument_text(key: Any, value: Any) -> str:
    """One bounded ``key=value`` for a tool line.

    Payload arguments are named by size, never embedded; other values are
    scrubbed from a bounded prefix and clipped.  The durable worker activity
    keeps the complete redacted arguments.
    """
    text = str(value)
    if str(key) in _WORKER_TOOL_ELIDED_ARGUMENTS:
        return f"{key}=<{len(text.encode('utf-8'))} bytes>"
    limit = _WORKER_TOOL_ARGUMENT_MAX_CHARS
    return f"{key}={_clip_chars(scrub_text(text[: limit * 4], max_chars=4000), limit)}"


def _worker_tool_details(
    title: str, status: str | None, locations: Any, content: Any,
    raw_input: Any, raw_output: Any, project_root: str | None = None,
    *, kind: str | None = None,
) -> str:
    """Render one compact worker tool line without protocol JSON or output.

    The line is bounded (``_WORKER_TOOL_LINE_MAX_CHARS``): it is a label on a
    card, while the complete call stays in the durable worker activity.
    """
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
                prefix = str(command)[: _WORKER_TOOL_LINE_MAX_CHARS * 4]
                argument = scrub_text(prefix, max_chars=max(4000, len(prefix) * 2 + 1000))
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
                    values = [
                        _tool_argument_text(key, value)
                        for key, value in [
                            item for item in args.items()
                            if isinstance(item[1], (str, int, float, bool))
                        ][:4]
                    ]
                    if values:
                        argument += " " + ", ".join(values)
            else:
                values = [
                    _tool_argument_text(key, value)
                    for key, value in [
                        item for item in raw_input.items()
                        if isinstance(item[1], (str, int, float, bool))
                    ][:4]
                ]
                if values:
                    argument = ", ".join(values)
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
            # Keep room for the error beside a long command.
            line = _clip_chars(line, _WORKER_TOOL_LINE_MAX_CHARS // 2)
            detail_prefix = str(detail)[: _WORKER_TOOL_LINE_MAX_CHARS * 4]
            line += "\nError: " + scrub_text(detail_prefix, max_chars=max(4000, len(detail_prefix) * 2 + 1000))
    suffix = " [failed]" if failed else ""
    line = scrub_display_text(line, max_chars=max(4000, len(line) * 2 + 1000))
    return _clip_chars(line, _WORKER_TOOL_LINE_MAX_CHARS - len(suffix)) + suffix


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





# How soon a delegation asked for mid-turn is picked up. Small enough that
# a worker starts while the coordinator is still talking, large enough that
# an idle turn is not a busy loop against the store.

# How many times the identical integration conflict may repeat for one
# assignment before the coordinator is told that retrying is not a strategy.

# This runtime's own MCP tools, as an engine names them in a tool call.
# Their individual cards are noise in a session feed: a reader wants the
# work, not the bookkeeping that supports it.
# This runtime's own MCP tools, however an engine spells the server
# prefix: codex writes "mcp.cortex.create_task", claude writes
# "mcp__cortex__delegate". Matching only the first spelling put every one
# of the coordinator's own calls back in the feed the moment the engine
# changed.

# Cortex's private store is bookkeeping even when a provider reports the
# operation as an ordinary native file tool. Match a path component (rather
# than the substring) so project names such as ``my.cortex-file`` remain
# visible. Tool metadata is compared case-insensitively because providers do
# not consistently preserve the casing of displayed paths.

# Host-side computer/UI controls are runtime plumbing, not work performed on
# the delegated project.  Zed's live ACP mirror currently exposes the CUA
# bridge as e.g. ``mcp.cua_repl.js — cua_repl.js code=...``.  Match only the
# known runtime namespaces; ordinary project MCP servers (including codebase
# memory/search servers) must remain visible in the worker card.

# ``delegate`` is represented by the assignment lifecycle card. Showing the
# coordinator's Cortex MCP dispatch as a second top-level card duplicates the
# same work in Paseo; all Cortex MCP calls are runtime bookkeeping.

# The engine's own tool-discovery step, which it reports as a tool call of
# its own ("ToolSearch"). Like its self-review below, it is the engine
# working out what it can do, not an action on the project.

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





def _compact_text(value: Any, max_chars: int) -> str:
    """Flatten and bound one display value with a normal ellipsis."""
    text = scrub_display_text(str(value), max_chars=max(4000, len(str(value)) + 1000))
    text = " ".join(text.split())
    if len(text) > max_chars:
        return text[: max_chars - 1].rstrip() + "…"
    return text


# A worker card's title (role, work, and the action it is on right now).
_WORKER_CARD_TITLE_MAX_CHARS = 300


def _worker_card_title(base: Any, suffix: str | None = None) -> str:
    """The one rendering of a worker card title, bounded with its suffix kept.

    Every title a worker card sends passes through here, so two paths naming
    the same card can never produce two spellings of one title: a clipped and
    an unclipped copy of a long title used to alternate on every tool event,
    each counted as a change and sent to a host that keeps every update.
    Idempotent for its own output.
    """
    tail = f" · {suffix}" if suffix else ""
    return _compact_text(base, _WORKER_CARD_TITLE_MAX_CHARS - len(tail)) + tail

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


def _worker_card_line(entry: Any, project_root: str | None = None) -> str:
    """One display-safe, single-line, bounded worker-card entry.

    Only a bounded prefix is scrubbed, so the cost is independent of how
    large the underlying message or tool payload was.
    """
    text = str(entry or "").strip()
    if not text:
        return ""
    safe = _worker_mirror_text(text[: _WORKER_ACTIVITY_MAX_CHARS * 4], project_root)
    return _clip_chars(" ".join(safe.split()), _WORKER_ACTIVITY_MAX_CHARS)




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


@dataclass(frozen=True)
class PromptOutcome:
    """One engine session turn's outcome, as observed by this runtime.

    ``text`` is the visible assistant text for that turn. The coordinator
    emits it only as complete message blocks; worker text stays in the
    Workers activity history and is never a raw chat chunk.
    Any delegation MCP tool call the engine made during the turn has
    already been durably committed by the time this is returned (the ACP
    ``session/prompt`` response only arrives after the engine's own tool
    calls to its Cortex MCP subprocess complete).
    """

    stop_reason: StopReason
    text: str = ""


def _describe_failure(error: BaseException) -> dict:
    """What failed, in terms a person can act on, without user content.

    A bare class name told nobody anything: every Claude worker once failed
    as just ``PortableError`` while the actual cause was a refused,
    UNSUPPORTED instruction delivery (live, 2026-09-22). A portable/store
    error code and the failing source line carry no secret; the message is
    deliberately left out, since it can quote a path or a value.
    """
    code = getattr(error, "code", None)
    if code is None and isinstance(error, StoreError):
        code = str(error)
    where = ""
    traceback = error.__traceback__
    while traceback is not None:
        frame = traceback.tb_frame.f_code
        where = f"{os.path.basename(frame.co_filename)}:{traceback.tb_lineno}"
        traceback = traceback.tb_next
    name = type(error).__name__
    described = dict(error=f"{name}:{code}" if isinstance(code, str) and code else name)
    if where:
        described["where"] = where
    return described


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
    """ACP execution services driven by the deterministic orchestration controller."""

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
        # Per-attempt running max context-window size; see
        # _clamp_attempt_context_size's docstring for why.
        self._attempt_context_size_max: dict[str, int] = {}
        self._model_context_size_max: dict[str, int] = {}
        self._attempt_models: dict[str, str | None] = {}
        # The most recent scrubbed excerpt of each in-flight worker's own
        # engine turn, so its card can show what it is doing rather than
        # only that it is running. Written by the worker's own on_delta,
        # read by _continuation_loop's workers_running tick -- the tick
        # that already exists, never a second channel or a second card.
        self._worker_activity: dict[str, str] = {}
        # A worker's latest actions, one scrubbed bounded line each -- the
        # rolling tail a generic client's card body shows. At most
        # _WORKER_CARD_TAIL_ENTRIES lines; the complete account is durable
        # worker activity (the Workers panel) and the worker's report.
        self._worker_action_log: dict[str, list[str]] = {}
        # One rendered line per logical engine tool ref, as an absolute
        # position (evicted count + index). Status updates revise that line
        # instead of appending a second progress/completion row.
        self._worker_action_refs: dict[str, dict[str, int]] = {}
        # How many lines each worker's rolling tail has evicted.
        self._worker_action_dropped: dict[str, int] = {}
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
        # Per card: a digest of each field last sent (never the payload
        # itself) and when its body last changed, so an unchanged update is
        # never sent twice and body-only redraws are rate limited.
        self._worker_card_sent: dict[str, dict[str, Any]] = {}
        self._worker_card_clock: Callable[[], float] = time.monotonic
        # Per task: the coordinator/engine tool-card budget shared by all of
        # its coordinator emitters (see _CoordinatorCardBudget).
        self._coordinator_card_budgets: dict[str, _CoordinatorCardBudget] = {}
        self._coordinator_card_clock: Callable[[], float] = time.monotonic
        # Once a worker card reaches a terminal ACP status, a late activity
        # tick from the worker executor must not move it back to
        # ``in_progress``.  The activity watcher snapshots its input before
        # emitting, so merely clearing ``_worker_activity`` on completion is
        # racy; retain this terminal fence until the task runtime is gone.
        self._worker_card_terminal: dict[str, str] = {}
        # Client surface is an explicit ACP session signal (clientInfo.name).
        # Paseo's Cortex plugin provides the durable Workers panel and renders
        # only a worker card's title, status and result, so its cards carry no
        # body; other clients get a bounded rolling tail.
        self._worker_panel_mode: dict[str, bool] = {}
        # The panel decision is made per coordinator task, but worker cards
        # are keyed by assignment.  Bind that decision at card creation so
        # later worker callbacks (which do not carry the task id) keep the
        # surface of the connection that created the card.
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
        self._task_display_emitters: dict[str, object] = {}
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
        # Tasks whose host connection closed (plugin update/restart), and the
        # daemon teardown that follows: transport loss, not a user stop or a
        # worker failure.  See recovery.worker_reconciliation.
        self._host_disconnected: set[str] = set()
        self._shutting_down = threading.Event()
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
            # A new turn arrives over a live connection.
            self._host_disconnected.discard(task_id)
            self._session_task[session_id] = task_id
            self._worker_panel_mode[task_id] = _is_panel_surface(client_surface)
            self._task_display_emitters[task_id] = emit_display_update
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
            # A cancel ends this turn, not the session: releasing the session
            # made every later cortex tool call of the continuing coordinator
            # refused, so it could never submit again.
            invalidate_turn = getattr(self._engine, "invalidate_turn", None)
            invalidate = getattr(self._engine, "invalidate_session", None)
            try:
                if invalidate_turn is not None:
                    invalidate_turn(session_ref)
                elif invalidate is not None:
                    invalidate(session_ref, "session_cancelled")
            except Exception:
                pass
        return None

    def cancel_task(
        self, task_id: str, session_id: str,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
    ) -> bool:
        """Cancel a task and all of its owned work from an explicit client stop.

        This explicit task-wide adapter entry is distinct from incoming ACP
        ``session/cancel``, which must use ``cancel``: Paseo sends that same
        notification before follow-up prompts. The callback discards textual
        confirmation; worker-card updates remain durable for replay.
        """
        return self.stop_task(
            task_id, session_id, lambda _text: False,
            emit_tool_call=emit_tool_call, stop_source="paseo_session_cancel",
        )

    def disconnect_session(self, session_id: str, trigger: str | None = None) -> bool:
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
        call redundantly (idempotent, bounded).

        ``trigger`` is what the ACP process observed first (``sigterm``,
        ``stdin_eof``, ``writer_closed``), kept on the record.

        Returns whether the ``host_disconnected`` record was written, so the
        ACP agent records a session that was only loaded itself."""
        recorded = False
        task_id = self._session_task.get(session_id)
        if task_id is not None:
            # The host transport is now definitively closed.  Do not leave a
            # requester closure pointing at it: a later worker permission
            # request must fail closed rather than block on a dead pipe.
            self._release_task_host_requesters(task_id, session_id=session_id)
            with self._lock:
                # Marked before the event is set: the turn it wakes suspends
                # without cancelling work (orchestration.controller).
                self._host_disconnected.add(task_id)
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
                from ..recovery.worker_reconciliation import record_host_disconnect

                record_host_disconnect(
                    self._store, task_id, session_id, f"disconnect:{session_id}:{time.monotonic_ns()}",
                    turn=turn_state, permission_invalidated=permission_invalidated, trigger=trigger,
                )
                recorded = True
            except Exception:
                pass
        try:
            engine_reaper.sweep(self._store, self._engine)
        except Exception:
            pass
        return recorded

    def shutdown(self) -> None:
        """T017 (fresh review, MEDIUM): signal every queued-for-capacity
        open to abort promptly (never a reservation leak), terminate every
        owned coordinator/worker process with a confirmed/ambiguous
        release, and shut down the worker-dispatch executor. Idempotent;
        safe to call once at process/runtime teardown."""
        # First, so every turn and worker the teardown interrupts reads it
        # as transport loss rather than a failure (see transport_lost).
        self._shutting_down.set()
        shutdown = getattr(self._engine, "shutdown", None)
        if shutdown is not None:
            # Capacity-aware SessionRuntime (see
            # delegation.live_adapters.EngineSessionManagerAdapter.shutdown):
            # aborts every queued open and terminates every owned process
            # with a confirmed/ambiguous release.
            shutdown()
        self._executor.shutdown(wait=False, cancel_futures=True)

    def transport_lost(self, task_id: str) -> bool:
        """Whether this task's host connection closed or this daemon is
        tearing down: interrupted work then stays as persisted for recovery
        instead of being cancelled or recorded as failed."""
        with self._lock:
            return self._shutting_down.is_set() or task_id in self._host_disconnected

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
            self._task_display_emitters.pop(task_id, None)

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
        try:
            session_id = self._engine.open("shared_project_root", launch, instructions)
        except Exception as error:
            # P1 diagnosable launch failure: this is the one place a
            # coordinator engine session actually starts (spawns the child
            # process, e.g. codex-acp/claude-agent-acp) -- a failure here
            # happens *before* orchestration.state.TaskMachine.create ever
            # runs (see orchestration.controller.run, called only once this
            # method has returned), so the task can be durably bound
            # (bootstrap.bootstrap_task already committed a `tasks` row and
            # this method's own caller already committed `task_orchestration`)
            # with no `runtime_tasks` row yet -- exactly the shape a live
            # trace with no such row, but a bound task id, shows. Tagging
            # (never replacing) the exception lets acp.agent's crash
            # diagnostic name the stage/task/attempt without this module
            # importing anything from acp.agent, and without changing the
            # exception's own type (so unrelated callers that inspect it,
            # e.g. by isinstance, are unaffected).
            error.cortex_launch_stage = "engine_launch"
            error.cortex_launch_task_id = task_id
            error.cortex_launch_attempt_id = attempt_ref
            raise
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

    def _resolve_context_window_tokens(self, task_id: str, preview: Mapping[str, Any], session_ref: str | None = None) -> int | None:
        """The route's real context window: the catalog's evidenced value,
        else the window the engine itself reported for this session (or for
        the same connection and route model earlier) on ``usage_update``.
        ``None`` only when neither source ever said."""
        window = self._catalog_context_window_tokens(task_id, preview)
        if window is None and session_ref is not None:
            reported = getattr(self._engine, "context_window", None)
            if reported is not None:
                try:
                    window = reported(session_ref)
                except Exception:
                    window = None
        return window

    def _catalog_context_window_tokens(self, task_id: str, preview: Mapping[str, Any]) -> int | None:
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
        context_window_tokens = self._resolve_context_window_tokens(task_id, preview, session_ref)
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
        if decision.decision == "unverified_window":
            blocked = self._block_generation_delivery(
                task_id, session_ref, generation, capsule["capsule_revision"], "budget_window_unverified",
                "budget_window_unverified", dict(
                    mandatory_tokens=decision.mandatory_tokens,
                    context_window_tokens=decision.context_window_tokens,
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
            usable_window = (
                int(decision.context_window_tokens * token_budget.SAFETY_MARGIN_FRACTION)
                if decision.context_window_basis != token_budget.ESTIMATED_BASIS else None
            )
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
        # Runtime-owned record, no model turn spent on it (see
        # pipeline_bootstrap module docstring). Always called -- never
        # gated on ``not pre_existing`` -- because ``task_orchestration``
        # (what ``pre_existing`` reflects) is committed *before* the
        # coordinator's engine session ever opens (see
        # ``_ensure_coordinator``): an engine-launch failure on a task's
        # very first turn (P1 diagnosable launch failure -- see
        # ``acp.agent._write_crash_diagnostic``) leaves that row committed
        # with no pipeline ever published, so every retry's ``pre_existing``
        # already reads True and a ``not pre_existing`` gate would skip this
        # call forever. ``publish_initial_pipeline``'s own durable check
        # (the task's ``reports`` row, checked first, before any write) is
        # the actual idempotency guarantee and is cheap (one indexed
        # SELECT) -- a second first-turn (recovery, replacement session,
        # this retry case) is always a safe no-op.
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
                dict(kind="question_answered", question_id=answering_question.id, answer_text=answer_text,
                     session_id=session_id),
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
                render_project_docs_catalog(self._task_workspace_root(task_id))
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
        from ..orchestration.controller import run
        return run(self, task_id, session_id, channel, session_ref, content, emit_update,
                   cancel_event, language, host_elicitor, emit_tool_call=emit_tool_call,
                   emit_display_update=emit_display_update, prompt_blocks=prompt_blocks,
                   initial_project_docs_context=initial_project_docs_context)

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
            dict(kind="question_answered", question_id=question.id, answer_text=answer_text, session_id=session_id),
            origin="runtime",
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
            self._host_disconnected.discard(task_id)
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
        *, stop_source: str = "cortex_stop_command",
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
            transitioned = acp_steering.mark_stopped(
                self._store, task_id, source=stop_source, session_id=session_id,
            )
        except LookupError:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="unknown_task",
                remediation="This task id does not exist; there is nothing to stop.",
                retry_eligible=False,
            ) from None
        if not transitioned:
            return False
        from ..orchestration.state import TaskMachine
        TaskMachine(self._store).cancel(task_id)
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
                            result_payload=payload if isinstance(payload, dict) else None,
                        )
                    else:
                        payload = json.loads(terminal_row["payload_json"] or "{}")
                        reason = payload.get("reason") or payload.get("error") or event_type
                        self._emit_worker_card(
                            bound_emitter, assignment_ref, status="failed",
                            entry=_compact_text(f"Cancelled: {reason}" if event_type == "assignment_cancelled"
                                                else f"Not delegated/failed: {reason}",
                                                _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                        )
                continue
            self._cancel_assignment(
                task_id, assignment_ref,
                "Paseo session/cancel" if stop_source == "paseo_session_cancel" else "urgent stop (/cortex-stop)",
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
        # A stopped task cannot resume through any reconnect. Close every ACP
        # host session bound to it, including older sessions left behind by a
        # reconnect, after all task-owned engine cleanup has been requested.
        # This keeps durable host-session state aligned with terminal task
        # state and is idempotent across repeated cleanup.
        acp_sessions.close_task_sessions(self._store, task_id)
        questions_store.supersede_open(self._store, task_id, "task stopped")
        acp_steering.supersede_undelivered_steering(self._store, task_id)
        self._set_state(
            task_id, "stopped", wait_reason="stopped",
            wait_detail=dict(
                explanation=("Stopped by Paseo session/cancel."
                             if stop_source == "paseo_session_cancel" else "Stopped by /cortex-stop."),
                source=stop_source, session_id=session_id,
            ),
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

    def _evict_coordinator_session(self, task_id: str, reason: str) -> None:
        """Forget a coordinator engine session this process can no longer
        prompt (its stdio transport hit EOF / the process is gone).

        Without this, ``_ensure_coordinator`` keeps handing back the same
        dead cached ``session_ref`` on every following turn, so each one
        re-raises the identical ``transport_eof`` and the task stays blocked
        forever. Terminating the owned attempt (releases its capacity) and
        dropping the cached channel/session makes the next turn fall through
        to the reopen/recovery path instead of reusing the corpse.
        """
        self._terminate_coordinator_attempt(task_id)
        with self._lock:
            self._coordinator_channels.pop(task_id, None)
            self._coordinator_sessions.pop(task_id, None)

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
        model = (audit["model"] if audit else None) or parameters.get("model")
        # T035: the routing id (``model``, used for pin/deny) stays the
        # audited column value; the summary prefers ``live_adapters
        # ._resolve_route_model_name``'s real, engine-observed name
        # (``detail["model_name"]``) and otherwise marks the id an alias --
        # never a guessed version. The attempt's own audited effort wins; the
        # revision's parameters are the coordinator's, used only without it.
        detail = {}
        if audit is not None:
            try:
                detail = json.loads(audit["decision_detail_json"] or "{}")
            except Exception:  # noqa: BLE001 - a label never fails a card
                detail = {}
            if not isinstance(detail, dict):
                detail = {}
        if not detail.get("effort"):
            # Each engine names its own effort knob (see configuration.policy).
            for key in ("effort", "reasoning_effort", "thought_level"):
                value = parameters.get(key)
                if value:
                    detail["effort"] = value
                    break
        from ..routing.decision_display import route_decision_summary
        return route_decision_summary(engine, model, detail, compact=True)

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
            if event.event_type in {"delegation_requested", "assignment_ready"} and event.assignment_ref:
                # Recovery can deliver an old informational start event after
                # its result was consumed. Do not resurrect the completed
                # worker in this new host turn; terminal evidence is durable.
                with self._store.connection() as db:
                    terminal = db.execute(
                        "SELECT 1 FROM task_events e WHERE e.task_id=? AND e.superseded=0 "
                        "AND e.event_type IN ('worker_result','assignment_failed','assignment_cancelled') "
                        "AND (e.assignment_ref=? OR e.assignment_ref IN "
                        "(SELECT p.attempt_id FROM delegation_provisioning p "
                        "WHERE p.task_id=? AND p.idempotency_key=?)) LIMIT 1",
                        (task_id, event.assignment_ref, task_id, event.assignment_ref),
                    ).fetchone()
                if terminal is not None:
                    continue
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
                base_title = self._worker_base_title(task_id, assignment_ref)
                if base_title:
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
                lines = [event.payload.get("summary", "")]
                with self._lock:
                    has_final_response = assignment_ref in self._worker_final_response_seen
                status, outcome_label = self._worker_card_outcome(task_id, assignment_ref)
                fields: dict[str, Any] = {"status": status}
                if outcome_label:
                    with self._lock:
                        base_title = getattr(self, "_worker_card_title", {}).get(assignment_ref)
                    if base_title:
                        fields["title"] = _worker_card_title(base_title, outcome_label)
                self._emit_worker_card(
                    emit_tool_call, assignment_ref,
                    entry=((outcome_label + ": " if outcome_label else "") + "Result: " +
                           _compact_text(lines[0], _WORKER_ACTIVITY_MAX_CHARS - 8)
                           if lines[0] and not has_final_response else None),
                    result_payload=event.payload,
                    **fields,
                )
            elif event.event_type in {"assignment_failed", "assignment_cancelled"} and event.assignment_ref \
                    and self._worker_lost_to_host(task_id, event.assignment_ref):
                # Recovery wrote off a worker its lost host left running
                # (recovery_worker_lost), or the replan after it cancelled
                # that assignment: the card stays Interrupted, never a
                # generic failure or cancellation. A retry is a new card.
                self._close_assignment(task_id, event.assignment_ref)
                self._emit_interrupted_worker_card(task_id, emit_tool_call, event.assignment_ref)
            elif event.event_type == "assignment_failed" and event.assignment_ref:
                self._close_assignment(task_id, event.assignment_ref)
                text = f"Not delegated/failed: {event.payload.get('error', 'unknown error')}"
                detail = event.payload.get("detail")
                if detail:
                    text += f" {detail}"
                self._emit_worker_card(
                    emit_tool_call, event.assignment_ref, status="failed",
                    entry=_compact_text(text, _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
            elif event.event_type == "assignment_cancelled" and event.assignment_ref:
                self._close_assignment(task_id, event.assignment_ref)
                text = f"Cancelled: {event.payload.get('reason', '')}"
                fields: dict[str, Any] = {"status": _TOOL_CALL_STATUS_FOR_CANCELLED}
                # ACP's ToolCallStatus has no "cancelled" value (this card is
                # rendered "failed", per the module docstring), so a client
                # with zero live pipeline data cannot otherwise tell a
                # cancelled assignment from a genuine failure. Carry it on
                # the same portable title channel T035 already uses for
                # "Completed with issues" -- see worker-display.ts's
                # `resolveWorkerCardStatus`.
                with self._lock:
                    base_title = getattr(self, "_worker_card_title", {}).get(event.assignment_ref)
                if base_title:
                    fields["title"] = _worker_card_title(base_title, "Cancelled")
                self._emit_worker_card(
                    emit_tool_call, event.assignment_ref,
                    entry=_compact_text(text, _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                    **fields,
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

    def _worker_card_outcome(self, task_id: str, assignment_ref: str) -> tuple[str, str | None]:
        """Read the same runtime node state used by pipeline and worker history."""
        try:
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT status,result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                    (task_id, assignment_ref),
                ).fetchone()
        except (sqlite3.Error, StoreError):
            row = None
        if row is None:
            # A result event is terminal. This compatibility fallback only
            # applies to pre-runtime or already-pruned rows.
            return "completed", None
        return worker_card_outcome(row["status"], row["result_json"])

    def _clamp_attempt_context_size(self, attempt_id: str | None, size: Any) -> int | None:
        """The largest context-window ``size`` reported so far for this
        attempt, or ``None`` when ``size`` is not a valid positive int.

        Some engines (observed: claude-agent-acp) report a smaller
        placeholder context-window size on every mid-turn ``usage_update``
        and only correct it to the model's real size on the turn's final
        ``usage_update`` -- same attempt, same ``used`` count, size jumping
        e.g. 200000 -> 1000000. A context window never actually shrinks
        mid-attempt, so a smaller size reported after a larger one is the
        same stale placeholder, not new evidence; the panel keeps showing
        the largest (most accurate) size seen so far instead of flickering
        back down.
        """
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0 or not attempt_id:
            return None
        model = self._attempt_model(attempt_id)
        learned = self._learned_context_size(model) if model else 0
        with self._lock:
            # Per model too: the engine only corrects the size at the end of a
            # turn, so without this every new worker opened on 200k.
            size = max(size, self._attempt_context_size_max.get(attempt_id, 0), learned)
            self._attempt_context_size_max[attempt_id] = size
            grew = bool(model) and size > learned
            if model:
                self._model_context_size_max[model] = size
        if grew:
            self._save_context_sizes()
        return size

    def _context_sizes_path(self):
        base = os.environ.get("XDG_STATE_HOME") or os.path.join(os.path.expanduser("~"), ".local", "state")
        return Path(base) / "cortex" / "context-windows.json"

    def _learned_context_size(self, model: str) -> int:
        """The largest window ever reported for ``model``: remembered across
        processes (a file) and recovered from this project's history, so a
        worker opens on 1M rather than the engine's mid-turn 200k."""
        # The first load's write-back happens after this lock is released:
        # ``self._lock`` is not reentrant, and calling _save_context_sizes
        # (which takes it again) from inside deadlocked the whole runtime
        # permanently -- the worker thread that first learned a size never
        # released the lock, so the controller loop (blocked in
        # _terminate_completed_workers) never observed the state change that
        # would have shown the user the worker's question, and the ACP reader
        # thread (blocked in cancel) stopped handling any client message at
        # all. The session simply stopped, mid-turn, with nothing shown
        # (live, 2026-09-22).
        pending: dict[str, int] | None = None
        with self._lock:
            if not getattr(self, "_context_sizes_loaded", False):
                self._context_sizes_loaded = True
                known = {}
                try:
                    known.update(json.loads(self._context_sizes_path().read_text()))
                except (OSError, ValueError):
                    pass
                try:
                    with self._store.connection() as db:
                        for row in db.execute(
                                "SELECT a.engine||'/'||a.model AS m, MAX(CAST(json_extract(e.payload_json,'$.content.size') AS INTEGER)) AS s "
                                "FROM task_events e JOIN attempt_route_audit a ON a.attempt_id=json_extract(e.payload_json,'$.attempt_id') "
                                "WHERE e.event_type='progress' AND json_extract(e.payload_json,'$.activity_ref')='context' GROUP BY m"):
                            if isinstance(row["s"], int):
                                known[row["m"]] = max(row["s"], known.get(row["m"], 0))
                except Exception:  # noqa: BLE001 - display nicety only
                    pass
                for name, value in known.items():
                    if isinstance(value, int) and value > self._model_context_size_max.get(name, 0):
                        self._model_context_size_max[name] = value
                if known:
                    pending = dict(self._model_context_size_max)
            size = self._model_context_size_max.get(model, 0)
        if pending is not None:
            self._write_context_sizes(pending)
        return size

    def _save_context_sizes(self) -> None:
        with self._lock:
            data = dict(self._model_context_size_max)
        self._write_context_sizes(data)

    def _write_context_sizes(self, data: dict) -> None:
        """Write an already-taken snapshot. Separate from
        :meth:`_save_context_sizes` so a caller that already holds
        ``self._lock`` can write without retaking it (see
        :meth:`_learned_context_size`), and so this file I/O never happens
        under the lock at all."""
        path = self._context_sizes_path()
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            temporary = path.with_suffix(".tmp")
            temporary.write_text(json.dumps(data, sort_keys=True))
            os.replace(temporary, path)
        except OSError:
            pass

    def _attempt_model(self, attempt_id: str) -> str | None:
        """``engine/model`` this attempt was routed to (cached)."""
        with self._lock:
            if attempt_id in self._attempt_models:
                return self._attempt_models[attempt_id]
        try:
            with self._store.connection() as db:
                row = db.execute("SELECT engine, model FROM attempt_route_audit WHERE attempt_id=? ORDER BY rowid DESC LIMIT 1",
                                 (attempt_id,)).fetchone()
        except Exception:  # noqa: BLE001 - display nicety only
            return None
        model = f"{row['engine']}/{row['model']}" if row is not None else None
        with self._lock:
            self._attempt_models[attempt_id] = model
        return model

    def _append_worker_entry(self, assignment_ref: str, entry: str, ref: str | None = None) -> None:
        """Add one worker event to the rolling tail, replacing the prior line
        for the same tool.

        Each entry is scrubbed once here, as one bounded line, and the tail
        keeps the latest ``_WORKER_CARD_TAIL_ENTRIES``: runtime memory and
        per-update work are O(1) in the worker's length.
        """
        with self._lock:
            project_root = getattr(self, "_worker_card_project_root", {}).get(assignment_ref)
        line = _worker_card_line(entry, project_root)
        if not line:
            return
        with self._lock:
            log = self._worker_action_log.setdefault(assignment_ref, [])
            refs = self._worker_action_refs.setdefault(assignment_ref, {})
            dropped_by_card = getattr(self, "_worker_action_dropped", None)
            if dropped_by_card is None:
                dropped_by_card = self._worker_action_dropped = {}
            evicted_by_card = getattr(self, "_worker_action_evicted_refs", None)
            if evicted_by_card is None:
                evicted_by_card = self._worker_action_evicted_refs = {}
            dropped = dropped_by_card.get(assignment_ref, 0)
            evicted = evicted_by_card.setdefault(assignment_ref, [])
            if ref:
                position = refs.get(ref)
                if position is not None and 0 <= position - dropped < len(log):
                    log[position - dropped] = line
                    return
                if ref in evicted:
                    return
                refs[ref] = dropped + len(log)
                # Distinct refs are distinct logical calls even when their
                # compact text happens to be identical.
                log.append(line)
            elif not log or log[-1] != line:
                log.append(line)
            else:
                return
            overflow = len(log) - _WORKER_CARD_TAIL_ENTRIES
            if overflow > 0:
                del log[:overflow]
                dropped += overflow
                dropped_by_card[assignment_ref] = dropped
                for stale in [key for key, position in refs.items() if position < dropped]:
                    del refs[stale]
                    evicted.append(stale)
                del evicted[:-_WORKER_ACTION_EVICTED_REFS_MAX]

    def _record_worker_activity(
        self, task_id: str, assignment_ref: str, attempt_id: str | None,
        activity_ref: str, text: str, *, status: str | None = None,
        input_value: Any = None, output_value: Any = None, error_value: Any = None,
        content_value: Any = None, kind: str | None = None,
        title: str | None = None, locations_value: Any = None,
        stream_sequence: int | None = None, provider_event_id: str | None = None,
        message_id: str | None = None,
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
        # A worker's prose is delivered as one complete activity block, but a
        # tool call is a real message boundary even when the tool itself is
        # hidden from the worker panel (Cortex bookkeeping, provider wait,
        # etc.).  Persist that boundary identity so a replay can group only
        # chunks from the same logical message.
        if message_id is not None:
            payload["message_id"] = str(message_id)
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

    # Match one worker card's rows inside SQLite: an exact substring
    # prefilter (the journal stores compact sorted JSON) and then an exact
    # JSON comparison, so no whole update row is loaded into Python.
    _DURABLE_CARD_ROWS = (
        "FROM host_updates h JOIN acp_host_sessions s ON s.session_id=h.session_id "
        "WHERE s.task_id=? AND instr(h.update_json, ?) > 0 "
        "AND CASE WHEN json_valid(h.update_json) "
        "THEN json_extract(h.update_json, '$.toolCallId') END=? "
    )

    def _durable_worker_card_sessions(self, task_id: str, assignment_ref: str) -> dict[str, str]:
        """Return each session's latest durable status for this exact card."""
        card_id = _worker_tool_call_id(assignment_ref)
        needle = json.dumps(card_id, ensure_ascii=False)
        try:
            with self._store.connection() as db:
                rows = db.execute(
                    "SELECT h.session_id AS session_id, MAX(h.seq) AS seq, "
                    "json_extract(h.update_json, '$.status') AS status "
                    + self._DURABLE_CARD_ROWS
                    + "AND json_type(h.update_json, '$.status')='text' GROUP BY h.session_id",
                    (task_id, needle, card_id),
                ).fetchall()
        except (sqlite3.Error, StoreError):
            return {}
        return {row["session_id"]: row["status"] for row in rows}

    def _worker_lost_to_host(self, task_id: str, assignment_ref: str) -> bool:
        """Whether recovery wrote this assignment off as lost to its host."""
        from ..recovery.worker_reconciliation import RECOVERY_WORKER_LOST

        store = getattr(self, "_store", None)
        if store is None:
            return False
        try:
            with store.connection() as db:
                return db.execute(
                    "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND superseded=0 "
                    "AND event_type='assignment_failed' AND json_valid(payload_json) "
                    "AND json_extract(payload_json, '$.error')=? LIMIT 1",
                    (task_id, assignment_ref, RECOVERY_WORKER_LOST),
                ).fetchone() is not None
        except (sqlite3.Error, StoreError):
            return False

    def _worker_base_title(self, task_id: str, assignment_ref: str) -> str | None:
        """``Worker: role · route — title`` from durable delegation state."""
        role_id = self._delegation_provisioning_role(task_id, assignment_ref)
        if not role_id:
            return None
        requested = self._delegation_requested_payload(task_id, assignment_ref)
        if requested is None:
            return None
        # Requirement: the title names the *resolved* role id, not the raw
        # hint the coordinator typed -- assignment_ready is the first point
        # a resolved id exists.
        route = self._delegation_route_label(task_id, assignment_ref)
        who = _display_role(role_id) + (f" · {route}" if route else "")
        return f"Worker: {who} — {requested.get('title', '')}"

    def _durable_worker_card(self, task_id: str, assignment_ref: str) -> tuple[bool, str | None]:
        """Whether the host already has this card, and its latest title
        without any status suffix. A replacement process never cached
        either in memory."""
        card_id = _worker_tool_call_id(assignment_ref)
        needle = json.dumps(card_id, ensure_ascii=False)
        try:
            with self._store.connection() as db:
                seen = db.execute(
                    "SELECT 1 " + self._DURABLE_CARD_ROWS + "LIMIT 1",
                    (task_id, needle, card_id),
                ).fetchone() is not None
                row = db.execute(
                    "SELECT json_extract(h.update_json, '$.title') AS title "
                    + self._DURABLE_CARD_ROWS
                    + "AND json_type(h.update_json, '$.title')='text' "
                    "AND trim(json_extract(h.update_json, '$.title'))<>'' "
                    "ORDER BY h.seq DESC LIMIT 1",
                    (task_id, needle, card_id),
                ).fetchone() if seen else None
        except (sqlite3.Error, StoreError):
            return False, None
        title = row["title"] if row is not None else None
        return seen, (_WORKER_STATUS_SUFFIX.sub("", title) if title else None)

    def _emit_interrupted_worker_card(self, task_id: str, emit_tool_call, assignment_ref: str) -> None:
        from . import replay_store

        seen, durable_title = self._durable_worker_card(task_id, assignment_ref)
        card_id = _worker_tool_call_id(assignment_ref)
        with self._lock:
            base_title = getattr(self, "_worker_card_title", {}).get(assignment_ref)
            emitted_here = card_id in self._emitted_worker_cards
        if seen and not emitted_here:
            # The lost process created this card and the host still shows
            # it. Paseo 0.9.2 renders any update for it from this later turn
            # as a second row (observed live, even as tool_call_update); its
            # own row already settles to Interrupted from the durable attempt
            # status, and session/load replays the same correction for other
            # clients (replay_store.terminal_worker_card_reconciliation).
            return
        base_title = base_title or durable_title or self._worker_base_title(task_id, assignment_ref)
        fields: dict[str, Any] = {"status": "failed"}
        if base_title:
            fields["title"] = _worker_card_title(base_title, "Interrupted")
        self._emit_worker_card(
            emit_tool_call, assignment_ref,
            entry=replay_store.interrupted_notice_text(self._store, task_id), **fields,
        )

    def _persist_terminal_worker_card(self, session_id: str, assignment_ref: str, reason: str) -> None:
        """Persist a terminal update to the card's owning session only."""
        from . import replay_store

        replay_store.append_host_update(
            self._store, session_id,
            dict(
                sessionUpdate="tool_call_update", toolCallId=_worker_tool_call_id(assignment_ref),
                status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                content=[dict(type="text", text=_compact_text(f"Cancelled: {reason}", _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS))],
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
        result_payload = fields.pop("result_payload", None)
        display_project_root = fields.pop("_project_root", None)
        if display_project_root:
            with self._lock:
                roots = getattr(self, "_worker_card_project_root", None)
                if roots is None:
                    roots = self._worker_card_project_root = {}
                roots[assignment_ref] = str(display_project_root)
        # A worker can finish its first engine turn before the coordinator's
        # event-claim path observes ``assignment_ready``. Re-issue the
        # resolved assignment title on that first running/terminal update so
        # the client never receives an untitled lifecycle row merely because
        # the worker callback won the race.
        if incoming_status in {"in_progress", "completed", "failed"} and "title" not in fields:
            with self._lock:
                resolved_title = getattr(self, "_worker_card_title", {}).get(assignment_ref)
            if resolved_title:
                fields["title"] = resolved_title
        if fields.get("title"):
            fields["title"] = _worker_card_title(fields["title"])
        if entry:
            self._append_worker_entry(assignment_ref, entry, entry_ref)
        # The card body is not a caller's to set, and no transcript mirror is
        # ever sent: the pre-fix card re-sent the whole growing transcript
        # (twice, as content and rawInput) on every tool event and streamed
        # word, which a host that retains every event could not survive.
        for key in ("text", "raw_input", "content"):
            fields.pop(key, None)
        panel = self._worker_card_is_panel(emit_tool_call, assignment_ref)
        with self._lock:
            tail = list(self._worker_action_log.get(assignment_ref) or ())
            earlier = getattr(self, "_worker_action_dropped", {}).get(assignment_ref, 0)
            card_root = getattr(self, "_worker_card_project_root", {}).get(assignment_ref)
        if not panel:
            body = _worker_card_tail_text(tail, earlier)
            if body:
                fields["text"] = body
        # Paseo validates a failed timeline item with a non-null ``error``,
        # which its ACP reducer takes from rawOutput: one short, redacted
        # failure line from this card, never a transcript.
        if incoming_status == "failed":
            failure = _worker_card_line(entry, card_root) if entry else (tail[-1] if tail else "")
            if failure:
                fields["raw_output"] = clip_text(failure, _WORKER_CARD_FAILURE_MAX_BYTES)
        # Embed the worker's terminal result projection (bounded summary +
        # checks) directly on the completed card's rawOutput, so a client
        # that persists/replays this ACP update can render the result
        # section with zero follow-up RPC. Only ever set for "completed"
        # (a bare "failed" card owns raw_output as its failure line above,
        # and worker_card_outcome only maps to "failed" when there is no
        # result at all).
        if incoming_status == "completed" and isinstance(result_payload, Mapping):
            fields["raw_output"] = _bounded_worker_result(
                _client_safe_raw(worker_result_projection(result_payload)),
            )
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
        # ACP updates are partial. Compare the effective rendered state
        # (per-field digests), rather than each update's subset of fields:
        # reattaching an unchanged title after a tool event must not redraw
        # the card.  Then apply the card's event budget: a status transition
        # is sent at once; anything else is a progress update, limited in
        # rate and in number (see _WORKER_CARD_PROGRESS_*).  Digests advance
        # only for what is sent, so a held-back change rides the next
        # permitted update and always the terminal transition.
        clock = getattr(self, "_worker_card_clock", None) or time.monotonic
        with self._lock:
            sent = self._worker_card_sent.get(card_id) or {}
            digests = dict(sent.get("fields") or {})
            changed = {
                key for key, value in payload.items()
                if key not in {"new", "tool_call_id"} and digests.get(key) != _field_digest(value)
            }
            rank = _WORKER_CARD_STATUS_RANK.get(str(payload.get("status")), 0)
            sent_rank = int(sent.get("rank") or 0)
            if not first and "status" in changed and rank <= sent_rank:
                # Only forward lifecycle moves are transitions; a stale
                # earlier status (a late "pending") never regresses the card.
                changed.discard("status")
                payload.pop("status", None)
                rank = sent_rank
            if not first and not changed:
                return
            now = clock()
            progress = int(sent.get("progress") or 0)
            revisions = int(sent.get("terminal_revisions") or 0)
            if not first and "status" not in changed:
                if incoming_status in {"completed", "failed"}:
                    # A later path refining the settled card (its outcome
                    # label or result) is not deferred: nothing would flush
                    # it after the worker is done.
                    if revisions >= _WORKER_CARD_TERMINAL_REVISIONS:
                        return
                    revisions += 1
                else:
                    progress_at = sent.get("progress_at")
                    if progress >= _WORKER_CARD_PROGRESS_MAX_UPDATES:
                        return
                    if (progress_at is not None
                            and now - progress_at < _WORKER_CARD_PROGRESS_MIN_INTERVAL_SECONDS):
                        return
                    progress += 1
                    sent = {**sent, "progress_at": now}
            for key in changed:
                digests[key] = _field_digest(payload[key])
            self._worker_card_sent[card_id] = {
                "fields": digests,
                "progress_at": sent.get("progress_at"),
                "progress": progress,
                "terminal_revisions": revisions,
                "rank": max(rank, sent_rank),
            }
        if not first:
            # Clients merge partial updates; an unchanged title (the longest
            # field of a Paseo card) is not sent again with every status.
            payload = {key: value for key, value in payload.items()
                       if key in changed or key in {"new", "tool_call_id"}}
        emit_tool_call(payload)

    def _mirror_durable_worker_result_card(self, task_id: str, assignment_ref: str) -> None:
        """Settle a worker card from its durable ``worker_result``, if any.

        A no-op when the card already reached a terminal status or no result
        was published.
        """
        with self._lock:
            if _worker_tool_call_id(assignment_ref) in self._worker_card_terminal:
                return
        try:
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
        except (sqlite3.Error, StoreError):
            return
        if result_row is None:
            return
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
            result_payload=result_payload if isinstance(result_payload, dict) else None,
        )

    def _worker_card_is_panel(self, emit_tool_call, assignment_ref: str) -> bool:
        """Whether this card renders on Paseo (title/status/result only).

        The surface bound at card creation wins; a card emitted before any
        binding (e.g. a recovery path) uses the emitting connection's own
        client surface when the host adapter exposes it.
        """
        with self._lock:
            bound = getattr(self, "_worker_card_panel_mode", {}).get(assignment_ref)
        if bound is not None:
            return bool(bound)
        return _is_panel_surface(getattr(emit_tool_call, "client_surface", None))


    def _coordinator_card_budget(self, task_id: str | None) -> _CoordinatorCardBudget:
        """The task's shared card budget, seeded once from its host journal."""
        if task_id is None:
            return _CoordinatorCardBudget()
        with self._lock:
            budget = self._coordinator_card_budgets.get(task_id)
        if budget is not None:
            return budget
        seeded = self._seed_coordinator_card_budget(task_id)
        with self._lock:
            return self._coordinator_card_budgets.setdefault(task_id, seeded)

    def _seed_coordinator_card_budget(self, task_id: str) -> _CoordinatorCardBudget:
        """Count what earlier runtimes already sent for this task.

        The journal stores compact sorted JSON, so an exact substring
        prefilter selects candidate rows inside SQLite before any JSON is
        parsed; no update row is loaded into Python.  An unreadable journal
        starts from zero (the budget then bounds this runtime's lifetime).
        """
        needle = json.dumps(acp_wire_budget.COORDINATOR_CARD_PREFIX)[:-1]
        folded_id = acp_wire_budget.FOLDED_CARD_PREFIX + task_id
        rows = "FROM host_updates h JOIN acp_host_sessions s ON s.session_id=h.session_id WHERE s.task_id=? "
        try:
            with self._store.connection() as db:
                distinct = db.execute(
                    "SELECT COUNT(DISTINCT json_extract(h.update_json, '$.toolCallId')) " + rows
                    + "AND instr(h.update_json, '\"sessionUpdate\":\"tool_call\",') > 0 "
                    "AND instr(h.update_json, ?) > 0 AND json_valid(h.update_json) "
                    "AND json_extract(h.update_json, '$.sessionUpdate')='tool_call' "
                    "AND substr(json_extract(h.update_json, '$.toolCallId'), 1, ?)=?",
                    (task_id, '"toolCallId":' + needle, len(acp_wire_budget.COORDINATOR_CARD_PREFIX),
                     acp_wire_budget.COORDINATOR_CARD_PREFIX),
                ).fetchone()[0] or 0
                shown_bytes = db.execute(
                    "SELECT COALESCE(SUM(LENGTH(CAST(h.update_json AS BLOB))), 0) " + rows
                    + "AND instr(h.update_json, ?) > 0 AND json_valid(h.update_json) "
                    "AND json_extract(h.update_json, '$.sessionUpdate') IN ('tool_call', 'tool_call_update') "
                    "AND substr(json_extract(h.update_json, '$.toolCallId'), 1, ?)=? "
                    "AND json_extract(h.update_json, '$.toolCallId') != ?",
                    (task_id, '"toolCallId":' + needle, len(acp_wire_budget.COORDINATOR_CARD_PREFIX),
                     acp_wire_budget.COORDINATOR_CARD_PREFIX, folded_id),
                ).fetchone()[0] or 0
                folded = db.execute(
                    "SELECT COUNT(*) AS updates, "
                    "MAX(json_extract(h.update_json, '$.rawOutput.furtherToolCalls')) AS further, "
                    "MAX(json_extract(h.update_json, '$.rawOutput.failed')) AS failed " + rows
                    + "AND instr(h.update_json, ?) > 0 AND json_valid(h.update_json) "
                    "AND json_extract(h.update_json, '$.toolCallId')=?",
                    (task_id, json.dumps(folded_id), folded_id),
                ).fetchone()
                sessions = [row[0] for row in db.execute(
                    "SELECT DISTINCT h.session_id " + rows
                    + "AND instr(h.update_json, ?) > 0 AND json_valid(h.update_json) "
                    "AND json_extract(h.update_json, '$.toolCallId')=? "
                    "AND json_extract(h.update_json, '$.sessionUpdate')='tool_call'",
                    (task_id, json.dumps(folded_id), folded_id),
                )]
        except (sqlite3.Error, StoreError):
            return _CoordinatorCardBudget()
        budget = acp_wire_budget.COORDINATOR_CARD_BUDGET
        return _CoordinatorCardBudget(
            shown=min(distinct, budget),
            errors_shown=min(max(0, distinct - budget), acp_wire_budget.COORDINATOR_ERROR_CARD_BUDGET),
            shown_bytes=int(shown_bytes),
            folded=int(folded["further"] or 0), folded_failed=int(folded["failed"] or 0),
            folded_updates=int(folded["updates"] or 0), folded_sessions=sessions,
        )

    def _engine_tool_call_emitter(
        self, emit_tool_call, *, prefix="", id_scope="coordinator", language="en",
        project_root=None, task_id=None, cancel_event=None,
    ):
        """Mirror observed tool lifecycles within the task's total card budget.

        The on-wire rawInput, rawOutput and each content block are capped at
        ``_COORDINATOR_CARD_FIELD_MAX_BYTES`` with an explicit truncation
        marker; the host retains every event, and the engine session and
        task history hold the full values.

        The host keeps every card it is sent, so nothing is sent twice: an
        update carries only the fields that changed since this card's last
        update (ACP updates are partial and clients merge them), an MCP
        result's text copy of its ``structuredContent`` is dropped, and a
        completed call whose tool, input and output repeat an earlier call
        exactly (a coordinator polling ``task_get_context``) references that
        card instead of carrying the same output again.

        Nor does the number of cards grow with the task: the task's first
        ``COORDINATOR_CARD_BUDGET`` distinct calls get their own card, later
        ones are counted on one aggregate card (``wire_budget``) that is
        updated at most once per interval, a fixed number of times per task,
        and once more when the turn ends (``close_group``).  A failed call
        past the budget still gets its own card while the error budget
        lasts.  Permission requests and user questions are separate host
        requests and never pass through here.
        """
        if emit_tool_call is None:
            return None
        budget = self._coordinator_card_budget(task_id)
        cap = _COORDINATOR_CARD_FIELD_MAX_BYTES
        folded_id = acp_wire_budget.FOLDED_CARD_PREFIX + (task_id or id_scope)
        host_session = getattr(emit_tool_call, "session_id", None)
        clock = getattr(self, "_coordinator_card_clock", None) or time.monotonic

        def emit_folded(*, turn_end: bool) -> None:
            # Caller holds budget.lock.
            if budget.folded_updates >= acp_wire_budget.COORDINATOR_FOLDED_MAX_UPDATES:
                return
            new = host_session not in budget.folded_sessions
            counts = (budget.folded, budget.folded_failed)
            if not new and counts == budget.folded_sent:
                return
            now = clock()
            # The interval doubles after every update, so the fixed number
            # of updates keeps counting through a long task.
            interval = (acp_wire_budget.COORDINATOR_FOLDED_MIN_INTERVAL_SECONDS
                        * 2 ** min(max(0, budget.folded_updates - 1), 16))
            if (not new and not turn_end and budget.folded_last_at is not None
                    and now - budget.folded_last_at < interval):
                return
            final = budget.folded_updates + 1 >= acp_wire_budget.COORDINATOR_FOLDED_MAX_UPDATES
            card = acp_wire_budget.folded_card_fields(*counts, final=final)
            fields = dict(tool_call_id=folded_id, new=new, status="completed",
                          title=card["title"], raw_output=card["raw_output"])
            if new:
                fields["kind"] = "other"
            budget.folded_updates += 1
            budget.folded_last_at = now
            budget.folded_sent = counts
            budget.folded_sessions.add(host_session)
            emit_tool_call(fields)

        def on_tool_call(ref, kind, title, status, locations=None, content=None,
                         raw_input=None, raw_output=None):
            tool_call_id = f"call:{id_scope}:{ref}"
            acp_status = _acp_tool_call_status(status) if status is not None else None
            with budget.lock:
                entry = budget.seen.get(tool_call_id)
                new = entry is None
                if new:
                    entry = (dict(updates=0, terminal=False, held={}) if budget.admit(acp_status)
                             else dict(kind=None, title=None, failed=False))
                if "updates" not in entry:
                    entry["kind"] = kind if kind is not None else entry["kind"]
                    entry["title"] = (str(title)[:_WORKER_CARD_TITLE_MAX_CHARS]
                                      if title is not None else entry["title"])
                    if (acp_status == "failed"
                            and budget.errors_shown < acp_wire_budget.COORDINATOR_ERROR_CARD_BUDGET):
                        # A failure is shown on its own card: create it now,
                        # complete, and stop counting it as folded.
                        budget.errors_shown += 1
                        budget.folded -= 1
                        if entry["failed"]:
                            budget.folded_failed -= 1
                        new, kind, title = True, entry["kind"], entry["title"]
                        entry = dict(updates=0, terminal=False, held={})
                    else:
                        if acp_status == "failed" and not entry["failed"]:
                            entry["failed"] = True
                            budget.folded_failed += 1
                        budget.remember(budget.seen, tool_call_id, entry)
                        emit_folded(turn_end=False)
                        return
                budget.remember(budget.seen, tool_call_id, entry)
            fields = dict(tool_call_id=tool_call_id, new=new)
            if kind is not None or fields["new"]:
                fields["kind"] = _acp_tool_call_kind(kind)
            if acp_status is not None:
                fields["status"] = acp_status
            if title is not None or fields["new"]:
                label = str(title or f"Tool call {ref}")
                resolved_prefix = prefix() if callable(prefix) else prefix
                fields["title"] = scrub_display_text(
                    f"{resolved_prefix}{label}", max_chars=max(4000, len(label) * 2 + 1000))
            if content is not None:
                safe_content = _client_safe_content(content)
                fields["content"] = (
                    [bound_value(block, cap) for block in safe_content]
                    if isinstance(safe_content, list) else safe_content
                )
            if raw_input is not None:
                fields["raw_input"] = bound_value(_client_safe_raw(raw_input), cap)
            if raw_output is not None:
                fields["raw_output"] = bound_value(
                    _without_mcp_text_mirror(_client_safe_raw(raw_output)), cap)
            if locations is not None:
                fields["locations"] = bound_value(_client_safe_raw(locations), cap)
            with budget.lock:
                terminal = fields.get("status") in {"completed", "failed"}
                limit = acp_wire_budget.COORDINATOR_CARD_MAX_UPDATES
                first_terminal = terminal and not entry["terminal"]
                if entry["updates"] >= limit or (entry["updates"] >= limit - 1 and not first_terminal):
                    # Card update budget spent: the last slot is kept for the
                    # first terminal update, which carries the latest held
                    # change; nothing is sent past the cap.
                    if not entry["terminal"]:
                        entry["held"].update(
                            (key, value) for key, value in fields.items() if key not in {"tool_call_id", "new"})
                    return
                if first_terminal and entry["held"]:
                    fields = {**entry["held"], **fields}
                    entry["held"] = {}
                card = budget.sent.get(tool_call_id)
                if card is None:
                    card = {}
                budget.remember(budget.sent, tool_call_id, card)
                for key in ("title", "raw_input"):
                    if key in fields:
                        card["identity:" + key] = _field_digest(fields[key])
                outputs = budget.outputs
                if "raw_output" in fields and fields.get("status") == "completed":
                    identity = _field_digest([card.get("identity:title"), card.get("identity:raw_input")])
                    digest = _field_digest(fields["raw_output"])
                    earlier = outputs.pop(identity, None)
                    if earlier is not None and earlier[0] == digest and earlier[1] != tool_call_id:
                        fields["raw_output"] = dict(
                            repeated_output=True, same_as=earlier[1],
                            note="Same output as the earlier identical call; full result in task history.",
                        )
                        outputs[identity] = earlier
                    else:
                        outputs[identity] = (digest, tool_call_id)
                    while len(outputs) > _COORDINATOR_REPEAT_MEMORY:
                        outputs.pop(next(iter(outputs)))
                if not fields["new"]:
                    for key in [key for key in fields if key not in {"tool_call_id", "new"}]:
                        if card.get(key) == _field_digest(fields[key]):
                            del fields[key]
                    if len(fields) == 2:
                        if terminal:
                            budget.sent.pop(tool_call_id, None)
                        return
                for key, value in fields.items():
                    if key not in {"tool_call_id", "new"}:
                        card[key] = _field_digest(value)
                if terminal:
                    entry["terminal"] = True
                    budget.sent.pop(tool_call_id, None)
                entry["updates"] += 1
                budget.shown_bytes += encoded_size(fields)
                emit_tool_call(fields)

        def close_group():
            """Turn end: send the aggregate card's latest count, if it changed."""
            with budget.lock:
                if budget.folded or budget.folded_failed:
                    emit_folded(turn_end=True)

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
        for assignment_ref, latest in activity.items():
            latest = " ".join(latest.split())
            if not latest:
                continue
            fields: dict[str, Any] = dict(status="in_progress")
            # The title (names the worker and its assignment) is resolved and
            # normalized by _emit_worker_card itself; a second spelling of it
            # here would be a change on every tick.
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
        question = questions_store.get(self._store, decision['question_id'])
        if question is None or question.task_id != task_id:
            raise StoreError('runtime_question_missing')
        decision["question_id"] = question.id
        if question.status != "open":
            # The terminal decision was inspected before this row was
            # answered, so create_if_absent can observe the answer winning
            # the race. Continue so the caller re-checks the durable state;
            # otherwise it would incorrectly park an answered question in
            # waiting_for_user. Cancellation/supersession remains terminal.
            return question.status == "answered"
        shown_body = False
        if elicitor is not None:
            if hasattr(elicitor, "set_question_id"):
                elicitor.set_question_id(question.id)
            card_text = decision["question"]
            split = getattr(elicitor, "split_question", None)
            if split is not None:
                # A card shows plain text only: long/markdown content goes to
                # the chat (rendered), the card keeps the short question.
                body, title = split(card_text)
                if body:
                    # Own paragraph: chunks of one message are concatenated.
                    shown_body = bool(emit_update(f"\n\n{body}\n\n"))
                    card_text = title or acp_messages.render("question_card_title", language)
            try:
                answer_text = elicitor.elicit(session_id, card_text, decision.get("options"))
            except Exception:
                answer_text = None
            if answer_text is not None:
                if questions_store.answer(self._store, question.id, answer_text, "elicitation"):
                    delegation_events.append_event(
                        self._store, task_id, "progress", f"question_answered:{question.id}",
                        dict(kind="question_answered", question_id=question.id, answer_text=answer_text,
                             session_id=session_id),
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
        if shown_body:
            # The body is already in the chat; repeat only the choices.
            decision = dict(decision, question=acp_messages.render("question_card_title", language))
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




    # -- waiting --------------------------------------------------------


    def _notify(self) -> None:
        with self._cond:
            self._cond.notify_all()

    def _record_coordinator_dispatch_failure(
        self, task_id: str, error: BaseException, *, phase: str,
        assignment_ref: str | None = None,
    ) -> dict[str, str]:
        """Persist a dispatch failure instead of abandoning a claimed request.

        The early dispatcher runs in a daemon thread, so an exception there
        cannot reach the coordinator's call stack.  Keep both the bounded
        diagnostic and, when an exact assignment is known, a terminal
        ``assignment_failed`` event.  The latter closes the assignment's
        durable contract; the former preserves the dispatch phase/type even
        when the failure happened before an executor future existed.
        """
        error_type = type(error).__name__
        detail = _bounded_detail(str(error) or error_type)
        ref_suffix = assignment_ref or new_id("dispatch_")
        payload = dict(
            kind="coordinator_dispatch_failed", phase=phase,
            error=error_type, detail=detail,
        )
        delegation_events.append_event(
            self._store, task_id, "progress",
            f"coordinator_dispatch_failed:{phase}:{ref_suffix}", payload,
            origin="runtime", assignment_ref=assignment_ref,
        )
        if assignment_ref:
            self._append_assignment_failure_if_unreported(
                task_id, assignment_ref,
                dict(error=f"dispatch_failed:{error_type}", detail=detail),
            )
        return dict(error=error_type, detail=detail, phase=phase)

    # -- delegation_requested / assignment_cancel_requested processing --

    def _process_new_events_safely(
        self, task_id: str, coordinator_channel: ChannelContext,
        cancel_event: threading.Event,
        emit_tool_call: Callable[[Mapping[str, Any]], bool] | None = None,
        *, phase: str,
    ) -> None:
        """Run event dispatch without hiding a coordinator-side failure."""
        try:
            self._process_new_events(task_id, coordinator_channel, cancel_event, emit_tool_call)
        except Exception as error:  # noqa: BLE001 - durable diagnostic is the recovery contract
            self._record_coordinator_dispatch_failure(task_id, error, phase=phase)

    def _process_new_events(
        self, task_id, coordinator_channel, cancel_event, emit_tool_call=None,
    ):
        """Execute only reservations created by the runtime scheduler."""
        with self._store.connection() as db:
            requests = db.execute(
                "SELECT e.* FROM task_events e "
                "JOIN runtime_nodes n ON n.task_id=e.task_id AND n.assignment_ref=e.assignment_ref "
                "JOIN runtime_tasks t ON t.task_id=n.task_id AND t.revision=n.revision "
                "WHERE e.task_id=? AND e.event_type='delegation_requested' AND e.superseded=0 "
                "AND n.status='running' AND t.state IN ('EXECUTING','REVIEWING') "
                # A restarted runtime has no in-memory dispatch record. An
                # assignment recovery already closed (its worker was lost
                # with the old process) waits for retry_lost, never a
                # second dispatch of the same request.
                "AND NOT EXISTS (SELECT 1 FROM task_events f WHERE f.task_id=e.task_id "
                "AND f.assignment_ref=e.assignment_ref AND f.event_type IN ('assignment_failed','assignment_cancelled')) "
                "ORDER BY e.id",
                (task_id,),
            ).fetchall()
        to_delegate = []
        with self._lock:
            for row in requests:
                assignment_ref = row["assignment_ref"]
                if assignment_ref in self._dispatched_delegations:
                    continue
                self._dispatched_delegations.add(assignment_ref)
                to_delegate.append((assignment_ref, json.loads(row["payload_json"])))

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
            try:
                future = self._executor.submit(
                    self._provision_and_prompt, task_id, coordinator_channel, payload, cancel_event,
                )
            except Exception as error:  # noqa: BLE001 - convert submit failure into durable assignment state
                with self._lock:
                    open_refs = self._open_assignments.get(task_id)
                    if open_refs is not None:
                        open_refs.discard(assignment_ref)
                        if not open_refs:
                            self._open_assignments.pop(task_id, None)
                if self._shutting_down.is_set():
                    # The executor refuses work during teardown; the request
                    # stays undispatched for the restarted runtime.
                    continue
                self._record_coordinator_dispatch_failure(
                    task_id, error, phase="executor_submit", assignment_ref=assignment_ref,
                )
                continue
            with self._lock:
                self._running.setdefault(task_id, set()).add(future)
            future.add_done_callback(
                lambda f, t=task_id, a=assignment_ref: self._on_worker_done(t, f, a)
            )



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
            in_flight = set(self._worker_turns_in_flight)
            with self._store.connection() as db:
                attempt_ids = {a for a in attempt_ids if not (row := db.execute(
                    'SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?',(a,)
                ).fetchone()) or row[0] not in in_flight}
            newly_terminal = [a for a in attempt_ids if a not in self._terminated_attempts]
            self._terminated_attempts.update(newly_terminal)
        for attempt_id in newly_terminal:
            try:
                self._engine.terminate_owned(attempt_id)
                from ..orchestration.recovery import release_observed
                if not release_observed(self,task_id,attempt_id):
                    with self._lock:
                        self._terminated_attempts.discard(attempt_id)
            except Exception:
                with self._lock:
                    self._terminated_attempts.discard(attempt_id)
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

    def _task_workspace_root(self, task_id: str) -> str:
        """The tree this task's files live in (its session's worktree, else
        the project); the store stays at ``_task_project_root``."""
        from ..task_workspace import store_task_root
        return store_task_root(self._store, task_id)





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
            # Likewise an attempt already failed stays failed with its first
            # reason: an interrupted turn cancels a worker, and the RECOVERING
            # pass that follows cancels the same assignment again.
            with self._store.connection() as db:
                attempt_row = db.execute(
                    "SELECT status FROM work_assignment_attempts WHERE attempt_id=?",
                    (work_attempt_ref,),
                ).fetchone()
            if attempt_row is not None and attempt_row["status"] in ("reported", "failed", "needs_revision"):
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


    def _on_worker_done(
        self, task_id: str, future: Future, assignment_ref: str | None = None,
    ) -> None:
        with self._cond:
            pending = self._running.get(task_id)
            if pending is not None:
                pending.discard(future)
            self._cond.notify_all()
        if assignment_ref is None:
            return
        try:
            error = future.exception()
        except BaseException as lookup_error:  # noqa: BLE001 - make callback failures observable too
            error = lookup_error
        if error is not None:
            self._record_coordinator_dispatch_failure(
                task_id, error, phase="worker_future", assignment_ref=assignment_ref,
            )

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

    def _assignment_node_cancelled(self, task_id: str, assignment_ref: str) -> bool:
        """The runtime node carrying ``assignment_ref`` was durably cancelled
        (turn interruption, replanning, stop)."""
        with self._store.connection() as db:
            row = db.execute(
                "SELECT 1 FROM runtime_nodes WHERE task_id=? AND assignment_ref=? AND status='cancelled'",
                (task_id, assignment_ref),
            ).fetchone()
        return row is not None

    def _provision_and_prompt(self, task_id, coordinator_channel, payload, cancel_event):
        from ..orchestration.limits import assignment_current
        if not assignment_current(self._store,task_id,payload['assignment_ref']): return
        # No clock bound: a worker that is only thinking must never be
        # killed; a silent one is woken by live_adapters.prompt's nudge.
        return self._provision_and_prompt_inner(task_id,coordinator_channel,payload,cancel_event)

    def _provision_and_prompt_inner(self, task_id: str, coordinator_channel: ChannelContext, payload: Mapping[str, Any], cancel_event: threading.Event) -> None:
        # T017: capacity queuing happens *inside* SessionRuntime.open() (see
        # delegation.live_adapters.EngineSessionManagerAdapter.open) -- never
        # by retrying this whole delegate() call. delegate() uses an
        # idempotency-key claim/steal protocol per provisioning step
        # (delegation.provisioning._acquire_step); retrying it wholesale
        # after a mid-step exception would fight that protocol's own
        # contention recovery instead of just taking longer, exactly like a
        # slow real engine launch it is already built to tolerate.
        from ..orchestration.limits import assignment_current
        assignment_ref = payload["assignment_ref"]
        # A worker future may have been submitted immediately before
        # /cortex-stop.  The continuation cancel event is intentionally not
        # a worker kill switch for ordinary session/cancel, so the durable
        # stop marker is the fence checked before any provisioning side
        # effect.
        if acp_steering.is_stopped(self._store, task_id):
            return
        # A replacement coordinator has a new channel. Reusing its channel
        # for an old request would derive a different provisioning identity
        # and launch a worker which no original card or graph attempt owns.
        from ..delegation.provisioning import _idempotency_key

        if _idempotency_key(task_id, coordinator_channel.channel_id, payload["request_key"]) != assignment_ref:
            from ..recovery.worker_reconciliation import _mark_graph_attempt_failed_tx

            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                _mark_graph_attempt_failed_tx(db, task_id, assignment_ref, "recovery_worker_lost")
                delegation_events.append_event_tx(
                    db, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    dict(error="recovery_worker_lost", detail="Delegation belongs to a previous coordinator channel; issue a fresh delegation."),
                    origin="runtime", assignment_ref=assignment_ref,
                )
                db.commit()
            return
        try:
            result = self._provisioning.delegate(
                coordinator_channel, payload["title"], payload["brief"], payload["role"],
                payload["acceptance"], payload["request_key"],
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
            if self._assignment_node_cancelled(task_id, assignment_ref):
                # A cancelled assignment's queued open ends with the cancel
                # itself (``attempt``) or its revoked channel: the durable
                # cancellation already says what happened. Reporting it
                # also as a failure showed cancelled workers as errored.
                return
            result = None
            if failure_code == "capacity_timeout":
                # T027: this delegation's own attempt was allocated before
                # open() timed out -- resolvable via the identical idempotency key
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
        from ..orchestration.limits import assignment_current
        if cancel_event.is_set() or not assignment_current(self._store,task_id,assignment_ref):
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
        generation_delivery_id = None
        # A clean worker END_TURN with no durable Cortex outcome gets one
        # procedural chance to finish the existing report/publication
        # protocol in this same engine session.  The marker is durable so a
        # duplicate runtime cannot execute that continuation twice.
        worker_recovery_count = 0
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
                    f"{render_worker_catalog(self._task_workspace_root(task_id))}\n\n---\n\n{brief}"
                )
                recovery_prompt = None
                worker_prompt = (
                    'You are a worker, not the user-facing coordinator. Communicate exclusively in English: '
                    'all progress messages, summaries, reports, questions and final responses. '
                    'The original user text and coordinator language below are context, not your response language. '
                    'Preserve exact quoted evidence and any language explicitly required for project artifacts.\n\n'
                    + worker_prompt + '\n\nWorker response language: English only, including the final response.'
                )
                delivered_prompt, generation_delivery_id, delivery_blocked = self._begin_generation_delivery(
                    task_id, result.engine_session_ref, payload["role"], worker_prompt,
                )
                if delivery_blocked:
                    with self._lock:
                        if worker_recovery_count:
                            self._worker_recoveries_in_flight.discard(assignment_ref)
                    delegation_events.append_event(
                        self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                        dict(error="context_budget_blocked"), origin="runtime", assignment_ref=assignment_ref,
                    )
                    return
                if acp_steering.is_stopped(self._store, task_id):
                    with self._lock:
                        if worker_recovery_count:
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
                # runtime contract. Keep chunks in memory until the next
                # observable tool boundary or the turn end. The worker panel
                # receives complete message blocks, never token-sized rows.
                worker_delta_sequence = 0
                # ACP message chunks do not survive the portable engine
                # adapter with their provider messageId. Keep an explicit
                # durable identity for each logical worker prose segment;
                # increment it for every tool boundary, including hidden
                # Cortex/provider calls that are intentionally omitted from
                # the worker panel.
                worker_message_turn_id = f"{getattr(result, 'attempt_id', None) or result.engine_session_ref}:turn:{_hop}"
                worker_message_segment = 0
                worker_message_block_sequence = 0
                worker_stream_redactor = StreamingRedactor(max_chars=4000)

                def _on_worker_delta(chunk_text: str) -> bool:
                    # Buffer contiguous chunks so a credential split across
                    # ACP updates can be redacted. The coordinator's text
                    # channel stays clear, and the card is not redrawn per
                    # streamed token: the block reaches it once, at the next
                    # tool boundary or turn end (_flush_worker_message_block).
                    nonlocal worker_delta_seen, worker_delta_sequence
                    if chunk_text:
                        worker_delta_sequence += 1
                        worker_delta_seen = True
                        safe_chunk = worker_stream_redactor.push(str(chunk_text))
                        if not safe_chunk:
                            return True
                        with self._lock:
                            worker_message_buffer.append(safe_chunk)
                    return True

                def _on_worker_display_update(update: dict, _ref: str = assignment_ref) -> None:
                    if not isinstance(update, dict):
                        return
                    if update.get("sessionUpdate") == "usage_update":
                        # The worker's context fill for the Workers views:
                        # durable, but neither prose nor a transcript row.
                        clamped_size = self._clamp_attempt_context_size(result.attempt_id, update.get("size"))
                        if clamped_size is not None and clamped_size != update.get("size"):
                            update = {**update, "size": clamped_size}
                        self._record_worker_activity(
                            task_id, _ref, result.attempt_id, "context",
                            f"context {update.get('used')}/{update.get('size')}",
                            kind="usage_update", content_value=update,
                        )
                        return
                    with self._lock:
                        feed = self._worker_card_emitter(task_id, _ref)
                    _flush_worker_message_block(feed=feed)
                    kind = str(update.get("sessionUpdate") or update.get("type") or "display")
                    text_value = _worker_display_text(update)
                    if not text_value:
                        return
                    self._record_worker_activity(
                        task_id, _ref, result.attempt_id, f"display:{new_id('display_')}",
                        text_value, kind=kind, content_value=update,
                    )
                    if feed is not None:
                        self._append_worker_entry(_ref, text_value)
                        self._emit_worker_card(feed, _ref, status="in_progress")

                def _on_worker_tool_call(ref, kind, title, status, *rest, _ref=assignment_ref) -> None:
                    nonlocal worker_message_segment
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
                    with self._lock:
                        feed = self._worker_card_emitter(task_id, _ref)
                    # Finalize the ordinary message before changing its
                    # segment identity. This also applies to hidden runtime
                    # tools, whose own card is intentionally omitted.
                    _flush_worker_message_block(feed=feed)
                    boundary_ref = str(ref) if ref else f"anonymous:{worker_message_segment + 1}"
                    if boundary_ref not in worker_message_boundary_refs:
                        worker_message_boundary_refs.add(boundary_ref)
                        worker_message_segment += 1
                    worker_message_buffer.clear()
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
                    supplied_title = isinstance(title, str) and bool(title.strip())
                    title_value = title
                    if ref and supplied_title and ref in worker_pending_titleless:
                        pending = worker_pending_titleless.pop(ref)
                        _on_worker_tool_call(
                            ref, pending[0], title, pending[1], pending[2], pending[3], pending[4], pending[5],
                        )
                    with self._lock:
                        _tool_feed = self._worker_card_emitter(task_id, assignment_ref)
                    if supplied_title and ref and isinstance(title_value, str) and title_value.strip():
                        worker_tool_titles[ref] = _compact_text(title_value.strip(), _TOOL_CALL_TITLE_MAX_CHARS)
                    if not title:
                        title = worker_tool_titles.get(ref) or "unnamed observable tool call"
                    safe_content = _client_safe_content(content)
                    action = _worker_tool_details(
                        str(title), status, locations, safe_content,
                        _client_safe_raw(raw_input) if raw_input is not None else None,
                        None, self._task_workspace_root(task_id),
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
                            _project_root=self._task_workspace_root(task_id),
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
                worker_message_boundary_refs: set[str] = set()
                worker_wait_refs: set[str] = set()
                worker_pending_titleless: dict[str, tuple[Any, ...]] = {}
                worker_message_buffer: list[str] = []

                def _emit_worker_tool_update(*, feed, ref, kind, title, status,
                                              locations, content, raw_input,
                                              raw_output=None) -> None:
                    """Append one worker project/MCP call to its lifecycle card."""
                    from .read_history import failed_read_output, metadata_only_read
                    if metadata_only_read(title, kind, raw_input):
                        raw_output = (failed_read_output(raw_output, content)
                                      if _acp_tool_call_status(status) == 'failed' else None)
                        content = None
                    # The call is recorded for the worker panel only. It is
                    # never a sibling card in the coordinator's timeline.
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
                        self._task_workspace_root(task_id),
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

                def _flush_worker_message_block(*, feed, final_response: bool = False) -> None:
                    """Persist and render one complete worker prose block.

                    This is the only worker-delta emission point. A tool
                    boundary or normal turn end closes the block, so live
                    cards and replayed history contain the same message-sized
                    records and never expose token-sized fragments.
                    """
                    nonlocal worker_message_block_sequence
                    pending_stream_text = worker_stream_redactor.flush()
                    if pending_stream_text:
                        worker_message_buffer.append(pending_stream_text)
                    text_value = "".join(worker_message_buffer)
                    worker_message_buffer.clear()
                    if not text_value.strip():
                        return
                    worker_message_block_sequence += 1
                    attempt_id = str(getattr(result, "attempt_id", None) or result.engine_session_ref)
                    label = "final response" if final_response else "worker message"
                    activity_ref = "final-response" if final_response else "message"
                    message_id = f"{worker_message_turn_id}:segment:{worker_message_segment}"
                    safe_text = scrub_text(text_value, max_chars=max(4000, len(text_value) * 2 + 1000))
                    if final_response:
                        self._worker_final_response_seen.add(assignment_ref)
                    recorded = self._record_worker_activity(
                        task_id, assignment_ref, attempt_id, activity_ref,
                        f"[{label}] {safe_text}", status="in_progress",
                        stream_sequence=worker_message_block_sequence,
                        message_id=message_id,
                    )
                    if not recorded:
                        return
                    with self._lock:
                        self._worker_activity[assignment_ref] = f"[{label}] {safe_text}"
                    if feed is not None:
                        self._emit_worker_card(
                            feed, assignment_ref, status="in_progress",
                            entry=f"[{label}] {safe_text}",
                            entry_ref=message_id,
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
                # Recheck immediately before every native turn, including the
                # one bounded same-session completion follow-up.
                if cancel_event.is_set() or not assignment_current(self._store,task_id,assignment_ref):
                    with self._lock:
                        self._worker_turns_in_flight.discard(assignment_ref)
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
                        worker_message_block_sequence += 1
                        if not self._record_worker_activity(
                            task_id, assignment_ref, attempt_id, "final-response",
                            f"[final response] {scrub_text(outcome.text, max_chars=max(4000, len(outcome.text) * 2 + 1000))}",
                            status="in_progress",
                            stream_sequence=worker_message_block_sequence,
                            message_id=f"{worker_message_turn_id}:segment:{worker_message_segment}",
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
                    delta_only_terminal = (
                        worker_outcome is not None
                        and worker_outcome.stop_reason == StopReason.END_TURN
                        and not worker_terminal_text
                    )
                    if delta_only_terminal:
                        _flush_worker_message_block(
                            feed=_final_worker_feed, final_response=True,
                        )
                    else:
                        # PromptOutcome.text is authoritative when present;
                        # cancellation/failure leaves only a partial block.
                        # Neither should duplicate or relabel buffered text.
                        worker_message_buffer.clear()
                    task_stopped = not assignment_current(self._store,task_id,assignment_ref)
                    with self._lock:
                        pending_result = getattr(self, "_pending_worker_results", {}).pop(
                            assignment_ref, None
                        )
                        hold_recovery_fence = (
                            worker_outcome is not None
                            and worker_outcome.stop_reason == StopReason.END_TURN
                            and pending_result is None
                            and worker_recovery_count < MAX_WORKER_CONTINUATIONS
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
                            result_payload=pending_result,
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
                            result_payload=pending_result,
                        )
                # A normal END_TURN is not itself a worker failure.  Give a
                # worker which left no question, result, or owned published
                # report one bounded, same-session request to complete its
                # existing Cortex report protocol.  This is runtime
                # procedure, deliberately separate from the user's text.
                if (
                    outcome.stop_reason == StopReason.END_TURN
                    and not pending_result
                    and worker_recovery_count < MAX_WORKER_CONTINUATIONS
                    and not cancel_event.is_set()
                    and assignment_current(self._store,task_id,assignment_ref)
                ):
                    with self._worker_recovery_claim_lock:
                        recovery_state = self._claim_worker_same_session_recovery(
                            task_id, assignment_ref, worker_recovery_count + 1)
                        if recovery_state == "claimed":
                            with self._lock:
                                self._worker_recoveries_in_flight.add(assignment_ref)
                    if recovery_state == "claimed":
                        self._finalize_generation_delivery(
                            result.engine_session_ref, generation_delivery_id,
                            failure_kind=None, stop_reason=outcome.stop_reason,
                        )
                        generation_delivery_id = None
                        worker_recovery_count += 1
                        recovery_prompt = (
                            "Runtime completion follow-up for this existing assignment. "
                            "You ended your turn without publishing a result. A background job's "
                            "notification cannot reach you after your turn ends: if you started a "
                            "build, test or other background work, wait for it inside this turn by "
                            "checking its output or status (with short sleeps) until it finishes, and "
                            "never end the turn to wait. "
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
                        self._mirror_durable_worker_result_card(task_id, assignment_ref)
                with self._lock:
                    # The first-turn fence was held through the durable
                    # result/publication check.  Every non-continuation
                    # path (including ``complete``) must release it before
                    # returning to the coordinator wait loop.
                    self._worker_turns_in_flight.discard(assignment_ref)
                    self._worker_recoveries_in_flight.discard(assignment_ref)
                    # The event-claim path parks a result it observes while
                    # this fence is held, and decides that under this same
                    # lock.  A result published as the turn's last action
                    # can be parked after both checks above; release and
                    # pop atomically so it always reaches the card.
                    parked_result = getattr(self, "_pending_worker_results", {}).pop(
                        assignment_ref, None
                    )
                if parked_result is not None:
                    self._close_assignment(task_id, assignment_ref)
                    summary = parked_result.get("summary", "")
                    with self._lock:
                        has_final_response = assignment_ref in self._worker_final_response_seen
                        pending_feed = self._worker_card_emitter(task_id, assignment_ref)
                    self._emit_worker_card(
                        pending_feed, assignment_ref, status="completed",
                        entry=("Result: " + _compact_text(summary, _WORKER_ACTIVITY_MAX_CHARS - 8)
                               if summary and not has_final_response else None),
                        result_payload=parked_result,
                    )
                elif pending_result is None and outcome.stop_reason == StopReason.END_TURN:
                    # The coordinator's own delivery path can consume the
                    # worker_result event (it is coordinator input as well),
                    # so neither the card claim path nor the checks above
                    # need ever see it.  A durable result must still settle
                    # this card; the terminal fence keeps this idempotent.
                    self._mirror_durable_worker_result_card(task_id, assignment_ref)
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
                # A stopped/cancelled turn is not evidence of an engine fault.
                # In particular, never route an interrupted worker to fallback.
                # Daemon teardown kills the engine: transport loss, left for
                # restart recovery, never an engine failure or a fallback.
                from ..orchestration.limits import assignment_current
                if (cancel_event.is_set() or self._shutting_down.is_set()
                        or acp_steering.is_stopped(self._store, task_id)
                        or not assignment_current(self._store, task_id, assignment_ref)):
                    with self._lock:
                        self._worker_recoveries_in_flight.discard(assignment_ref)
                    return
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
                if self._shutting_down.is_set():
                    return
                failure = _describe_failure(error)
                self._mark_work_attempt_failed(task_id, assignment_ref, failure["error"], f"work-failed:{assignment_ref}:exception")
                delegation_events.append_event(
                    self._store, task_id, "assignment_failed", f"assignment_failed:{assignment_ref}",
                    failure, origin="runtime", assignment_ref=assignment_ref,
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

    def _claim_worker_same_session_recovery(self, task_id: str, assignment_ref: str, number: int = 1) -> str:
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
                # One marker per continuation (up to MAX_WORKER_CONTINUATIONS):
                # a single one left a worker waiting on its own background
                # build failed and restarted from scratch.
                marker_key = f"worker_recovery:{assignment_ref}" + (f":{number}" if number > 1 else "")
                marker = db.execute(
                    "SELECT 1 FROM task_events WHERE task_id=? "
                    "AND assignment_ref=? AND event_type='progress' "
                    "AND dedup_key=? LIMIT 1",
                    (task_id, assignment_ref, marker_key),
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
                    db, task_id, "progress", marker_key,
                    dict(kind="worker_same_session_recovery", assignment_ref=assignment_ref, number=number),
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
                from ..orchestration.recovery import release_observed
                release_observed(self,task_id,row['attempt_id'])
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
                    entry=_compact_text(f"Cancelled: {reason}", _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
        else:
            with self._lock:
                bound_emitter = self._worker_card_emitters.get(assignment_ref)
            if bound_emitter is not None:
                self._emit_worker_card(
                    bound_emitter, assignment_ref,
                    status=_TOOL_CALL_STATUS_FOR_CANCELLED,
                    entry=_compact_text(f"Cancelled: {reason}", _ASSIGNMENT_FAILED_DETAIL_MAX_CHARS),
                )
            else:
                for owner_session, status in durable_sessions.items():
                    if status not in {"completed", "failed"}:
                        self._persist_terminal_worker_card(owner_session, assignment_ref, reason)
        # Not self._notify() here: a waiting coordinator observes the new
        # event via its own bounded DB poll instead (see
        # _wait_for_progress) -- no _running entry is added/removed by this
        # call, so there is nothing for _cond to usefully notify here.


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
        already produced.

        In the task's Auto access mode (``configuration.access_mode``) the
        runtime answers instead of the user: the returned callable selects
        the request's own allow option without asking any host, so it is
        returned even when no connection is live. Every caller of this
        method is a user-approval prompt (a native deletion via
        ``engine.permissions``, and ``orchestration.consent``'s outside
        write/command and project deletion); refusals that are not the
        user's to give -- a native subagent, a stale binding, a gateway
        scope or role rule -- are decided before any requester is reached
        and are unchanged by the mode."""
        if configuration_access_mode.current(self._store, task_id) == configuration_access_mode.AUTO:
            store = self._store
            return lambda _engine_session_id, params, _timeout: configuration_access_mode.auto_approve(
                store, task_id, params)
        with self._lock:
            return self._task_permission_requesters.get(task_id)


        # Not self._notify() here -- same reasoning as _cancel_assignment
        # above.
