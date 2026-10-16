"""Deterministic task driver over ACP engine sessions and runtime services."""
from __future__ import annotations

import contextlib
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import threading
import time

from .state import CONSULTATION_NODE, TaskMachine, encoded, event, now, transition
from .prompt_context import coordinator_prompt_context
from . import user_text
from .worker_brief import build_worker_brief
from ..contracts import StoreError
from ..delegation import events
from ..delegation import revision
from ..portable.engine_contract import StopReason
from ..observability.redact import StreamingRedactor
from ..acp.wire_budget import SESSION_UPDATE_MAX_BYTES, bound_value, truncation_marker

# Coordinator engine failure kinds that mean the session can never be prompted
# again (the process/transport is gone), as distinct from a slow-but-alive
# engine. See engine.failure_classification._ENGINE_FAILURE_MARKERS;
# 'timeout_alive' is deliberately excluded -- it never applies to the unbounded
# coordinator prompt turn, and the engine is still alive when it is raised.
_COORDINATOR_SESSION_DEAD_KINDS = frozenset({
    'transport_eof', 'poisoned', 'process_exit', 'process_crashed', 'nonzero_exit',
})


def _transient_turn_failure(kind, dead):
    """A coordinator turn failure a resend can cure: its session died, or
    the engine/transport failed (never a rate limit, an auth loss, a
    refusal or a cancellation)."""
    if dead:
        return True
    if not kind:
        return False
    from ..engine import failure_classification
    return failure_classification.classify(kind) in {
        failure_classification.ENGINE_FAILURE, failure_classification.TRANSPORT_FAILURE}


# While workers run, the scheduler repeats its full pass (context rebuild,
# claim_ready, event processing) only when the task's change fingerprint moves,
# a worker future of this task ends, or a runtime notification arrives -- and
# in any case at least every _RECONCILE_SECONDS, which also recovers a change
# the fingerprint does not cover. Permission cards keep the _POLL_SECONDS tick.
_POLL_SECONDS = .1
_RECONCILE_SECONDS = 2.0
# Allow owned post-result settlement teardown plus exit cleanup, never an
# unbounded wait for an arbitrary live or unknown worker.
_SYNTHESIS_SETTLE_SECONDS = 10.0


_VERIFICATION_OUTPUT_TAIL_BYTES = 2500
_VERIFICATION_RAW_OUTPUT_MAX_BYTES = SESSION_UPDATE_MAX_BYTES // 2


def _own_paragraph(text):
    """Chunks of one ACP message are concatenated, so a runtime block after
    the coordinator's streamed prose must start its own paragraph; otherwise
    a leading Markdown heading or list lands mid-line and is not parsed."""
    return text if text.startswith('\n') else '\n\n' + text


def _execution_lock(task_id):
    """Serializes starting a plan revision and dispatching its ready nodes
    between the controller loop and the early start below."""
    with _EXECUTION_LOCKS_GUARD:
        return _EXECUTION_LOCKS.setdefault(task_id, threading.Lock())


_EXECUTION_LOCKS = {}
_EXECUTION_LOCKS_GUARD = threading.Lock()


def _title_from_objective(objective, max_chars=200):
    """A short display title for an assignment card/capsule entry.

    A hard ``[:max_chars]`` slice can land mid-word. Cut at the last
    whitespace within the limit instead; an objective with no whitespace
    before the limit falls back to the hard slice rather than growing
    unbounded."""
    if len(objective) <= max_chars:
        return objective
    cut = objective[:max_chars]
    boundary = cut.rfind(' ')
    return (cut[:boundary] if boundary > 0 else cut).rstrip()


def _dispatch_ready(runtime, machine, task_id, channel, context, emit_tool_call):
    for node in machine.claim_ready(task_id, channel.channel_id):
        dependencies = set(json.loads(node['dependencies_json']))
        inputs = [dict(node=n['node_id'], report_ref=n['result']['report_ref'])
                  for n in context['nodes'] if n['node_id'] in dependencies and n['result']]
        if node['node_id'] == CONSULTATION_NODE:
            # An escalation-created consultant reads no project source: its
            # packet is the runtime-built report selection.
            inputs = machine.consultation_reports(task_id, node.get('revision', context['revision'])) + inputs
        review_evidence = []
        if node['stage'] == 'review':
            from .artifacts import project_git_evidence
            from .reviewer_handoff import review_runtime_facts
            observation = {}
            try:
                machine.start_review(task_id, node['assignment_ref'], observation=observation)
            except StoreError as error:
                # Never a crashed turn: ask the user with the reason.
                machine.move(task_id, 'BLOCKED', blocker=str(error))
                return
            review_evidence.append(review_runtime_facts(
                runtime._store, task_id, node['assignment_ref'], runtime._task_workspace_root(task_id),
                observed=observation or None))
            for kind in ('status', 'diff'):
                ref = f"review:{node['assignment_ref']}:{kind}"
                if emit_tool_call:
                    emit_tool_call(dict(tool_call_id=ref,new=True,title='Read project '+kind,kind='read',status='in_progress',raw_input=dict(kind=kind)))
                observed = project_git_evidence(runtime._store,task_id,node['assignment_ref'],runtime._task_workspace_root(task_id),kind)
                review_evidence.append(observed)
                if emit_tool_call:
                    emit_tool_call(dict(tool_call_id=ref,new=False,status='completed' if observed['available'] else 'failed',raw_output=observed))
        brief = build_worker_brief(
            objective=node['objective'],
            scope=json.loads(node['scope_json']),
            dependency_reports=inputs,
            original_goal=context['goal'],
            review_evidence=review_evidence,
            assignment_ref=node['assignment_ref'],
        )
        from .scheduler import dispatch_assignment
        dispatch_assignment(runtime._store, channel, dict(
            title=_title_from_objective(node['objective']), brief=brief, role=node['role'],
            acceptance='Publish a truthful immutable report and structured result. '
                       + ('Publish the complete review report inline with result_submit and include a review verdict.'
                          if node['stage'] == 'review' else 'Complete the assigned objective.'),
            request_key=node['request_key']))


def _start_execution_early(runtime, machine, task_id, channel, cancel_event, emit_tool_call):
    """A plan accepted while the coordinator's turn is still streaming starts
    its workers now instead of after that turn's closing text."""
    try:
        with _execution_lock(task_id):
            if cancel_event.is_set() or _transport_lost(runtime, task_id):
                return
            if machine.get_state(task_id) != 'PLAN_READY':
                return
            machine.move(task_id, 'EXECUTING')
            _dispatch_ready(runtime, machine, task_id, channel, machine.context(task_id), emit_tool_call)
        runtime._process_new_events_safely(task_id, channel, cancel_event, emit_tool_call, phase='early_scheduler')
    except Exception as error:  # noqa: BLE001 - the controller loop retries the same start
        runtime._record_coordinator_dispatch_failure(task_id, error, phase='early_scheduler')


def _verification_output_tail(output):
    """Read a bounded tail from the temporary output without loading it whole."""
    output.seek(0, os.SEEK_END)
    total_bytes = output.tell()
    start = max(0, total_bytes - _VERIFICATION_OUTPUT_TAIL_BYTES)
    output.seek(start)
    tail = output.read(_VERIFICATION_OUTPUT_TAIL_BYTES).decode("utf-8", errors="replace")
    if start:
        tail = truncation_marker(start) + "\n" + tail
    return tail


