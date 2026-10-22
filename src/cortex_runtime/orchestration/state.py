"""Durable, transactional task lifecycle and dependency scheduler.

No transition, permission, model, or worker handle is accepted from a model.
State and events commit together in the project's existing SQLite database.
"""
from __future__ import annotations

from datetime import datetime, timezone
from contextlib import contextmanager
import hashlib
import json
import os
import re
import threading
import time
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
    # -> REVIEWING only when another Cortex task changed the shared project
    # (TaskMachine._recheck_after_foreign_change_tx).
    'VERIFYING': {'RECONCILING', 'PLANNING', 'REVIEWING'},
    'RECONCILING': {'COMPLETED', 'WAITING_USER', 'REVIEWING'},
    # Sessionless recovered workers use the task-global question fallback.
    'WAITING_USER': {'PLANNING', 'RECONCILING', 'EXECUTING', 'REVIEWING'},
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


# Retry delays after infrastructure failures (orchestration.recovery
# .retry_lost), held in this process only: an idle scheduler pass then costs
# no extra read. The delay is also on the ASSIGNMENT_RETRIED event for the
# record; a restarted runtime simply retries at once.
_RETRY_HOLDS = {}
_RETRY_HOLDS_LOCK = threading.Lock()


def hold_retry(task_id, revision, node_id, *, until, retry, reason):
    with _RETRY_HOLDS_LOCK:
        _RETRY_HOLDS[(task_id, revision, node_id)] = dict(
            until=until, info=dict(kind='retry', retry=retry, reason=reason, until=until))


def event(db, task_id, kind, payload, actor='runtime'):
    from ..observability.wire_trace import get_trace
    trace = get_trace()
    if trace is not None:
        # Attempted transactional insert, not evidence of a later commit.
        identity = payload if isinstance(payload, dict) else {}
        trace.record("lifecycle", "internal", dict(type=kind, actor=actor, payload=payload,
                     transaction="pending"), task_id=task_id,
                     assignment_ref=identity.get("assignment_ref"),
                     attempt_id=identity.get("attempt_id"),
                     engine_session_id=identity.get("engine_session_id"))
    db.execute('INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)',
               (task_id, kind, actor, encoded(payload), now()))


#: What a writing worker does to a shared project tree.
_PROJECT_WRITE_OPERATIONS = ('project_write', 'project_edit', 'project_delete', 'project_mkdir',
                             'project_skill_link', 'command_run', 'outside_command', 'outside_write')


def dropped_failed_checks_unacknowledged(db, task_id):
    """The failed verification commands the latest revision of this plan's
    verification removed, when the user has not answered since: twice a
    popup placement check that failed only for environment reasons was
    dropped and the task completed on lint and unit tests, so the user's
    "fixed" popup still moved (2026-10-20/22)."""
    accepted = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type='PLAN_ACCEPTED'",
                          (task_id,)).fetchone()[0] or 0
    revised = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='VERIFICATION_REVISED' "
                         "AND id>? ORDER BY id DESC LIMIT 1", (task_id, accepted)).fetchone()
    if revised is None:
        return []
    dropped = json.loads(revised['payload_json']).get('dropped_failed') or []
    if not dropped:
        return []
    answered = db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='USER_ANSWER_RECEIVED' AND id>? LIMIT 1",
                          (task_id, revised['id'])).fetchone()
    return [] if answered else dropped


def foreign_project_writers(db, task_id, since):
    """Other tasks of this project's store whose writing workers acted on
    the project at or after ``since`` (read-only roles' commands discard
    their project writes and are not counted)."""
    from .contracts import READ_ONLY
    marks = ','.join('?' * len(_PROJECT_WRITE_OPERATIONS))
    rows = db.execute(
        "SELECT DISTINCT e.task_id, p.role_id FROM runtime_events e "
        "JOIN delegation_provisioning p ON p.channel_id=e.actor AND p.task_id=e.task_id "
        "WHERE e.task_id!=? AND e.type='TOOL_AUTHORIZED' AND e.created_at>=? "
        f"AND json_extract(e.payload_json,'$.operation') IN ({marks})",
        (task_id, since, *_PROJECT_WRITE_OPERATIONS)).fetchall()
    return sorted({row['task_id'] for row in rows if resolve_role(row['role_id']) not in READ_ONLY})


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


@contextmanager
def observed_project(store, task_id, needed=True):
    """Scan the task's project before the caller's write transaction, so a
    capture inside it reuses that scan instead of holding the store's write
    lock for its duration (see delegation.revision.observation_snapshot)."""
    root = None
    if needed:
        try:
            with store.connection() as db:
                root = task_workspace.task_root(db, task_id)
        except StoreError:
            root = None
    if root is None:
        yield
        return
    from ..delegation.revision import observation_snapshot
    with observation_snapshot(store, root):
        yield


def _observe_project(db, task_id):
    from ..delegation.revision import capture_revision
    return capture_revision(task_workspace.task_root(db, task_id))


def _record_execution_baseline(db, task_id):
    """Observe the project as a plan revision starts executing.

    Review is decided later against this runtime observation, never against
    a model's claim that nothing changed."""
    revision = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()['revision']
    observed = _observe_project(db, task_id)
    from .reviewer_handoff import path_metadata
    event(db, task_id, 'EXECUTION_BASELINE', dict(revision=revision, kind=observed['kind'], head=observed['head'],
                                                  revision_digest=observed['dirty_digest'],
                                                  path_metadata=path_metadata(observed)))


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


# Kept equal to acp.messages stop_option_continue/stop_option_finish (en).
# Finish is the owner's force-finish (questions.stop_choice): CANCELLED with
# USER_FORCE_FINISHED. Continue resumes the runtime flow and never releases
# the stopped work to a direct answer (released_by_user).
STOP_OPTIONS = ('Continue: let the coordinator decide how to proceed',
                'Finish: close the task as is, without certifying unfinished work')


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
    from .worker_waits import cancel_tx
    if source != 'WAITING_USER' and _coordinator_should_ask(db, task_id, blocker):
        event(db, task_id, 'ASK_USER_REQUIRED', dict(blocker=blocker))
        # The stopped plan's open worker questions end with it: otherwise
        # the next user message is swallowed as a late answer to them.
        cancel_tx(db, task_id, 'task_stopped_for_question')
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
    # Two open questions (a stopped worker's and this one) would make the
    # user disambiguate with /cortex-answer; only the stop question remains.
    cancel_tx(db, task_id, 'task_stopped_for_question')
    from .questions import STOP_QUESTION_DEDUP_PREFIX, STOP_QUESTION_PREFIX, STOP_QUESTION_SUFFIX
    question_id = new_id('pq_')
    text = f'{STOP_QUESTION_PREFIX}{blocker}{STOP_QUESTION_SUFFIX}'
    # Structured markers, never the text alone, identify this as the
    # runtime's own stop question (questions.runtime_stop_request and
    # questions.runtime_stop_dedup_key).
    db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at) VALUES(?,?,?,?,?,'open',?)",
               (question_id, task_id, f'{STOP_QUESTION_DEDUP_PREFIX}{question_id}', text, encoded(list(STOP_OPTIONS)), now()))
    event(db, task_id, 'USER_QUESTION_REQUESTED', dict(question_id=question_id, question=text, options=list(STOP_OPTIONS),
                                                       resume_state='RECOVERING', blocker=blocker, revision=revision,
                                                       runtime_stop=True))
    db.execute('UPDATE runtime_tasks SET state=?,blocker=?,updated_at=? WHERE task_id=?',
               ('WAITING_USER', blocker, now(), task_id))
    event(db, task_id, 'TASK_STATE_CHANGED', dict(previous=source, state='WAITING_USER', blocker=blocker))


