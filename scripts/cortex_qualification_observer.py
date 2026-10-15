#!/usr/bin/env python3
"""Bounded no-paid observer primitives for one guarded dev CLI qualification case.

Nothing here launches a host, engine, daemon or model. Launch, steering and
cleanup are injected callables; the defaults only observe metadata through the
public bound task inspector (``cortex_runtime.storage.inspection``), which stays
fail-closed: it never initializes, checkpoints, migrates or bypasses a lock.
A classified observation is evidence of what the inspector returned, never an
acceptance verdict: run ``scripts/cortex_qualification_assert.py`` for that.
"""
import errno
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from cortex_runtime.contracts import StoreError  # noqa: E402

CASE_LIMIT_SECONDS = 600.0
CLEANUP_RESERVE_SECONDS = 90.0
MAX_WAIT_SECONDS = 60.0
MAX_SNAPSHOT_RESTARTS = 3
DECISION_FREE_STEER = 'Thanks, carry on; nothing to change.'
_REFUSAL_PREFIXES = ('inspection_', 'unsafe_storage', 'legacy_storage', 'unsupported_storage')
_TERMINAL_WORKER_EVENTS = ('WORKER_COMPLETED', 'WORKER_FAILED')


class Budget:
    """One case's monotonic budget; observation ends before the cleanup reserve."""

    def __init__(self, *, limit=CASE_LIMIT_SECONDS, cleanup_reserve=CLEANUP_RESERVE_SECONDS, clock=time.monotonic):
        if not (CLEANUP_RESERVE_SECONDS <= cleanup_reserve < limit <= CASE_LIMIT_SECONDS):
            raise ValueError('invalid_case_budget')
        self.clock = clock
        self.start = clock()
        self.case_deadline = self.start + limit
        self.observe_deadline = self.case_deadline - cleanup_reserve

    def wait_deadline(self, seconds=MAX_WAIT_SECONDS):
        """Deadline of one wait: at most 60 s and never past the observation deadline."""
        return min(self.clock() + min(seconds, MAX_WAIT_SECONDS), self.observe_deadline)

    def elapsed(self):
        return self.clock() - self.start


def classify(error):
    """``(code, transient)`` for one inspector exception; its text is never kept.

    The inspector refuses a busy access lock by letting its nonblocking
    ``flock`` raise ``BlockingIOError`` (the CLIs print ``inspection_busy``).
    That advertised refusal is the only transient condition.
    """
    if isinstance(error, BlockingIOError) and error.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
        return 'inspection_busy', True
    if isinstance(error, StoreError) and error.args and isinstance(error.args[0], str):
        code = error.args[0]
        return (code if code.startswith(_REFUSAL_PREFIXES) else 'inspection_refused'), False
    return 'unexpected_error:' + type(error).__name__, False


def inspector_pages(binding, *, limit=200):
    """``read_page(cursor)`` over the public inspector for one observed binding."""
    from cortex_runtime.storage.inspection import read_binding_page

    def read_page(cursor):
        return read_binding_page(binding['project_root'], binding['session_id'], binding['task_id'],
                                 binding['owner_epoch'], cursor=cursor, limit=limit)
    return read_page


def _page_problem(page, binding, digest, offset):
    if page.get('binding') != binding:
        return 'inspection_binding_changed'
    if digest is not None and page.get('snapshot_sha256') != digest:
        return 'inspection_mixed_snapshot'
    if page.get('offset') != offset:
        return 'inspection_page_offset'
    cursor = page.get('next_cursor')
    if cursor is not None and (page.get('complete') or not cursor.startswith(page.get('snapshot_sha256', '') + ':')):
        return 'inspection_mixed_snapshot'
    return None


def collect_snapshot(read_page, binding, *, deadline, clock=time.monotonic, sleep=time.sleep,
                     restarts=MAX_SNAPSHOT_RESTARTS):
    """Every page of ONE inspection snapshot, or a classified failure, by ``deadline``.

    Only the advertised busy access-lock refusal is retried, for the same page and
    only before ``deadline``. A stale cursor (``inspection_snapshot_changed``)
    restarts from the first page at most ``restarts`` times. Every other refusal
    or exception ends this observation at once. Pages must share one binding,
    one snapshot digest and contiguous offsets.
    """
    counts = dict(reads=0, busy=0, restarts=0)
    rows, digest, cursor, delay = [], None, None, 0.05

    def ended(status, blocker):
        return dict(status=status, complete=False, blocker=blocker, counts=counts)

    while True:
        if clock() >= deadline:
            return ended('busy_expired' if counts['busy'] else 'expired', 'observation_deadline')
        counts['reads'] += 1
        try:
            page = read_page(cursor)
        except Exception as error:  # noqa: BLE001 - classified, never echoed
            code, transient = classify(error)
            if code == 'inspection_snapshot_changed' and cursor is not None and counts['restarts'] < restarts:
                counts['restarts'] += 1
                rows, digest, cursor = [], None, None
                continue
            if not transient:
                return ended('error' if code.startswith('unexpected_error:') else 'refused', code)
            counts['busy'] += 1
            if clock() + delay >= deadline:
                return ended('busy_expired', code)
            sleep(delay)
            delay = min(delay * 2, 1.0)
            continue
        problem = _page_problem(page, binding, digest, len(rows))
        if problem:
            return ended('refused', problem)
        digest = page['snapshot_sha256']
        rows.extend(page['rows'])
        if page.get('next_cursor') is None:
            if not page.get('complete') or len(rows) != page.get('total'):
                return ended('refused', 'inspection_incomplete_snapshot')
            return dict(status='complete', complete=True, binding=binding, snapshot_sha256=digest,
                        total=len(rows), rows=rows, counts=counts)
        cursor = page['next_cursor']


