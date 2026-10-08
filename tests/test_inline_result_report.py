"""result_submit carries its report as one plain string: the complete
Markdown (optionally titled by report_title) or the exact report_id of the
worker's own published report.

A Claude engine emits a top-level string argument verbatim but must
hand-write JSON for object values; the former report={title, markdown} object
made Sonnet 5 workers break the escaping of nested Markdown, and the engine
refused those calls before Cortex received them (incident review
2026-10-08).

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
    return dict(dict(report=markdown, report_title='Inline result', summary='The change works.',
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


@pytest.mark.parametrize('form', ['legacy_inline', 'legacy_reference', 'missing', 'empty', 'whitespace', 'draft_id',
                                  'tool_call_ref', 'malformed_report_id'])
def test_report_is_one_required_plain_string(tmp_path, form):
    store, task, worker, assignment = setup_worker(tmp_path)
    report = publish_fixture_report(store, worker)
    args = inline_args()
    if form == 'legacy_inline':
        args['report'] = dict(title='Inline result', markdown=MARKDOWN)
    elif form == 'legacy_reference':
        args['report'] = dict(report_ref=report)
    elif form == 'missing':
        del args['report']
    else:
        args['report'] = {'empty': '', 'whitespace': ' \n\t ', 'draft_id': 'd_' + 'a' * 12,
                          'tool_call_ref': 'gateway:' + 'a' * 32, 'malformed_report_id': 'r_' + 'g' * 12}[form]
    before = report_count(store, task.task_id)
    with pytest.raises(StoreError, match='invalid_arguments') as caught:
        mcp_tools.dispatch(store, worker, 'result_submit', args)
    assert caught.value.field == 'arguments.report'
    if form.startswith('legacy'):
        assert caught.value.correction.startswith('Send arguments.report as one plain JSON string, not an object')
    if form != 'empty':
        # The one unclaimed own report is named in the correction.
        assert f'report="{report}"' in caught.value.correction
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


@pytest.mark.parametrize('markdown, title', [
    ('Intro line\n\n## Details\n\n# Real title ##\n\nBody.\n', 'Real title'),
    ('## Decision brief\n\nConclusion.\n', 'Decision brief'),
    ('\n\n**Conclusion:** works.\n', '**Conclusion:** works.'),
    ('- first bullet\n', 'first bullet'),
])
def test_an_omitted_report_title_comes_from_the_markdown(tmp_path, markdown, title):
    store, task, worker, assignment = setup_worker(tmp_path)
    args = inline_args(markdown=markdown)
    del args['report_title']
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', args)
    with store.connection() as db:
        stored = db.execute('SELECT title FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1',
                            (receipt['report_ref'],)).fetchone()
        request = json.loads(db.execute('SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                                        (task.task_id, worker.channel_id, 'result')).fetchone()[0])
    assert request['result']['report']['title'] == title
    assert stored['title'] == title
    assert report_text(store, task, receipt['report_ref']).endswith(markdown)


def test_a_long_markdown_report_with_quotes_backslashes_and_fences_is_published_verbatim(tmp_path):
    """The content that broke nested-object escaping (raw newlines, quotes,
    backslashes, code fences, a regex) is an ordinary top-level string."""
    store, task, worker, assignment = setup_worker(tmp_path)
    block = ('## Evidence\n\n```python\npattern = r"ReviveAdsManager::shared\\(\\)|C:\\\\path"\nprint("a \\"quoted\\" value")\n```\n'
             '\n| check | outcome |\n| --- | --- |\n| `grep -n \'a\\|b\'` | passed |\n\tTabbed line\n')
    markdown = '# Hedge-grid results\n\n' + block * 120
    assert len(markdown) > 20000
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', inline_args(markdown=markdown))
    assert report_text(store, task, receipt['report_ref']).endswith(markdown)
    assert node_of(store, task, assignment)['status'] == 'completed'


def test_report_title_is_ignored_with_a_reference(tmp_path):
    store, task, worker, assignment = setup_worker(tmp_path)
    report = publish_fixture_report(store, worker)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', dict(
        report='  ' + report + '\n', report_title='Ignored', summary='Observed', checks=[], request_key='result'))
    assert receipt['report_ref'] == report


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
    args = dict(report=report, summary='Observed', checks=[], request_key='result')
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
    assert report['type'] == 'string' and report['maxLength'] == 1000000
    assert not {'properties', 'if', 'then', 'else', 'oneOf', 'anyOf'} & set(report)
    assert schema['properties']['report_title']['type'] == 'string' and 'report_title' not in schema['required']
    assert 'Required in every result_submit' in report['description'] and 'one plain string, never an object' in report['description']
    assert 'report is required in every call' in tool['description'] and 'one plain string, never an object' in tool['description']
    delegation_contracts.validate('worker', 'result_submit', inline_args())
    delegation_contracts.validate('worker', 'result_submit', dict(inline_args(), report='r_' + 'a' * 12))
    for bad in (dict(title='T'), dict(title='T', markdown='x'), dict(report_ref='r_' + 'a' * 12), {}, '', ['# T']):
        with pytest.raises(StoreError, match='invalid_arguments'):
            delegation_contracts.validate('worker', 'result_submit', dict(inline_args(), report=bad))


def test_no_result_submit_object_or_array_carries_long_free_text():
    """Long free text must be a top-level string: only there do Claude engines
    emit it verbatim instead of hand-escaping it as nested JSON."""
    (tool,) = [t for t in mcp_tools.tool_definitions('worker', role_id='worker-code-reviewer', stage='review')
               if t['name'] == 'result_submit']

    def nested_strings(schema, path):
        if schema.get('type') == 'string':
            yield path, schema.get('maxLength', 0)
        for name, child in (schema.get('properties') or {}).items():
            yield from nested_strings(child, f'{path}.{name}')
        if isinstance(schema.get('items'), dict):
            yield from nested_strings(schema['items'], f'{path}[]')

    nested = [(path, size) for name, child in tool['inputSchema']['properties'].items()
              if child.get('type') in {'object', 'array'} for path, size in nested_strings(child, name)]
    assert nested and all(size <= 4096 for _, size in nested), nested
