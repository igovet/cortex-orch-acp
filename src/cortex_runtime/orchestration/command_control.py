"""User-only control of exact gateway command processes, using format-12 events.

This module is not a model tool. Its callers are the ACP host input boundary and
the local owner CLI. A started row is live/unresolved, not proof of OS liveness
after a runtime crash; only the runner can confirm a stop.
"""
from __future__ import annotations

import json
import re
import shlex

from ..contracts import StoreError
from ..observability.redact import scrub_text
from .state import event, now

CONFIG_ID = "cortex_stop_command"
MAX_REASON = 2000
_ORIGINS = {"acp_command", "paseo_config", "owner_cli"}


def validate_stop(tool_call_ref, reason, request_key):
    if not isinstance(tool_call_ref, str) or not re.fullmatch(r"gateway:[0-9a-f]{32}", tool_call_ref):
        raise StoreError("command_stop_invalid_ref")
    if not isinstance(reason, str) or not reason.strip() or len(reason) > MAX_REASON:
        raise StoreError("command_stop_invalid_reason", "reason", None,
                         "non-empty text, at most 2000 Unicode characters")
    if not isinstance(request_key, str) or not request_key.strip() or len(request_key) > 256:
        raise StoreError("command_stop_invalid_request_key")
    return reason.strip()


def _event(db, task_id, kind, ref):
    row = db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? "
        "AND json_extract(payload_json,'$.tool_call_ref')=? ORDER BY id LIMIT 1",
        (task_id, kind, ref),
    ).fetchone()
    return json.loads(row[0]) if row else None


def pending_stop(db, task_id, ref):
    return _event(db, task_id, "COMMAND_STOP_REQUESTED", ref) if ref else None


def _live_commands(db, task_id):
    return db.execute(
        "SELECT e.payload_json,e.created_at,c.channel_id,c.assignment_id,c.attempt_id,"
        "p.idempotency_key AS assignment_ref FROM runtime_events e "
        "JOIN trusted_channels c ON c.channel_id=e.actor AND c.task_id=e.task_id "
        "JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch "
        "JOIN runtime_tasks t ON t.task_id=e.task_id "
        "LEFT JOIN delegation_provisioning p ON p.channel_id=c.channel_id AND p.task_id=c.task_id "
        "WHERE e.task_id=? AND e.type='COMMAND_PROCESS_STARTED' "
        "AND c.role='worker' AND c.revoked_at IS NULL "
        "AND (c.expires_at IS NULL OR c.expires_at>?) "
        "AND t.state NOT IN ('COMPLETED','FAILED','CANCELLED') "
        "AND NOT EXISTS (SELECT 1 FROM runtime_events x WHERE x.task_id=e.task_id "
        "AND x.type='COMMAND_PROCESS_EXITED' AND json_extract(x.payload_json,'$.tool_call_ref')="
        "json_extract(e.payload_json,'$.tool_call_ref'))",
        (task_id, now()),
    ).fetchall()


def list_commands(store, task_id, *, assignment_ref=None):
    with store.connection() as db:
        if not db.execute("SELECT 1 FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone():
            raise StoreError("runtime_task_missing")
        result = []
        for row in _live_commands(db, task_id):
            if assignment_ref is not None and row["assignment_ref"] != assignment_ref:
                continue
            data = json.loads(row["payload_json"])
            result.append(dict(tool_call_ref=data["tool_call_ref"], operation=data["operation"],
                               assignment_ref=row["assignment_ref"], assignment_id=row["assignment_id"],
                               attempt_id=row["attempt_id"], started_at=row["created_at"],
                               argv_summary=scrub_text(shlex.join(data["argv"]), max_chars=500)[:500], state="running_or_unresolved"))
        return result


def request_stop(store, task_id, tool_call_ref, reason, request_key, *, origin,
                 session_id=None, owner_epoch=None):
    # An origin label is routing validation, not a hostile-code security boundary.
    # No MCP operation dispatches here or accepts a user-origin claim.
    if origin not in _ORIGINS:
        raise StoreError("command_stop_user_only")
    reason = validate_stop(tool_call_ref, reason, request_key)
    actor = "user:owner_cli" if origin == "owner_cli" else f"user:{origin}:{session_id}"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        if origin != "owner_cli":
            bound = db.execute(
                "SELECT 1 FROM acp_host_sessions s JOIN task_owners o ON o.task_id=s.task_id "
                "WHERE s.session_id=? AND s.task_id=? AND s.closed_at IS NULL AND o.epoch=?",
                (session_id, task_id, owner_epoch),
            ).fetchone()
            if not bound:
                raise StoreError("command_stop_session_not_owner")
        # A retry key cannot target another command or change the accepted reason.
        prior = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='COMMAND_STOP_REQUESTED' "
            "AND actor=? AND json_extract(payload_json,'$.request_key')=?",
            (task_id, actor, request_key),
        ).fetchone()
        if prior:
            data = json.loads(prior[0])
            if data["tool_call_ref"] != tool_call_ref or data["reason"] != reason:
                raise StoreError("request_key_conflict")
            return data
        prior = pending_stop(db, task_id, tool_call_ref)
        if prior:
            return prior
        matches = [row for row in _live_commands(db, task_id)
                   if json.loads(row["payload_json"])["tool_call_ref"] == tool_call_ref]
        if len(matches) != 1:
            raise StoreError("command_stop_not_running", "tool_call_ref", tool_call_ref,
                             "one running worker command in this task",
                             "Unknown, finished, stale or ambiguous command; nothing was stopped.")
        row = matches[0]
        data = dict(accepted=True, outcome="stop_requested", task_id=task_id, tool_call_ref=tool_call_ref,
                    reason=reason, request_key=request_key, origin=origin,
                    channel_id=row["channel_id"], assignment_ref=row["assignment_ref"],
                    attempt_id=row["attempt_id"], operation=json.loads(row["payload_json"])["operation"])
        event(db, task_id, "COMMAND_STOP_REQUESTED", data, actor)
        db.commit()
        return data


