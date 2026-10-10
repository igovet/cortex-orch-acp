"""Read-only aggregates-only multi-task mode of scripts/cortex_timeline.py:
counts/ratios/durations/availability only, same refusal behavior as the
existing single-task mode for busy/uncheckpointed/legacy/unsupported
stores, plus its own over-limit refusal and the owner-approved
decision-gate verdict.
"""
import importlib.util
import json
from pathlib import Path
import sqlite3

import pytest

from test_runtime_task_machine import machine, plan  # noqa: F401 (fixture import)

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('timeline_aggregates', ROOT / 'scripts/cortex_timeline.py')
timeline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(timeline)


def test_aggregate_over_one_task_is_structurally_sound_and_invalid_by_default(machine):
    state, ch = machine
    result = timeline.read_aggregates(state.store.project_root, [ch.task_id])
    assert result['validity']['total_tasks'] == 1
    assert result['validity']['enough_samples'] is False
    assert result['validity']['valid'] is False
    assert result['branches']['worker_loops'] is None
    assert result['branches']['note']
    assert result['per_task'] == [dict(
        task_id=ch.task_id, attempts=0, phase_chain_complete=False, tokens_basis='unavailable_reason',
        runtime_synthesis=dict(reconciled=False, composed=None, fallback_reason=None),
    )]


def test_aggregate_defaults_to_every_task_in_the_store(machine):
    state, ch = machine
    result = timeline.read_aggregates(state.store.project_root)
    assert [row['task_id'] for row in result['per_task']] == [ch.task_id]


def test_aggregate_unknown_explicit_task_is_refused(machine):
    state, ch = machine
    with pytest.raises(Exception, match='timeline_task_missing'):
        timeline.read_aggregates(state.store.project_root, [ch.task_id, 'not_a_real_task'])


def test_aggregate_over_limit_is_refused(machine):
    state, ch = machine
    with pytest.raises(Exception, match='timeline_aggregate_overflow'):
        timeline.read_aggregates(state.store.project_root, [ch.task_id], max_tasks=0)


def test_aggregate_refuses_live_wal_same_as_single_task_mode(machine, tmp_path):
    state, ch = machine
    wal = tmp_path / '.cortex/cortex.sqlite3-wal'
    wal.write_bytes(b'nonempty')
    wal.chmod(0o600)
    with pytest.raises(Exception, match='inspection_uncheckpointed_wal'):
        timeline.read_aggregates(tmp_path, [ch.task_id])


def test_aggregate_refuses_legacy_format_same_as_single_task_mode(machine, tmp_path):
    state, ch = machine
    path = tmp_path / '.cortex/cortex.sqlite3'
    with sqlite3.connect(path) as db:
        db.execute('PRAGMA user_version=11')
        db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    with pytest.raises(Exception, match='legacy_storage_migration_required'):
        timeline.read_aggregates(tmp_path, [ch.task_id])


def test_aggregate_output_never_carries_report_bodies_or_logs(machine):
    state, ch = machine
    from cortex_runtime.observability import events as observability_events
    observability_events.append_event(state.store, ch.task_id, 'project_read', 'text', 'observed',
                                      attempt_id='xa_1', detail=dict(path='a.txt', kind='text', sha256='sha-a'))
    result = timeline.read_aggregates(state.store.project_root, [ch.task_id])
    blob = json.dumps(result)
    # Only the project-relative identifiers above may legitimately appear
    # (counts/ratios/availability text); no path *content* or report text.
    assert 'sha-a' not in blob  # a sha value itself is identity, not content, but still never surfaced here
    assert result['per_task'][0]['task_id'] == ch.task_id


# --- CLI wiring -------------------------------------------------------------

def test_cli_requires_task_unless_aggregate(machine):
    with pytest.raises(SystemExit):
        timeline.main(['--project', '/tmp'])


def test_cli_aggregate_mode_runs_without_task(machine, tmp_path, capsys):
    state, ch = machine
    assert timeline.main(['--project', str(tmp_path), '--aggregate', '--json']) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload['validity']['total_tasks'] == 1


