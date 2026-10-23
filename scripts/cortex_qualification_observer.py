#!/usr/bin/env python3
"""Bounded no-paid observer primitives for one guarded dev CLI qualification case.

Nothing here launches a host, engine, daemon or model. Launch, steering and
cleanup are injected callables; the defaults only observe metadata through the
public bound task inspector (``cortex_runtime.storage.inspection``), which stays
fail-closed: it never initializes, checkpoints, migrates or bypasses a lock.
A classified observation is evidence of what the inspector returned, never an
acceptance verdict. ``scripts/cortex_qualification_assert.py`` checks outcome
expectations over metadata; it does not decide acceptance either.
"""
from copy import deepcopy
import errno
import hashlib
import json
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
    event_ids = [e.get('id') for e in _section(snapshot, 'runtime_events')]
    if any(type(i) is not int or i < 1 for i in event_ids) or len(set(event_ids)) != len(event_ids):
        return dict(status='refused', reason='ambiguous_runtime_events')
    return dict(status='selected', snapshot_sha256=snapshot['snapshot_sha256'],
                binding=dict(snapshot['binding']), event_high_watermark=max(event_ids, default=0), **found[0])


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

    def __call__(self, selection, text=DECISION_FREE_STEER, *, origin=None, event_id=None):
        if self.sent is not None:
            raise RuntimeError('steer_already_sent')
        if selection.get('status') != 'selected':
            raise ValueError('steer_requires_selected_attempt')
        # Supply the actual supported input identity, never a guessed host id.
        # Missing identity permits a send, but cannot prove continuation.
        actor = f'user:{origin}:{event_id}' if all(isinstance(v, str) and v for v in (origin, event_id)) else None
        actor_hash = None
        if actor is not None:
            encoded = json.dumps(actor, ensure_ascii=False, separators=(',', ':')).encode()
            actor_hash = hashlib.sha256(encoded).hexdigest()
        self.sent = dict(selection=deepcopy(selection), actor_sha256=actor_hash, receipt=None)
        self.sent['receipt'] = self._send(text)
        return self.sent['receipt']


def check_continuation(selection, snapshot, *, steering=None):
    """Same-identity progress AFTER capture, not proof of host cancel-then-prompt.

    USER_STEERING is the capture boundary, not STEERING_APPLIED: the latter
    can be applied after this worker has already completed.
    """
    if snapshot.get('status') != 'complete':
        return dict(status='unverified', reason='snapshot_incomplete')
    binding = selection.get('binding') or {}
    if not all(binding.get(k) is not None for k in ('project_root', 'task_id', 'session_id', 'owner_epoch')):
        return dict(status='unverified', reason='baseline_binding_unavailable')
    if snapshot.get('binding') != binding:
        return dict(status='failed', reason='binding_changed')
    baseline = selection.get('event_high_watermark')
    if type(baseline) is not int or baseline < 0:
        return dict(status='unverified', reason='baseline_events_unavailable')
    if not steering or steering.get('selection') != selection or not steering.get('actor_sha256'):
        return dict(status='unverified', reason='steering_identity_unavailable')
    ref, attempt_id, session = selection['assignment_ref'], selection['attempt_id'], selection['engine_session_ref']
    provisioned = [a for a in _section(snapshot, 'attempts')
                   if a.get('idempotency_key') == ref or a.get('attempt_id') == attempt_id
                   or a.get('engine_session_ref') == session]
    if (len(provisioned) != 1 or provisioned[0].get('idempotency_key') != ref
            or provisioned[0].get('attempt_id') != attempt_id or provisioned[0].get('engine_session_ref') != session):
        return dict(status='failed', reason='attempt_replaced')
    if provisioned[0].get('owner_epoch') != binding['owner_epoch']:
        return dict(status='failed', reason='owner_changed')
    confirmed = [e for e in _section(snapshot, 'task_events')
                 if e.get('kind') == 'worker_configuration_confirmed'
                 and (e.get('attempt_id') == attempt_id or e.get('engine_session_ref') == session
                      or e.get('assignment_ref') == ref)]
    if (len(confirmed) != 1 or confirmed[0].get('attempt_id') != attempt_id
            or confirmed[0].get('engine_session_ref') != session
            or confirmed[0].get('assignment_ref') != ref):
        return dict(status='failed', reason='native_confirmation_mismatch')
    events = _section(snapshot, 'runtime_events')
    ids = [e.get('id') for e in events]
    if any(type(i) is not int or i < 1 for i in ids) or len(set(ids)) != len(ids):
        return dict(status='failed', reason='ambiguous_runtime_events')
    terminal = [e for e in events if e['type'] in _TERMINAL_WORKER_EVENTS and e.get('attempt_id') == attempt_id]
    if len(terminal) > 1:
        return dict(status='failed', reason='ambiguous_terminal_result')
    if any(e['type'] == 'WORKER_FAILED' or e.get('assignment_outcome') == 'failed' for e in terminal):
        return dict(status='failed', reason='attempt_failed')
    captured = [e for e in events if e['type'] == 'USER_STEERING'
                and e.get('actor_sha256') == steering['actor_sha256']]
    if len(captured) > 1 or (captured and captured[0]['id'] <= baseline):
        return dict(status='failed', reason='steering_replayed_or_ambiguous')
    if snapshot.get('snapshot_sha256') == selection.get('snapshot_sha256') or not captured:
        return dict(status='pending', reason='steering_capture_not_observed')
    boundary = captured[0]['id']
    if terminal and terminal[0]['id'] > boundary:
        return dict(status='continued', reason='completed_same_attempt')
    again = select_running_attempt(snapshot)
    if (again.get('status') == 'selected' and all(again[k] == selection[k] for k in
                                               ('assignment_ref', 'attempt_id', 'engine_session_ref'))
            and any(e['type'] == 'TOOL_COMPLETED' and e['id'] > boundary
                    and e.get('channel_attempt_id') == attempt_id
                    and e.get('channel_owner_epoch') == binding['owner_epoch'] for e in events)):
        return dict(status='continued', reason='progressed_same_attempt')
    return dict(status='pending', reason='continuation_not_observed')


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


