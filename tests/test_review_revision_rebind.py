"""A review whose project changed under it is re-bound, never stuck.

2026-09-28: a code reviewer's result was refused three times with
review_project_revision_changed while another writer edited the same
checkout. The REVIEW_STARTED binding was never refreshed, so after one change
every later submission of that review was refused for good.
"""
import json

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.state import MAX_REVIEW_REBINDS, TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report


def setup_review(tmp_path):
    (tmp_path / 'feature.py').write_text('changed = True\n')
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='review', event_id='one'), 'Review', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator, key='reviewer', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (assignment,))
        db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task.task_id,))
        db.commit()
    machine = TaskMachine(store)
    started = machine.start_review(task.task_id, assignment)
    report = publish_fixture_report(store, worker)
    return store, task, worker, assignment, report, machine, started


def _args(report, key):
    return dict(report=report, summary='Reviewed', checks=[], verdict='APPROVE', request_key=key)


def _bindings(store, task_id):
    with store.connection() as db:
        return [json.loads(r[0]) for r in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' ORDER BY id", (task_id,))]


def _published(store, task, worker):
    with store.connection() as db:
        return db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='worker_result'",
                          (task.task_id, worker.attempt_id)).fetchone()[0]


def _node(machine, task, assignment):
    return next(n for n in machine.context(task.task_id)['nodes'] if n['assignment_ref'] == assignment)


def test_unchanged_project_records_the_review_bound_to_its_start(tmp_path):
    store, task, worker, assignment, report, machine, started = setup_review(tmp_path)
    assert mcp_tools.dispatch(store, worker, 'result_submit', _args(report, 'r1'))['recorded']
    node = _node(machine, task, assignment)
    assert node['status'] == 'completed' and node['result']['project_digest'] == started


def test_a_changed_project_rebinds_the_review_and_a_new_submission_succeeds(tmp_path):
    store, task, worker, assignment, report, machine, started = setup_review(tmp_path)
    (tmp_path / 'feature.py').write_text('changed = "by another writer"\n')
    with pytest.raises(StoreError, match='review_project_revision_changed'):
        mcp_tools.dispatch(store, worker, 'result_submit', _args(report, 'r1'))
    # Refused before publication: no worker_result, node still running.
    assert _published(store, task, worker) == 0
    assert _node(machine, task, assignment)['status'] == 'running'
    bindings = _bindings(store, task.task_id)
    assert len(bindings) == 2 and bindings[1]['rebound_from'] == started and bindings[1]['rebind'] == 1
    assert bindings[1]['refused_request_key'] == 'r1' and bindings[1]['revision_digest'] != started
    # The refused request never records a verdict, even once re-bound.
    with pytest.raises(StoreError, match='review_resubmit_required'):
        mcp_tools.dispatch(store, worker, 'result_submit', _args(report, 'r1'))
    # After re-checking, a new request binds to the current revision.
    assert mcp_tools.dispatch(store, worker, 'result_submit', _args(report, 'r2'))['recorded']
    node = _node(machine, task, assignment)
    assert node['status'] == 'completed' and node['result']['project_digest'] == bindings[1]['revision_digest']


def test_a_project_that_keeps_changing_is_refused_after_the_bound(tmp_path):
    store, task, worker, assignment, report, machine, _ = setup_review(tmp_path)
    for index in range(MAX_REVIEW_REBINDS):
        (tmp_path / 'feature.py').write_text(f'changed = {index}\n')
        with pytest.raises(StoreError, match='review_project_revision_changed'):
            mcp_tools.dispatch(store, worker, 'result_submit', _args(report, f'r{index}'))
    (tmp_path / 'feature.py').write_text('changed = "again"\n')
    with pytest.raises(StoreError, match='review_project_revision_unstable'):
        mcp_tools.dispatch(store, worker, 'result_submit', _args(report, 'last'))
    assert len(_bindings(store, task.task_id)) == 1 + MAX_REVIEW_REBINDS
    assert _published(store, task, worker) == 0
    assert _node(machine, task, assignment)['status'] == 'running'