def test_cli_aggregate_mode_accepts_repeated_task_flags(machine, tmp_path, capsys):
    state, ch = machine
    assert timeline.main(['--project', str(tmp_path), '--aggregate', '--task', ch.task_id, '--json']) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload['per_task'][0]['task_id'] == ch.task_id


def _entry(i, engine='claude-agent-acp', route='same', effort='high', complete=True, task_completed=True):
    attempt = dict(attempt_id=f'a{i}', engine=engine, connection_id=route, model='model',
                   params_fingerprint=effort, stage='execution', dependent=True,
                   completed=complete, phase_chain_complete=complete and task_completed,
                   execution_seconds_excluding_user_waits=50,
                   execution_intervals_excluding_user_waits=[[0, 50]])
    critical = dict(availability='available', task_completed=task_completed, truncated=False,
                    wall_seconds_excluding_user_waits=100, per_attempt=[attempt],
                    combined=dict(orchestration=dict(share_of_wall_time_excluding_user_waits=.4),
                                  review_verification=dict(share_of_wall_time_excluding_user_waits=.3)))
    summary = dict(
        critical_path=critical,
        loop_profile=dict(per_attempt=[dict(attempt_id=f'a{i}', tool_calls_for_decision=20,
                                            inference_gap_share_of_execution=.5)]),
        write_redundancy=dict(per_attempt=[dict(attempt_id=f'a{i}', project_write_bytes=1000,
                                                 project_write_changed_bytes=100,
                                                 project_write_share_of_all_writes=.4,
                                                 project_write_calls=4, all_write_calls=10)]),
        reinvestigation=dict(per_attempt=[dict(attempt_id=f'a{i}', repeat_ratio=.3,
                                               repeat_time_share_of_execution=.2, repeated_text_reads=3,
                                               text_reads=10, repeated_read_seconds=10)]))
    return dict(task_id=f't{i}', attempts=1, phase_chain_complete=complete,
                tokens_basis='available', usage_by_attempt={f'a{i}': dict(calls=10, unknown=0, released=0)}, summary=summary)


def test_complete_single_cohort_applies_all_approved_gates():
    result = timeline._aggregate_decision([_entry(i) for i in range(10)])
    assert result['validity']['valid']
    assert result['validity']['phase_chain_complete_ratio'] == 1
    assert result['branches']['worker_loops']['holds']
    assert result['branches']['worker_loops']['code_apply_recommended']
    assert result['branches']['reinvestigation']['holds']
    assert result['branches']['orchestration_chain']['holds']
    assert result['branches']['reinvestigation']['cross_task_reuse_considered'] is None


@pytest.mark.parametrize('engine,route,effort', [
    ('codex-acp', 'same', 'high'), ('claude-agent-acp', 'different', 'high'),
    ('claude-agent-acp', 'same', 'low')])
def test_mixed_cohorts_do_not_pool_sample_or_token_gates(engine, route, effort):
    entries = [_entry(i) for i in range(5)] + [_entry(i, engine, route, effort) for i in range(5, 10)]
    result = timeline._aggregate_decision(entries)
    assert not result['validity']['valid']
    assert result['branches']['worker_loops'] is None
    assert len(result['cohorts']) == 2
    assert all(not c['validity']['valid'] and c['validity']['completed_tasks'] == 5 for c in result['cohorts'])


@pytest.mark.parametrize('reason', ['missing', 'truncated', 'running'])
def test_incomplete_chain_cannot_pass_validity_gate(reason):
    entries = [_entry(i) for i in range(10)]
    for entry in entries[:3]:
        critical = entry['summary']['critical_path']
        attempt = critical['per_attempt'][0]
        if reason == 'missing':
            attempt['phase_chain_complete'] = False
        elif reason == 'truncated':
            critical['truncated'] = True
            attempt['phase_chain_complete'] = False
        else:
            critical['task_completed'] = False
            attempt['phase_chain_complete'] = False
    result = timeline._aggregate_decision(entries)
    assert not result['validity']['valid']
    assert result['validity']['phase_chain_complete_ratio'] == .7
    assert result['branches']['worker_loops']['availability'] == 'unavailable_reason'
    assert result['branches']['reinvestigation']['holds'] is None


