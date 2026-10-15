"""User steering that reaches the runtime after the work was verified.

A message sent while a worker, the review or the verification runs is applied
only at the next boundary: once the runtime verification passed, RECONCILING
-> RECOVERING -> PLANNING. These tests drive the real controller, MCP dispatch,
review and verification with fake engines and observe what the coordinator is
told and which submissions the runtime accepts at that boundary.
"""
import json
import threading
from pathlib import Path

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')

STEER = 'Thanks, carry on; nothing to change.'


def _plan(command='print("verified")'):
    return dict(tasks=[dict(id='N0', role='backend-dev', objective='Implement part 0', dependencies=[],
                            scope=['src/part0/**'], model='auto')],
                verification=[['python3', '-B', '-c', command]])


def _events(store, task_id, *types):
    with store.connection() as db:
        rows = db.execute('SELECT id,type,payload_json FROM runtime_events WHERE task_id=? ORDER BY id',
                          (task_id,)).fetchall()
    return [(r['id'], r['type'], json.loads(r['payload_json'])) for r in rows if not types or r['type'] in types]


def _drive(tmp_path, monkeypatch, after_steer, *, steer_at='execution', verification='print("verified")',
           runtime_synthesis=True, reconciling=None, follow_up=None, worker_checks=()):
    """Run one task through the controller; ``after_steer`` is the
    coordinator's behaviour in every PLANNING turn after the first plan."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.acp.steering import record_steering
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from orchestration_fakes import FakeSessionRuntime, FakeWorkspace, FakeConfigSnapshotSource

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='steered'), 'Implement part 0', 'owner')
    if not runtime_synthesis:
        monkeypatch.setattr('cortex_runtime.orchestration.synthesis.complete', lambda *args: None)
    seen = dict(planning=[], refusals=[], coordinator=[], roles=[])

    def steer(identity='steer-1', text=STEER):
        return record_steering(store, task.task_id, 'acp:test', identity, text)

    seen['steer'] = steer

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == 'coordinator':
                seen['coordinator'].append(channel)
                context = mcp_tools.dispatch(store, channel, 'task_get_context', {})
                if context['state'] == 'PLANNING' and not seen.get('planned'):
                    seen['planned'] = True
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=_plan(verification), request_key='plan-0'))
                elif context['state'] == 'PLANNING':
                    seen['planning'].append((message, context))
                    return after_steer(store, channel, context, seen) or PromptOutcome(StopReason.END_TURN, 'Turn ended')
                elif context['state'] == 'RECONCILING':
                    if reconciling:
                        reconciling(store, channel, context, seen)
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Reviewed and verified.'))
                return PromptOutcome(StopReason.END_TURN, 'Turn ended')
            with store.connection() as db:
                role = db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',
                                  (channel.attempt_id,)).fetchone()['role_id']
            seen['roles'].append(role)
            reviewer = role == 'worker-code-reviewer'
            if not reviewer:
                (tmp_path / 'src' / 'part0').mkdir(parents=True, exist_ok=True)
                (tmp_path / 'src' / 'part0' / f'{channel.attempt_id}.txt').write_text('implemented')
            if steer_at == ('review' if reviewer else 'execution') and not seen.get('steered'):
                seen['steered'] = True
                # The same message delivered twice is one requirement.
                seen['revisions'] = [steer(), steer()]
            draft = store.call('create_draft', dict(template='general', request_key='draft-' + channel.attempt_id),
                               channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
            path = Path(draft['draft_path'])
            path.write_bytes(path.read_bytes().split(b'\n\n', 1)[0] + b'\n\n# Report\n\nObserved result.\n')
            report = store.call('write_report', dict(draft_id=draft['draft_id'], title='Result', summary='Observed result',
                                                     author='worker', request_key='report-' + channel.attempt_id),
                                channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
            args = dict(report=report['report_id'], summary='Observed result',
                        checks=[] if reviewer else list(worker_checks), request_key='result')
            if reviewer:
                args['verdict'] = 'APPROVE'
            mcp_tools.dispatch(store, channel, 'result_submit', args)
            return PromptOutcome(StopReason.END_TURN, 'Worker turn ended')
        return behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    output = []
    try:
        result = runtime.start_prompt(task.task_id, 'incoming', 'Implement part 0', output.append,
                                      threading.Event(), emit_tool_call=lambda update: True)
        if follow_up:
            result = follow_up(runtime, task, output)
    finally:
        runtime.shutdown()
    seen['result'] = result
    seen['output'] = output
    return store, task, TaskMachine(store).context(task.task_id), seen


def _try(seen, store, channel, operation, args):
    from cortex_runtime.delegation import mcp_tools
    try:
        return mcp_tools.dispatch(store, channel, operation, args)
    except StoreError as error:
        seen['refusals'].append((operation, str(error), error.field, error.received, error.correction or ''))
        return None


def _misused_then_answered(store, channel, context, seen):
    # What the live coordinators did first: synthesis while planning.
    _try(seen, store, channel, 'synthesis_submit', dict(text='Reviewed and verified.'))
    _try(seen, store, channel, 'answer_submit', dict(text='Done: part 0 is implemented, reviewed and verified.',
                                                     request_key='close'))


def _assert_steering_boundary(store, task_id):
    """Steering waited for the runtime verification, then returned the task
    to planning in one transaction, exactly once."""
    events = _events(store, task_id)
    steering = [e for e in events if e[1] == 'USER_STEERING']
    applied = [e for e in events if e[1] == 'STEERING_APPLIED']
    passed = [e for e in events if e[1] == 'VERIFICATION_PASSED']
    assert len(steering) == 1 and len(applied) == 1
    assert passed and passed[0][0] < applied[0][0]
    changes = [(e[2]['previous'], e[2]['state']) for e in events if e[1] == 'TASK_STATE_CHANGED']
    index = changes.index(('VERIFYING', 'RECONCILING'))
    assert changes[index + 1:index + 3] == [('RECONCILING', 'RECOVERING'), ('RECOVERING', 'PLANNING')]


@pytest.mark.parametrize('steer_at', ['execution', 'review'])
def test_decision_free_steering_after_verification_closes_the_verified_work(tmp_path, monkeypatch, steer_at):
    """The live dead end: every node completed, review approved and the
    runtime verification passed, but the steer returned the task to planning
    where synthesis_submit and answer_submit were both refused, so three
    prompts ended in structured_proposal_missing (WAITING_USER)."""
    store, task, context, seen = _drive(tmp_path, monkeypatch, _misused_then_answered, steer_at=steer_at)
    _assert_steering_boundary(store, task.task_id)
    assert seen['revisions'][0] == seen['revisions'][1]
    [(message, planning)] = seen['planning']
    assert planning['revision'] == 1 and {n['status'] for n in planning['nodes']} == {'completed'}
    # The coordinator is told what is valid in this phase.
    assert 'answer_submit is not available now' not in message
    assert 'synthesis_submit is not accepted while planning' in message
    assert 'Otherwise submit answer_submit' in message
    # synthesis_submit stays refused in planning, with the current state.
    [(operation, code, field, received, correction)] = seen['refusals']
    assert (operation, code, field, received) == ('synthesis_submit', 'synthesis_not_expected', 'state', 'PLANNING')
    assert 'answer_submit' in correction and 'plan_submit' in correction
    # The answer completed only the reviewed and verified revision.
    assert (context['state'], context['revision'], context['blocker']) == ('COMPLETED', 1, None)
    assert context['synthesis'] == 'Done: part 0 is implemented, reviewed and verified.'
    assert [n['stage'] for n in context['nodes']] == ['execution', 'review']
    assert not _events(store, task.task_id, 'USER_QUESTION_REQUESTED', 'ANSWER_NOT_CERTIFIED')


@pytest.mark.parametrize('outcome', ['not_run', 'unverified'])
@pytest.mark.parametrize('declared_command', [False, True])
def test_post_steer_answer_requires_gap_disclosure(tmp_path, monkeypatch, outcome, declared_command):
    from cortex_runtime.delegation import mcp_tools

    gap = dict(name='Live qualification', outcome=outcome, blocker='The live environment is unavailable.')
    if declared_command:
        # A passing final verification of different argv must not close this gap.
        gap['command'] = ['python3', '-B', '-c', 'print("live")']
    text = 'Part 0 is implemented and reviewed.'

    def disclose(store, channel, context, seen):
        args = dict(text=text, request_key='close')
        assert _try(seen, store, channel, 'answer_submit', args) is None
        assert seen['refusals'][-1][1] == 'synthesis_gaps_undisclosed'
        assert gap['name'] in seen['refusals'][-1][-1]
        current = TaskMachine(store).context(channel.task_id)
        assert current['state'] == 'PLANNING' and current['synthesis'] is None
        assert not _events(store, channel.task_id, 'ANSWER_SUBMITTED')

        # A non-concrete blank blocker cannot release the answer either.
        args['not_verified'] = [dict(item=gap['name'], blocker='   ')]
        assert _try(seen, store, channel, 'answer_submit', args) is None
        assert seen['refusals'][-1][1] == 'synthesis_blocker_required'
        args['not_verified'] = [dict(item=gap['name'], blocker=gap['blocker'])]
        receipt = mcp_tools.dispatch(store, channel, 'answer_submit', args)
        assert mcp_tools.dispatch(store, channel, 'answer_submit', args) == receipt
        # A new delivery key with identical content is still idempotent.
        assert mcp_tools.dispatch(store, channel, 'answer_submit', dict(args, request_key='same')) == receipt
        changed = dict(args, not_verified=[dict(item=gap['name'], blocker='Different blocker.')])
        assert _try(seen, store, channel, 'answer_submit', changed) is None
        assert seen['refusals'][-1][1] == 'request_key_conflict'
        assert _try(seen, store, channel, 'answer_submit', dict(changed, request_key='changed')) is None
        assert seen['refusals'][-1][1] == 'answer_conflict'
        seen['disclosure_checked'] = True

    store, task, context, seen = _drive(tmp_path, monkeypatch, disclose, worker_checks=[gap])
    _assert_steering_boundary(store, task.task_id)
    # Fake engine exceptions become prompt failures; require all callback assertions to finish.
    assert seen.get('disclosure_checked')
    stored = text + '\n\n## Not verified\n\n- ' + gap['name'] + ' — blocker: ' + gap['blocker']
    assert (context['state'], context['synthesis']) == ('COMPLETED', stored)
    submitted = _events(store, task.task_id, 'ANSWER_SUBMITTED')
    assert len(submitted) == 1 and submitted[0][2]['text'] == stored
    assert submitted[0][2]['not_verified'] == [dict(item=gap['name'], blocker=gap['blocker'])]
    assert stored in ''.join(seen['output'])


@pytest.mark.parametrize('outcome', ['not_run', 'unverified'])
def test_post_steer_answer_omits_exactly_closed_gap(tmp_path, monkeypatch, outcome):
    from cortex_runtime.orchestration.state import synthesis_checks

    gap = dict(name='Deferred verification', outcome=outcome, blocker='Awaiting final verification.',
               command=_plan()['verification'][0])
    store, task, context, seen = _drive(tmp_path, monkeypatch, _misused_then_answered, worker_checks=[gap])
    assert (context['state'], context['synthesis']) == (
        'COMPLETED', 'Done: part 0 is implemented, reviewed and verified.')
    with store.connection() as db:
        checks, gaps = synthesis_checks(db, task.task_id, context['revision'])
    assert not gaps
    retained = [check for _, check, _, _, _ in checks if check['name'] == gap['name']]
    assert len(retained) == 1 and retained[0]['outcome'] == outcome
    assert not _events(store, task.task_id, 'ANSWER_NOT_CERTIFIED')


def test_steering_asking_for_changes_gets_a_new_reviewed_and_verified_revision(tmp_path, monkeypatch):
    from cortex_runtime.delegation import mcp_tools

    def change(store, channel, context, seen):
        mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=_plan(), request_key='plan-1'))
        # Once a new plan exists, the earlier verification closes nothing.
        _try(seen, store, channel, 'answer_submit', dict(text='Skip the new work.', request_key='skip'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, change)
    assert [r[:2] for r in seen['refusals']] == [('answer_submit', 'direct_answer_not_available')]
    assert (context['state'], context['revision']) == ('COMPLETED', 2)
    assert [n['stage'] for n in context['nodes']] == ['execution', 'review']
    passed = _events(store, task.task_id, 'VERIFICATION_PASSED')
    assert [p[2]['revision'] for p in passed] == [1, 2]
    assert not _events(store, task.task_id, 'ANSWER_SUBMITTED')


def test_coordinator_that_only_submits_synthesis_still_reaches_the_user(tmp_path, monkeypatch):
    """Ordinary model misuse is not completed for it: synthesis stays refused
    in planning (with actionable guidance) and the user is asked."""
    def synthesis_only(store, channel, context, seen):
        _try(seen, store, channel, 'synthesis_submit', dict(text='Reviewed and verified.'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, synthesis_only)
    assert len(seen['planning']) == 3
    assert [r[1:4] for r in seen['refusals']] == [('synthesis_not_expected', 'state', 'PLANNING')] * 3
    assert (context['state'], context['blocker']) == ('WAITING_USER', 'structured_proposal_missing')
    assert context['synthesis'] is None
    assert not _events(store, task.task_id, 'ANSWER_SUBMITTED', 'SYNTHESIS_SUBMITTED')


def test_failed_verification_with_steering_is_never_closed_by_an_answer(tmp_path, monkeypatch):
    def answer(store, channel, context, seen):
        _try(seen, store, channel, 'answer_submit', dict(text='Done.', request_key='close'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, answer, verification='raise SystemExit(7)')
    assert {r[1] for r in seen['refusals']} == {'direct_answer_not_available'}
    assert 'answer_submit is not available now' in seen['planning'][0][0]
    assert context['state'] == 'WAITING_USER'
    assert not _events(store, task.task_id, 'ANSWER_SUBMITTED', 'VERIFICATION_PASSED')


def test_second_message_while_planning_keeps_the_verified_close_available(tmp_path, monkeypatch):
    def second_message_then_answer(store, channel, context, seen):
        if len(seen['planning']) == 1:
            assert seen['steer']('steer-2', 'One more remark, still nothing to change.') == seen['revisions'][0] + 1
            return None
        _try(seen, store, channel, 'answer_submit', dict(text='Done.', request_key='close'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, second_message_then_answer)
    assert len(_events(store, task.task_id, 'STEERING_APPLIED')) == 2
    assert len(seen['planning']) == 2 and 'Otherwise submit answer_submit' in seen['planning'][1][0]
    assert not seen['refusals']
    assert (context['state'], context['revision']) == ('COMPLETED', 1)


def test_steering_during_the_reconciling_turn_supersedes_its_synthesis(tmp_path, monkeypatch):
    """The coordinator's prompt was composed while reconciling; the message
    arrived during that turn. Its synthesis is recorded, but the runtime
    applies the steering before completing, and the answer closes the task."""
    def steer_mid_turn(store, channel, context, seen):
        seen['steered'] = True
        seen['revisions'] = [seen['steer'](), seen['steer']()]

    def answer(store, channel, context, seen):
        _try(seen, store, channel, 'answer_submit', dict(text='Done after the remark.', request_key='close'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, answer, steer_at='none',
                                        runtime_synthesis=False, reconciling=steer_mid_turn)
    assert _events(store, task.task_id, 'SYNTHESIS_SUBMITTED')
    assert len(_events(store, task.task_id, 'STEERING_APPLIED')) == 1
    assert not seen['refusals']
    assert (context['state'], context['synthesis']) == ('COMPLETED', 'Done after the remark.')


def test_project_changed_after_verification_is_not_certified_by_the_answer(tmp_path, monkeypatch):
    def drift_then_answer(store, channel, context, seen):
        (tmp_path / 'src' / 'part0' / 'late.txt').write_text('changed after verification')
        _try(seen, store, channel, 'answer_submit', dict(text='Done.', request_key='close'))

    store, task, context, seen = _drive(tmp_path, monkeypatch, drift_then_answer)
    assert not seen['refusals']
    assert (context['state'], context['blocker']) == ('FAILED', 'answer_not_certified:completion_project_revision_changed')


def test_question_after_steering_waits_for_the_user_and_then_closes(tmp_path, monkeypatch):
    from cortex_runtime.delegation import mcp_tools

    def ask_then_answer(store, channel, context, seen):
        if len(seen['planning']) == 1:
            mcp_tools.dispatch(store, channel, 'user_question_request',
                               dict(question='Keep the verified result?', options=['Keep it', 'Change it'],
                                    request_key='keep'))
            return None
        assert context['user_answers'][-1]['answer_text'] == 'Keep it'
        _try(seen, store, channel, 'answer_submit', dict(text='Kept as verified.', request_key='close'))

    def follow_up(runtime, task, output):
        store = runtime._store
        assert TaskMachine(store).context(task.task_id)['state'] == 'WAITING_USER'
        # An unanswered question is never consumed by a direct answer.
        with pytest.raises(StoreError):
            from cortex_runtime.delegation import mcp_tools
            mcp_tools.dispatch(store, seen_holder['seen']['coordinator'][-1], 'answer_submit',
                               dict(text='Closed without the user.', request_key='early'))
        return runtime.start_prompt(task.task_id, 'incoming', 'Keep it', output.append,
                                    threading.Event(), emit_tool_call=lambda update: True)

    seen_holder = {}

    def first(store, channel, context, seen):
        seen_holder['seen'] = seen
        return ask_then_answer(store, channel, context, seen)

    store, task, context, seen = _drive(tmp_path, monkeypatch, first, follow_up=follow_up)
    assert not seen['refusals']
    assert (context['state'], context['revision'], context['synthesis']) == ('COMPLETED', 1, 'Kept as verified.')