def observe(binding, *, deadline, read_page=None, clock=time.monotonic, sleep=time.sleep):
    return collect_snapshot(read_page or inspector_pages(binding), binding, deadline=deadline,
                            clock=clock, sleep=sleep)


def poll_until(observe_once, predicate, *, deadline, interval=2.0, clock=time.monotonic, sleep=time.sleep):
    """Repeat bounded observations until ``predicate`` holds or ``deadline`` passes."""
    while True:
        observation = observe_once(deadline)
        if observation['status'] == 'complete' and predicate(observation):
            return dict(status='met', observation=observation)
        if observation['status'] in ('busy_expired', 'expired') or clock() + interval >= deadline:
            return dict(status='timeout', blocker=observation.get('blocker', 'condition_not_observed'),
                        observation=observation)
        if observation['status'] != 'complete':
            return dict(status='failed', blocker=observation['blocker'], observation=observation)
        sleep(interval)


def _section(snapshot, name):
    return [row for row in snapshot['rows'] if row['section'] == name]


def _candidate(snapshot, node, epoch, terminal):
    """Why one running node has no single live attempt, or its exact identity."""
    ref = node['assignment_ref']
    provisioned = [a for a in _section(snapshot, 'attempts') if a['idempotency_key'] == ref]
    if len(provisioned) != 1:
        return 'provisioning_not_unique', None
    attempt = provisioned[0]
    if (attempt['status'], attempt['step']) != ('ready', 'ready') or attempt['owner_epoch'] != epoch:
        return 'provisioning_not_ready', None
    attempt_id, session = attempt['attempt_id'], attempt['engine_session_ref']
    if not attempt_id or not session:
        return 'provisioning_identity_missing', None
    if attempt_id in terminal:
        return 'attempt_terminal', None
    confirmed = [e for e in _section(snapshot, 'task_events')
                 if e.get('kind') == 'worker_configuration_confirmed' and e.get('attempt_id') == attempt_id]
    if not confirmed or any(e.get('engine_session_ref') != session or e.get('assignment_ref') != ref
                            for e in confirmed):
        return 'native_confirmation_mismatch', None
    reserved = [c for c in _section(snapshot, 'capacity') if c['attempt_id'] == attempt_id
                and c['kind'] == 'worker' and c['state'] == 'reserved']
    if len(reserved) != 1:
        return 'capacity_not_reserved', None
    return None, dict(node_id=node['node_id'], revision=node['revision'], assignment_ref=ref,
                      attempt_id=attempt_id, engine_session_ref=session, role_id=attempt['role_id'])


def select_running_attempt(snapshot, *, stage='execution'):
    """Exactly one running attempt of ``stage``, joined on exact identities.

    Join: current runtime revision -> running node -> the provisioning row whose
    ``idempotency_key`` IS that node's ``assignment_ref`` (the runtime derives
    both from task, coordinator channel and request key; the provisioning
    ``assignment_id`` ``a_...`` is a different identity and is never compared)
    -> its ready attempt/session of the current owner epoch -> the native
    ``worker_configuration_confirmed`` of that same attempt, session and
    assignment -> its reserved worker capacity, with no terminal worker event.
    No title, timestamp or missing value is read as liveness.
    """
    if snapshot.get('status') != 'complete':
        return dict(status='refused', reason='snapshot_incomplete')
    runtime = _section(snapshot, 'runtime')
    expected = dict(execution='EXECUTING', review='REVIEWING')[stage]
    if len(runtime) != 1 or runtime[0]['state'] != expected:
        return dict(status='refused', reason='not_' + expected.lower(),
                    observed=runtime[0]['state'] if len(runtime) == 1 else None)
    revision = runtime[0]['revision']
    terminal = {e.get('attempt_id') for e in _section(snapshot, 'runtime_events')
                if e['type'] in _TERMINAL_WORKER_EVENTS}
    epoch = snapshot['binding']['owner_epoch']
    reasons, found = [], []
    for node in _section(snapshot, 'nodes'):
        if node['revision'] != revision or node['status'] != 'running' or node['stage'] != stage:
            continue
        reason, identity = _candidate(snapshot, node, epoch, terminal)
        (found.append(identity) if identity else reasons.append(reason))
    if len(found) != 1:
        return dict(status='refused', reason='ambiguous_running_attempts' if found else 'no_running_attempt',
                    candidates=len(found), node_reasons=sorted(reasons))
    return dict(status='selected', snapshot_sha256=snapshot['snapshot_sha256'], **found[0])


