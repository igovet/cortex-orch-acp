"""Telling the user when a requested model was not used.

The coordinator told the owner that Opus would own an audit; the router
ignored that hint (``ignored:owner_preferred``) and the audit ran on Sol,
visible only in the route audit (observed 2026-10-22). Each worker attempt
whose model hint was ignored now gets one plain line in the chat.
"""
from __future__ import annotations

import json

from .state import event

_REASONS = {
    'pinned': ('a different model is pinned in the routing settings', 'в настройках маршрутов закреплена другая модель'),
    'owner_preferred': ('the routing settings prefer another model for this work',
                        'настройки маршрутов предпочитают для этой работы другую модель'),
    'tier_mismatch': ("the model's level does not match this work in the routing settings",
                      'уровень модели в настройках не подходит для этой работы'),
    'escalated': ('the work was escalated to a stronger model', 'работу передали более сильной модели'),
    'provider_not_allowed': ('its engine is not allowed for this task', 'её движок не разрешён для этой задачи'),
    'provider_not_configured': ('its engine is not configured', 'её движок не настроен'),
    'not_offered': ('it is not enabled in the routing settings', 'она не включена в настройках маршрутов'),
    'auto_excluded': ('it is excluded from automatic routing', 'она исключена из автоматического выбора'),
    'superseded': ('a newer request replaced it', 'её заменила более новая просьба'),
    'unavailable': ('it is unavailable right now', 'сейчас она недоступна'),
    'provider_unavailable': ('its engine is unavailable right now', 'её движок сейчас недоступен'),
    'no_candidate_in_tier': ('no model of that level is available', 'нет доступной модели этого уровня'),
    'near_quota_limit': ('its plan is close to its usage limit', 'её тариф близок к лимиту'),
    'provider_near_quota_limit': ('its plan is close to its usage limit', 'её тариф близок к лимиту'),
}


def _ignored(db, task_id):
    shown = {json.loads(row[0]).get('attempt_id') for row in db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='MODEL_HINT_IGNORED_SHOWN'", (task_id,))}
    found = []
    for row in db.execute("SELECT attempt_id, role, engine, model, decision_detail_json FROM attempt_route_audit "
                          "WHERE task_id=? AND decision_detail_json LIKE '%ignored:%' ORDER BY recorded_at",
                          (task_id,)).fetchall():
        if row['attempt_id'] in shown:
            continue
        try:
            decision = (json.loads(row['decision_detail_json'] or '{}').get('route_decision') or {})
        except (TypeError, ValueError):
            continue
        hint, status = decision.get('model_hint'), decision.get('model_hint_status') or ''
        if hint and hint != 'auto' and status.startswith('ignored:'):
            found.append((row, hint, status))
    return found


def ignored_hint_notices(store, task_id, language):
    """One chat line per not-yet-announced attempt whose model hint was
    ignored, recorded so it is shown once. The idle controller loop only
    reads; a write transaction opens only when there is something to say."""
    with store.connection() as db:
        if not _ignored(db, task_id):
            return []
    lines = []
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        for row, hint, status in _ignored(db, task_id):
            reason = status.split(':', 1)[1]
            en, ru = _REASONS.get(reason, (reason.replace('_', ' '), reason.replace('_', ' ')))
            role = (row['role'] or 'worker').replace('worker-', '')
            used = f"{row['engine']}/{row['model']}"
            lines.append(f'Для «{role}» была запрошена модель {hint}, но запущена {used}: {ru}.' if language == 'ru'
                         else f'{hint} was requested for {role}, but {used} runs it: {en}.')
            event(db, task_id, 'MODEL_HINT_IGNORED_SHOWN', dict(attempt_id=row['attempt_id'], hint=hint,
                                                                status=status, used=used))
        db.commit()
    return lines