def test_only_exact_completed_chains_contribute_branch_inputs_and_review_is_excluded():
    entries = [_entry(i) for i in range(10)]
    bad = entries[-1]['summary']
    bad['critical_path']['per_attempt'][0]['phase_chain_complete'] = False
    bad['loop_profile']['per_attempt'][0]['tool_calls_for_decision'] = 10000
    for e in entries:
        review = dict(e['summary']['critical_path']['per_attempt'][0], attempt_id='review', stage='review')
        e['summary']['critical_path']['per_attempt'].append(review)
        e['summary']['loop_profile']['per_attempt'].append(dict(attempt_id='review', tool_calls_for_decision=999,
                                                               inference_gap_share_of_execution=0))
    result = timeline._aggregate_decision(entries)
    assert result['validity']['total_attempts'] == 10
    assert result['validity']['eligible_attempts'] == 9
    assert result['inputs']['median_tool_calls_per_attempt'] == 20
    assert result['inputs']['mean_inference_gap_share_of_worker_execution'] == .5


@pytest.mark.parametrize('section,field,branch', [
    ('loop_profile', 'tool_calls_for_decision', 'worker_loops'),
    ('loop_profile', 'inference_gap_share_of_execution', 'worker_loops'),
    ('reinvestigation', 'repeated_read_seconds', 'reinvestigation'),
    ('reinvestigation', 'repeated_text_reads', 'reinvestigation'),
])
def test_missing_approved_input_withholds_branch_not_false(section, field, branch):
    entries = [_entry(i) for i in range(10)]
    entries[0]['summary'][section]['per_attempt'][0][field] = None
    result = timeline._aggregate_decision(entries)
    assert result['validity']['valid']
    assert result['branches'][branch]['holds'] is None
    assert result['branches'][branch]['availability'] == 'unavailable_reason'


def test_missing_combined_phase_withholds_orchestration_not_zero():
    entries = [_entry(i) for i in range(10)]
    entries[0]['summary']['critical_path']['combined']['orchestration']['share_of_wall_time_excluding_user_waits'] = None
    result = timeline._aggregate_decision(entries)
    assert result['validity']['valid']
    assert result['branches']['orchestration_chain']['holds'] is None


@pytest.mark.parametrize('read_time,holds', [(.149, False), (.15, True)])
def test_reinvestigation_requires_the_15_percent_time_condition(read_time, holds):
    entries = [_entry(i) for i in range(10)]
    for entry in entries:
        entry['summary']['reinvestigation']['per_attempt'][0]['repeated_read_seconds'] = 50 * read_time
    result = timeline._aggregate_decision(entries)
    assert result['branches']['reinvestigation']['holds'] is holds


def test_token_coverage_is_per_call_not_task():
    entries = [_entry(i) for i in range(10)]
    for entry in entries:
        entry['usage_by_attempt'][entry['summary']['critical_path']['per_attempt'][0]['attempt_id']] = dict(calls=1, unknown=0, released=0)
    entries[-1]['usage_by_attempt']['a9'] = dict(calls=91, unknown=70, released=21)
    result = timeline._aggregate_decision(entries)
    assert result['validity']['token_calls'] == 100
    assert result['validity']['unknown_token_ratio'] == .91
    assert result['validity']['token_criteria_usable'] is False
    entries[-1]['usage_by_attempt']['a9'] = dict(calls=91, unknown=10, released=10)
    assert timeline._aggregate_decision(entries)['validity']['token_criteria_usable'] is True
    del entries[-1]['usage_by_attempt']['a9']
    result = timeline._aggregate_decision(entries)
    assert result['validity']['unknown_token_ratio'] is None


