"""Deterministic task driver over ACP engine sessions and runtime services."""
from __future__ import annotations

import contextlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time

from .state import TaskMachine, encoded, event, now, transition
from ..contracts import StoreError
from ..delegation import events
from ..delegation import revision
from ..portable.engine_contract import StopReason
from ..observability.redact import StreamingRedactor

# Coordinator engine failure kinds that mean the session can never be prompted
# again (the process/transport is gone), as distinct from a slow-but-alive
# engine. See engine.failure_classification._ENGINE_FAILURE_MARKERS;
# 'timeout_alive' is deliberately excluded -- it never applies to the unbounded
# coordinator prompt turn, and the engine is still alive when it is raised.
_COORDINATOR_SESSION_DEAD_KINDS = frozenset({
    'transport_eof', 'poisoned', 'process_exit', 'process_crashed', 'nonzero_exit',
})


def _verification(runtime, task_id, machine, emit_tool_call, cancel):
    """Run retained literal argv commands, with bounded time and complete output.

    The root filesystem is read-only; checks can write only in
    their private temporary directory. The real runtime store is inaccessible.
    Missing OS isolation is a failure, never an unsandboxed fallback.
    """
    context = machine.context(task_id)
    root = Path(runtime._task_project_root(task_id))
    before = revision.capture_revision(str(root))
    digest = before.get('dirty_digest') or before.get('digest')
    if not digest:
        machine.move(task_id, 'BLOCKED', blocker='verification_revision_unavailable')
        return
    reviews = [node for node in context['nodes'] if node['stage'] == 'review']
    if not reviews or not reviews[-1]['result'] or reviews[-1]['result'].get('project_digest') != digest:
        machine.move(task_id, 'BLOCKED', blocker='verification_review_revision_changed')
        return
    checks = []
    for index, argv in enumerate(context['plan']['verification']):
        ref = f"verification:{context['revision']}:{index}"
        if emit_tool_call:
            emit_tool_call(dict(tool_call_id=ref, new=True, title='Verify project', kind='execute',
                                status='in_progress', raw_input=dict(argv=argv, cwd=str(root))))
        with tempfile.TemporaryDirectory(prefix='cortex-verification-') as scratch, contextlib.ExitStack() as stack:
            # The owner's toolchain (npm, node in their home), as a worker's
            # command_run has it: a bare /usr/bin sandbox made every npm check
            # fail. Only directories the project ignores (caches, builds,
            # node_modules) are writable, so sources stay untouched and the
            # revision (which skips ignored paths) cannot drift. No access to
            # the real private store, even from an interpreter.
            from .sandbox import as_owner, gateway_command
            try:
                scopes = [path + '/**' for path in revision.ignored_directories(str(root))]
                command, fds = stack.enter_context(gateway_command(root, scratch, argv, scopes, system_root=True))
                as_owner(command, os.environ)
            except StoreError as error:
                machine.move(task_id, 'BLOCKED', blocker=str(error))
                return
            with tempfile.TemporaryFile() as output:
                from .sandbox import compact_argv
                launch, launch_fds = compact_argv(command, fds)
                try:
                    proc = subprocess.Popen(launch, stdout=output, stderr=subprocess.STDOUT, pass_fds=launch_fds,
                                            env={'PATH': '/usr/bin:/bin'})
                finally:
                    for extra in set(launch_fds) - set(fds):
                        os.close(extra)
                deadline = time.monotonic() + 300
                interrupted = False
                while proc.poll() is None:
                    if cancel.wait(.1) or time.monotonic() >= deadline:
                        interrupted = True
                        proc.kill()
                        break
                code = proc.wait()
                output.seek(0)
                data = output.read()
                body = data.decode('utf-8', errors='replace')
        after = revision.capture_revision(str(root))
        unchanged = (after.get('dirty_digest') or after.get('digest')) == digest
        from ..recovery.large_output_store import persist_output
        saved=persist_output(runtime._store,task_id=task_id,owner_scope='coordinator',source_kind='terminal',
                             source_ref=ref,data=data,exit_code=code,
                             failure='interrupted' if interrupted else None)
        observed = dict(argv=argv,exit_code=code,complete=not interrupted and unchanged,
                        revision_digest=digest,large_output_id=saved['id'],
                        # What the coordinator needs to repair a failing check.
                        output_tail=body[-3000:] if code != 0 or interrupted or not unchanged else None)
        checks.append(observed)
        if emit_tool_call:
            emit_tool_call(dict(tool_call_id=ref, new=False, status='completed' if code == 0 and observed['complete'] else 'failed',
                                raw_output=dict(observed,output=body)))
        if interrupted or code != 0 or not unchanged:
            break
    machine.record_verification(task_id, checks, digest)


