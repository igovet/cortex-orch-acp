"""Consultation is escalation-only and the runtime builds the packet (D8).

An escalation-created senior consultant reads no project source, so its
brief must carry the escalated revision's published reports and their
versions; it is offered no access or question route, and a plan can never
name it directly.
"""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.contracts import NODE, REQUESTS
from cortex_runtime.orchestration.policy import resume_planning
from cortex_runtime.orchestration.state import TaskMachine, validate_plan
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel


def _plan(*ids, role='backend-dev'):
    return dict(tasks=[dict(id=key, role=role, objective=f'Implement {key}.', dependencies=[],
                            scope=[f'src/{key}/**'], model='auto') for key in ids],
                verification=[['true']])


def _publish(store, channel, key, artifacts=()):
    draft = store.call('create_draft', dict(template='general', request_key='draft-' + key),
                       channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
    Path(draft['draft_path']).write_text(draft['required_first_line'] + f'\n\n# {key}\n\nObserved {key}.\n')
    receipt = store.call('write_report', dict(draft_id=draft['draft_id'], title=key, summary=key, author='worker',
                                              request_key='report-' + key, artifacts=list(artifacts)),
                         channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
    return receipt['report_id']


def _digest(store, task_id, report_id):
    """The published report's current SHA-256, as read_report returns it."""
    with store.connection() as db:
        return db.execute('SELECT e.digest FROM editions e JOIN reports r ON r.id=e.report_id '
                          'WHERE r.task_id=? AND r.id=? ORDER BY e.sequence DESC LIMIT 1',
                          (task_id, report_id)).fetchone()[0]


@pytest.fixture
def escalated(tmp_path):
    """Revision 1 ran two tasks; A finished with a report, B did not. A
    worker then escalated with a question and named one more report."""
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='consultant-packet', event_id='one'), 'Implement', 'owner')
    machine = TaskMachine(store)
    machine.create(boot.task_id, 'Implement')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    assert mcp_tools.dispatch(store, coordinator, 'plan_submit',
                              dict(plan=_plan('A', 'B'), request_key='first'))['revision'] == 1
    finished = _publish(store, coordinator, 'finished', [dict(reference='src/A/core.py', version='abc123')])
    named = _publish(store, coordinator, 'named')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=? AND revision=1 AND node_id='A'",
                   (json.dumps(dict(report_ref=finished, checks=[])), boot.task_id))
        db.commit()
    missing = 'r_' + '0' * 12
    receipt = mcp_tools.dispatch(store, coordinator, 'escalation_request', dict(
        reason='Two designs remain and the evidence conflicts.',
        question='Which design keeps the review independent?',
        report_ids=[named, finished, missing], request_key='escalate'))
    assert receipt['decision'] == 'senior_consultation'
    resume_planning(store, boot.task_id)
    accepted = mcp_tools.dispatch(store, coordinator, 'plan_submit', dict(plan=_plan('C'), request_key='second'))
    assert accepted['revision'] == 2
    return SimpleNamespace(store=store, machine=machine, coordinator=coordinator, task_id=boot.task_id,
                           finished=finished, named=named, missing=missing)


def test_escalation_creates_a_consultant_whose_packet_names_reports_and_versions(escalated):
    store, task_id = escalated.store, escalated.task_id
    context = escalated.machine.context(task_id)
    consultant = next(node for node in context['nodes'] if node['node_id'] == '_consultation')
    assert consultant['role'] == 'worker-senior-consultant'
    assert 'Which design keeps the review independent?' in consultant['objective']
    assert 'Two designs remain and the evidence conflicts.' in consultant['objective']
    later = next(node for node in context['nodes'] if node['node_id'] == 'C')
    assert '_consultation' in later['dependencies']

    packet = escalated.machine.consultation_reports(task_id, 2)
    # Named reports first, then the escalated revision's results; a report
    # both named and recorded appears once; an unknown id stays visible.
    assert packet == [
        dict(node='escalation_request', report_ref=escalated.named,
             sha256=_digest(store, task_id, escalated.named), artifacts=[]),
        dict(node='escalation_request', report_ref=escalated.finished,
             sha256=_digest(store, task_id, escalated.finished),
             artifacts=[dict(reference='src/A/core.py', version='abc123')]),
        dict(node='escalation_request', report_ref=escalated.missing, available=False),
    ]
    assert escalated.machine.consultation_reports(task_id, 1) == []


def test_recorded_results_join_the_packet_without_named_reports(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='consultant-packet', event_id='two'), 'Implement', 'owner')
    machine = TaskMachine(store)
    machine.create(boot.task_id, 'Implement')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    mcp_tools.dispatch(store, coordinator, 'plan_submit', dict(plan=_plan('A'), request_key='first'))
    finished = _publish(store, coordinator, 'finished')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=? AND node_id='A'",
                   (json.dumps(dict(report_ref=finished, checks=[])), boot.task_id))
        db.commit()
    mcp_tools.dispatch(store, coordinator, 'escalation_request',
                       dict(reason='The fix keeps failing review.', request_key='escalate'))
    resume_planning(store, boot.task_id)
    mcp_tools.dispatch(store, coordinator, 'plan_submit', dict(plan=_plan('C'), request_key='second'))
    consultant = next(n for n in machine.context(boot.task_id)['nodes'] if n['node_id'] == '_consultation')
    assert consultant['objective'] == 'The fix keeps failing review.'
    assert machine.consultation_reports(boot.task_id, 2) == [
        dict(node='A', report_ref=finished, sha256=_digest(store, boot.task_id, finished), artifacts=[])]


