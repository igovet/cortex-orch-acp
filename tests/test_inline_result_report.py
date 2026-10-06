"""result_submit may carry the report itself (report={title, markdown})
instead of a reference (report={report_ref}).

The runtime publishes an inline report through the ordinary
create_draft/write_report path as the worker's own draft, bound to the
result's request_key: a retry after any crash reuses that one report, and no
result becomes visible before its report is published.
"""
import hashlib
import json

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts as delegation_contracts
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import gateway
from cortex_runtime.orchestration.recovery import reconcile_results
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report

MARKDOWN = '# Inline report\n\nConclusion: the change works.\n\n## Verification\n\nNone run.\n'


def setup_worker(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='inline', event_id='one'), 'Build', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    return store, task, worker, assignment


def inline_args(markdown=MARKDOWN, key='result', **extra):
    return dict(dict(report=dict(title='Inline result', markdown=markdown), summary='The change works.',
                     checks=[], request_key=key), **extra)


def report_count(store, task_id):
    with store.connection() as db:
        return db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'", (task_id,)).fetchone()[0]


def worker_results(store, task, worker):
    with store.connection() as db:
        rows = db.execute("SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='worker_result'",
                          (task.task_id, worker.attempt_id)).fetchall()
    return [json.loads(row[0]) for row in rows]


def node_of(store, task, assignment):
    return next(n for n in TaskMachine(store).context(task.task_id)['nodes'] if n['assignment_ref'] == assignment)


def report_text(store, task, report_ref):
    with store.connection() as db:
        filename = db.execute('SELECT filename FROM reports WHERE id=?', (report_ref,)).fetchone()[0]
        directory = store._task_directory(db, task.task_id)
    return (directory / filename).read_text()


class Crash(Exception):
    pass


def crash_once(monkeypatch, module, name, *, after=False):
    """Replace module.name so its first call crashes (before or after the real call)."""
    real = getattr(module, name)
    calls = []

    def wrapper(*args, **kwargs):
        calls.append(1)
        if len(calls) == 1 and not after:
            raise Crash(name)
        value = real(*args, **kwargs)
        if len(calls) == 1:
            raise Crash(name)
        return value
    monkeypatch.setattr(module, name, wrapper)
    return calls