def run(runtime, task_id, session_id, channel, session_ref, content, emit_update,
        cancel_event, language, host_elicitor=None, *, emit_tool_call=None,
        emit_display_update=None, prompt_blocks=None, initial_project_docs_context=None):
    machine = TaskMachine(runtime._store)
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
        resume_answered(runtime._store, task_id)
    if context['state'] == 'BLOCKED':
        # A task stopped before stops became questions: ask now.
        machine.move(task_id, 'BLOCKED', blocker=context['blocker'])
    prompted = {}
    published_states = set()
    capacity_wait_started = None
    failure_stop_reason = StopReason.END_TURN
    reopened = False
    while True:
        if cancel_event.is_set():
            _pause_turn(runtime,task_id,machine,emit_tool_call)
            return StopReason.CANCELLED
        context = machine.context(task_id)
        state = context['state']
        if state == 'ANSWER_READY':
            machine.move(task_id,'COMPLETED')
            from .pipeline import publish
            publish(runtime._store,channel,machine.context(task_id))
            runtime._set_state(task_id,'accepted',wait_reason=None)
            emit_update(context['synthesis'])
            return StopReason.END_TURN
        if state == 'RECOVERING':
            # Revoke first (policy.steer), terminate second, then accept new plans.
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
            context=machine.context(task_id)
            continue
        publication_key = (context['revision'], context['review_cycle'], state)
        if publication_key not in published_states:
            from .pipeline import publish
            publish(runtime._store, channel, context)
            published_states.add(publication_key)
        if state in {'PLANNING', 'RECONCILING'}:
            key = (state, context['revision'])
            if prompted.get(key,0)>=3:
                machine.move(task_id, 'BLOCKED', blocker='structured_proposal_missing')
                continue
            prompted[key]=prompted.get(key,0)+1
            request = ('First determine whether the user wants information or changes. For a simple question or project overview, '
                       'read only the relevant files yourself and submit a concise direct answer; do not create workers, reports, '
                       'code review or verification jobs for an explanation. Explaining how to run tests does not require executing them. '
                       'The coordinator can inspect files but cannot execute commands. Do not expand a description into a defect audit. '
                       'If changes or substantial delegated work are required, submit a focused structured plan. '
                       'Runtime schedules planned workers and mandatory review. '
                       + _released_request(runtime, task_id)
                       + (_ask_user_request(context) or _verification_repair_request(context)
                          or _failed_worker_request(context))
                       if state == 'PLANNING' else
                       'Reconcile the retained results and runtime verification, then submit synthesis. State plainly every not_run or unverified '
                       'worker check as a gap the user should know about. Runtime decides completion. ')
            delivery_marker=f'runtime-coordinator:{time.monotonic_ns()}'
            pending_events=events.claim_unconsumed(runtime._store,task_id,delivery_marker)
            from ..acp import messages as localized_messages
            message = localized_messages.render('answer_in_language',language)+'\n'+request + '\n' + encoded(context)
            if pending_events:
                message += '\nRetained events:\n'+encoded([dict(type=e.event_type,payload=e.payload) for e in pending_events])
            if state == 'PLANNING':
                if not prompt_blocks:
                    message += '\nCurrent user input:\n' + content
                if initial_project_docs_context:
                    message += '\n' + initial_project_docs_context
            # Reassert the selected turn language after English report/context
            # blocks. Worker text is evidence, never the coordinator's locale.
            message += '\n\n' + localized_messages.render('answer_in_language',language)
            message, delivery_id, delivery_blocked = runtime._begin_generation_delivery(
                task_id, session_ref, 'orchestrator', message, emit_update=emit_update, language=language)
            if delivery_blocked:
                events.release_claim(runtime._store,task_id,delivery_marker)
                with runtime._store.connection() as db:
                    delivery_state=db.execute('SELECT state,wait_reason FROM task_orchestration WHERE task_id=?',(task_id,)).fetchone()
                if delivery_state and delivery_state['state']=='blocked':
                    machine.move(task_id,'BLOCKED',blocker=delivery_state['wait_reason'] or 'context_delivery_blocked')
                return StopReason.END_TURN
            streamed = []
            answer_pending = False
            redactor = StreamingRedactor(max_chars=4000)
            tool_emitter = runtime._engine_tool_call_emitter(emit_tool_call, id_scope=f'coordinator:{session_ref}',
                                                           task_id=task_id, cancel_event=cancel_event)
            def flush():
                tail = redactor.flush()
                if tail:
                    emit_update(tail)
            def on_delta(text):
                nonlocal answer_pending
                # Some adapters deliver text before the tool lifecycle callback.
                # The committed proposal is the authority for suppressing its echo.
                answer_pending = answer_pending or machine.context(task_id)['state'] == 'ANSWER_READY'
                if answer_pending: return True
                streamed.append(text)
                safe = redactor.push(text)
                return emit_update(safe) if safe else True
            tool_seen = []
            def on_tool(*args):
                nonlocal answer_pending
                tool_seen.append(True)
                answer_pending = machine.context(task_id)['state'] == 'ANSWER_READY'
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
            try:
                # No clock bound: a silent engine is woken by the engine
                # adapter's stall nudge, never killed (see live_adapters.prompt).
                outcome = runtime._engine.prompt(
                    session_ref, message, cancel_event, on_delta=on_delta,
                    on_tool_call=on_tool, on_display_update=on_display,
                    **({'prompt_blocks': [dict(type='text',text=message),*prompt_blocks]} if prompt_blocks and runtime._engine_accepts_prompt_blocks else {}),
                )
                from .. import _fault_injection as fault_injection
                fault_injection.checkpoint('t023_after_continuation_prompt')
            except Exception as error:
                flush()
                runtime._finalize_generation_delivery(session_ref, delivery_id,
                                                     failure_kind=type(error).__name__, stop_reason=None)
                if cancel_event.is_set():
                    _pause_turn(runtime,task_id,machine,emit_tool_call)
                    return StopReason.CANCELLED
                else:
                    kind = getattr(error, 'kind', None)
                    if kind in _COORDINATOR_SESSION_DEAD_KINDS:
                        # Drop the dead coordinator session so the next user
                        # message reopens a fresh one via recovery, instead of
                        # reusing the corpse and re-raising this forever.
                        runtime._evict_coordinator_session(task_id, kind)
                        # A session that died idle between turns did nothing
                        # with this prompt: reopen once and resend it, so the
                        # user's message simply runs instead of a question.
                        if (not reopened and not streamed and not tool_seen
                                and machine.context(task_id)['state'] == state):
                            reopened = True
                            try:
                                channel, session_ref = runtime._ensure_coordinator(task_id)
                            except Exception:
                                pass
                            else:
                                events.release_claim(runtime._store, task_id, delivery_marker)
                                prompted[key] -= 1
                                continue
                    machine.move(task_id, 'BLOCKED', blocker=str(error) if isinstance(error,StoreError) else ('engine_failure:'+kind if kind else f'coordinator_prompt_failed:{type(error).__name__}'))
                continue
            if pending_events:
                events.mark_delivered(runtime._store,task_id,delivery_marker)
            runtime._finalize_generation_delivery(session_ref, delivery_id, failure_kind=None, stop_reason=outcome.stop_reason)
            flush()
            prompt_blocks = None
            if outcome.text and not streamed and machine.context(task_id)['state'] != 'ANSWER_READY':
                emit_update(outcome.text)
            if outcome.stop_reason == StopReason.CANCELLED:
                _pause_turn(runtime,task_id,machine,emit_tool_call)
                return StopReason.CANCELLED
            if outcome.stop_reason != StopReason.END_TURN:
                failure_stop_reason = outcome.stop_reason
                machine.move(task_id,'BLOCKED',blocker='coordinator_turn_failed:'+outcome.stop_reason.value)
            elif state == 'RECONCILING' and machine.context(task_id)['synthesis']:
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
            machine.move(task_id, 'EXECUTING')
            continue
        if state in {'EXECUTING', 'REVIEWING'}:
            runtime._terminate_completed_workers(task_id)
            # Start only dependency-ready nodes reserved by the runtime scheduler.
            for node in machine.claim_ready(task_id, channel.channel_id):
                dependencies = set(json.loads(node['dependencies_json']))
                inputs = [dict(node=n['node_id'], report_ref=n['result']['report_ref'])
                          for n in context['nodes'] if n['node_id'] in dependencies and n['result']]
                review_evidence = []
                if node['stage'] == 'review':
                    from .artifacts import project_git_evidence
                    try:
                        machine.start_review(task_id, node['assignment_ref'])
                    except StoreError as error:
                        # Never a crashed turn: ask the user with the reason.
                        machine.move(task_id, 'BLOCKED', blocker=str(error))
                        break
                    for kind in ('status', 'diff'):
                        ref = f"review:{node['assignment_ref']}:{kind}"
                        if emit_tool_call:
                            emit_tool_call(dict(tool_call_id=ref,new=True,title='Read project '+kind,kind='read',status='in_progress',raw_input=dict(kind=kind)))
                        observed = project_git_evidence(runtime._store,task_id,node['assignment_ref'],runtime._task_project_root(task_id),kind)
                        review_evidence.append(observed)
                        if emit_tool_call:
                            emit_tool_call(dict(tool_call_id=ref,new=False,status='completed' if observed['available'] else 'failed',raw_output=observed))
                brief = encoded(dict(review_evidence=review_evidence,objective=node['objective'], scope=json.loads(node['scope_json']),
                                     dependency_reports=inputs, original_goal=context['goal']))
                from .scheduler import dispatch_assignment
                dispatch_assignment(runtime._store, channel, dict(
                    title=node['objective'][:200], brief=brief, role=node['role'],
                    acceptance='Publish a truthful immutable report and structured result. '
                               + ('Include a review verdict.' if node['stage'] == 'review' else 'Complete the assigned objective.'),
                    request_key=node['request_key']))
            runtime._process_new_events_safely(task_id, channel, cancel_event, emit_tool_call, phase="scheduler")
            # Project a batch once to lifecycle cards. This is presentation, not decision input.
            marker = f'runtime-controller:{time.monotonic_ns()}'
            batch = events.claim_unconsumed(runtime._store, task_id, marker)
            runtime._emit_assignment_tool_calls(task_id, emit_tool_call, batch)
            if batch:
                events.mark_delivered(runtime._store, task_id, marker)
            context = machine.context(task_id)
            if context['state']!=state: continue
            nodes = context['nodes']
            with runtime._lock:
                running = bool(runtime._running.get(task_id))
            if any(n['status'] == 'failed' for n in nodes):
                # Wait only for nodes still working; a finished node's session
                # teardown must never hold the task (it hung here once).
                if running and any(n['status'] in ('running', 'pending') for n in nodes):
                    # Let parallel workers finish; the coordinator decides
                    # with every result in hand.
                    from .consent import relay
                    relay(runtime, task_id, language)
                    cancel_event.wait(.1)
                    continue
                # A failed check (often unrelated, pre-existing) is the
                # coordinator's to handle, not a stop: it replans or notes it.
                if not machine.return_to_coordinator(task_id, 'worker_failed'):
                    machine.move(task_id, 'BLOCKED', blocker='worker_failed')
                continue
            if running:
                capacity_wait_started = None
                # A worker waiting on the user (a deletion, a write outside
                # the project) gets its permission card here.
                from .consent import relay
                relay(runtime, task_id, language)
                cancel_event.wait(.1)
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
                capacity_wait_started = capacity_wait_started or time.monotonic()
                if time.monotonic()-capacity_wait_started < 1800:
                    cancel_event.wait(.1)
                    continue
                machine.move(task_id,'BLOCKED',blocker='scheduler_capacity_timeout')
            else:
                machine.move(task_id, 'BLOCKED', blocker='no_runnable_nodes')
            continue
        if state == 'VERIFYING':
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
            if runtime._handle_ask_user(task_id, session_id, emit_update, language, question, host_elicitor):
                if resume_answered(runtime._store, task_id):
                    prompted.pop((question['resume_state'], context['revision']),None)
                    continue
            runtime._set_state(task_id, 'waiting_for_user', wait_reason=question.get('blocker') or 'user_question')
            return failure_stop_reason
        if state in {'COMPLETED', 'FAILED', 'BLOCKED', 'CANCELLED'}:
            runtime._set_state(task_id, 'accepted' if state == 'COMPLETED' else state.lower(),
                               wait_reason=context['blocker'])
            labels = {'COMPLETED':'Завершено','FAILED':'Ошибка выполнения','BLOCKED':'Выполнение заблокировано','CANCELLED':'Отменено'}
            emit_update('\n\n' + (labels[state] if language == 'ru' else f"Runtime: {state}") + (f" — {context['blocker']}" if context['blocker'] else ''))
            return StopReason.CANCELLED if state == 'CANCELLED' else failure_stop_reason
        machine.move(task_id, 'BLOCKED', blocker='recovery_required')


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
            unpassed = ', '.join(f"{c['name']}: {c['outcome']}" for c in result.get('checks', []) if c.get('outcome') != 'passed')
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


