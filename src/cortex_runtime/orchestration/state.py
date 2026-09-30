"""Durable, transactional task lifecycle and dependency scheduler.

No transition, permission, model, or worker handle is accepted from a model.
State and events commit together in the project's existing SQLite database.
"""
from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import json
import os
import re
from pathlib import Path, PurePosixPath
from typing import NamedTuple

from ..connections.model import SUPPORTED_ENGINES
from .. import task_workspace
from ..contracts import StoreError
from ..portable.roles import resolve_role

SCHEMA = '''
CREATE TABLE IF NOT EXISTS runtime_tasks (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 state TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0,
 goal TEXT NOT NULL, plan_json TEXT, synthesis TEXT,
 review_cycle INTEGER NOT NULL DEFAULT 0, blocker TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_nodes (
 task_id TEXT NOT NULL REFERENCES runtime_tasks(task_id),
 revision INTEGER NOT NULL, node_id TEXT NOT NULL,
 role TEXT NOT NULL, objective TEXT NOT NULL, dependencies_json TEXT NOT NULL,
 scope_json TEXT NOT NULL, stage TEXT NOT NULL, status TEXT NOT NULL,
 assignment_ref TEXT UNIQUE, result_json TEXT,
 PRIMARY KEY(task_id,revision,node_id)
);
CREATE TABLE IF NOT EXISTS runtime_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL REFERENCES runtime_tasks(task_id),
 type TEXT NOT NULL, actor TEXT NOT NULL, payload_json TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS runtime_events_task ON runtime_events(task_id,id);
CREATE TABLE IF NOT EXISTS runtime_requests (
 task_id TEXT NOT NULL, request_key TEXT NOT NULL, actor TEXT NOT NULL,
 operation TEXT NOT NULL, digest TEXT NOT NULL, result_json TEXT NOT NULL,
 PRIMARY KEY(task_id,actor,request_key)
);
'''

TRANSITIONS = {
    'CREATED': {'PLANNING'},
    'PLANNING': {'PLAN_READY', 'ANSWER_READY', 'WAITING_USER', 'VERIFYING'},
    'ANSWER_READY': {'COMPLETED'},
    'PLAN_READY': {'EXECUTING'},
    # EXECUTING -> VERIFYING only with the runtime's own REVIEW_NOT_REQUIRED
    # observation for the revision (see transition and review_waiver).
    'EXECUTING': {'REVIEWING', 'VERIFYING', 'WAITING_USER'},
    'REVIEWING': {'EXECUTING', 'VERIFYING', 'WAITING_USER'},
    'VERIFYING': {'RECONCILING', 'PLANNING'},
    'RECONCILING': {'COMPLETED', 'WAITING_USER'},
    'WAITING_USER': {'PLANNING', 'RECONCILING'},
    'RECOVERING': {'PLANNING', 'EXECUTING', 'REVIEWING', 'VERIFYING', 'RECONCILING'},
    'BLOCKED': {'RECOVERING'},
    'FAILED': set(), 'CANCELLED': set(), 'COMPLETED': set(),
}
TERMINAL = {'FAILED', 'CANCELLED', 'COMPLETED'}
MAX_NODES = 64
MAX_REVIEW_CYCLES = 3
# Times one review assignment is re-bound to a project that changed under it
# (another writer in the same checkout) before its result is refused for good.
MAX_REVIEW_REBINDS = 3
# Automatic returns of a failed verification to the coordinator, per task.
MAX_VERIFICATION_REPAIRS = 3
# Automatic returns of a failed worker result to the coordinator, per task.
MAX_COORDINATOR_RETURNS = 3
# Verification uses exec-style argv, never a shell. These are only tokens that
# acquire shell control meaning when separated from an argument; embedded
# characters remain valid literal arguments (for example pytest -k expressions).
_VERIFICATION_SHELL_TOKENS = frozenset({"<", ">", ">>", "|", "&&", "||", ";", "(", ")"})


def now():
    return datetime.now(timezone.utc).isoformat(timespec='microseconds')


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))


def event(db, task_id, kind, payload, actor='runtime'):
    db.execute('INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)',
               (task_id, kind, actor, encoded(payload), now()))


def transition(db, task_id, target, *, blocker=None):
    row = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
    if row is None:
        raise StoreError('runtime_task_missing')
    source = row['state']
    allowed = TRANSITIONS.get(source, set()) | ({'BLOCKED', 'FAILED', 'CANCELLED', 'RECOVERING'} if source not in TERMINAL else set())
    if target not in allowed or (source == 'EXECUTING' and target == 'VERIFYING'
                                 and not current_review_waiver(db, task_id)):
        raise StoreError('invalid_state_transition', 'state', f'{source} -> {target}',
                         ', '.join(sorted(allowed)), 'The runtime owns stage transitions.')
    if source == 'PLAN_READY' and target == 'EXECUTING':
        _record_execution_baseline(db, task_id)
    if target == 'BLOCKED':
        # A stop is never a dead end: the user is asked why and what next,
        # and the answer resumes through RECOVERING (workers confirmed gone)
        # into PLANNING, where the coordinator acts on it.
        _ask_about_stop(db, task_id, source, blocker or 'unknown')
        return
    db.execute('UPDATE runtime_tasks SET state=?,blocker=?,updated_at=? WHERE task_id=?',
               (target, blocker, now(), task_id))
    event(db, task_id, 'TASK_STATE_CHANGED', dict(previous=source, state=target, blocker=blocker))


def _observe_project(db, task_id):
    from ..delegation.revision import capture_revision
    return capture_revision(task_workspace.task_root(db, task_id))


def _record_execution_baseline(db, task_id):
    """Observe the project as a plan revision starts executing.

    Review is decided later against this runtime observation, never against
    a model's claim that nothing changed."""
    revision = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()['revision']
    observed = _observe_project(db, task_id)
    event(db, task_id, 'EXECUTION_BASELINE', dict(revision=revision, kind=observed['kind'], head=observed['head'],
                                                  revision_digest=observed['dirty_digest']))


def review_waiver(db, task_id, revision, nodes):
    """Why independent review is not required for this revision, or None.

    Only when the project observed now is exactly a state that needs no
    review: the one observed before this task's first planned work (since the
    user last reopened it -- later changes by an earlier revision are still
    caught), or one an independent reviewer approved. A revision that already
    has a review node (a fix cycle) is always reviewed again, and an
    unavailable observation always requires review."""
    if any(n['stage'] == 'review' for n in nodes):
        return None
    observed = _observe_project(db, task_id)
    if observed['kind'] != 'observed' or not observed['dirty_digest']:
        return None
    digest, head = observed['dirty_digest'], observed['head']
    reopened = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'",
                          (task_id,)).fetchone()[0] or 0
    baseline = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='EXECUTION_BASELINE' "
                          "AND id>? ORDER BY id LIMIT 1", (task_id, reopened)).fetchone()
    baseline = json.loads(baseline[0]) if baseline else {}
    if baseline.get('kind') == 'observed' and baseline.get('revision_digest') == digest and baseline.get('head') == head:
        return dict(reason='no_project_changes', reference='execution_baseline',
                    reference_revision=baseline['revision'], revision_digest=digest, head=head)
    for row in db.execute("SELECT revision,result_json FROM runtime_nodes WHERE task_id=? AND stage='review' "
                          "AND result_json IS NOT NULL ORDER BY rowid DESC", (task_id,)).fetchall():
        result = json.loads(row['result_json'])
        if result.get('verdict') == 'APPROVE' and result.get('project_digest') == digest:
            return dict(reason='no_changes_since_approved_review', reference='approved_review',
                        reference_revision=row['revision'], revision_digest=digest, head=head)
    return None


def current_review_waiver(db, task_id):
    """The runtime's REVIEW_NOT_REQUIRED record for the current revision."""
    task = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
    if task is None or db.execute("SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='review'",
                                  (task_id, task['revision'])).fetchone():
        return None
    row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_NOT_REQUIRED' "
                     "AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1",
                     (task_id, task['revision'])).fetchone()
    return json.loads(row[0]) if row else None