def released_by_user(db, task):
    """The user answered a runtime stop question for this plan revision, or
    a question the coordinator asked after this revision was verified.

    Only then may the coordinator close delegated work with a direct answer;
    it can never skip review or verification on its own. Only an answer in
    the user's own words releases a stop question: its Continue resumes the
    runtime flow and its Finish force-finishes the task. A released answer
    still completes only certified work (answer_needs_certified_work)."""
    from .questions import runtime_stop_request, stop_choice
    if db.execute("SELECT 1 FROM runtime_nodes WHERE task_id=? AND status IN ('pending','running')",
                  (task['task_id'],)).fetchone():
        return False
    asked = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'",
                       (task['task_id'],)).fetchall()
    stops = {q['question_id']: q for q in map(lambda r: json.loads(r[0]), asked)
             if (q.get('blocker') or q.get('after_verification')) and q.get('revision') == task['revision']}
    answered = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_ANSWER_RECEIVED'",
                          (task['task_id'],)).fetchall()
    for row in answered:
        received = json.loads(row[0])
        asked_question = stops.get(received.get('question_id'))
        if asked_question is None:
            continue
        if runtime_stop_request(asked_question) and stop_choice(
                received.get('answer'), asked_question.get('question'), runtime_stop=True):
            # The runtime's own options: Continue resumes the flow, Finish
            # force-finishes; neither hands the stopped work to an answer.
            continue
        return True
    return False


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


def steered_after_verification(db, task):
    """A user message returned the verified revision to planning before it
    completed, and nothing was planned since.

    Steering waits for a boundary, so a message sent while workers, review or
    verification ran is applied once the runtime verification passed
    (RECONCILING -> RECOVERING -> PLANNING, synthesis cleared). The
    coordinator then decides what the message asks for: a plan for changes,
    or a direct answer closing the reviewed and verified work. Derived only
    from runtime events: the latest verification passed for this revision, a
    steering was applied after it, and since then no plan, assignment, replan
    or verification repair was accepted and no worker was provisioned. Such an
    answer completes only certified work (answer_needs_certified_work)."""
    task_id = task['task_id']
    verified = db.execute("SELECT id,type,payload_json FROM runtime_events WHERE task_id=? "
                          "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                          (task_id,)).fetchone()
    if (not verified or verified['type'] != 'VERIFICATION_PASSED'
            or json.loads(verified['payload_json']).get('revision') != task['revision']):
        return False
    if not db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND id>? AND type='STEERING_APPLIED'",
                      (task_id, verified['id'])).fetchone():
        return False
    if db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND id>? AND type IN "
                  "('PLAN_ACCEPTED','ASSIGNMENT_CREATED','REPLAN_REQUEST_ACCEPTED','VERIFICATION_REVISED')",
                  (task_id, verified['id'])).fetchone():
        return False
    if db.execute("SELECT 1 FROM runtime_nodes WHERE task_id=? AND status IN ('pending','running')",
                  (task_id,)).fetchone():
        return False
    return not db.execute(
        "SELECT 1 FROM delegation_provisioning p WHERE p.task_id=? AND p.role_id!='orchestrator' AND NOT EXISTS ("
        "SELECT 1 FROM runtime_events e WHERE e.task_id=p.task_id AND e.type='ASSIGNMENT_CREATED' AND e.id<? "
        "AND json_extract(e.payload_json,'$.assignment_ref')=p.idempotency_key)", (task_id, verified['id'])).fetchone()


def _informational(db, task):
    """No plan, node or worker has ever existed for this task."""
    return (task['revision'] == 0 and task['plan_json'] is None
            and not db.execute('SELECT 1 FROM runtime_nodes WHERE task_id=?', (task['task_id'],)).fetchone()
            and not db.execute("SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'",
                               (task['task_id'],)).fetchone())


def direct_answer_allowed(db, task):
    """Whether answer_submit may close the task: before any plan or worker
    exists, after the user released a stopped or verified revision, for a
    follow-up on a completed task with no work planned since it reopened, or
    after a user message returned the verified revision to planning."""
    return (released_by_user(db, task) or reopened_after_completion(db, task)
            or steered_after_verification(db, task) or _informational(db, task))


def answer_needs_certified_work(db, task):
    """Whether a direct answer closes planned work rather than informing.

    An informational answer (no plan or worker ever) and a follow-up on a
    task that already reached verified COMPLETED certify nothing new. Any
    other answer -- one the user released after a stop or after verification
    -- completes the task only when the current revision's work itself
    passes every completion gate (TaskMachine.close_answer)."""
    return not _informational(db, task) and not reopened_after_completion(db, task)


# Codes TaskMachine._work_invariants raises: what an uncertified answer lacks.
WORK_CERTIFICATION_GAPS = frozenset({
    'completion_nodes_incomplete', 'completion_artifact_changed_or_missing', 'completion_review_missing',
    'completion_verification_missing', 'completion_project_revision_changed',
})


class AttemptRouting(NamedTuple):
    escalate: bool
    complexity: str
    model_hint: str
    model_reason: str | None
    previous_engine: str | None
    # The user's own request for the exact hint's model family in this
    # task (routing.auto_route.user_model_request), for every configured tier.
    user_request: dict | None = None
    # The newer user refusal that revoked an earlier request for the exact
    # hint (routing.auto_route.user_model_revocation); never consent.
    user_revocation: dict | None = None
    work_kind: str | None = None


# A retry caused only by an infrastructure failure -- the engine/transport
# itself became unreachable, or the worker's process was lost to a host
# disconnect or a runtime restart, not a quality signal about the work --
# must not raise the model tier. Mirrors engine.failure_classification's own
# closed class names (plus ``worker_lost``, recorded by
# orchestration.recovery.retry_lost) so a future class added there is
# excluded here only by deliberate choice. Native auth replacement shares
# infrastructure timing/no-escalation accounting, with its own one-shot fence.
WORKER_LOST_FAILURE_CLASS = 'worker_lost'
_INFRASTRUCTURE_RETRY_CLASSES = frozenset({'engine_failure', 'transport_failure', WORKER_LOST_FAILURE_CLASS, 'authorization_loss'})