def _released_request(runtime, task_id):
    from .state import released_by_user
    with runtime._store.connection() as db:
        task = db.execute('SELECT * FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        released = bool(task) and released_by_user(db, task)
    return ('The runtime stopped this task and the user answered its stop question (see retained events). '
            'If the user chose to finish or stop, submit answer_submit with a final summary grounded in the retained reports, '
            'stating plainly which checks did not pass. Otherwise submit a new plan or ask the user. ') if released else ''


def _pause_turn(runtime,task_id,machine,emit_tool_call):
    from .policy import steer
    context=machine.context(task_id)
    if context['state'] in {'COMPLETED','FAILED','CANCELLED'}: return
    steer(runtime._store,task_id,'Turn interrupted; reconcile observed work before continuing.',
          'cancel:'+str(time.monotonic_ns()))
    for node in context['nodes']:
        if node['assignment_ref'] and node['status']=='running':
            runtime._cancel_assignment(task_id,node['assignment_ref'],'turn_interrupted',emit_tool_call=emit_tool_call)

    # The engine already cancelled the prompt itself (the cancel event is
    # the engine's session/cancel); keep its process and context, as after
    # any turn, so the next message continues the same session. A task stop
    # (stop_task) is what releases the coordinator.
    runtime._set_state(task_id,'recovering',wait_reason='turn_interrupted')