def reviewed_digest(db, task_id, revision):
    """The project digest verification and completion must match: the last
    review's, or the runtime's no-change observation when none was required."""
    review = db.execute("SELECT result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='review' "
                        "ORDER BY rowid DESC LIMIT 1", (task_id, revision)).fetchone()
    if review:
        return json.loads(review[0]).get('project_digest') if review[0] else None
    waiver = current_review_waiver(db, task_id)
    return waiver['revision_digest'] if waiver and waiver['revision'] == revision else None


STOP_OPTIONS = ('Continue: let the coordinator decide how to proceed',
                'Finish: accept the current results and close the task')


# Stops about the work itself: the coordinator knows the context, so it asks
# the user a concrete question (what failed, why, what was tried, concrete
# options). A stop about the coordinator itself gets the runtime's question.
WORK_LEVEL_STOPS = ('verification_failed', 'worker_failed', 'worker_retry_limit', 'worker_failure_not_retryable',
                    'review_cycle_limit', 'review_blocked', 'no_runnable_nodes', 'scheduler_capacity_timeout',
                    'lost_worker_', 'verification_review_revision_changed', 'verification_revision_unavailable',
                    'review_revision_unavailable', 'completion_')


def _coordinator_should_ask(db, task_id, blocker):
    if not blocker.startswith(WORK_LEVEL_STOPS):
        return False
    asked = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type='ASK_USER_REQUIRED'", (task_id,)).fetchone()[0]
    answered = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type='USER_ANSWER_RECEIVED'", (task_id,)).fetchone()[0]
    # Once per user answer: a coordinator that could not ask gets the runtime's question.
    return asked is None or (answered is not None and answered > asked)


def _ask_about_stop(db, task_id, source, blocker):
    from ..portable.identity import new_id
    if source != 'WAITING_USER' and _coordinator_should_ask(db, task_id, blocker):
        event(db, task_id, 'ASK_USER_REQUIRED', dict(blocker=blocker))
        db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')", (task_id,))
        db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",
                   (now(), task_id))
        db.execute('UPDATE runtime_tasks SET state=?,blocker=NULL,updated_at=? WHERE task_id=?',
                   ('RECOVERING', now(), task_id))
        event(db, task_id, 'TASK_STATE_CHANGED', dict(previous=source, state='RECOVERING', blocker=None))
        return
    revision = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()['revision']
    # The answer resumes through RECOVERING into a new plan, as above: nothing
    # of the stopped plan may keep holding its scope (a review node left
    # 'running' with scope ** blocked every later plan's dispatch).
    db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')", (task_id,))
    db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",
               (now(), task_id))
    if source == 'WAITING_USER':
        db.execute("UPDATE pending_questions SET status='superseded',answered_at=? WHERE task_id=? AND status='open'",
                   (now(), task_id))
    question_id = new_id('pq_')
    text = f'The task stopped: {blocker}. What should happen next?'
    db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at) VALUES(?,?,?,?,?,'open',?)",
               (question_id, task_id, f'runtime:stop:{question_id}', text, encoded(list(STOP_OPTIONS)), now()))
    event(db, task_id, 'USER_QUESTION_REQUESTED', dict(question_id=question_id, question=text, options=list(STOP_OPTIONS),
                                                       resume_state='RECOVERING', blocker=blocker, revision=revision))
    db.execute('UPDATE runtime_tasks SET state=?,blocker=?,updated_at=? WHERE task_id=?',
               ('WAITING_USER', blocker, now(), task_id))
    event(db, task_id, 'TASK_STATE_CHANGED', dict(previous=source, state='WAITING_USER', blocker=blocker))


def released_by_user(db, task):
    """The user answered a runtime stop question for this plan revision, or
    a question the coordinator asked after this revision was verified.

    Only then may the coordinator close delegated work with a direct answer;
    it can never skip review or verification on its own."""
    if db.execute("SELECT 1 FROM runtime_nodes WHERE task_id=? AND status IN ('pending','running')",
                  (task['task_id'],)).fetchone():
        return False
    asked = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'",
                       (task['task_id'],)).fetchall()
    stops = {q['question_id'] for q in map(lambda r: json.loads(r[0]), asked)
             if (q.get('blocker') or q.get('after_verification')) and q.get('revision') == task['revision']}
    answered = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_ANSWER_RECEIVED'",
                          (task['task_id'],)).fetchall()
    return any(json.loads(r[0]).get('question_id') in stops for r in answered)


def reopened_after_completion(db, task):
    """The user's message reopened a COMPLETED task and nothing was planned since.

    Derived only from runtime events: the last state change before the latest
    TASK_REOPENED is COMPLETED (reachable only through the completion
    invariants: verified work, or a direct answer the runtime accepted), and
    since then no plan was accepted, no assignment created, no replan accepted
    and no worker provisioned outside an earlier assignment. A follow-up that only asks
    about the finished work may be answered directly; any new work still goes
    through a plan, review and verification."""
    task_id = task['task_id']
    reopened = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'",
                          (task_id,)).fetchone()[0]
    if reopened is None:
        return False
    before = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='TASK_STATE_CHANGED' "
                        "AND id<? ORDER BY id DESC LIMIT 1", (task_id, reopened)).fetchone()
    if not before or json.loads(before[0]).get('state') != 'COMPLETED':
        return False
    if db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND id>? AND type IN "
                  "('PLAN_ACCEPTED','ASSIGNMENT_CREATED','REPLAN_REQUEST_ACCEPTED')", (task_id, reopened)).fetchone():
        return False
    if db.execute("SELECT 1 FROM runtime_nodes WHERE task_id=? AND status IN ('pending','running')",
                  (task_id,)).fetchone():
        return False
    return not db.execute(
        "SELECT 1 FROM delegation_provisioning p WHERE p.task_id=? AND p.role_id!='orchestrator' AND NOT EXISTS ("
        "SELECT 1 FROM runtime_events e WHERE e.task_id=p.task_id AND e.type='ASSIGNMENT_CREATED' AND e.id<? "
        "AND json_extract(e.payload_json,'$.assignment_ref')=p.idempotency_key)", (task_id, reopened)).fetchone()


def direct_answer_allowed(db, task):
    """Whether answer_submit may close the task: before any plan or worker
    exists, after the user released a stopped or verified revision, or for a
    follow-up on a completed task with no work planned since it reopened."""
    return (released_by_user(db, task) or reopened_after_completion(db, task) or (
        task['revision'] == 0 and task['plan_json'] is None
        and not db.execute('SELECT 1 FROM runtime_nodes WHERE task_id=?', (task['task_id'],)).fetchone()
        and not db.execute("SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'",
                           (task['task_id'],)).fetchone()))


class AttemptRouting(NamedTuple):
    escalate: bool
    complexity: str
    model_hint: str
    model_reason: str | None
    previous_engine: str | None
    # The user's own request for the exact hint's model family in this
    # task (routing.auto_route.user_model_request); only a flagship hint uses it.
    user_request: dict | None = None