def stopped_receipt(value, depth=0):
    """Find a candidate "stopped by user" shape inside the supported MCP
    mirrors. This alone is never trust: it only recognizes the shape, which
    a worker can fabricate (for example by writing and reading back a
    crafted file). A caller MUST also confirm the candidate's
    ``tool_call_ref`` against this exact tool call's own identity and a real
    durable record (see ``confirmed_stop``) before presenting it as a
    genuine stop.
    """
    if depth > 8:
        return None
    if isinstance(value, str):
        try:
            value = json.loads(value)
        except (ValueError, TypeError):
            return None
    if isinstance(value, dict):
        if (value.get("failure") == "stopped_by_user"
                and isinstance(value.get("stop_reason"), str)
                and isinstance(value.get("tool_call_ref"), str)):
            return value
        for key in ("structuredContent", "result", "content", "text"):
            if key in value:
                found = stopped_receipt(value[key], depth + 1)
                if found:
                    return found
    elif isinstance(value, list):
        for item in value:
            found = stopped_receipt(item, depth + 1)
            if found:
                return found
    return None


def confirmed_stop(db, task_id, tool_call_ref, *, attribute_to=None):
    """A verified real stop of this exact gateway command, or ``None``.

    Only the gateway's own command-process loop -- never a worker's tool
    output -- writes a ``COMMAND_STOPPED`` event, and only after it has
    actually killed the process. A worker can fabricate the *shape*
    ``stopped_receipt`` recognizes (for example by writing and reading back
    a crafted file naming any ``tool_call_ref`` it likes), but it cannot
    fabricate this durable record for a ref that was never really stopped.
    The caller must present the reason from this confirmed record, never
    the candidate's own claimed ``stop_reason`` text.

    ``attribute_to`` is this exact row's own display identity (the ACP
    engine's per-call ref). A genuine stop can still be *replayed*: a
    worker whose own command A was really stopped can echo A's already-
    observed receipt (naming A's real ``tool_call_ref`` and reason) into an
    unrelated, never-stopped command B's own output, since the gateway's
    ``tool_call_ref`` and the ACP engine's per-call ref are different,
    unrelated identifier namespaces with no existing mapping between them.
    When ``attribute_to`` is given, this one real stop is durably bound, on
    first use, to the single row that first presents it; a later call
    naming a *different* ``attribute_to`` for the same ``tool_call_ref`` is
    refused (returns ``None``) even though the underlying record is real.
    """
    if not isinstance(tool_call_ref, str) or not tool_call_ref:
        return None
    record = _event(db, task_id, "COMMAND_STOPPED", tool_call_ref)
    if record is None or attribute_to is None:
        return record
    attribute_to = str(attribute_to)
    db.execute("BEGIN IMMEDIATE")
    shown = _event(db, task_id, "COMMAND_STOP_SHOWN", tool_call_ref)
    if shown is None:
        event(db, task_id, "COMMAND_STOP_SHOWN",
              dict(tool_call_ref=tool_call_ref, shown_ref=attribute_to), "system")
        db.commit()
        return record
    db.commit()
    return record if shown["shown_ref"] == attribute_to else None


def stopped_fields(effects):
    reason = effects.get("stop_reason")
    if reason is None:
        return {}
    return dict(outcome="stopped_by_user", stop_reason=reason,
                message=f"Stopped by user: {reason}\nContinue your assignment, taking the user's reason "
                        "into account. Do not re-run the same command unchanged; use another route "
                        "or ask the user if blocked.")
