from copy import deepcopy
from types import SimpleNamespace
import concurrent.futures
import json

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.orchestration.state import TaskMachine, validate_plan
from cortex_runtime.delegation.contracts import schemas_for


@pytest.fixture
def machine(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='one'), 'Implement feature', 'owner')
    machine = TaskMachine(store)
    machine.create(task.task_id, 'Implement feature')
    channel = SimpleNamespace(task_id=task.task_id, channel_id='coordinator', role='coordinator')
    return machine, channel


def plan(count=1):
    return dict(tasks=[dict(id=f'N{i}', role='backend-dev', objective=f'Implement part {i}',
                           dependencies=[], scope=[f'src/part{i}/**', f'tests/part{i}/**'], model='auto') for i in range(count)],
                verification=[['python3', '-B', '-m', 'pytest', '-q']])


def test_plan_atomic_and_idempotent(machine):
    state, channel = machine
    accepted = state.submit_plan(channel, plan(), 'p1')
    assert accepted == state.submit_plan(channel, plan(), 'p1')
    changed = plan(2)
    with pytest.raises(StoreError, match='request_key_conflict'):
        state.submit_plan(channel, changed, 'p1')
    context = state.context(channel.task_id)
    assert context['state'] == 'PLAN_READY'
    assert len(context['nodes']) == 1
    assert context['goal'] == 'Implement feature'


def test_invalid_plans_do_not_change_state(machine):
    state, channel = machine
    variants = []
    p = plan(2); p['tasks'][0]['dependencies'] = ['N1']; p['tasks'][1]['dependencies'] = ['N0']; variants.append(p)
    p = plan(); p['tasks'][0]['dependencies'] = ['missing']; variants.append(p)
    p = plan(); p['tasks'][0]['scope'] = ['../outside']; variants.append(p)
    p = plan(); p['tasks'][0]['model'] = 'expensive'; variants.append(p)
    p = plan(); p['verification'] = []; variants.append(p)
    p = plan(); p['tasks'][0]['role'] = 'code-reviewer'; variants.append(p)
    for p in variants:
        with pytest.raises(StoreError):
            state.submit_plan(channel, p, 'invalid')
    assert state.context(channel.task_id)['state'] == 'PLANNING'


@pytest.mark.parametrize('argv', [
    ['test', '$(wc', '-c', '<', 'README.md', ')', '-eq', '41'],
    ['printf', '`date`'],
    ['printf', 'ok', '|', 'cat'],
    ['true', '&&', 'false'],
    ['echo', 'ok', ';', 'false'],
    ['echo', '('],
    ['echo', '>'],
    ['echo', '>>'],
])
def test_plan_rejects_shell_syntax_in_verification_argv(machine, argv):
    state, channel = machine
    proposal = plan()
    proposal['verification'] = [argv]
    with pytest.raises(StoreError, match='verification_shell_syntax') as error:
        state.submit_plan(channel, proposal, 'shell-syntax')
    assert error.value.field == 'plan.verification'
    assert 'documented project command or script' in error.value.correction
    assert state.context(channel.task_id)['state'] == 'PLANNING'


@pytest.mark.parametrize('argv', [
    ['pytest', '-k', 'not (slow or integration)'],
    ['env', 'NAME=value', 'python3', '-m', 'pytest', 'tests/*.py'],
])
def test_plan_allows_literal_verification_arguments(machine, argv):
    proposal = plan()
    proposal['verification'] = [argv]
    assert validate_plan(proposal) == proposal


def test_synthesis_discloses_retained_gap_with_blocker(machine):
    state, channel = machine
    state.submit_plan(channel, plan(), 'gap-plan')
    with state.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (channel.task_id,))
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND node_id='N0'",
                   ('{"checks":[{"name":"Live GUI","outcome":"not_run","blocker":"GUI unavailable"}]}',
                    channel.task_id))
        db.commit()
    with pytest.raises(StoreError, match='synthesis_gaps_undisclosed') as error:
        state.submit_synthesis(channel, 'Done')
    assert 'Live GUI' in error.value.correction
    with state.store.connection() as db:
        assert db.execute('SELECT synthesis FROM runtime_tasks WHERE task_id=?',
                          (channel.task_id,)).fetchone()[0] is None
    items = [dict(item='  LIVE   gui ', blocker='The disposable GUI failed to start.')]
    assert state.submit_synthesis(channel, 'Done', items)['recorded']
    with state.store.connection() as db:
        stored = db.execute('SELECT synthesis FROM runtime_tasks WHERE task_id=?',
                            (channel.task_id,)).fetchone()[0]
    assert stored == 'Done\n\n## Not verified\n\n-   LIVE   gui  — blocker: The disposable GUI failed to start.'
    assert state.submit_synthesis(channel, 'Done', items)['recorded']
    with pytest.raises(StoreError, match='synthesis_conflict'):
        state.submit_synthesis(channel, 'Done', [dict(item='Live GUI', blocker='Different reason')])


def test_same_revision_pass_resolves_synthesis_gap(machine):
    state, channel = machine
    state.submit_plan(channel, plan(2), 'gap-plan')
    with state.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (channel.task_id,))
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND node_id='N0'",
                   ('{"checks":[{"name":"Live GUI","outcome":"unverified"}]}', channel.task_id))
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND node_id='N1'",
                   ('{"checks":[{"name":"  live   gui ","outcome":"passed"}]}', channel.task_id))
        db.commit()
    assert state.submit_synthesis(channel, 'Verified')['recorded']
    with state.store.connection() as db:
        stored = db.execute('SELECT synthesis FROM runtime_tasks WHERE task_id=?',
                            (channel.task_id,)).fetchone()[0]
    assert stored == 'Verified'


def test_cannot_skip_review_or_complete_with_chat(machine):
    state, channel = machine
    state.submit_plan(channel, plan(), 'p1')
    state.move(channel.task_id, 'EXECUTING')
    for target in ('COMPLETED', 'VERIFYING', 'RECONCILING'):
        with pytest.raises(StoreError):
            state.move(channel.task_id, target)
    assert state.context(channel.task_id)['state'] == 'EXECUTING'


def test_scheduler_respects_dependencies_with_no_worker_limit(machine):
    state, channel = machine
    proposed = plan(8)
    proposed['tasks'][1]['dependencies'] = ['N0']
    state.submit_plan(channel, proposed, 'p1')
    state.move(channel.task_id, 'EXECUTING')
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(state.claim_ready, channel.task_id, channel.channel_id) for _ in range(2)]
        nodes = [n for future in futures for n in future.result()]
    assert len(nodes) == 7
    assert len({n['assignment_ref'] for n in nodes}) == 7
    assert 'N1' not in {n['node_id'] for n in nodes}


def test_restart_preserves_plan_and_reservations(machine):
    state, channel = machine
    state.submit_plan(channel, plan(), 'p1')
    state.move(channel.task_id, 'EXECUTING')
    claimed = state.claim_ready(channel.task_id, channel.channel_id)
    restarted = TaskMachine(state.store)
    assert restarted.claim_ready(channel.task_id, channel.channel_id) == []
    assert restarted.context(channel.task_id)['nodes'][0]['assignment_ref'] == claimed[0]['assignment_ref']


def test_retired_model_controls_not_advertised():
    coordinator = schemas_for('coordinator')
    assert {'plan_submit','task_get_context','synthesis_submit'} <= set(coordinator)
    assert not {'delegate','decide','cancel','integrate','update_plan','assess_check'} & set(coordinator)
    assert 'result_submit' in schemas_for('worker')
    assert not {'delegate','decide','plan_submit'} & set(schemas_for('worker'))