@pytest.mark.parametrize('truncation', ['timing_rows_truncated', 'external_duration_rows_truncated',
                                      'gateway_duration_rows_truncated'])
def test_duration_truncation_withholds_worker_branch(truncation):
    entries = [_entry(i) for i in range(10)]
    entries[0]['summary']['loop_profile'][truncation] = True
    result = timeline._aggregate_decision(entries)
    assert result['branches']['worker_loops']['holds'] is None


def test_project_write_gate_uses_sum_bytes_over_sum_changed_bytes_not_mean_ratios():
    entries = [_entry(i) for i in range(10)]
    for entry in entries:
        entry['summary']['write_redundancy']['per_attempt'][0].update(
            project_write_bytes=10, project_write_changed_bytes=1)
    entries[0]['summary']['write_redundancy']['per_attempt'][0].update(
        project_write_bytes=1000, project_write_changed_bytes=1000)
    result = timeline._aggregate_decision(entries)
    assert result['inputs']['mean_write_redundancy_ratio'] == pytest.approx(1090 / 1009)
    assert result['branches']['worker_loops']['code_apply_recommended'] is False
    assert result['branches']['worker_loops']['promote_project_edit'] is True
    entries[0]['summary']['write_redundancy']['per_attempt'][0]['all_write_calls'] = None
    result = timeline._aggregate_decision(entries)
    assert result['branches']['worker_loops']['code_apply_recommended'] is None
    assert result['branches']['worker_loops']['promote_project_edit'] is None


@pytest.mark.parametrize('ratio,considered', [(.29, False), (.3, True)])
def test_cross_task_reuse_considered_only_at_approved_ratio(ratio, considered):
    entries = [_entry(i) for i in range(10)]
    cross = dict(availability='available', per_attempt=[
        dict(task_id=f't{i}', attempt_id=f'a{i}', sample_count=100, repeat_count=int(ratio * 100))
        for i in range(10)])
    result = timeline._aggregate_decision(entries, cross)
    assert result['branches']['reinvestigation']['cross_task_reuse_considered'] is considered


def test_read_aggregate_has_cross_task_call_history_without_disclosing_paths(machine):
    state, ch = machine
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.observability import events
    second = bootstrap_task(state.store, SourceEvent(origin='second', event_id='history'), 'Synthetic', 'owner')
    for task, attempt in [(ch.task_id, 'a'), (second.task_id, 'b')]:
        events.append_event(state.store, task, 'project_read', 'text', 'observed',
                            attempt_id=attempt, detail=dict(path='private-path', sha256='private-sha'))
    result = timeline.read_aggregates(state.store.project_root, [second.task_id])
    assert result['cross_task_repeat_ratio']['availability'] == 'available'
    assert result['cross_task_repeat_ratio']['ratio'] == .5
    blob = json.dumps(result)
    assert 'private-path' not in blob and 'private-sha' not in blob


def test_repeat_read_population_is_call_weighted_and_time_uses_all_execution_attempts():
    entries = [_entry(i) for i in range(10)]
    for e in entries:
        e['summary']['reinvestigation']['per_attempt'][0].update(text_reads=1, repeated_text_reads=1,
                                                                repeated_read_seconds=10)
    entries[0]['summary']['reinvestigation']['per_attempt'][0].update(text_reads=100, repeated_text_reads=0)
    result = timeline._aggregate_decision(entries)
    assert result['inputs']['mean_cross_attempt_within_task_repeat_ratio'] == pytest.approx(9 / 109)
    assert result['branches']['reinvestigation']['holds'] is False
    entries[0]['summary']['critical_path']['per_attempt'][0]['dependent'] = False
    entries[0]['summary']['critical_path']['per_attempt'][0]['execution_seconds_excluding_user_waits'] = 1000
    result = timeline._aggregate_decision(entries)
    # Only dependent repeats count, over all worker execution time.
    assert result['inputs']['mean_cross_attempt_within_task_repeat_ratio'] == 1
    assert result['inputs']['mean_repeat_read_share_of_worker_execution'] == pytest.approx(90 / 1450)
    assert result['branches']['reinvestigation']['holds'] is False