@contextlib.contextmanager
def _verification_command(root, scratch, argv):
    """Prepare the production sandbox; tests may replace only this launch seam.

    Every access mode runs a check in the read-only host profile of a role
    without a write grant (``sandbox.readonly_command``):
    the host and HOME read-only except the explicit tool-cache allowlist, a
    private TMPDIR and /var/tmp, its own PID namespace, the protected control
    plane read-only. Only the project's git-ignored directories (caches,
    builds, node_modules) are writable, so sources stay untouched and the
    revision (which skips ignored paths) cannot drift. pytest's cache goes
    to the scratch directory."""
    from .sandbox import readonly_command, with_umask, readonly_test_cache
    scopes = [path + '/**' for path in revision.ignored_directories(str(root))]
    with readonly_command(root, scratch, with_umask(argv), environ=os.environ, scopes=scopes) as (command, fds):
        readonly_test_cache(command, scratch, os.environ)
        yield command, fds


def _verification_observation(runtime, task_id, context, expected, emit_tool_call, cancel, run_id):
    """Run retained literal argv commands to completion with complete output.

    The project is read-only outside its git-ignored directories and the
    rest of the host is read-only except tool caches and private temp, in
    every access mode (``_verification_command``), so a mode switch during a
    check changes nothing about it. The real runtime store is inaccessible.
    Missing OS isolation is a failure, never an unsandboxed fallback.
    """
    root = Path(runtime._task_workspace_root(task_id))
    before = revision.capture_revision(str(root))
    digest = before.get('dirty_digest') or before.get('digest')
    if not digest:
        return dict(blocker='verification_revision_unavailable', stale=True)
    if not expected or expected != digest:
        return dict(blocker='verification_review_revision_changed', stale=True)
    checks = []
    for index, argv in enumerate(context['plan']['verification']):
        ref = f"verification:{context['revision']}:{run_id}:{index}"
        if cancel.is_set():
            return dict(checks=checks, digest=digest, stale=True)
        if emit_tool_call:
            emit_tool_call(dict(tool_call_id=ref, new=True, title='Verify project', kind='execute',
                                status='in_progress', raw_input=dict(argv=argv, cwd=str(root))))
        from ..observability.wire_trace import get_trace
        trace = get_trace()
        if trace is not None:
            trace.record("verification", "out", dict(argv=argv, cwd=str(root)),
                         task_id=task_id, command_ref=ref)
        with tempfile.TemporaryDirectory(prefix='cortex-verification-') as scratch, contextlib.ExitStack() as stack:
            # The owner's toolchain (npm, node in their home), as a worker's
            # command_run has it: a bare /usr/bin sandbox made every npm check
            # fail. Only directories the project ignores (caches, builds,
            # node_modules) are writable, so sources stay untouched and the
            # revision (which skips ignored paths) cannot drift. No access to
            # the real private store, even from an interpreter.
            from ..configuration.access_mode import READ_ONLY_COMMAND_PROFILE
            profile = READ_ONLY_COMMAND_PROFILE
            try:
                command, fds = stack.enter_context(_verification_command(root, scratch, argv))
            except StoreError as error:
                if trace is not None:
                    trace.record("verification", "in", dict(blocker=str(error)),
                                 task_id=task_id, command_ref=ref)
                return dict(blocker=str(error), stale=False)
            with tempfile.TemporaryFile() as output:
                from .sandbox import compact_argv
                launch, launch_fds = compact_argv(command, fds)
                with runtime._store.connection() as db:
                    db.execute('BEGIN IMMEDIATE')
                    # Execution metadata is diagnostic until authoritative credit;
                    # never emit a verification lifecycle event from this runner.
                    event(db, task_id, 'CHECK_EXECUTION_STARTED', dict(
                        command_ref=ref, argv=argv, revision=context['revision'],
                        revision_digest=digest, index=index, profile=profile))
                    started_event_id = db.execute('SELECT last_insert_rowid()').fetchone()[0]
                    db.commit()
                try:
                    proc = subprocess.Popen(launch, stdout=output, stderr=subprocess.STDOUT, pass_fds=launch_fds,
                                            env={'PATH': '/usr/bin:/bin'}, start_new_session=True)
                except Exception as error:
                    with runtime._store.connection() as db:
                        db.execute('BEGIN IMMEDIATE')
                        event(db, task_id, 'CHECK_EXECUTION_ABORTED', dict(command_ref=ref, run_id=run_id))
                        db.commit()
                    if trace is not None:
                        trace.record("verification", "in", dict(launch_error=type(error).__name__),
                                     task_id=task_id, command_ref=ref)
                    if emit_tool_call:
                        emit_tool_call(dict(tool_call_id=ref, new=False, status='failed',
                                            raw_output=dict(launch_error=type(error).__name__)))
                    # A visible stop with its reason; re-raised, the same
                    # launch crashed every later VERIFYING turn again.
                    return dict(blocker=f'verification_execution_error:{type(error).__name__}', stale=False)
                finally:
                    for extra in set(launch_fds) - set(fds):
                        os.close(extra)
                # No time limit, as for a worker's command_run: a project's
                # full suite runs to completion; only a task stop ends it.
                interrupted = False
                while proc.poll() is None:
                    if cancel.wait(.1):
                        interrupted = True
                        _kill_verification(proc)
                        break
                code = proc.wait()
                if trace is not None:
                    # Read the spool separately; retention below still receives
                    # the complete stream from offset zero. Lines keep credentials
                    # intact for redaction even across pipe-read boundaries.
                    output.seek(0)
                    private_key = False
                    for line in output:
                        text = line.decode("utf-8", errors="replace")
                        if private_key or ("-----BEGIN " in text and "PRIVATE KEY-----" in text):
                            private_key = not ("-----END " in text and "PRIVATE KEY-----" in text)
                            text = "[REDACTED private-key output]\n"
                        trace.record("verification", "in", dict(output=text),
                                     task_id=task_id, command_ref=ref)
                    trace.record("verification", "in",
                                 dict(exit_code=code, interrupted=interrupted),
                                 task_id=task_id, command_ref=ref)
                after = revision.capture_revision(str(root))
                unchanged = (after.get('dirty_digest') or after.get('digest')) == digest
                from ..recovery.large_output_store import persist_output_stream
                output.seek(0)
                saved = persist_output_stream(
                    runtime._store, task_id=task_id, owner_scope='coordinator',
                    source_kind='terminal', source_ref=ref, stream=output,
                    exit_code=code, failure='interrupted' if interrupted else None,
                )
                output_tail = _verification_output_tail(output)
        failed = interrupted or code != 0 or not unchanged
        observed = dict(argv=argv,exit_code=code,complete=not interrupted and unchanged,
                        revision_digest=digest,large_output_id=saved['id'],profile=profile,
                        command_ref=ref,started_event_id=started_event_id,
                        # The full output is retained separately; this excerpt is only for repair context.
                        output_tail=output_tail if failed else None)
        with runtime._store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            event(db, task_id, 'CHECK_EXECUTION_FINISHED',
                  dict({k: v for k, v in observed.items() if k != 'output_tail'}, run_id=run_id))
            db.commit()
        checks.append(observed)
        if emit_tool_call:
            raw_output = {key: value for key, value in observed.items() if key != 'output_tail'}
            raw_output['output'] = output_tail
            emit_tool_call(dict(
                tool_call_id=ref, new=False,
                status='completed' if code == 0 and observed['complete'] else 'failed',
                raw_output=bound_value(raw_output, _VERIFICATION_RAW_OUTPUT_MAX_BYTES),
            ))
        if failed:
            break
    after = revision.capture_revision(str(root))
    stale = (cancel.is_set() or
             (after.get('dirty_digest') or after.get('digest')) != digest or
             any(not c['complete'] for c in checks))
    if stale and checks:
        checks[-1]['complete'] = False
    return dict(checks=checks, digest=digest, stale=stale)


