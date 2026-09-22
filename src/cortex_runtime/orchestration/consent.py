"""The user's consent for a worker action the sandbox cannot allow on its own.

A worker writes the whole project; outside it the sandbox is read-only, and a
deletion is the user's to confirm. The worker's MCP server (its own process)
records a request here and waits; the task's controller, which runs for as
long as its workers do, shows the user a permission card and records the
answer. Anything but an explicit allow -- a refusal, no answer, a stopped or
revoked worker -- means the action does not happen.
"""
from __future__ import annotations

import json
import threading
import time

from ..portable.identity import new_id
from .state import now

SCHEMA = '''CREATE TABLE IF NOT EXISTS consent_requests (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL, attempt_id TEXT, kind TEXT NOT NULL,
 subject TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, decided_at TEXT
)'''
#: How long a worker waits for the person.
TIMEOUT_SECONDS = 1800.0


def _ensure(db):
    db.execute(SCHEMA)


def request(store, channel, kind: str, subject: str, *, still_allowed=None, timeout: float = TIMEOUT_SECONDS) -> bool:
    """Block until the user allows (True) or not (False). ``still_allowed``
    (optional, raises when the worker lost its authority) is re-checked while
    waiting, so a stopped task never leaves an approval pending."""
    request_id = new_id('cr_')
    with store.connection() as db:
        _ensure(db)
        db.execute('INSERT INTO consent_requests VALUES(?,?,?,?,?,?,?,NULL)',
                   (request_id, channel.task_id, channel.attempt_id, kind, subject, 'pending', now()))
        db.commit()
    deadline = time.monotonic() + timeout
    checked = 0.0
    while time.monotonic() < deadline:
        with store.connection() as db:
            status = db.execute('SELECT status FROM consent_requests WHERE id=?', (request_id,)).fetchone()[0]
        if status in ('approved', 'denied'):
            return status == 'approved'
        if still_allowed is not None and time.monotonic() - checked > 2:
            checked = time.monotonic()
            try:
                still_allowed()
            except Exception:  # noqa: BLE001 - lost authority is a refusal
                break
        time.sleep(0.3)
    decide(store, request_id, False)
    return False


def decide(store, request_id: str, approved: bool) -> None:
    with store.connection() as db:
        _ensure(db)
        db.execute("UPDATE consent_requests SET status=?, decided_at=? WHERE id=? AND status IN ('pending','asked')",
                   ('approved' if approved else 'denied', now(), request_id))
        db.commit()


def _claim_pending(store, task_id: str) -> list[dict]:
    with store.connection() as db:
        _ensure(db)
        db.execute('BEGIN IMMEDIATE')
        rows = [dict(row) for row in db.execute(
            "SELECT * FROM consent_requests WHERE task_id=? AND status='pending' ORDER BY created_at", (task_id,))]
        for row in rows:
            db.execute("UPDATE consent_requests SET status='asked' WHERE id=?", (row['id'],))
        db.commit()
    return rows


_TITLES = {
    'outside_write': {'en': 'Write outside the project: {subject}', 'ru': 'Запись вне проекта: {subject}'},
    'outside_command': {'en': 'Run outside the sandbox: {subject}', 'ru': 'Команда вне песочницы: {subject}'},
    'project_delete': {'en': 'Delete from the project: {subject}', 'ru': 'Удаление из проекта: {subject}'},
}


def relay(runtime, task_id: str, language: str) -> None:
    """Show every new request of ``task_id`` to the user (non-blocking)."""
    for row in _claim_pending(runtime._store, task_id):
        threading.Thread(target=_ask, args=(runtime, task_id, row, language), name='cortex-consent', daemon=True).start()


def _ask(runtime, task_id, row, language) -> None:
    requester = runtime.permission_requester_for_task(task_id)
    if requester is None:
        decide(runtime._store, row['id'], False)
        return
    template = _TITLES.get(row['kind'], {}).get(language) or _TITLES.get(row['kind'], {}).get('en') or '{subject}'
    params = dict(
        toolCall=dict(toolCallId=f"cortex-consent:{row['id']}", kind='delete' if row['kind'] == 'project_delete' else 'other',
                      title=template.format(subject=row['subject'][:600]),
                      rawInput=dict(kind=row['kind'], subject=row['subject'])),
        options=[dict(optionId='allow', name='Allow' if language != 'ru' else 'Разрешить', kind='allow_once'),
                 dict(optionId='deny', name='Deny' if language != 'ru' else 'Запретить', kind='reject_once')],
    )
    try:
        response = requester(None, params, TIMEOUT_SECONDS)
    except Exception:  # noqa: BLE001 - no answer is a refusal
        response = None
    outcome = (response or {}).get('outcome') if isinstance(response, dict) else None
    approved = isinstance(outcome, dict) and outcome.get('outcome') == 'selected' and outcome.get('optionId') == 'allow'
    decide(runtime._store, row['id'], approved)


def subject_of(kind: str, args: dict) -> str:
    if kind == 'outside_command':
        return ' '.join(json.dumps(a) if ' ' in a else a for a in args['argv']) + (f"  (cwd {args['cwd']})" if args.get('cwd') else '')
    return args['path']
