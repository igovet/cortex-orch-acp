"""The incoming ACP v1 stdio agent: wires transport + protocol + storage + runtime.

Entry point: ``main()`` (see ``cortex_acp.py``, a thin bootstrap identical in
spirit to ``cortex.py``). No network server, no MITM proxy, no custom
editor, no CODEX_HOME dependency. stdout is exclusively protocol traffic;
every log/banner goes to stderr (or the optional
``CORTEX_OBSERVATION_DIR`` value-free JSONL sink, mirroring
``server._write_observation``).

``main(runtime=...)`` injects the runtime this agent orchestrates with. The
production entry point (``cortex_acp.py``, T016) passes a real
``acp.production_runtime.ProductionOrchestratingRuntime``; omitting
``runtime=`` (as every non-orchestration test that does not need a live
runtime does) falls back to :class:`runtime.UnavailableRuntime`, which
fails closed on every prompt. Tests that need
``tests.acp_fakes.FakeRuntime`` import it themselves and pass it explicitly
(see ``tests/test_acp_*.py``); this module never imports anything from
``tests/``.
"""
from __future__ import annotations

import collections
import copy
import hashlib
import inspect
import json
import sqlite3
import os
import re
import signal
import sys
import threading
from typing import Mapping
import time
import uuid
from pathlib import Path
from types import SimpleNamespace

from . import commands as acp_commands
from . import config_options as acp_config_options
from . import messages as acp_messages
from . import protocol
from . import sessions as acp_sessions
from . import steering as acp_steering
from .host_requests import HostRequestClosed, HostRequester, HostRequestError, HostRequestTimeout
from .protocol import RpcError
from .runtime import OrchestrationUnavailable, UnavailableRuntime
from .transport import StdioTransport, EOF
from . import replay_store as acp_replay_store
from . import wire_budget as acp_wire_budget
from . import questions_store as acp_questions_store
from ..catalog import refresh as catalog_refresh
from ..bootstrap import _current_owner_epoch_obj, append_source_event, bootstrap_task
from ..catalog.model import CatalogEntry
from ..catalog.evidence import Fact, ProvenanceSource
from ..configuration.resolution import ConfigurationInputs, resolve_saved_route
from ..configuration.service import ConfigurationService
from ..configuration import access_mode as configuration_access_mode
from ..configuration import store as configuration_store
from ..configuration.mcp_servers import load_mcp_servers
from ..connections import registry as connection_registry
from ..contracts import StoreError
from ..diagnostic_boundary import emit_bounded_diagnostic, run_passive_diagnostic
from ..portable.engine_contract import StopReason
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from ..project_storage import canonical_project, logical_project_root, migrate_legacy_project_storage, project_store_directory
from ..store import Store, private_directory
from .. import task_workspace

PLUGIN = Path(__file__).resolve().parents[3]
_PROMPT_JOIN_TIMEOUT = 5.0
# Paseo discovers the provider catalog from the synchronous ``session/new``
# response and does not consume a later config_option_update while building
# its model picker.  Give a fresh, seed-only catalog a short opportunity to
# obtain the engine's real options.  This is deliberately bounded: an
# unavailable/slow engine leaves the truthful seed response in place and the
# normal post-session refresh remains the fallback.
_PASEO_DISCOVERY_WAIT_SECONDS = 3.0

# Bound on how many recent JSON-RPC request ids one connection remembers for
# the session/prompt duplicate-id rejection below (Agent._track_request_id).
# A long-lived connection cannot grow this without bound: once full, the
# oldest tracked id is evicted first (so an id far enough in the past could
# in principle be reused undetected -- accepted, since it is already well
# outside any plausible client retry window).
MAX_TRACKED_REQUEST_IDS = 10_000


def _request_id_identity(value):
    """An exact-JSON-type identity for a JSON-RPC request id.

    Python's ``==``/``hash`` treat ``1 == 1.0 == True`` (and would collapse
    them to the same dict key), but they are three distinct JSON-RPC ids
    (an integer, a number with a fractional part, and a boolean -- not
    actually a spec-legal id type at all, but still distinct from either
    number if a client sends one). Keying by ``(type name, exact JSON
    text)`` keeps every JSON type distinct without that collision, and
    without rejecting any id type here (transport-level id-shape validation,
    if ever added, is a separate, earlier concern -- see
    ``transport.parse_frame``).
    """
    return (type(value).__name__, json.dumps(value, sort_keys=True))


def _agent_version():
    try:
        return json.loads((PLUGIN / "package-identity.json").read_text())["version"]
    except (OSError, ValueError, KeyError):
        return "0.0.0"


def _write_observation(environ, operation, outcome):
    location = environ.get("CORTEX_OBSERVATION_DIR")
    if not location:
        return
    root = private_directory(location)
    path = root / f"{os.getpid()}.jsonl"
    row = dict(pid=os.getpid(), time_ns=time.time_ns(), operation=operation, outcome=outcome)
    fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        if os.fstat(fd).st_size < 256_000:
            os.write(fd, (json.dumps(row) + "\n").encode())
    finally:
        os.close(fd)


def observe(environ, operation, outcome):
    """Optional bounded passive observation; failures never alter behavior."""
    return run_passive_diagnostic(
        lambda: _write_observation(environ, operation, outcome),
        operation="observation",
        emit=lambda finding: emit_bounded_diagnostic(sys.stderr, finding),
    )


_MAX_CRASH_DIAGNOSTIC_FILES = 200


def _write_crash_diagnostic(
    environ, exc: BaseException, *, stage: str | None = None,
    task_id: str | None = None, session_id: str | None = None, attempt_id: str | None = None,
) -> None:
    """Durable, server-side-only record of a prompt-turn crash.

    Live (2026-09-19, see the except block below) a prompt died leaving
    nothing behind but "the runtime failed" -- no type, no line, nothing
    on stderr the operator could find. The class name and source line
    were then added to the ACP error's own ``data`` field, but a real
    third-party host (Paseo 0.9.2, observed 2026-09-25) logs only the
    error's plain-text message and drops that ``data`` field before it
    ever reaches a log an operator can read, and does not durably capture
    this process's own stderr either -- so the improved detail was still
    unreachable in practice. This writes the full traceback (never sent
    to the client -- the reply below is unchanged) to the same
    ``$XDG_STATE_HOME/cortex/`` root ``engine.logs`` already uses for
    per-engine diagnostics, so a crash is still findable after the host's
    own logging drops it. Best-effort and bounded like every sibling log
    under that root: a write that cannot land, or a crash-diagnostics
    directory already at capacity, is silently skipped -- this must never
    itself turn one failure into a second one.

    2026-09-25 audit follow-up: some retained dev tracebacks showed a
    start with a bound task id but no ``runtime_tasks`` row -- a
    coordinator engine launch failing (e.g. the engine's own child process
    could not initialize its state sqlite db, see gotchas.md) before
    ``orchestration.state.TaskMachine.create`` ever runs. ``stage``/
    ``task_id``/``session_id``/``attempt_id`` (every caller passes only
    what it actually has -- never guessed) and the runtime package version
    of *this exact running process* (``_agent_version()``, not the
    checkout it may have been built from -- see
    ``scripts/cortex_diagnose.py``'s separate installed-vs-checkout check)
    are recorded as a small plain-text header so an operator does not have
    to correlate a bare traceback against separately retained session
    state by hand. None of this is secret: identities are opaque ids this
    codebase itself mints, and the stage name is a fixed short label, never
    free text."""
    try:
        import traceback as _traceback

        from ..connections import registry as connection_registry

        root = connection_registry.state_root(environ)
        directory = root / "agent-crashes"
        connection_registry.ensure_private_dir(directory, root=root, environ=environ)
        try:
            existing = sum(1 for _ in directory.iterdir())
        except OSError:
            existing = 0
        if existing >= _MAX_CRASH_DIAGNOSTIC_FILES:
            return
        path = directory / f"{time.time_ns()}-{os.getpid()}.log"
        header_fields = [
            ("runtime_package_version", _agent_version()),
            ("stage", stage or "prompt"),
            ("task_id", task_id),
            ("session_id", session_id),
            ("attempt_id", attempt_id),
        ]
        header = "".join(f"{name}: {value}\n" for name, value in header_fields if value is not None)
        text = header + "".join(_traceback.format_exception(type(exc), exc, exc.__traceback__))
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            os.write(fd, text[:65536].encode("utf-8", errors="replace"))
        finally:
            os.close(fd)
    except Exception:
        pass


def _store_error_message(error: StoreError) -> str:
    """What the client is told when the store refuses.

    ``StoreError`` already carries why -- the refused field, what it
    received, what it expected -- and dropping all of it left a user
    staring at "Storage failed while handling this request." for causes
    they could have fixed in a second, such as a ``.cortex`` directory
    that is not owner-only. Everything here is the user's own local
    storage state (paths and modes, already truncated by
    ``contracts._received``), not engine or account data.
    """
    parts = [str(error)]
    if error.field:
        parts.append(f"field {error.field}")
    if error.received is not None:
        parts.append(f"got {error.received}")
    if error.expected:
        parts.append(f"expected {error.expected}")
    if error.correction:
        parts.append(str(error.correction))
    return "Storage failed while handling this request: " + "; ".join(parts) + "."


class _RealHostElicitor:
    """T020: the real ``acp.orchestrating_runtime.HostElicitor`` -- relays a
    coordinator ``ask_user`` question through the connected host's own
    ``elicitation/create`` request over that connection's own
    ``HostRequester`` (see ``Agent._make_elicitor``, which only ever
    constructs this when the host advertised
    ``clientCapabilities.elicitation.form`` at ``initialize``).

    Request shape (pinned schema-1.4.0 ``CreateElicitationRequest``, form
    mode, session scope): ``{"message", "mode": "form", "requestedSchema":
    {"type": "object", "properties": {"answer": {"type": "string", ...}},
    "required": ["answer"]}, "sessionId"}`` -- a single required text
    field, or (when ``options`` were given) a single-select enum on that
    same field (``StringPropertySchema``'s ``enum``). Response shape
    (``CreateElicitationResponse``): ``{"action": "accept", "content":
    {"answer": "..."}}`` on accept; ``{"action": "decline"}`` or
    ``{"action": "cancel"}`` otherwise -- both, plus a timeout/closed
    transport, map to ``None`` (the caller's plain-text fallback), never
    raised.
    """

    def __init__(self, host_requester: HostRequester, *, timeout: float = 30.0) -> None:
        self._host_requester = host_requester
        self._timeout = timeout
        self._question_cancelled = lambda: False

    def set_question_cancelled(self, cancelled):
        self._question_cancelled = cancelled

    def elicit(self, session_id: str, question: str, options: list[str] | None) -> str | None:
        answer_schema: dict = {"type": "string"}
        if options:
            answer_schema["enum"] = list(options)
        params = dict(
            message=question,
            mode="form",
            requestedSchema=dict(
                type="object",
                properties=dict(answer=answer_schema),
                required=["answer"],
            ),
            sessionId=session_id,
        )
        try:
            result = self._host_requester.request("elicitation/create", params, timeout=self._timeout,
                                                  cancelled=self._question_cancelled)
        except (HostRequestTimeout, HostRequestClosed, HostRequestError):
            return None
        except Exception:
            # A malformed/unexpected host reply is a decline-equivalent,
            # never an exception that aborts the coordinator's turn.
            return None
        if not isinstance(result, dict) or result.get("action") != "accept":
            return None
        content = result.get("content")
        if not isinstance(content, dict):
            return None
        answer = content.get("answer")
        return answer if isinstance(answer, str) and answer else None


class _PaseoQuestionElicitor:
    """Paseo plugin bridge using ACP's core permission card as a choice UI.

    The card stays until the user answers -- a 30 s timeout used to end the
    turn while Paseo kept showing the card, and a later Submit was dropped.
    The wait ends on a card choice, on a message typed into the same session
    (``register`` hands the agent a way to deliver it; it is the answer), or
    when the turn is cancelled."""

    #: A question waits for a person, not for a host round trip.
    QUESTION_TIMEOUT_SECONDS = 24 * 3600.0

    #: The config id that carries a card's typed "Other..." answer (see
    #: ``Agent._answer_question_card``).
    ANSWER_CONFIG_ID = "cortex_question_answer"

    def __init__(self, host_requester: HostRequester, *, session_id: str, timeout: float = 30.0,
                 cancel_event=None, register=None) -> None:
        self._host_requester = host_requester
        self._session_id = session_id
        self._timeout = timeout
        self._cancel_event = cancel_event
        self._register = register
        self._question_id = None

    def set_question_cancelled(self, cancelled):
        self._question_cancelled = cancelled

    def set_question_id(self, question_id: str) -> None:
        self._question_id = question_id

    #: Paseo renders the card's question as plain text, never markdown.
    CARD_TEXT_LIMIT = 200

    def split_question(self, question: str) -> tuple[str | None, str | None]:
        """``(body, title)``: ``body`` belongs in the chat, where Paseo renders
        markdown; ``title`` is the short line for the card (``None``: use a
        generic prompt). A short single-line question stays on the card."""
        text = question.strip()
        if "\n" not in text and len(text) <= self.CARD_TEXT_LIMIT:
            return None, text
        lines = text.splitlines()
        last = lines[-1].strip()
        if (last.endswith("?") and len(last) <= self.CARD_TEXT_LIMIT
                and not last.startswith(("-", "*", "#", "|", ">", "`")) and len(lines) > 1):
            return "\n".join(lines[:-1]).rstrip(), last.strip("* ")
        return text, None

    def elicit(self, session_id: str, question: str, options: list[str] | None) -> str | None:
        del session_id
        choices = list(options or [])
        # Paseo's permission bridge needs at least one action to render a
        # question card. The fallback action is instructional, not an answer.
        display_choices = choices or ["Reply in chat or type an answer under Other…"]
        option_ids = {f"cortex-question-{index}": value for index, value in enumerate(display_choices, 1)}
        params = dict(
            sessionId=self._session_id,
            toolCall=dict(
                toolCallId=f"cortex-question:{self._question_id or self._session_id}",
                kind="other", title=question[:4000],
                rawInput={"__cortex_question": True, "questionId": self._question_id, "question": question[:4000]},
            ),
            options=[
                dict(optionId=option_id, name=value, kind="allow_once")
                for option_id, value in option_ids.items()
            ],
            _meta=dict(cortexQuestion=True),
        )
        typed = []

        def deliver(text: str) -> bool:
            value = text.strip()
            typed.append(choices[int(value) - 1] if value.isdigit() and 1 <= int(value) <= len(choices) else text)
            return True

        if self._register is not None:
            # Tagged with its question: a late click on another question's
            # stale card must never answer this one.
            self._register((self._question_id, deliver))
        try:
            result = self._host_requester.request(
                "session/request_permission", params,
                timeout=max(self._timeout, self.QUESTION_TIMEOUT_SECONDS),
                cancelled=lambda: bool(typed) or (self._cancel_event is not None and self._cancel_event.is_set())
                or getattr(self, "_question_cancelled", lambda: False)(),
                # A Submit that arrives after this wait ended is still this
                # question's answer (see Agent._on_late_host_response).
                id_suffix=f"q.{self._question_id}" if self._question_id else None,
            )
        except (HostRequestTimeout, HostRequestClosed, HostRequestError):
            return typed[0] if typed else None
        finally:
            if self._register is not None:
                self._register(None)
            # O2: this toolCallId is never announced as its own tool_call --
            # only inside session/request_permission params above -- so the
            # host retires the form itself; a tool_call_update for it would be
            # an orphan update that surfaces as a generic "Tool call" row.
        if typed:
            return typed[0]
        if not isinstance(result, dict):
            return None
        outcome = result.get("outcome")
        if not isinstance(outcome, dict) or outcome.get("outcome") != "selected":
            return None
        return option_ids.get(outcome.get("optionId")) if choices else None

    def elicit_batch(self, session_id, batch):
        del session_id
        members = [dict(questionId=q['question_id'], question=q['question'], options=q['options'],
                        **({'topic': q['topic']} if q.get('topic') else {}))
                   for q in batch['questions']]
        tool_id = f"cortex-question-batch:{batch['batch_id']}"
        params = dict(
            sessionId=self._session_id,
            toolCall=dict(toolCallId=tool_id, kind='other', title='Answer related questions',
                          rawInput=dict(__cortex_question=True, batchId=batch['batch_id'],
                                        questionId=batch['question_id'], questions=members)),
            options=[dict(optionId=f"cortex-batch:{q['questionId']}:{i}", name=label, kind='allow_once')
                     for q in members for i, label in enumerate(q['options'], 1)],
            _meta=dict(cortexQuestion=True),
        )
        try:
            self._host_requester.request(
                'session/request_permission', params,
                timeout=max(self._timeout, self.QUESTION_TIMEOUT_SECONDS),
                cancelled=lambda: (self._cancel_event is not None and self._cancel_event.is_set())
                or getattr(self, '_question_cancelled', lambda: False)(),
            )
            # ACP itself returns only one option. That cannot answer a batch.
            # Only the validated complete config submission resolves it.
        except (HostRequestTimeout, HostRequestClosed, HostRequestError):
            pass
        # O2: the batch's toolCallId is never announced as its own tool_call --
        # only inside session/request_permission params above -- so the host
        # retires the form itself; a tool_call_update here would be an orphan
        # update that surfaces as a generic "Tool call" row (see agent.py's
        # fix spec and provider.ts's visibleTimeline defence in depth).