def attempt_routing(store, task_id, attempt_id):
    """Read a node's plan hint, prior audited engine and, for an exact route
    hint, whether the user asked for its model family in this task."""
    with store.connection() as db:
        provision = db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                               (task_id, attempt_id)).fetchone()
        node = provision and db.execute('SELECT node_id,revision,role FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
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
    # A retry whose own recorded cause is purely an infrastructure
    # failure never counts toward escalation; a retry recorded before this
    # fix (no failure_class) or for any other/quality cause still does.
    retried = any(p.get('node') == node['node_id'] and p.get('revision') == node['revision'] and not p.get('route_failure')
                  and p.get('failure_class') not in _INFRASTRUCTURE_RETRY_CLASSES
                  for p in (json.loads(r[0]) for r in retries))
    hint = task.get('model') or task.get('provider') or 'auto'
    from ..routing.auto_route import user_model_request, user_model_revocation, user_purpose_model_request
    if '/' not in hint:
        # No exact hint (the runtime's review node, or a task left on auto):
        # the model the owner asked for this role's purpose still applies,
        # ahead of pins and preferences, as an explicit request does.
        hint = user_purpose_model_request(store, task_id, resolve_role(node['role'])) or hint
    return AttemptRouting(bool(fix) or retried, task.get('complexity', 'normal'), hint,
                          task.get('model_reason'), prior['engine'] if prior else None,
                          user_model_request(store, task_id, hint) if '/' in hint else None,
                          user_model_revocation(store, task_id, hint) if '/' in hint else None,
                          task.get('work_kind'))


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


def fix_role(role, scope):
    """Keep writing specialists; replace non-writers by the reviewed scope.

    Documentation-only scopes use technical-writer, tests-only scopes use
    qa-engineer, and all other (including mixed or whole-project) scopes use
    backend-dev. Use the runtime's access classes, not a second role list.
    Scope is retained exactly: findings never authorize a wider grant.
    """
    from .contracts import READ_ONLY
    role = resolve_role(role)
    if role not in READ_ONLY:
        return role
    paths = [PurePosixPath(path) for path in scope]
    if paths and all(path.parts and (path.parts[0] == 'docs' or path.suffix == '.md')
                     for path in paths):
        return 'worker-technical-writer'
    if paths and all(path.parts and path.parts[0] == 'tests' for path in paths):
        return 'worker-qa-engineer'
    return 'worker-backend-dev'


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


def synthesis_checks(db, task_id, revision):
    """Shared current check assessments and unresolved disclosed gaps."""
    def normalized(name):
        return ' '.join(str(name).split()).casefold()

    rows = db.execute(
        'SELECT node_id,assignment_ref,result_json FROM runtime_nodes '
        'WHERE task_id=? AND revision=? AND result_json IS NOT NULL',
        (task_id, revision),
    ).fetchall()
    from ..delegation.evidence import assess_checks, current_outcomes
    from .check_closure import retain_runtime_closures
    closures = retain_runtime_closures(db, task_id, revision)

    checks = []
    for row in rows:
        result_checks = json.loads(row['result_json']).get('checks', [])
        provision = db.execute(
            'SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?',
            (task_id, row['assignment_ref']),
        ).fetchone() if row['assignment_ref'] else None
        outcomes = current_outcomes(db, task_id, provision['attempt_id'], result_checks) if provision else None
        assessments = assess_checks(db, task_id, provision['attempt_id'], result_checks) if provision else None
        for index, check in enumerate(result_checks):
            checks.append((row['node_id'], check, outcomes[index] if outcomes else None,
                           assessments[index] if assessments else None,
                           closures.get((row['assignment_ref'], index))))

    def fix_origin(node):
        match = re.fullmatch(r'_fix_(\d+)_([A-Za-z][A-Za-z0-9_-]*)', node)
        return (match.group(2), int(match.group(1))) if match else (node, -1)

    def same_lineage(gap_node, pass_node):
        if gap_node == pass_node:
            return True
        gap_origin, gap_cycle = fix_origin(gap_node)
        pass_origin, pass_cycle = fix_origin(pass_node)
        return gap_origin == pass_origin and pass_cycle >= 0 and pass_cycle >= gap_cycle

    from .. import task_workspace
    from ..configuration.access_mode import command_profile, READ_ONLY_COMMAND_PROFILE, GUARDED_COMMAND_PROFILE
    from .gateway import _command_policy
    command_cwd = task_workspace.task_root(db, task_id)
    # A pass closes a gap only in a profile a command of this task gets now:
    # the one the task's mode gives a writing worker, or the read-only host
    # profile that reviewers, other read-only roles and runtime verification
    # get in every mode. The Guarded writer profile is never wider than
    # either, and also names read-only passes recorded before the read-only
    # profile had its own name, so it still closes a gap. A host-writable
    # pass never closes a gap once the task is Guarded.
    profiles = {command_profile(_command_policy(db, task_id)['mode']), READ_ONLY_COMMAND_PROFILE,
                GUARDED_COMMAND_PROFILE}
    passes = [(node, check, current, assessment) for node, check, current, assessment, _ in checks
              if check.get('outcome') == 'passed' and current is not None
              and current['outcome'] == 'passed' and assessment['strength'] == 'verified']
    gaps = {}
    for node, check, _, _, runtime_closure in checks:
        if runtime_closure or check.get('outcome') not in {'not_run', 'unverified'}:
            continue
        name = normalized(check['name'])
        command = check.get('command')
        if command:
            resolved = isinstance(command, list) and any(
                assessment.get('identity') == ('command_run', tuple(command), command_cwd, profile)
                for _, _, _, assessment in passes for profile in profiles
            )
        else:
            # Compatibility for retained gaps without a declared command.
            resolved = any(
                same_lineage(node, passed_node) and normalized(passed_check['name']) == name
                for passed_node, passed_check, _, _ in passes
            )
        if not resolved:
            gaps[name] = check['name']

    return checks, gaps


def _has_authored_not_verified_section(text, localized_messages):
    """Recognize reserved disclosure headings, not arbitrary semantic similarity.

    Refuse rather than erase sections: they can contain unique context as well
    as repeated gaps. Ignore fenced/indented code and quoted examples. The
    vocabulary covers both chat languages independently of the selected locale.
    """
    headings = {localized_messages.render('not_verified_heading', lang).casefold() for lang in ('en', 'ru')}
    headings.update({
        'what is not verified', 'what was not verified', 'what was not checked',
        'what was not tested', 'unverified', 'unverified checks', 'unverified items',
        'verification gaps', 'not checked', 'not tested', 'checks not run',
        'tests not run', 'skipped checks', 'skipped tests',
        'что не проверено', 'что не было проверено', 'что осталось непроверенным',
        'непроверенное', 'непроверенные проверки', 'непроверенные пункты',
        'не выполнено', 'невыполненные проверки', 'не запускалось',
    })

    def reserved(value):
        value = re.sub(r'[*_`]', '', value).strip(' \t:#')
        return ' '.join(value.split()).casefold() in headings

    fence = None
    previous = ''
    for line in text.splitlines():
        marker = re.match(r'^ {0,3}(`{3,}|~{3,})(.*)$', line)
        if fence:
            if marker and marker[1][0] == fence[0] and len(marker[1]) >= len(fence) and not marker[2].strip():
                fence = None
            continue
        if marker:
            fence = marker[1]
            previous = ''
            continue
        if line.startswith(('    ', '\t', '>')):
            previous = ''
            continue
        atx = re.match(r'^ {0,3}#{1,6}[ \t]+(.+?)\s*$', line)
        if atx and reserved(atx[1]):
            return True
        if re.fullmatch(r' {0,3}(?:=+|-+)[ \t]*', line) and reserved(previous):
            return True
        previous = line
    return False


def disclosed_synthesis(text, not_verified, gaps, language='en'):
    """Require and render the same gap disclosure on both final-answer paths.

    The heading and label are runtime-authored, so they follow the task's
    selected language; item, description and blocker are the coordinator's
    own text. item stays untouched (the exact check name when it closes a
    retained gap, so matching in ``listed``/``gaps`` below never depends on
    language); an item's optional description is the coordinator's own
    localized, human-readable stand-in rendered instead of the raw item.
    The exact item stays in structured evidence, not the displayed line;
    gap matching still looks only at item. A coordinator that sent no
    description (older callers, or an item that is already freeform text)
    renders exactly as before. With structured gaps, a second authored
    disclosure section is refused before persistence; the caller must retain
    unique context outside it and let the runtime render the gaps once.
    """
    normalized = lambda name: ' '.join(str(name).split()).casefold()
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
                         'A gap with a declared command closes after a verified command_run with exactly matching argv or a complete passing runtime final-verification receipt with identical argv, bound to the gap author\'s observed publication or reviewed digest and the current approved project digest, regardless of execution order; a command-less gap may close by a verified same-name pass in its node or later fix. List each exact name in not_verified with its concrete blocker (do not plan new work only to close a gap): ' + '; '.join(missing))
    if not not_verified:
        return text
    from ..acp import messages as localized_messages
    heading = localized_messages.render('not_verified_heading', language)
    label = localized_messages.render('not_verified_blocker_label', language)
    if _has_authored_not_verified_section(text, localized_messages):
        raise StoreError('synthesis_disclosure_in_text', 'arguments.text', 'authored disclosure section',
                         'gap disclosures only in not_verified, rendered once by the runtime',
                         'Remove the authored not-verified section from text, keep every gap and blocker '
                         'in not_verified, and preserve any other context elsewhere in text. Retry the submission.')

    def rendered_item(item):
        description = (item.get('description') or '').strip()
        return description if description else item['item']

    # One line per distinct blocker: the same device/assistive-technology
    # limit was listed about ten times in one final answer (2026-10-22).
    groups = {}
    for item in not_verified:
        groups.setdefault(normalized(item['blocker']), [item['blocker'], []])[1].append(rendered_item(item))
    return (
        text.rstrip() + f'\n\n## {heading}\n\n' +
        '\n'.join(f"- {'; '.join(names)} — {label}: {blocker}" for blocker, names in groups.values())
    )


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


