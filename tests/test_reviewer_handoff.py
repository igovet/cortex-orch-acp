"""Reviewer handoff is bounded context, not new acceptance authority."""
import hashlib
import json

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.revision import capture_revision
from cortex_runtime.orchestration.reviewer_handoff import (
    FACTS_MAX_BYTES, path_metadata, review_runtime_facts,
)
from cortex_runtime.orchestration.state import TaskMachine, _record_execution_baseline
from cortex_runtime.orchestration.worker_brief import build_worker_brief
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture

ARGV = ['python3', '-B', '-c', 'assert 2 + 2 == 4']
REPORT = """# Independent review
## Conclusion
APPROVE; no findings.
## Observed evidence
Inspected the changed source.
## Checks
No additional commands; runtime verification remains required.
## Contradictions
None.
## Limits
No live client evidence.
## Next action
Runtime verification.
"""


def setup(tmp_path):
    (tmp_path / '.gitignore').write_text('/.cortex/\n')
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='handoff', event_id='one'), 'Implement', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    executor, execution_ref = provision_fixture(store, coordinator)
    with store.connection() as db:
        db.execute('UPDATE runtime_tasks SET plan_json=? WHERE task_id=?',
                   (json.dumps(dict(verification=[ARGV])), task.task_id))
        _record_execution_baseline(db, task.task_id)
        db.commit()
    return store, task, coordinator, executor, execution_ref


def reviewer(store, task, coordinator):
    worker, ref = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (ref,))
        db.commit()
    TaskMachine(store).start_review(task.task_id, ref)
    return worker, ref


def test_runtime_facts_use_verified_receipts_not_worker_interpretation(tmp_path, unjailed_gateway_command):
    store, task, coordinator, executor, ref = setup(tmp_path)
    receipt = mcp_tools.dispatch(store, executor, 'command_run', dict(argv=ARGV, request_key='check'))
    mcp_tools.dispatch(store, executor, 'result_submit', dict(
        report='# Executor\n\nSECRET_WORKER_INTERPRETATION',
        summary='SECRET_WORKER_SUMMARY', checks=[
            dict(name='focused test', outcome='passed', tool_call_ref=receipt['tool_call_ref']),
            dict(name='unknown receipt', outcome='passed', tool_call_ref='gateway:' + '0' * 32),
            dict(name='deferred', outcome='not_run', blocker='SECRET_BLOCKER', command=ARGV),
        ], request_key='execution-result'))
    (tmp_path / 'new.py').write_text('value = 1\n')
    worker, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert facts['checks'] == [dict(
        node='fixture_' + ref[:12], name='focused test', outcome='passed', current_outcome='passed',
        closed_by=None, receipt_ref=receipt['tool_call_ref'],
        digest_binding='not established for reviewed digest')]
    assert facts['verification_argv'] == [ARGV]
    assert facts['changed_paths_status'] == 'observed'
    assert facts['changed_paths'] == [dict(path='new.py', before=None, after=dict(
        kind='f', size_bytes=10, sha256=hashlib.sha256(b'value = 1\n').hexdigest()))]
    encoded = json.dumps(facts)
    for excluded in ('SECRET_WORKER', 'SECRET_BLOCKER', 'unknown receipt', 'report_ref', 'summary'):
        assert excluded not in encoded
    assert 'not results' in facts['verification_note']
    assert 'matching author, approved and current digests' in facts['verification_note']
    retained = mcp_tools.dispatch(store, worker, 'read_large_output',
                                 dict(large_output_id=facts['large_output_id'], offset=0))
    assert retained['truncated'] is False


def test_paths_compare_execution_baseline_not_head_and_never_follow_links(tmp_path):
    (tmp_path / 'existing.py').write_text('before')
    (tmp_path / 'deleted.py').write_text('gone')
    store, task, coordinator, executor, ref = setup(tmp_path)
    (tmp_path / 'existing.py').write_text('after')
    (tmp_path / 'deleted.py').unlink()
    (tmp_path / 'link').symlink_to('missing-target')
    _, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    paths = {entry['path']: entry for entry in facts['changed_paths']}
    assert paths['existing.py']['before']['size_bytes'] == 6
    assert paths['existing.py']['after']['size_bytes'] == 5
    assert paths['deleted.py']['after'] is None
    assert paths['link']['after']['sha256'] == hashlib.sha256(b'missing-target').hexdigest()
    assert paths['link']['after']['size_bytes'] is None
    assert 'missing-target' not in json.dumps(facts)
    assert path_metadata(dict(kind='unavailable', manifest=None)) is None