@pytest.mark.parametrize("scenario", ["success", "advice", "fix", "review_block", "verification_fail", "synthesis_drift", "post_review_drift", "elicitation", "text_question", "worker_question", "replan", "scope", "escalation", "result_recovery", "result_correction", "check_history"])
def test_composed_runtime_executes_plan_review_verification_synthesis(tmp_path, scenario):
    """A model cannot finish the task; the runtime drives every required stage."""
    import threading
    import uuid
    from pathlib import Path
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.delegation import mcp_tools
    from orchestration_fakes import FakeSessionRuntime, FakeWorkspace, FakeConfigSnapshotSource

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='runtime'), 'Inspect an empty project', 'owner')
    calls = []
    reviews = []
    questions = []
    history = []
    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            calls.append(channel.role)
            if channel.role == 'coordinator':
                context = mcp_tools.dispatch(store, channel, 'task_get_context', {})
                if context['state'] == 'PLANNING':
                    if scenario in {'elicitation', 'text_question'} and not questions:
                        questions.append(mcp_tools.dispatch(store, channel, 'user_question_request',
                            dict(question='Which target?', options=['Small target', 'Large target'], request_key='question')))
                        return PromptOutcome(StopReason.END_TURN, 'Visible question request')
                    if questions and scenario in {'elicitation','text_question','worker_question'}:
                        assert context['user_answers'][-1]['answer_text'] == 'Small target'
                    proposal = plan()
                    proposal['verification'] = [['python3', '-B', '-c', 'raise SystemExit(7)' if scenario == 'verification_fail' else 'print("verified")']]
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=proposal, request_key='plan-'+str(context['revision'])))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Result reviewed and verified.', **(
                        dict(not_verified=[dict(item='Unbound passing claim', blocker='No command receipt was cited for it.')])
                        if scenario == 'result_correction' else {})))
                    if scenario == 'synthesis_drift':
                        (tmp_path / 'unexpected.txt').write_text('external change')
            else:
                assert 'Communicate exclusively in English' in message
                assert message.endswith('Worker response language: English only, including the final response.')
                if scenario in {'worker_question','replan','scope','escalation'} and not questions:
                    questions.append('pending')
                    if scenario=='worker_question':
                        mcp_tools.dispatch(store,channel,'ask_question',dict(question='Which target?',request_key='worker-q'))
                    else:
                        operation={'replan':'replan_request','scope':'scope_request','escalation':'escalation_request'}[scenario]
                        args=dict(reason='Additional evidence requires a new plan.',request_key='change')
                        if scenario=='scope': args['scope']=['src/part0/**']
                        receipt=mcp_tools.dispatch(store,channel,operation,args)
                        assert receipt['scope_granted'] is False
                        with pytest.raises(StoreError):
                            mcp_tools.dispatch(store,channel,'project_read',dict(path='.'))
                    return PromptOutcome(StopReason.END_TURN,'Requested runtime decision')
                with store.connection() as db:
                    worker_role = db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',
                                             (channel.attempt_id,)).fetchone()['role_id']
                if scenario != 'advice' and worker_role not in {'worker-code-reviewer', 'worker-senior-consultant'}:
                    # An implementation changes the project, as a native edit tool would.
                    (tmp_path / 'src' / 'part0').mkdir(parents=True, exist_ok=True)
                    (tmp_path / 'src' / 'part0' / f'{channel.attempt_id}.txt').write_text('implemented')
                draft = store.call('create_draft', dict(template='general', request_key='draft-' + channel.attempt_id),
                                   channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
                path = Path(draft['draft_path'])
                marker = path.read_bytes().split(b'\n\n', 1)[0] + b'\n\n'
                path.write_bytes(marker + b'# Report\n\nObserved result.\n')
                receipt = store.call('write_report', dict(draft_id=draft['draft_id'], title='Result',
                                     summary='Observed result', author='worker', request_key='report-' + channel.attempt_id),
                                     channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
                report_id = receipt['report_id']
                with store.connection() as db:
                    provision = db.execute('SELECT role_id FROM delegation_provisioning WHERE attempt_id=?',
                                           (channel.attempt_id,)).fetchone()
                args = dict(report_ref=report_id, summary='Observed result', checks=[], request_key='result')
                if provision['role_id'] == 'worker-code-reviewer' and scenario in {'elicitation', 'text_question', 'worker_question'}:
                    # The reviewer sees the user's answer, bound to the session it came from.
                    answers = mcp_tools.dispatch(store, channel, 'list_sources', {})['answered_questions']
                    assert [(a['question_text'], a['answer_text'], a['session_id'], a['asked_by']) for a in answers] == [
                        ('Which target?', 'Small target', 'incoming',
                         'worker' if scenario == 'worker_question' else 'coordinator')]
                    capsule = mcp_tools.dispatch(store, channel, 'read_capsule', {})
                    assert [a['question_id'] for a in capsule['sections']['constraints']['answered_questions']] == [
                        answers[0]['question_id']]
                if provision['role_id'] == 'worker-code-reviewer':
                    args['verdict'] = ('BLOCK' if scenario == 'review_block' else 'REQUEST_CHANGES' if scenario == 'fix' and not reviews else 'APPROVE')
                    reviews.append(args['verdict'])
                if scenario == 'result_correction':
                    # An unbound pass is kept as a disclosed unverified gap;
                    # the cited real command stays passed.
                    observed = mcp_tools.dispatch(store, channel, 'command_run',
                        dict(argv=['python3', '-B', '-c', 'print("observed check")'], request_key='verify-real'))
                    args['checks'] = [dict(name='Unbound passing claim', outcome='passed'),
                                      dict(name='Actual check', outcome='passed', tool_call_ref=observed['tool_call_ref'])]
                if scenario == 'check_history' and provision['role_id'] != 'worker-code-reviewer':
                    # A long task's own history: the check failed, the worker
                    # fixed it, and an exact rerun of the same command passed.
                    toggle = ['python3', '-B', '-c', 'import pathlib,sys; sys.exit(0 if pathlib.Path("fixed.flag").exists() else 1)']
                    first = mcp_tools.dispatch(store, channel, 'command_run', dict(argv=toggle, request_key='first-run'))
                    (tmp_path / 'fixed.flag').write_text('fixed')
                    rerun = mcp_tools.dispatch(store, channel, 'command_run', dict(argv=toggle, request_key='rerun'))
                    assert (first['exit_code'], rerun['exit_code']) == (1, 0)
                    args['checks'] = [dict(name='unit tests', outcome='failed', tool_call_ref=first['tool_call_ref']),
                                      dict(name='unit tests', outcome='passed', tool_call_ref=rerun['tool_call_ref'])]
                    history.append((first['tool_call_ref'], rerun['tool_call_ref']))
                if scenario == 'result_recovery':
                    from unittest.mock import patch
                    # Crash after the exact publication event, before its runtime
                    # result projection. Recovery uses real retained evidence.
                    with patch.object(TaskMachine, 'record_result', side_effect=RuntimeError('process interruption')):
                        with pytest.raises(RuntimeError, match='process interruption'):
                            mcp_tools.dispatch(store, channel, 'result_submit', args)
                    from cortex_runtime.orchestration.recovery import reconcile_results
                    reconcile_results(store, task.task_id)
                else:
                    receipt = mcp_tools.dispatch(store, channel, 'result_submit', args)
                    if scenario == 'result_correction':
                        assert [(c['outcome'], c.get('evidence_gap')) for c in receipt['checks']] == [
                            ('unverified', 'tool_call_ref_missing'), ('passed', None)]
                    if scenario == 'post_review_drift' and provision['role_id'] == 'worker-code-reviewer':
                        (tmp_path / 'changed-after-review.txt').write_text('external change')
            return PromptOutcome(StopReason.END_TURN, 'Visible model message')
        return behavior
    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    output = []
    class Elicitor:
        def elicit(self, session_id, question, options):
            assert question == 'Which target?'
            return options[0] if options else 'Small target'
    try:
        result = runtime.start_prompt(task.task_id, 'incoming', 'Inspect an empty project', output.append,
                                      threading.Event(), emit_tool_call=lambda update: True,
                                      host_elicitor=Elicitor() if scenario in {'elicitation','worker_question'} else None)
        if scenario == 'text_question':
            assert TaskMachine(store).context(task.task_id)['state'] == 'WAITING_USER'
            result = runtime.start_prompt(task.task_id, 'incoming', 'Small target', output.append,
                                          threading.Event(), emit_tool_call=lambda update: True)
    finally:
        runtime.shutdown()
    assert result == StopReason.END_TURN
    context = TaskMachine(store).context(task.task_id)
    if scenario == 'advice':
        # Nothing in the project changed: no reviewer, and the runtime says why.
        assert context['state'] == 'COMPLETED', context
        assert calls == ['coordinator', 'worker', 'coordinator']
        assert reviews == [] and not any(n['stage'] == 'review' for n in context['nodes'])
        with store.connection() as db:
            waived = [json.loads(r[0]) for r in db.execute(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_NOT_REQUIRED'", (task.task_id,))]
        assert [(w['reason'], w['revision']) for w in waived] == [('no_project_changes', 1)]
        assert context['review_not_required']['revision_digest'] == context['verification']['revision_digest']
        return
    if scenario == 'check_history':
        # The earlier failure stays in the result as history, closed by the
        # exact later rerun; the task completes through review and verification.
        assert context['state'] == 'COMPLETED', context
        [(failed_ref, passed_ref)] = history
        node = next(n for n in context['nodes'] if n['stage'] == 'execution')
        assert node['status'] == 'completed'
        assert [(c['outcome'], c.get('closed_by'), c['evidence_strength']) for c in node['result']['checks']] == [
            ('failed', passed_ref, 'verified'), ('passed', None, 'verified')]
        # A failed verification recorded after the pass is the current outcome:
        # the earlier pass can no longer complete the task.
        from cortex_runtime.orchestration.state import event
        machine = TaskMachine(store)
        with store.connection() as db:
            event(db, task.task_id, 'VERIFICATION_FAILED', dict(context['verification'], checks=[]))
            db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (task.task_id,))
            db.commit()
        with pytest.raises(StoreError, match='completion_verification_missing'):
            machine.move(task.task_id, 'COMPLETED')
        return
    if scenario in {'success', 'fix', 'elicitation', 'text_question', 'worker_question', 'replan', 'scope', 'escalation', 'result_recovery', 'result_correction'}:
        assert context['state'] == 'COMPLETED', context
        expected = ['coordinator','worker','worker','worker','worker','coordinator'] if scenario == 'fix' else ['coordinator','worker','worker','coordinator']
        if scenario in {'worker_question','replan','scope','escalation'}:
            expected=['coordinator','worker']+expected
        elif questions:
            expected.insert(0, 'coordinator')
        if scenario=='escalation':
            expected.insert(3,'worker')
            assert any(n['role']=='worker-senior-consultant' for n in context['nodes'])
        assert calls == expected
        assert any('COMPLETED' in text for text in output)
    elif scenario in {'synthesis_drift', 'post_review_drift'}:
        # A project that changed under the pipeline is recovered by the
        # coordinator re-planning, not a stop.
        assert context['state'] == 'COMPLETED', context
    else:
        # Never a dead end: after the automatic attempts the user is asked.
        assert context['state'] == 'WAITING_USER', context


def test_question_requires_exact_real_answer_and_retries_share_identity(machine):
    from cortex_runtime.orchestration.questions import request, resume_answered
    from cortex_runtime.acp.questions_store import answer
    state, channel = machine
    args = dict(question='Choose a target.', options=['A', 'B'], request_key='q1')
    first = request(state.store, channel, args)
    assert request(state.store, channel, args) == first
    assert not resume_answered(state.store, channel.task_id)
    assert state.context(channel.task_id)['state'] == 'WAITING_USER'
    with pytest.raises(StoreError, match='request_key_conflict'):
        request(state.store, channel, dict(args, question='Changed question'))
    assert answer(state.store, first['question_id'], 'B', 'text')
    assert resume_answered(state.store, channel.task_id)
    assert not resume_answered(state.store, channel.task_id)
    assert state.context(channel.task_id)['user_answers'][0]['answer_text'] == 'B'


def test_cancel_is_idempotent_and_refuses_late_proposals(machine):
    state, channel = machine
    state.cancel(channel.task_id)
    state.cancel(channel.task_id)
    with pytest.raises(StoreError, match='plan_not_expected'):
        state.submit_plan(channel, plan(), 'late')
    assert state.context(channel.task_id)['state'] == 'CANCELLED'


def test_report_digest_rejects_symlinked_file_and_parent(machine, tmp_path):
    from cortex_runtime.orchestration.artifacts import report_digest
    state, channel = machine
    folder = state.store.directory / channel.task_id
    folder.mkdir(exist_ok=True)
    outside = tmp_path / 'external-report'
    outside.write_text('not a publication')
    (folder / 'symlink.md').symlink_to(outside)
    with pytest.raises(StoreError, match='assignment_report_unavailable'):
        report_digest(state.store, channel.task_id, 'symlink.md')
    (state.store.directory / 'fake_task').symlink_to(folder, target_is_directory=True)
    with pytest.raises(StoreError, match='assignment_report_unavailable'):
        report_digest(state.store, 'fake_task', 'symlink.md')


def test_user_steering_applies_denials_only_after_active_work(machine):
    from cortex_runtime.orchestration.policy import steer, apply_pending_steering
    state, ch = machine
    state.submit_plan(ch, plan(), 'old')
    state.move(ch.task_id, 'EXECUTING')
    state.claim_ready(ch.task_id, ch.channel_id)
    assert steer(state.store, ch.task_id, 'Не меняй database', 'user:one')
    assert not steer(state.store, ch.task_id, 'Не меняй database', 'user:one')
    context = state.context(ch.task_id)
    assert context['state'] == 'EXECUTING'
    assert context['nodes'][0]['status'] == 'running'
    assert context['policy']['deny_write'] == []
    assert not apply_pending_steering(state.store, ch.task_id)
    # This unit fixture advances the completed plan to its verified
    # reconciliation boundary; the controller integration test covers the
    # actual worker, review and verification path.
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (ch.task_id,))
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (ch.task_id,))
        db.commit()
    assert apply_pending_steering(state.store, ch.task_id)
    context = state.context(ch.task_id)
    assert context['state'] == 'PLANNING'
    assert context['nodes'][0]['status'] == 'completed'
    assert context['policy']['deny_write'] == ['database']
    denied = plan()
    denied['tasks'][0]['scope'] = ['database/schema/**']
    with pytest.raises(StoreError, match='scope_conflicts_with_user_policy'):
        state.submit_plan(ch, denied, 'denied')
    state.submit_plan(ch, plan(), 'new')
    assert state.context(ch.task_id)['revision'] == 2


def test_scheduler_serializes_overlapping_write_scopes(machine):
    state,ch=machine
    proposed=plan(3)
    proposed['tasks'][0]['scope']=['src/**']
    proposed['tasks'][1]['scope']=['src/a.py']
    proposed['tasks'][2]['scope']=['docs/**']
    state.submit_plan(ch,proposed,'scoped');state.move(ch.task_id,'EXECUTING')
    nodes=state.claim_ready(ch.task_id,ch.channel_id)
    assert {n['node_id'] for n in nodes}=={'N0','N2'}


def test_lost_unprovisioned_reservation_is_retried_bounded(machine):
    from cortex_runtime.orchestration.recovery import retry_lost
    state,ch=machine
    state.submit_plan(ch,plan(),'p');state.move(ch.task_id,'EXECUTING')
    runtime=SimpleNamespace(_store=state.store)
    identities=[]
    for attempt in range(3):
        node=state.claim_ready(ch.task_id,ch.channel_id)[0]
        identities.append(node['assignment_ref'])
        if attempt<2: retry_lost(runtime,ch.task_id,node)
        else:
            with pytest.raises(StoreError,match='worker_retry_limit'):
                retry_lost(runtime,ch.task_id,node)
    assert len(set(identities))==3


@pytest.mark.parametrize('left,right,expected',[
    ('database*','database_backup',True),('src/a?.py','src/ab.py',True),
    ('src/a','src/ab',False),('src/**','tests/**',False),('**/*.py','secret/file.py',True),
])
def test_scope_overlap_never_treats_a_wildcard_prefix_as_a_directory(left,right,expected):
    from cortex_runtime.orchestration.policy import overlaps
    assert overlaps(left,right)==expected
    assert overlaps(right,left)==expected


def test_finished_node_holds_scope_until_engine_absence_is_observed(machine):
    from cortex_runtime.orchestration.recovery import release_observed
    state,ch=machine
    proposed=plan(2)
    for node in proposed['tasks']: node['scope']=['src/**']
    state.submit_plan(ch,proposed,'p');state.move(ch.task_id,'EXECUTING')
    first=state.claim_ready(ch.task_id,ch.channel_id)[0]
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE assignment_ref=?",(first['assignment_ref'],))
        db.execute('INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)',
            (first['assignment_ref'],ch.task_id,'attempt','worker-backend-dev','ready','ready',1,'now','now'))
        db.commit()
    observed=['alive']
    runtime=SimpleNamespace(_store=state.store,_engine=SimpleNamespace(lookup_owned=lambda _:observed[0]))
    assert not release_observed(runtime,ch.task_id,'attempt')
    assert state.claim_ready(ch.task_id,ch.channel_id)==[]
    observed[0]='unknown'
    assert not release_observed(runtime,ch.task_id,'attempt')
    assert state.claim_ready(ch.task_id,ch.channel_id)==[]
    observed[0]='absent'
    assert release_observed(runtime,ch.task_id,'attempt')
    assert [n['node_id'] for n in state.claim_ready(ch.task_id,ch.channel_id)]==['N1']


def test_runtime_stop_question_releases_the_stopped_plans_nodes(machine):
    state,ch=machine
    proposed=plan(2)
    proposed['tasks'][1]['dependencies']=['N0']
    state.submit_plan(ch,proposed,'p');state.move(ch.task_id,'EXECUTING')
    assert [n['node_id'] for n in state.claim_ready(ch.task_id,ch.channel_id)]==['N0']
    state.move(ch.task_id,'BLOCKED',blocker='stale_assignment_result')
    context=state.context(ch.task_id)
    assert context['state']=='WAITING_USER'
    assert {n['status'] for n in context['nodes']}=={'cancelled'}


def test_stale_running_node_of_an_earlier_revision_holds_scope_only_until_released(machine):
    state,ch=machine
    proposed=plan()
    proposed['tasks'][0]['scope']=['src/**']
    state.submit_plan(ch,proposed,'old');state.move(ch.task_id,'EXECUTING')
    old=state.claim_ready(ch.task_id,ch.channel_id)[0]
    # A path that replanned without cancelling left the old node 'running'.
    with state.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",(ch.task_id,))
        db.commit()
    state.submit_plan(ch,proposed,'new');state.move(ch.task_id,'EXECUTING')
    assert state.claim_ready(ch.task_id,ch.channel_id)==[]
    with state.store.connection() as db:
        db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
                   (ch.task_id,'WORKER_RELEASED','runtime',
                    '{"assignment_ref":"%s","observed_session":"absent"}'%old['assignment_ref'],'now'))
        db.commit()
    assert [n['node_id'] for n in state.claim_ready(ch.task_id,ch.channel_id)]==['N0']