def test_consultant_brief_carries_the_runtime_built_dependency_reports(escalated, monkeypatch):
    from cortex_runtime.orchestration import controller, scheduler
    dispatched = []
    monkeypatch.setattr(scheduler, 'dispatch_assignment',
                        lambda store, channel, request: dispatched.append(request))
    machine, task_id = escalated.machine, escalated.task_id
    machine.move(task_id, 'EXECUTING')
    controller._dispatch_ready(SimpleNamespace(_store=escalated.store), machine, task_id,
                               escalated.coordinator, machine.context(task_id), None)
    # The later task waits for the consultation; only the consultant starts.
    [request] = dispatched
    assert request['role'] == 'worker-senior-consultant'
    brief = json.loads(request['brief'])
    assert brief['dependency_reports'] == machine.consultation_reports(task_id, 2)
    assert [entry['report_ref'] for entry in brief['dependency_reports']] == [
        escalated.named, escalated.finished, escalated.missing]
    assert all('sha256' in entry for entry in brief['dependency_reports'][:2])


def test_a_large_packet_bounds_itself_explicitly(tmp_path):
    from cortex_runtime.delegation.provisioning import WORKER_BRIEF_MAX_CHARS
    from cortex_runtime.orchestration.state import consultation_packet, encoded
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='consultant-packet', event_id='three'), 'Implement', 'owner')
    TaskMachine(store).create(boot.task_id, 'Implement')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    big = [dict(reference=f'src/file{i}.py', version='v' * 200) for i in range(40)]
    report = _publish(store, coordinator, 'big', big)
    objective = 'x' * 8000
    with store.connection() as db:
        packet = consultation_packet(db, boot.task_id, 1, dict(reason=objective, report_ids=[report]), objective)
        many = consultation_packet(db, boot.task_id, 1,
                                   dict(reason=objective, report_ids=[f'r_{i:012x}' for i in range(400)]), objective)
    # Artifact lists give way first; read_report still returns them.
    assert packet == [dict(node='escalation_request', report_ref=report,
                           sha256=_digest(store, boot.task_id, report), artifacts_omitted=40)]
    # Trailing entries give way next, counted, never silently dropped.
    assert many[-1]['omitted_reports'] == 400 - (len(many) - 1)
    assert len(encoded(dict(objective=objective, scope=['.'], dependency_reports=many))) < WORKER_BRIEF_MAX_CHARS


@pytest.mark.parametrize('role', ['senior-consultant', 'worker-senior-consultant'])
def test_a_plan_never_names_the_consultant(role):
    proposal = _plan('A', role=role)
    with pytest.raises(StoreError, match='consultant_requires_escalation') as error:
        validate_plan(proposal)
    assert 'escalation_request' in error.value.correction
    assert 'senior-consultant' not in NODE['properties']['role']['enum']


def test_escalation_request_advertises_the_packet_fields():
    schema = REQUESTS['escalation_request']
    assert schema['required'] == ['reason', 'request_key']
    assert {'question', 'report_ids'} <= set(schema['properties'])
    assert schema['properties']['report_ids']['items']['pattern'] == r'^r_[0-9a-f]{12}$'
    assert 'runtime builds the consultation packet' in schema['properties']['report_ids']['description']


def test_consultant_tool_definitions_lack_access_and_question_routes():
    def offered(role_id):
        return {tool['name'] for tool in mcp_tools.tool_definitions('worker', role_id=role_id, stage='execution')}
    consultant = offered('worker-senior-consultant')
    assert not consultant & {'access_request', 'ask_question', 'command_run', 'project_read', 'project_git'}
    assert {'result_submit', 'read_skill'} <= consultant
    assert {'access_request', 'ask_question'} <= offered('worker-architect')


def test_consultant_channel_is_refused_access_and_question_routes(tmp_path):
    from runtime_scenarios import provision_fixture
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='consultant-packet', event_id='four'), 'Implement', 'owner')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    consultant, _ = provision_fixture(store, coordinator, key='consultant', role='senior-consultant')
    for operation, args in (
            ('access_request', dict(paths=['src/x.py'], reason='Need it.', request_key='access')),
            ('ask_question', dict(question='Which one?', options=['One', 'Two'], request_key='question'))):
        with pytest.raises(StoreError, match='wrong_channel_role') as error:
            mcp_tools.dispatch(store, consultant, operation, args)
        assert 'limitation' in error.value.correction


def test_consultant_prompt_records_missing_evidence_and_asks_nothing():
    from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt
    consultant = cortex_system_prompt('worker', None, environ={}, access_class='consultant')
    worker = cortex_system_prompt('worker', None, environ={})
    assert "ask through the runtime's question tool" not in consultant
    assert 'access_request' not in consultant
    assert 'record it in your report as a limitation' in consultant
    assert 'you ask the user nothing' in consultant
    assert "ask through the runtime's question tool" in worker and '`access_request`' in worker
    assert '@@' not in consultant + worker