def test_all_independent_attempts_cannot_satisfy_the_dependent_read_gate():
    entries = [_entry(i) for i in range(10)]
    for entry in entries:
        entry['summary']['critical_path']['per_attempt'][0]['dependent'] = False
    result = timeline._aggregate_decision(entries)
    assert result['validity']['valid']
    assert result['inputs']['mean_cross_attempt_within_task_repeat_ratio'] is None
    assert result['inputs']['mean_repeat_read_share_of_worker_execution'] is None
    assert result['branches']['reinvestigation']['holds'] is None
    assert result['branches']['reinvestigation']['availability'] == 'unavailable_reason'
    # The raw independent measurements remain diagnostic, not discarded.
    assert entries[0]['summary']['reinvestigation']['per_attempt'][0]['repeated_text_reads'] == 3


def test_independent_repeats_do_not_raise_a_mixed_cohorts_gate_inputs():
    entries = [_entry(i) for i in range(10)]
    entries[0]['summary']['critical_path']['per_attempt'][0]['dependent'] = False
    entries[0]['summary']['reinvestigation']['per_attempt'][0].update(
        text_reads=1000, repeated_text_reads=1000, repeated_read_seconds=50)
    for entry in entries[1:]:
        entry['summary']['reinvestigation']['per_attempt'][0].update(
            text_reads=10, repeated_text_reads=1, repeated_read_seconds=1)
    result = timeline._aggregate_decision(entries)
    assert result['inputs']['mean_cross_attempt_within_task_repeat_ratio'] == .1
    assert result['inputs']['mean_repeat_read_share_of_worker_execution'] == pytest.approx(9 / 500)
    assert result['branches']['reinvestigation']['holds'] is False


def test_unknown_dependent_status_withholds_the_ratio_never_zero():
    """An attempt whose dependency status could not be established
    (`dependent is None`) must withhold the whole cohort ratio as unknown,
    never be dropped silently or coerced to 0."""
    entries = [_entry(i) for i in range(10)]
    for e in entries:
        e['summary']['reinvestigation']['per_attempt'][0].update(
            text_reads=10, repeated_text_reads=3, repeated_read_seconds=5)
    entries[0]['summary']['critical_path']['per_attempt'][0]['dependent'] = None
    result = timeline._aggregate_decision(entries)
    assert result['inputs']['mean_cross_attempt_within_task_repeat_ratio'] is None
    assert result['inputs']['mean_repeat_read_share_of_worker_execution'] is None
    assert result['branches']['reinvestigation']['availability'] == 'unavailable_reason'


def test_truncated_phase_summary_is_rejected_even_with_a_cached_complete_flag():
    entries = [_entry(i) for i in range(10)]
    for e in entries[:3]:
        e['summary']['critical_path']['truncated'] = True
    result = timeline._aggregate_decision(entries)
    assert result['validity']['phase_chain_complete_ratio'] == .7
    assert result['branches']['worker_loops']['holds'] is None


# --- Runtime-composed-synthesis share and fallback reasons ----------------

def _entry_with_synthesis(i, runtime_synthesis, **kwargs):
    entry = _entry(i, **kwargs)
    entry['runtime_synthesis'] = runtime_synthesis
    return entry


def test_cohort_reports_runtime_synthesis_share_and_fallback_reasons():
    entries = [_entry_with_synthesis(i, dict(reconciled=True, composed=True, fallback_reason=None))
               for i in range(7)]
    entries += [_entry_with_synthesis(i, dict(reconciled=True, composed=False, fallback_reason='open_check'))
                for i in range(7, 9)]
    entries.append(_entry_with_synthesis(9, dict(reconciled=True, composed=False, fallback_reason='unrecorded')))
    result = timeline._aggregate_decision(entries)
    cohort_rs = result['cohorts'][0]['runtime_synthesis']
    assert cohort_rs == dict(reconciled_tasks=10, composed_tasks=7, pending_tasks=0, share=pytest.approx(0.7),
                             fallback_reasons=dict(open_check=2, unrecorded=1))
    # Single-cohort passthrough keeps the same shape at the top level.
    assert result['runtime_synthesis'] == cohort_rs