def test_direct_answer_is_runtime_completed_and_cannot_bypass_a_plan(machine):
    state,channel=machine
    with pytest.raises(StoreError): state.move(channel.task_id,'COMPLETED')
    state.submit_answer(channel,'An informational answer.','answer-1')
    assert state.context(channel.task_id)['state']=='ANSWER_READY'
    state.submit_answer(channel,'An informational answer.','answer-1')
    with pytest.raises(StoreError,match='answer_conflict'):
        state.submit_answer(channel,'Different answer.','answer-2')
    state.move(channel.task_id,'COMPLETED')
    assert not state.context(channel.task_id)['nodes']
    assert not state.context(channel.task_id)['verification']
    assert state.submit_answer(channel,'An informational answer.','answer-1')['recorded']
    with pytest.raises(StoreError,match='request_key_conflict'):
        state.submit_answer(channel,'Changed delivery.','answer-1')


def test_direct_answer_cannot_replace_planned_work_or_review(machine):
    state,channel=machine
    state.submit_plan(channel,plan(),'plan')
    for phase in ('PLAN_READY','EXECUTING'):
        if phase=='EXECUTING': state.move(channel.task_id,phase)
        with pytest.raises(StoreError,match='direct_answer_not_available'):
            state.submit_answer(channel,'Skip the required work.','skip')
    assert 'answer_submit' not in schemas_for('worker')


