"""Runtime-only synthesis for a fully closed, unrepaired reviewed plan."""
import json
import string
import unicodedata
from urllib.parse import quote

from ..contracts import StoreError
from .state import event, synthesis_checks, transition
from .check_closure import verified_runtime_runs
from .reviewer_handoff import path_metadata
from .worker_waits import ACTIVE, projection

MAX_SYNTHESIS_BYTES = 48 * 1024
MAX_OUTCOME_CHARS = 1600

# Deterministic reason codes the runtime-composed synthesis gate may record on
# decline. Eligibility itself is unchanged; this only names, for a durable
# metadata-only event, which already-evaluated condition(s) made the gate
# decline and hand off to the coordinator. 'open_check' additionally carries
# node/check/outcome (never check content).
DECLINE_REASON_CODES = (
    'worker_live', 'replan_history', 'repair_path', 'pending_input',
    'node_incomplete', 'review_not_approved', 'verification_not_passed', 'open_check',
)


def _pending(db, task_id):
    if db.execute("SELECT 1 FROM pending_questions WHERE task_id=? AND status='open'",
                  (task_id,)).fetchone():
        return True
    if db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND superseded=0 "
            "AND delivered_at IS NULL AND event_type IN "
            "('user_message','worker_question','assignment_failed','assignment_cancelled')",
            (task_id,)).fetchone():
        return True
    from ..delegation.events import TaskEvent, is_meaningful
    rows = db.execute("SELECT * FROM task_events WHERE task_id=? AND superseded=0 AND delivered_at IS NULL "
                      "AND event_type='progress'", (task_id,)).fetchall()
    if any(is_meaningful(TaskEvent._from_row(row)) for row in rows):
        return True
    if db.execute(
            "SELECT 1 FROM runtime_events e WHERE e.task_id=? AND e.type='USER_STEERING' "
            "AND NOT EXISTS (SELECT 1 FROM runtime_events a WHERE a.task_id=e.task_id "
            "AND a.type='STEERING_APPLIED' AND a.actor=e.actor)", (task_id,)).fetchone():
        return True
    if any(wait['status'] in ACTIVE for wait in projection(db, task_id)):
        return True
    # Consent storage is lazy: absence means no request has been made.
    if db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='consent_requests'").fetchone():
        if db.execute("SELECT 1 FROM consent_requests WHERE task_id=? AND status IN ('pending','asked')",
                      (task_id,)).fetchone():
            return True
    asked = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type='ASK_USER_REQUIRED'",
                       (task_id,)).fetchone()[0]
    answered = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? "
                          "AND type IN ('USER_ANSWER_RECEIVED','PLAN_ACCEPTED')", (task_id,)).fetchone()[0]
    if asked and (answered is None or asked > answered):
        return True
    from .skill_proposals import task_proposals
    if any(p['status'] == 'pending' or p['awaiting_plan'] for p in task_proposals(db, task_id)):
        return True
    return False


def _data(value):
    # Keep filenames and submitted prose inert, including fences/HTML/newlines.
    return json.dumps(value, ensure_ascii=True, sort_keys=True).replace('<', '\\u003c').replace('>', '\\u003e').replace('&', '\\u0026').replace('`', '\\u0060')


def _metadata(value):
    return '```json\n' + _data(value) + '\n```'


