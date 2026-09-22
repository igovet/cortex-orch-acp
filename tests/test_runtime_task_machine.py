from copy import deepcopy
from types import SimpleNamespace
import concurrent.futures

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
                           dependencies=[], scope=[f'src/part{i}/**', f'tests/part{i}/**']) for i in range(count)],
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


@pytest.mark.parametrize("scenario", ["success", "fix", "review_block", "verification_fail", "synthesis_drift", "post_review_drift", "elicitation", "text_question", "worker_question", "replan", "scope", "escalation", "result_recovery", "result_correction"])
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
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Result reviewed and verified.'))
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
                if provision['role_id'] == 'worker-code-reviewer':
                    args['verdict'] = ('BLOCK' if scenario == 'review_block' else 'REQUEST_CHANGES' if scenario == 'fix' and not reviews else 'APPROVE')
                    reviews.append(args['verdict'])
                if scenario == 'result_correction':
                    invalid = dict(args, checks=[dict(name='Unbound passing claim', outcome='passed')])
                    with pytest.raises(StoreError, match='check_evidence_required'):
                        mcp_tools.dispatch(store, channel, 'result_submit', invalid)
                    with store.connection() as db:
                        assert db.execute('SELECT 1 FROM runtime_requests WHERE actor=? AND request_key=?',
                                          (channel.channel_id, args['request_key'])).fetchone() is None
                        assert db.execute("SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='worker_result'",
                                          (channel.task_id, channel.attempt_id)).fetchone() is None
                    observed = mcp_tools.dispatch(store, channel, 'command_run',
                        dict(argv=['python3', '-B', '-c', 'print("observed check")'], request_key='verify-real'))
                    args['checks'] = [dict(name='Actual check', outcome='passed', tool_call_ref=observed['tool_call_ref'])]
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
                    mcp_tools.dispatch(store, channel, 'result_submit', args)
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


def test_user_steering_revokes_old_scopes_before_replanning(machine):
    from cortex_runtime.orchestration.policy import steer,resume_planning
    state,ch=machine
    state.submit_plan(ch,plan(),'old');state.move(ch.task_id,'EXECUTING')
    state.claim_ready(ch.task_id,ch.channel_id)
    assert steer(state.store,ch.task_id,'Не меняй database','user:one')
    assert not steer(state.store,ch.task_id,'Не меняй database','user:one')
    context=state.context(ch.task_id)
    assert context['state']=='RECOVERING'
    assert context['nodes'][0]['status']=='cancelled'
    assert context['policy']['deny_write']==['database']
    resume_planning(state.store,ch.task_id)
    denied=plan();denied['tasks'][0]['scope']=['database/schema/**']
    with pytest.raises(StoreError,match='scope_conflicts_with_user_policy'):
        state.submit_plan(ch,denied,'denied')
    state.submit_plan(ch,plan(),'new')
    assert state.context(ch.task_id)['revision']==2


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
    assert attempt_routing(state.store, channel.task_id, 'xa_first') == (False, 'high')
    with state.store.connection() as db:
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED', dict(node=node['node_id'], revision=1, retry=1)); db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first') == (True, 'high')
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET node_id='_fix_0_N0' WHERE task_id=?", (channel.task_id,))
        db.execute("DELETE FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'"); db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first') == (True, 'high')  # a fix keeps its node's complexity


def test_plan_complexity_is_optional_and_bounded(machine):
    state, channel = machine
    bad = plan(); bad['tasks'][0]['complexity'] = 'extreme'
    with pytest.raises(StoreError, match='invalid_plan_node'):
        state.submit_plan(channel, bad, 'bad')
    light = plan(); light['tasks'][0]['complexity'] = 'low'
    state.submit_plan(channel, light, 'ok')


def test_a_task_that_failed_before_stops_became_questions_reopens_on_a_new_message(machine):
    from cortex_runtime.orchestration.policy import steer
    state, channel = machine
    state.move(channel.task_id, 'FAILED', blocker='coordinator_turn_failed:refusal')
    assert steer(state.store, channel.task_id, 'Try again, please.', 'user:test:1')
    assert state.context(channel.task_id)['state'] == 'RECOVERING'


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