@pytest.mark.parametrize('name', ['name\tpart.py', 'name\npart.py', 'name\t-\tpart.py'])
def test_delimiter_filenames_survive_baseline_and_review(tmp_path, name):
    directory = tmp_path / ('directory-' + name)
    directory.mkdir()
    file = directory / name
    file.write_text('before')
    (tmp_path / 'link\tname').symlink_to('missing\ttarget')
    store, task, coordinator, executor, ref = setup(tmp_path)
    file.write_text('after')
    _, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert facts['changed_paths_status'] == 'observed'
    paths = {entry['path']: entry for entry in facts['changed_paths']}
    assert list(paths) == [file.relative_to(tmp_path).as_posix()]
    assert paths[file.relative_to(tmp_path).as_posix()]['before']['size_bytes'] == 6
    assert paths[file.relative_to(tmp_path).as_posix()]['after']['size_bytes'] == 5
    metadata = path_metadata(capture_revision(tmp_path))
    assert metadata[directory.name] == dict(kind='d', sha256=None, size_bytes=None)


@pytest.mark.parametrize('kind', ['f', 'd', 'p', 's', 'dev', 'o'])
def test_fixed_fields_preserve_delimiters_in_paths(kind):
    path = 'directory\t-\tname/file\tpart'
    size, digest = ('1', 'a' * 64) if kind == 'f' else ('-', '-')
    assert path_metadata(dict(kind='observed', manifest=[f'{kind}\t{path}\t{size}\t{digest}'])) == {
        path: dict(kind=kind, sha256=digest if kind == 'f' else None,
                   size_bytes=1 if kind == 'f' else None)}


@pytest.mark.parametrize('name,target,ambiguous', [
    ('link\tname', 'missing\ttarget', False),
    ('link\tname', 'missing\t-\ttarget', True),
    ('link\tname', '-\ttarget', True),
    ('link\t-\tname', 'missing', True),
    ('link\t-', 'missing', True),
])
def test_link_targets_are_hashed_or_explicitly_unavailable(tmp_path, name, target, ambiguous):
    link = tmp_path / name
    link.symlink_to(target)
    store, task, coordinator, executor, ref = setup(tmp_path)
    _, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert target not in json.dumps(facts)
    metadata = path_metadata(capture_revision(tmp_path))
    if ambiguous:
        assert metadata is None
        assert facts['changed_paths_status'] == 'path_metadata_unavailable'
    else:
        assert metadata[link.name] == dict(
            kind='l', size_bytes=None, sha256=hashlib.sha256(target.encode()).hexdigest())
        assert facts['changed_paths_status'] == 'observed'


@pytest.mark.parametrize('record', [
    None, 'broken', 'f\tname\tnot-a-size\t' + 'a' * 64,
    'f\tname\t-1\t' + 'a' * 64, 'f\tname\t1\tinvalid',
    'd\tname\t-\tnot-a-placeholder', 'unknown\tname\t-\t-',
    'l\tname\t-\ttarget\t-\tother', 'l\tname\t-\t-\ttarget',
    'l\tname\t-\t-\ttarget\t-\tother', 'f\t\t1\t' + 'a' * 64,
    'f\tvalid.py\t1\t' + 'b' * 64,
])
def test_undecodable_manifest_is_unavailable_not_partial(record):
    observed = dict(kind='observed', manifest=['f\tvalid.py\t1\t' + 'b' * 64, record])
    assert path_metadata(observed) is None


@pytest.mark.parametrize('case', ['baseline', 'current'])
def test_undecodable_metadata_never_reports_observed_no_changes(tmp_path, monkeypatch, case):
    store, task, coordinator, executor, ref = setup(tmp_path)
    if case == 'baseline':
        with store.connection() as db:
            row = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=? "
                             "AND type='EXECUTION_BASELINE'", (task.task_id,)).fetchone()
            payload = json.loads(row['payload_json'])
            payload['path_metadata'] = None
            db.execute('UPDATE runtime_events SET payload_json=? WHERE id=?',
                       (json.dumps(payload), row['id']))
            db.commit()
    _, review_ref = reviewer(store, task, coordinator)
    if case == 'current':
        observed = capture_revision(tmp_path)
        observed['manifest'] = ['undecodable']
        monkeypatch.setattr('cortex_runtime.orchestration.reviewer_handoff.capture_revision',
                            lambda root: observed)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert facts['changed_paths_status'] == (
        'baseline_unavailable' if case == 'baseline' else 'path_metadata_unavailable')
    assert facts['changed_paths'] == []