def _decline_reasons(db, task):
    """Evaluate the conditions covered by the fixed decline vocabulary.

    This is not an exhaustive explanation of compose or completion failures:
    missing baseline metadata, oversized text and post-compose rollback have no
    reason code. The earlier worker-live gate does not call this evaluator.
    Read-only; never changes eligibility.
    """
    task_id, revision = task['task_id'], task['revision']
    reasons = []
    if db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type IN "
                  "('REPLAN_REQUEST_ACCEPTED','RETURNED_TO_COORDINATOR','TASK_REOPENED')",
                  (task_id,)).fetchone():
        reasons.append(dict(code='replan_history'))
    if task['review_cycle'] > 1 or db.execute(
            "SELECT 1 FROM runtime_events WHERE task_id=? AND type IN "
            "('VERIFICATION_REVISED','VERIFICATION_FAILED','REVIEW_CHANGES_REQUESTED') "
            "AND (json_extract(payload_json,'$.revision')=? OR type IN ('VERIFICATION_REVISED','REVIEW_CHANGES_REQUESTED'))",
            (task_id, revision)).fetchone():
        reasons.append(dict(code='repair_path'))
    if _pending(db, task_id):
        reasons.append(dict(code='pending_input'))
    nodes = db.execute("SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid",
                       (task_id, revision)).fetchall()
    node_ok = bool(nodes) and all(n['status'] == 'completed' and n['result_json'] for n in nodes)
    if node_ok:
        plan = json.loads(task['plan_json'])
        node_ok = ({n['node_id'] for n in nodes if n['stage'] == 'execution'}
                   == {n['id'] for n in plan['tasks']})
    if not node_ok:
        reasons.append(dict(code='node_incomplete'))
    reviews = [n for n in nodes if n['stage'] == 'review']
    from .state import current_review_waiver
    if (reviews and json.loads(reviews[-1]['result_json'] or '{}').get('verdict') != 'APPROVE') or (
            not reviews and not current_review_waiver(db, task_id)):
        # A review the runtime waived (no project change since an approved
        # review) is not a missing approval.
        reasons.append(dict(code='review_not_approved'))
    if verified_runtime_runs(db, task_id, revision) is None:
        reasons.append(dict(code='verification_not_passed'))
    checks, gaps = synthesis_checks(db, task_id, revision)
    open_checks = [dict(code='open_check', node=node, check=check['name'], outcome=check.get('outcome'))
                   for node, check, outcome, assessment, closure in checks
                   if not closure and (not outcome or outcome['outcome'] not in {'passed', 'retracted'}
                                       or not assessment or assessment['strength'] != 'verified')]
    if gaps and not open_checks:
        # A disclosed gap with no matching current check row still blocks
        # synthesis; name it without inventing a node or submitted outcome.
        open_checks = [dict(code='open_check', node=None, check=name, outcome=None) for name in gaps.values()]
    reasons.extend(open_checks)
    return reasons


def _record_decline(db, task_id, revision, reasons):
    """Record one durable, metadata-only decline event, once per revision.

    Idempotent: a repeat call (recovery, replay, another poll) that finds an
    existing decline event for this revision writes nothing more.
    """
    if not reasons:
        return
    if db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='SYNTHESIS_RUNTIME_DECLINED' "
                  "AND json_extract(payload_json,'$.revision')=?", (task_id, revision)).fetchone():
        return
    event(db, task_id, 'SYNTHESIS_RUNTIME_DECLINED',
          dict(revision=revision, reason=reasons[0]['code'], reasons=reasons))


