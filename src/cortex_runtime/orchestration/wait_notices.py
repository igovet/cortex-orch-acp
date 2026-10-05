"""Scheduler waits shown in the chat as runtime cards.

A planned step that cannot start (its paths are held by another worker, or
the chat's worker slots are full) used to leave the chat silent: the wait
was recorded only as a ``SCHEDULER_WAIT_REASON`` measurement event. This
module projects those recorded decisions onto one card per wait, which
names what holds the step and closes when the wait ends.

Presentation only: it reads the durable events the scheduler already
committed and never decides anything. Dependency waits are not shown;
they are the plan itself. Card text comes from ``acp.messages`` with
runtime facts as parameters (node ids, roles, scope paths, ids, times).
"""
from __future__ import annotations

from datetime import datetime
import json

from ..acp import messages
from ..observability.redact import scrub_display_text

SHOWN = frozenset({'scope_overlap', 'capacity_limit'})
_PATH_MAX_CHARS = 160


def _role(role):
    role = str(role or '?')
    return role[len('worker-'):] if role.startswith('worker-') else role


def _since(created_at):
    """The wait's start in the machine's local time, as the user reads it."""
    try:
        return datetime.fromisoformat(created_at).astimezone().strftime('%H:%M')
    except (TypeError, ValueError):
        return str(created_at or '?')


def _holder_session(db, holder_task):
    row = db.execute("SELECT session_id FROM acp_host_sessions WHERE task_id=? "
                     "ORDER BY closed_at IS NULL DESC, created_at DESC LIMIT 1", (holder_task,)).fetchone()
    return row[0] if row else '?'


def current_waits(db, task_id):
    """The latest shown wait per node of the task's current revision."""
    task = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
    if task is None:
        return {}
    latest = {}
    for row in db.execute("SELECT id,payload_json,created_at FROM runtime_events WHERE task_id=? "
                          "AND type='SCHEDULER_WAIT_REASON' AND json_extract(payload_json,'$.identity') LIKE ? "
                          "ORDER BY id", (task_id, f"node:{task[0]}:%")):
        payload = json.loads(row[1])
        latest[payload['identity']] = dict(payload, event_id=row[0], created_at=row[2])
    return {identity: wait for identity, wait in latest.items() if wait.get('reason') in SHOWN}


def render(db, task_id, wait, language):
    """(title, text) of one wait's card."""
    t = lambda key, **params: messages.render(key, language, **params)
    holder = wait.get('blocked_by') or {}
    node = wait.get('node') or '?'
    params = dict(node=node, role=_role(holder.get('role_waiting')), since=_since(wait['created_at']))
    if wait['reason'] == 'capacity_limit':
        title = t('scheduler_wait.title.capacity_limit', node=node)
        return title, t('scheduler_wait.capacity_limit', limit=holder.get('limit', '?'), **params)
    params.update(
        held=scrub_display_text(holder.get('held') or '?', max_chars=_PATH_MAX_CHARS),
        requested=scrub_display_text(holder.get('requested') or '?', max_chars=_PATH_MAX_CHARS),
        holder_task=holder.get('task_id') or '?',
        holder_node=holder.get('node') or '?',
        holder_role=_role(holder.get('role')),
        holder_state=t('scheduler_wait.holder_running' if holder.get('running')
                       else 'scheduler_wait.holder_finishing'),
    )
    if holder.get('kind') == 'access_request':
        title = t('scheduler_wait.title.scope_overlap', node=node,
                  holder=t('scheduler_wait.holder_access_request'))
        return title, t('scheduler_wait.scope_overlap.access_request', **params)
    if holder.get('task_id') == task_id:
        title = t('scheduler_wait.title.scope_overlap', node=node, holder=params['holder_node'])
        return title, t('scheduler_wait.scope_overlap.same_task', **params)
    title = t('scheduler_wait.title.scope_overlap', node=node,
              holder=f"{t('scheduler_wait.holder_other_chat')}: {params['holder_node']}")
    return title, t('scheduler_wait.scope_overlap.other_chat',
                    holder_session=_holder_session(db, holder.get('task_id')), **params)