def read_only_output_paths(nodes, project_root):
    """Refuse a read-only task's nonexistent literal scope path no writer owns.

    A read-only role's jail and tools never write, so such a path is usually
    an output it cannot create; accepted silently, its worker did the work and
    then fought the read-only project to save it (2026-09-28). The path is
    legitimate when a writing task of the same plan whose scope overlaps it is
    ordered with the read-only task: a reviewer of the directory its
    predecessor writer creates, or a task whose dependent writer creates the
    target -- the remedy this refusal prescribes. Coordinators wrote exactly
    those plans and were refused on their first attempt (2026-10-14). An
    unordered writer is not enough: the read could precede the write.
    """
    from .gateway import READ_ONLY
    from .policy import overlaps
    deps = {node['id']: set(node['dependencies']) for node in nodes}

    def ancestors(key, seen=None):
        seen = set() if seen is None else seen
        for parent in deps[key] - seen:
            seen.add(parent)
            ancestors(parent, seen)
        return seen

    before = {key: ancestors(key) for key in deps}
    writers = [node for node in nodes if resolve_role(node['role']) not in READ_ONLY]
    for node in nodes:
        if resolve_role(node['role']) not in READ_ONLY:
            continue
        ordered = [writer for writer in writers
                   if writer['id'] in before[node['id']] or node['id'] in before[writer['id']]]
        missing = [scope for scope in node['scope']
                   if not any(mark in scope for mark in '*?[')
                   and not os.path.lexists(os.path.join(project_root, scope))
                   and not any(overlaps(scope, owned) for writer in ordered for owned in writer['scope'])]
        if missing:
            field = f"tasks[{node['id']}].scope"
            correction = (f"{node['role']} is read-only and these paths do not exist: {', '.join(missing)}. "
                          'Keep only existing paths, or let a writing task whose scope covers each path create it, '
                          'ordered with this task (this task depends on it, or it depends on this task). '
                          'When this task must produce the file, have it return the content in its report and give '
                          'the file to a writing role in a dependent task.')
            raise StoreError('read_only_scope_not_writable', field, missing[0], 'an existing path to read',
                             correction, violations=[dict(field=field, received=path, expected='an existing path to read',
                                                          correction=correction) for path in missing])


CONSULTANT_ROLE = 'worker-senior-consultant'
CONSULTATION_NODE = '_consultation'


def consultation_objective(proposal):
    """The consultant's objective: the escalation's own question and reason."""
    question, reason = proposal.get('question'), proposal['reason']
    return f'{question}\n\nEscalation reason: {reason}' if question else reason


def consultation_packet(db, task_id, source_revision, proposal, objective):
    """Dependency reports of an escalation-created consultant.

    The consultant reads no project source, so the runtime -- never the
    escalating model's free text alone -- selects its evidence: the reports
    the escalation named, then every report the escalated revision's tasks
    recorded as their results, each with the report's current SHA-256 and
    recorded artifact versions. An id that names no report of this task stays
    listed as unavailable, so the consultant records the gap as a limitation.
    The brief never truncates dependency reports, so the packet bounds itself
    explicitly: artifact lists first (read_report returns them), then the
    trailing entries, counted in a final omission entry.
    """
    selected = [('escalation_request', ref) for ref in proposal.get('report_ids') or []]
    for row in db.execute('SELECT node_id,result_json FROM runtime_nodes WHERE task_id=? AND revision=? '
                          'AND result_json IS NOT NULL ORDER BY rowid', (task_id, source_revision)):
        result = json.loads(row['result_json'])
        if isinstance(result, dict) and isinstance(result.get('report_ref'), str):
            selected.append((row['node_id'], result['report_ref']))
    packet, seen = [], set()
    for node, ref in selected:
        if ref in seen:
            continue
        seen.add(ref)
        row = db.execute("SELECT e.digest,COALESCE(p.artifacts,'[]') AS artifacts FROM reports r "
                         'JOIN editions e ON e.report_id=r.id LEFT JOIN report_provenance p ON p.edition_sequence=e.sequence '
                         'WHERE r.task_id=? AND r.id=? ORDER BY e.sequence DESC LIMIT 1', (task_id, ref)).fetchone()
        packet.append(dict(node=node, report_ref=ref, available=False) if row is None else
                      dict(node=node, report_ref=ref, sha256=row['digest'], artifacts=json.loads(row['artifacts'])))
    from ..delegation.provisioning import WORKER_BRIEF_MAX_CHARS
    # Room for the objective, the scope and the encoding around them.
    budget = WORKER_BRIEF_MAX_CHARS - len(encoded(dict(objective=objective, scope=['.']))) - 200
    if len(encoded(packet)) > budget:
        for entry in packet:
            if 'artifacts' in entry:
                entry['artifacts_omitted'] = len(entry.pop('artifacts'))
    omitted = 0
    while packet and len(encoded(packet + [dict(omitted_reports=omitted + 1)])) > budget:
        packet.pop()
        omitted += 1
    return packet + ([dict(omitted_reports=omitted)] if omitted else [])


def validate_plan(plan):
    if not isinstance(plan, dict) or set(plan) != {'tasks', 'verification'}:
        raise StoreError('invalid_plan_fields')
    nodes = plan['tasks']
    if not isinstance(nodes, list) or not 1 <= len(nodes) <= MAX_NODES:
        raise StoreError('invalid_plan_size')
    ids = set()
    for node in nodes:
        if (not isinstance(node, dict) or not {'id', 'role', 'objective', 'dependencies', 'scope'} <= set(node)
                or set(node) - {'id', 'role', 'objective', 'dependencies', 'scope', 'complexity', 'provider', 'model', 'model_reason', 'work_kind'}
                or node.get('complexity', 'normal') not in {'low', 'normal', 'high'}
                or node.get('provider', SUPPORTED_ENGINES[0]) not in SUPPORTED_ENGINES):
            raise StoreError('invalid_plan_node')
        from .contracts import NODE, validate as validate_contract
        if 'work_kind' in node:
            validate_contract(NODE['properties']['work_kind'], node['work_kind'], 'work_kind')
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
        if role == CONSULTANT_ROLE:
            # Consultation is escalation-only. A planned
            # consultant node had no runtime-built packet to work from.
            raise StoreError('consultant_requires_escalation', f"tasks[{key}].role", node['role'],
                             'a non-consultant specialist role',
                             'Plan no consultant task. A worker or the coordinator calls escalation_request; '
                             'the runtime then adds the senior consultant with its consultation packet.')
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
    validate_verification(plan['verification'])
    return plan