class Agent:
    """One ACP connection's worth of state: sessions, stores, dispatch.

    JSON-RPC request id reuse: within one connection, a ``session/prompt``
    whose id was already used by any earlier request on this connection
    (still in flight or already completed) is rejected with ``-32600``
    ("duplicate_request_id") before any durable capture is attempted --
    see ``_track_request_id``/``_dispatch``. Ids are tracked per connection
    only (bounded to the most recent ``MAX_TRACKED_REQUEST_IDS``); a fresh
    connection (a new agent process/reconnect) starts with an empty set, so
    an id that was used on a prior connection is not remembered and a
    request reusing it is treated as new. This is intentional: ACP clients
    do not auto-retry a ``session/prompt`` across a reconnect, so an id
    collision after a reconnect is not the same in-flight-retry hazard this
    guard defends against, and it durably becomes a new source event
    (``bootstrap.append_source_event`` allocates a fresh ``event_id`` per
    delivery regardless of the JSON-RPC id); any reconciliation of
    duplicate *content* across a reconnect is a later recovery concern, not
    this guard's job.
    """

    #: Default (production) ``elicitation/create`` request timeout in
    #: seconds -- see ``_make_elicitor``/``elicitation_timeout_seconds``.
    DEFAULT_ELICITATION_TIMEOUT_SECONDS = 30.0
    #: Bounds a caller-supplied ``elicitation_timeout_seconds`` (T020
    #: fresh-review fix: previously an unbounded os.environ lookup in
    #: production code -- see the constructor docstring).
    _MIN_ELICITATION_TIMEOUT_SECONDS = 0.05
    _MAX_ELICITATION_TIMEOUT_SECONDS = 300.0

    def __init__(self, runtime, *, environ=None, elicitation_timeout_seconds=None):
        """``elicitation_timeout_seconds`` (T020 fresh-review fix, MEDIUM):
        an explicit constructor parameter for the real
        ``elicitation/create`` request timeout (see ``_make_elicitor``) --
        never an ``os.environ`` lookup in production code. ``None`` (every
        production caller) uses ``DEFAULT_ELICITATION_TIMEOUT_SECONDS``; a
        test that needs a short, bounded wait (see
        ``tests/test_acp_elicitation.py``) passes a small value directly.
        Validated positive and bounded -- an out-of-range value is rejected
        immediately (construction-time, never a silent clamp)."""
        if elicitation_timeout_seconds is None:
            elicitation_timeout_seconds = self.DEFAULT_ELICITATION_TIMEOUT_SECONDS
        elif not (self._MIN_ELICITATION_TIMEOUT_SECONDS <= elicitation_timeout_seconds <= self._MAX_ELICITATION_TIMEOUT_SECONDS):
            raise ValueError(
                f"elicitation_timeout_seconds must be between {self._MIN_ELICITATION_TIMEOUT_SECONDS} and "
                f"{self._MAX_ELICITATION_TIMEOUT_SECONDS}; got {elicitation_timeout_seconds!r}."
            )
        self._elicitation_timeout_seconds = float(elicitation_timeout_seconds)
        self.runtime = runtime
        # T016: a multi-project-capable runtime (see
        # acp.production_runtime.ProductionOrchestratingRuntime) accepts an
        # optional 'project_root' kwarg to resolve its own per-project
        # store/route; detected once here (never per-call) so every other
        # IncomingRuntime (UnavailableRuntime, tests.acp_fakes.FakeRuntime)
        # keeps receiving the exact same 5-positional-argument call it
        # always has.
        try:
            self._runtime_accepts_project_root = 'project_root' in inspect.signature(runtime.start_prompt).parameters
        except (TypeError, ValueError):
            self._runtime_accepts_project_root = False
        # T018: detected the exact same way as project_root above -- a
        # runtime (real or FakeRuntime) that has not been updated for T018
        # simply never receives source_origin/source_event_id and falls
        # back to its own pre-T018 turn-count identity (see
        # acp.orchestrating_runtime.start_prompt's docstring).
        try:
            self._runtime_accepts_source_identity = 'source_event_id' in inspect.signature(runtime.start_prompt).parameters
        except (TypeError, ValueError):
            self._runtime_accepts_source_identity = False
        # T020: detected the same way -- a runtime not updated for T020
        # (every FakeRuntime) simply never receives host_elicitor and
        # stays on the plain-text ask_user fallback unconditionally.
        try:
            self._runtime_accepts_host_elicitor = 'host_elicitor' in inspect.signature(runtime.start_prompt).parameters
        except (TypeError, ValueError):
            self._runtime_accepts_host_elicitor = False
        # T035: detected the same way -- a runtime not updated for T035
        # (every pre-T035 FakeRuntime/test) simply never receives
        # emit_tool_call and stays on the plain-text-only narration it
        # already had.
        try:
            self._runtime_accepts_emit_tool_call = 'emit_tool_call' in inspect.signature(runtime.start_prompt).parameters
            self._runtime_accepts_emit_display_update = (
                'emit_display_update' in inspect.signature(runtime.start_prompt).parameters
            )
            self._runtime_accepts_host_mcp_servers = (
                'host_mcp_servers' in inspect.signature(runtime.start_prompt).parameters
            )
        except (TypeError, ValueError):
            self._runtime_accepts_emit_tool_call = False
            self._runtime_accepts_emit_display_update = False
            self._runtime_accepts_host_mcp_servers = False
        # EXT-01 #5: detected the same way -- a runtime not updated for this
        # fix (every pre-existing FakeRuntime/test) simply never receives
        # host_permission_requester/host_fs_requester and keeps its own
        # pre-existing (unwired) behavior unchanged.
        try:
            self._runtime_accepts_host_permission_requester = (
                'host_permission_requester' in inspect.signature(runtime.start_prompt).parameters
            )
        except (TypeError, ValueError):
            self._runtime_accepts_host_permission_requester = False
        try:
            self._runtime_accepts_host_fs_requester = (
                'host_fs_requester' in inspect.signature(runtime.start_prompt).parameters
            )
        except (TypeError, ValueError):
            self._runtime_accepts_host_fs_requester = False
        try:
            self._runtime_accepts_prompt_blocks = 'prompt_blocks' in inspect.signature(runtime.start_prompt).parameters
            self._runtime_accepts_client_surface = 'client_surface' in inspect.signature(runtime.start_prompt).parameters
        except (TypeError, ValueError):
            self._runtime_accepts_prompt_blocks = False
            self._runtime_accepts_client_surface = False
        try:
            cancel_task_params = inspect.signature(runtime.cancel_task).parameters
            self._runtime_accepts_cancel_task_project_root = 'project_root' in cancel_task_params
            self._runtime_accepts_cancel_task_emitter = 'emit_tool_call' in cancel_task_params
        except (AttributeError, TypeError, ValueError):
            self._runtime_accepts_cancel_task_project_root = False
            self._runtime_accepts_cancel_task_emitter = False
        self.environ = environ if environ is not None else os.environ
        self.version = _agent_version()
        self.stores = {}
        self.sessions = {}
        self.transport = None
        self.host_requester = None
        self.initialized = False
        self.client_info = {}
        self.client_capabilities = {}
        self._workers = []
        self._workers_lock = threading.Lock()
        # Host history and outbound queue insertion share one lock per ACP
        # session.  This makes the durable sequence and live notification
        # order identical even when coordinator/worker callbacks race.
        self._history_locks = {}
        self._history_locks_lock = threading.Lock()
        self._seen_request_ids = collections.OrderedDict()

    def bind_transport(self, transport):
        self.transport = transport
        # T014: this agent can also *initiate* a request to the host (for
        # example relaying a downstream engine's session/request_permission
        # -- see engine.permissions.PermissionBridge), tracked in a
        # namespace ("hr_"-prefixed ids) disjoint from any id the host
        # itself chooses for its own requests to this agent.
        self.host_requester = HostRequester(transport)
        self.host_requester.on_late = self._on_late_host_response

    _LATE_QUESTION_ID = re.compile(r"_q\.(pq_[0-9A-Za-z]+)$")

    def _on_late_host_response(self, request_id, message):
        """A question card answered after its turn stopped waiting (the turn
        was cancelled, the connection or process was replaced): record the
        choice as that question's answer and carry the task on, instead of
        dropping it."""
        match = self._LATE_QUESTION_ID.search(request_id) if isinstance(request_id, str) else None
        outcome = (message.get("result") or {}).get("outcome") if isinstance(message, dict) else None
        if match is None or not isinstance(outcome, dict) or outcome.get("outcome") != "selected":
            return
        option = str(outcome.get("optionId") or "")
        if not option.startswith("cortex-question-") or not option[len("cortex-question-"):].isdigit():
            return
        question_id, choice = match.group(1), option[len("cortex-question-"):]
        for session_id, session in list(self.sessions.items()):
            if not session.get("task_id"):
                continue
            store = self._store_for(session["project"])
            question = acp_questions_store.get(store, question_id)
            if question is not None and question.task_id == session["task_id"]:
                # A free-text card's sole action is instructional, not an answer.
                # Invalid/stale option ids are not user-provided answer text either.
                if not question.options or not 1 <= int(choice) <= len(question.options):
                    return
                threading.Thread(target=self._run_late_answer, name="acp-late-answer",
                                 args=(session_id, session, question, choice), daemon=True).start()
                return
        observe(self.environ, "session/request_permission", "late_answer_unmatched")

    def _run_late_answer(self, session_id, session, question, choice, recorded=False):
        cancel_event = threading.Event()
        with session["lock"]:
            waiter = session.get("question_waiter")
            if session["active"] is not None:
                if recorded:
                    return  # the durable status wakes the existing waiter/runner
                # A turn is running: hand it the answer if it waits on this
                # very question's card, else record it durably (a no-op for
                # a question already answered) for that turn to pick up.
                if waiter is not None and waiter[0] == question.id:
                    waiter[1](choice)
                else:
                    store = self._store_for(session["project"])
                    options = question.options or []
                    index = int(choice) if choice.strip().isdigit() else 0
                    value = options[index - 1] if 1 <= index <= len(options) else choice
                    if acp_questions_store.answer(store, question.id, value, "elicitation", session_id=session_id):
                        # Same runtime record every other answer path writes,
                        # so workers see which session answered.
                        from ..delegation import events as delegation_events

                        delegation_events.append_event(
                            store, question.task_id, "progress", f"question_answered:{question.id}",
                            dict(kind="question_answered", question_id=question.id, answer_text=value,
                                 session_id=session_id),
                            origin="runtime",
                        )
                return
            session["active"] = cancel_event
            session["background"] = True
        try:
            answer_method, answer_kwargs = self._runtime_call_kwargs(
                "answer_question", project_root=session["project"], recorded=recorded,
                emit_tool_call=self._make_emit_tool_call(session_id, cancel_event),
            )
            if answer_method is None:
                return
            store = self._store_for(session["project"])
            message_id = f"assistant:late-answer:{question.id}"

            def emit_update(chunk_text):
                if cancel_event.is_set() or self.transport is None:
                    return False
                with self._history_lock(session_id):
                    try:
                        acp_replay_store.append_host_transcript(
                            store, session_id, "agent_message", chunk_text, message_id=message_id)
                    except (StoreError, OSError, ValueError, TypeError, sqlite3.Error):
                        return False
                return self.transport.send_notification(protocol.agent_message_chunk(session_id, chunk_text, message_id))

            answer_method(session["task_id"], session_id, question, choice, emit_update, cancel_event, **answer_kwargs)
        except Exception:
            observe(self.environ, "session/request_permission", "late_answer_failed")
        finally:
            self._finish_prompt(session_id, session, cancel_event)

    def _track_request_id(self, request_id):
        """Record ``request_id`` as used on this connection; return whether
        it was already used (in flight or completed) by an earlier request.

        Bounded to the most recent ``MAX_TRACKED_REQUEST_IDS`` ids: marks a
        request id as "in flight" immediately on receipt (not only once
        processing completes), so a resend of the same id while the
        original is still being handled is also caught.

        Keyed by an exact JSON identity (``_request_id_identity``), not the
        raw Python value: JSON-RPC ids of different JSON types must never
        collide even when Python's own equality/hashing would consider them
        equal -- ``1 == 1.0 == True`` in Python, but the ids ``1``, ``1.0``
        and ``true`` are three distinct JSON-RPC request ids.
        """
        key = _request_id_identity(request_id)
        already_used = key in self._seen_request_ids
        if not already_used:
            self._seen_request_ids[key] = True
            if len(self._seen_request_ids) > MAX_TRACKED_REQUEST_IDS:
                self._seen_request_ids.popitem(last=False)
        return already_used

    # -- storage -------------------------------------------------------

    def _store_for(self, project):
        store = self.stores.get(project)
        if store is None:
            directory = project_store_directory(project)
            store = Store(directory, initialize=True, project_root=project)
            self.stores[project] = store
        return store

    # -- inbound dispatch (called synchronously on the transport reader
    #    thread; only session/prompt hands work off to a worker thread so
    #    the reader keeps servicing session/cancel while a turn runs) ----

    def handle_message(self, message, error):
        if message is EOF:
            self._handle_eof("stdin_eof")
            return
        if error is not None:
            self._reply_error(error.request_id, error.code, error.message)
            return
        if "id" in message and "method" not in message and ("result" in message or "error" in message):
            # A response, not a request/notification: only ever a reply to
            # one of this agent's own HostRequester calls (T014). Never
            # dispatched as a method call and never replied to again.
            if self.host_requester is None or not self.host_requester.handle_response(message):
                sys.stderr.write("acp: unmatched response-shaped message ignored\n")
            return
        has_id = "id" in message
        request_id = message.get("id") if has_id else None
        method = message.get("method")
        if not isinstance(method, str):
            if has_id:
                self._reply_error(request_id, -32600, "Missing or invalid \"method\".")
            return
        if method != "initialize" and not self.initialized:
            if has_id:
                self._reply_error(request_id, -32600, "Call initialize before any other method.")
            else:
                sys.stderr.write(f"acp: notification {method!r} before initialize, ignored\n")
            return
        try:
            self._dispatch(method, message.get("params"), request_id, has_id)
        except RpcError as exc:
            if has_id:
                self._reply_error(request_id, exc.code, exc.message, exc.data)
            observe(self.environ, method, "error")
        except StoreError as exc:
            if has_id:
                self._reply_error(request_id, -32603, _store_error_message(exc))
            observe(self.environ, method, str(exc))
        except PortableError as exc:
            if has_id:
                mapped = self._portable_error_to_rpc(exc)
                self._reply_error(request_id, mapped.code, mapped.message, mapped.data)
            observe(self.environ, method, exc.code)
        except Exception:
            if has_id:
                self._reply_error(request_id, -32603, "Internal error while handling this request.")
            observe(self.environ, method, "internal_error")

    def _dispatch(self, method, params, request_id, has_id):
        if has_id:
            already_used = self._track_request_id(request_id)
            if method == "session/prompt" and already_used:
                # Reject before any durable capture is attempted (see the
                # class docstring): bootstrap_task/append_source_event must
                # never even be reached for a resent id.
                raise RpcError(-32600, "duplicate_request_id")
        if method == "initialize":
            result = self._initialize(params)
            self.initialized = True
            self.transport.send_response(protocol.result_response(request_id, result))
            observe(self.environ, method, "success")
        elif method == "session/new":
            result = self._session_new(params)
            self.transport.send_response(protocol.result_response(request_id, result))
            self._post_session_new_notifications(result["sessionId"])
            observe(self.environ, method, "success")
        elif method == "session/prompt":
            self._session_prompt(request_id, params)
        elif method == "session/cancel":
            self._session_cancel(params)
            observe(self.environ, method, "success")
        elif method == "session/set_config_option":
            self._session_set_config_option(request_id, params)
            observe(self.environ, method, "success")
        elif method == "session/set_mode":
            self._session_set_mode(request_id, params)
            observe(self.environ, method, "success")
        elif method == "session/load":
            self._session_load(request_id, params)
            observe(self.environ, method, "success")
        elif method in protocol.UNSUPPORTED_METHODS:
            if has_id:
                self._reply_error(request_id, -32601, f"Method '{method}' is not supported.")
            observe(self.environ, method, "unsupported")
        else:
            if has_id:
                self._reply_error(request_id, -32601, f"Unknown method '{method}'.")
            else:
                sys.stderr.write(f"acp: unknown notification {method!r} ignored\n")
            observe(self.environ, method, "unknown")

    def _reply_error(self, request_id, code, message, data=None):
        if self.transport is None:
            return
        self.transport.send_response(protocol.error_response(request_id, RpcError(code, message, data)))

    # -- method handlers -------------------------------------------------

    def _initialize(self, params):
        protocol.validate_initialize_params(params)
        self.client_info = params.get("clientInfo") or {}
        self.client_capabilities = params.get("clientCapabilities") or {}
        return protocol.initialize_result(self.version)

    def _session_workspace(self, session):
        """The tree this session's files live in: its task's recorded
        working tree once there is a task, else the tree it was opened in."""
        if session.get("task_id"):
            try:
                return task_workspace.store_task_root(self._store_for(session["project"]), session["task_id"])
            except StoreError:
                pass
        return session.get("workspace") or session["project"]

    def _session_new(self, params):
        cwd, raw_mcp_servers = protocol.validate_session_new_params(params)
        try:
            # A linked worktree shares its main repository's store but is
            # its own working tree (see ``task_workspace``).
            workspace = canonical_project(cwd)
            project = logical_project_root(workspace)
        except StoreError:
            raise RpcError(-32602, "cwd must be an absolute, canonical, existing directory.") from None
        migrate_legacy_project_storage(project)
        store = self._store_for(project)
        # T018 (fresh-review follow-up): opportunistic, best-effort sweep of
        # any attachment staging temp files orphaned by a prior crashed
        # process (see acp.replay_store.sweep_orphaned_staging's docstring
        # for why an age-only sweep here can never touch a referenced
        # body). Never fails session/new -- a sweep failure is silently
        # skipped, not surfaced to the client.
        try:
            acp_replay_store.sweep_orphaned_staging(project)
        except Exception:
            pass
        effective_mcp_servers = load_mcp_servers(project, client_servers=raw_mcp_servers, environ=self.environ)
        stripped_servers = protocol.strip_mcp_servers(effective_mcp_servers)
        session_id = new_id("acps_")
        acp_sessions.create_session(store, session_id, project, self.client_info, self.client_capabilities, stripped_servers,
                                    workspace_root=workspace)
        session = dict(
            project=project, workspace=workspace, task_id=None, channel=None, active=None, lock=threading.Lock(),
            owner_epoch=None, pending_selection=None, default_selection=None,
            pending_language=None, pending_access_mode=None,
            client_capabilities=dict(self.client_capabilities),
            # The client's own MCP servers, kept whole and in memory only so
            # the engines this session drives are offered the same tools the
            # user configured in their editor. Storage keeps the stripped
            # shape above and never these values: an entry's env/headers are
            # the client's secrets.
            mcp_servers=effective_mcp_servers,
            # Connection ids for which this session already attempted the
            # bounded discovery wait.  The post-session refresh skips these
            # ids so a timed-out probe is not launched a second time while
            # its daemon probe is still unwinding.
            catalog_refresh_started=set(),
        )
        self.sessions[session_id] = session
        # A prior explicit choice persisted as the *normal* T008 user-default
        # route/parameters (see _apply_pre_task_selection) is only ever this
        # session's *default* (shown with state "default", never "pending")
        # until this session itself makes its own explicit choice -- see
        # _current_selection_context. It lives only in this session's memory
        # (never re-marked as "pending" anywhere durable).
        session["default_selection"] = self._default_selection_from_user_defaults()
        self._wait_for_paseo_live_catalog(session)
        options = self._config_options_for_session(session, store)
        result = dict(sessionId=session_id)
        if options:
            result["configOptions"] = options
        modes = self._access_modes_for_session(session, store)
        if modes is not None:
            result["modes"] = modes
        return result

    def _post_session_new_notifications(self, session_id):
        """Sent by the caller after the session/new response itself (see
        ``_dispatch``): ``available_commands_update`` is not part of the
        ``NewSessionResponse`` payload, only ever a follow-up notification.

        Catalog discovery is deliberately started after the response has been
        sent.  A fresh ACP session must become usable immediately, while the
        engine-advertised model list is refreshed in the background and then
        delivered as a normal ``config_option_update``.  Starting this before
        the response would allow a slow engine to reorder protocol frames.
        """
        if self.transport is not None:
            self.transport.send_notification(
                protocol.available_commands_update(session_id, acp_commands.AVAILABLE_COMMANDS))
        self._start_catalog_refresh(session_id)

    def _start_catalog_refresh(self, session_id):
        """Refresh every configured engine for a new ACP session.

        This is prompt-free discovery (``initialize`` + ``session/new`` only),
        not a model turn.  Each engine runs in its own daemon thread so one
        unavailable provider cannot delay the ``session/new`` response or
        another provider.  ``perform_refresh`` is fail-closed and preserves
        the last good snapshot on every failure; consequently a failed refresh
        emits no selector update and leaves the current catalog visible.

        The transport check also keeps direct in-process unit calls—which do
        not have a live ACP connection—from accidentally starting real engine
        processes.
        """
        if self.transport is None:
            return
        session = self.sessions.get(session_id)
        if session is None:
            return
        project = session["project"]
        try:
            connections = connection_registry.read_registry(self.environ).connections
        except Exception:
            return
        for record in connections.values():
            if record.id in session.get("catalog_refresh_started", set()):
                continue
            session.setdefault("catalog_refresh_started", set()).add(record.id)
            thread = threading.Thread(
                target=self._refresh_catalog_for_session,
                args=(session_id, session, project, record),
                name=f"cortex-catalog-refresh-{record.id}",
                daemon=True,
            )
            thread.start()

    def _wait_for_paseo_live_catalog(self, session):
        """Bounded live discovery for Paseo's initial provider catalog.

        Paseo's provider discovery reads only the ``session/new`` response.
        A static seed is useful for truthful fallback, but it intentionally
        carries no effort values.  When the session is a Paseo session and
        any selectable entry is seed-provenance, probe those connections in
        parallel and wait briefly for a real engine observation.  The probe
        is prompt-free and uses the existing refresh path; no effort values
        are synthesized on timeout or failure.
        """
        if not self._combined_model_picker() or self.transport is None:
            return
        try:
            entries, connections = self._selectable_entries_and_connections(session["project"])
        except Exception:
            return
        seed_connections = {
            entry.connection_id
            for entry in entries
            if (entry.evidence.get(Fact.CATALOG_OBTAINED) is not None
                and entry.evidence.get(Fact.CATALOG_OBTAINED).source is ProvenanceSource.SEED)
        }
        records = [connections[connection_id] for connection_id in seed_connections if connection_id in connections]
        if not records:
            return
        started = session.setdefault("catalog_refresh_started", set())
        records = [record for record in records if record.id not in started]
        if not records:
            return
        done = threading.Event()
        remaining = {record.id for record in records}
        lock = threading.Lock()

        def refresh(record):
            try:
                catalog_refresh.perform_refresh(record, session["project"], environ=self.environ)
            except Exception:
                pass
            finally:
                with lock:
                    remaining.discard(record.id)
                    if not remaining:
                        done.set()

        for record in records:
            started.add(record.id)
            threading.Thread(
                target=refresh, args=(record,),
                name=f"cortex-paseo-discovery-{record.id}", daemon=True,
            ).start()
        done.wait(_PASEO_DISCOVERY_WAIT_SECONDS)

    def _refresh_catalog_for_session(self, session_id, session, project, record):
        """Run one prompt-free refresh and publish only a successful result."""
        try:
            outcome = catalog_refresh.perform_refresh(
                record, project, environ=self.environ,
            )
        except Exception:
            # ``perform_refresh`` already converts expected failures to an
            # unsuccessful outcome.  Keep this last-resort guard here so a
            # provider implementation bug cannot kill the ACP connection's
            # background worker or create a false selector update.
            return
        if not outcome.success or self.transport is None:
            return
        if self.sessions.get(session_id) is not session:
            return
        try:
            store = self._store_for(project)
            options = self._config_options_for_session(session, store)
            if options:
                self.transport.send_notification(protocol.config_option_update(session_id, options))
        except Exception:
            # The snapshot is durable even if the client disconnected while
            # the refresh completed.  It will be picked up by the next ACP
            # session; never turn a notification failure into a session error.
            return

    # -- configuration options / commands (shared by session/new,
    #    session/set_config_option, and the /cortex-* command equivalents) --

    def _selectable_entries_and_connections(self, project_root=None):
        registry_state = connection_registry.read_registry(self.environ)
        entries = acp_config_options.selectable_entries_from_snapshots(registry_state.connections, self.environ)
        entries = acp_config_options.filter_denied_entries(
            entries, project_root, self.environ)
        return acp_config_options.presented_entries(entries), registry_state.connections

    def _default_selection_from_user_defaults(self):
        """The last explicit ``/cortex-model``/``set_config_option`` choice
        persisted as the *normal* T008 user-default route/parameters (see
        ``_apply_pre_task_selection``) -- plain ``route_id``/``parameters``
        fields, never a Cortex-specific "pending" marker -- or ``None``.

        ``connection_id``/``model`` (T035 FINDING-05 fix): the saved
        choice's stable identity, persisted alongside ``route_id`` by every
        write site since this fix -- ``None``/``None`` for a defaults.json
        written before this fix existed, which
        ``_propose_pending_selection_for_new_task`` recovers one-time via
        ``catalog.store.find_route_identity`` and then persists forward."""
        defaults = configuration_store.read_user_defaults(self.environ)["defaults"]
        if isinstance(defaults, dict) and defaults.get("coordinator") is not None and not defaults.get("route_id"):
            from ..configuration.resolution import resolve_owner_coordinator
            from ..configuration.resolution import portable_from_defaults
            value = portable_from_defaults(defaults)
            try:
                entry = resolve_owner_coordinator(value, self.environ)
            except PortableError as error:
                return dict(route_id=None, connection_id=None, model=value.get("model"),
                            parameters=value.get("parameters", {}), coordinator=value,
                            resolution_error=error.code)
            return dict(route_id=entry.route.id, connection_id=entry.connection_id, model=entry.route.model,
                        parameters=value["parameters"], coordinator=value)
        route_id = defaults.get("route_id") if isinstance(defaults, dict) else None
        if not isinstance(route_id, str) or not route_id:
            return None
        parameters = defaults.get("parameters") if isinstance(defaults, dict) else None
        connection_id = defaults.get("route_connection_id") if isinstance(defaults, dict) else None
        model = defaults.get("route_model") if isinstance(defaults, dict) else None
        return dict(
            route_id=route_id, parameters=parameters if isinstance(parameters, dict) else {},
            connection_id=connection_id if isinstance(connection_id, str) and connection_id else None,
            model=model if isinstance(model, str) and model else None,
        )

    def _current_selection_context(self, store, session):
        task_id = session["task_id"]
        if task_id is not None:
            history = ConfigurationService(store).history(task_id)
            best = acp_config_options.most_recent_live_selection(history)
            if best is not None:
                return dict(route_id=best["route_id"], state=best["state"],
                            revision=best["revision"], parameters=best["parameters"])
            return None
        pending = session.get("pending_selection")
        if pending is not None:
            # This session itself made an explicit choice pre-task: shown as
            # pending, applies on the next (first) prompt.
            return dict(route_id=pending["route_id"], state="pending_next_use",
                        revision=None, parameters=pending.get("parameters") or {})
        default = session.get("default_selection")
        if default is not None:
            # Inherited from a prior session's persisted user default: shown
            # as this session's plain default, never "pending" -- nothing in
            # *this* session has changed it yet.
            return dict(route_id=default["route_id"], state="default",
                        revision=None, parameters=default.get("parameters") or {},
                        coordinator=default.get("coordinator"), resolution_error=default.get("resolution_error"))
        return None

    def _default_language_from_user_defaults(self):
        """The last explicit ``/cortex-language``/``set_config_option``
        choice persisted pre-task (see ``_apply_language_selection``), or
        ``None`` (meaning "auto") -- mirrors ``_default_selection_from_user_defaults``."""
        defaults = configuration_store.read_user_defaults(self.environ)["defaults"]
        value = defaults.get("language") if isinstance(defaults, dict) else None
        return value if value in ("en", "ru") else None

    def _make_elicitor(self, session_id, cancel_event=None, session=None):
        """T020: a real ``_RealHostElicitor`` iff the host advertised
        ``clientCapabilities.elicitation.form`` at ``initialize`` --
        schema-1.4.0's own convention: ``form`` omitted or ``null`` means
        no support; ``{}`` (present but empty) already means support (see
        ``ElicitationCapabilities.form``'s pinned description). ``None``
        otherwise -- the caller's plain-text ask_user fallback.

        The request timeout is ``self._elicitation_timeout_seconds`` (an
        explicit, validated constructor parameter -- T020 fresh-review fix,
        MEDIUM: previously an unbounded ``os.environ`` lookup reachable in
        production code; see ``__init__``)."""
        if self.host_requester is None:
            return None
        if self.environ.get("CORTEX_PASEO_QUESTION_UI") == "1":
            return _PaseoQuestionElicitor(
                self.host_requester, session_id=session_id, timeout=self._elicitation_timeout_seconds,
                cancel_event=cancel_event,
                register=None if session is None else (lambda deliver: session.__setitem__("question_waiter", deliver)),
            )
        capabilities = self.client_capabilities if isinstance(self.client_capabilities, dict) else {}
        elicitation = capabilities.get("elicitation")
        if not isinstance(elicitation, dict) or elicitation.get("form", None) is None:
            return None
        return _RealHostElicitor(self.host_requester, timeout=self._elicitation_timeout_seconds)

    def _make_permission_requester(self, session_id):
        """EXT-01 #5: a real, connection-bound
        ``(engine_session_id, params, timeout) -> dict`` relay for a
        downstream engine's ``session/request_permission`` (see
        ``engine.permissions.PermissionBridge``), or ``None`` when this
        connection has no transport yet. Unlike ``_make_elicitor``, ACP
        defines no client capability flag for permission requests at all
        (``session/request_permission`` is a core, not-optional host
        feature -- verified against this repository's own vendored
        ``ClientCapabilities`` schema, which has no such property): the only
        gate is whether a live ``HostRequester`` exists, exactly matching
        ``engine.permissions.PermissionBridge``'s own pre-existing
        "``None`` means never reaches a host" contract.

        ``engine_session_id`` is accepted and discarded here -- this
        connection has exactly one ``session_id`` per ``start_prompt`` call
        (the caller's own), and the *engine*-session-to-*task* identity
        mapping this relay is answering for is already resolved one layer
        below, by ``delegation.live_adapters.EngineSessionManagerAdapter``,
        before this callable is ever invoked (see
        ``EngineSessionManagerAdapter._host_permission_requester``); this
        closure only ever needs to know which ACP ``sessionId`` (this
        connection's own) to put on the outbound wire request."""
        if self.host_requester is None:
            return None
        host_requester = self.host_requester

        def request_permission(engine_session_id, params, timeout=30.0):
            del engine_session_id  # see docstring: resolved one layer below
            body = dict(params or {})
            body["sessionId"] = session_id
            try:
                return host_requester.request("session/request_permission", body, timeout=timeout or 30.0)
            except (HostRequestTimeout, HostRequestClosed, HostRequestError):
                return None

        return request_permission

    def _make_fs_requester(self, session_id):
        """EXT-01 #5: a real, connection-bound ``(method, params) -> dict``
        relay for the host's own negotiated ``fs/*`` capability (see
        ``engine.fs_bridge.HostFsForwarder``), built -- like
        ``_make_elicitor`` -- iff the host actually advertised
        ``clientCapabilities.fs.readTextFile`` at ``initialize`` (schema-
        1.4.0's own ``FileSystemCapabilities``; ``fs`` omitted, or with
        ``readTextFile`` false/missing, both mean "cannot read a host
        buffer" -- never guessed at). ``None`` otherwise -- the caller
        (``acp.orchestrating_runtime._make_host_fs_checker``) then records
        the integration's own ``host_buffer_state: 'unknown'`` rather than
        ever treating "capability not negotiated" as "checked, no
        conflict".

        Deliberately never catches an exception from the underlying
        ``HostRequester.request`` call: the caller distinguishes a clean "no
        buffer for this path" (``None``/empty content) from a failed
        round-trip (timeout/closed transport/error) by whether *this*
        callable raises -- see ``delegation.integration.integrate``'s own
        ``host_fs_checker`` contract docstring for exactly why swallowing an
        exception here would be the one thing this fix must not do."""
        capabilities = self.client_capabilities if isinstance(self.client_capabilities, dict) else {}
        fs = capabilities.get("fs")
        if not isinstance(fs, dict) or not fs.get("readTextFile"):
            return None
        if self.host_requester is None:
            return None
        host_requester = self.host_requester

        def request_fs(method, params):
            # ACP's own fs/* requests are session-scoped: schema 1.4.0's
            # ReadTextFileRequest requires ``sessionId``, and a real client
            # rejects a request without one outright -- Zed answered
            # -32602 "missing field `sessionId`", which
            # ``delegation.integration.integrate`` could only read as a
            # failed round-trip, so every integration against a real client
            # conflicted with ``host_buffer_check_failed`` and no worker
            # change could ever land. This relay is bound to exactly one
            # session, so it is the one place that knows it; a caller that
            # supplies its own is never overridden.
            payload = dict(params or {})
            payload.setdefault("sessionId", session_id)
            return host_requester.request(method, payload)

        return request_fs

    def _make_emit_display_update(self, session_id, cancel_event):
        """T035: relay one engine ``session/update`` that exists only to be
        shown -- the model's reasoning (``agent_thought_chunk``) and its
        task list (``plan``) -- onto this agent's own session, unchanged
        except for the session id.

        These were previously dropped as unrecognized, so a person watching
        saw neither. They are forwarded rather than reinterpreted: the
        schema already defines how a client renders them, and inventing a
        second representation would only lose detail.

        Only the kinds ``engine.session_manager._DISPLAY_ONLY_UPDATES``
        allows ever reach here; the engine's own session-state updates are
        not among them, because this agent's session is not that session.
        """
        def emit_display_update(update):
            if cancel_event.is_set() or self.transport is None:
                return False
            if not isinstance(update, Mapping) or not update.get("sessionUpdate"):
                return False
            payload = {key: value for key, value in update.items() if key != "sessionId"}
            return self._persist_and_enqueue_update(session_id, payload, cancel_event)
        return emit_display_update

    def _history_lock(self, session_id):
        with self._history_locks_lock:
            return self._history_locks.setdefault(session_id, threading.Lock())

    def _persist_and_enqueue_update(self, session_id, update, cancel_event):
        """Record a real runtime update before offering it to the host.

        ``send_notification`` only means that the bounded transport accepted
        the frame; its writer may fail later.  Recording first preserves the
        event for reconnect in that case without claiming host receipt.  A
        persistence failure is fail-closed: no update is enqueued and the
        prompt is cancelled.
        """
        if cancel_event.is_set() or self.transport is None:
            return False
        with self._history_lock(session_id):
            from .read_history import ReadHistoryFilter
            filters = getattr(self, '_read_history_filters', None)
            if filters is None:
                filters = self._read_history_filters = {}
            update = filters.setdefault(session_id, ReadHistoryFilter()).project(update)
            # Last-resort size guard: the host retains every event, so one
            # oversized update must never reach it (or the replay journal).
            capped = acp_wire_budget.cap_session_update(update)
            if capped is not update:
                observe(self.environ, "session/update", "truncated")
                update = capped
            try:
                persisted = self._record_host_update(session_id, update)
            except Exception:
                persisted = False
            if not persisted:
                cancel_event.set()
                return False
            delivered = self.transport.send_notification(
                protocol.notification("session/update", dict(sessionId=session_id, update=update)),
            )
            if not delivered:
                cancel_event.set()
            return delivered

    def _record_host_update(self, session_id, update):
        """Persist one structured runtime update before transport enqueue.

        A true return means only that the event is durable and may safely be
        offered to the bounded transport; it never claims that Zed rendered
        or even wrote the frame.  A false return is fail-closed.
        """
        session = self.sessions.get(session_id)
        if session is None:
            return False
        try:
            store = self._store_for(session["project"])
            acp_replay_store.append_host_update(store, session_id, update)
        except (StoreError, OSError, ValueError, TypeError, sqlite3.Error):
            return False
        return True

    def _make_emit_tool_call(self, session_id, cancel_event, on_message_boundary=None):
        """T035: the runtime-facing ``emit_tool_call`` callable -- accepts
        one already-shaped update dict (``tool_call_id``, ``new`` [bool],
        ``title``/``kind``/``status``/``text`` -- fields the runtime omits
        are left unchanged on an existing card, per
        ``acp.protocol.tool_call_update``'s own "only changed fields"
        contract) and sends the matching ``session/update`` notification:
        ``tool_call`` the first time a given ``tool_call_id`` is seen
        (``new=True``, set by ``acp.orchestrating_runtime`` itself, which
        already knows -- from the durable event log -- exactly once per
        assignment when that is), ``tool_call_update`` every time after.
        Never raises (same convention as ``emit_update`` below); ``None``
        transport or an already-cancelled turn is a silent no-op, exactly
        like a dropped text chunk."""
        def emit_tool_call(update):
            if cancel_event.is_set() or self.transport is None:
                return False
            tool_call_id = update["tool_call_id"]
            if update.get("new"):
                # A tool card is a timeline boundary.  Text emitted after a
                # new tool call must receive a fresh messageId so Paseo never
                # merges coordinator prose on either side of the tool.
                if on_message_boundary is not None:
                    on_message_boundary()
                notif = protocol.tool_call(
                    session_id, tool_call_id, update.get("title") or "Worker assignment",
                    update.get("kind") or "other", update.get("status") or "pending",
                    text=update.get("text"), locations=update.get("locations"),
                    content=update.get("content"), raw_input=update.get("raw_input"),
                    raw_output=update.get("raw_output"),
                )
            else:
                notif = protocol.tool_call_update(
                    session_id, tool_call_id, status=update.get("status"), title=update.get("title"),
                    text=update.get("text"), locations=update.get("locations"),
                    content=update.get("content"), raw_input=update.get("raw_input"),
                    raw_output=update.get("raw_output"),
                )
            return self._persist_and_enqueue_update(session_id, notif["params"]["update"], cancel_event)
        # The runtime sizes worker cards for the client that renders them
        # (Paseo shows only title, status and result).
        emit_tool_call.client_surface = self.client_info.get("name")
        # The ACP session this feed writes to: a card created in one session
        # must be created again, not updated, in another.
        emit_tool_call.session_id = session_id
        return emit_tool_call

    def _current_language_for_session(self, store, session):
        """T020: the ``language`` SessionConfigOption's current value for
        this session -- the task's own durable per-task override once a
        task exists (``acp.steering.get_language_override``), else this
        session's own not-yet-applied pending choice, else the inherited
        persisted user default; ``None`` at every level means "auto"."""
        task_id = session["task_id"]
        if task_id is not None:
            return acp_steering.get_language_override(store, task_id)
        pending = session.get("pending_language")
        if pending is not None:
            return pending
        return self._default_language_from_user_defaults()

    def _config_options_for_session(self, session, store):
        entries, connections = self._selectable_entries_and_connections(session["project"])
        current = self._current_selection_context(store, session)
        # Catalog refreshes mint a new route id when the engine's advertised
        # parameters change.  A fresh, task-less session can therefore still
        # inherit a valid saved (connection, model) choice whose old route id
        # is no longer present.  Resolve that identity before building the
        # dependent Paseo model/thought-level options; otherwise the combined
        # picker marks the saved route unavailable and substitutes its first
        # model (for example Sol for a saved Luna selection).
        if current and current.get("state") == "default":
            route_ids = {entry.route.id for entry in entries}
            if current.get("route_id") not in route_ids:
                saved = session.get("default_selection") or {}
                connection_id = saved.get("connection_id")
                model = saved.get("model")
                match = next(
                    (entry for entry in entries
                     if entry.connection_id == connection_id and entry.route.model == model),
                    None,
                )
                if match is not None:
                    current = dict(current, route_id=match.route.id)
        language_current = self._current_language_for_session(store, session)
        return acp_config_options.build_config_options(
            entries, connections, current, include_language=True, language_current=language_current,
            language_as_mode=self._combined_model_picker(),
            # Paseo shows the access mode as its mode (shield) control from
            # ``modes`` (see _access_modes_for_session); a second, option
            # copy would render as a duplicate setting there.
            include_access_mode=not self._combined_model_picker(),
            access_mode_current=self._current_access_mode_for_session(store, session),
            environ=self.environ, project_root=session["project"],
            combined_model_picker=self._combined_model_picker())

    def _current_access_mode_for_session(self, store, session):
        """The task's recorded access mode once a task exists, else this
        session's own not-yet-applied choice, else Guarded. Never read from
        user defaults: every new session starts Guarded."""
        if session["task_id"] is not None:
            return configuration_access_mode.current(store, session["task_id"])
        return configuration_access_mode.normalize(session.get("pending_access_mode"))

    def _access_modes_for_session(self, session, store):
        """ACP ``modes`` for Paseo, whose composer mode control (the access
        shield) is built from them; ``None`` for other clients, which get
        the ``access_mode`` config option instead."""
        if not self._combined_model_picker():
            return None
        return acp_config_options.access_mode_state(self._current_access_mode_for_session(store, session))

    def _apply_access_mode_selection(self, store, session, session_id, value, origin):
        """``value`` is already validated against the offered modes. Before
        a task exists the choice waits in this session (applied as the
        task's first revision on the first prompt); afterwards it is the
        task's next recorded revision, read by the runtime at every
        permission request and worker open."""
        if session["task_id"] is None:
            session["pending_access_mode"] = value
        else:
            configuration_access_mode.switch(store, session["task_id"], value, origin=origin, session_id=session_id)
        observe(self.environ, "access_mode", f"switched:{value}")
        return True, self._config_options_for_session(session, store), None

    def _combined_model_picker(self):
        """Paseo currently renders the model selector but not the provider selector.

        This only changes option presentation; route policy and dispatch remain
        the same. Zed continues to receive its separate provider/model axes.
        """
        return str(self.client_info.get("name", "")).casefold() == "paseo"

    def _connection_setup_summary(self, connections):
        if not connections:
            return "no connection registered yet; see /cortex-setup"
        state_counts = self._connection_state_counts(connections)
        configured = state_counts["configured"]
        details = [f"{configured}/{len(connections)} connection(s) configured"]
        if state_counts["authorization_required"]:
            details.append(f"{state_counts['authorization_required']} awaiting authorization")
        if state_counts["unavailable"]:
            details.append(f"{state_counts['unavailable']} unavailable")
        if state_counts["invalid"]:
            details.append(f"{state_counts['invalid']} invalid")
        return "; ".join(details)

    @staticmethod
    def _connection_state_counts(connections):
        return collections.Counter(record.state.value for record in connections.values())

    def _session_prompt(self, request_id, params):
        session_id, text, raw_blocks = protocol.validate_session_prompt_params(params)
        recognized = acp_commands.recognize(raw_blocks)
        meta = params.get("_meta") if isinstance(params, dict) else None
        session = self.sessions.get(session_id)
        if session is None:
            raise RpcError(-32602, "Unknown sessionId.")
        cancel_event = threading.Event()
        background = None
        with session["lock"]:
            if session.get("cancelling"):
                raise RpcError(-32602, "This session is being cancelled.")
            if session["active"] is not None:
                if recognized is not None and recognized[0] == acp_commands.CORTEX_CLEAR:
                    raise RpcError(-32602, "Cortex cleanup requires an idle session; no deletion started.")
                # The running turn is waiting on a question card: a message
                # typed into this session is that question's answer, and the
                # running turn carries on with it.
                waiter = session.get("question_waiter")
                if text and (recognized is None or recognized[0] == acp_commands.CORTEX_ANSWER):
                    store = self._store_for(session["project"])
                    questions = acp_questions_store.list_open(store, session["task_id"]) if session.get("task_id") else []
                    chosen, answer = None, text
                    if recognized is not None:
                        parsed = acp_commands.parse_answer_argument(recognized[1])
                        if not parsed or not 1 <= parsed[0] <= len(questions):
                            raise RpcError(-32602, "Use /cortex-answer <question-number> <answer text>.")
                        chosen, answer = questions[parsed[0] - 1], parsed[1]
                    elif len(questions) > 1:
                        raise RpcError(-32602, "Multiple questions are open; use /cortex-answer <question-number> <answer text>.")
                    elif questions:
                        chosen = questions[0]
                    if chosen is not None and chosen.assignment_ref:
                        # Bind before delivery; do not let the singleton visible card
                        # select a different worker when several questions are durable.
                        with self._history_lock(session_id):
                            acp_replay_store.append_host_transcript(store, session_id, "user_message", answer)
                        if waiter is not None and waiter[0] == chosen.id:
                            waiter[1](answer)
                        else:
                            threading.Thread(target=self._run_late_answer, name="acp-worker-answer",
                                             args=(session_id, session, chosen, answer), daemon=True).start()
                        self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result("end_turn")))
                        return
                    if waiter is not None and text and recognized is None and waiter[1](text):
                        self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result("end_turn")))
                        return
                terminal_command = recognized is not None and recognized[0] in (
                    acp_commands.CORTEX_STOP, acp_commands.CORTEX_FINISH)
                background = session["active"] if session.get("background") or terminal_command else None
                if background is None:
                    raise RpcError(-32602, "A prompt is already in flight for this session.")
                if recognized is None or recognized[0] not in (
                        acp_commands.CORTEX_STOP, acp_commands.CORTEX_FINISH):
                    # Host idle/queue draining is not a Stop. Keep this
                    # request unanswered until the continuation releases
                    # ownership; never pass its cancellation token to a
                    # second controller or roll back captured steering.
                    pending = session.setdefault("pending_prompts", [])
                    if len(pending) >= 64:
                        raise RpcError(-32602, "Too many prompts waiting for this session.")
                    pending.append((request_id, session_id, session, text, cancel_event,
                                    recognized, copy.deepcopy(raw_blocks), copy.deepcopy(meta)))
                    return
        if background is not None:
            # Explicit terminal commands retain their urgent interruption.
            self._session_cancel(dict(sessionId=session_id))
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                with session["lock"]:
                    if session["active"] is None:
                        break
                time.sleep(0.05)
        with session["lock"]:
            if session["active"] is not None:
                raise RpcError(-32602, "A prompt is already in flight for this session.")
            session["active"] = cancel_event
        self._launch_prompt((request_id, session_id, session, text, cancel_event,
                             recognized, raw_blocks, meta))

    def _launch_prompt(self, args):
        worker = threading.Thread(
            target=self._run_prompt, name="acp-prompt", args=args, daemon=True)
        with self._workers_lock:
            self._workers.append(worker)
        worker.start()

    def _finish_prompt(self, session_id, session, cancel_event):
        next_prompt = None
        cancelled = []
        with session["lock"]:
            if session["active"] is not cancel_event:
                return
            session["active"] = None
            session["background"] = False
            pending = session.get("pending_prompts")
            if pending and cancel_event.is_set():
                cancelled = session.pop("pending_prompts")
            elif pending:
                next_prompt = pending.pop(0)
                # Reserve under the same lock as release: an arriving
                # prompt/continuation cannot overtake a waiting request.
                session["active"] = next_prompt[4]
        for prompt in cancelled:
            prompt[4].set()
            self.transport.send_response(protocol.result_response(
                prompt[0], protocol.prompt_result("cancelled")))
        if next_prompt is not None:
            self._launch_prompt(next_prompt)
        elif not cancel_event.is_set():
            self._start_queued_message_continuation(session_id, session)

    def _run_prompt(self, request_id, session_id, session, text, cancel_event, recognized=None, raw_blocks=None, meta=None):
        try:
            if cancel_event.is_set():
                self.transport.send_response(protocol.result_response(
                    request_id, protocol.prompt_result("cancelled")))
                return
            if recognized is not None:
                try:
                    with self._history_lock(session_id):
                        acp_replay_store.append_host_transcript(
                            self._store_for(session["project"]), session_id, "user_message", text)
                except (StoreError, OSError, ValueError, TypeError, sqlite3.Error):
                    self._reply_error(request_id, -32603, "Could not record command prompt.")
                    observe(self.environ, "session/prompt", "command_history_failed")
                    return
                self._run_command_body(request_id, session_id, session, recognized, cancel_event)
                return
            self._run_prompt_body(request_id, session_id, session, text, cancel_event, raw_blocks, meta)
        except Exception:
            # Anything unexpected here -- including a failure in
            # _store_for/SimpleNamespace construction *before* the durable-
            # capture try/except below is even reached -- must still reply.
            # Otherwise the client hangs forever waiting for this request's
            # response (never dropped, per the transport's own contract).
            # No content is echoed back.
            self._reply_error(request_id, -32603, "Internal error while processing this prompt.")
            observe(self.environ, "session/prompt", "internal_error")
        finally:
            self._finish_prompt(session_id, session, cancel_event)

    def _run_prompt_body(self, request_id, session_id, session, text, cancel_event, raw_blocks=None, meta=None):
        """The actual prompt turn; wrapped by ``_run_prompt`` for the
        catch-all -32603 safety net and the ``session["active"]`` cleanup.
        """
        store = self._store_for(session["project"])
        origin = f"acp:{session_id}"
        event_id = str(uuid.uuid4())
        source_event = SimpleNamespace(origin=origin, event_id=event_id)
        is_first_prompt = session["task_id"] is None
        # T018: resolve every non-text content block (EmbeddedResource
        # 'resource' / 'resource_link') and stage its body to an
        # owner-private temp file *before* the durable capture transaction
        # opens (no DB lock held during this file I/O; see
        # acp.replay_store.resolve_prompt_blocks/stage_blocks -- neither
        # ever raises, an unexpected per-block failure is recorded as
        # availability='inaccessible', reason='capture_error', never
        # dropped). bootstrap_task/append_source_event then records every
        # staged block's metadata (and renames its body into place)
        # *inside the same transaction* as the source text/source_events
        # row -- atomic: a crash before commit persists neither; a crash
        # after commit has both together (a replay unlinks the staged
        # bodies instead of recording anything new).
        staged_blocks = []
        if raw_blocks:
            resolved_blocks = acp_replay_store.resolve_prompt_blocks(raw_blocks, self._session_workspace(session))
            image_failures = [
                capture for capture in resolved_blocks
                if capture.kind == "image" and capture.availability != "stored"
            ]
            if image_failures:
                # An image is meaningful to the selected engine only when
                # its exact bytes are retained. Refuse before bootstrap or
                # append so an oversized/unreadable image can never be
                # recorded as accepted and then forwarded from the raw
                # inbound request. Keep the response bounded: MIME and the
                # capture policy reason are enough to remediate it.
                reason = image_failures[0].reason or "capture_unavailable"
                self._reply_error(
                    request_id, -32602,
                    "Image content could not be captured within Cortex limits.",
                    data={"reason": reason, "retryable": False},
                )
                observe(self.environ, "session/prompt", "image_capture_rejected:" + reason)
                return
            if resolved_blocks:
                staged_blocks = acp_replay_store.stage_blocks(session["project"], resolved_blocks)
            staged_image_failures = [
                item.capture for item in staged_blocks
                if item.capture.kind == "image" and item.capture.availability != "stored"
            ]
            if staged_image_failures:
                reason = staged_image_failures[0].reason or "capture_unavailable"
                acp_replay_store.cleanup_staged(staged_blocks)
                self._reply_error(
                    request_id, -32602,
                    "Image content could not be captured within Cortex limits.",
                    data={"reason": reason, "retryable": False},
                )
                observe(self.environ, "session/prompt", "image_capture_rejected:" + reason)
                return
        try:
            if is_first_prompt:
                # Read before the task exists, which is the whole point:
                # with a task_id in hand this same lookup answers with the
                # brand-new task's own (empty) override instead of the
                # choice the session is carrying.
                session_language = self._current_language_for_session(store, session)
                session_access_mode = session.get("pending_access_mode")
                result = bootstrap_task(store, source_event, text, owner_ref=origin, blocks=staged_blocks, meta=meta,
                                        workspace_root=session.get("workspace"))
                session["task_id"] = result.task_id
                session["owner_epoch"] = result.owner_epoch
                if result.channel_id is not None:
                    # The channel secret is kept only in this process's
                    # memory (this dict), never logged, never sent to
                    # the ACP client, never persisted.
                    session["channel"] = (result.channel_id, result.secret)
                acp_sessions.bind_task(store, session_id, result.task_id)
                # The language the user already chose, carried onto the
                # task the moment it exists. Before a task there is none,
                # so the choice lives in the session (and in the user's
                # defaults); nothing moved it across, so a brand-new
                # thread had no language override at all and the
                # coordinator was never told to answer in it. Live: the
                # composer read "Russian" and every reply came back in
                # English.
                if session_language is not None:
                    acp_steering.set_language_override(store, result.task_id, session_language)
                # Likewise the access mode the user picked before the
                # first prompt; a session that never picked one leaves the
                # new task Guarded.
                if session_access_mode is not None:
                    configuration_access_mode.switch(
                        store, result.task_id, session_access_mode,
                        origin="acp_pre_task_selection", session_id=session_id)
                    session["pending_access_mode"] = None
            else:
                result = append_source_event(store, source_event, text, session["task_id"], blocks=staged_blocks, meta=meta)
                if result.owner_epoch is not None:
                    session["owner_epoch"] = result.owner_epoch
        except StoreError:
            acp_replay_store.cleanup_staged(staged_blocks)
            self._reply_error(request_id, -32603, "Durable capture failed; nothing was forwarded to orchestration.")
            observe(self.environ, "session/prompt", "capture_failed")
            return
        task_id = session["task_id"]
        attachment_descriptors = [
            dict(source_ref=f"{result.report_id}:{b.capture.block_index}", name=b.capture.name, uri=b.capture.uri,
                 kind=b.capture.kind, availability=b.capture.availability, reason=b.capture.reason)
            for b in staged_blocks
        ] if not result.replayed else []
        if is_first_prompt:
            # The selector is rendered by the ACP client, while the first
            # task's configuration is only materialised after the durable
            # bootstrap.  Re-send the *applied* selection before opening an
            # engine session so a stale client-side label cannot silently
            # disagree with the route recorded for this attempt.  Do not
            # send a fallback surface when no saved route was applied: that
            # would make an unconfigured task look configured.
            applied_selection = self._propose_pending_selection_for_new_task(store, session, task_id)
            if applied_selection and self.transport is not None and not cancel_event.is_set():
                self.transport.send_notification(
                    protocol.config_option_update(session_id, self._config_options_for_session(session, store))
                )
        # Keep inbound capture compatible with older store adapters; replay
        # assigns a per-row fallback id when this legacy-style row has none.
        acp_replay_store.append_host_transcript(store, session_id, "user_message", text, attachment_descriptors)
        # One prompt turn is one logical assistant message.  Every streamed
        # delta in this turn carries the same ACP messageId, while a later
        # turn gets a fresh id and therefore cannot be merged by Paseo.
        message_id_state = {"value": f"assistant:{event_id}", "counter": 0}

        def rotate_message_id():
            message_id_state["counter"] += 1
            message_id_state["value"] = f"assistant:{event_id}:{message_id_state['counter']}"

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            with self._history_lock(session_id):
                try:
                    acp_replay_store.append_host_transcript(
                        store, session_id, "agent_message", chunk_text, message_id=message_id_state["value"],
                    )
                except (StoreError, OSError, ValueError, TypeError, sqlite3.Error):
                    cancel_event.set()
                    return False
                delivered = self.transport.send_notification(
                    protocol.agent_message_chunk(session_id, chunk_text, message_id_state["value"])
                )
                if not delivered:
                    # The client cannot keep up: the event remains durable,
                    # but this turn must stop rather than silently continue.
                    cancel_event.set()
                return delivered

        start_prompt_kwargs = {}
        if self._runtime_accepts_project_root:
            start_prompt_kwargs["project_root"] = session["project"]
        if self._runtime_accepts_source_identity:
            start_prompt_kwargs["source_origin"] = origin
            start_prompt_kwargs["source_event_id"] = event_id
        if self._runtime_accepts_host_elicitor:
            # T020: only a real, per-connection HostRequester-backed
            # elicitor when the host actually advertised
            # clientCapabilities.elicitation.form at initialize -- see
            # _make_elicitor. None (the ordinary case for every host that
            # doesn't advertise it, and every pre-T020 runtime) falls back
            # to the runtime's own constructor default (also None), which
            # is the unconditional plain-text ask_user fallback.
            start_prompt_kwargs["host_elicitor"] = self._make_elicitor(session_id, cancel_event, session)
        if self._runtime_accepts_host_permission_requester:
            # EXT-01 #5: same "never guessed, None is the honest default"
            # convention as host_elicitor above.
            start_prompt_kwargs["host_permission_requester"] = self._make_permission_requester(session_id)
        if self._runtime_accepts_host_fs_requester:
            start_prompt_kwargs["host_fs_requester"] = self._make_fs_requester(session_id)
        if self._runtime_accepts_emit_tool_call:
            start_prompt_kwargs["emit_tool_call"] = self._make_emit_tool_call(
                session_id, cancel_event, rotate_message_id,
            )
        if self._runtime_accepts_emit_display_update:
            start_prompt_kwargs["emit_display_update"] = self._make_emit_display_update(session_id, cancel_event)
        if self._runtime_accepts_host_mcp_servers:
            # The servers the client sent with this session, forwarded to
            # the coordinator and to every worker (see
            # ``engine.mcp_bridge.host_mcp_servers``). They were parsed and
            # kept on the session from the first day and then never passed
            # anywhere: the bridge that forwards them had tests, and
            # nothing ever called it. Live evidence: an engine Cortex
            # launched carried only its own ``cortex`` server, while the
            # same editor's own agent had the user's three.
            start_prompt_kwargs["host_mcp_servers"] = session.get("mcp_servers") or None
        if self._runtime_accepts_prompt_blocks:
            start_prompt_kwargs["prompt_blocks"] = raw_blocks
        if self._runtime_accepts_client_surface:
            start_prompt_kwargs["client_surface"] = self.client_info.get("name")
        try:
            reason = self.runtime.start_prompt(task_id, session_id, text, emit_update, cancel_event, **start_prompt_kwargs)
        except OrchestrationUnavailable as exc:
            # str(exc) is "orchestration_unavailable" for the default
            # UnavailableRuntime (unchanged wire behavior); a
            # ProductionOrchestratingRuntime instead names the concrete,
            # non-silent reason (e.g. "no_configuration_revision",
            # "instruction_delivery_unqualified") so the client sees what
            # is actually missing rather than a generic refusal.
            reason_code = str(exc) or "orchestration_unavailable"
            data = {"retryable": False, "reason": reason_code}
            remediation = getattr(exc, "remediation", None)
            if remediation:
                # What to change, not only what went wrong.
                data["remediation"] = remediation
            self.transport.send_response(protocol.error_response(
                request_id, RpcError(-32001, "orchestration_unavailable", data=data)))
            observe(self.environ, "session/prompt", reason_code)
            return
        except Exception as exc:
            if cancel_event.is_set():
                reason = StopReason.CANCELLED
            else:
                # Say *what* failed. Live (2026-09-19) a prompt died here
                # and left nothing behind but "the runtime failed": no type,
                # no line, nothing on stderr, and the observation says only
                # "runtime_error". That is unowned time for whoever has to
                # find it. The class name and the failing source line are
                # not user content and carry no secret; the message is
                # deliberately not included, since it can quote a path or a
                # value.
                where = ""
                traceback = exc.__traceback__
                while traceback is not None:
                    frame = traceback.tb_frame.f_code
                    where = f"{os.path.basename(frame.co_filename)}:{traceback.tb_lineno}"
                    traceback = traceback.tb_next
                detail = type(exc).__name__ + (f" at {where}" if where else "")
                print(f"cortex: prompt failed: {detail}", file=sys.stderr, flush=True)
                # P1 diagnosable launch failure: acp.orchestrating_runtime
                # ._open_coordinator_session tags (never replaces) an
                # engine-launch exception with these attributes -- present
                # only for that one failure shape, absent (None) for every
                # other prompt-turn crash, which keeps the pre-existing
                # generic message/behavior unchanged for those.
                launch_stage = getattr(exc, "cortex_launch_stage", None)
                _write_crash_diagnostic(
                    self.environ, exc, stage=launch_stage, task_id=task_id, session_id=session_id,
                    attempt_id=getattr(exc, "cortex_launch_attempt_id", None),
                )
                if launch_stage == "engine_launch":
                    # A third-party host (Paseo 0.9.2) drops this response's
                    # own ``data`` field before it reaches any log an
                    # operator can read (see gotchas.md) -- only ``message``
                    # itself is ever shown, so the stage name goes there,
                    # not only into ``data``/the durable diagnostic file.
                    message = "The engine failed to start for this task (stage: engine_launch)."
                else:
                    message = "The runtime failed while processing this prompt."
                self._reply_error(request_id, -32603, message, data={"failure": detail})
                observe(self.environ, "session/prompt", "runtime_error")
                return
        if cancel_event.is_set() and reason != StopReason.CANCELLED:
            # session/cancel MUST be reflected even if the runtime
            # otherwise reports a different outcome.
            reason = StopReason.CANCELLED
        self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason.value)))
        observe(self.environ, "session/prompt", reason.value)

    def _propose_pending_selection_for_new_task(self, store, session, task_id):
        """Apply this session's pre-task selection to the just-bootstrapped
        task, before the runtime is ever invoked (see the module docstring's
        "First prompt" rule): this session's own explicit ``pending_selection``
        if it made one, else the inherited ``default_selection`` (the
        persisted user default from a prior session). Either way the session
        field consumed is cleared afterward -- it is never shown as
        "pending" again once a real task/revision exists (see
        ``_current_selection_context``, which now reads task history
        instead). Best-effort: a failure here never fails the prompt itself
        -- the client still sees its ordinary model/thought_level defaults
        via the next ``configOptions`` read, just without this choice
        honored.

        If the saved route is no longer selectable (its connection was
        removed/deauthorized since the choice was made), this never
        auto-proposes the config_options.py substitute value as if it were
        the saved choice: the task is deliberately left without a
        Cortex-proposed revision at all, and the skip is reported via
        ``observe`` (never silent) -- see the F-minor "unavailable route"
        review finding.

        T035 FINDING-05 fix: a catalog refresh mints a new route id for an
        unchanged (connection, model) pair whenever the observed
        ``supported_parameters`` differ (e.g. per-model thought levels were
        recorded) -- see ``catalog.discovery.route_fingerprint``. The saved
        route is therefore resolved by its stable ``(connection_id, model)``
        identity (``configuration.resolution.resolve_saved_route``) against
        *this call's own freshly-read* ``entries``, never by a remembered
        id alone; a legacy ``defaults.json`` with no saved identity (written
        before this fix) is recovered once via
        ``catalog.store.find_route_identity`` scanning still-cached older
        snapshots. Either way, once resolved, the fresh ``route_id`` (and
        identity, for the legacy-recovery case) is written back to
        ``defaults.json`` so this never has to re-resolve on the next task.
        A connection+model match failure (the model was genuinely
        withdrawn) still fails closed, but now names the model instead of
        the opaque ``pending_selection_route_unavailable``.
        """
        pending = session.get("pending_selection") or session.get("default_selection")
        if pending is None:
            return False
        # The selector has already resolved the selected route's effective
        # thought level.  A route/model selection deliberately clears any
        # *previous* route's dependent settings, but must not leave that
        # effective default as presentation-only: the first coordinator
        # revision has to carry it to the downstream engine.  Read the
        # current ACP option while the pending/default selection is still
        # present, before consuming it below.
        parameters = dict(pending.get("parameters") or {})
        if "thought_level" not in parameters and not pending.get("coordinator"):
            options = self._config_options_for_session(session, store)
            thought_option = next(
                (option for option in options if option.get("id") == acp_config_options.THOUGHT_LEVEL_OPTION_ID),
                None,
            )
            thought_level = thought_option.get("currentValue") if thought_option is not None else None
            if isinstance(thought_level, str) and thought_level:
                parameters["thought_level"] = thought_level
        session["pending_selection"] = None
        session["default_selection"] = None
        entries, connections = self._selectable_entries_and_connections(session["project"])
        catalog_entry, withdrawn_model = self._resolve_pending_route(entries, connections, pending)
        if catalog_entry is None:
            if withdrawn_model is not None:
                observe(self.environ, "session/prompt", f"pending_selection_model_withdrawn:{withdrawn_model}")
            else:
                observe(self.environ, "session/prompt", "pending_selection_route_unavailable")
            return False
        if not pending.get("coordinator") and catalog_entry.route.id != pending.get("route_id"):
            self._persist_migrated_default(catalog_entry, parameters)
            observe(self.environ, "session/prompt", "pending_selection_route_migrated")
        service = ConfigurationService(store)
        inputs = ConfigurationInputs(route=catalog_entry.route, parameters=parameters)
        try:
            service.ui_select(task_id, catalog_entry, inputs, 0, session["owner_epoch"],
                              user_layer=configuration_store.read_user_defaults(self.environ),
                              project_layer=configuration_store.read_project_defaults(store, session["project"]))
        except PortableError:
            observe(self.environ, "session/prompt", "pending_selection_apply_failed")
            return False
        return True

    def _resolve_pending_route(self, entries, connections, pending):
        """T035 FINDING-05: resolve a saved pre-task selection (``pending``,
        the ``route_id``/``connection_id``/``model``/``parameters`` shape
        ``_default_selection_from_user_defaults``/``_apply_pre_task_selection``
        both produce) against ``entries`` -- this call's own freshly-read
        selectable catalog entries.

        Returns ``(catalog_entry, withdrawn_model)``: on success
        ``withdrawn_model`` is ``None`` (whether or not the id actually
        changed -- the caller compares ``catalog_entry.route.id`` against
        ``pending["route_id"]`` itself to decide whether a migration write
        is needed); on failure ``catalog_entry`` is ``None`` and
        ``withdrawn_model`` names the model that disappeared, or is itself
        ``None`` when there was not even a recoverable identity to check
        (an unknown connection/route with nothing to name).

        See ``configuration.resolution.resolve_saved_route`` for the actual
        identity match (connection_id + model, never model alone) --
        exactly what makes a different model or a different connection
        impossible to land on here by construction.
        """
        if pending.get("coordinator") is not None:
            from ..configuration.resolution import resolve_portable_coordinator
            entry = resolve_portable_coordinator(entries, pending["coordinator"])
            return entry, None
        connection_id, model = pending.get("connection_id"), pending.get("model")
        if not connection_id or not model:
            # Legacy defaults.json (written before this fix): try the
            # direct id match first (still the common case -- most saved
            # defaults are not yet stale), then a one-time reverse lookup
            # of this route id's own identity from older cached snapshots.
            route_id = pending.get("route_id")
            direct = next((entry for entry in entries if entry.route.id == route_id), None)
            if direct is not None:
                return direct, None
            from ..catalog import store as catalog_store

            identity = catalog_store.find_route_identity(connections.keys(), route_id, self.environ)
            if identity is None:
                return None, None
            connection_id, model = identity
        resolution = resolve_saved_route(entries, connection_id, model)
        return resolution.entry, resolution.withdrawn_model

    def _persist_migrated_default(self, catalog_entry, parameters) -> None:
        """T035 FINDING-05: record a saved default's migration to its
        current route id (a refresh minted a new one for the same
        connection+model) -- self-healing ``defaults.json`` so the next
        task resolves directly by id again, without needing
        :meth:`_resolve_pending_route`'s fallback path at all.

        The saved value belongs to the session that observed it.  Another
        session may explicitly select a new default while this session is
        resolving its stale route, so this is a compare-before-write under
        the registry's existing cross-process lock.  Agent-owned explicit
        default writes use the same lock (see ``_write_user_defaults``).
        """
        observed = configuration_store.read_user_defaults(self.environ)
        with connection_registry.transaction(self.environ):
            current = configuration_store.read_user_defaults(self.environ)
            if current != observed:
                return
            defaults = dict(current["defaults"])
            defaults.pop("coordinator", None)
            defaults["route_id"] = catalog_entry.route.id
            defaults["route_connection_id"] = catalog_entry.connection_id
            defaults["route_model"] = catalog_entry.route.model
            defaults["parameters"] = dict(parameters)
            configuration_store.write_user_defaults(defaults, current["ceiling"], self.environ)

    def _write_user_defaults(self, defaults, ceiling) -> None:
        """Write user defaults while coordinating with migration writes."""
        with connection_registry.transaction(self.environ):
            configuration_store.write_user_defaults(defaults, ceiling, self.environ)

    def _run_command_body(self, request_id, session_id, session, recognized, cancel_event):
        command_id, argument = recognized
        store = self._store_for(session["project"])

        def _reply(text_reply):
            # A command never streams incrementally and never calls the
            # runtime; a request that was cancelled while this synchronous
            # handling ran still gets its one, final response -- the
            # transport contract never drops a response, only intermediate
            # notifications (see agent_message_chunk emit_update elsewhere).
            if self.transport is None:
                return
            if not cancel_event.is_set():
                with self._history_lock(session_id):
                    try:
                        acp_replay_store.append_host_transcript(
                            store, session_id, "agent_message", text_reply,
                            message_id=f"assistant:command:{session_id}:{request_id}",
                        )
                    except (StoreError, OSError, ValueError, TypeError, sqlite3.Error):
                        self._reply_error(request_id, -32603, "Could not record command output.")
                        observe(self.environ, "session/prompt", "command_history_failed")
                        return
                    if not self.transport.send_notification(protocol.agent_message_chunk(
                        session_id, text_reply, f"assistant:command:{session_id}:{request_id}"
                    )):
                        cancel_event.set()
            reason = StopReason.CANCELLED.value if cancel_event.is_set() else StopReason.END_TURN.value
            self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason)))
            observe(self.environ, "session/prompt", "command:" + command_id)

        if command_id == acp_commands.CORTEX_CLEAR:
            parsed = acp_commands.parse_clear_argument(argument)
            if parsed is None:
                _reply(acp_commands.CLEAR_USAGE)
                return
            if cancel_event.is_set():
                _reply("Cortex cleanup cancelled; no deletion started.")
                return
            from ..cleanup import clear_tasks
            # Loaded sessions and durable active rows are soft protection.
            # Even force must keep the invoking session and its task.
            protected = {item.get("task_id") for item in tuple(self.sessions.values())
                         if item.get("project") == session["project"] and item.get("task_id")}
            if session.get("task_id"):
                protected.add(session["task_id"])
            def forget_deleted(kind, identity):
                # Called after each commit, under the cleanup lock. No second
                # store read can race another connection or miss partial success.
                for sid, item in tuple(self.sessions.items()):
                    if sid != session_id and item.get("project") == session["project"]:
                        if ((kind == "task" and item.get("task_id") == identity)
                                or (kind == "session" and sid == identity)):
                            self.sessions.pop(sid, None)

            try:
                result = clear_tasks(
                    store, session["project"], parsed["days"], keep_threads=(session_id,),
                    cutoff=parsed["cutoff"], dry_run=not parsed["confirm"],
                    protected_task_ids=protected, protect_active=True, force=parsed["force"],
                    on_deleted=forget_deleted,
                    keep_task_ids=(session["task_id"],) if session.get("task_id") else (),
                    keep_session_ids=(session_id,),
                    protected_session_ids=tuple(sid for sid, item in tuple(self.sessions.items())
                                                if item.get("project") == session["project"]),
                )
            except (StoreError, OSError, ValueError, sqlite3.Error):
                _reply("Cortex cleanup refused or incomplete: store busy, active-task conflict, or storage failure. No success is claimed; retry only after activity is idle and storage is healthy.")
                return
            force_text = "; force=true (assumes stale processes are dead; no OS processes killed)" if parsed["force"] else ""
            if parsed["confirm"]:
                space_text = f"; vacuum={result['vacuum_status']}; reclaimed_bytes={result['reclaimed_bytes']}"
                if "db_bytes_before" in result:
                    space_text += f"; db_bytes_before={result['db_bytes_before']}"
                if "db_bytes_after" in result:
                    space_text += f"; db_bytes_after={result['db_bytes_after']}"
                if result.get("vacuum_error"):
                    space_text += "; VACUUM failed: deletion remains committed; check free disk space before retrying reclamation."
                _reply(f"cutoff={result['cutoff']}; deleted_tasks={result['deleted_tasks']}; deleted_orphan_sessions={result['deleted_orphan_sessions']}; skipped_protected={result['skipped_protected']}" + force_text + space_text)
            else:
                _reply(f"Dry run: cutoff={result['cutoff']}; would_delete_tasks={result['would_delete_tasks']}; would_delete_orphan_sessions={result['would_delete_orphan_sessions']}; skipped_protected={result['skipped_protected']}" + force_text + f"\nConfirm: {parsed['confirm_command']}")
            return
        if command_id == acp_commands.CORTEX_STATUS:
            entries, connections = self._selectable_entries_and_connections(session["project"])
            options = self._config_options_for_session(session, store)
            _reply(acp_commands.build_status_text(
                config_options=options, connection_setup_summary=self._connection_setup_summary(connections),
                execution_mode="direct-user (no OS filesystem isolation)"))
            return
        if command_id == acp_commands.CORTEX_SETUP:
            connections = self._selectable_entries_and_connections(session["project"])[1]
            _reply(acp_commands.build_setup_text(
                connection_states=self._connection_state_counts(connections)))
            return
        if command_id in (acp_commands.CORTEX_STOP, acp_commands.CORTEX_FINISH):
            if command_id == acp_commands.CORTEX_FINISH and argument:
                _reply("Usage: /cortex-finish (no arguments)")
                return
            self._run_cortex_stop(request_id, session_id, session, cancel_event, _reply,
                                  finish=command_id == acp_commands.CORTEX_FINISH)
            return
        if command_id == acp_commands.CORTEX_ANSWER:
            self._run_cortex_answer(request_id, session_id, session, argument, cancel_event, _reply)
            return
        if command_id == acp_commands.CORTEX_NEW_TASK:
            self._run_cortex_new_task(request_id, session_id, session, argument, cancel_event)
            return
        if command_id == acp_commands.SKILL_MANAGEMENT:
            self._run_skill_management(
                request_id, session_id, session, argument, cancel_event, _reply,
            )
            return
        if command_id in (acp_commands.HARVEST, acp_commands.HARVEST_REFRESH):
            self._run_harvest(
                request_id, session_id, session, argument, cancel_event,
                refresh=(command_id == acp_commands.HARVEST_REFRESH),
            )
            return

        # cortex-model / cortex-effort / cortex-language: share the exact
        # selection code path with session/set_config_option (see
        # _apply_config_selection) -- except cortex-model, which (T035)
        # still resolves a *whole* route (any provider, by route id or
        # disambiguated label; see resolve_model_argument) rather than a
        # provider-scoped model name, and so goes through
        # _apply_route_selection (provider+model bundled) instead of the
        # single-axis config_id='model' path session/set_config_option uses.
        route_selection_entry = None
        if command_id == acp_commands.CORTEX_LANGUAGE:
            config_id = acp_config_options.LANGUAGE_OPTION_ID
            resolved = acp_commands.resolve_language_argument(argument)
            if resolved is None:
                _reply("Usage: /cortex-language auto|en|ru")
                return
        else:
            entries, _connections = self._selectable_entries_and_connections(session["project"])
            if command_id == acp_commands.CORTEX_MODEL:
                config_id = acp_config_options.MODEL_OPTION_ID
                if not entries:
                    _reply("No model is selectable yet. Use /cortex-setup to configure an authorized engine connection.")
                    return
                resolved = acp_commands.resolve_model_argument(argument, entries)
                if resolved is None:
                    _reply(f"No matching model for {argument!r}. Use /cortex-status to see the current options.")
                    return
                route_selection_entry = next(entry for entry in entries if entry.route.id == resolved)
            else:
                config_id = acp_config_options.THOUGHT_LEVEL_OPTION_ID
                resolved = argument.strip() or None
                if resolved is None:
                    _reply("/cortex-effort requires a value. Use /cortex-status to see the current options.")
                    return
                options = self._config_options_for_session(session, store)
                if not any(item["id"] == config_id for item in options):
                    _reply("No thought level is selectable for the current model. Use /cortex-status to inspect options, or /cortex-setup if no model is available.")
                    return

        if session["task_id"] is not None:
            # Durable capture first, mirroring an ordinary prompt turn (see
            # module docstring); pre-task commands never create a task and
            # record nothing durable beyond the user-default write inside
            # _apply_config_selection.
            origin = f"acp-command:{session_id}"
            source_event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
            try:
                append_source_event(store, source_event, f"/{command_id} {argument}".rstrip(), session["task_id"])
            except StoreError:
                self._reply_error(request_id, -32603, "Durable capture failed; the command was not applied.")
                observe(self.environ, "session/prompt", "capture_failed")
                return

        with session["lock"]:
            if route_selection_entry is not None:
                accepted, options, error = self._apply_route_selection(store, session, route_selection_entry)
            else:
                accepted, options, error = self._apply_config_selection(store, session, config_id, "value_id", resolved)
        if not accepted:
            self.transport.send_notification(protocol.config_option_update(session_id, options))
            message = error.message if error is not None else "The selection could not be applied."
            _reply(f"Could not apply {command_id}: {message}")
            return
        self.transport.send_notification(protocol.config_option_update(session_id, options))
        _reply(f"{command_id} set to {resolved!r} (pending — applies to the next run).")

    # -- T020 steering commands ------------------------------------------

    def _runtime_call_kwargs(self, method_name, **candidate_kwargs):
        """Only pass kwargs the bound runtime method actually declares (the
        same detect-then-pass pattern as ``project_root``/``source_origin``
        elsewhere in this class) -- lets a plain ``OrchestratingRuntime``
        (no ``project_root`` concept; used directly in tests) and the
        multi-project ``ProductionOrchestratingRuntime`` share this code."""
        method = getattr(self.runtime, method_name, None)
        if method is None:
            return None, {}
        try:
            accepted = set(inspect.signature(method).parameters)
        except (TypeError, ValueError):
            accepted = set()
        return method, {key: value for key, value in candidate_kwargs.items() if key in accepted}

    def _run_cortex_stop(self, request_id, session_id, session, cancel_event, reply, *, finish=False):
        task_id = session["task_id"]
        if task_id is None:
            reply("There is no active task on this session to close." if finish else
                  "There is no active task on this session to stop.")
            return
        source_identity = {}
        if finish:
            source_event = SimpleNamespace(origin=f"acp-command:{session_id}", event_id=str(uuid.uuid4()))
            try:
                append_source_event(self._store_for(session["project"]), source_event,
                                    "/cortex-finish", task_id)
            except StoreError:
                self._reply_error(request_id, -32603, "Durable capture failed; the task was not closed.")
                return
            source_identity = dict(stop_source="cortex_finish_command",
                                   source_origin=source_event.origin, source_event_id=source_event.event_id)
        method, kwargs = self._runtime_call_kwargs(
            "stop_task", project_root=session["project"],
            emit_tool_call=self._make_emit_tool_call(session_id, cancel_event),
        )
        if method is None:
            reply("This runtime does not support /cortex-finish." if finish else
                  "This runtime does not support /cortex-stop.")
            return
        try:
            method(task_id, session_id, lambda text: True, **kwargs, **source_identity)
        except Exception:
            self._reply_error(request_id, -32603,
                              "Failed to close the task." if finish else "Failed to stop the task.")
            observe(self.environ, "session/prompt", "cortex_stop_failed")
            return
        language = acp_messages.select_language(None, [])
        reply(acp_messages.render("force_finish_confirmed" if finish else "stop_confirmed", language))

    def _run_cortex_answer(self, request_id, session_id, session, argument, cancel_event, reply):
        task_id = session["task_id"]
        parsed = acp_commands.parse_answer_argument(argument)
        if task_id is None or parsed is None:
            reply("Usage: /cortex-answer <question-number> <answer text>")
            return
        index, text = parsed
        list_method, list_kwargs = self._runtime_call_kwargs("list_open_questions", project_root=session["project"])
        answer_method, answer_kwargs = self._runtime_call_kwargs(
            "answer_question", project_root=session["project"],
            emit_tool_call=self._make_emit_tool_call(session_id, cancel_event),
        )
        if list_method is None or answer_method is None:
            reply("This runtime does not support /cortex-answer.")
            return
        open_questions = list_method(task_id, **list_kwargs)
        if not open_questions:
            reply(acp_messages.render("question_answer_none_open", acp_messages.select_language(None, [])))
            return
        if index < 1 or index > len(open_questions):
            reply(acp_messages.render("question_answer_unknown_index", acp_messages.select_language(None, [])))
            return
        question = open_questions[index - 1]
        store = self._store_for(session["project"])
        origin = f"acp-command:{session_id}"
        source_event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
        try:
            append_source_event(store, source_event, f"/cortex-answer {argument}".rstrip(), task_id)
        except StoreError:
            self._reply_error(request_id, -32603, "Durable capture failed; the answer was not applied.")
            observe(self.environ, "session/prompt", "capture_failed")
            return

        assistant_message_id = f"assistant:command:{session_id}:{request_id}"

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            return self.transport.send_notification(protocol.agent_message_chunk(
                session_id, chunk_text, assistant_message_id,
            ))

        try:
            reason = answer_method(task_id, session_id, question, text, emit_update, cancel_event, **answer_kwargs)
        except Exception:
            self._reply_error(request_id, -32603, "Failed to deliver the answer.")
            observe(self.environ, "session/prompt", "cortex_answer_failed")
            return
        self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason.value)))
        observe(self.environ, "session/prompt", "command:" + acp_commands.CORTEX_ANSWER)

    def _run_skill_management(self, request_id, session_id, session, argument, cancel_event, reply):
        """Run one explicitly requested project skill-management workflow."""
        request = argument.strip()
        if not request:
            reply("Usage: /skill-management <request>")
            return
        from ..orchestration.skill_proposals import workflow_instruction
        instruction = "The user explicitly invoked /skill-management. " + workflow_instruction(
            "the user invoked the registered command")
        self._run_prompt_body(
            request_id, session_id, session,
            f"{request}\n\n{instruction}",
            cancel_event,
        )

    def _run_harvest(self, request_id, session_id, session, argument, cancel_event, *, refresh=False):
        """T035: `/harvest` -- an explicit knowledge-harvest request for the
        task this session is already on, or the first task in the project.

        The coordinator's role keeps a harvest explicit-only, and until this
        command existed "explicit" had nothing to express it: a model had to
        decide from prose whether the user had really asked. A command was
        pressed or it was not.

        For an existing task this is an ordinary turn, not a mechanism of its
        own: the request is recorded as the user's own message, so
        steering, continuation, language and every durable guarantee apply to
        it exactly as they do to anything else the user says.

        The first request, with or without an argument, creates the task and
        uses a complete project-documentation goal when no scope is supplied.
        An optional argument narrows that goal. It goes
        through `_run_prompt_body` so first-prompt configuration, language,
        durable capture and runtime identity handling remain identical to a
        normal first prompt. The explicit harvest instruction is appended to
        that goal and is visible to the coordinator in the same prompt turn.
        """
        store = self._store_for(session["project"])
        language = self._current_language_for_session(store, session) or "en"
        scope = argument.strip()
        key = "harvest_refresh_requested" if refresh else "harvest_requested"
        if session.get("task_id") is None:
            if scope:
                goal = scope
            elif refresh:
                goal = "Update the project's existing source-backed documentation"
            else:
                goal = "Create complete source-backed documentation for the project"
            instruction = acp_messages.render(key, language, scope=(f" ({scope})" if scope else ""))
            self._run_prompt_body(request_id, session_id, session, f"{goal}\n\n{instruction}", cancel_event)
            return
        text = acp_messages.render(
            key, language,
            scope=(f" ({scope})" if scope else ""),
        )
        self._run_prompt_body(request_id, session_id, session, text, cancel_event)

    def _run_cortex_new_task(self, request_id, session_id, session, argument, cancel_event):
        """T020: creates a brand-new, separate task from ``argument`` and
        gives it its own first coordinator turn -- ``session["task_id"]``
        (the active task) is never read or mutated here. Continuing this
        new task from a later ordinary prompt on the same ACP session is
        out of this task's scope (see docs/acp/steering-and-questions.md's
        UNKNOWNs) -- this command's own turn is this new task's only
        guaranteed turn for now."""
        text = argument.strip()
        if not text:
            if self.transport is not None:
                self.transport.send_notification(protocol.agent_message_chunk(
                    session_id, "Usage: /cortex-new-task <task text>",
                    f"assistant:command:{session_id}:{request_id}",
                ))
                self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(StopReason.END_TURN.value)))
            return
        store = self._store_for(session["project"])
        origin = f"acp-new-task:{session_id}"
        source_event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
        try:
            result = bootstrap_task(store, source_event, text, owner_ref=origin,
                                    workspace_root=session.get("workspace"))
        except StoreError:
            self._reply_error(request_id, -32603, "Durable capture failed; the new task was not created.")
            observe(self.environ, "session/prompt", "capture_failed")
            return
        new_task_id = result.task_id

        assistant_message_id = f"assistant:command:{session_id}:{request_id}"

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            return self.transport.send_notification(protocol.agent_message_chunk(
                session_id, chunk_text, assistant_message_id,
            ))

        try:
            if self._runtime_accepts_project_root and self._runtime_accepts_source_identity:
                reason = self.runtime.start_prompt(
                    new_task_id, session_id, text, emit_update, cancel_event,
                    project_root=session["project"], source_origin=origin, source_event_id=source_event.event_id)
            elif self._runtime_accepts_project_root:
                reason = self.runtime.start_prompt(new_task_id, session_id, text, emit_update, cancel_event, project_root=session["project"])
            else:
                reason = self.runtime.start_prompt(new_task_id, session_id, text, emit_update, cancel_event)
        except OrchestrationUnavailable as exc:
            self.transport.send_response(protocol.error_response(
                request_id, RpcError(-32001, "orchestration_unavailable", data={"retryable": False, "reason": str(exc) or "orchestration_unavailable"})))
            observe(self.environ, "session/prompt", "cortex_new_task_unavailable")
            return
        except Exception as exc:
            launch_stage = getattr(exc, "cortex_launch_stage", None)
            _write_crash_diagnostic(
                self.environ, exc, stage=launch_stage, task_id=new_task_id, session_id=session_id,
                attempt_id=getattr(exc, "cortex_launch_attempt_id", None),
            )
            message = ("The engine failed to start for this task (stage: engine_launch)."
                       if launch_stage == "engine_launch" else "Failed to start the new task.")
            self._reply_error(request_id, -32603, message)
            observe(self.environ, "session/prompt", "cortex_new_task_failed")
            return
        language = acp_messages.select_language(None, [])
        if self.transport is not None:
            self.transport.send_notification(protocol.agent_message_chunk(
                session_id, acp_messages.render("new_task_created", language, task_id=new_task_id),
                assistant_message_id,
            ))
            self.transport.send_response(protocol.result_response(request_id, protocol.prompt_result(reason.value)))
        observe(self.environ, "session/prompt", "command:" + acp_commands.CORTEX_NEW_TASK)

    @staticmethod
    def _portable_error_to_rpc(error: PortableError) -> RpcError:
        data = dict(code=error.code, remediation=error.remediation)
        if error.category == PortableErrorCategory.VALIDATION:
            return RpcError(-32602, error.remediation, data=data)
        return RpcError(-32001, error.remediation, data=data)

    def _apply_config_selection(self, store, session, config_id, kind, value):
        """Shared by ``session/set_config_option`` and the ``/cortex-effort``/
        ``/cortex-language`` commands (see module + ``commands.py``
        docstrings; ``/cortex-model`` instead goes through
        ``_apply_route_selection`` -- see ``_run_command_body``).

        Zed receives independent ``provider``, ``model``, and
        ``thought_level`` configIds. Paseo's model option instead carries
        exact route IDs across providers because it displays only that
        category. Changing ``provider``
        necessarily changes engine and so may change the valid model/effort
        sets: the same model *name* is kept if the new provider happens to
        offer it too, otherwise the new provider's own first model is used
        explicitly (never a silently-kept invalid model, never a silently
        dropped selection) and ``thought_level`` is always reset (a
        different route's supported values may differ entirely). Changing
        ``model`` also resets
        ``thought_level`` for the same reason; only changing
        ``thought_level`` itself preserves the current provider+model.

        Returns ``(accepted: bool, options: list[dict], error: RpcError | None)``.
        ``options`` is the *new* full configOptions list on success, or the
        *unchanged, previous* list (never a partially-applied one) on failure.
        """
        previous_options = self._config_options_for_session(session, store)
        option = next((item for item in previous_options if item["id"] == config_id), None)
        if option is None:
            return False, previous_options, RpcError(-32602, f"Unknown configId {config_id!r}.")
        if kind == "boolean":
            # No boolean-typed option is ever emitted by this agent (see
            # config_options.py); a boolean value is therefore always
            # rejected regardless of any client capability advertisement.
            return False, previous_options, RpcError(-32602, f"configId {config_id!r} does not accept a boolean value.")
        allowed_values = {item["value"] for item in option.get("options", [])}
        if value not in allowed_values:
            return False, previous_options, RpcError(-32602, f"{value!r} is not a valid value for configId {config_id!r}.")

        if config_id == acp_config_options.ACCESS_MODE_OPTION_ID:
            session_id = next((key for key, known in self.sessions.items() if known is session), None)
            return self._apply_access_mode_selection(store, session, session_id, value, "acp_config_option")

        if config_id == acp_config_options.LANGUAGE_OPTION_ID:
            # T020: an independent settings axis -- never a route/parameter
            # override, never gated on model selectability, and (unlike
            # provider/model/thought_level) never revision-gated against
            # ConfigurationService history: this value lives on a single
            # durable task_orchestration column (or, pre-task, the same
            # plain T008 user-default row provider/model/thought_level use),
            # so there is no concurrent-revision race to detect/reject here.
            return self._apply_language_selection(store, session, value)

        provider_option = next(
            (item for item in previous_options if item["id"] == acp_config_options.PROVIDER_OPTION_ID), None)
        current_connection_id = provider_option["currentValue"] if provider_option is not None else None
        model_option = next(
            (item for item in previous_options if item["id"] == acp_config_options.MODEL_OPTION_ID), None)
        current_model = model_option["currentValue"] if model_option is not None else None

        entries, _connections = self._selectable_entries_and_connections(session["project"])
        combined_model_picker = self._combined_model_picker()
        combined_values = acp_config_options.combined_model_values(entries) if combined_model_picker else {}
        if combined_model_picker and current_model is not None:
            current_entry = next(
                (entry for entry in entries if combined_values[entry.route.id] == current_model), None)
            current_model_name = current_entry.route.model if current_entry is not None else None
        else:
            current_model_name = current_model

        if config_id == acp_config_options.PROVIDER_OPTION_ID:
            provider_entries = [entry for entry in entries if entry.connection_id == value]
            target_entry = next((entry for entry in provider_entries if entry.route.model == current_model_name), None)
            if target_entry is None and provider_entries:
                target_entry = provider_entries[0]
            parameters_override: dict = {}
        elif config_id == acp_config_options.MODEL_OPTION_ID:
            target_entry = next(
                (entry for entry in entries
                 if (combined_values[entry.route.id] == value if combined_model_picker else
                     entry.connection_id == current_connection_id and entry.route.model == value)), None)
            parameters_override = {}
        else:
            target_entry = next(
                (entry for entry in entries
                 if (combined_values[entry.route.id] == current_model if combined_model_picker else
                     entry.connection_id == current_connection_id and entry.route.model == current_model)), None)
            parameters_override = {config_id: value}

        if target_entry is None:
            return False, previous_options, RpcError(-32602, "The selected route is no longer selectable.")

        if session["task_id"] is None:
            return self._apply_pre_task_selection(store, session, target_entry, parameters_override, config_id)
        return self._apply_mid_task_selection(store, session, target_entry, parameters_override)

    def _apply_route_selection(self, store, session, catalog_entry):
        """Select a whole route (provider+model together) in one step --
        used by the ``/cortex-model`` command's free-text resolution
        (which matches by full route id/label across *every* provider, see
        ``resolve_model_argument``/``_run_command_body``). The Paseo model
        picker uses the same cross-provider route identity through
        ``_apply_config_selection``.
        Always resets ``thought_level``, exactly like a protocol-level
        provider/model change."""
        if session["task_id"] is None:
            return self._apply_pre_task_selection(
                store, session, catalog_entry, {}, acp_config_options.MODEL_OPTION_ID)
        return self._apply_mid_task_selection(store, session, catalog_entry, {})

    def _apply_pre_task_selection(self, store, session, catalog_entry, parameters_override, config_id):
        """Persist the choice as the *normal* T008 user-default route/parameters
        (plain ``route_id``/``parameters`` fields -- never a Cortex-specific
        "pending" marker; see the F2 review finding) and record it on this
        session (in-memory only) as its own pending selection, shown with
        state "pending_next_use" until the first prompt consumes it (see
        ``_propose_pending_selection_for_new_task``)."""
        previous = session.get("pending_selection") or session.get("default_selection")
        if previous and config_id in (acp_config_options.PROVIDER_OPTION_ID, acp_config_options.MODEL_OPTION_ID):
            entries, connections = self._selectable_entries_and_connections(session["project"])
            current_entry, _withdrawn = self._resolve_pending_route(entries, connections, previous)
            if current_entry is not None and current_entry.route.id == catalog_entry.route.id:
                # Hosts can echo the initial model/provider without supplying
                # native parameters. This is not a route change or a reset.
                # Keep the configured values and their portable provenance.
                return True, self._config_options_for_session(session, store), None
        current = configuration_store.read_user_defaults(self.environ)
        defaults = dict(current["defaults"])
        if config_id in (acp_config_options.PROVIDER_OPTION_ID, acp_config_options.MODEL_OPTION_ID):
            merged_parameters: dict = {}
        else:
            previous = previous or {}
            merged_parameters = dict(previous.get("parameters") or {})
            merged_parameters.update(parameters_override)
        defaults.pop("coordinator", None)
        defaults["route_id"] = catalog_entry.route.id
        # T035 FINDING-05: the stable identity alongside the fingerprint id
        # -- see resolve_saved_route/_resolve_pending_route.
        defaults["route_connection_id"] = catalog_entry.connection_id
        defaults["route_model"] = catalog_entry.route.model
        defaults["parameters"] = merged_parameters
        self._write_user_defaults(defaults, current["ceiling"])
        session["pending_selection"] = dict(
            route_id=catalog_entry.route.id, parameters=merged_parameters,
            connection_id=catalog_entry.connection_id, model=catalog_entry.route.model,
        )
        session["default_selection"] = None
        return True, self._config_options_for_session(session, store), None

    def _apply_mid_task_selection(self, store, session, catalog_entry, parameters_override):
        """Propose exactly once against the caller's own view of the latest
        revision; never automatically retry on ``stale_configuration_revision``.

        An automatic retry that silently re-reads the latest revision and
        re-proposes would build its ``parameters``/route merge from state a
        concurrent session may have just changed, clobbering that change
        without ever telling either caller (a lost update -- see F1 review
        finding). Instead this always surfaces staleness to the caller as an
        explicit error carrying freshly rebuilt current options, so the
        caller re-reads and decides, rather than this method guessing.
        """
        task_id = session["task_id"]
        service = ConfigurationService(store)
        history = service.history(task_id)
        expected_revision = history[-1]["revision"] if history else 0
        inputs = ConfigurationInputs(route=catalog_entry.route, parameters=parameters_override)
        try:
            service.ui_select(task_id, catalog_entry, inputs, expected_revision, session["owner_epoch"],
                              user_layer=configuration_store.read_user_defaults(self.environ),
                              project_layer=configuration_store.read_project_defaults(store, session["project"]))
        except PortableError as violation:
            previous_options = self._config_options_for_session(session, store)
            if violation.code == "stale_configuration_revision":
                error = RpcError(-32001, "configuration changed concurrently; review current options and retry",
                                  data={"code": "stale_configuration_revision",
                                        "remediation": "configuration changed concurrently; review current options and retry"})
                return False, previous_options, error
            return False, previous_options, self._portable_error_to_rpc(violation)
        return True, self._config_options_for_session(session, store), None

    def _apply_language_selection(self, store, session, value):
        """``value`` is already validated against ``_LANGUAGE_VALUES`` by
        the caller (``_apply_config_selection``). ``"auto"`` is stored as
        ``None`` everywhere (task column and user-default row alike) --
        the same "unset means auto" convention
        ``acp.messages.select_language`` already uses."""
        normalized = None if value == "auto" else value
        task_id = session["task_id"]
        if task_id is None:
            current = configuration_store.read_user_defaults(self.environ)
            defaults = dict(current["defaults"])
            defaults["language"] = normalized
            self._write_user_defaults(defaults, current["ceiling"])
            session["pending_language"] = normalized
        else:
            acp_steering.set_language_override(store, task_id, normalized)
        return True, self._config_options_for_session(session, store), None

    def _session_set_config_option(self, request_id, params):
        session_id, config_id, kind, value = protocol.validate_set_config_option_params(params)
        session = self.sessions.get(session_id)
        if session is None:
            raise RpcError(-32602, "Unknown sessionId.")
        if (config_id == _PaseoQuestionElicitor.ANSWER_CONFIG_ID and kind == "value_id"
                and self.environ.get("CORTEX_PASEO_QUESTION_UI") == "1"):
            self._answer_question_card(request_id, session_id, session, value)
            return
        if (config_id == "cortex_queued_message" and kind == "value_id"
                and self.environ.get("CORTEX_PASEO_QUESTION_UI") == "1"):
            self._queue_paseo_message(request_id, session_id, session, value)
            return
        with session["lock"]:
            store = self._store_for(session["project"])
            accepted, options, error = self._apply_config_selection(store, session, config_id, kind, value)
        if error is not None:
            self.transport.send_response(protocol.error_response(request_id, error))
            if self.transport is not None:
                self.transport.send_notification(protocol.config_option_update(session_id, options))
            return
        self.transport.send_response(protocol.result_response(request_id, protocol.set_config_option_response(options)))
        self.transport.send_notification(protocol.config_option_update(session_id, options))

    def _start_queued_message_continuation(self, session_id, session):
        """Resume a queued message that crossed the just-finished turn boundary."""
        from ..orchestration.policy import has_pending_steering

        with session["lock"]:
            queued = session.get("queued_messages") or []
            if session["active"] is not None or session.get("cancelling") or not queued:
                return
            store = self._store_for(session["project"])
            if (acp_steering.is_stopped(store, session["task_id"])
                    or not has_pending_steering(store, session["task_id"])):
                queued.clear()
                return
            origin, event_id, text = queued.pop(0)
            cancel_event = threading.Event()
            session["active"] = cancel_event
            session["background"] = True
        worker = threading.Thread(
            target=self._run_queued_message_continuation, name="acp-queued-message",
            args=(session_id, session, origin, event_id, text, cancel_event), daemon=True,
        )
        with self._workers_lock:
            self._workers.append(worker)
        worker.start()

    def _run_queued_message_continuation(
        self, session_id, session, origin, event_id, text, cancel_event,
    ):
        store = self._store_for(session["project"])
        message_id = f"assistant:queued-message:{event_id}"

        def emit_update(chunk_text):
            if cancel_event.is_set() or self.transport is None:
                return False
            with self._history_lock(session_id):
                try:
                    acp_replay_store.append_host_transcript(
                        store, session_id, "agent_message", chunk_text, message_id=message_id,
                    )
                except (StoreError, OSError, ValueError, TypeError, sqlite3.Error):
                    return False
                return self.transport.send_notification(
                    protocol.agent_message_chunk(session_id, chunk_text, message_id),
                )

        try:
            method, kwargs = self._runtime_call_kwargs(
                "start_prompt", project_root=session["project"],
                source_origin=origin, source_event_id=event_id,
                emit_tool_call=self._make_emit_tool_call(session_id, cancel_event),
                emit_display_update=self._make_emit_display_update(session_id, cancel_event),
                host_permission_requester=self._make_permission_requester(session_id),
                host_fs_requester=self._make_fs_requester(session_id),
                host_mcp_servers=session.get("mcp_servers") or None,
                client_surface=self.client_info.get("name"),
            )
            if method is not None:
                method(session["task_id"], session_id, text, emit_update, cancel_event, **kwargs)
        except Exception:
            observe(self.environ, "session/prompt", "queued_message_continuation_failed")
        finally:
            self._finish_prompt(session_id, session, cancel_event)

    def _queue_paseo_message(self, request_id, session_id, session, value):
        """Capture a Paseo message (text, and any image/uploaded-file
        attachments) while the existing ACP turn remains in flight.

        Paseo's installed ACP bridge has no native prompt.steer. Its provider
        forwards this request through the bridge's live config channel, whose
        response is acknowledged only after durable source/event capture.
        Attachments are resolved and staged exactly as an ordinary new-turn
        ``session/prompt`` would (see ``_run_prompt_body``): the durably
        captured source event carries them, so orchestration context built
        later sees the same attachment descriptors either way.
        """
        try:
            payload = json.loads(value)
            message_id, content = payload["clientMessageId"], payload["content"]
        except (ValueError, TypeError, KeyError):
            raise RpcError(-32602, "A queued message needs clientMessageId and content.") from None
        if not isinstance(message_id, str) or not 1 <= len(message_id) <= 256:
            raise RpcError(-32602, "A queued message needs a valid message id.")
        text = protocol.content_blocks_to_text(content)
        if not text.strip():
            raise RpcError(-32602, "A queued message needs non-empty content.")
        if len(text) > 100000:
            raise RpcError(-32602, "A queued message's text exceeds Cortex's limit.")
        task_id = session.get("task_id")
        if task_id is None:
            raise RpcError(-32602, "A queued message needs an existing task.")
        store = self._store_for(session["project"])
        with store.connection() as db:
            owner_epoch = _current_owner_epoch_obj(db, task_id)
        # A loaded session captures the epoch before its next prompt. That
        # prompt may replace a lost coordinator and acquire a new epoch while
        # it is still running. Trust only this process's active, session-bound
        # coordinator channel for that exact epoch; never refresh from the
        # database alone (a different connection may now own the task).
        if owner_epoch is not None and session.get("owner_epoch") != owner_epoch:
            with session["lock"]:
                active = session["active"]
            owned_epoch = getattr(self.runtime, "active_session_owner_epoch", lambda *_: None)(
                session["project"], task_id, session_id,
            )
            if active is not None and not active.is_set() and owned_epoch == owner_epoch.epoch:
                session["owner_epoch"] = owner_epoch
        if owner_epoch is None or session.get("owner_epoch") != owner_epoch:
            raise RpcError(-32602, "This session no longer owns the task.")
        if acp_steering.is_stopped(store, task_id):
            raise RpcError(-32602, "The task has been stopped.")
        resolved_blocks = acp_replay_store.resolve_prompt_blocks(content, self._session_workspace(session))
        image_failures = [
            capture for capture in resolved_blocks
            if capture.kind == "image" and capture.availability != "stored"
        ]
        if image_failures:
            reason = image_failures[0].reason or "capture_unavailable"
            raise RpcError(-32602, "Image content could not be captured within Cortex limits.",
                           data={"reason": reason, "retryable": False})
        staged_blocks = (
            acp_replay_store.stage_blocks(session["project"], resolved_blocks) if resolved_blocks else []
        )
        staged_image_failures = [
            item.capture for item in staged_blocks
            if item.capture.kind == "image" and item.capture.availability != "stored"
        ]
        if staged_image_failures:
            reason = staged_image_failures[0].reason or "capture_unavailable"
            acp_replay_store.cleanup_staged(staged_blocks)
            raise RpcError(-32602, "Image content could not be captured within Cortex limits.",
                           data={"reason": reason, "retryable": False})
        origin = f"paseo-steer:{session_id}"
        source_event = SimpleNamespace(origin=origin, event_id=message_id)
        # Runtime-owned checksum, not client _meta or a source identity.
        # Dedup must not acknowledge changed content as an "edit" after
        # capture, even if the old item has not reached reconciliation yet.
        content_digest = hashlib.sha256(json.dumps(
            content, sort_keys=True, separators=(",", ":"), ensure_ascii=True,
        ).encode("ascii")).hexdigest()
        try:
            result = append_source_event(
                store, source_event, text, task_id, blocks=staged_blocks,
                meta={"cortex_queue_content_sha256": content_digest},
            )
        except StoreError:
            acp_replay_store.cleanup_staged(staged_blocks)
            raise RpcError(-32603, "Durable capture failed; nothing was forwarded to orchestration.") from None
        if result.replayed:
            with store.connection() as db:
                captured = db.execute(
                    "SELECT meta_json FROM source_events WHERE origin=? AND event_id=? AND task_id=?",
                    (origin, message_id, task_id),
                ).fetchone()
            try:
                captured_meta = json.loads(captured["meta_json"]) if captured else None
            except (ValueError, TypeError):
                captured_meta = None
            if (not isinstance(captured_meta, dict)
                    or captured_meta.get("cortex_queue_content_sha256") != content_digest):
                raise RpcError(-32602, "This message was already captured and cannot be edited. "
                               "Send a correction as a new message.")
        if acp_commands.recognize(content) == (acp_commands.CORTEX_FINISH, ""):
            method, kwargs = self._runtime_call_kwargs("stop_task", project_root=session["project"])
            if method is None:
                raise RpcError(-32603, "This runtime does not support /cortex-finish.")
            options = self._config_options_for_session(session, store)
            try:
                method(task_id, session_id, lambda _text: True, **kwargs,
                       stop_source="cortex_finish_command", source_origin=origin,
                       source_event_id=message_id)
            except Exception:
                raise RpcError(-32603, "Failed to close the task after durable capture.") from None
            self.transport.send_response(protocol.result_response(
                request_id, protocol.set_config_option_response(options),
            ))
            return
        # Capture and steering commit separately. A retry may be the first
        # call to record steering, so recover descriptors from durable capture,
        # not the replay's staging files (which append_source_event cleans up).
        with store.connection() as db:
            captured_blocks = db.execute(
                "SELECT b.report_id,b.block_index,b.name,b.uri,b.kind,b.availability,b.reason "
                "FROM source_blocks b JOIN source_revisions sr "
                "ON sr.task_id=b.task_id AND sr.report_id=b.report_id "
                "WHERE sr.task_id=? AND sr.message_id=? ORDER BY b.block_index",
                (task_id, f"{origin}:{message_id}"),
            ).fetchall()
        attachment_descriptors = [
            dict(source_ref=f"{b['report_id']}:{b['block_index']}", name=b["name"], uri=b["uri"],
                 kind=b["kind"], availability=b["availability"], reason=b["reason"])
            for b in captured_blocks
        ]
        acp_replay_store.record_user_message_event(
            store, task_id, f"user_message:{origin}:{message_id}", session_id,
            dict(content=text, session_id=session_id),
            ledger_origin=origin, ledger_event_id=message_id,
        )
        acp_steering.record_steering(
            store, task_id, origin, message_id, text, attachments=attachment_descriptors,
        )
        if acp_steering.is_stopped(store, task_id):
            acp_steering.supersede_undelivered_steering(store, task_id)
            raise RpcError(-32602, "The task has been stopped.")
        if not result.replayed:
            with self._history_lock(session_id):
                acp_replay_store.append_host_transcript(
                    store, session_id, "user_message", text, attachment_descriptors, message_id=message_id,
                )
        with session["lock"]:
            queued = session.setdefault("queued_messages", [])
            if not result.replayed and not any(item[1] == message_id for item in queued):
                queued.append((origin, message_id, text))
            options = self._config_options_for_session(session, store)
        self.transport.send_response(
            protocol.result_response(request_id, protocol.set_config_option_response(options))
        )
        self._start_queued_message_continuation(session_id, session)

    def _answer_question_card(self, request_id, session_id, session, value):
        """A question card's typed "Other..." answer, from Paseo.

        Paseo's ACP bridge answers a card only with one of its options, so
        the Cortex provider carries the typed text here (the bridge forwards
        any config id) and then retires the card. The text answers that
        question exactly as a message typed into the chat would."""
        try:
            batch_payload = json.loads(value)
        except (ValueError, TypeError):
            raise RpcError(-32602, 'Invalid question answer.') from None
        if isinstance(batch_payload, dict) and 'batchId' in batch_payload:
            return self._answer_question_batch_card(request_id, session_id, session, batch_payload)
        try:
            payload = json.loads(value)
            question_id, text = payload["questionId"], payload["answer"]
        except (ValueError, TypeError, KeyError):
            raise RpcError(-32602, "A question answer needs questionId and answer.") from None
        if not isinstance(question_id, str) or not isinstance(text, str) or not text.strip():
            raise RpcError(-32602, "A question answer needs questionId and a non-empty answer.")
        store = self._store_for(session["project"])
        question = acp_questions_store.get(store, question_id)
        if question is None or question.task_id != session.get("task_id"):
            raise RpcError(-32602, "Unknown question.")
        from ..orchestration.question_batches import answer_single
        if set(payload) not in ({'questionId','answer'}, {'questionId','answer','requestKey'}):
            raise RpcError(-32602, 'Unknown question answer fields.')
        owner = self._question_answer_owner(session_id, session, store)
        try:
            receipt = answer_single(store, question.task_id, question.id, text,
                                    payload.get('requestKey', 'single-card:' + question.id),
                                    session_id=session_id, owner_epoch=owner.epoch)
        except StoreError as error:
            raise RpcError(-32602, 'Question answer refused.', data={'reason': str(error)}) from None
        self._record_card_answer_history(store, session_id, [dict(questionId=question.id, answer=text)])
        if not receipt['replayed'] or self._card_answer_needs_continuation(store, question):
            threading.Thread(target=self._run_late_answer, name='acp-late-answer',
                             args=(session_id, session, question, text, True), daemon=True).start()
        with session["lock"]:
            options = self._config_options_for_session(session, store)
        self.transport.send_response(protocol.result_response(request_id, protocol.set_config_option_response(options)))

    def _answer_question_batch_card(self, request_id, session_id, session, payload):
        from ..orchestration import question_batches
        if set(payload) != {'batchId', 'answers', 'requestKey'} or not isinstance(payload['batchId'], str):
            raise RpcError(-32602, 'A batch answer needs batchId, answers and requestKey.')
        task_id = session.get('task_id')
        if not task_id:
            raise RpcError(-32602, 'Unknown question task.')
        store = self._store_for(session['project'])
        owner = self._question_answer_owner(session_id, session, store)
        try:
            receipt = question_batches.answer(
                store, task_id, payload['batchId'], payload['answers'], payload['requestKey'],
                session_id=session_id, owner_epoch=owner.epoch)
        except StoreError as error:
            raise RpcError(-32602, 'Batch answer refused.', data={'reason': str(error)}) from None
        self._record_card_answer_history(store, session_id, payload['answers'])
        question = acp_questions_store.get(store, receipt['question_ids'][0])
        if not receipt['replayed'] or self._card_answer_needs_continuation(store, question):
            threading.Thread(target=self._run_late_answer, name='acp-batch-answer',
                             args=(session_id, session, question, question.answer_text, True), daemon=True).start()
        with session['lock']:
            options = self._config_options_for_session(session, store)
        self.transport.send_response(protocol.result_response(request_id, protocol.set_config_option_response(options)))

    def _question_answer_owner(self, session_id, session, store):
        task_id = session.get('task_id')
        with store.connection() as db:
            owner = _current_owner_epoch_obj(db, task_id)
        if owner is not None and session.get('owner_epoch') != owner:
            with session['lock']:
                active = session['active']
            epoch = getattr(self.runtime, 'active_session_owner_epoch', lambda *_: None)(
                session['project'], task_id, session_id)
            if active is not None and not active.is_set() and epoch == owner.epoch:
                session['owner_epoch'] = owner
        if owner is None or session.get('owner_epoch') != owner:
            raise RpcError(-32602, 'This session no longer owns the task.')
        return owner

    def _record_card_answer_history(self, store, session_id, answers):
        # Repair display history on an exact retry after interruption. The
        # durable answer receipt already prevents repeated state changes.
        with self._history_lock(session_id):
            for item in answers:
                message_id = 'question-answer:' + item['questionId']
                with store.connection() as db:
                    exists = db.execute('SELECT 1 FROM host_transcript WHERE session_id=? AND message_id=?',
                                        (session_id, message_id)).fetchone()
                if not exists:
                    acp_replay_store.append_host_transcript(store, session_id, 'user_message', item['answer'],
                                                           message_id=message_id)

    def _card_answer_needs_continuation(self, store, question):
        from ..orchestration.worker_waits import projection
        from ..orchestration.question_batches import find_tx
        with store.connection() as db:
            task = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (question.task_id,)).fetchone()
            if task and task['state'] == 'WAITING_USER':
                return True
            batch = find_tx(db, question.task_id, question_id=question.id)
            return any(w['status'] == 'ready' for w in projection(
                db, question.task_id, question_id=batch['question_id'] if batch else question.id))

    def _session_set_mode(self, request_id, params):
        """The standard ACP mode switch sets the user's access mode
        (``guarded``/``auto``, see ``configuration.access_mode``) -- the
        modes Cortex reports to Paseo, whose composer shows them as its
        access shield. Zed and other clients reach the same choice through
        the ``access_mode`` config option. No other setting is reachable
        here; an unknown mode id is refused and changes nothing.
        """
        session_id, mode_id = protocol.validate_set_mode_params(params)
        session = self.sessions.get(session_id)
        if session is None:
            raise RpcError(-32602, "Unknown sessionId.")
        if mode_id not in configuration_access_mode.MODES:
            self.transport.send_response(protocol.error_response(request_id, RpcError(
                -32602, f"{mode_id!r} is not a valid mode; expected one of "
                        f"{list(configuration_access_mode.MODES)}.")))
            return
        with session["lock"]:
            store = self._store_for(session["project"])
            _accepted, options, _error = self._apply_access_mode_selection(
                store, session, session_id, mode_id, "acp_set_mode")
        self.transport.send_response(protocol.result_response(request_id, {}))
        self.transport.send_notification(protocol.current_mode_update(session_id, mode_id))
        self.transport.send_notification(protocol.config_option_update(session_id, options))

    def _session_load(self, request_id, params):
        """(T018) Display-only history replay -- see ``acp.replay_store``
        module docstring and ``docs/acp/input-history-and-replay.md``.

        Pure read over the durable ``acp_host_sessions``/``host_transcript``/
        ``host_updates``
        rows: never calls ``runtime.start_prompt``, never opens an engine
        session, never writes ``source_events``/``task_events``/
        ``deliveries``/reports -- this method issues no ``BEGIN IMMEDIATE``
        transaction at all. Unknown ``sessionId`` (never durably created, or
        durably created for a different ``cwd``/project) is a plain
        ``-32602`` error, exactly like an unknown ``sessionId`` on any other
        method here.
        """
        session_id, cwd, raw_mcp_servers = protocol.validate_load_session_params(params)
        try:
            workspace = canonical_project(cwd)
            project = logical_project_root(workspace)
        except StoreError:
            raise RpcError(-32602, "cwd must be an absolute, canonical, existing directory.") from None
        store = self._store_for(project)
        durable = acp_sessions.get_session(store, session_id)
        if durable is None or durable["project_root"] != project:
            raise RpcError(-32602, "Unknown sessionId.")
        # A session belongs to the worktree it was opened in; another
        # worktree of the same repository does not own it.
        recorded_workspace = task_workspace.session_root(store, session_id)
        if recorded_workspace is not None and recorded_workspace != workspace:
            raise RpcError(-32602, "Unknown sessionId.")
        try:
            history = acp_replay_store.replay_session(store, session_id)
        except ValueError as exc:
            raise RpcError(-32603, str(exc)) from None
        # The live per-task tool-card budget applies to replay too: a journal
        # written before it existed can hold thousands of coordinator cards.
        for replay_index, entry in enumerate(acp_wire_budget.fold_replayed_tool_cards(history, session_id)):
            if entry["kind"] == "update":
                # Journals written before the per-update cap can hold
                # multi-megabyte cumulative cards; replay the same bounded
                # shape a live update has today.
                replayed = acp_wire_budget.cap_session_update(entry["update"])
                queued = self.transport.send_notification(
                    protocol.notification("session/update", dict(sessionId=session_id, update={
                        **replayed, "_meta": {**replayed.get("_meta", {}),
                            "cortexReplay": {"sessionId": session_id, "sequences": entry["source_sequences"],
                                             **({"foldedCards": entry["folded_cards"]}
                                                if "folded_cards" in entry else {})}},
                    }))
                )
                if not queued:
                    raise RpcError(-32603, "Cannot enqueue complete session history.")
                continue
            text = entry["text"]
            for attachment in entry["attachments"]:
                text += (
                    f"\n[attachment: {attachment.get('name') or attachment.get('uri') or attachment.get('kind')} "
                    f"({attachment.get('availability')}) source_ref={attachment.get('source_ref')}]"
                )
            message_id = entry.get("message_id") or f"replay:{session_id}:{entry['source_sequences'][0]}"
            notification = (protocol.user_message_chunk(session_id, text, message_id)
                            if entry["kind"] == "user_message"
                            else protocol.agent_message_chunk(session_id, text, message_id))
            notification["params"]["update"]["_meta"] = {"cortexReplay": {
                "sessionId": session_id, "sequences": entry["source_sequences"],
                "textBytes": entry["source_text_bytes"],
            }}
            queued = self.transport.send_notification(notification)
            if not queued:
                raise RpcError(-32603, "Cannot enqueue complete session history.")
        # A result/cancellation can win the race with a worker turn's final
        # card update.  Replay the exact durable terminal evidence for an
        # already-present worker card so Paseo does not keep rendering it as
        # working after reconnect.  This helper is read-only: session/load
        # never mutates the transcript or task store.
        for update in acp_replay_store.terminal_worker_card_reconciliation(
            store, durable["task_id"], history,
        ):
            queued = self.transport.send_notification(
                protocol.notification("session/update", dict(sessionId=session_id, update=update)),
            )
            if not queued:
                raise RpcError(-32603, "Cannot enqueue complete session history.")
        # A plugin update closed this chat's connection and the host marked
        # it with a connection error. Show the saved task state beside it
        # (read-only; recomputed on every load until the next user message).
        from ..recovery.worker_reconciliation import host_disconnect_notice

        notice = host_disconnect_notice(store, durable["task_id"], session_id)
        if notice is not None:
            language = acp_messages.select_language(
                acp_steering.get_language_override(store, durable["task_id"]),
                acp_steering.recent_user_direct_texts(store, durable["task_id"]),
            )
            text = acp_messages.render("connection_closed_notice", language, state=notice["state"])
            if notice["state"] == "COMPLETED":
                text += acp_messages.render("connection_closed_unchanged", language)
            if notice["interrupted_workers"]:
                text += acp_messages.render("connection_closed_interrupted", language, count=notice["interrupted_workers"])
            if notice["unknown_outcome_actions"]:
                text += acp_messages.render("connection_closed_unknown_actions", language,
                                            count=notice["unknown_outcome_actions"])
            message = protocol.agent_message_chunk(session_id, text, f"cortex-connection:{notice['event_id']}")
            message["params"]["update"]["_meta"] = {"cortexConnectionNotice": {
                "taskId": notice["task_id"], "taskState": notice["state"], "disconnectEventId": notice["event_id"],
                "interruptedWorkers": notice["interrupted_workers"],
                "unknownOutcomeActions": notice["unknown_outcome_actions"],
            }}
            if not self.transport.send_notification(message):
                raise RpcError(-32603, "Cannot enqueue complete session history.")
        if session_id not in self.sessions:
            # Best-effort: lets a subsequent session/prompt on this
            # connection continue the same durable task (via
            # bootstrap.append_source_event, which needs no held channel
            # secret) without re-running session/new. Not itself asserted by
            # any T018 acceptance line -- session/load's own contract is the
            # replay above; this is only convenience continuity.
            # The task's ownership epoch as it stands at reconnect. Without
            # it every session/set_config_option on a reloaded thread was
            # refused with "missing_ownership_epoch" (live in Zed: the
            # client's declared default_config_options and the user's own
            # picks in the composer alike). Read once, here, where this
            # session's claim to the task is already established (the
            # durable row and its project match, checked above) -- never
            # re-read at proposal time, so a *later* ownership transfer
            # correctly makes this session's epoch stale and its proposal
            # refused, exactly as for a session that never disconnected.
            with store.connection() as db:
                restored_epoch = _current_owner_epoch_obj(db, durable["task_id"])
            self.sessions[session_id] = dict(
                project=project, workspace=workspace, task_id=durable["task_id"], channel=None, active=None, lock=threading.Lock(),
                owner_epoch=restored_epoch, pending_selection=None, default_selection=None,
                pending_language=None, pending_access_mode=None,
                client_capabilities=dict(self.client_capabilities),
                # ``session/load`` is a reconnect, not a new client identity.
                # The incoming client still supplies the current session's
                # MCP configuration, and that configuration must reach the
                # coordinator/workers just as it does after ``session/new``.
                # Durable storage intentionally retains only shape metadata
                # (never env/header values), so use this request-local copy.
                mcp_servers=load_mcp_servers(project, client_servers=raw_mcp_servers, environ=self.environ),
            )
        session = self.sessions[session_id]
        # A client may issue session/load on an Agent object that already has
        # the durable session (for example after a transport-level retry).
        # Refresh the request-scoped MCP configuration in that case too;
        # otherwise a changed client list would silently keep stale servers.
        session["mcp_servers"] = load_mcp_servers(
            project, client_servers=raw_mcp_servers, environ=self.environ,
        )
        # T035 FINDING-08 fix: report the same selection surface as
        # session/new, not a bare null. ``_config_options_for_session`` ->
        # ``_current_selection_context`` already does the right thing here
        # for free: with ``session["task_id"]`` set (from durable storage
        # above), it reads the task's own configuration_revisions history
        # (state "active"/etc, never "default") -- exactly the task-owned
        # configuration this reconnect must show, distinguishable from a
        # fresh session merely inheriting the current global default (state
        # "default", the ``task_id is None`` branch). A task that never got
        # an active configuration falls through to ``None`` there, which
        # ``build_config_options`` reports with nothing marked as current --
        # itself distinguishable from a task with a real selection.
        options = self._config_options_for_session(session, store)
        response = protocol.load_session_response(
            config_options=options, modes=self._access_modes_for_session(session, store))
        self.transport.send_response(protocol.result_response(request_id, response))

    def _session_cancel(self, params):
        session_id = protocol.validate_session_cancel_params(params)
        session = self.sessions.get(session_id)
        if session is None:
            return
        with session["lock"]:
            session["cancelling"] = True
            active = session["active"]
            pending = session.pop("pending_prompts", [])
            if active is not None:
                active.set()
        # Waiting requests have not been captured or acknowledged. Stop
        # cancels them too; none may auto-resume work after interruption.
        for prompt in pending:
            prompt[4].set()
            self.transport.send_response(protocol.result_response(
                prompt[0], protocol.prompt_result("cancelled")))
        try:
            # Paseo also sends session/cancel before an in-flight follow-up
            # prompt. ACP carries no signal distinguishing that steering from
            # a Stop click. Preserve the protocol's turn scope; the explicit
            # /cortex-stop command remains the task-wide terminal operation.
            self.runtime.cancel(session_id)
        except Exception:
            pass
        finally:
            with session["lock"]:
                session["cancelling"] = False

    # -- shutdown ----------------------------------------------------

    def _handle_eof(self, trigger=None):
        """Cancel every in-flight prompt; the caller (see ``main``) joins workers.

        Runs once per connection: a termination signal and the stdin EOF
        that can follow it describe the same disconnect. ``trigger`` names
        the first one observed (``sigterm``, ``stdin_eof``, ``writer_closed``)
        and is kept on the durable ``host_disconnected`` record.
        """
        from .. import _fault_injection

        with self._workers_lock:
            if getattr(self, "_disconnect_handled", False):
                return
            self._disconnect_handled = True
        _fault_injection.checkpoint("t028_disconnect_before_cancel")
        if self.host_requester is not None:
            self.host_requester.close()
        for session in list(self.sessions.values()):
            with session["lock"]:
                active = session["active"]
                for prompt in session.pop("pending_prompts", []):
                    prompt[4].set()
                if active is not None:
                    active.set()
        # T028: stop this connection's tasks from starting any new paid
        # work and run the bounded owned-resource reaper -- see
        # acp.production_runtime.ProductionOrchestratingRuntime.disconnect/
        # acp.orchestrating_runtime.OrchestratingRuntime.disconnect_session.
        # A runtime that predates T028 (UnavailableRuntime, most
        # FakeRuntime test doubles) simply has no ``disconnect`` attribute
        # -- never required, never fabricated here.
        disconnect = getattr(self.runtime, "disconnect", None)
        if disconnect is not None:
            for session_id, session in list(self.sessions.items()):
                recorded = False
                try:
                    try:
                        recorded = bool(disconnect(session_id, trigger=trigger))
                    except TypeError:
                        # A runtime double that predates ``trigger``.
                        recorded = bool(disconnect(session_id))
                except Exception:
                    pass
                if not recorded and session.get("task_id"):
                    # A session only loaded in this process (a completed chat
                    # the host restored) had no turn here; its task is still
                    # recorded so a later session/load can show the persisted
                    # outcome next to the host's connection-closed error.
                    try:
                        from ..recovery.worker_reconciliation import record_host_disconnect

                        record_host_disconnect(
                            self._store_for(session["project"]), session["task_id"], session_id,
                            f"disconnect:{session_id}:{time.monotonic_ns()}", turn="no_turn_in_process",
                            trigger=trigger,
                        )
                    except Exception:
                        pass

    def handle_writer_closed(self):
        """Called (on the transport's writer thread; see
        ``transport.StdioTransport``'s ``on_writer_closed``) when a write
        actually failed -- the client's stdout reader is gone, so this
        agent can never successfully reply to anything again, even though
        its own stdin may still be open and its reader thread still
        blocked waiting on it (T012 review item 1: without this, the
        process would wedge forever in ``join_reader()``, unable to either
        reply or exit). Treated identically to inbound EOF: cancel every
        in-flight prompt so workers unblock and ``main`` can proceed to
        shut down."""
        self._handle_eof("writer_closed")

    def join_workers(self, timeout=_PROMPT_JOIN_TIMEOUT):
        deadline = time.monotonic() + timeout
        with self._workers_lock:
            workers = list(self._workers)
        for worker in workers:
            remaining = max(0.0, deadline - time.monotonic())
            worker.join(remaining)

    def shutdown_runtime(self):
        """Release engine processes owned by this ACP daemon.

        EOF and writer-close are both normal host lifecycle boundaries.  The
        prior path only disconnected the host requesters and left the live
        engine manager to be killed externally, which left durable process
        registry rows looking active after a clean Paseo shutdown.  Runtime
        doubles used by protocol tests do not need a shutdown method, so this
        remains a capability-based call and is safe for every ACP runtime.
        """
        shutdown = getattr(self.runtime, "shutdown", None)
        if shutdown is None:
            return
        try:
            shutdown()
        except Exception:
            # Teardown must not wedge the ACP process after the host pipe is
            # already gone.  The registry/reaper will retain any ambiguous
            # resource for the next observed recovery rather than guessing.
            pass