def card_id(task_id, wait):
    return f"scheduler-wait:{task_id}:{wait['event_id']}"


def show(store, task_id, emit_tool_call, language, cards):
    """Open a card for each new wait; close the cards of waits that ended.

    ``cards`` maps a node identity to its open card (id, text) and is owned
    by the caller's turn, so one turn never repeats a card it already sent.
    A card update replaces its content, so the closing line is appended to
    the card's own explanation rather than replacing it.
    """
    if not emit_tool_call:
        return
    with store.connection() as db:
        waits = current_waits(db, task_id)
        rendered = {identity: render(db, task_id, wait, language) for identity, wait in waits.items()
                    if cards.get(identity, (None,))[0] != card_id(task_id, wait)}
        cleared = {identity: db.execute(
            "SELECT json_extract(payload_json,'$.reason') FROM runtime_events WHERE task_id=? "
            "AND type='SCHEDULER_WAIT_REASON' AND json_extract(payload_json,'$.identity')=? "
            "ORDER BY id DESC LIMIT 1", (task_id, identity)).fetchone()
            for identity in cards if identity not in waits}
    for identity, (open_card, explanation) in list(cards.items()):
        if identity in waits and open_card == card_id(task_id, waits[identity]):
            continue
        if identity in waits:
            key = 'scheduler_wait.changed'
        else:
            row = cleared.get(identity)
            key = 'scheduler_wait.resolved' if row is not None and row[0] is None else 'scheduler_wait.dropped'
        emit_tool_call(dict(tool_call_id=open_card, new=False, status='completed',
                            text=f"{explanation}\n\n{messages.render(key, language)}"))
        del cards[identity]
    for identity, (title, text) in rendered.items():
        ref = card_id(task_id, waits[identity])
        emit_tool_call(dict(tool_call_id=ref, new=True, title=title, kind='other',
                            status='in_progress', text=text))
        cards[identity] = (ref, text)


def close_all(task_id, emit_tool_call, language, cards):
    """End the turn's open wait cards; the wait itself stays with the task."""
    if emit_tool_call:
        for open_card, explanation in cards.values():
            emit_tool_call(dict(tool_call_id=open_card, new=False, status='completed',
                                text=f"{explanation}\n\n{messages.render('scheduler_wait.turn_ended', language)}"))
    cards.clear()


def holder_alive(db, holder):
    """Whether the worker holding a scope wait is observed alive right now.

    Only a recorded worker process with a matching identity counts
    (``recovery.registry.probe_liveness``); a missing record, an
    unreadable process or a confirmed exit is not alive.
    """
    from ..recovery.registry import probe_liveness
    ref = holder.get('assignment_ref')
    if not ref:
        return False
    row = db.execute("SELECT r.* FROM delegation_provisioning p JOIN attempt_process_registry r "
                     "ON r.attempt_id=p.attempt_id AND r.role='worker' "
                     "WHERE p.task_id=? AND p.idempotency_key=?",
                     (holder.get('task_id'), ref)).fetchone()
    return row is not None and probe_liveness(dict(row)) == 'alive'


def waiting_on_live_holders(store, task_id):
    """Every shown wait of the task is a scope wait on a live worker.

    Such a wait ends when that worker does, so the scheduler keeps waiting
    instead of stopping the task on its deadline. A capacity wait, or a
    scope wait whose holder is gone, finishing or unverifiable, does not
    qualify: those keep the deadline that bounds a release never observed.
    """
    with store.connection() as db:
        waits = current_waits(db, task_id)
        return bool(waits) and all(
            wait['reason'] == 'scope_overlap'
            and (wait.get('blocked_by') or {}).get('kind') == 'worker'
            and holder_alive(db, wait['blocked_by'])
            for wait in waits.values())