def validate_verification(checks, field='plan.verification'):
    """The one verification argv contract, for a plan and for a repair
    (verification_submit): literal exec-style argv, never shell syntax, and
    nothing Popen cannot launch (a NUL byte crashed every VERIFYING turn)."""
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
                'verification_shell_syntax', field, shell_token,
                'literal argv without shell syntax',
                'Use a documented project command or script. Verification executes argv directly without a shell.',
            )


def _relative(path, root):
    """A scope path shown relative to the task's project root when inside it."""
    if path == root:
        return '.'
    return path[len(root) + 1:] if path.startswith(root + '/') else path


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
            from .worker_waits import projection
            from .access import context as access_context
            waits = projection(db, task_id)
            for node in value_nodes:
                node['access'] = access_context(db, node)
                node['wait'] = next((w for w in reversed(waits)
                                     if w['assignment_ref'] == node['assignment_ref']
                                     and w['status'] in {'open', 'ready', 'resuming'}), None)
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
            from .check_closure import runtime_gap_closures, apply_runtime_closures
            closures = runtime_gap_closures(db, task_id, task['revision'])
            for node in value_nodes:
                result = node['result']
                if isinstance(result, dict) and result.get('checks'):
                    attempt_id = attempts_by_assignment.get(node['assignment_ref'])
                    result['checks'] = annotate_checks(
                        db, task_id, attempt_id,
                        apply_runtime_closures(result['checks'], node['assignment_ref'], closures)
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
        from ..routing.auto_route import hint_serves_tier, route_menu, tier_for, work_kind
        exact_hints = [n['model'] for n in plan['tasks'] if '/' in n.get('model', '')]
        if exact_hints:
            menu = route_menu(self.store.project_root)
            offered = {item['route'] for group in menu['tiers'].values() for item in group}
            if any(hint not in offered for hint in exact_hints):
                raise StoreError('unknown_model_hint', 'model', ', '.join(exact_hints),
                                 'an exact route from list_route_options',
                                 'Call list_route_options and copy an offered route.')
        from ..routing.auto_route import ambiguous_user_model_request, user_model_request

        def preview(n):
            model, role = n.get('model', 'auto'), resolve_role(n['role'])
            complexity = n.get('complexity', 'normal')
            item = dict(node=n['id'], model=model, tier_now=tier_for(role, complexity=complexity),
                        work_kind=work_kind(role, n.get('work_kind')))
            if '/' not in model:
                item['preview_status'] = 'auto' if model == 'auto' else 'pending_dispatch'
                return item
            # A model the owner placed in several tiers is listed under each.
            hinted_tiers = [tier for tier, group in menu['tiers'].items()
                            for entry in group if entry['route'] == model]
            hinted = next((entry for group in menu['tiers'].values() for entry in group
                           if entry['route'] == model), {})
            # Every exact user-requested route precedes automatic tier selection.
            request = user_model_request(self.store, channel.task_id, model)
            if request is not None:
                item.update(preview_status='pending_dispatch:user_request', user_request=request['source_ref'])
            elif not any(hint_serves_tier(model.partition('/')[2], tier, role, item['tier_now'], complexity,
                                          also_standard_for=(hinted.get('policy') or {}).get('also_standard_for'),
                                          work_kind_override=n.get('work_kind'))
                         for tier in hinted_tiers or [None]):
                item['preview_status'] = 'ignored:tier_mismatch'
            else:
                item['preview_status'] = 'pending_dispatch'
            if request is not None and request['matched'] == model.partition('/')[2]:
                item['user_request_diagnostic'] = (
                    'User text names the model without its engine; it uniquely matches this enabled configured route.')
            elif request is None:
                ambiguous = ambiguous_user_model_request(self.store, channel.task_id, model)
                if ambiguous is not None:
                    item.update(ambiguous_user_request=ambiguous['source_ref'], user_request_diagnostic=(
                        'User text names a model ambiguously without its engine; ask the user to name engine/model.'))
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
            from .gateway import READ_ONLY
            for node in plan['tasks']:
                if resolve_role(node['role']) not in READ_ONLY:
                    validate_grants(db, channel.task_id, node['scope'])
            if project is not None:
                read_only_output_paths(plan['tasks'], project[0])
            proposed_nodes=plan['tasks']
            request=db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED' ORDER BY id DESC LIMIT 1",(channel.task_id,)).fetchone()
            escalation=json.loads(request[0]) if request else {}
            if escalation.get('operation')=='escalation_request' and escalation.get('revision')==task['revision']:
                # The runtime routes a readonly consultant automatically
                # (routing.auto_route). The model never chooses a model.
                # It also builds the consultation packet;
                # the controller hands it over as the dependency reports.
                proposal=escalation['proposal']
                objective=consultation_objective(proposal)
                packet=consultation_packet(db,channel.task_id,task['revision'],proposal,objective)
                proposed_nodes=[dict(id=CONSULTATION_NODE,role='senior-consultant',
                    objective=objective,dependencies=[],scope=['.'])]+[
                    dict(node,dependencies=[CONSULTATION_NODE,*node['dependencies']]) for node in proposed_nodes]
                event(db,channel.task_id,'CONSULTATION_REQUIRED',dict(role=CONSULTANT_ROLE,source_revision=task['revision'],
                    revision=task['revision']+1,node=CONSULTATION_NODE,dependency_reports=packet))
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
                          hint='Plan accepted; the runtime is starting its workers now. '
                               'End this turn now -- do not call task_get_context to wait for dispatch, '
                               'the runtime resumes you when results arrive.')
            db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                       (channel.task_id, request_key, channel.channel_id, 'plan_submit', digest, encoded(result)))
            db.commit()
        return result

    def consultation_reports(self, task_id, revision):
        """The runtime-built packet of the escalation-created consultant node
        of ``revision``; [] when that revision has none."""
        with self.store.connection() as db:
            for (payload,) in db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                                         "AND type='CONSULTATION_REQUIRED' ORDER BY id DESC", (task_id,)):
                value = json.loads(payload)
                if value.get('revision') == revision:
                    return list(value.get('dependency_reports') or [])
        return []

    def move(self, task_id, target, *, blocker=None):
        # Completion checks the project; PLAN_READY -> EXECUTING records its
        # baseline. Both are observed before the write lock.
        needed = target == 'COMPLETED' or (target == 'EXECUTING' and self.get_state(task_id) == 'PLAN_READY')
        with observed_project(self.store, task_id, needed), self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            if target == 'COMPLETED':
                self._completion_invariants(db, task_id)
            transition(db, task_id, target, blocker=blocker)
            db.commit()

    def close_answer(self, task_id):
        """End ANSWER_READY and return the terminal state.

        COMPLETED only under the completion invariants. A released answer on
        planned work whose review or verification did not pass is still the
        user's honest final answer, but not a certified result: the task ends
        FAILED with the missing gate as its blocker, and no passing review or
        verification is ever inserted for it."""
        with observed_project(self.store, task_id), self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            try:
                self._completion_invariants(db, task_id)
            except StoreError as error:
                task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
                gap = error.args[0] if error.args else None
                if (gap not in WORK_CERTIFICATION_GAPS or task is None or task['state'] != 'ANSWER_READY'
                        or not answer_needs_certified_work(db, task)):
                    raise
                event(db, task_id, 'ANSWER_NOT_CERTIFIED', dict(revision=task['revision'], reason=gap))
                transition(db, task_id, 'FAILED', blocker=f'answer_not_certified:{gap}')
                db.commit()
                return 'FAILED'
            transition(db, task_id, 'COMPLETED')
            db.commit()
        return 'COMPLETED'

    def submit_answer(self, channel, text, request_key, *, not_verified=(), authorization=None, language='en'):
        """Record an informational proposal; only the runtime completes it.

        This cannot replace a plan, escape a review or accept a worker result.
        """
        proposal = dict(text=text)
        if not_verified:
            proposal['not_verified'] = list(not_verified)
        digest=hashlib.sha256(encoded(proposal).encode()).hexdigest()
        with observed_project(self.store, channel.task_id), self.store.connection() as db:
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
            gaps = {}
            if answer_needs_certified_work(db, task):
                _, gaps = synthesis_checks(db, channel.task_id, task['revision'])
            stored = disclosed_synthesis(text, not_verified, gaps, language)
            if task['state']=='ANSWER_READY':
                if task['synthesis']!=stored: raise StoreError('answer_conflict')
            else:
                db.execute('UPDATE runtime_tasks SET synthesis=? WHERE task_id=?',(stored,channel.task_id))
                event(db,channel.task_id,'ANSWER_SUBMITTED',
                      dict(text=stored,not_verified=list(not_verified),request_key=request_key),channel.channel_id)
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
                from .worker_waits import cancel_tx
                cancel_tx(db, task_id, 'task_cancelled')
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
    def _ready_nodes(db, task_id, waits=None, details=None):
        """Select reservable nodes without writing; longest remaining chain first.

        ``details`` (optional) receives, per waiting node, what holds it:
        for ``scope_overlap`` the holder and the overlapping paths, for
        ``capacity_limit`` the session's worker limit. It only describes the
        decision for display; it never changes it."""
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if task is None or task['state'] not in {'EXECUTING', 'REVIEWING'}:
            return task, []
        nodes = [dict(n) for n in db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid',
                                            (task_id, task['revision']))]
        completed = {n['node_id'] for n in nodes if n['status'] == 'completed'}
        stage = 'review' if task['state'] == 'REVIEWING' else 'execution'
        ready = [n for n in nodes if n['status'] == 'pending' and n['stage'] == stage
                 and set(json.loads(n['dependencies_json'])) <= completed]
        if waits is not None:
            waits.update({n['node_id']: 'dependency' for n in nodes
                          if n['status'] == 'pending' and n['stage'] == stage
                          and n not in ready})
        if ready:
            # A node retried after an infrastructure failure waits out its
            # delay (orchestration.recovery.retry_lost) before it starts again.
            backoff = TaskMachine._retry_backoff(db, task_id, task['revision'])
            for candidate in [n for n in ready if n['node_id'] in backoff]:
                ready.remove(candidate)
                if waits is not None:
                    waits[candidate['node_id']] = 'retry_backoff'
                if details is not None:
                    details[candidate['node_id']] = dict(backoff[candidate['node_id']], role_waiting=candidate['role'])
        if not ready:
            return task, []
        from .access import holdings, effective_scope, absolute_scope, pending
        active_nodes = holdings(db)
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
            (absolute_scope(db, n['task_id'], effective_scope(db, n)),
             dict(kind='worker', task_id=n['task_id'], node=n['node_id'], role=n['role'],
                  objective=n['objective'], running=n['status'] == 'running',
                  assignment_ref=n['assignment_ref']))
            for n in active_nodes
            if n['stage'] != 'review' and n['role'] not in READ_ONLY
        ]
        occupied.extend((absolute_scope(db, w['task_id'], w['paths']),
                         dict(kind='access_request', task_id=w['task_id'],
                              assignment_ref=w.get('assignment_ref')))
                        for w in pending(db))
        selected = []
        for candidate in ready:
            scope = absolute_scope(db, task_id, effective_scope(db, candidate))
            read_only = candidate['stage'] == 'review' or candidate['role'] in READ_ONLY
            holder = None if read_only else next(
                (dict(who, held=a, requested=b) for grant, who in occupied
                 for a in grant for b in scope if overlaps(a, b)), None)
            if holder is not None:
                if waits is not None:
                    waits[candidate['node_id']] = 'scope_overlap'
                if details is not None:
                    root = str(Path(task_workspace.task_root(db, task_id)).resolve())
                    holder.update({key: _relative(holder[key], root) for key in ('held', 'requested')})
                    details[candidate['node_id']] = dict(holder, role_waiting=candidate['role'])
                continue
            selected.append(candidate)
            if not read_only:
                occupied.append((scope, dict(kind='worker', task_id=task_id, node=candidate['node_id'],
                                             role=candidate['role'], objective=candidate['objective'],
                                             running=True, assignment_ref=None)))
        # At most MAX_WORKERS_PER_SESSION workers of this chat session run
        # at once, whatever their engine or role; the rest stay pending.
        from ..delegation.capacity import MAX_WORKERS_PER_SESSION
        free = MAX_WORKERS_PER_SESSION - TaskMachine._session_worker_slots(db, task_id, task['revision'])
        if waits is not None:
            waits.update({n['node_id']: (None if i < max(0, free) else 'capacity_limit')
                          for i, n in enumerate(selected)})
        if details is not None:
            details.update({n['node_id']: dict(limit=MAX_WORKERS_PER_SESSION, role_waiting=n['role'])
                            for n in selected[max(0, free):]})
        return task, selected[:max(0, free)]

    @staticmethod
    def _retry_backoff(db, task_id, revision):
        """Nodes of ``revision`` still waiting out a retry delay: node id ->
        what the wait card shows (retry number, cause, until)."""
        current = time.time()
        with _RETRY_HOLDS_LOCK:
            for key in [k for k, hold in _RETRY_HOLDS.items() if hold['until'] <= current]:
                del _RETRY_HOLDS[key]
            return {node: dict(hold['info']) for (task, rev, node), hold in _RETRY_HOLDS.items()
                    if task == task_id and rev == revision}

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
        from .measurement import wait_reason, wait_signature
        with self.store.connection() as db:
            waits, details = {}, {}
            task, ready = self._ready_nodes(db, task_id, waits, details)
            previous = {}
            for r in db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                                "AND type='SCHEDULER_WAIT_REASON' ORDER BY id", (task_id,)):
                payload = json.loads(r[0])
                previous[payload['identity']] = wait_signature(payload.get('reason'), payload.get('blocked_by'))
            if not ready and not any(previous.get(f"node:{task['revision']}:{node}")
                                     != wait_signature(reason, details.get(node))
                                     for node, reason in waits.items()):
                return []
            db.execute('BEGIN IMMEDIATE')
            waits, details = {}, {}
            task, ready = self._ready_nodes(db, task_id, waits, details)
            for node, reason in waits.items():
                blocked_by = details.get(node) if reason else None
                wait_reason(db, task_id, f"node:{task['revision']}:{node}", reason,
                            node=node, revision=task['revision'],
                            **({'blocked_by': blocked_by} if blocked_by else {}))
            if not ready:
                db.commit()
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
        with observed_project(self.store, task_id), self.store.connection() as db:
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
        bound_at = db.execute("SELECT MAX(created_at) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'",
                              (task_id,)).fetchone()[0]
        others = foreign_project_writers(db, task_id, bound_at) if bound_at else []
        source = (f' The change came from another Cortex task in this project ({", ".join(others)}).'
                  if others else '')
        if rebinds >= MAX_REVIEW_REBINDS:
            raise StoreError('review_project_revision_unstable')
        event(db, task_id, 'REVIEW_STARTED', dict(
            assignment_ref=assignment_ref, revision_digest=observed['dirty_digest'],
            rebound_from=binding['revision_digest'], rebind=rebinds + 1, refused_request_key=request_key))
        db.commit()
        from .reviewer_handoff import review_runtime_facts
        try:
            facts = review_runtime_facts(self.store, task_id, assignment_ref,
                                         task_workspace.task_root(db, task_id))
            refreshed = ' Refreshed runtime facts (metadata only): ' + encoded(facts)
        except (StoreError, OSError):
            refreshed = ' Runtime facts refresh unavailable; inspect the current project independently.'
        raise StoreError('review_project_revision_changed', 'project revision', binding['revision_digest'],
                         'the project revision this review is bound to',
                         'The project changed while you reviewed it; the review is now bound to the current revision. '
                         'Re-check what changed (project_git diff), then submit your verdict again with a new request_key.'
                         + source + refreshed)

    def record_result(self, task_id, assignment_ref, result, request_key=None):
        # Only a review result binds to an observed project revision.
        with self.store.connection() as db:
            node = db.execute('SELECT stage FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                              (task_id, assignment_ref)).fetchone()
        review = node is not None and node['stage'] == 'review'
        with observed_project(self.store, task_id, review), self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            self.record_result_tx(db, task_id, assignment_ref, result, request_key)
            db.commit()

    def record_result_tx(self, db, task_id, assignment_ref, result, request_key=None):
        """Record a node result inside the caller's transaction.

        Every refusal is raised before anything is written (a review re-bind
        commits only its own REVIEW_STARTED event first), so a caller may
        catch a StoreError and still commit its other writes."""
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        node = db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                          (task_id, assignment_ref)).fetchone()
        if not task or not node or node['revision'] != task['revision'] or task['state'] in TERMINAL:
            raise StoreError('stale_assignment_result')
        if node['result_json'] is not None:
            retained = json.loads(node['result_json'])
            retained.pop('report_digest', None)
            retained.pop('project_digest', None)
            for check in retained.get('checks', []):
                if check.get('outcome') in {'not_run', 'unverified'}:
                    check.pop('closed_by', None)
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
        failed = result.get('assignment_outcome') == 'failed' or open_failure(db, task_id, provision['attempt_id'], checks)
        status = 'failed' if failed else 'completed'
        if node['stage'] == 'review' and result.get('verdict') not in {'APPROVE', 'REQUEST_CHANGES', 'BLOCK'}:
            raise StoreError('review_verdict_required')
        if node['stage'] == 'review':
            result['project_digest'] = self._review_binding_tx(db, task_id, assignment_ref, request_key)
        db.execute('UPDATE runtime_nodes SET status=?,result_json=? WHERE task_id=? AND revision=? AND node_id=?',
                   (status, encoded(result), task_id, task['revision'], node['node_id']))
        # Accepted results end MCP authority, not engine ownership. Keep
        # the capacity reservation until an observed process exit; revoke
        # every channel of this exact attempt in the same transaction.
        db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? "
                   "AND attempt_id=? AND role='worker' AND revoked_at IS NULL",
                   (now(), task_id, provision['attempt_id']))
        event(db, task_id, 'WORKER_COMPLETED' if not failed else 'WORKER_FAILED',
              dict(node=node['node_id'], assignment_ref=assignment_ref,
                   attempt_id=provision['attempt_id'], result=result))

    def start_review(self, task_id, assignment_ref, *, observation=None):
        """Bind the review to the project observed now. ``observation``
        (a dict) receives the capture so the reviewer's facts reuse it."""
        from ..delegation.revision import capture_revision
        # The tree walk runs before the write lock: holding it here stalled
        # every other writer, and the lock never froze the files anyway.
        with self.store.connection() as db:
            root = task_workspace.task_root(db, task_id)
        observed = capture_revision(root)
        if observed['kind'] != 'observed':
            raise StoreError('review_revision_unavailable')
        with self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            event(db, task_id, 'REVIEW_STARTED', dict(assignment_ref=assignment_ref,
                  revision_digest=observed['dirty_digest']))
            db.commit()
        if observation is not None:
            observation.update(observed)
        return observed['dirty_digest']

    def recheck_after_foreign_change(self, task_id, reason):
        """The task ids of other Cortex tasks whose writers changed this
        task's project, after starting a new review cycle on the current
        revision; ``None`` (nothing changed) when the change cannot be
        attributed to another task or the review cycle limit is reached."""
        with observed_project(self.store, task_id), self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            others = self._recheck_after_foreign_change_tx(db, task_id, reason)
            db.commit()
        return others

    def _recheck_after_foreign_change_tx(self, db, task_id, reason):
        """Two tasks working in one project invalidated each other's
        verified completion, and the user was asked what to do while the
        runtime's own store named the writer (observed 2026-10-22). Here the
        change is attributed and the work is reviewed and verified again on
        the project as it is now."""
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if task is None or task['state'] not in {'VERIFYING', 'RECONCILING'}:
            return None
        if task['review_cycle'] >= MAX_REVIEW_CYCLES:
            return None
        bound = db.execute("SELECT MAX(created_at) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'",
                           (task_id,)).fetchone()[0]
        if bound is None:
            return None
        others = foreign_project_writers(db, task_id, bound)
        if not others:
            return None
        nodes = db.execute("SELECT node_id FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='execution'",
                           (task_id, task['revision'])).fetchall()
        cycle = task['review_cycle'] + 1
        db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                   (task_id, task['revision'], f'_review_{cycle}', 'worker-code-reviewer',
                    'Independently review the original objective, implementation and retained reports on the '
                    'current project, which another Cortex task also changed. Submit an explicit review verdict.',
                    encoded([n['node_id'] for n in nodes]), encoded(['**']), 'review', 'pending', None, None))
        # The synthesis described the earlier verified project; it is written
        # again after this review and verification.
        db.execute('UPDATE runtime_tasks SET review_cycle=?,synthesis=NULL WHERE task_id=?', (cycle, task_id))
        event(db, task_id, 'PROJECT_CHANGED_BY_OTHER_TASK',
              dict(tasks=others, revision=task['revision'], reason=reason, review_cycle=cycle))
        transition(db, task_id, 'REVIEWING')
        return others

    def add_review(self, task_id):
        with observed_project(self.store, task_id), self.store.connection() as db:
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
        # A request for changes without structured findings names no task to
        # fix: the coordinator, who reads the review report, plans the fix.
        # Every original task used to get a blind fix, and a devops task got
        # an application-code fix it could not make (2026-10-22).
        if not review['result'].get('findings'):
            with self.store.connection() as db:
                task = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
                implementers = db.execute("SELECT COUNT(*) FROM runtime_nodes WHERE task_id=? AND revision=? "
                                          "AND stage='execution' AND node_id NOT LIKE '\\_%' ESCAPE '\\'",
                                          (task_id, task['revision'] if task else -1)).fetchone()[0]
            if implementers > 1 and self.return_to_coordinator(task_id, 'review_changes_without_findings'):
                with self.store.connection() as db:
                    db.execute('BEGIN IMMEDIATE')
                    event(db, task_id, 'REVIEW_CHANGES_REQUESTED', dict(
                        review=review['node_id'], fixes=[], targeted=False, fallback='no_findings', returned=True))
                    db.commit()
                return
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
                from .access import effective_scope
                targets,fallback=targeted_fixes(tasks,{n['node_id']: effective_scope(db, n) for n in implementations},
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
                    scope = effective_scope(db, implementation)
                    role = fix_role(implementation['role'], scope)
                    db.execute('INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                               (task_id,task['revision'],keys[node_id],role,objective,
                                encoded(dependencies),encoded(scope),'execution','pending',None,None))
                    fixes.append(keys[node_id])
                event(db,task_id,'REVIEW_CHANGES_REQUESTED',dict(review=review['node_id'],fixes=fixes,
                                                                  targeted=targets is not None,fallback=fallback))
                transition(db, task_id, 'EXECUTING')
            db.commit()

    def submit_synthesis(self, channel, text, not_verified=(), *, authorization=None, language='en'):
        with observed_project(self.store, channel.task_id), self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            if authorization: authorization(db)
            task = db.execute('SELECT state,synthesis,revision FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
            if not task or task['state'] != 'RECONCILING':
                state = task['state'] if task else 'missing'
                raise StoreError('synthesis_not_expected', 'state', state, 'RECONCILING', (
                    'The task is planning: synthesis_submit is never accepted there. A user message after the '
                    'verification, a failed verification or a returned worker sent it back. Submit plan_submit '
                    'for requested changes, verification_submit to repair a failed verification, answer_submit '
                    'when the planning prompt offers it, or user_question_request for a decision.'
                    if state == 'PLANNING' else
                    'The runtime has not reached reconciliation (or the task already finished). End this turn '
                    'and let the runtime continue; read task_get_context for the current state.'))

            _, gaps = synthesis_checks(db, channel.task_id, task['revision'])
            stored = disclosed_synthesis(text, not_verified, gaps, language)
            if task['synthesis'] is not None and task['synthesis'] != stored:
                raise StoreError('synthesis_conflict')
            if task['synthesis'] == stored:
                return dict(recorded=True,state='RECONCILING')
            db.execute('UPDATE runtime_tasks SET synthesis=? WHERE task_id=?', (stored, channel.task_id))
            event(db, channel.task_id, 'SYNTHESIS_SUBMITTED', dict(text=stored, not_verified=list(not_verified)), channel.channel_id)
            db.commit()
        return dict(recorded=True)

    def record_verification(self, task_id, checks, revision_digest, *, speculative_binding=None):
        with observed_project(self.store, task_id), self.store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
            if speculative_binding is not None:
                review = db.execute(
                    "SELECT assignment_ref,result_json FROM runtime_nodes WHERE task_id=? AND revision=? "
                    "AND stage='review' ORDER BY rowid DESC LIMIT 1",
                    (task_id, task['revision'] if task else -1)).fetchone()
                result = json.loads(review['result_json']) if review and review['result_json'] else {}
                if (not task or task['state'] != 'VERIFYING'
                        or task['revision'] != speculative_binding['revision']
                        or not review or review['assignment_ref'] != speculative_binding['assignment_ref']
                        or result.get('verdict') != 'APPROVE'
                        or result.get('project_digest') != speculative_binding['digest']
                        or revision_digest != speculative_binding['digest']):
                    return None
            if not task or task['state'] != 'VERIFYING':
                raise StoreError('verification_not_expected')
            if reviewed_digest(db, task_id, task['revision']) != revision_digest:
                if self._recheck_after_foreign_change_tx(db, task_id, 'verification_review_revision_changed') is None:
                    transition(db, task_id, 'BLOCKED', blocker='verification_review_revision_changed')
                db.commit()
                return False
            commands = json.loads(task['plan_json'])['verification']
            passed = (len(checks) == len(commands) and len(revision_digest) == 64
                      and all(c.get('argv') == argv and c.get('exit_code') == 0 and c.get('complete') is True
                              and c.get('revision_digest') == revision_digest for c, argv in zip(checks, commands)))
            if speculative_binding is not None:
                event(db, task_id, 'SPECULATIVE_VERIFICATION_CREDITED', dict(binding=speculative_binding))
            event(db, task_id, 'VERIFICATION_PASSED' if passed else 'VERIFICATION_FAILED',
                  dict(revision=task['revision'], revision_digest=revision_digest, checks=checks))
            failures = db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND type='VERIFICATION_FAILED'",
                                  (task_id,)).fetchone()[0]
            if passed:
                from .check_closure import retain_runtime_closures
                retain_runtime_closures(db, task_id, task['revision'])
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
            # Its open questions go with it, or the next user message is
            # taken as a late answer to a cancelled worker and swallowed.
            from .worker_waits import cancel_tx
            cancel_tx(db, task_id, 'returned_to_coordinator')
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
            # The same argv contract as a plan's verification.
            validate_verification(verification, 'verification')
            task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()
            last = db.execute("SELECT type FROM runtime_events WHERE task_id=? AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED','PLAN_ACCEPTED') ORDER BY id DESC LIMIT 1",
                              (channel.task_id,)).fetchone()
            nodes = db.execute('SELECT status FROM runtime_nodes WHERE task_id=? AND revision=?',
                               (channel.task_id, task['revision'] if task else -1)).fetchall()
            if (not task or task['state'] != 'PLANNING' or not last or last[0] != 'VERIFICATION_FAILED'
                    or not nodes or any(n['status'] != 'completed' for n in nodes)):
                raise StoreError('verification_repair_not_expected')
            plan = json.loads(task['plan_json'])
            failed = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='VERIFICATION_FAILED' "
                                "ORDER BY id DESC LIMIT 1", (channel.task_id,)).fetchone()
            failed_argv = [check.get('argv') for check in json.loads(failed[0]).get('checks', [])
                           if check.get('exit_code') != 0] if failed else []
            # A check that failed and is now gone: the runtime asks the user
            # before completing (TaskMachine._work_invariants), so a
            # behavioral check is never silently replaced by lint and units.
            # Correcting a command one for one (the repair the runtime asks
            # for) is not a removal; a shorter list that lost a failed command is.
            previous = plan.get('verification') or []
            dropped = ([argv for argv in failed_argv if argv and argv not in verification]
                       if len(verification) < len(previous) else [])
            plan['verification'] = verification
            db.execute('UPDATE runtime_tasks SET plan_json=? WHERE task_id=?', (encoded(plan), channel.task_id))
            event(db, channel.task_id, 'VERIFICATION_REVISED', dict(verification=verification, dropped_failed=dropped),
                  channel.channel_id)
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
            if answer_needs_certified_work(db, task):
                # A released answer is the user's final word on planned work,
                # never a substitute for its review and verification.
                self._work_invariants(db, task_id, task)
            return
        self._work_invariants(db, task_id, task)
        if task['blocker'] or not task['synthesis']:
            raise StoreError('completion_synthesis_or_blocker')

    def _work_invariants(self, db, task_id, task):
        """The current revision's work is complete, reviewed (or waived) and
        verified on the observed project. Raises WORK_CERTIFICATION_GAPS."""
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
        if dropped_failed_checks_unacknowledged(db, task_id):
            raise StoreError('completion_dropped_failed_check')
        from ..delegation.revision import capture_revision
        observed = capture_revision(task_workspace.task_root(db, task_id))
        if observed['kind'] != 'observed' or observed['dirty_digest'] != json.loads(evidence['payload_json'])['revision_digest']:
            raise StoreError('completion_project_revision_changed')