def test_cohort_runtime_synthesis_excludes_not_yet_reconciled_tasks():
    entries = [_entry_with_synthesis(i, dict(reconciled=False, composed=None, fallback_reason=None))
               for i in range(10)]
    result = timeline._aggregate_decision(entries)
    assert result['cohorts'][0]['runtime_synthesis'] == dict(
        reconciled_tasks=0, composed_tasks=0, pending_tasks=0, share=None, fallback_reasons={})


@pytest.mark.parametrize('fallback_reason', [None, 'worker_live', 'unrecorded'])
def test_pending_synthesis_stays_incomplete_and_json_serializable(fallback_reason):
    pending = _entry_with_synthesis(
        0, dict(reconciled=True, composed=False, fallback_reason=None),
        complete=False, task_completed=False)
    entries = [pending, pending]  # Several attempts/rows must not duplicate a task.
    if fallback_reason is not None:
        entries.append(_entry_with_synthesis(
            1, dict(reconciled=True, composed=False, fallback_reason=fallback_reason)))
        entries.append(_entry_with_synthesis(
            2, dict(reconciled=True, composed=True, fallback_reason=None)))
    result = timeline._aggregate_decision(entries)
    # Exercise the entire CLI JSON shape and its exact sorting policy.
    payload = json.loads(json.dumps(result, sort_keys=True))
    synthesis = payload['cohorts'][0]['runtime_synthesis']
    assert synthesis == dict(
        reconciled_tasks=1 if fallback_reason is None else 3,
        composed_tasks=0 if fallback_reason is None else 1,
        pending_tasks=1, share=None,
        fallback_reasons={} if fallback_reason is None else {fallback_reason: 1})
    assert payload['runtime_synthesis'] == synthesis


@pytest.mark.parametrize('declined', [False, True])
def test_read_aggregates_keeps_unfinished_reconciliation_pending(machine, declined):
    from cortex_runtime.orchestration.state import transition, event
    state, ch = machine
    with state.store.connection() as db:
        db.execute('BEGIN')
        transition(db, ch.task_id, 'VERIFYING')
        transition(db, ch.task_id, 'RECONCILING')
        if declined:
            event(db, ch.task_id, 'SYNTHESIS_RUNTIME_DECLINED',
                  dict(revision=0, reason='worker_live'))
        db.commit()
    result = timeline.read_aggregates(state.store.project_root, [ch.task_id])
    outcome = result['per_task'][0]['runtime_synthesis']
    assert outcome == dict(reconciled=True, composed=None, fallback_reason=None)
    entry = _entry_with_synthesis(0, outcome, complete=False, task_completed=False)
    synthesis = timeline._aggregate_decision([entry])['runtime_synthesis']
    assert synthesis['pending_tasks'] == 1
    assert synthesis['fallback_reasons'] == {}
    assert synthesis['share'] is None


def test_mixed_cohorts_withhold_a_pooled_runtime_synthesis_verdict():
    entries = ([_entry_with_synthesis(i, dict(reconciled=True, composed=True, fallback_reason=None))
               for i in range(5)]
               + [_entry_with_synthesis(i, dict(reconciled=True, composed=False, fallback_reason='worker_live'),
                                       engine='codex-acp') for i in range(5, 10)])
    result = timeline._aggregate_decision(entries)
    assert len(result['cohorts']) == 2
    assert result['cohorts'][0]['runtime_synthesis']['composed_tasks'] == 5
    assert result['cohorts'][1]['runtime_synthesis']['fallback_reasons'] == dict(worker_live=5)
    assert 'note' in result['runtime_synthesis']


