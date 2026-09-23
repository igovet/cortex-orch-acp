"""Runtime-published initial pipeline: the store owns the record, the model
owns only the decisions (see task card).

Previously a coordinator's very first turn had to spend a model turn and
several MCP round trips (``create_draft`` -> edit the file with native tools
-> ``write_report``) before any delegation could start. :func:`publish_initial_pipeline`
does that same publication itself, through the exact same ``create_draft``/
``write_report`` store operations a coordinator channel would use (never by
writing tables or files directly -- see ``store.Store.call``), so the audit
trail is indistinguishable in kind from a model-published one except for its
``author``.

Idempotent and crash-safe: gated on the durable ``reports`` row for this
task's single pipeline (``one_pipeline`` unique index in ``store.SCHEMA``),
never an in-process flag -- a second first-turn (recovery, replacement
session) observes that row already exists and publishes nothing.

Never blocks the turn: any failure here is caught, durably recorded (an
``observability_events`` row and a best-effort ``progress`` task event), and
swallowed -- a task that cannot publish a pipeline is still a task the user
asked for (see this module's only caller, ``OrchestratingRuntime._run``).
"""
from __future__ import annotations

import os
from pathlib import Path
from typing import Any, Mapping

from ..catalog.labels import engine_short_name, model_display_name

_NOT_DETERMINED_SECTION = (
    "Not yet determined: this section is model-owned guidance the runtime "
    "does not fill in; the coordinator has not yet run."
)


def _request_summary(text: str | None, limit: int = 3600) -> str:
    text = (text or "").strip()
    if not text:
        return "The user's original request text is not recorded for this task."
    if len(text) > limit:
        text = text[: limit - 3] + "..."
    return "> " + text.replace("\n", "\n> ")


def _route_line(route_info: Mapping[str, Any] | None) -> str:
    if not route_info:
        return "The bound engine, model and effort for this task's coordinator route are not yet determined."
    engine = engine_short_name(str(route_info.get("engine") or "not yet determined"))
    model_id = str(route_info.get("model") or "not yet determined")
    model = route_info.get("model_name") or model_display_name(model_id)
    if model == model_id and model_id != "not yet determined":
        model = f"{model_id} (alias)"
    effort = route_info.get("effort") or "not recorded for this route"
    return f"Bound coordinator route: engine `{engine}`, model `{model}`, effort `{effort}`."


def _placeholder_replacements(task_id: str, request_text: str | None, route_info: Mapping[str, Any] | None) -> dict:
    objective = (
        f"Task `{task_id}` was just opened from this exact user request:\n\n"
        f"{_request_summary(request_text)}\n\n"
        "Status: not yet worked. This initial edition was published by the "
        "runtime itself, before any coordinator turn ran."
    )
    assignments = _route_line(route_info) + " No assignment has been delegated yet."
    return {
        "{{CURRENT_OBJECTIVE_AND_DECISION_BASIS}}": objective,
        "{{CURRENT_REQUIREMENTS_AND_CONSTRAINTS}}": (
            "Not yet determined: the coordinator has not yet reviewed the request for explicit "
            "requirements or constraints. The user's own words above are the only source of truth "
            "until it does."
        ),
        "{{CURRENT_WORK_GRAPH}}": "Not yet determined: no work has been planned or delegated yet.",
        "{{CURRENT_ASSIGNMENTS_AND_ROUTING}}": assignments,
        "{{CURRENT_EVIDENCE_AND_VERIFICATION}}": "Not yet determined: no work has been checked or verified yet.",
        "{{CURRENT_DECISIONS_QUESTIONS_AND_REMAINING_WORK}}": (
            "Not yet determined: no decisions have been made and no work remains scoped yet."
        ),
    }


def _fill_template(markdown: str, task_id: str, request_text: str | None, route_info: Mapping[str, Any] | None) -> str:
    """Replace every marker ``create_draft`` reported as ``replaceable_markers``
    (both the six ``{{CURRENT_*}}`` placeholders and every ``<!-- ... -->``
    guidance comment) with honest content -- never inventing what the
    runtime does not actually know, and never leaving a raw marker behind
    (``write_report`` rejects that -- see ``store.Store._publish_source``).
    """
    replacements = _placeholder_replacements(task_id, request_text, route_info)
    out = []
    for line in markdown.splitlines(keepends=True):
        body = line[:-1] if line.endswith("\n") else line
        newline = "\n" if line.endswith("\n") else ""
        if body in replacements:
            out.append(replacements[body] + newline)
        elif body.startswith("<!--") and body.endswith("-->"):
            out.append(_NOT_DETERMINED_SECTION + newline)
        else:
            out.append(line)
    return "".join(out)


def _write_draft_body(draft_path: str, markdown: str) -> None:
    """Overwrite the server-created draft file in place -- same identity
    (device/inode) a native file tool's in-place edit would leave, which is
    exactly what ``store.Store._draft_source`` requires before publication."""
    path = Path(draft_path)
    fd = os.open(path, os.O_WRONLY | os.O_TRUNC | os.O_NOFOLLOW)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(markdown.encode("utf-8"))
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        raise


def _pipeline_already_published(store, task_id: str) -> bool:
    with store.connection() as db:
        row = db.execute(
            "SELECT 1 FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()
    return row is not None


def _record_failure(store, task_id: str, error: BaseException) -> None:
    from ..delegation import events as delegation_events
    from ..observability import events as observability_events

    detail = dict(reason=str(error), error_type=type(error).__name__)
    try:
        observability_events.append_event(
            store, task_id, "pipeline_bootstrap", "failed", "observed", detail=detail,
        )
    except Exception:
        pass
    try:
        delegation_events.append_event(
            store, task_id, "progress", f"pipeline_bootstrap_failed:{task_id}",
            dict(note="runtime-published initial pipeline failed", **detail),
            origin="runtime",
        )
    except Exception:
        pass


def publish_initial_pipeline(store, task_id: str, channel, request_text: str | None,
                              route_info: Mapping[str, Any] | None = None) -> None:
    """Publish this task's initial pipeline edition through the coordinator's
    own trusted channel, with no model turn involved.

    ``channel`` is the task's already-resolved coordinator
    :class:`trusted_channel.ChannelContext` (see
    ``OrchestratingRuntime._ensure_coordinator``); ``store.call`` is invoked
    exactly the way ``server.Server._dispatch_channeled`` invokes it for a
    real MCP tool call (``trusted=True``, the channel's own principal/parent
    thread ids) -- never by writing the ``drafts``/``reports``/``editions``
    tables or task files directly.

    A no-op, safely, whenever this task already has a pipeline report (the
    durable idempotency check -- see this module's docstring) or when
    publication fails for any reason; failure is recorded, never raised.
    """
    if _pipeline_already_published(store, task_id):
        return
    try:
        draft = store.call(
            "create_draft", dict(template="pipeline"),
            channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
        )
        filled = _fill_template(draft["markdown"], task_id, request_text, route_info)
        _write_draft_body(draft["draft_path"], filled)
        store.call(
            "write_report",
            dict(
                draft_id=draft["draft_id"],
                title="Initial pipeline",
                summary="Runtime-published initial pipeline; the coordinator has not yet run.",
                author="runtime",
                request_key=f"runtime-initial-pipeline:{draft['draft_id']}",
            ),
            channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
        )
    except Exception as error:
        _record_failure(store, task_id, error)