def test_facts_bounds_and_brief_recovery_keep_dependencies(tmp_path):
    store, task, coordinator, executor, ref = setup(tmp_path)
    for index in range(100):
        (tmp_path / f'{index:03d}.py').write_text('x')
    with store.connection() as db:
        db.execute('UPDATE runtime_tasks SET plan_json=? WHERE task_id=?',
                   (json.dumps(dict(verification=[['python3', '-c', 'x' * 10000], ARGV])), task.task_id))
        db.commit()
    _, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert len(json.dumps(facts, ensure_ascii=True, separators=(',', ':')).encode()) <= FACTS_MAX_BYTES
    assert facts['truncated'] is True
    assert facts['omitted']['verification_argv'] == 1
    assert facts['omitted']['changed_paths'] > 0
    assert facts['verification_argv'] == [ARGV]
    dependencies = [dict(node='executor', report_ref='r_123456789abc')]
    brief = json.loads(build_worker_brief(
        objective='Review', scope=['**'], dependency_reports=dependencies,
        original_goal='goal' * 10000,
        review_evidence=[facts, dict(kind='diff', output='diff' * 30000)],
        assignment_ref=review_ref))
    assert brief['dependency_reports'] == dependencies
    assert brief['review_evidence']['truncated'] is True
    refs = brief['review_evidence']['full_text_references']
    assert dict(kind='runtime_verified_facts', tool='read_large_output',
                arguments=dict(large_output_id=facts['large_output_id'], offset=0)) in refs


@pytest.mark.parametrize('case', ['missing_baseline', 'unavailable', 'drift'])
def test_missing_or_stale_observation_never_becomes_no_changes(tmp_path, monkeypatch, case):
    store, task, coordinator, executor, ref = setup(tmp_path)
    _, review_ref = reviewer(store, task, coordinator)
    if case == 'missing_baseline':
        with store.connection() as db:
            db.execute("DELETE FROM runtime_events WHERE task_id=? AND type='EXECUTION_BASELINE'",
                       (task.task_id,))
            db.commit()
    elif case == 'unavailable':
        monkeypatch.setattr('cortex_runtime.orchestration.reviewer_handoff.capture_revision',
                            lambda root: dict(kind='unavailable', dirty_digest=None, manifest=None))
    else:
        (tmp_path / 'late.py').write_text('late')
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert facts['changed_paths_status'] != 'observed'
    assert facts['changed_paths'] == []


@pytest.mark.parametrize('verdict', ['APPROVE', 'REQUEST_CHANGES', 'BLOCK'])
def test_inline_review_report_retained_with_unchanged_verdict_contract(tmp_path, verdict):
    store, task, coordinator, executor, ref = setup(tmp_path)
    worker, review_ref = reviewer(store, task, coordinator)
    with store.connection() as db:
        before = db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'",
                            (task.task_id,)).fetchone()[0]
    args = dict(report=REPORT, summary='Independent review', checks=[], verdict=verdict, request_key='result')
    with pytest.raises(StoreError):
        mcp_tools.dispatch(store, worker, 'result_submit', {k: v for k, v in args.items() if k != 'verdict'})
    result = mcp_tools.dispatch(store, worker, 'result_submit', args)
    assert result['recorded']
    assert mcp_tools.dispatch(store, worker, 'result_submit', args) == result
    with store.connection() as db:
        saved = json.loads(db.execute('SELECT result_json FROM runtime_nodes WHERE assignment_ref=?',
                                      (review_ref,)).fetchone()[0])
        filename = db.execute('SELECT filename FROM reports WHERE id=?', (saved['report_ref'],)).fetchone()[0]
        text = (store._task_directory(db, task.task_id) / filename).read_text()
        assert db.execute("SELECT COUNT(*) FROM reports WHERE task_id=? AND kind='report'",
                          (task.task_id,)).fetchone()[0] == before + 1
    assert text.endswith(REPORT)
    assert saved['verdict'] == verdict and saved['report_digest'] and saved['project_digest']


def test_rebound_regeneration_uses_new_digest(tmp_path):
    store, task, coordinator, executor, ref = setup(tmp_path)
    worker, review_ref = reviewer(store, task, coordinator)
    first = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    (tmp_path / 'late.py').write_text('late')
    with pytest.raises(StoreError, match='review_project_revision_changed') as caught:
        mcp_tools.dispatch(store, worker, 'result_submit', dict(
            report=REPORT, summary='Review', checks=[], verdict='APPROVE', request_key='old'))
    assert 'Refreshed runtime facts' in caught.value.correction
    second = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert first['review_digest'] != second['review_digest']
    assert second['review_digest'] == second['observed_digest']
    assert second['changed_paths_status'] == 'observed'
    assert [item['path'] for item in second['changed_paths']] == ['late.py']
