"""Telling the user when a requested model was not used.

The coordinator told the owner that Opus would own an audit; the router
ignored that hint (``ignored:owner_preferred``) and the audit ran on Sol,
visible only in the route audit (observed 2026-10-22). Each worker attempt
whose model hint differs from the effective dispatch gets one chat line.
An ignored advisory input can still match a pin; the audit keeps its provenance.
"""
from __future__ import annotations

import json

from .state import event

_REASONS = {
    'pinned': ('the route is pinned in the routing settings', 'в настройках маршрутов закреплён этот маршрут'),
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


def _detail(row):
    try:
        detail = json.loads(row['decision_detail_json'] or '{}')
    except (TypeError, ValueError):
        return {}
    return detail if isinstance(detail, dict) else {}


def _decision(detail):
    decision = detail.get('route_decision')
    return decision if isinstance(decision, dict) else {}


def _actual(rows):
    """Audit-bound dispatch identity, not proof of native configuration.
    Conflicting or incomplete bindings must never establish a matching route."""
    identities = set()
    for row in rows:
        engine, model, connection = row['engine'], row['model'], row['connection_id']
        if not all(isinstance(value, str) and value for value in (engine, model, connection)):
            return None
        detail = _detail(row)
        decision = _decision(detail)
        route = f'{engine}/{model}'
        if (decision.get('chosen', route) != route
                or decision.get('chosen_connection_id', connection) != connection):
            return None
        effort = detail.get('effort')
        if effort is not None and not isinstance(effort, str):
            return None
        identities.add((route, effort or None, connection))
    if len(identities) != 1:
        return None
    route, effort, _ = identities.pop()
    return route, effort


def _ignored(db, task_id):
    shown = {json.loads(row[0]).get('attempt_id') for row in db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='MODEL_HINT_IGNORED_SHOWN'", (task_id,))}
    attempts = {}
    for row in db.execute("SELECT attempt_id, role, engine, model, connection_id, decision_detail_json "
                          "FROM attempt_route_audit WHERE task_id=? ORDER BY recorded_at", (task_id,)):
        attempts.setdefault(row['attempt_id'], []).append(row)
    found = []
    for attempt_id, rows in attempts.items():
        if attempt_id in shown:
            continue
        actual = _actual(rows)
        for row in rows:
            decision = _decision(_detail(row))
            hint, status = decision.get('model_hint'), decision.get('model_hint_status')
            if not (isinstance(hint, str) and hint and hint != 'auto'
                    and isinstance(status, str) and status.startswith('ignored:')):
                continue
            request = decision.get('user_model_request')
            requested_effort = request.get('effort') if isinstance(request, dict) else None
            if actual:
                route, effort = actual
                engine, _, _ = route.partition('/')
                if (hint in (route, engine)
                        and (requested_effort is None or requested_effort == effort)):
                    continue
            found.append((row, hint, status, requested_effort, actual))
            break  # One notice per attempt, including duplicate audit rows.
    return found


def ignored_hint_notices(store, task_id, language):
    """One durable notice per differing or unverifiable ignored hint.
    Matching pins remain silent without changing route provenance. The idle
    loop only reads; a write transaction opens only when there is a notice."""
    with store.connection() as db:
        if not _ignored(db, task_id):
            return []
    lines = []
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        for row, hint, status, requested_effort, actual in _ignored(db, task_id):
            reason = status.split(':', 1)[1]
            en, ru = _REASONS.get(reason, (reason.replace('_', ' '), reason.replace('_', ' ')))
            role = (row['role'] or 'worker').replace('worker-', '')
            requested = f'{hint}:{requested_effort}' if requested_effort else hint
            used = None
            if actual:
                route, effort = actual
                used = f'{route}:{effort}' if effort else route
                if requested_effort is not None and not effort:
                    en += '; the effective effort could not be verified from the attempt audit'
                    ru += '; фактический effort не удалось проверить по аудиту попытки'
                lines.append(f'Для «{role}» был запрошен {requested}; для отправки выбран {used}: {ru}.' if language == 'ru'
                             else f'{requested} was requested for {role}; dispatch selected {used}: {en}.')
            else:
                lines.append(f'Для «{role}» рекомендация {requested} не определяла выбор ({status}); '
                             'фактический маршрут не удалось проверить по аудиту попытки.' if language == 'ru'
                             else f'The hint {requested} did not determine routing for {role} ({status}); '
                             'the effective route could not be verified from the attempt audit.')
            event(db, task_id, 'MODEL_HINT_IGNORED_SHOWN', dict(attempt_id=row['attempt_id'], hint=hint,
                                                                status=status, used=used))
        db.commit()
    return lines