def _kill_verification(proc):
    # Also stop descendants: discarded checks must not leave cache writers.
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def _verification(runtime, task_id, machine, emit_tool_call, cancel):
    context = machine.context(task_id)
    reviews = [node for node in context['nodes'] if node['stage'] == 'review']
    expected = ((reviews[-1]['result'] or {}).get('project_digest') if reviews
                else (context['review_not_required'] or {}).get('revision_digest'))
    try:
        observed = _verification_observation(
            runtime, task_id, context, expected, emit_tool_call, cancel, time.monotonic_ns())
    except Exception as error:  # noqa: BLE001 - any runner failure is a visible stop, never a crash loop
        if cancel.is_set():
            return
        # Steering is not applied in VERIFYING: an escaping exception left
        # the task there and crashed every later turn the same way.
        machine.move(task_id, 'BLOCKED', blocker=f'verification_execution_error:{type(error).__name__}')
        return
    if cancel.is_set():
        return
    if observed.get('blocker'):
        machine.move(task_id, 'BLOCKED', blocker=observed['blocker'])
    else:
        machine.record_verification(task_id, observed['checks'], observed['digest'])


class _SpeculativeVerification:
    """One owned command sequence, never an extra engine/worker reservation.

    Only the controller can credit it after review and engine teardown. Losing
    the turn discards the candidate; recovery uses ordinary verification.
    """
    def __init__(self, runtime, task_id, emit_tool_call):
        self.runtime, self.task_id, self.emit_tool_call = runtime, task_id, emit_tool_call
        self.thread = None
        self.binding = None
        self.observed = None

    def _event(self, kind, **payload):
        with self.runtime._store.connection() as db:
            db.execute('BEGIN IMMEDIATE')
            event(db, self.task_id, 'SPECULATIVE_VERIFICATION_' + kind,
                  dict(binding=self.binding, run_id=self.run_id, **payload))
            db.commit()

    def start(self, context, assignment_ref, digest):
        binding = dict(revision=context['revision'], assignment_ref=assignment_ref, digest=digest)
        if self.binding == binding:
            return
        self.close('review_binding_changed')
        self.binding = binding
        self.cancel = threading.Event()
        self.observed = None
        self.run_id = time.monotonic_ns()
        self._event('STARTED')
        def work():
            try:
                self.observed = _verification_observation(
                    self.runtime, self.task_id, context, digest, self.emit_tool_call,
                    self.cancel, self.run_id)
            except Exception:
                # Never a pass or a lifecycle mutation from the command thread.
                self.observed = dict(stale=True, blocker='speculative_verification_error')
            self._event('FINISHED', stale=self.observed.get('stale', False),
                        blocker=self.observed.get('blocker'),
                        failed=any(c.get('exit_code') != 0 or not c.get('complete')
                                   for c in self.observed.get('checks', [])))
        self.thread = threading.Thread(target=work, name='cortex-speculative-verification')
        self.thread.start()

    def refresh(self, context):
        reviews = [n for n in context['nodes'] if n['stage'] == 'review']
        if not reviews or reviews[-1]['status'] != 'running':
            return
        review = reviews[-1]
        with self.runtime._store.connection() as db:
            row = db.execute(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' "
                "AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
                (self.task_id, review['assignment_ref'])).fetchone()
        if row:
            self.start(context, review['assignment_ref'], json.loads(row[0])['revision_digest'])

    def credit(self, context, machine):
        if not self.thread:
            return False
        reviews = [n for n in context['nodes'] if n['stage'] == 'review']
        review = reviews[-1] if reviews else None
        expected = dict(revision=context['revision'], assignment_ref=review['assignment_ref'],
                        digest=review['result']['project_digest']) if review else None
        if self.binding != expected:
            self.close('approved_binding_changed')
            return False
        if self.thread.is_alive():
            return None
        observed = self.observed
        current = revision.capture_revision(self.runtime._task_workspace_root(self.task_id))
        if (not observed or observed.get('stale') or
                (current.get('dirty_digest') or current.get('digest')) != self.binding['digest']):
            self.close('project_revision_changed')
            return False
        if observed.get('blocker'):
            self.close('speculative_verification_unavailable')
            return False
        # Bind credit transactionally to the exact plan and approving reviewer.
        recorded = machine.record_verification(
            self.task_id, observed['checks'], observed['digest'], speculative_binding=self.binding)
        if recorded is None:
            self.close('approved_binding_changed')
            return False
        self.thread = None
        self.binding = None
        return True

    def close(self, reason='controller_turn_ended'):
        if self.thread:
            self.cancel.set()
            self.thread.join()
            self._event('DISCARDED', reason=reason)
            self.thread = None
            self.binding = None
            self.observed = None


def _synthesis_workers_absent(runtime, task_id, cancel_event, db):
    """Completion liveness checks must use the completion writer connection."""
    lookup = getattr(runtime._engine, 'lookup_owned_tx', None)
    # An engine without a transaction-safe gate retains coordinator synthesis.
    if lookup is None:
        return False
    with runtime._lock:
        if runtime._running.get(task_id) or cancel_event.is_set():
            return False
        workers = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'",
                             (task_id,)).fetchall()
        return all(lookup(row['attempt_id'], db) == 'absent' for row in workers)