def supervise_cli(consent, preflight, launch, assert_task, cleanup, *, clock=time.monotonic):
    """Finite phase supervisor for an explicitly owner-requested CLI case.

    Adapters use the current guarded CLI and bounded_command below. They must
    return observed host/ACP bindings, never guessed IDs. This function grants
    no permission, supplies no route/model defaults and never accepts a task.
    Cleanup is adapter-owned (only resources this case actually started).
    """
    route = consent.get("route") if isinstance(consent, dict) else None
    if (not isinstance(consent, dict) or consent.get("mode") != "cli"
            or not isinstance(consent.get("owner_request"), str) or not consent["owner_request"].strip()
            or not isinstance(route, str) or not route.strip()):
        return dict(passed=False, phase="consent_required", dispatched=False, asserted=False, accepted=False)
    budget = Budget(clock=clock)
    phase = dict(phase="preflight", dispatched=False, asserted=False, accepted=False)

    def exercise(budget):
        ready = preflight(budget)
        required = {"isolation", "native_login", "build", "plugin", "supported_cli", "route", "expectations"}
        checks = ready.get("checks", {}) if isinstance(ready, dict) else {}
        if (not isinstance(ready, dict) or ready.get("ready") is not True or ready.get("route") != route
                or set(checks) != required or any(v is not True for v in checks.values())):
            return dict(status="preflight_failed", **phase)
        if budget.clock() >= budget.observe_deadline:
            return dict(status="deadline", **phase)
        # An interrupted launch has unknown disposition, never preflight-only.
        phase.update(phase="launch_attempted", dispatched=None)
        started = launch(budget, route)
        if not isinstance(started, dict) or started.get("dispatched") is not True or not started.get("binding"):
            return dict(status="launch_uncertain", **phase)
        phase.update(phase="dispatched", dispatched=True)
        result = assert_task(budget, started["binding"])
        if (not isinstance(result, dict) or result.get("passed") is not True
                or result.get("binding") != started["binding"]):
            return dict(status="assertion_failed", **phase)
        phase.update(phase="asserted", asserted=True)
        return dict(status="passed", **phase)

    result = run_case(budget, exercise, cleanup)
    return dict(result, **phase)


def bounded_command(argv, budget, *, cleanup=False, timeout=MAX_WAIT_SECONDS):
    """Real finite subprocess primitive for adapters; no shell or paid defaults.

    Captures at most 1 MiB, kills only this process group on a timeout, and
    returns complete stdout privately to the adapter. Never print raw output.
    """
    import os
    import signal
    import subprocess
    import tempfile

    deadline = budget.case_deadline if cleanup else budget.observe_deadline
    remaining = min(timeout, MAX_WAIT_SECONDS, deadline - budget.clock())
    if remaining <= 0:
        raise TimeoutError("case_deadline")
    with tempfile.TemporaryFile() as output:
        process = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=remaining)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
            raise
        size = output.tell()
        if size > 1048576:
            raise ValueError("command_output_limit")
        output.seek(0)
        raw = output.read()
    return dict(exit_code=code, complete=True, byte_count=size, output=raw)