def _complete_and_reopen(state, channel, key='done'):
    """A planned revision reaches COMPLETED, then a user message reopens it.

    Unit fixture: the completed plan is advanced to its verified completion
    directly; the controller integration test in
    test_acp_steering_and_questions.py drives the real review and
    verification path."""
    from cortex_runtime.orchestration.policy import steer, apply_pending_steering
    from cortex_runtime.orchestration.state import event
    state.submit_plan(channel, plan(), key); state.move(channel.task_id, 'EXECUTING')
    node = state.claim_ready(channel.task_id, channel.channel_id)[0]
    with state.store.connection() as db:
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                   (node['assignment_ref'], channel.task_id, 'xa_'+key, 'worker-backend-dev', 'ready', 'ready', 1, 'now', 'now'))
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (channel.task_id,))
        event(db, channel.task_id, 'WORKER_RELEASED', dict(assignment_ref=node['assignment_ref'], observed_session='absent'))
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Verified.' WHERE task_id=?", (channel.task_id,))
        event(db, channel.task_id, 'TASK_STATE_CHANGED', dict(previous='RECONCILING', state='COMPLETED', blocker=None))
        db.commit()
    assert steer(state.store, channel.task_id, 'Can I already see the drafts?', f'user:{key}')
    assert apply_pending_steering(state.store, channel.task_id)
    context = state.context(channel.task_id)
    assert (context['state'], context['plan'] is not None, bool(context['nodes'])) == ('PLANNING', True, True)
    return context