def attempt_routing(store, task_id, attempt_id):
    """Read a node's plan hint, prior audited engine and, for an exact route
    hint, whether the user asked for its model family in this task."""
    with store.connection() as db:
        provision = db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                               (task_id, attempt_id)).fetchone()
        node = provision and db.execute('SELECT node_id,revision FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                                        (task_id, provision['idempotency_key'])).fetchone()
        if not node:
            return AttemptRouting(False, 'normal', 'auto', None, None)
        plan = db.execute('SELECT plan_json FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        retries = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'",
                             (task_id,)).fetchall()
        # runtime_nodes.assignment_ref changes on retry. The immutable
        # ASSIGNMENT_CREATED event retains each earlier assignment-to-node
        # mapping, so previous audits still resolve after that update.
        prior = db.execute(
            "SELECT a.engine FROM attempt_route_audit a "
            "JOIN delegation_provisioning p ON p.task_id=a.task_id AND p.attempt_id=a.attempt_id "
            "JOIN runtime_events e ON e.task_id=p.task_id AND e.type='ASSIGNMENT_CREATED' "
            "AND json_extract(e.payload_json,'$.assignment_ref')=p.idempotency_key "
            "WHERE a.task_id=? AND json_extract(e.payload_json,'$.revision')=? "
            "AND json_extract(e.payload_json,'$.node') IN (?,?) AND a.attempt_id<>? "
            "ORDER BY a.rowid DESC LIMIT 1",
            (task_id, node['revision'], node['node_id'],
             re.sub(r'^_fix_\d+_', '', node['node_id']), attempt_id),
        ).fetchone()
    fix = re.fullmatch(r'_fix_\d+_(.+)', node['node_id'])
    planned = fix.group(1) if fix else node['node_id']
    tasks = json.loads(plan['plan_json'])['tasks'] if plan and plan['plan_json'] else []
    task = next((t for t in tasks if t['id'] == planned), {})
    retried = any(p.get('node') == node['node_id'] and p.get('revision') == node['revision'] and not p.get('route_failure')
                  for p in (json.loads(r[0]) for r in retries))
    hint = task.get('model') or task.get('provider') or 'auto'
    from ..routing.auto_route import user_model_request
    return AttemptRouting(bool(fix) or retried, task.get('complexity', 'normal'), hint,
                          task.get('model_reason'), prior['engine'] if prior else None,
                          user_model_request(store, task_id, hint) if '/' in hint else None)


def review_provider_preference(store, task_id, attempt_id):
    """Prefer an independent reviewer from the other engine when determinable.

    The preference comes only from immutable audit rows for completed execution
    nodes in this review's revision. A mixed-provider implementation has no
    single opposite provider, so ordinary ranking remains in effect and the
    returned reason makes that limitation explicit.
    """
    with store.connection() as db:
        reviewer = db.execute(
            "SELECT n.stage,n.revision FROM delegation_provisioning p "
            "JOIN runtime_nodes n ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key "
            "WHERE p.task_id=? AND p.attempt_id=?",
            (task_id, attempt_id),
        ).fetchone()
        if reviewer is None or reviewer["stage"] != "review":
            return None, "not_review"
        rows = db.execute(
            "SELECT DISTINCT a.engine FROM attempt_route_audit a "
            "JOIN delegation_provisioning p ON p.task_id=a.task_id AND p.attempt_id=a.attempt_id "
            "JOIN runtime_nodes n ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key "
            "WHERE a.task_id=? AND n.revision=? AND n.stage='execution' AND n.status='completed'",
            (task_id, reviewer["revision"]),
        ).fetchall()
    engines = {row["engine"] for row in rows}
    if engines == {"claude-agent-acp"}:
        return "codex-acp", "prefer_codex_after_claude_execution"
    if engines == {"codex-acp"}:
        return "claude-agent-acp", "prefer_claude_after_codex_execution"
    if len(engines) > 1:
        return None, "no_single_opposite_provider:mixed_execution"
    return None, "no_audited_execution_provider"


def targeted_fixes(tasks, scopes, findings, report_ref=None):
    """Fixes a review's findings can be confined to, or None and why not.

    A fix goes only to the plan tasks the findings name when every finding
    names one of them, is not cross-cutting and lists only files the runtime
    finds inside that task's scope. Anything else -- no findings, an unknown
    task, a cross-cutting problem, a file outside the scope, or findings too
    long to deliver as one objective -- keeps the conservative fix of every
    original task. Contract changes are never guessed: a reviewer marks them
    cross_cutting. Each targeted fix waits for the fixes of the plan tasks it
    (transitively) depended on, so related fixes keep the plan's order.
    """
    from .gateway import _matches
    from .project_paths import permitted_path
    if not findings:
        return None, 'no_findings'
    grouped = {}
    for finding in findings:
        node = finding.get('node')
        if finding.get('cross_cutting') is not False:
            return None, 'cross_cutting'
        if node not in scopes:
            return None, 'unknown_node'
        files = finding.get('files') or []
        if not files or any(not isinstance(path, str) or not path or not permitted_path(path)
                            or not _matches(PurePosixPath(path).as_posix(), scopes[node]) for path in files):
            return None, 'files_outside_scope'
        grouped.setdefault(node, []).append(finding)
    parents = {t['id']: list(t['dependencies']) for t in tasks}
    order = [t['id'] for t in tasks]
    targets = {}
    for node, items in grouped.items():
        seen, pending = set(), list(parents.get(node, []))
        while pending:
            current = pending.pop()
            if current not in seen:
                seen.add(current)
                pending.extend(parents.get(current, []))
        reference = f' (review report: {report_ref})' if report_ref else ''
        objective = (f'Address these review findings within the assigned scope{reference}:\n' + '\n'.join(
            f"- Files: {', '.join(item['files'])}\n  Fix: {item['fix']}" for item in items))
        if len(objective) > 8000:
            return None, 'findings_too_long'
        targets[node] = dict(objective=objective, after=[n for n in order if n in seen and n in grouped])
    return targets, None


def validate_scope(scope):
    if not isinstance(scope, list) or not scope or len(scope) > 64:
        raise StoreError('invalid_assignment_scope')
    for value in scope:
        if not isinstance(value, str) or not value or len(value) > 512:
            raise StoreError('invalid_assignment_scope')
        from .project_paths import permitted_path
        if not permitted_path(value):
            raise StoreError('forbidden_assignment_scope')
    return scope


def validate_plan(plan):
    if not isinstance(plan, dict) or set(plan) != {'tasks', 'verification'}:
        raise StoreError('invalid_plan_fields')
    nodes = plan['tasks']
    if not isinstance(nodes, list) or not 1 <= len(nodes) <= MAX_NODES:
        raise StoreError('invalid_plan_size')
    ids = set()
    for node in nodes:
        if (not isinstance(node, dict) or not {'id', 'role', 'objective', 'dependencies', 'scope'} <= set(node)
                or set(node) - {'id', 'role', 'objective', 'dependencies', 'scope', 'complexity', 'provider', 'model', 'model_reason'}
                or node.get('complexity', 'normal') not in {'low', 'normal', 'high'}
                or node.get('provider', SUPPORTED_ENGINES[0]) not in SUPPORTED_ENGINES):
            raise StoreError('invalid_plan_node')
        from .contracts import NODE, validate as validate_contract
        if 'model' in node:
            validate_contract(NODE['properties']['model'], node['model'], 'model')
            if node['model'] != 'auto':
                if not isinstance(node.get('model_reason'), str) or not 1 <= len(node['model_reason']) <= 300:
                    raise StoreError('model_reason_required')
            elif 'model_reason' in node:
                validate_contract(NODE['properties']['model_reason'], node['model_reason'], 'model_reason')
        elif 'model_reason' in node:
            raise StoreError('invalid_plan_node')
        key = node['id']
        if not isinstance(key, str) or not re.fullmatch(r'[A-Za-z][A-Za-z0-9_-]{0,63}', key) or key in ids:
            raise StoreError('duplicate_or_invalid_node')
        ids.add(key)
        role = resolve_role(node['role'])
        if not role.startswith('worker-') or role in {'worker-code-reviewer', 'worker-build-verification'}:
            raise StoreError('runtime_reserved_role')
        if not isinstance(node['objective'], str) or not 1 <= len(node['objective']) <= 8000:
            raise StoreError('invalid_node_objective')
        validate_scope(node['scope'])
        deps = node['dependencies']
        if not isinstance(deps, list) or any(not isinstance(d, str) for d in deps) or len(set(deps)) != len(deps):
            raise StoreError('invalid_dependencies')
    remaining = {n['id']: set(n['dependencies']) for n in nodes}
    if any(not deps <= ids for deps in remaining.values()):
        raise StoreError('unknown_dependency')
    resolved = set()
    while remaining:
        ready = [key for key, deps in remaining.items() if deps <= resolved]
        if not ready:
            raise StoreError('cyclic_plan')
        for key in ready:
            resolved.add(key)
            del remaining[key]
    checks = plan['verification']
    if not isinstance(checks, list) or not 1 <= len(checks) <= 16:
        raise StoreError('verification_required')
    for argv in checks:
        if not isinstance(argv, list) or not 1 <= len(argv) <= 64 or any(
                not isinstance(arg, str) or not arg or len(arg) > 4096 or '\x00' in arg for arg in argv):
            raise StoreError('invalid_verification_command')
        shell_token = next((arg for arg in argv
                            if arg in _VERIFICATION_SHELL_TOKENS or "$(" in arg or "`" in arg), None)
        if shell_token is not None:
            raise StoreError(
                'verification_shell_syntax', 'plan.verification', shell_token,
                'literal argv without shell syntax',
                'Use a documented project command or script. Verification executes argv directly without a shell.',
            )
    return plan


class TaskMachine:
    def __init__(self, store):
        self.store = store

    def create(self, task_id, goal):
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            row = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            if row is None:
                stamp = now()
                db.execute('INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)',
                           (task_id, 'CREATED', goal, stamp, stamp))
                event(db, task_id, 'TASK_CREATED', dict(goal=goal))
                transition(db, task_id, 'PLANNING')
            db.commit()
        return self.context(task_id)

    def get_state(self, task_id):
        """Read only the durable orchestration state for a task."""
        with self.store.connection() as db:
            task = db.execute(
                'SELECT state FROM runtime_tasks WHERE task_id=?', (task_id,)
            ).fetchone()
        if task is None:
            raise StoreError('runtime_task_missing')
        return task['state']

    def context(self, task_id):
        # A single explicit read transaction keeps every part of the context
        # on the same SQLite snapshot.
        with self.store.connection() as db:
            db.execute('BEGIN')
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            if task is None:
                raise StoreError('runtime_task_missing')
            task_value = dict(task)
            nodes = db.execute(
                'SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid',
                (task_id, task['revision']),
            ).fetchall()
            answers = db.execute(
                "SELECT id,text,answer_text FROM pending_questions "
                "WHERE task_id=? AND status='answered' ORDER BY created_at",
                (task_id,),
            ).fetchall()
            steering = db.execute(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_STEERING' "
                "ORDER BY id DESC LIMIT 64",
                (task_id,),
            ).fetchall()
            proposals = db.execute(
                "SELECT payload_json FROM runtime_events WHERE task_id=? "
                "AND type='REPLAN_REQUEST_ACCEPTED' ORDER BY id DESC LIMIT 3",
                (task_id,),
            ).fetchall()
            verification = db.execute(
                "SELECT payload_json FROM runtime_events WHERE task_id=? "
                "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                (task_id,),
            ).fetchone()
            from .policy import denied_paths
            denials = denied_paths(db, task_id)
            waiver = current_review_waiver(db, task_id)

            asked = db.execute(
                "SELECT id,payload_json FROM runtime_events WHERE task_id=? "
                "AND type='ASK_USER_REQUIRED' ORDER BY id DESC LIMIT 1",
                (task_id,),
            ).fetchone()
            answered = db.execute(
                "SELECT MAX(id) FROM runtime_events WHERE task_id=? "
                "AND type IN ('USER_ANSWER_RECEIVED','PLAN_ACCEPTED')",
                (task_id,),
            ).fetchone()[0]
            verification_attempts = db.execute(
                "SELECT COUNT(*) FROM runtime_events WHERE task_id=? "
                "AND type='VERIFICATION_REVISED'",
                (task_id,),
            ).fetchone()[0]

            from .skill_proposals import task_proposals
            skill_proposals = task_proposals(db, task_id)

            value_nodes = [
                {
                    **dict(node),
                    'dependencies': json.loads(node['dependencies_json']),
                    'scope': json.loads(node['scope_json']),
                    'result': json.loads(node['result_json'] or 'null'),
                }
                for node in nodes
            ]
            for node in value_nodes:
                for key in ('dependencies_json', 'scope_json', 'result_json'):
                    node.pop(key)

            # Resolve all assignment attempts in one query. The evidence
            # annotator still validates each result against its own attempt.
            assignment_refs = list(dict.fromkeys(
                node['assignment_ref'] for node in value_nodes
                if node['assignment_ref']
            ))
            attempts_by_assignment = {}
            if assignment_refs:
                placeholders = ','.join('?' for _ in assignment_refs)
                attempt_rows = db.execute(
                    "SELECT idempotency_key,attempt_id FROM delegation_provisioning "
                    f"WHERE task_id=? AND idempotency_key IN ({placeholders}) ORDER BY rowid",
                    (task_id, *assignment_refs),
                ).fetchall()
                for row in attempt_rows:
                    attempts_by_assignment.setdefault(
                        row['idempotency_key'], row['attempt_id']
                    )

            from ..delegation.evidence import annotate_checks
            for node in value_nodes:
                result = node['result']
                if isinstance(result, dict) and result.get('checks'):
                    attempt_id = attempts_by_assignment.get(node['assignment_ref'])
                    result['checks'] = annotate_checks(
                        db, task_id, attempt_id, result['checks']
                    )

        value = task_value
        value['review_not_required'] = waiver
        value['runtime_proposals'] = [
            json.loads(row[0]) for row in reversed(proposals)
        ]
        value['steering'] = [
            json.loads(row[0]) for row in reversed(steering)
        ]
        value['policy'] = dict(deny_write=denials, native_tools=True)
        value['verification'] = json.loads(verification[0]) if verification else None
        value['ask_user_required'] = (
            json.loads(asked[1])['blocker']
            if asked and (answered is None or answered < asked[0])
            else None
        )
        value['verification_attempts'] = verification_attempts
        value['user_answers'] = [dict(answer) for answer in answers]
        value['skill_proposals'] = skill_proposals
        value['plan'] = json.loads(value.pop('plan_json') or 'null')
        value['nodes'] = value_nodes
        return value

    def submit_plan(self, channel, plan, request_key, *, authorization=None):
        validate_plan(plan)
        digest = hashlib.sha256(encoded(plan).encode()).hexdigest()
        # Exact model labels must be in the currently confirmed, policy-allowed
        # menu. Engine hints and auto do not require a catalog at plan time.
        from ..routing.auto_route import hint_serves_tier, route_menu, tier_for
        exact_hints = [n['model'] for n in plan['tasks'] if '/' in n.get('model', '')]
        if exact_hints:
            menu = route_menu(self.store.project_root)
            offered = {item['route'] for group in menu['tiers'].values() for item in group}
            if any(hint not in offered for hint in exact_hints):
                raise StoreError('unknown_model_hint', 'model', ', '.join(exact_hints),
                                 'an exact route from list_route_options',
                                 'Call list_route_options and copy an offered route.')
        from ..routing.auto_route import user_model_request

        def preview(n):
            model, role = n.get('model', 'auto'), resolve_role(n['role'])
            complexity = n.get('complexity', 'normal')
            item = dict(node=n['id'], model=model, tier_now=tier_for(role, complexity=complexity))
            if '/' not in model:
                item['preview_status'] = 'auto' if model == 'auto' else 'pending_dispatch'
                return item
            hinted_tier, hinted = next(((tier, entry) for tier, group in menu['tiers'].items()
                                        for entry in group if entry['route'] == model), (None, {}))
            # A flagship route runs only on escalation or when the user asked
            # for its family in this task (auto_route.user_model_request).
            request = user_model_request(self.store, channel.task_id, model) if hinted_tier == 'flagship' else None
            if request is not None:
                item.update(preview_status='pending_dispatch:user_request', user_request=request['source_ref'])
            elif not hint_serves_tier(model.partition('/')[2], hinted_tier, role, item['tier_now'], complexity,
                                      also_standard_for=(hinted.get('policy') or {}).get('also_standard_for')):
                item['preview_status'] = 'ignored:tier_mismatch'
            else:
                item['preview_status'] = 'pending_dispatch'
            return item

        previews = [preview(n) for n in plan['tasks']]
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            if authorization: authorization(db)
            previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                  (channel.task_id, channel.channel_id, request_key)).fetchone()
            if previous:
                if previous['operation'] != 'plan_submit' or previous['digest'] != digest:
                    raise StoreError('request_key_conflict')
                return json.loads(previous['result_json'])
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
            if task is None or task['state'] != 'PLANNING':
                raise StoreError('plan_not_expected')
            from .policy import validate_grants
            project = db.execute('SELECT project_root FROM tasks WHERE id=?', (channel.task_id,)).fetchone()
            for node in plan['tasks']:
                from .gateway import READ_ONLY
                if resolve_role(node['role']) not in READ_ONLY:
                    validate_grants(db, channel.task_id, node['scope'])
                elif project is not None:
                    # A read-only role's jail and tools never write, so a path
                    # that does not exist yet can only be an output it cannot
                    # create; accepted silently, its worker did the work and
                    # then fought the read-only project to save it.
                    for scope in node['scope']:
                        if not any(mark in scope for mark in '*?[') and not os.path.lexists(os.path.join(project[0], scope)):
                            raise StoreError('read_only_scope_not_writable', f"tasks[{node['id']}].scope", scope,
                                             'an existing path to read',
                                             f"{node['role']} is read-only: have it return the content in its report and "
                                             'give the file to a writing role in a dependent task.')
            proposed_nodes=plan['tasks']
            request=db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED' ORDER BY id DESC LIMIT 1",(channel.task_id,)).fetchone()
            escalation=json.loads(request[0]) if request else {}
            if escalation.get('operation')=='escalation_request' and escalation.get('revision')==task['revision']:
                # The runtime routes a readonly consultant automatically
                # (routing.auto_route). The model never chooses a model.
                proposed_nodes=[dict(id='_consultation',role='senior-consultant',
                    objective=escalation['proposal']['reason'],dependencies=[],scope=['.'])]+[
                    dict(node,dependencies=['_consultation',*node['dependencies']]) for node in proposed_nodes]
                event(db,channel.task_id,'CONSULTATION_REQUIRED',dict(role='worker-senior-consultant',source_revision=task['revision']))
            revision = task['revision'] + 1
            db.execute('UPDATE runtime_tasks SET revision=?,plan_json=?,synthesis=NULL,review_cycle=0 WHERE task_id=?',
                       (revision, encoded(plan), channel.task_id))
            for node in proposed_nodes:
                db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                           (channel.task_id, revision, node['id'], resolve_role(node['role']), node['objective'],
                            encoded(node['dependencies']), encoded(node['scope']), 'execution', 'pending', None, None))
            event(db, channel.task_id, 'PLAN_ACCEPTED', dict(revision=revision, plan=plan), channel.channel_id)
            transition(db, channel.task_id, 'PLAN_READY')
            result = dict(revision=revision, state='PLAN_READY', model_hints=previews,
                          hint='Plan accepted; the runtime dispatches workers once this turn ends. '
                               'End this turn now -- do not call task_get_context to wait for dispatch, '
                               'the runtime resumes you when results arrive.')
            db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                       (channel.task_id, request_key, channel.channel_id, 'plan_submit', digest, encoded(result)))
            db.commit()
        return result

    def move(self, task_id, target, *, blocker=None):
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            if target == 'COMPLETED':
                self._completion_invariants(db, task_id)
            transition(db, task_id, target, blocker=blocker)
            db.commit()

    def submit_answer(self, channel, text, request_key, *, authorization=None):
        """Record an informational proposal; only the runtime completes it.

        This cannot replace a plan, escape a review or accept a worker result.
        """
        digest=hashlib.sha256(encoded(dict(text=text)).encode()).hexdigest()
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            if authorization: authorization(db)
            previous=db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                (channel.task_id,channel.channel_id,request_key)).fetchone()
            if previous:
                if previous['operation']!='answer_submit' or previous['digest']!=digest:
                    raise StoreError('request_key_conflict')
                return json.loads(previous['result_json'])
            task=db.execute('SELECT * FROM runtime_tasks WHERE task_id=?',(channel.task_id,)).fetchone()
            if not task or task['state'] not in {'PLANNING','ANSWER_READY'} or not direct_answer_allowed(db, task):
                raise StoreError('direct_answer_not_available')
            if task['state']=='ANSWER_READY':
                if task['synthesis']!=text: raise StoreError('answer_conflict')
            else:
                db.execute('UPDATE runtime_tasks SET synthesis=? WHERE task_id=?',(text,channel.task_id))
                event(db,channel.task_id,'ANSWER_SUBMITTED',dict(text=text,request_key=request_key),channel.channel_id)
                transition(db,channel.task_id,'ANSWER_READY')
            receipt=dict(recorded=True,state='ANSWER_READY')
            db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                       (channel.task_id,request_key,channel.channel_id,'answer_submit',digest,encoded(receipt)))
            db.commit()
        return receipt

    def cancel(self, task_id):
        """Idempotently fence further results and completion before process cleanup."""
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            row = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            if row and row['state'] not in TERMINAL:
                transition(db, task_id, 'CANCELLED')
            db.commit()

    def change_fingerprint(self, task_id):
        """One cheap read that changes whenever scheduler input may have.

        The controller skips its full pass while this value is unchanged. It is
        a comparison token only, never state: the database stays the source of
        truth and a periodic full pass covers anything it does not capture.
        """
        with self.store.connection() as db:
            row = db.execute(
                "SELECT t.state,t.revision,t.updated_at,t.review_cycle,t.blocker,"
                "length(t.plan_json),length(t.synthesis),"
                "(SELECT MAX(id) FROM runtime_events WHERE task_id=t.task_id),"
                "(SELECT MAX(id) FROM task_events WHERE task_id=t.task_id),"
                "(SELECT COUNT(*)||':'||COALESCE(group_concat(node_id||'/'||status||'/'"
                "||COALESCE(assignment_ref,'')||'/'||(result_json IS NOT NULL),','),'') "
                "FROM runtime_nodes WHERE task_id=t.task_id AND revision=t.revision) "
                "FROM runtime_tasks t WHERE t.task_id=?", (task_id,),
            ).fetchone()
        return tuple(row) if row is not None else None

    @staticmethod
    def _ready_nodes(db, task_id):
        """Select reservable nodes without writing; longest remaining chain first."""
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if task is None or task['state'] not in {'EXECUTING', 'REVIEWING'}:
            return task, []
        nodes = [dict(n) for n in db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid',
                                            (task_id, task['revision']))]
        completed = {n['node_id'] for n in nodes if n['status'] == 'completed'}
        stage = 'review' if task['state'] == 'REVIEWING' else 'execution'
        ready = [n for n in nodes if n['status'] == 'pending' and n['stage'] == stage
                 and set(json.loads(n['dependencies_json'])) <= completed]
        if not ready:
            return task, []
        # Only this task's own in-flight workers hold a write scope or a
        # worker slot; no other task -- live, blocked or long dead -- can
        # keep this one from starting. An earlier revision's
        # node still marked 'running' holds scope only while its worker
        # is not yet released; a stale marker alone never blocks a new plan.
        active_nodes = db.execute("""SELECT n.scope_json, n.role, n.stage FROM runtime_nodes n WHERE n.task_id=? AND (
            (n.status='running' AND (n.revision=? OR NOT EXISTS (
                SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id AND e.type='WORKER_RELEASED'
                AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref)))
            OR (n.assignment_ref IS NOT NULL AND EXISTS (
                SELECT 1 FROM delegation_provisioning p WHERE p.idempotency_key=n.assignment_ref
            ) AND NOT EXISTS (
                SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id AND e.type='WORKER_RELEASED'
                AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref
            )))""", (task_id, task['revision'])).fetchall()
        # Start first what unblocks the longest remaining dependency chain;
        # ties keep plan (rowid) order. Graph structure only, no model call.
        dependents = {}
        for n in nodes:
            for dependency in json.loads(n['dependencies_json']):
                dependents.setdefault(dependency, []).append(n['node_id'])
        chain = {}

        def remaining(node_id, visiting=frozenset()):
            if node_id not in chain:
                below = [remaining(d, visiting | {node_id}) for d in dependents.get(node_id, ())
                         if d not in visiting]
                chain[node_id] = 1 + max(below, default=0)
            return chain[node_id]

        ready.sort(key=lambda n: -remaining(n['node_id']))
        from .policy import overlaps
        # Import lazily: gateway imports this module for shared state helpers.
        from .gateway import READ_ONLY
        occupied = [
            json.loads(n['scope_json']) for n in active_nodes
            if n['stage'] != 'review' and n['role'] not in READ_ONLY
        ]
        selected = []
        for candidate in ready:
            scope = json.loads(candidate['scope_json'])
            read_only = candidate['stage'] == 'review' or candidate['role'] in READ_ONLY
            if not read_only and any(overlaps(a,b) for grant in occupied for a in grant for b in scope):
                continue
            selected.append(candidate)
            if not read_only:
                occupied.append(scope)
        # At most MAX_WORKERS_PER_SESSION workers of this chat session run
        # at once, whatever their engine or role; the rest stay pending.
        from ..delegation.capacity import MAX_WORKERS_PER_SESSION
        free = MAX_WORKERS_PER_SESSION - TaskMachine._session_worker_slots(db, task_id, task['revision'])
        return task, selected[:max(0, free)]

    @staticmethod
    def _session_worker_slots(db, task_id, revision):
        """Worker slots this task holds: every claimed, unfinished node of
        the current revision (queued, opening or working), plus every worker
        whose engine still holds a capacity reservation after its node
        finished or its revision was replaced. A reservation is released or
        reclaimed once its engine is observed gone, so a finished worker
        never holds a slot for good."""
        return db.execute("""SELECT COUNT(*) FROM (
            SELECT COALESCE(assignment_ref, 'node:'||node_id) FROM runtime_nodes
             WHERE task_id=? AND revision=? AND status='running'
            UNION
            SELECT COALESCE(p.idempotency_key, 'attempt:'||r.attempt_id) FROM capacity_reservations r
              LEFT JOIN delegation_provisioning p ON p.attempt_id=r.attempt_id
             WHERE r.task_id=? AND r.kind='worker' AND r.state IN ('reserved','ambiguous'))""",
            (task_id, revision, task_id)).fetchone()[0]

    def claim_ready(self, task_id, channel_id):
        """Reserve dependency-ready nodes atomically.

        A plain read looks for candidates first, so an idle scheduler pass
        never takes the write lock; the selection is repeated under it.
        """
        with self.store.connection() as db:
            if not self._ready_nodes(db, task_id)[1]:
                return []
            db.execute('BEGIN IMMEDIATE')
            task, ready = self._ready_nodes(db, task_id)
            if not ready:
                return []
            retries = [json.loads(row[0]) for row in db.execute(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'", (task_id,))]
            for node in ready:
                retry = sum(p.get('node') == node['node_id'] and p.get('revision') == task['revision'] for p in retries)
                key = f"runtime:{task['revision']}:{node['node_id']}:{retry}"
                assignment = hashlib.sha256(f'{task_id}|{channel_id}|{key}'.encode()).hexdigest()
                db.execute('UPDATE runtime_nodes SET status=?,assignment_ref=? WHERE task_id=? AND revision=? AND node_id=?',
                           ('running', assignment, task_id, task['revision'], node['node_id']))
                node.update(assignment_ref=assignment, request_key=key)
                event(db, task_id, 'ASSIGNMENT_CREATED', dict(node=node['node_id'], assignment_ref=assignment,
                                                             role=node['role'], revision=task['revision']))
            db.commit()
        return ready

    def check_review_binding(self, task_id, assignment_ref, request_key=None):
        """Refuse a review result whose project changed since its review
        started, before anything of that result is published."""
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            digest = self._review_binding_tx(db, task_id, assignment_ref, request_key)
            db.commit()
        return digest

    def _review_binding_tx(self, db, task_id, assignment_ref, request_key=None):
        """The exact project revision a review result binds to.

        The verdict binds to the revision the reviewer was told to review.
        When the project changed since (another writer in the same checkout),
        the review is re-bound to the current revision and this result is
        refused with that reason: the reviewer re-checks the changes and
        submits again with a new request_key. A refused request_key never
        records a verdict later. Bounded by MAX_REVIEW_REBINDS; before this
        a single change refused every later submission of the review for good.
        """
        from ..delegation.revision import capture_revision
        observed = capture_revision(task_workspace.task_root(db, task_id))
        bindings = [b for b in (json.loads(r[0]) for r in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' ORDER BY id", (task_id,)))
            if b.get('assignment_ref') == assignment_ref]
        if observed['kind'] != 'observed' or not bindings:
            raise StoreError('review_project_revision_changed')
        binding = bindings[-1]
        if request_key is not None and any(b.get('refused_request_key') == request_key for b in bindings):
            raise StoreError('review_resubmit_required', 'request_key', request_key,
                             'a new request_key for a verdict on the current project revision',
                             'This request was refused because the project changed during the review. Re-check the '
                             'changes (project_git diff), then submit your verdict with a new request_key.')
        if binding['revision_digest'] == observed['dirty_digest']:
            return observed['dirty_digest']
        rebinds = len(bindings) - 1
        if rebinds >= MAX_REVIEW_REBINDS:
            raise StoreError('review_project_revision_unstable')
        event(db, task_id, 'REVIEW_STARTED', dict(
            assignment_ref=assignment_ref, revision_digest=observed['dirty_digest'],
            rebound_from=binding['revision_digest'], rebind=rebinds + 1, refused_request_key=request_key))
        db.commit()
        raise StoreError('review_project_revision_changed', 'project revision', binding['revision_digest'],
                         'the project revision this review is bound to',
                         'The project changed while you reviewed it; the review is now bound to the current revision. '
                         'Re-check what changed (project_git diff), then submit your verdict again with a new request_key.')

    def record_result(self, task_id, assignment_ref, result, request_key=None):
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            node = db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                              (task_id, assignment_ref)).fetchone()
            if not task or not node or node['revision'] != task['revision'] or task['state'] in TERMINAL:
                raise StoreError('stale_assignment_result')
            if node['result_json'] is not None:
                retained = json.loads(node['result_json'])
                retained.pop('report_digest', None)
                retained.pop('project_digest', None)
                if encoded(retained) != encoded(result):
                    raise StoreError('conflicting_assignment_result')
                return
            if node['status'] != 'running':
                raise StoreError('assignment_not_running')
            # Durable publication identity and ownership, never final chat text.
            report = db.execute('SELECT * FROM reports WHERE id=? AND task_id=?',
                                (result.get('report_ref'), task_id)).fetchone()
            provision = db.execute('SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=? AND task_id=?',
                                   (assignment_ref, task_id)).fetchone()
            if report is None or provision is None:
                raise StoreError('assignment_report_missing')
            published = db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref IN (?,?) "
                                   "AND event_type='worker_result' AND superseded=0",
                                   (task_id, assignment_ref, provision['attempt_id'])).fetchall()
            if not any(json.loads(r['payload_json']).get('report_ref') == result['report_ref'] for r in published):
                raise StoreError('assignment_report_mismatch')
            from .artifacts import report_digest
            result = dict(result, report_digest=report_digest(self.store, task_id, report['filename']))
            checks = result.get('checks', [])
            from ..delegation.evidence import open_failure
            # A current failure fails the node: a failed check that no exact
            # later rerun of the same command closed (its closed_by, derived
            # again here), or a pass without its own verified, still-latest
            # run (result_submit already records an unverifiable pass as
            # unverified; this backstops older retained requests). A closed
            # failure stays in the result as run history. not_run/unverified
            # are honest disclosures, not failures: an investigation's
            # evidence steps are rarely runtime command_run receipts, and
            # failing on them stopped every one. They stay in the result for
            # review, verification and synthesis.
            failed = open_failure(db, task_id, provision['attempt_id'], checks)
            status = 'failed' if failed else 'completed'
            if node['stage'] == 'review' and result.get('verdict') not in {'APPROVE', 'REQUEST_CHANGES', 'BLOCK'}:
                raise StoreError('review_verdict_required')
            if node['stage'] == 'review':
                result['project_digest'] = self._review_binding_tx(db, task_id, assignment_ref, request_key)
            db.execute('UPDATE runtime_nodes SET status=?,result_json=? WHERE task_id=? AND revision=? AND node_id=?',
                       (status, encoded(result), task_id, task['revision'], node['node_id']))
            event(db, task_id, 'WORKER_COMPLETED' if not failed else 'WORKER_FAILED',
                  dict(node=node['node_id'], assignment_ref=assignment_ref, result=result))
            db.commit()

    def start_review(self, task_id, assignment_ref):
        from ..delegation.revision import capture_revision
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            observed = capture_revision(task_workspace.task_root(db, task_id))
            if observed['kind'] != 'observed':
                raise StoreError('review_revision_unavailable')
            event(db, task_id, 'REVIEW_STARTED', dict(assignment_ref=assignment_ref,
                  revision_digest=observed['dirty_digest']))
            db.commit()
        return observed['dirty_digest']

    def add_review(self, task_id):
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            nodes = db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND revision=?', (task_id, task['revision'])).fetchall()
            if not nodes or any(n['status'] != 'completed' for n in nodes):
                raise StoreError('execution_incomplete')
            # Review exists to check changes: planned work that changed no
            # project file (advice, investigation) goes straight to
            # verification, with the runtime's observation retained.
            waiver = review_waiver(db, task_id, task['revision'], nodes)
            cycle = task['review_cycle'] + 1
            if waiver:
                event(db, task_id, 'REVIEW_NOT_REQUIRED', dict(waiver, revision=task['revision']))
                transition(db, task_id, 'VERIFYING')
            elif cycle > MAX_REVIEW_CYCLES:
                transition(db, task_id, 'BLOCKED', blocker='review_cycle_limit')
            else:
                key = f'_review_{cycle}'
                db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                           (task_id, task['revision'], key, 'worker-code-reviewer',
                            'Independently review the original objective, implementation and retained reports. Submit an explicit review verdict.',
                            encoded([n['node_id'] for n in nodes]), encoded(['**']), 'review', 'pending', None, None))
                db.execute('UPDATE runtime_tasks SET review_cycle=? WHERE task_id=?', (cycle, task_id))
                transition(db, task_id, 'REVIEWING')
            db.commit()

    def add_fix(self, task_id, review):
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            if task['state'] != 'REVIEWING':
                raise StoreError('review_not_expected')
            if task['review_cycle'] >= MAX_REVIEW_CYCLES:
                transition(db, task_id, 'BLOCKED', blocker='review_cycle_limit')
            else:
                tasks=json.loads(task['plan_json'])['tasks']
                original={n['id'] for n in tasks}
                implementations=[n for n in db.execute("SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='execution' ORDER BY rowid",
                                                       (task_id,task['revision'])).fetchall() if n['node_id'] in original]
                targets,fallback=targeted_fixes(tasks,{n['node_id']: json.loads(n['scope_json']) for n in implementations},
                                                review['result'].get('findings'),review['result'].get('report_ref'))
                keys={n['node_id']: f"_fix_{task['review_cycle']}_{n['node_id']}" for n in implementations}
                parents={task['id']: task['dependencies'] for task in tasks}
                fixes=[]
                for implementation in implementations:
                    node_id=implementation['node_id']
                    if targets is None:
                        objective='Address review findings within the assigned scope: '+review['result']['summary']
                        dependencies=[review['node_id'],*(keys[parent] for parent in parents[node_id]
                                                          if parent in keys)]
                    elif node_id in targets:
                        objective,dependencies=targets[node_id]['objective'],[review['node_id'],*(keys[n] for n in targets[node_id]['after'])]
                    else:
                        continue
                    db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                               (task_id,task['revision'],keys[node_id],implementation['role'],objective,
                                encoded(dependencies),implementation['scope_json'],'execution','pending',None,None))
                    fixes.append(keys[node_id])
                event(db,task_id,'REVIEW_CHANGES_REQUESTED',dict(review=review['node_id'],fixes=fixes,
                                                                  targeted=targets is not None,fallback=fallback))
                transition(db, task_id, 'EXECUTING')
            db.commit()

    def submit_synthesis(self, channel, text, not_verified=(), *, authorization=None):
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            if authorization: authorization(db)
            task = db.execute('SELECT state,synthesis,revision FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
            if not task or task['state'] != 'RECONCILING':
                raise StoreError('synthesis_not_expected')

            def normalized(name):
                return ' '.join(str(name).split()).casefold()

            rows = db.execute(
                'SELECT node_id,assignment_ref,result_json FROM runtime_nodes '
                'WHERE task_id=? AND revision=? AND result_json IS NOT NULL',
                (channel.task_id, task['revision']),
            ).fetchall()
            from ..delegation.evidence import assess_checks, current_outcomes

            checks = []
            for row in rows:
                result_checks = json.loads(row['result_json']).get('checks', [])
                provision = db.execute(
                    'SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?',
                    (channel.task_id, row['assignment_ref']),
                ).fetchone() if row['assignment_ref'] else None
                outcomes = current_outcomes(db, channel.task_id, provision['attempt_id'], result_checks) if provision else None
                assessments = assess_checks(db, channel.task_id, provision['attempt_id'], result_checks) if provision else None
                for index, check in enumerate(result_checks):
                    checks.append((row['node_id'], check, outcomes[index] if outcomes else None,
                                   assessments[index] if assessments else None))

            def fix_origin(node):
                match = re.fullmatch(r'_fix_(\d+)_([A-Za-z][A-Za-z0-9_-]*)', node)
                return (match.group(2), int(match.group(1))) if match else (node, -1)

            def same_lineage(gap_node, pass_node):
                if gap_node == pass_node:
                    return True
                gap_origin, gap_cycle = fix_origin(gap_node)
                pass_origin, pass_cycle = fix_origin(pass_node)
                return gap_origin == pass_origin and pass_cycle >= 0 and pass_cycle >= gap_cycle

            passes = [(node, check, current, assessment) for node, check, current, assessment in checks
                      if check.get('outcome') == 'passed' and current is not None
                      and current['outcome'] == 'passed' and assessment['strength'] == 'verified']
            gaps = {}
            for node, check, _, _ in checks:
                if check.get('outcome') not in {'not_run', 'unverified'}:
                    continue
                name = normalized(check['name'])
                command = check.get('command')
                if command:
                    resolved = isinstance(command, list) and any(
                        assessment.get('identity') == ('command_run', tuple(command), None)
                        for _, _, _, assessment in passes
                    )
                else:
                    # Compatibility for retained gaps without a declared command.
                    resolved = any(
                        same_lineage(node, passed_node) and normalized(passed_check['name']) == name
                        for passed_node, passed_check, _, _ in passes
                    )
                if not resolved:
                    gaps[name] = check['name']
            for index, item in enumerate(not_verified):
                if not item['blocker'].strip():
                    raise StoreError('synthesis_blocker_required', f'arguments.not_verified[{index}].blocker',
                                     'blank', 'a concrete blocker that made verification impossible',
                                     'Verify this item or state the concrete blocker.')
            listed = {normalized(item['item']) for item in not_verified}
            missing = [name for key, name in gaps.items() if key not in listed]
            if missing:
                raise StoreError('synthesis_gaps_undisclosed', 'arguments.not_verified', ', '.join(missing)[:160],
                                 'every retained not_run or unverified check by its exact name, each with its blocker',
                                 'A gap with a declared command closes only after a verified command_run with exactly matching argv; a command-less gap may close by a verified same-name pass in its node or later fix. Close these gaps with replan_request if they can be verified; otherwise list each exact name in not_verified with its concrete blocker: ' + '; '.join(missing))
            stored = text if not not_verified else (
                text.rstrip() + '\n\n## Not verified\n\n' +
                '\n'.join(f"- {item['item']} — blocker: {item['blocker']}" for item in not_verified)
            )
            if task['synthesis'] is not None and task['synthesis'] != stored:
                raise StoreError('synthesis_conflict')
            if task['synthesis'] == stored:
                return dict(recorded=True,state='RECONCILING')
            db.execute('UPDATE runtime_tasks SET synthesis=? WHERE task_id=?', (stored, channel.task_id))
            event(db, channel.task_id, 'SYNTHESIS_SUBMITTED', dict(text=stored, not_verified=list(not_verified)), channel.channel_id)
            db.commit()
        return dict(recorded=True)

    def record_verification(self, task_id, checks, revision_digest):
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            if not task or task['state'] != 'VERIFYING':
                raise StoreError('verification_not_expected')
            if reviewed_digest(db, task_id, task['revision']) != revision_digest:
                transition(db, task_id, 'BLOCKED', blocker='verification_review_revision_changed')
                db.commit()
                return False
            commands = json.loads(task['plan_json'])['verification']
            passed = (len(checks) == len(commands) and len(revision_digest) == 64
                      and all(c.get('argv') == argv and c.get('exit_code') == 0 and c.get('complete') is True
                              and c.get('revision_digest') == revision_digest for c, argv in zip(checks, commands)))
            event(db, task_id, 'VERIFICATION_PASSED' if passed else 'VERIFICATION_FAILED',
                  dict(revision=task['revision'], revision_digest=revision_digest, checks=checks))
            failures = db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='VERIFICATION_FAILED'",
                                  (task_id,)).fetchone()[0]
            if passed:
                transition(db, task_id, 'RECONCILING')
            elif failures <= MAX_VERIFICATION_REPAIRS:
                # A wrong or failing check goes back to the coordinator with
                # its output to repair (verification_submit, or a new plan
                # when the code is wrong) -- never a stop for a mistake.
                transition(db, task_id, 'PLANNING')
            else:
                transition(db, task_id, 'BLOCKED', blocker='verification_failed')
            db.commit()
        return passed

    def return_to_coordinator(self, task_id, reason):
        """Hand the task back to the coordinator (RECOVERING, then PLANNING)
        instead of stopping, at most MAX_COORDINATOR_RETURNS times per task."""
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            returned = db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='RETURNED_TO_COORDINATOR'",
                                  (task_id,)).fetchone()[0]
            if returned >= MAX_COORDINATOR_RETURNS:
                db.commit()
                return False
            event(db, task_id, 'RETURNED_TO_COORDINATOR', dict(reason=reason))
            # As steering does: nothing of the old plan keeps holding its
            # scope (a node left 'running' blocked the new plan forever).
            db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')", (task_id,))
            db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL",
                       (now(), task_id))
            transition(db, task_id, 'RECOVERING')
            db.commit()
        return True

    def submit_verification(self, channel, verification, request_key, *, authorization=None):
        """Replace only the plan's verification after it failed, and verify
        again -- the reviewed work is not redone."""
        digest = hashlib.sha256(encoded(dict(verification=verification)).encode()).hexdigest()
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            if authorization: authorization(db)
            previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                  (channel.task_id, channel.channel_id, request_key)).fetchone()
            if previous:
                if previous['operation'] != 'verification_submit' or previous['digest'] != digest:
                    raise StoreError('request_key_conflict')
                return json.loads(previous['result_json'])
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
            last = db.execute("SELECT type FROM runtime_events WHERE task_id=? AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED','PLAN_ACCEPTED') ORDER BY id DESC LIMIT 1",
                              (channel.task_id,)).fetchone()
            nodes = db.execute('SELECT status FROM runtime_nodes WHERE task_id=? AND revision=?',
                               (channel.task_id, task['revision'] if task else -1)).fetchall()
            if (not task or task['state'] != 'PLANNING' or not last or last[0] != 'VERIFICATION_FAILED'
                    or not nodes or any(n['status'] != 'completed' for n in nodes)):
                raise StoreError('verification_repair_not_expected')
            plan = json.loads(task['plan_json'])
            plan['verification'] = verification
            db.execute('UPDATE runtime_tasks SET plan_json=? WHERE task_id=?', (encoded(plan), channel.task_id))
            event(db, channel.task_id, 'VERIFICATION_REVISED', dict(verification=verification), channel.channel_id)
            transition(db, channel.task_id, 'VERIFYING')
            receipt = dict(recorded=True, state='VERIFYING')
            db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                       (channel.task_id, request_key, channel.channel_id, 'verification_submit', digest, encoded(receipt)))
            db.commit()
        return receipt

    def _completion_invariants(self, db, task_id):
        if db.execute('SELECT 1 FROM task_stopped WHERE task_id=?', (task_id,)).fetchone():
            raise StoreError('completion_task_stopped')
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if task['state']=='ANSWER_READY':
            if not task['synthesis'] or task['blocker'] or not direct_answer_allowed(db, task):
                raise StoreError('completion_answer_invariant')
            proposal=db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ANSWER_SUBMITTED' ORDER BY id DESC LIMIT 1",(task_id,)).fetchone()
            if not proposal or json.loads(proposal[0]).get('text')!=task['synthesis']:
                raise StoreError('completion_answer_missing')
            return
        nodes = db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid',
                           (task_id, task['revision'])).fetchall()
        if not nodes or any(n['status'] != 'completed' or not n['result_json'] for n in nodes):
            raise StoreError('completion_nodes_incomplete')
        for node in nodes:
            result = json.loads(node['result_json'])
            report = db.execute('SELECT filename FROM reports WHERE task_id=? AND id=?',
                                (task_id, result.get('report_ref'))).fetchone()
            from .artifacts import report_digest
            if report is None or report_digest(self.store, task_id, report['filename']) != result.get('report_digest'):
                raise StoreError('completion_artifact_changed_or_missing')
        reviews = [n for n in nodes if n['stage'] == 'review']
        if reviews and json.loads(reviews[-1]['result_json']).get('verdict') != 'APPROVE':
            raise StoreError('completion_review_missing')
        if not reviews and not current_review_waiver(db, task_id):
            raise StoreError('completion_review_missing')
        # The latest verification run decides, never the latest passing one:
        # a failure recorded after an earlier pass is the current outcome.
        evidence = db.execute("SELECT type,payload_json FROM runtime_events WHERE task_id=? "
                              "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                              (task_id,)).fetchone()
        if (not evidence or evidence['type'] != 'VERIFICATION_PASSED'
                or json.loads(evidence['payload_json'])['revision'] != task['revision']):
            raise StoreError('completion_verification_missing')
        if reviewed_digest(db, task_id, task['revision']) != json.loads(evidence['payload_json'])['revision_digest']:
            raise StoreError('completion_review_missing')
        from ..delegation.revision import capture_revision
        observed = capture_revision(task_workspace.task_root(db, task_id))
        if observed['kind'] != 'observed' or observed['dirty_digest'] != json.loads(evidence['payload_json'])['revision_digest']:
            raise StoreError('completion_project_revision_changed')
        if task['blocker'] or not task['synthesis']:
            raise StoreError('completion_synthesis_or_blocker')