def _await_synthesis_settlement(runtime, task_id, cancel_event):
    """Await owned post-result cleanup without holding a SQLite transaction.

    This cannot authorize completion: the caller retries every composition and
    transaction-safe liveness gate afterwards. Never terminate an engine here.
    """
    pending = getattr(runtime._engine, 'result_settlement_pending', None)
    if pending is None or getattr(runtime._engine, 'lookup_owned_tx', None) is None:
        return
    with runtime._lock:
        if runtime._running.get(task_id):
            return
    with runtime._store.connection() as db:
        task = db.execute('SELECT state,synthesis FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if not task or task['state'] != 'RECONCILING' or task['synthesis'] is not None:
            return
        attempts = [row[0] for row in db.execute(
            "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'",
            (task_id,))]
    deadline = time.monotonic() + _SYNTHESIS_SETTLE_SECONDS
    while not cancel_event.is_set() and not _transport_lost(runtime, task_id):
        states = [(attempt, runtime._engine.lookup_owned(attempt)) for attempt in attempts]
        if any(state not in {'alive', 'absent'} for _, state in states):
            return
        live = [attempt for attempt, state in states if state == 'alive']
        if not live or any(not pending(attempt) for attempt in live):
            return
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return
        cancel_event.wait(min(_POLL_SECONDS, remaining))


def run(runtime, task_id, session_id, channel, session_ref, content, emit_update,
        cancel_event, language, host_elicitor=None, *, emit_tool_call=None,
        emit_display_update=None, prompt_blocks=None, initial_project_docs_context=None):
    speculation = _SpeculativeVerification(runtime, task_id, emit_tool_call)
    wait_cards = {}
    try:
        return _run(runtime, task_id, session_id, channel, session_ref, content, emit_update,
                    cancel_event, language, host_elicitor, emit_tool_call=emit_tool_call,
                    emit_display_update=emit_display_update, prompt_blocks=prompt_blocks,
                    initial_project_docs_context=initial_project_docs_context, speculation=speculation,
                    wait_cards=wait_cards)
    finally:
        speculation.close()
        if wait_cards:
            from .wait_notices import close_all
            close_all(task_id, emit_tool_call, language, wait_cards)


def _run(runtime, task_id, session_id, channel, session_ref, content, emit_update,
        cancel_event, language, host_elicitor=None, *, emit_tool_call=None,
        emit_display_update=None, prompt_blocks=None, initial_project_docs_context=None,
        speculation=None, wait_cards=None):
    machine = TaskMachine(runtime._store)
    wait_cards = {} if wait_cards is None else wait_cards
    context = machine.create(task_id, content)
    from .recovery import reconcile_results
    if context['state'] not in {'COMPLETED', 'FAILED', 'CANCELLED'}:
        try:
            reconcile_results(runtime._store, task_id)
        except StoreError as error:
            machine.move(task_id, 'BLOCKED', blocker=str(error))
            context = machine.context(task_id)
    context = machine.context(task_id)
    if context['state'] == 'WAITING_USER':
        from .questions import resume_answered
        resumed = resume_answered(runtime._store, task_id)
        if isinstance(resumed, tuple) and resumed[0] == 'force_finish':
            runtime.stop_task(task_id, session_id, emit_update, emit_tool_call=emit_tool_call,
                              stop_source='user_answer_force_finish', question_id=resumed[1])
            return StopReason.CANCELLED
    if context['state'] == 'BLOCKED':
        # A task stopped before stops became questions: ask now.
        machine.move(task_id, 'BLOCKED', blocker=context['blocker'])
    prompted = {}
    published_states = set()
    capacity_wait_started = None
    failure_stop_reason = StopReason.END_TURN
    turn_failures = []
    runtime_synthesis_emitted = False
    while True:
        if cancel_event.is_set() or _transport_lost(runtime, task_id):
            speculation.close('task_stopped')
            return _stop_turn(runtime, task_id, machine, emit_tool_call, emit_update, language)
        # Read before the pass: a change committed during it moves the value.
        fingerprint = machine.change_fingerprint(task_id)
        context = machine.context(task_id)
        state = context['state']
        if state not in {'REVIEWING', 'VERIFYING'}:
            speculation.close('review_not_approved')
        from .policy import apply_pending_steering
        # Steering applies only at a boundary; in EXECUTING/REVIEWING the call
        # is a no-op that would still take the write lock on every pass.
        if state not in {'EXECUTING', 'REVIEWING'} and apply_pending_steering(runtime._store, task_id):
            prompted.clear()
            continue
        if state == 'ANSWER_READY':
            # COMPLETED only for an informational answer or certified work: a
            # final answer on work whose review/verification did not pass is
            # shown, and the task ends FAILED (answer_not_certified:...).
            closed = machine.close_answer(task_id)
            from .pipeline import publish
            publish(runtime._store,channel,machine.context(task_id))
            emit_update(_own_paragraph(context['synthesis']))
            if closed != 'COMPLETED':
                continue
            runtime._set_state(task_id,'accepted',wait_reason=None)
            return StopReason.END_TURN
        if state == 'RECOVERING':
            # Explicit cancellation/replanning revokes first, then terminates
            # owned work before accepting a new plan.
            for node in context['nodes']:
                if node['assignment_ref']:
                    runtime._cancel_assignment(task_id,node['assignment_ref'],'replanning',emit_tool_call=emit_tool_call)
            with runtime._store.connection() as db:
                attempts=db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'",(task_id,)).fetchall()
            # Finish off what is still owned before judging it: otherwise an
            # answer to the resulting question re-enters this same check forever.
            for row in attempts:
                if runtime._engine.lookup_owned(row['attempt_id']) != 'absent':
                    try: runtime._engine.terminate_owned(row['attempt_id'])
                    except Exception: pass
            if any(runtime._engine.lookup_owned(row['attempt_id']) != 'absent' for row in attempts):
                machine.move(task_id,'BLOCKED',blocker='replanning_worker_termination_unconfirmed')
            else:
                from .policy import resume_planning
                resume_planning(runtime._store,task_id)
                # Recovery returns to PLANNING, not the question's
                # RECOVERING resume phase. Its exhausted prompt count must
                # not block the newly answered decision before an engine turn.
                prompted.pop(('PLANNING', context['revision']), None)
            context=machine.context(task_id)
            continue
        publication_key = (context['revision'], context['review_cycle'], state,
                           tuple(n['wait']['event_id'] for n in context['nodes'] if n.get('wait')))
        if publication_key not in published_states:
            from .pipeline import publish
            publish(runtime._store, channel, context)
            published_states.add(publication_key)
        if state == 'RECONCILING':
            _await_synthesis_settlement(runtime, task_id, cancel_event)
            if cancel_event.is_set() or _transport_lost(runtime, task_id):
                continue
            from .synthesis import complete as compose_synthesis
            def workers_absent(db):
                return _synthesis_workers_absent(runtime, task_id, cancel_event, db)
            synthesis = compose_synthesis(machine, task_id, language, workers_absent)
            if synthesis is not None:
                emit_update(_own_paragraph(synthesis))
                runtime_synthesis_emitted = True
                continue
        if state in {'PLANNING', 'RECONCILING'}:
            key = (state, context['revision'])
            if prompted.get(key,0)>=3:
                machine.move(task_id, 'BLOCKED', blocker='structured_proposal_missing')
                continue
            prompted[key]=prompted.get(key,0)+1
            request = (_direct_answer_request(runtime, task_id) +
                       'Explaining how to run tests does not require executing them. '
                       'The coordinator can inspect files but cannot execute commands. Do not expand a description into a defect audit. '
                       'If changes or substantial delegated work are required, submit a focused structured plan. '
                       'Runtime schedules planned workers, then independent review whenever the project changed, then verification. '
                       + _released_request(runtime, task_id)
                       + _unknown_outcome_request(runtime, task_id)
                       + _skill_proposal_request(context)
                       + (_ask_user_request(context) or _verification_repair_request(context)
                          or _failed_worker_request(context))
                       if state == 'PLANNING' else
                       'Reconcile the retained results and runtime verification. Every changed surface must be verified. '
                       'If a retained not_run or unverified check, or a changed surface nobody verified, can still be verified '
                       'with the available environment and the user\'s authorizations, submit replan_request to verify it before synthesis. '
                       'List in synthesis not_verified only what a real blocker makes impossible, each with that blocker; '
                       'the synthesis text\'s not-verified list must be exactly those items with their blockers, and omit that list when everything was verified. '
                       'Write blocker, and description whenever item is an exact retained check name, in this turn\'s selected chat language; item itself stays the exact check name, unchanged by language, whenever it closes a retained gap. '
                       'Runtime decides completion. '
                       'If the user must still decide something (approve a proposed plan, choose between fixes, confirm manual steps), '
                       'do not submit synthesis: write the full details once as message text before the call, then ask with '
                       'user_question_request and end the turn without writing anything after it. ')
            delivery_marker=f'runtime-coordinator:{time.monotonic_ns()}'
            pending_events=events.claim_unconsumed(runtime._store,task_id,delivery_marker)
            from ..acp import messages as localized_messages
            prompt_context = coordinator_prompt_context(context)
            message = localized_messages.render('answer_in_language',language)+'\n'+request
            if prompt_context is not context:
                message += ('\nEarlier review-cycle nodes are compact summaries; '
                           'historical_check_gaps retains older non-passing check records. '
                           'Full node history remains available through task_get_context. ')
            message += '\n' + encoded(user_text.bound_context(runtime._store, task_id, prompt_context))
            if pending_events:
                message += '\nRetained events:\n'+encoded([
                    dict(type=e.event_type,payload=user_text.bound_event_payload(runtime._store,task_id,e.payload))
                    for e in pending_events])
            if state == 'PLANNING':
                if not prompt_blocks:
                    message += '\nCurrent user input:\n' + user_text.bound_text(runtime._store, task_id, content)
                if initial_project_docs_context:
                    message += '\n' + initial_project_docs_context
            # Reassert the selected turn language after English report/context
            # blocks. Worker text is evidence, never the coordinator's locale.
            message += '\n\n' + localized_messages.render('answer_in_language',language)
            try:
                message, delivery_id, delivery_blocked = runtime._begin_generation_delivery(
                    task_id, session_ref, 'orchestrator', message, emit_update=emit_update, language=language)
            except BaseException:
                # Nothing reached the coordinator: its claimed events stay deliverable.
                events.release_claim(runtime._store, task_id, delivery_marker)
                raise
            if delivery_blocked:
                events.release_claim(runtime._store,task_id,delivery_marker)
                with runtime._store.connection() as db:
                    delivery_state=db.execute('SELECT state,wait_reason FROM task_orchestration WHERE task_id=?',(task_id,)).fetchone()
                if delivery_state and delivery_state['state']=='blocked':
                    machine.move(task_id,'BLOCKED',blocker=delivery_state['wait_reason'] or 'context_delivery_blocked')
                return StopReason.END_TURN
            streamed = []
            answer_pending = False
            # Once the coordinator's question is committed (WAITING_USER), its
            # text for the rest of this turn is suppressed. A delayed lifecycle
            # callback (even a new id) cannot prove that preceding deltas were
            # written before the question. Do not release text on that signal.
            # Already accepted redactor tails still flush; later turns reset.
            question_asked = False
            redactor = StreamingRedactor(max_chars=4000, preserve_paths=True)
            tool_emitter = runtime._engine_tool_call_emitter(emit_tool_call, id_scope=f'coordinator:{session_ref}',
                                                           task_id=task_id, cancel_event=cancel_event)
            def flush():
                tail = redactor.flush()
                if tail:
                    emit_update(tail)
            early_start = []
            def start_early(current):
                # The accepted plan's workers start while the coordinator is
                # still writing its closing text, not seconds after it.
                if current == 'PLAN_READY' and not early_start:
                    early_start.append(threading.Thread(
                        target=_start_execution_early, name='cortex-early-start', daemon=True,
                        args=(runtime, machine, task_id, channel, cancel_event, emit_tool_call)))
                    early_start[0].start()
            def on_delta(text):
                nonlocal answer_pending, question_asked
                # Some adapters deliver text before the tool lifecycle callback.
                # The committed proposal is the authority for suppressing its echo.
                if answer_pending: return True
                if not question_asked:
                    current = machine.get_state(task_id)
                    start_early(current)
                    answer_pending = current == 'ANSWER_READY'
                    if answer_pending: return True
                    question_asked = current == 'WAITING_USER'
                streamed.append(text)
                if question_asked:
                    return True
                safe = redactor.push(text)
                return emit_update(safe) if safe else True
            tool_seen = []
            def on_tool(*args):
                nonlocal answer_pending, question_asked
                tool_seen.append(True)
                current = machine.get_state(task_id)
                start_early(current)
                answer_pending = current == 'ANSWER_READY'
                question_asked = question_asked or current == 'WAITING_USER'
                flush()
                if tool_emitter:
                    tool_emitter(*args)
            def on_display(update):
                # A context-fill update is not a message boundary: the
                # redactor keeps holding back its tail.
                if update.get('sessionUpdate') != 'usage_update':
                    flush()
                if emit_display_update:
                    return emit_display_update(update)
            with runtime._store.connection() as db:
                db.execute('BEGIN IMMEDIATE')
                event(db, task_id, 'COORDINATOR_TURN_STARTED',
                      dict(revision=context['revision'], state=state, engine_session_id=session_ref))
                db.commit()
            try:
                # No clock bound: a silent engine is woken by the engine
                # adapter's stall nudge, never killed (see live_adapters.prompt).
                outcome = runtime._engine.prompt(
                    session_ref, message, cancel_event, on_delta=on_delta,
                    on_tool_call=on_tool, on_display_update=on_display,
                    **({'prompt_blocks': [dict(type='text',text=message),*user_text.bound_prompt_blocks(runtime._store,task_id,prompt_blocks)]} if prompt_blocks and runtime._engine_accepts_prompt_blocks else {}),
                )
                from .. import _fault_injection as fault_injection
                fault_injection.checkpoint('t023_after_continuation_prompt')
                # G6: pairs with COORDINATOR_TURN_STARTED above -- this
                # prompt() call returned normally.
                with runtime._store.connection() as db:
                    db.execute('BEGIN IMMEDIATE')
                    event(db, task_id, 'COORDINATOR_TURN_ENDED',
                          dict(engine_session_id=session_ref, stop_reason=outcome.stop_reason.value))
                    db.commit()
            except Exception as error:
                # The prompt raised (or was cancelled by raising): its delivery
                # is unconfirmed, so the claimed events (worker results, user
                # messages) are released for the next prompt instead of being
                # lost for the rest of this process. At-least-once, as
                # mark_delivered documents.
                events.release_claim(runtime._store, task_id, delivery_marker)
                # G6: pairs with COORDINATOR_TURN_STARTED above -- this
                # prompt() call raised instead of returning.
                with runtime._store.connection() as db:
                    db.execute('BEGIN IMMEDIATE')
                    event(db, task_id, 'COORDINATOR_TURN_ENDED',
                          dict(engine_session_id=session_ref, failure_kind=type(error).__name__))
                    db.commit()
                flush()
                if tool_emitter:
                    tool_emitter.close_group()
                runtime._finalize_generation_delivery(session_ref, delivery_id,
                                                     failure_kind=type(error).__name__, stop_reason=None)
                if cancel_event.is_set() or _transport_lost(runtime, task_id):
                    return _stop_turn(runtime, task_id, machine, emit_tool_call, emit_update, language)
                else:
                    kind = getattr(error, 'kind', None)
                    dead = kind in _COORDINATOR_SESSION_DEAD_KINDS
                    if dead:
                        # Drop the dead coordinator session so the next user
                        # message reopens a fresh one via recovery, instead of
                        # reusing the corpse and re-raising this forever.
                        runtime._evict_coordinator_session(task_id, kind)
                    # A turn that failed for an engine/transport reason
                    # before it streamed or called anything did nothing with
                    # this prompt: resend it (on a reopened session if this
                    # one died), announced in the chat, with the bounded
                    # retry window, so the user's message simply runs
                    # instead of a question.
                    if (not streamed and not tool_seen and _transient_turn_failure(kind, dead)
                            and machine.context(task_id)['state'] == state):
                        from ..engine import retry_policy
                        turn_failures.append(time.monotonic())
                        decision = retry_policy.windowed_retry(
                            turn_failures, time.monotonic(), runtime._engine_retry_window)
                        if not decision.may_retry:
                            runtime._emit_retry_notice(
                                task_id, emit_update, language, 'engine_retry.gave_up', error=kind,
                                retries=len(turn_failures) - 1,
                                minutes=max(1, round(decision.elapsed_seconds / 60)))
                        else:
                            runtime._emit_retry_notice(
                                task_id, emit_update, language, 'engine_retry.turn', error=kind,
                                retry=decision.retry_number, delay=round(decision.delay_seconds))
                            if cancel_event.wait(decision.delay_seconds):
                                return _stop_turn(runtime, task_id, machine, emit_tool_call, emit_update, language)
                            try:
                                if dead:
                                    channel, session_ref = runtime._ensure_coordinator_retrying(
                                        task_id, emit_update, cancel_event, language)
                            except Exception:
                                pass
                            else:
                                prompted[key] -= 1
                                continue
                    machine.move(task_id, 'BLOCKED', blocker=str(error) if isinstance(error,StoreError) else ('engine_failure:'+kind if kind else f'coordinator_prompt_failed:{type(error).__name__}'))
                continue
            if pending_events:
                events.mark_delivered(runtime._store,task_id,delivery_marker)
            runtime._finalize_generation_delivery(session_ref, delivery_id, failure_kind=None, stop_reason=outcome.stop_reason)
            flush()
            if tool_emitter:
                # Turn end: the aggregate tool-call card gets its final count.
                tool_emitter.close_group()
            prompt_blocks = None
            if (outcome.text and not streamed and not question_asked
                    and machine.get_state(task_id) not in {'ANSWER_READY', 'WAITING_USER'}):
                emit_update(outcome.text)
            if outcome.stop_reason == StopReason.CANCELLED:
                return _stop_turn(runtime, task_id, machine, emit_tool_call, emit_update, language)
            if early_start:
                early_start[0].join()
            if outcome.stop_reason != StopReason.END_TURN and machine.get_state(task_id) != 'EXECUTING':
                failure_stop_reason = outcome.stop_reason
                machine.move(task_id,'BLOCKED',blocker='coordinator_turn_failed:'+outcome.stop_reason.value)
            elif state == 'RECONCILING' and machine.context(task_id)['synthesis']:
                if apply_pending_steering(runtime._store, task_id):
                    prompted.clear()
                    continue
                with runtime._lock:
                    active = bool(runtime._running.get(task_id))
                if active:
                    machine.move(task_id, 'BLOCKED', blocker='workers_still_running')
                else:
                    try:
                        with runtime._store.connection() as db:
                            workers = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'", (task_id,)).fetchall()
                        if any(runtime._engine.lookup_owned(row['attempt_id']) != 'absent' for row in workers):
                            raise StoreError('workers_still_live_or_unknown')
                        machine.move(task_id, 'COMPLETED')
                    except StoreError as error:
                        machine.move(task_id, 'BLOCKED', blocker=str(error))
            continue
        if state == 'PLAN_READY':
            with _execution_lock(task_id):
                if machine.get_state(task_id) == 'PLAN_READY':
                    machine.move(task_id, 'EXECUTING')
            continue
        if state in {'EXECUTING', 'REVIEWING'}:
            # The context already carries active waits. Keep the ordinary
            # idle path free of question delivery and access reconciliation.
            question_delivery_active = False
            if any(n.get('wait') for n in context['nodes']):
                question_delivery_active = runtime._service_worker_questions(
                    task_id, session_id, emit_update, language, host_elicitor)
            runtime._terminate_completed_workers(task_id)
            if any(n['status'] == 'pending' for n in context['nodes']):
                # A worker slot whose engine is gone must not keep the rest
                # of the plan waiting.
                reclaim = getattr(runtime, '_reclaim_stale_worker_capacity', None)
                if reclaim is not None:
                    reclaim(task_id)
            # Start only dependency-ready nodes reserved by the runtime
            # scheduler, at most MAX_WORKERS_PER_SESSION at once.
            with _execution_lock(task_id):
                _dispatch_ready(runtime, machine, task_id, channel, context, emit_tool_call)
            # A step that cannot start is shown, with what holds it, instead
            # of a silent chat; the card closes when the wait ends.
            if wait_cards or any(n['status'] == 'pending' for n in context['nodes']):
                from .wait_notices import show as show_waits
                show_waits(runtime._store, task_id, emit_tool_call, language, wait_cards)
            runtime._process_new_events_safely(task_id, channel, cancel_event, emit_tool_call, phase="scheduler")
            # Project a batch once to lifecycle cards. This is presentation, not decision input.
            marker = f'runtime-controller:{time.monotonic_ns()}'
            # Keep queued user messages unclaimed until the coordinator's
            # next planning turn; the scheduler consumes only lifecycle rows.
            batch = events.claim_unconsumed(
                runtime._store, task_id, marker, exclude_types=("user_message",),
            ) if _unconsumed_lifecycle(runtime._store, task_id) else []
            runtime._emit_assignment_tool_calls(task_id, emit_tool_call, batch)
            if batch:
                events.mark_delivered(runtime._store, task_id, marker)
            # Live workers first, then the nodes: a worker commits its result
            # before its future leaves _running, so a worker missing from this
            # set already shows its result below. Read the other way round, a
            # worker finishing in between looked lost to retry_lost.
            with runtime._lock:
                workers = frozenset(runtime._running.get(task_id, ()))
            context = machine.context(task_id)
            if state == 'REVIEWING' and context['state'] == state:
                speculation.refresh(context)
            if context['state']!=state: continue
            nodes = context['nodes']
            running = bool(workers)
            # A worker ends early when its turn is cancelled or its host
            # connection closes; that is not a lost worker to retry here.
            if cancel_event.is_set() or _transport_lost(runtime, task_id):
                speculation.close('task_stopped')
                return _stop_turn(runtime, task_id, machine, emit_tool_call, emit_update, language)
            open_waits = [n['wait'] for n in nodes
                          if n.get('wait') and n['wait']['status'] == 'open']
            if running and open_waits:
                waiting = {w['assignment_ref'] for w in open_waits}
                # A terminal failure event may precede the node's retry projection.
                # It is not active work that should hold a text-only question turn.
                with runtime._store.connection() as db:
                    failed_refs = {row[0] for row in db.execute(
                        "SELECT n.assignment_ref FROM runtime_nodes n "
                        "JOIN delegation_provisioning p ON p.idempotency_key=n.assignment_ref AND p.task_id=n.task_id "
                        "JOIN task_events e ON e.task_id=n.task_id AND e.assignment_ref IN (p.idempotency_key,p.attempt_id) "
                        "WHERE n.task_id=? AND n.revision=? AND e.event_type='assignment_failed' AND e.superseded=0",
                        (task_id, context['revision']),
                    )}
                live = [n for n in nodes if n['status'] == 'running' and n['assignment_ref'] not in failed_refs]
                with runtime._lock:
                    in_turn = bool(runtime._worker_turns_in_flight & waiting)
                    presented = all(w['question_id'] in runtime._worker_question_deliveries
                                    and runtime._worker_question_deliveries[w['question_id']].done()
                                    for w in open_waits)
                if (live and all(n['assignment_ref'] in waiting for n in live)
                        and presented and not question_delivery_active and not in_turn
                        and not any(w.get('kind') == 'access' and w.get('retry_at')
                                    for w in open_waits)):
                    # Text-only/declined/timed-out host: end only the host turn.
                    # Retained runners, sessions and reservations wait for an exact answer.
                    runtime._set_state(task_id, 'waiting_for_user', wait_reason='worker_question')
                    return StopReason.END_TURN
            if any(n['status'] == 'failed' for n in nodes):
                # Wait only for nodes still working; a finished node's session
                # teardown must never hold the task (it hung here once).
                if running and any(n['status'] in ('running', 'pending') for n in nodes):
                    # Let parallel workers finish; the coordinator decides
                    # with every result in hand.
                    _wait_for_change(runtime, machine, task_id, fingerprint, workers, cancel_event, language, relay_requests=True)
                    continue
                # A failed check (often unrelated, pre-existing) is the
                # coordinator's to handle, not a stop: it replans or notes it.
                if not machine.return_to_coordinator(task_id, 'worker_failed'):
                    machine.move(task_id, 'BLOCKED', blocker='worker_failed')
                continue
            # Results, not engine teardown, unlock independent review.
            # Lingering engines still own their capacity slots, so claim_ready
            # may queue the reviewer until a slot is positively released.
            if state == 'EXECUTING' and nodes and all(n['status'] == 'completed' for n in nodes):
                machine.add_review(task_id)
                capacity_wait_started = None
                continue
            if running:
                capacity_wait_started = None
                # A worker waiting on the user (a deletion, a write outside
                # the project) gets its permission card here.
                _wait_for_change(runtime, machine, task_id, fingerprint, workers, cancel_event, language, relay_requests=True)
                continue
            if any(n['status'] == 'running' for n in nodes):
                from .recovery import retry_lost
                try:
                    for node in nodes:
                        if node['status']=='running': retry_lost(runtime,task_id,node)
                except StoreError as error:
                    # Retries used up: the coordinator decides how to get the
                    # work done (another approach, another role, a smaller
                    # step) before the user is asked.
                    if not machine.return_to_coordinator(task_id, str(error)):
                        machine.move(task_id,'BLOCKED',blocker=str(error))
                continue
            if all(n['status'] == 'completed' for n in nodes):
                runtime._terminate_completed_workers(task_id)
                if state == 'EXECUTING':
                    machine.add_review(task_id)
                else:
                    review = [n for n in nodes if n['stage'] == 'review'][-1]
                    verdict = review['result']['verdict']
                    if verdict == 'APPROVE':
                        machine.move(task_id, 'VERIFYING')
                    elif verdict == 'REQUEST_CHANGES':
                        machine.add_fix(task_id, review)
                    else:
                        machine.move(task_id, 'BLOCKED', blocker='review_blocked')
                continue
            # A finished worker of this task may still hold an overlapping
            # scope until its engine is seen gone. Wait with a deadline.
            if any(n['status']=='pending' for n in nodes):
                from .wait_notices import waiting_on_live_holders
                if waiting_on_live_holders(runtime._store, task_id):
                    # A live worker (often of another chat) holds the paths:
                    # it releases them when it ends, so no deadline applies
                    # until it is gone. The chat shows this wait on its card.
                    capacity_wait_started = None
                    _wait_for_change(runtime, machine, task_id, fingerprint, workers, cancel_event, language, relay_requests=False)
                    continue
                capacity_wait_started = capacity_wait_started or time.monotonic()
                if time.monotonic()-capacity_wait_started < 1800:
                    _wait_for_change(runtime, machine, task_id, fingerprint, workers, cancel_event, language, relay_requests=False)
                    continue
                machine.move(task_id,'BLOCKED',blocker='scheduler_capacity_timeout')
            else:
                machine.move(task_id, 'BLOCKED', blocker='no_runnable_nodes')
            continue
        if state == 'VERIFYING':
            # An early unchanged-project decision can bypass REVIEWING while
            # result-bearing engines still wind down. Reap them before the
            # ordinary verification/synthesis path, just as after review.
            runtime._terminate_completed_workers(task_id)
            with runtime._lock:
                workers = frozenset(runtime._running.get(task_id, ()))
            if workers:
                _wait_for_change(runtime, machine, task_id, fingerprint, workers, cancel_event, language, relay_requests=True)
                continue
            credited = speculation.credit(context, machine)
            if credited is None:
                cancel_event.wait(_POLL_SECONDS)
                continue
            if not credited:
                _verification(runtime, task_id, machine, emit_tool_call, cancel_event)
            continue
        if state == 'WAITING_USER':
            for node in context['nodes']:
                if node['status']=='cancelled' and node['assignment_ref']:
                    runtime._cancel_assignment(task_id,node['assignment_ref'],'worker_question',emit_tool_call=emit_tool_call)
            from .questions import pending, resume_answered
            question = pending(runtime._store, task_id)
            if question.get('blocker'):
                question = _stop_question(question, context, language)
            elif question.get('skill_proposal'):
                from .skill_proposals import question_text
                question = question_text(question, language)
            if runtime._handle_ask_user(task_id, session_id, emit_update, language, question, host_elicitor):
                resumed = resume_answered(runtime._store, task_id)
                if isinstance(resumed, tuple) and resumed[0] == 'force_finish':
                    runtime.stop_task(task_id, session_id, emit_update, emit_tool_call=emit_tool_call,
                                      stop_source='user_answer_force_finish', question_id=resumed[1])
                    return StopReason.CANCELLED
                if resumed:
                    prompted.pop((question['resume_state'], context['revision']),None)
                    continue
            runtime._set_state(task_id, 'waiting_for_user', wait_reason=question.get('blocker') or 'user_question')
            return failure_stop_reason
        if state in {'COMPLETED', 'FAILED', 'BLOCKED', 'CANCELLED'}:
            if state == 'COMPLETED' and not runtime_synthesis_emitted and context['synthesis']:
                # Runtime-composed or the coordinator's synthesis_submit text
                # alike: the final answer belongs in the chat, not only in
                # pipeline.md behind a bare completion label.
                emit_update(_own_paragraph(context['synthesis']))
                runtime_synthesis_emitted = True
            runtime._set_state(task_id, 'accepted' if state == 'COMPLETED' else state.lower(),
                               wait_reason=context['blocker'])
            labels = {'COMPLETED':'Завершено','FAILED':'Ошибка выполнения','BLOCKED':'Выполнение заблокировано','CANCELLED':'Отменено'}
            emit_update('\n\n' + (labels[state] if language == 'ru' else f"Runtime: {state}") + (f" — {context['blocker']}" if context['blocker'] else ''))
            return StopReason.CANCELLED if state == 'CANCELLED' else failure_stop_reason
        machine.move(task_id, 'BLOCKED', blocker='recovery_required')


def _wait_for_change(runtime, machine, task_id, fingerprint, started, cancel_event, language, *, relay_requests):
    """Hold the scheduler until its full pass can observe something new.

    ``fingerprint`` was read before the pass and ``started`` is the worker set
    the pass saw. The in-memory notification only shortens the wait; the
    database stays the source of truth, and the deadline bounds any missed
    wake-up.
    """
    cond = getattr(runtime, '_cond', None)
    deadline = time.monotonic() + _RECONCILE_SECONDS
    if relay_requests:
        _relay_pending(runtime, task_id, language)
    while True:
        if cond is None:
            notified = cancel_event.wait(_POLL_SECONDS)
        else:
            # _on_worker_done and capacity releases notify this condition.
            with cond:
                notified = cond.wait(_POLL_SECONDS)
        with runtime._lock:
            # A worker that ended before the wait began is seen here.
            ended = frozenset(runtime._running.get(task_id, ())) != started
        if (notified or ended or cancel_event.is_set() or _transport_lost(runtime, task_id)
                or time.monotonic() >= deadline):
            return
        if relay_requests:
            _relay_pending(runtime, task_id, language)
        if machine.change_fingerprint(task_id) != fingerprint:
            return


def _relay_pending(runtime, task_id, language):
    """Show new permission requests; claim them only when some exist."""
    try:
        with runtime._store.connection() as db:
            pending = db.execute("SELECT 1 FROM consent_requests WHERE task_id=? AND status='pending' LIMIT 1",
                                 (task_id,)).fetchone()
    except sqlite3.OperationalError as error:
        # The table is created with the first request; anything else: claim.
        pending = 'no such table' not in str(error)
    if pending:
        from .consent import relay
        relay(runtime, task_id, language)


def _unconsumed_lifecycle(store, task_id):
    """Whether events.claim_unconsumed would claim anything for the scheduler."""
    with store.connection() as db:
        return db.execute("SELECT 1 FROM task_events WHERE task_id=? AND consumed_by IS NULL AND superseded=0 "
                          "AND event_type<>'user_message' LIMIT 1", (task_id,)).fetchone() is not None


def _stop_question(question, context, language):
    """Render a runtime stop question in the turn language, with the evidence."""
    from ..acp import messages
    code, _, detail = question['blocker'].partition(':')
    try:
        # The code stays visible for diagnosis next to the plain reason.
        reason = messages.render('stop_reason.' + code, language, detail=detail or '').replace(' ()', '') + f" [{question['blocker']}]"
    except KeyError:
        reason = question['blocker']
    details = ''
    if code == 'worker_failed':
        # The retained report is still the user's deliverable.
        for node in context['nodes']:
            result = node['status'] == 'failed' and node['result']
            if not result:
                continue
            unpassed = ', '.join(
                f"{c['name']}: {c['outcome']}" + (f" ({c['blocker']})" if c.get('blocker') else '')
                # A failure an exact later rerun closed is history, not a current failure.
                for c in result.get('checks', []) if c.get('outcome') != 'passed' and not c.get('closed_by') and c.get('current_outcome') != 'retracted'
            )
            details += messages.render('worker_failed_details', language, node=node['node_id'], report_ref=result['report_ref'],
                                       summary=result['summary'],
                                       unpassed=f"\n{messages.render('unpassed_checks_label', language)}: {unpassed}" if unpassed else '')
    if code == 'verification_failed':
        # Exactly what failed and what it printed: a bare code left the user
        # with nothing to decide on.
        for check in (context.get('verification') or {}).get('checks') or ():
            if check.get('exit_code') == 0 and check.get('complete'):
                continue
            command = ' '.join(check.get('argv') or ())
            tail = (check.get('output_tail') or '').strip()[-1500:]
            details += f"\n\n`{command}` → exit {check.get('exit_code')}" + (f"\n```\n{tail}\n```" if tail else '')
        attempts = context.get('verification_attempts')
        if attempts:
            details += ('\n\nКоординатор пытался исправить проверку: ' if language == 'ru'
                        else '\n\nThe coordinator tried to repair it: ') + str(attempts)
    return dict(question, question=messages.render('stop_question', language, reason=reason, details=details),
                options=[messages.render('stop_option_continue', language), messages.render('stop_option_finish', language)])


def _ask_user_request(context):
    if not context.get('ask_user_required'):
        return ''
    return (f"Automatic handling is exhausted ({context['ask_user_required']}). Ask the user NOW with "
            "user_question_request, in the user's language: say plainly what failed and why (use the verification "
            "output, failed checks and reports in context), what was already tried, and offer 2-4 concrete next "
            "actions as options; the user may also answer in their own words. Do not submit a plan before the answer. ")


def _skill_proposal_request(context):
    from .skill_proposals import planning_instruction
    return planning_instruction(context)


def _failed_worker_request(context):
    if not any(n.get('status') == 'failed' and n.get('result') for n in context.get('nodes') or ()):
        if any(n.get('status') in ('running', 'cancelled') for n in context.get('nodes') or ()):
            return ('A worker could not finish its assignment after several attempts (it ended without a result or '
                    'failed to start). Get the work done another way: split it, change the approach or role, and '
                    'submit a new plan carrying over completed work by report id. ')
        return ''
    return ('A worker finished but some of its checks FAILED (see nodes[].result.checks and its report). Decide yourself: '
            'submit a new plan that fixes what is needed, carrying over the completed work by report id, or -- when the '
            'failure is unrelated to this task or pre-existing -- submit a plan for only what remains and state the gap. '
            'Do not stop the task for it. ')


def _verification_repair_request(context):
    verification = context.get('verification') or {}
    if not context.get('nodes') or 'checks' not in verification or all(
            c.get('exit_code') == 0 and c.get('complete') for c in verification['checks']):
        return ''
    return ('The runtime verification FAILED; context.verification has each command, exit code and output tail. '
            'Find how to run the check correctly in this project (read its docs, Makefile, package.json, its virtualenvs; '
            'a missing interpreter or tool is a wrong command, not a reason to stop) and submit verification_submit with '
            'corrected commands. If the code itself is wrong, submit a new plan that fixes it. ')


def _direct_answer_request(runtime, task_id):
    """Offer answer_submit only when the runtime will accept it."""
    from .state import direct_answer_allowed, reopened_after_completion, steered_after_verification
    with runtime._store.connection() as db:
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        allowed = bool(task) and direct_answer_allowed(db, task)
        reopened = allowed and reopened_after_completion(db, task)
        steered = allowed and not reopened and steered_after_verification(db, task)
    if not allowed:
        return ('answer_submit is not available now: work planned in this task must finish through the plan, '
                'independent review and runtime verification; synthesis_submit is accepted only after that '
                'verification, never while planning. Submit a plan or verification repair, '
                'or ask the user with user_question_request. ')
    if steered:
        return ('The planned work of this revision was reviewed and verified, then a new user message (see steering '
                'and retained events) returned the task to planning before it completed; synthesis_submit is not '
                'accepted while planning. If the message asks for changes or more work, submit a plan: it becomes '
                'a new revision with its own review and verification. Otherwise submit answer_submit with the final '
                'summary grounded in the retained reports and verification, also answering the message; the runtime '
                'completes the task only if the reviewed and verified work still passes every completion gate. '
                'If the user must decide something, ask with user_question_request. ')
    return (('This task was completed and the user\'s new message reopened it. ' if reopened else '') +
            'First determine whether the user wants information or changes. For a simple question or project overview, '
            'read only the relevant files yourself and submit a concise direct answer with answer_submit; do not create '
            'workers, reports, code review or verification jobs for an explanation. ')


def _released_request(runtime, task_id):
    from .state import released_by_user
    with runtime._store.connection() as db:
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        released = bool(task) and released_by_user(db, task)
    return ('The runtime stopped this task and the user answered its stop question in their own words (see retained events). '
            'If the user chose to finish or stop, submit answer_submit with a final summary grounded in the retained reports, '
            'stating plainly which checks did not pass; the runtime then closes the task without certifying work whose '
            'review or verification did not pass. Otherwise submit a new plan or ask the user. ') if released else ''


def _unknown_outcome_request(runtime, task_id):
    from ..recovery.worker_reconciliation import unknown_outcome_request
    with runtime._store.connection() as db:
        return unknown_outcome_request(db, task_id)


def _transport_lost(runtime, task_id):
    lost = getattr(runtime, 'transport_lost', None)
    return bool(lost is not None and lost(task_id))


def _stop_turn(runtime, task_id, machine, emit_tool_call, emit_update=None, language='en'):
    """End an interrupted turn. Only a user's cancel pauses the task.

    A closed host connection (a plugin update restarts the provider) or the
    daemon teardown after it is transport loss: task, nodes and workers stay
    as persisted, and the restarted runtime reconciles them (coordinator
    recovery marks lost workers; retry_lost never repeats an action whose
    outcome was not retained).
    """
    if not _transport_lost(runtime, task_id):
        from ..acp import messages
        from ..recovery.worker_reconciliation import unknown_outcome_actions
        with runtime._store.connection() as db:
            fenced = len(unknown_outcome_actions(db, task_id, revoked_only=True))
        paused = machine.get_state(task_id) not in {'COMPLETED', 'FAILED', 'CANCELLED'}
        _pause_turn(runtime, task_id, machine, emit_tool_call)
        if paused and emit_update is not None:
            # Say what the stop did now, not after a later reload: the task is
            # paused, and how many worker actions it cut off mid-way.
            with runtime._store.connection() as db:
                cut = len(unknown_outcome_actions(db, task_id, revoked_only=True)) - fenced
            text = messages.render('turn_paused', language)
            if cut > 0:
                text += messages.render('turn_paused_unknown_actions', language, count=cut)
            emit_update('\n\n' + text)
    return StopReason.CANCELLED


def _pause_turn(runtime,task_id,machine,emit_tool_call):
    from .policy import interrupt
    context=machine.context(task_id)
    if context['state'] in {'COMPLETED','FAILED','CANCELLED'}: return
    interrupt(runtime._store, task_id)
    for node in context['nodes']:
        if node['assignment_ref'] and node['status']=='running':
            runtime._cancel_assignment(task_id,node['assignment_ref'],'turn_interrupted',emit_tool_call=emit_tool_call)

    # The engine already cancelled the prompt itself (the cancel event is
    # the engine's session/cancel); keep its process and context, as after
    # any turn, so the next message continues the same session. A task stop
    # (stop_task) is what releases the coordinator.
    runtime._set_state(task_id,'recovering',wait_reason='turn_interrupted')