def test_follow_up_question_on_a_completed_task_is_answered_directly(machine):
    """A read-only question after verified completion dead-ended in
    structured_proposal_missing: answer_submit and synthesis_submit were both
    refused, leaving only a plan."""
    state, channel = machine
    revision = _complete_and_reopen(state, channel)['revision']
    assert state.submit_answer(channel, 'Yes: the drafts are in drafts/.', 'follow-up')['state'] == 'ANSWER_READY'
    state.move(channel.task_id, 'COMPLETED')
    context = state.context(channel.task_id)
    assert (context['state'], context['revision'], context['blocker']) == ('COMPLETED', revision, None)
    # A second follow-up on the same task is answered the same way.
    from cortex_runtime.orchestration.policy import steer, apply_pending_steering
    assert steer(state.store, channel.task_id, 'And the final version?', 'user:again')
    assert apply_pending_steering(state.store, channel.task_id)
    state.submit_answer(channel, 'Not yet.', 'again')
    state.move(channel.task_id, 'COMPLETED')
    assert state.context(channel.task_id)['state'] == 'COMPLETED'


def test_reopened_completed_task_with_new_work_cannot_answer_until_it_is_verified(machine):
    from cortex_runtime.orchestration.state import event
    state, channel = machine
    _complete_and_reopen(state, channel)
    state.submit_plan(channel, plan(), 'change')
    with pytest.raises(StoreError, match='direct_answer_not_available'):
        state.submit_answer(channel, 'Skip the new work.', 'skip-ready')
    state.move(channel.task_id, 'EXECUTING')
    assert state.claim_ready(channel.task_id, channel.channel_id)
    # The unverified new revision comes back to the coordinator in PLANNING.
    assert state.return_to_coordinator(channel.task_id, 'worker_failed')
    state.move(channel.task_id, 'PLANNING')
    with pytest.raises(StoreError, match='direct_answer_not_available'):
        state.submit_answer(channel, 'Skip the new work.', 'skip-planning')
    # Nor can a forced ANSWER_READY complete it.
    with state.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='ANSWER_READY',synthesis='Skip.' WHERE task_id=?", (channel.task_id,))
        event(db, channel.task_id, 'ANSWER_SUBMITTED', dict(text='Skip.', request_key='forced'))
        db.commit()
    with pytest.raises(StoreError, match='completion_answer_invariant'):
        state.move(channel.task_id, 'COMPLETED')


def test_reopened_completed_task_with_a_new_worker_cannot_answer(machine):
    state, channel = machine
    _complete_and_reopen(state, channel)
    with state.store.connection() as db:
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                   ('unplanned', channel.task_id, 'xa_unplanned', 'worker-backend-dev', 'ready', 'ready', 1, 'now', 'now'))
        db.commit()
    with pytest.raises(StoreError, match='direct_answer_not_available'):
        state.submit_answer(channel, 'Skip.', 'skip')


def test_unverified_executed_work_is_never_closed_by_a_direct_answer(machine):
    """Work that never reached COMPLETED: neither a return to planning nor a
    reopened FAILED task makes answer_submit available."""
    from cortex_runtime.orchestration.policy import steer, apply_pending_steering
    state, channel = machine
    state.submit_plan(channel, plan(), 'p'); state.move(channel.task_id, 'EXECUTING')
    assert state.claim_ready(channel.task_id, channel.channel_id)
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", (channel.task_id,)); db.commit()
    assert state.return_to_coordinator(channel.task_id, 'worker_failed')
    state.move(channel.task_id, 'PLANNING')
    with pytest.raises(StoreError, match='direct_answer_not_available'):
        state.submit_answer(channel, 'Done.', 'unverified')
    state.move(channel.task_id, 'FAILED', blocker='coordinator_turn_failed:refusal')
    assert steer(state.store, channel.task_id, 'What happened?', 'user:failed')
    assert apply_pending_steering(state.store, channel.task_id)
    assert state.context(channel.task_id)['state'] == 'PLANNING'
    with pytest.raises(StoreError, match='direct_answer_not_available'):
        state.submit_answer(channel, 'Done.', 'after-failure')


@pytest.mark.parametrize('language',['en','ru'])
@pytest.mark.parametrize('streamed',[True,False])
def test_project_overview_reads_and_answers_once_without_workers(tmp_path,language,streamed):
    import threading
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.acp import steering, messages
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.delegation import mcp_tools
    from orchestration_fakes import FakeSessionRuntime, FakeWorkspace, FakeConfigSnapshotSource
    (tmp_path/'README.md').write_text('A static website for focus sessions.\n')
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='overview',event_id='one'),'Расскажи о чем проект','owner')
    steering.set_language_override(store,task.task_id,language)
    answer='A static website for focus sessions.' if language=='en' else 'Статический сайт для фокус-сессий.'
    calls=[]
    def router(store,channel,ref,launch,instructions):
        assert channel.role=='coordinator', 'An overview must not launch a worker/reviewer'
        def behavior(store,channel,ref,prompt,index):
            calls.append(ref)
            assert prompt.endswith(messages.render('answer_in_language',language))
            observed=mcp_tools.dispatch(store,channel,'project_read',dict(path='README.md'))
            assert 'focus sessions' in str(observed)
            with pytest.raises(StoreError):
                mcp_tools.dispatch(store,channel,'project_write',dict(path='README.md',content='changed',expected_sha256='absent',request_key='no-write'))
            mcp_tools.dispatch(store,channel,'answer_submit',dict(text=answer,request_key='overview-answer'))
            return PromptOutcome(StopReason.END_TURN,answer)
        return behavior
    runtime=OrchestratingRuntime(store,FakeSessionRuntime(store,router,emit_outcome_text_as_delta=streamed),FakeWorkspace(),FakeConfigSnapshotSource(),allow_unqualified_delivery=True)
    output=[]
    try:
        result=runtime.start_prompt(task.task_id,'incoming','Расскажи о чем проект',output.append,threading.Event())
    finally: runtime.shutdown()
    assert result==StopReason.END_TURN
    assert output==[answer]
    assert len(calls)==1
    context=TaskMachine(store).context(task.task_id)
    assert context['state']=='COMPLETED' and context['nodes']==[] and context['verification'] is None
    assert (tmp_path/'README.md').read_text()=='A static website for focus sessions.\n'
    with store.connection() as db:
        review_events=db.execute("SELECT type FROM runtime_events WHERE task_id=? AND type IN "
                                 "('REVIEW_STARTED','REVIEW_NOT_REQUIRED','EXECUTION_BASELINE')",(task.task_id,)).fetchall()
    assert review_events==[], 'A direct answer never reaches the review stage'