class SteerOnce:
    """At most ONE decision-free steer of a selected attempt through ``send``.

    ``send(text)`` is the injected supported control seam (for a live case the
    guarded wrapper's ``send <agent-id> --prompt ... --no-wait``). The send is
    recorded before it is called, so a raising or uncertain send is never
    replayed.
    """

    def __init__(self, send):
        self._send = send
        self.sent = None

    def __call__(self, selection, text=DECISION_FREE_STEER):
        if self.sent is not None:
            raise RuntimeError('steer_already_sent')
        if selection.get('status') != 'selected':
            raise ValueError('steer_requires_selected_attempt')
        self.sent = dict(selection=selection, receipt=None)
        self.sent['receipt'] = self._send(text)
        return self.sent['receipt']


def check_continuation(selection, snapshot):
    """Whether the SAME attempt and session continued after the one steer."""
    if snapshot.get('status') != 'complete':
        return dict(status='failed', reason='snapshot_incomplete')
    ref, attempt_id, session = selection['assignment_ref'], selection['attempt_id'], selection['engine_session_ref']
    provisioned = [a for a in _section(snapshot, 'attempts') if a['idempotency_key'] == ref]
    if [(a['attempt_id'], a['engine_session_ref']) for a in provisioned] != [(attempt_id, session)]:
        return dict(status='failed', reason='attempt_replaced')
    if any(e.get('engine_session_ref') != session for e in _section(snapshot, 'task_events')
           if e.get('kind') == 'worker_configuration_confirmed' and e.get('attempt_id') == attempt_id):
        return dict(status='failed', reason='session_changed')
    outcomes = {e['type'] for e in _section(snapshot, 'runtime_events')
                if e['type'] in _TERMINAL_WORKER_EVENTS and e.get('attempt_id') == attempt_id}
    if 'WORKER_FAILED' in outcomes:
        return dict(status='failed', reason='attempt_failed')
    if 'WORKER_COMPLETED' in outcomes:
        return dict(status='continued', reason='completed_same_attempt')
    again = select_running_attempt(snapshot)
    if again.get('status') == 'selected' and all(again[k] == selection[k] for k in
                                                  ('assignment_ref', 'attempt_id', 'engine_session_ref')):
        return dict(status='continued', reason='running_same_attempt')
    return dict(status='failed', reason='continuation_not_observed')


def wait_absent(identities, alive, *, grace, clock=time.monotonic, sleep=time.sleep, poll=0.1):
    """Truthful cleanup of owned helpers: absent at once, after grace, or still present."""
    start = clock()
    remaining = [i for i in identities if alive(i)]
    if not remaining:
        return dict(absent=True, immediate=True, elapsed=0.0, remaining=[])
    deadline = start + min(grace, MAX_WAIT_SECONDS)
    while remaining and clock() < deadline:
        sleep(min(poll, max(0.0, deadline - clock())))
        remaining = [i for i in remaining if alive(i)]
    return dict(absent=not remaining, immediate=False, elapsed=clock() - start, remaining=remaining)


def run_case(budget, exercise, cleanup):
    """``exercise(budget)``, then ALWAYS ``cleanup(budget)``, judged on the budget.

    ``exercise`` returns ``dict(status='passed', ...)`` or any other status;
    ``cleanup`` returns ``dict(ok=bool, ...)``. An exception in either is kept
    only as its class name. Cleanup also runs on KeyboardInterrupt/SystemExit,
    which then propagate.
    """
    outcome = cleaned = None
    try:
        try:
            outcome = exercise(budget)
        except Exception as error:  # noqa: BLE001 - classified, never echoed
            outcome = dict(status='error', blocker='unexpected_error:' + type(error).__name__)
    finally:
        cleanup_started = budget.clock()
        try:
            cleaned = cleanup(budget)
        except Exception as error:  # noqa: BLE001 - classified, never echoed
            cleaned = dict(ok=False, blocker='cleanup_error:' + type(error).__name__)
    finished = budget.clock()
    blockers = []
    if not isinstance(outcome, dict) or outcome.get('status') != 'passed':
        blockers.append('exercise_' + str(outcome.get('status') if isinstance(outcome, dict) else 'invalid'))
    if cleanup_started > budget.observe_deadline:
        blockers.append('observation_overran_cleanup_reserve')
    if not isinstance(cleaned, dict) or cleaned.get('ok') is not True:
        blockers.append('cleanup_failed')
    if finished > budget.case_deadline:
        blockers.append('case_deadline_exceeded')
    return dict(passed=not blockers, blockers=blockers, outcome=outcome, cleanup=cleaned,
                elapsed=finished - budget.start, cleanup_started=cleanup_started - budget.start)