def test_inline_report_is_published_as_the_workers_own_report_and_recorded(tmp_path):
    store, task, worker, assignment = setup_worker(tmp_path)
    before = report_count(store, task.task_id)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', inline_args())
    assert receipt['recorded'] and receipt['report_ref'].startswith('r_')
    assert report_count(store, task.task_id) == before + 1
    text = report_text(store, task, receipt['report_ref'])
    assert text.endswith(MARKDOWN) and text.startswith('Cortex draft ID: `d_')
    with store.connection() as db:
        draft = db.execute('SELECT owner_thread_id, template FROM drafts WHERE published_report_id=?',
                           (receipt['report_ref'],)).fetchone()
        request = json.loads(db.execute('SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                        (task.task_id, worker.channel_id, 'result')).fetchone()[0])
    assert draft['owner_thread_id'] == worker.principal_thread_id
    node = node_of(store, task, assignment)
    assert node['status'] == 'completed' and node['result']['report_ref'] == receipt['report_ref']
    assert 'report' not in node['result']
    # The retained request binds the body by digest, never a second copy of it.
    assert request['result']['report'] == dict(title='Inline result',
                                               markdown_sha256=hashlib.sha256(MARKDOWN.encode()).hexdigest())
    assert MARKDOWN not in json.dumps(request)
    assert [r['report_ref'] for r in worker_results(store, task, worker)] == [receipt['report_ref']]


def test_an_exact_retry_returns_the_same_report_and_result(tmp_path):
    store, task, worker, assignment = setup_worker(tmp_path)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', inline_args())
    count = report_count(store, task.task_id)
    assert mcp_tools.dispatch(store, worker, 'result_submit', inline_args()) == receipt
    assert report_count(store, task.task_id) == count
    assert len(worker_results(store, task, worker)) == 1
    with pytest.raises(StoreError, match='request_key_conflict') as rejected:
        mcp_tools.dispatch(store, worker, 'result_submit', inline_args(markdown=MARKDOWN + '\nChanged.\n'))
    assert rejected.value.field == 'arguments.request_key'
    assert 'different operation or payload' in rejected.value.received
    assert 'new key' in rejected.value.correction


@pytest.mark.parametrize('module, name, after', [
    (gateway, 'write_draft', False),                  # draft created, body never written
    (mcp_tools, 'publish_inline_report', True),       # report published, request never updated
    (mcp_tools, '_worker_publish_result', False),     # report bound, result never published
])
def test_a_crash_between_publication_and_result_is_resumed_by_the_retry(tmp_path, monkeypatch, module, name, after):
    store, task, worker, assignment = setup_worker(tmp_path)
    before = report_count(store, task.task_id)
    crash_once(monkeypatch, module, name, after=after)
    with pytest.raises(Crash):
        mcp_tools.dispatch(store, worker, 'result_submit', inline_args())
    # No false completion: nothing is recorded or published as a result.
    assert node_of(store, task, assignment)['status'] == 'running'
    assert worker_results(store, task, worker) == []
    published = report_count(store, task.task_id)
    assert published == before + (1 if after or name == '_worker_publish_result' else 0)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', inline_args())
    # No duplicate: the retry reuses the report already published, if any.
    assert report_count(store, task.task_id) == before + 1
    with store.connection() as db:
        drafts = db.execute('SELECT published_report_id FROM drafts WHERE owner_thread_id=?',
                            (worker.principal_thread_id,)).fetchall()
    assert [row[0] for row in drafts] == [receipt['report_ref']]
    assert [r['report_ref'] for r in worker_results(store, task, worker)] == [receipt['report_ref']]
    assert node_of(store, task, assignment)['result']['report_ref'] == receipt['report_ref']


def test_recovery_records_a_published_inline_result_bound_to_its_own_delivery(tmp_path, monkeypatch):
    store, task, worker, assignment = setup_worker(tmp_path)
    crash_once(monkeypatch, TaskMachine, 'record_result')
    with pytest.raises(Crash):
        mcp_tools.dispatch(store, worker, 'result_submit', inline_args())
    assert node_of(store, task, assignment)['status'] == 'running'
    (published,) = worker_results(store, task, worker)
    reconcile_results(store, task.task_id)
    node = node_of(store, task, assignment)
    assert node['status'] == 'completed' and node['result']['report_ref'] == published['report_ref']


def test_recovery_refuses_an_inline_result_not_bound_to_its_own_delivery(tmp_path, monkeypatch):
    store, task, worker, assignment = setup_worker(tmp_path)
    other = publish_fixture_report(store, worker, 'A different report')
    crash_once(monkeypatch, TaskMachine, 'record_result')
    with pytest.raises(Crash):
        mcp_tools.dispatch(store, worker, 'result_submit', inline_args())
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        row = db.execute('SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                         (task.task_id, worker.channel_id, 'result')).fetchone()
        saved = json.loads(row[0])
        saved['accepted']['report_ref'] = other
        db.execute('UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?',
                   (json.dumps(saved), task.task_id, worker.channel_id, 'result'))
        db.commit()
    with pytest.raises(StoreError, match='result_receipt_digest_mismatch'):
        reconcile_results(store, task.task_id)


@pytest.mark.parametrize('form', ['both', 'missing', 'empty'])
def test_report_is_required_in_exactly_one_form(tmp_path, form):
    store, task, worker, assignment = setup_worker(tmp_path)
    report = publish_fixture_report(store, worker)
    args = inline_args()
    if form == 'both':
        args['report'] = dict(args['report'], report_ref=report)
    elif form == 'missing':
        del args['report']
    else:
        args['report'] = {}
    before = report_count(store, task.task_id)
    with pytest.raises(StoreError, match='invalid_arguments') as caught:
        mcp_tools.dispatch(store, worker, 'result_submit', args)
    assert caught.value.field == 'arguments.report'
    assert report_count(store, task.task_id) == before
    assert worker_results(store, task, worker) == []
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND operation='result_submit'",
                          (task.task_id, worker.channel_id)).fetchone() is None
        row = db.execute("SELECT outcome, detail_json FROM observability_events "
                         "WHERE task_id=? AND category='submission_rejection' ORDER BY created_at DESC LIMIT 1",
                         (task.task_id,)).fetchone()
    assert row['outcome'] == 'invalid_arguments'
    detail = json.loads(row['detail_json'])
    assert detail == {
        'operation': 'result_submit',
        'field': 'arguments.report',
        'fields': ['arguments.report'],
    }


def test_an_inline_report_with_an_unfilled_template_placeholder_is_refused_before_any_result(tmp_path):
    store, task, worker, assignment = setup_worker(tmp_path)
    placeholder = '<!-- Record relevant inputs, actions, observations, and exact changed paths. -->'
    with pytest.raises(StoreError, match='draft_guidance_remaining'):
        mcp_tools.dispatch(store, worker, 'result_submit', inline_args(markdown=f'# Report\n\n{placeholder}\n'))
    assert worker_results(store, task, worker) == []
    assert node_of(store, task, assignment)['status'] == 'running'


def test_the_reference_form_names_the_published_report(tmp_path):
    store, task, worker, assignment = setup_worker(tmp_path)
    report = publish_fixture_report(store, worker)
    args = dict(report=dict(report_ref=report), summary='Observed', checks=[], request_key='result')
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', args)
    assert receipt['recorded'] and receipt['report_ref'] == report
    assert node_of(store, task, assignment)['result']['report_ref'] == report


def test_inline_report_is_advertised_and_validated():
    (tool,) = [t for t in mcp_tools.tool_definitions('worker', role_id='worker-backend-dev', stage='execution')
               if t['name'] == 'result_submit']
    schema = tool['inputSchema']
    assert schema['required'] == ['report', 'summary', 'checks', 'request_key']
    assert 'report_ref' not in schema['properties'] and not {'if', 'then', 'else'} & set(schema)
    report = schema['properties']['report']
    assert set(report['properties']) == {'title', 'markdown', 'report_ref'} and report['additionalProperties'] is False
    assert report['else'] == {'required': ['title', 'markdown']}
    assert 'Required in every result_submit' in report['description'] and 'exactly one form' in report['description']
    assert 'report is required in every call' in tool['description'] and 'report={report_ref:' in tool['description']
    delegation_contracts.validate('worker', 'result_submit', inline_args())
    delegation_contracts.validate('worker', 'result_submit', dict(inline_args(), report=dict(report_ref='r_' + 'a' * 12)))
    for bad in (dict(title='T'), dict(title='T', markdown=''), dict(title='T', markdown='x', draft_id='d_' + 'a' * 12),
                {}, dict(report_ref='r_x'), dict(title='T', markdown='x', report_ref='r_' + 'a' * 12)):
        with pytest.raises(StoreError, match='invalid_arguments'):
            delegation_contracts.validate('worker', 'result_submit', dict(inline_args(), report=bad))