def test_another_tasks_stale_running_node_never_blocks_this_one(machine):
    """A blocked task whose worker is long gone kept its node 'running' with
    services/** and held every later task off the same files for good."""
    state, ch = machine
    other = bootstrap_task(state.store, SourceEvent(origin='test', event_id='two'), 'Earlier attempt', 'owner')
    state.create(other.task_id, 'Earlier attempt')
    other_channel = SimpleNamespace(task_id=other.task_id, channel_id='coordinator', role='coordinator')
    wide = plan(); wide['tasks'][0]['scope'] = ['src/**']
    state.submit_plan(other_channel, wide, 'other'); state.move(other.task_id, 'EXECUTING')
    assert state.claim_ready(other.task_id, 'coordinator')
    state.move(other.task_id, 'BLOCKED', blocker='worker_failure')
    state.submit_plan(ch, plan(5), 'mine'); state.move(ch.task_id, 'EXECUTING')
    assert len(state.claim_ready(ch.task_id, ch.channel_id)) == 5


def test_stop_asks_the_user_and_only_their_answer_lets_the_coordinator_close(machine):
    """A stopped investigation used to dead-end in BLOCKED with only a code."""
    from cortex_runtime.orchestration.questions import pending, resume_answered
    from cortex_runtime.acp.questions_store import answer
    state, channel = machine
    state.submit_plan(channel, plan(), 'p'); state.move(channel.task_id, 'EXECUTING')
    assert state.claim_ready(channel.task_id, channel.channel_id)
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='failed' WHERE task_id=?", (channel.task_id,)); db.commit()
    state.move(channel.task_id, 'BLOCKED', blocker='worker_failed')
    context = state.context(channel.task_id)
    # A stop about the work: the coordinator asks the user a concrete question.
    assert (context['state'], context['ask_user_required']) == ('RECOVERING', 'worker_failed')
    state.move(channel.task_id, 'EXECUTING')
    state.move(channel.task_id, 'BLOCKED', blocker='structured_proposal_missing')  # a stop about the coordinator
    context = state.context(channel.task_id)
    assert (context['state'], context['blocker']) == ('WAITING_USER', 'structured_proposal_missing')
    question = pending(state.store, channel.task_id)
    assert question['blocker'] == 'structured_proposal_missing' and question['resume_state'] == 'RECOVERING'
    state.move(channel.task_id, 'RECOVERING'); state.move(channel.task_id, 'PLANNING')
    with pytest.raises(StoreError, match='direct_answer_not_available'):
        state.submit_answer(channel, 'Done.', 'unasked')  # the coordinator alone cannot skip review
    state.move(channel.task_id, 'BLOCKED', blocker='structured_proposal_missing')
    question = pending(state.store, channel.task_id)
    assert answer(state.store, question['question_id'], 'Finish', 'text')
    assert resume_answered(state.store, channel.task_id)
    assert state.context(channel.task_id)['state'] == 'RECOVERING'
    state.move(channel.task_id, 'PLANNING')
    assert state.submit_answer(channel, 'Report r_x: root cause found; checks not run.', 'final')['state'] == 'ANSWER_READY'
    state.move(channel.task_id, 'COMPLETED')
    assert state.context(channel.task_id)['state'] == 'COMPLETED'


def test_only_retries_and_review_fixes_escalate_the_route(machine):
    from cortex_runtime.orchestration.state import attempt_routing, event
    state, channel = machine
    hard = plan(); hard['tasks'][0]['complexity'] = 'high'
    state.submit_plan(channel, hard, 'p'); state.move(channel.task_id, 'EXECUTING')
    node = state.claim_ready(channel.task_id, channel.channel_id)[0]
    with state.store.connection() as db:
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                   (node['assignment_ref'], channel.task_id, 'xa_first', 'worker-debugger', 'ready', 'ready', 0, 'now', 'now')); db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first') == (False, 'high', 'auto', None, None)
    with state.store.connection() as db:
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED', dict(node=node['node_id'], revision=1, retry=1)); db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first') == (True, 'high', 'auto', None, None)
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET node_id='_fix_0_N0' WHERE task_id=?", (channel.task_id,))
        db.execute("DELETE FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'"); db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first') == (True, 'high', 'auto', None, None)  # a fix keeps its node's complexity


def test_a_planned_provider_preference_reaches_routing(machine):
    from cortex_runtime.orchestration.state import attempt_routing
    state, channel = machine
    preferred = plan(); preferred['tasks'][0].pop('model'); preferred['tasks'][0]['provider'] = 'codex-acp'
    state.submit_plan(channel, preferred, 'p'); state.move(channel.task_id, 'EXECUTING')
    node = state.claim_ready(channel.task_id, channel.channel_id)[0]
    with state.store.connection() as db:
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                   (node['assignment_ref'], channel.task_id, 'xa_first', 'worker-debugger', 'ready', 'ready', 0, 'now', 'now')); db.commit()
        db.execute("UPDATE runtime_nodes SET node_id='_fix_0_N0' WHERE task_id=?", (channel.task_id,)); db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first') == (True, 'normal', 'codex-acp', None, None)  # a fix keeps it too


def test_plan_provider_is_optional_and_bounded(machine):
    state, channel = machine
    bad = plan(); bad['tasks'][0]['provider'] = 'gpt-6-astra'
    with pytest.raises(StoreError, match='invalid_plan_node'):
        state.submit_plan(channel, bad, 'bad')
    ok = plan(); ok['tasks'][0]['provider'] = 'claude-agent-acp'
    state.submit_plan(channel, ok, 'ok')


def test_plan_complexity_is_optional_and_bounded(machine):
    state, channel = machine
    bad = plan(); bad['tasks'][0]['complexity'] = 'extreme'
    with pytest.raises(StoreError, match='invalid_plan_node'):
        state.submit_plan(channel, bad, 'bad')
    light = plan(); light['tasks'][0]['complexity'] = 'low'
    state.submit_plan(channel, light, 'ok')


def test_a_task_that_failed_before_stops_became_questions_reopens_on_a_new_message(machine):
    from cortex_runtime.orchestration.policy import steer, apply_pending_steering
    state, channel = machine
    state.move(channel.task_id, 'FAILED', blocker='coordinator_turn_failed:refusal')
    assert steer(state.store, channel.task_id, 'Try again, please.', 'user:test:1')
    assert apply_pending_steering(state.store, channel.task_id)
    assert state.context(channel.task_id)['state'] == 'PLANNING'


def test_a_failed_verification_goes_back_to_the_coordinator_to_repair(machine):
    """A wrong check command stopped the task; it must come back to the
    coordinator with its output, and a corrected command verifies again
    without redoing the reviewed work."""
    import json
    from cortex_runtime.orchestration.state import MAX_VERIFICATION_REPAIRS
    state, channel = machine
    state.submit_plan(channel, plan(), 'p'); state.move(channel.task_id, 'EXECUTING')
    digest = 'd' * 64
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed', result_json=? WHERE task_id=?",
                   (json.dumps(dict(project_digest=digest)), channel.task_id))
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE task_id=?", (channel.task_id,))
        db.commit()
    bad = dict(argv=['python', '-m', 'pytest'], exit_code=127, complete=True, revision_digest=digest)
    for _ in range(MAX_VERIFICATION_REPAIRS):
        state.move(channel.task_id, 'REVIEWING'); state.move(channel.task_id, 'VERIFYING')
        assert state.record_verification(channel.task_id, [bad], digest) is False
        assert state.context(channel.task_id)['state'] == 'PLANNING'
        fixed = [['python3', '-m', 'pytest', '-q']]
        assert state.submit_verification(channel, fixed, f'fix-{_}')['state'] == 'VERIFYING'
        assert state.context(channel.task_id)['plan']['verification'] == fixed
        state.move(channel.task_id, 'RECOVERING'); state.move(channel.task_id, 'EXECUTING')
    state.move(channel.task_id, 'REVIEWING'); state.move(channel.task_id, 'VERIFYING')
    state.record_verification(channel.task_id, [bad], digest)
    context = state.context(channel.task_id)
    # Only now the user is asked -- by the coordinator, concretely.
    assert (context['state'], context['ask_user_required']) == ('RECOVERING', 'verification_failed')