def test_read_aggregates_reports_runtime_synthesis_outcome_for_a_real_task(machine):
    from cortex_runtime.orchestration.state import transition, event
    state, ch = machine
    with state.store.connection() as db:
        db.execute('BEGIN')
        transition(db, ch.task_id, 'VERIFYING')
        transition(db, ch.task_id, 'RECONCILING')
        event(db, ch.task_id, 'SYNTHESIS_RUNTIME_COMPOSED',
              dict(revision=0, text='x', language='en', not_verified=[]))
        transition(db, ch.task_id, 'COMPLETED')
        db.commit()
    result = timeline.read_aggregates(state.store.project_root, [ch.task_id])
    assert result['per_task'][0]['runtime_synthesis'] == dict(
        reconciled=True, composed=True, fallback_reason=None)


def test_read_aggregates_marks_a_coordinator_completion_without_a_decline_event_unrecorded(machine):
    from cortex_runtime.orchestration.state import transition
    state, ch = machine
    with state.store.connection() as db:
        db.execute('BEGIN')
        transition(db, ch.task_id, 'VERIFYING')
        transition(db, ch.task_id, 'RECONCILING')
        # Completed by the coordinator with no retained SYNTHESIS_RUNTIME_COMPOSED
        # or SYNTHESIS_RUNTIME_DECLINED event: the reason is explicitly unrecorded,
        # never guessed.
        transition(db, ch.task_id, 'COMPLETED')
        db.commit()
    result = timeline.read_aggregates(state.store.project_root, [ch.task_id])
    assert result['per_task'][0]['runtime_synthesis'] == dict(
        reconciled=True, composed=False, fallback_reason='unrecorded')


def test_read_aggregates_reports_a_declined_reason_for_a_real_task(machine):
    from cortex_runtime.orchestration.state import transition, event
    state, ch = machine
    with state.store.connection() as db:
        db.execute('BEGIN')
        transition(db, ch.task_id, 'VERIFYING')
        transition(db, ch.task_id, 'RECONCILING')
        event(db, ch.task_id, 'SYNTHESIS_RUNTIME_DECLINED',
              dict(revision=0, reason='open_check', reasons=[dict(code='open_check', node='N0',
                                                                  check='scoped test', outcome='not_run')]))
        transition(db, ch.task_id, 'COMPLETED')
        db.commit()
    result = timeline.read_aggregates(state.store.project_root, [ch.task_id])
    assert result['per_task'][0]['runtime_synthesis'] == dict(
        reconciled=True, composed=False, fallback_reason='open_check')


def test_read_aggregates_counts_unknown_and_released_calls_with_null_stop_reason(machine):
    state, ch = machine
    from test_observability_stage2_metrics import _node, _stream, _insert_route, _ts
    _node(state.store, ch.task_id, 'w')
    _insert_route(state.store, ch.task_id, 'w', 'claude-agent-acp', _ts(0))
    _stream(state.store, ch.task_id, [
        (0, 'ASSIGNMENT_CREATED', dict(assignment_ref='w')),
        (1, 'ENGINE_READY', dict(assignment_ref='w', attempt_id='w', engine_session_id='sw')),
        (3, 'WORKER_COMPLETED', dict(assignment_ref='w', attempt_id='w'))])
    with state.store.connection() as db:
        for i in range(10):
            basis = 'unknown' if i < 5 else 'engine_reported'
            stop = 'released_after_result' if i < 2 else None
            db.execute("INSERT INTO usage_records VALUES(?,?,?,?,?,?,?,?,?,?)",
                       (f'u{i}', ch.task_id, 'w', 'sw', i, basis, '{}', stop, 0, _ts(4 + i)))
        db.commit()
    result = timeline.read_aggregates(state.store.project_root, [ch.task_id])
    validity = result['cohorts'][0]['validity']
    assert validity['token_calls'] == 10
    assert validity['unknown_or_released_calls'] == 5
    assert validity['unknown_token_ratio'] == .5
    assert not validity['token_criteria_usable']