def main(argv=None, runtime=None, environ=None):
    """Production entry point. ``argv`` is accepted but currently unused."""
    os.umask(0o077)
    environ = environ if environ is not None else os.environ
    # Redirect every non-protocol stdout use to stderr *before* constructing
    # anything else; keep a private binary handle onto the real stdout for
    # the transport's writer thread.
    raw_stdout = sys.stdout.buffer
    sys.stdout = sys.stderr

    agent = Agent(runtime if runtime is not None else UnavailableRuntime(), environ=environ)
    def trace_context(session_id):
        session = agent.sessions.get(session_id) or {}
        channel = session.get("channel")
        return dict(task_id=session.get("task_id"),
                    channel_id=getattr(channel, "channel_id", None))

    transport = StdioTransport(agent.handle_message, raw_stdout,
                               on_writer_closed=agent.handle_writer_closed,
                               trace_context=trace_context)
    agent.bind_transport(transport)
    transport.start()
    # Stop waiting on whichever happens first: the reader hits EOF (normal
    # client-initiated shutdown) or the writer stops because a write
    # actually failed (the client's stdout reader is gone -- T012 review
    # item 1). Blocking only on `join_reader()` would otherwise wedge this
    # process forever in the writer-closed-but-stdin-still-open case.
    # Paseo stops a provider (plugin update/reload) with SIGTERM, then
    # SIGKILL a second later, instead of closing stdin; the Paseo shim
    # forwards that SIGTERM here. The default action would end the process
    # before the disconnect is recorded or owned engines are released, so
    # treat it as the same host disconnect as EOF.
    if threading.current_thread() is threading.main_thread():
        signal.signal(signal.SIGTERM, lambda _signum, _frame: transport.request_shutdown())
    reason = transport.wait_for_shutdown_signal()
    if reason == "signal":
        agent._handle_eof("sigterm")
    if reason in ("writer_closed", "signal"):
        if reason == "writer_closed":
            # The writer-closed event precedes its disconnect callback.
            # Join that thread before workers or runtime teardown can race it.
            transport.stop_writer()
        agent.join_workers()
        agent.shutdown_runtime()
        if reason == "signal":
            transport.stop_writer()
        # If stdin is still open, the reader thread may be permanently
        # blocked in `readline()` with no way for this process to make it
        # return (only the peer closing its write end can do that, and
        # this process has no standing to force that). It is a daemon
        # thread, so it would not stop a normal `sys.exit`/interpreter
        # shutdown from happening either way -- but CPython's own
        # finalization sequence has a known race where a daemon thread
        # still holding a buffered stream's internal lock during shutdown
        # can abort the process with a fatal error ("could not acquire
        # lock ... at interpreter shutdown"). Exiting via `os._exit()`
        # here (there is nothing left worth flushing: the writer already
        # failed) sidesteps that finalization sequence entirely rather
        # than trying to force an impossible clean join.
        os._exit(0)
    # The EOF event precedes the reader's disconnect callback. Wait for
    # that callback to finish recording/cancelling before tearing down the
    # runtime or letting interpreter shutdown abandon the daemon thread.
    transport.join_reader()
    agent.join_workers()
    agent.shutdown_runtime()
    transport.stop_writer()
    return 0


if __name__ == "__main__":  # pragma: no cover - exercised via cortex_acp.py
    raise SystemExit(main())