def test_a_failed_worker_returns_to_the_coordinator_a_bounded_number_of_times(machine):
    from cortex_runtime.orchestration.state import MAX_COORDINATOR_RETURNS
    state, channel = machine
    state.submit_plan(channel, plan(), 'p'); state.move(channel.task_id, 'EXECUTING')
    for _ in range(MAX_COORDINATOR_RETURNS):
        assert state.return_to_coordinator(channel.task_id, 'worker_failed')
        assert state.context(channel.task_id)['state'] == 'RECOVERING'
        state.move(channel.task_id, 'EXECUTING')
    assert not state.return_to_coordinator(channel.task_id, 'worker_failed')  # then the user is asked


def test_a_decision_left_after_verification_keeps_the_task_open_until_the_user_answers(machine):
    """A proposed plan used to go out as synthesis text, and the task showed
    as finished while the user still had to approve it."""
    import json
    from cortex_runtime.orchestration.questions import pending, request, resume_answered
    from cortex_runtime.acp.questions_store import answer
    state, channel = machine
    state.submit_plan(channel, plan(), 'p'); state.move(channel.task_id, 'EXECUTING')
    digest = 'd' * 64
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed', stage='review', result_json=? WHERE task_id=?",
                   (json.dumps(dict(project_digest=digest)), channel.task_id))
        db.commit()
    state.move(channel.task_id, 'REVIEWING'); state.move(channel.task_id, 'VERIFYING')
    ok = dict(argv=['python3', '-B', '-m', 'pytest', '-q'], exit_code=0, complete=True, revision_digest=digest)
    assert state.record_verification(channel.task_id, [ok], digest)
    request(state.store, channel, dict(question='Approve the plan above?', options=['Approve', 'Finish'], request_key='q'))
    assert state.context(channel.task_id)['state'] == 'WAITING_USER'
    assert answer(state.store, pending(state.store, channel.task_id)['question_id'], 'Approve', 'text')
    assert resume_answered(state.store, channel.task_id)
    # Planning again: the approved work is a new plan revision ...
    assert state.context(channel.task_id)['state'] == 'PLANNING'
    assert state.submit_plan(channel, plan(2), 'approved')['revision'] == 2
    # ... and a user who chose to finish lets the coordinator close it.
    state.move(channel.task_id, 'EXECUTING')
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed', stage='review', result_json=? WHERE task_id=?",
                   (json.dumps(dict(project_digest=digest)), channel.task_id))
        db.commit()
    state.move(channel.task_id, 'REVIEWING'); state.move(channel.task_id, 'VERIFYING')
    assert state.record_verification(channel.task_id, [ok], digest)
    request(state.store, channel, dict(question='Anything else?', options=['Finish'], request_key='q2'))
    assert answer(state.store, pending(state.store, channel.task_id)['question_id'], 'Finish', 'text')
    assert resume_answered(state.store, channel.task_id)
    assert state.submit_answer(channel, 'Done as approved.', 'close')['state'] == 'ANSWER_READY'



def test_retry_uses_prior_audit_even_after_assignment_ref_changes(machine):
    from cortex_runtime.orchestration.state import attempt_routing, event
    from cortex_runtime.routing.store import record_attempt_audit
    state, channel = machine
    proposal = plan()
    proposal['tasks'][0].update(model='claude-agent-acp', model_reason='Review the written contract.')
    receipt = state.submit_plan(channel, proposal, 'hint-plan')
    assert receipt['model_hints'][0]['model'] == 'claude-agent-acp'
    state.move(channel.task_id, 'EXECUTING')
    original = state.claim_ready(channel.task_id, channel.channel_id)[0]
    with state.store.connection() as db:
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                   (original['assignment_ref'], channel.task_id, 'xa_original', 'worker-backend-dev', 'ready', 'ready', 0, 'now', 'now'))
        db.commit()
    record_attempt_audit(state.store, attempt_id='xa_original', task_id=channel.task_id,
                         role='worker-backend-dev', engine='codex-acp', connection_id='cn_old',
                         model='gpt-5.6-terra', params_fingerprint='hash',
                         config_revision_id=None, decision='authorized', detail={})
    with state.store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        db.execute("UPDATE runtime_nodes SET status='pending',assignment_ref=NULL WHERE task_id=? AND node_id='N0'",
                   (channel.task_id,))
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED',
              dict(node='N0', revision=1, route_failure=False))
        db.commit()
    current = state.claim_ready(channel.task_id, channel.channel_id)[0]
    with state.store.connection() as db:
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                   (current['assignment_ref'], channel.task_id, 'xa_current', 'worker-backend-dev', 'ready', 'ready', 0, 'now', 'now'))
        db.commit()
    routed = attempt_routing(state.store, channel.task_id, 'xa_current')
    assert routed.escalate is True
    assert routed.model_hint == 'claude-agent-acp'
    assert routed.previous_engine == 'codex-acp'



def test_exact_model_hint_requires_live_menu_and_reports_tier_mismatch(machine, monkeypatch):
    from cortex_runtime.routing import auto_route
    state, channel = machine
    proposal = plan()
    proposal['tasks'][0].update(model='codex-acp/gpt-6-astra',
                                model_reason='The coordinator suspects a hard implementation.')
    monkeypatch.setattr(auto_route, 'route_menu', lambda *_: {
        'tiers': {'flagship': [{'route': 'codex-acp/gpt-6-astra'}],
                  'deep': [], 'standard': [], 'light': []}})
    receipt = state.submit_plan(channel, proposal, 'hint-preview')
    assert receipt['model_hints'] == [{
        'node': 'N0', 'model': 'codex-acp/gpt-6-astra',
        'tier_now': 'standard', 'preview_status': 'ignored:tier_mismatch'}]


def test_a_luna_hint_on_standard_development_work_is_not_a_tier_mismatch(machine, monkeypatch):
    from cortex_runtime.routing import auto_route
    state, channel = machine
    proposal = plan(2)
    for node in proposal['tasks']:
        node.update(model='codex-acp/gpt-6-luna', model_reason='Cheap and strong enough.')
    proposal['tasks'][1]['complexity'] = 'high'
    monkeypatch.setattr(auto_route, 'route_menu', lambda *_: {
        'tiers': {'flagship': [], 'deep': [], 'standard': [],
                  'light': [{'route': 'codex-acp/gpt-6-luna'}]}})
    receipt = state.submit_plan(channel, proposal, 'luna-preview')
    assert [(hint['tier_now'], hint['preview_status']) for hint in receipt['model_hints']] == [
        ('standard', 'pending_dispatch'), ('deep', 'ignored:tier_mismatch')]


def test_unknown_exact_model_and_missing_reason_are_rejected(machine, monkeypatch):
    from cortex_runtime.routing import auto_route
    state, channel = machine
    proposal = plan()
    proposal['tasks'][0]['model'] = 'codex-acp/gpt-6-astra'
    with pytest.raises(StoreError, match='model_reason_required'):
        state.submit_plan(channel, proposal, 'missing-reason')
    proposal['tasks'][0]['model_reason'] = 'This is a hard implementation.'
    monkeypatch.setattr(auto_route, 'route_menu', lambda *_: {
        'tiers': {'flagship': [], 'deep': [], 'standard': [], 'light': []}})
    with pytest.raises(StoreError, match='unknown_model_hint'):
        state.submit_plan(channel, proposal, 'missing-model')
    assert state.context(channel.task_id)['state'] == 'PLANNING'


