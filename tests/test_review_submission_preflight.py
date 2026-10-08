"""No-paid protection for the retained Claude review submission failures."""
import json
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report


def test_submission_guidance_preflights_report_presence_and_json_transport():
    tool = next(t for t in mcp_tools.tool_definitions(
        'worker', role_id='worker-code-reviewer', stage='review')
        if t['name'] == 'result_submit')
    assert 'summary, verdict and findings never replace it' in tool['description']
    assert tool['inputSchema']['required'][0] == 'report'
    assert 'could not be parsed, nothing reached Cortex' in tool['description']
    assert 'draft_write' in tool['description']
    protocol = (Path(__file__).resolve().parents[1] /
                'agent-sources/references/report-publication.md').read_text()
    assert "as the live tool descriptions specify" in protocol
    assert "retry and serialization guidance" in protocol
    assert "A conclusion or verdict alone is not a report" in protocol
    from cortex_runtime.portable.roles import composed_instructions, load_role
    delivered = composed_instructions(load_role('worker-code-reviewer'))
    assert 'required and conditional fields against the live schema' in delivered
    assert protocol.strip() in delivered
    assert 'one plain string, never an object' in tool['description']
    assert 'retry an uncertain call only with the identical key and payload' in tool['description']


@pytest.mark.parametrize('report_form', ['inline', 'reference'])
@pytest.mark.parametrize('omit_report_first', [False, True])
def test_review_first_valid_submission_and_exact_retry(tmp_path, report_form, omit_report_first):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='review', event_id='one'), 'Review', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator, role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (assignment,))
        db.commit()
    machine = TaskMachine(store)
    machine.start_review(task.task_id, assignment)
    with store.connection() as db:
        before = db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'",
                            (task.task_id,)).fetchone()[0]
    args = dict(summary='Independent review.', checks=[], verdict='APPROVE', request_key='review-result')
    if omit_report_first:
        # The observed omission (session 973dbe54) is refused by the plain
        # required list, without a report or accepted request.
        with pytest.raises(StoreError) as caught:
            mcp_tools.dispatch(store, worker, 'result_submit', args)
        assert caught.value.field == 'arguments.report'
        assert caught.value.received == 'missing'
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'",
                              (task.task_id,)).fetchone()[0] == before
            assert db.execute("SELECT 1 FROM runtime_requests WHERE task_id=? AND operation='result_submit'",
                              (task.task_id,)).fetchone() is None
    markdown = '# Review\n\nQuoted "name", path C:\\fixture and code `\\n`.\n\nNo defects.\n'
    if report_form == 'inline':
        args.update(report=markdown, report_title='Review')
    else:
        args['report'] = publish_fixture_report(store, worker, markdown)
    schema = next(t['inputSchema'] for t in mcp_tools.tool_definitions(
        'worker', role_id='worker-code-reviewer', stage='review') if t['name'] == 'result_submit')
    # Encode the object, rather than hand-escaping Markdown into a JSON string.
    transported = json.loads(json.dumps(args))
    Draft202012Validator(schema).validate(transported)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', transported)
    assert receipt['recorded']
    assert mcp_tools.dispatch(store, worker, 'result_submit', transported) == receipt
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'",
                          (task.task_id,)).fetchone()[0] == before + 1
        assert db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='worker_result'",
                          (task.task_id,)).fetchone()[0] == 1
        node = db.execute('SELECT status,result_json FROM runtime_nodes WHERE assignment_ref=?',
                          (assignment,)).fetchone()
    result = json.loads(node['result_json'])
    assert node['status'] == 'completed'
    assert result['verdict'] == 'APPROVE' and result['project_digest'] and result['report_digest']
    with pytest.raises(StoreError, match='request_key_conflict'):
        mcp_tools.dispatch(store, worker, 'result_submit', dict(transported, summary='Changed payload'))