def compose(db, task, language):
    task_id, revision = task['task_id'], task['revision']
    if task['state'] != 'RECONCILING' or task['synthesis'] is not None or task['blocker'] or task['review_cycle'] > 1:
        return None
    if db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type IN "
                  "('REPLAN_REQUEST_ACCEPTED','RETURNED_TO_COORDINATOR','TASK_REOPENED')",
                  (task_id,)).fetchone():
        return None
    if db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type IN "
                  "('VERIFICATION_REVISED','VERIFICATION_FAILED','REVIEW_CHANGES_REQUESTED') "
                  "AND (json_extract(payload_json,'$.revision')=? OR type IN ('VERIFICATION_REVISED','REVIEW_CHANGES_REQUESTED'))",
                  (task_id, revision)).fetchone():
        return None
    if _pending(db, task_id):
        return None
    nodes = db.execute("SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid",
                       (task_id, revision)).fetchall()
    if not nodes or any(n['status'] != 'completed' or not n['result_json'] for n in nodes):
        return None
    plan = json.loads(task['plan_json'])
    if {n['node_id'] for n in nodes if n['stage'] == 'execution'} != {n['id'] for n in plan['tasks']}:
        return None
    reviews = [n for n in nodes if n['stage'] == 'review']
    if not reviews or json.loads(reviews[-1]['result_json']).get('verdict') != 'APPROVE':
        return None
    runs = verified_runtime_runs(db, task_id, revision)
    if runs is None:
        return None
    checks, gaps = synthesis_checks(db, task_id, revision)
    if gaps or any(not closure and (not outcome or outcome['outcome'] not in {'passed', 'retracted'}
                                   or not assessment or assessment['strength'] != 'verified')
                   for _, _, outcome, assessment, closure in checks):
        return None
    from .state import _observe_project
    observed = _observe_project(db, task_id)
    current = path_metadata(observed)
    baseline = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                          "AND type='EXECUTION_BASELINE' AND json_extract(payload_json,'$.revision')=? "
                          "ORDER BY id DESC LIMIT 1", (task_id, revision)).fetchone()
    before = json.loads(baseline[0]).get('path_metadata') if baseline else None
    if current is None or not isinstance(before, dict):
        return None
    changed = [dict(path=p, before=before.get(p), after=current.get(p))
               for p in sorted(set(before) | set(current)) if before.get(p) != current.get(p)]
    paths = [entry['path'] for entry in changed]
    # Single-backtick spans cannot contain backticks or control characters.
    # Leave unusual names to coordinator synthesis rather than render them raw.
    if any(c == '`' or unicodedata.category(c)[0] == 'C'
           or c in '\u2028\u2029' for path in paths for c in path):
        return None
    outcomes = [json.loads(n['result_json']).get('summary')
                for n in nodes if n['stage'] == 'execution']
    if not outcomes or any(not isinstance(s, str) or not s.strip() for s in outcomes):
        return None
    if sum(len(s) for s in outcomes) > MAX_OUTCOME_CHARS:
        return None  # Never discard a result or a caveat to shorten the answer.
    if any(unicodedata.category(c)[0] == 'C' and not c.isspace()
           for s in outcomes for c in s):
        return None
    # Quote only execution outcomes, not review prose, reports or check evidence.
    # Flatten whitespace and escape all Markdown punctuation, including HTML.
    escape = str.maketrans({c: '\\' + c for c in string.punctuation})
    quoted = ['> ' + ' '.join(s.split()).translate(escape) for s in outcomes]
    if language == 'ru':
        lines = ['# Готово', 'Работа завершена.']
        if paths:
            lines.append('Изменены файлы: ' + ', '.join('`' + path + '`' for path in paths) + '.')
        else:
            lines.append('Файлы менять не пришлось.')
    else:
        lines = ['# Done', 'The requested work is done.']
        if paths:
            lines.append('Files changed: ' + ', '.join('`' + path + '`' for path in paths) + '.')
        else:
            lines.append('No files needed changes.')
    lines[2:2] = [('Результат работы:' if language == 'ru' else 'Work summary:')
                  + '\n\n' + '\n\n'.join(quoted)]
    text = '\n\n'.join(lines)
    if len(text.encode('utf-8')) > MAX_SYNTHESIS_BYTES:
        return None
    return text


def complete(machine, task_id, language, workers_absent):
    """Commit atomically; workers_absent must reuse db for any bookkeeping.

    Records at most one durable SYNTHESIS_RUNTIME_DECLINED per revision for
    failures covered by the fixed vocabulary. Missing-baseline/oversized-text
    returns and post-compose rollback can leave no event. Existing coordinator
    synthesis is never treated as a decline.
    """
    from .state import observed_project
    # The project is scanned several times below: once, before the lock.
    with observed_project(machine.store, task_id), machine.store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if not task or task['state'] != 'RECONCILING':
            return None
        if not workers_absent(db):
            if task['synthesis'] is None:
                _record_decline(db, task_id, task['revision'], [dict(code='worker_live')])
                db.commit()
            return None
        text = compose(db, task, language)
        if text is None:
            if task['synthesis'] is None:
                _record_decline(db, task_id, task['revision'], _decline_reasons(db, task))
                db.commit()
            return None
        db.execute('UPDATE runtime_tasks SET synthesis=? WHERE task_id=?', (text, task_id))
        try:
            machine._completion_invariants(db, task_id)
        except StoreError:
            # The context manager rolls back tentative text and derived closures.
            return None
        # Composition and integrity checks can take time. A worker that became
        # live meanwhile, or a cancellation, keeps coordinator reconciliation.
        if not workers_absent(db):
            return None
        event(db, task_id, 'SYNTHESIS_RUNTIME_COMPOSED',
              dict(revision=task['revision'], text=text, language=language, not_verified=[]))
        transition(db, task_id, 'COMPLETED')
        db.commit()
    return text