def _finish_execution(state, task_id, **result):
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=? AND status IN ('pending','running')",
                   (json.dumps(dict(dict(report_ref='r', summary='done', checks=[]), **result)), task_id))
        db.commit()


def _events(state, task_id, kind):
    with state.store.connection() as db:
        return [json.loads(r[0]) for r in db.execute(
            'SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id', (task_id, kind))]


def _review_nodes(state, task_id):
    return [n for n in state.context(task_id)['nodes'] if n['stage'] == 'review']


def test_planned_work_that_changed_nothing_completes_without_review(machine):
    """Advice or investigation: the runtime observes an unchanged project and
    goes straight to verification, recording why no review was scheduled."""
    state, channel = machine
    state.submit_plan(channel, plan(), 'advice'); state.move(channel.task_id, 'EXECUTING')
    baseline = _events(state, channel.task_id, 'EXECUTION_BASELINE')
    assert [(b['revision'], b['kind']) for b in baseline] == [(1, 'observed')]
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    context = state.context(channel.task_id)
    assert context['state'] == 'VERIFYING' and _review_nodes(state, channel.task_id) == []
    waiver = context['review_not_required']
    assert waiver['reason'] == 'no_project_changes' and waiver['reference'] == 'execution_baseline'
    assert waiver['revision_digest'] == baseline[0]['revision_digest'] and waiver['revision'] == 1
    # Verification binds to that observation (the composed advice scenario
    # also passes the completion gate).
    assert state.record_verification(channel.task_id, [dict(argv=['python3', '-B', '-m', 'pytest', '-q'], exit_code=0,
                                     complete=True, revision_digest=waiver['revision_digest'])], waiver['revision_digest'])
    assert _events(state, channel.task_id, 'REVIEW_STARTED') == []


def test_a_model_cannot_skip_review_of_changed_work(machine, tmp_path):
    state, channel = machine
    state.submit_plan(channel, plan(), 'change'); state.move(channel.task_id, 'EXECUTING')
    with pytest.raises(StoreError, match='invalid_state_transition'):
        state.move(channel.task_id, 'VERIFYING')
    (tmp_path / 'feature.py').write_text('changed = True\n')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    assert state.context(channel.task_id)['state'] == 'REVIEWING'
    assert [n['node_id'] for n in _review_nodes(state, channel.task_id)] == ['_review_1']
    assert _events(state, channel.task_id, 'REVIEW_NOT_REQUIRED') == []
    assert state.context(channel.task_id)['review_not_required'] is None


def test_a_fix_cycle_is_always_reviewed_again(machine, tmp_path):
    state, channel = machine
    state.submit_plan(channel, plan(), 'change'); state.move(channel.task_id, 'EXECUTING')
    (tmp_path / 'feature.py').write_text('changed = True\n')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    _finish_execution(state, channel.task_id, verdict='REQUEST_CHANGES', project_digest='x' * 64)
    state.add_fix(channel.task_id, dict(node_id='_review_1', result=dict(summary='Fix it')))
    assert state.context(channel.task_id)['state'] == 'EXECUTING'
    (tmp_path / 'feature.py').write_text('changed = "fixed"\n')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    assert state.context(channel.task_id)['state'] == 'REVIEWING'
    assert [n['node_id'] for n in _review_nodes(state, channel.task_id)] == ['_review_1', '_review_2']
    assert _events(state, channel.task_id, 'REVIEW_NOT_REQUIRED') == []


def test_a_fix_cycle_without_new_changes_is_still_reviewed(machine, tmp_path):
    state, channel = machine
    state.submit_plan(channel, plan(), 'change'); state.move(channel.task_id, 'EXECUTING')
    (tmp_path / 'feature.py').write_text('changed = True\n')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    _finish_execution(state, channel.task_id, verdict='REQUEST_CHANGES', project_digest='x' * 64)
    state.add_fix(channel.task_id, dict(node_id='_review_1', result=dict(summary='Fix it')))
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    assert [n['node_id'] for n in _review_nodes(state, channel.task_id)] == ['_review_1', '_review_2']


def test_an_earlier_revisions_unreviewed_change_still_requires_review(machine, tmp_path):
    """A worker changed files, then the task returned to the coordinator
    before review; a later investigation-only plan cannot hide that change."""
    state, channel = machine
    state.submit_plan(channel, plan(), 'first'); state.move(channel.task_id, 'EXECUTING')
    (tmp_path / 'feature.py').write_text('changed = True\n')
    assert state.return_to_coordinator(channel.task_id, 'worker_failed')
    from cortex_runtime.orchestration.policy import resume_planning
    resume_planning(state.store, channel.task_id)
    state.submit_plan(channel, plan(), 'second'); state.move(channel.task_id, 'EXECUTING')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    assert state.context(channel.task_id)['state'] == 'REVIEWING'


def test_a_repair_revision_after_an_approved_review(machine, tmp_path):
    """After an approved review, a new plan that changes nothing more is not
    reviewed again; one that changes the project is."""
    state, channel = machine
    state.submit_plan(channel, plan(), 'first'); state.move(channel.task_id, 'EXECUTING')
    (tmp_path / 'feature.py').write_text('changed = True\n')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    from cortex_runtime.delegation.revision import capture_revision
    approved = capture_revision(str(tmp_path))['dirty_digest']
    _finish_execution(state, channel.task_id, verdict='APPROVE', project_digest=approved)
    state.move(channel.task_id, 'VERIFYING')
    assert state.record_verification(channel.task_id, [dict(argv=['false'], exit_code=1, complete=True,
                                     revision_digest=approved)], approved) is False
    assert state.context(channel.task_id)['state'] == 'PLANNING'
    state.submit_plan(channel, plan(), 'inspect'); state.move(channel.task_id, 'EXECUTING')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    waiver = state.context(channel.task_id)['review_not_required']
    assert waiver['reason'] == 'no_changes_since_approved_review' and waiver['reference_revision'] == 1
    # Bound to the approved digest: a failing check returns to the coordinator, never a binding stop.
    assert state.record_verification(channel.task_id, [], approved) is False
    assert state.context(channel.task_id)['state'] == 'PLANNING'
    state.submit_plan(channel, plan(), 'repair'); state.move(channel.task_id, 'EXECUTING')
    (tmp_path / 'feature.py').write_text('changed = "repaired"\n')
    _finish_execution(state, channel.task_id)
    state.add_review(channel.task_id)
    assert state.context(channel.task_id)['state'] == 'REVIEWING'


def test_an_unobservable_project_always_requires_review(machine, monkeypatch):
    from cortex_runtime.delegation import revision
    state, channel = machine
    state.submit_plan(channel, plan(), 'advice'); state.move(channel.task_id, 'EXECUTING')
    _finish_execution(state, channel.task_id)
    monkeypatch.setattr(revision, 'capture_revision',
                        lambda root: dict(kind='unavailable', head=None, dirty_digest=None, detail='cap', manifest=None))
    state.add_review(channel.task_id)
    assert state.context(channel.task_id)['state'] == 'REVIEWING'
