"""Stage 2 aggregates (loop_profile, reinvestigation, write_redundancy,
critical_path) in observability.metrics.diagnostics_summary -- synthetic
streams only, no paid engine calls. Every assertion checks availability/
basis explicitly: an absent signal must read unavailable_reason, never 0.
"""
import json

import pytest

from cortex_runtime.observability import events as observability_events
from cortex_runtime.observability import metrics

from test_runtime_task_machine import machine, plan  # noqa: F401 (fixture import)
from test_worker_wait_continuation import bound  # noqa: F401 (fixture import)


def _insert_event(store, task_id, kind, created_at, payload=None, actor='runtime'):
    with store.connection() as db:
        db.execute('INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)',
                   (task_id, kind, actor, json.dumps(payload or {}), created_at))
        db.commit()


def _insert_read(store, task_id, attempt_id, path, sha256, kind='text'):
    detail = dict(path=path, kind=kind)
    if sha256 is not None:
        detail['sha256'] = sha256
    observability_events.append_event(store, task_id, 'project_read', kind, 'observed',
                                      attempt_id=attempt_id, detail=detail)


def _insert_route(store, task_id, attempt_id, engine, recorded_at):
    with store.connection() as db:
        db.execute(
            'INSERT INTO attempt_route_audit VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
            (f'ar_{attempt_id}', attempt_id, task_id, 'worker-backend-dev', engine, 'cn_test', 'model',
             'fp', None, 'authorized', '{}', None, recorded_at))
        db.commit()


def _insert_observation(store, task_id, attempt_id, tool_call_ref, kind, status, created_at):
    with store.connection() as db:
        db.execute(
            'INSERT INTO tool_result_observations (task_id, attempt_id, tool_call_ref, kind, title, status, created_at) '
            'VALUES (?,?,?,?,?,?,?)', (task_id, attempt_id, tool_call_ref, kind, None, status, created_at))
        db.commit()


# --- loop_profile ---------------------------------------------------------

def test_loop_profile_unavailable_without_any_rows(machine):
    state, ch = machine
    summary = metrics.diagnostics_summary(state.store, ch.task_id)
    assert summary['loop_profile']['availability'] == 'unavailable_reason'
    assert 'reason' in summary['loop_profile']


def test_loop_profile_counts_each_class_from_its_own_table_and_leaks_no_content(bound):
    store, _machine, _coordinator, workers = bound
    worker = workers[0]
    task_id = worker.task_id
    attempt = 'xa_test0'
    _insert_event(store, task_id, 'TOOL_AUTHORIZED', '2099-01-01T09:00:00+00:00',
                  dict(operation='project_write', tool_call_ref='gw1'), actor=worker.channel_id)
    _insert_event(store, task_id, 'TOOL_COMPLETED', '2099-01-01T09:00:02+00:00',
                  dict(operation='project_write', tool_call_ref='gw1',
                       result=dict(byte_count=10, changed_bytes=10, oldText='SECRET-OLD', newText='SECRET-NEW')),
                  actor=worker.channel_id)
    _insert_read(store, task_id, attempt, 'a.txt', 'sha-a')
    _insert_event(store, task_id, 'EXTERNAL_TOOL_AUTHORIZED', '2099-01-01T09:00:03+00:00',
                  dict(server='codebase-memory-mcp', tool='search_graph', request_id='req1'),
                  actor=worker.channel_id)
    _insert_event(store, task_id, 'EXTERNAL_TOOL_RESULT', '2099-01-01T09:00:04+00:00',
                  dict(server='codebase-memory-mcp', tool='search_graph', request_id='req1',
                       result=dict(secret='SECRET-EXTERNAL-REPLY')),
                  actor=worker.channel_id)
    _insert_observation(store, task_id, attempt, 'gw1', 'edit', 'completed', '2099-01-01T09:00:02+00:00')
    _insert_observation(store, task_id, attempt, 'ext1', 'fetch', 'completed', '2099-01-01T09:00:04+00:00')
    _insert_observation(store, task_id, attempt, 'native1', 'execute', 'completed', '2099-01-01T09:00:06+00:00')

    summary = metrics.diagnostics_summary(store, task_id)
    loop = summary['loop_profile']
    assert loop['availability'] == 'available'
    row = next(r for r in loop['per_attempt'] if r['attempt_id'] == attempt)
    assert row['cortex_gateway_calls'] == 1
    assert row['project_read_calls'] == 1
    assert row['external_mcp_calls'] == 1
    assert row['acp_reported_calls'] == 3
    assert row['gateway_tool_time_seconds'] == pytest.approx(2.0)
    assert row['external_mcp_tool_time_seconds'] == pytest.approx(1.0)
    assert row['cortex_delegation_calls']['availability'] == 'unavailable_reason'
    assert row['inference_gap_seconds']['sample_count'] == 2
    assert row['inference_gap_seconds']['sum'] == pytest.approx(4.0)
    assert row['inference_gap_seconds']['p50'] == pytest.approx(2.0)

    blob = json.dumps(summary)
    assert 'SECRET-OLD' not in blob
    assert 'SECRET-NEW' not in blob
    assert 'SECRET-EXTERNAL-REPLY' not in blob


# --- reinvestigation -------------------------------------------------------

def test_reinvestigation_within_and_cross_attempt_ratios(machine):
    state, ch = machine
    _insert_read(state.store, ch.task_id, 'xa_1', 'a.txt', 'sha-a')
    _insert_read(state.store, ch.task_id, 'xa_1', 'a.txt', 'sha-a')  # within-attempt repeat
    _insert_read(state.store, ch.task_id, 'xa_2', 'a.txt', 'sha-a')  # cross-attempt repeat
    _insert_read(state.store, ch.task_id, 'xa_2', 'b.txt', 'sha-b')  # unique
    summary = metrics.diagnostics_summary(state.store, ch.task_id)
    reinvest = summary['reinvestigation']
    assert reinvest['availability'] == 'available'
    assert reinvest['basis'] == 'observed_partial'
    assert reinvest['sample_count'] == 4
    assert reinvest['within_attempt_repeat_ratio'] == pytest.approx(1 / 4)
    assert reinvest['cross_attempt_within_task_repeat_ratio'] is None  # channels are absent
    assert reinvest['cross_task_repeat_ratio']['availability'] == 'unavailable_reason'
    assert reinvest['partial'] is True


def test_reinvestigation_flags_partial_for_non_claude_engine(machine):
    state, ch = machine
    _insert_route(state.store, ch.task_id, 'xa_codex', 'codex-acp', '2026-10-05T09:00:00+00:00')
    _insert_read(state.store, ch.task_id, 'xa_codex', 'a.txt', 'sha-a')
    summary = metrics.diagnostics_summary(state.store, ch.task_id)
    reinvest = summary['reinvestigation']
    assert reinvest['partial'] is True
    assert reinvest['basis'] == 'observed_partial'
    assert reinvest['engines_observed'] == ['codex-acp']


def test_reinvestigation_unavailable_without_successful_reads(machine):
    state, ch = machine
    _insert_read(state.store, ch.task_id, 'xa_1', 'missing.txt', None, kind='missing')
    summary = metrics.diagnostics_summary(state.store, ch.task_id)
    assert summary['reinvestigation']['availability'] == 'unavailable_reason'


# --- write_redundancy -------------------------------------------------------

def test_write_redundancy_ratio_and_codex_apply_patch_share(bound):
    store, _machine, _coordinator, workers = bound
    worker = workers[0]
    task_id = worker.task_id
    attempt = 'xa_test0'
    _insert_event(store, task_id, 'TOOL_COMPLETED', '2099-01-01T09:00:00+00:00',
                  dict(operation='project_write', tool_call_ref='gw1',
                       result=dict(byte_count=1000, changed_bytes=1000)), actor=worker.channel_id)
    _insert_event(store, task_id, 'TOOL_COMPLETED', '2099-01-01T09:00:01+00:00',
                  dict(operation='project_edit', tool_call_ref='gw2',
                       result=dict(byte_count=1000, changed_bytes=10)), actor=worker.channel_id)
    _insert_observation(store, task_id, attempt, 'codex1', 'edit', 'completed', '2099-01-01T09:00:02+00:00')
    summary = metrics.diagnostics_summary(store, task_id)
    write = summary['write_redundancy']
    assert write['availability'] == 'available'
    assert write['sample_count'] == 2
    assert write['redundancy_ratio'] == pytest.approx(2000 / 1010)
    assert write['codex_apply_patch_calls_estimated']['availability'] == 'unavailable_reason'
    assert write['share_of_writes_by_class'] == {}
    assert write['gateway_share_of_writes_by_class']['project_write'] == pytest.approx(.5)
    assert write['project_write_redundancy_ratio'] == 1
    assert write['project_write_share_of_all_writes'] is None


def test_write_redundancy_unavailable_without_rows(machine):
    state, ch = machine
    summary = metrics.diagnostics_summary(state.store, ch.task_id)
    assert summary['write_redundancy']['availability'] == 'unavailable_reason'


# --- critical_path -----------------------------------------------------------

def test_critical_path_unavailable_with_fewer_than_two_events(machine):
    state, ch = machine
    result = metrics._critical_path(state.store, 'no_such_task_id')
    assert result['availability'] == 'unavailable_reason'
    assert 'fewer than 2' in result['reason']


def test_critical_path_classes_merge_overlaps_and_exclude_user_wait(machine):
    state, ch = machine
    task_id = ch.task_id

    def ts(second):
        return f'2099-01-01T09:00:{second:02d}+00:00'

    _node(state.store, task_id, 'w1', 'execution')
    _node(state.store, task_id, 'rev', 'review')
    stream = [
        ('ASSIGNMENT_CREATED', ts(0), dict(assignment_ref='w1')),
        ('ENGINE_READY', ts(1), dict(assignment_ref='w1', attempt_id='xa_w1', engine_session_id='es_w1')),
        ('SCHEDULER_WAIT_REASON', ts(2), dict(identity='w1wait', reason='scope_overlap')),
        ('SCHEDULER_WAIT_REASON', ts(5), dict(identity='w1wait', reason=None)),
        ('WORKER_COMPLETED', ts(11), dict(assignment_ref='w1', attempt_id='xa_w1')),
        ('ASSIGNMENT_CREATED', ts(11), dict(assignment_ref='rev')),
        ('ENGINE_READY', ts(12), dict(assignment_ref='rev', attempt_id='xa_rev', engine_session_id='es_rev')),
        ('REVIEW_STARTED', ts(12), dict(assignment_ref='rev')),
        ('WORKER_COMPLETED', ts(14), dict(assignment_ref='rev', attempt_id='xa_rev')),
        ('COORDINATOR_TURN_STARTED', ts(15), dict(engine_session_id='es_coord')),
        ('COORDINATOR_TURN_ENDED', ts(17), dict(engine_session_id='es_coord')),
        ('TASK_STATE_CHANGED', ts(18), dict(state='VERIFYING')),
        ('VERIFICATION_PASSED', ts(20), dict()),
        ('TASK_STATE_CHANGED', ts(21), dict(state='waiting_for_user')),
        ('TASK_STATE_CHANGED', ts(26), dict(state='RECONCILING')),
        ('SYNTHESIS_SUBMITTED', ts(28), dict()),
    ]
    for kind, created_at, payload in stream:
        _insert_event(state.store, task_id, kind, created_at, payload)

    summary = metrics.diagnostics_summary(state.store, task_id)
    critical = summary['critical_path']
    assert critical['availability'] == 'available'
    # The real task also carries its own (unrelated) bootstrap/create
    # event(s) before this synthetic stream, so only each class's own
    # self-contained interval union -- never the overall total, which
    # those earlier events add to -- is asserted exactly here.
    assert critical['user_wait_seconds'] == pytest.approx(5.0)
    classes = critical['classes']
    assert classes['engine_startup']['seconds'] == pytest.approx(2.0)
    assert classes['worker_execution']['seconds'] == pytest.approx(10.0)
    assert classes['review']['seconds'] == pytest.approx(2.0)
    assert classes['coordinator_turns']['seconds'] == pytest.approx(2.0)
    assert classes['verification']['seconds'] == pytest.approx(2.0)
    assert classes['synthesis']['seconds'] == pytest.approx(2.0)
    assert classes['scheduler_waits']['seconds'] == pytest.approx(3.0)
    denominator = critical['wall_seconds_excluding_user_waits']
    assert classes['worker_execution']['share_of_wall_time_excluding_user_waits'] == pytest.approx(10 / denominator)


def test_critical_path_missing_pairs_give_zero_intervals_not_fabricated(machine):
    """An ASSIGNMENT_CREATED with no ENGINE_READY (an incomplete stream)
    gives engine_startup 0 real intervals -- never a guessed nonzero."""
    state, ch = machine
    task_id = ch.task_id
    _insert_event(state.store, task_id, 'ASSIGNMENT_CREATED', '2099-01-01T09:00:00+00:00',
                  dict(assignment_ref='w1'))
    _insert_event(state.store, task_id, 'TASK_STATE_CHANGED', '2099-01-01T09:00:05+00:00', dict(state='VERIFYING'))
    summary = metrics.diagnostics_summary(state.store, task_id)
    classes = summary['critical_path']['classes']
    assert classes['engine_startup']['interval_count'] == 0
    assert classes['engine_startup']['seconds'] is None
    assert classes['engine_startup']['availability'] == 'unavailable_reason'
    assert classes['verification']['seconds'] is None
    assert classes['verification']['interval_count'] == 0


# --- repeat_calls label/truncation fix ---------------------------------------

def test_repeat_calls_label_describes_prompt_redeliveries_not_tool_loops(machine):
    state, ch = machine
    summary = metrics.diagnostics_summary(state.store, ch.task_id)
    repeat = summary['repeat_calls']
    assert 'prompt' in repeat['unit'].lower()
    assert 'tool' not in repeat['unit'].lower().split('re-deliveries')[0]
    assert 'prompt' in repeat['note'].lower()
    assert repeat['truncated'] is False


def test_repeat_calls_surfaces_the_2000_row_cap_as_truncation(machine):
    state, ch = machine
    with state.store.connection() as db:
        rows = [(f'use_{i}', ch.task_id, None, 'es_cap', i + 1, 'unknown', '{}', None, 0, state.store._now())
               for i in range(2000)]
        db.executemany(
            'INSERT INTO usage_records (id,task_id,attempt_id,engine_session_ref,call_sequence,basis,'
            'tokens_json,stop_reason,failed,created_at) VALUES (?,?,?,?,?,?,?,?,?,?)', rows)
        db.commit()
    summary = metrics.diagnostics_summary(state.store, ch.task_id)
    assert summary['repeat_calls']['truncated'] is True


def _node(store, task_id, ref, stage='execution', dependencies=()):
    with store.connection() as db:
        db.execute("INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                   (task_id, 0, ref, 'worker-backend-dev', 'Synthetic', json.dumps(list(dependencies)),
                    '[]', stage, 'running', ref, None))
        db.commit()


def _ts(second):
    from datetime import datetime, timedelta, timezone
    return (datetime(2099, 1, 1, tzinfo=timezone.utc) + timedelta(seconds=second)).isoformat()


def _stream(store, task_id, events):
    with store.connection() as db:
        db.execute("DELETE FROM runtime_events WHERE task_id=?", (task_id,))
        for second, kind, payload in sorted(events, key=lambda e: e[0]):
            db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
                       (task_id, kind, 'runtime', json.dumps(payload), _ts(second)))
        db.commit()


def test_failed_attempt_replacement_and_session_mismatch_are_not_one_interval(machine):
    state, ch = machine
    _node(state.store, ch.task_id, 'w')
    _stream(state.store, ch.task_id, [
        (0, 'ASSIGNMENT_CREATED', dict(assignment_ref='w')),
        (1, 'ENGINE_READY', dict(assignment_ref='w', attempt_id='a1', engine_session_id='s1')),
        (4, 'WORKER_FAILED', dict(assignment_ref='w', attempt_id='a1')),
        (5, 'ENGINE_READY', dict(assignment_ref='w', attempt_id='a2', engine_session_id='s2')),
        (10, 'WORKER_COMPLETED', dict(assignment_ref='w', attempt_id='a2')),
    ])
    result = metrics._critical_path(state.store, ch.task_id)
    assert result['classes']['worker_execution']['seconds'] == 5
    assert result['classes']['failed_attempts']['seconds'] == 3
    assert result['classes']['engine_startup']['seconds'] is None  # assignment ambiguous
    assert [a['execution_seconds_excluding_user_waits'] for a in result['per_attempt']] == [3, 5]
    assert not any(a['phase_chain_complete'] for a in result['per_attempt'])
    _insert_event(state.store, ch.task_id, 'WORKER_COMPLETED', _ts(11),
                  dict(assignment_ref='w', attempt_id='a2', engine_session_id='wrong'))
    result = metrics._critical_path(state.store, ch.task_id)
    assert result['classes']['worker_execution']['seconds'] is None
    assert result['per_attempt'][1]['availability'] == 'unavailable_reason'


def test_user_wait_subtracted_from_every_phase_and_combined_unions(machine):
    state, ch = machine
    for ref, stage in [('w', 'execution'), ('r', 'review')]:
        _node(state.store, ch.task_id, ref, stage)
    _stream(state.store, ch.task_id, [
        (0, 'ASSIGNMENT_CREATED', dict(assignment_ref='w')),
        (0, 'ASSIGNMENT_CREATED', dict(assignment_ref='r')),
        (0, 'COORDINATOR_TURN_STARTED', dict(engine_session_id='coord')),
        (1, 'ENGINE_READY', dict(assignment_ref='w', attempt_id='aw', engine_session_id='sw')),
        (1, 'ENGINE_READY', dict(assignment_ref='r', attempt_id='ar', engine_session_id='sr')),
        (1, 'REVIEW_STARTED', dict(assignment_ref='r')),
        (1, 'TASK_STATE_CHANGED', dict(state='VERIFYING')),
        (1, 'SCHEDULER_WAIT_REASON', dict(identity='q', reason=None)),
        (2, 'TASK_STATE_CHANGED', dict(state='waiting_for_user')),
        (8, 'TASK_STATE_CHANGED', dict(state='EXECUTING')),
        (9, 'WORKER_COMPLETED', dict(assignment_ref='r', attempt_id='ar')),
        (9, 'VERIFICATION_PASSED', {}),
        (9, 'TASK_STATE_CHANGED', dict(state='RECONCILING')),
        (10, 'WORKER_COMPLETED', dict(assignment_ref='w', attempt_id='aw')),
        (10, 'COORDINATOR_TURN_ENDED', dict(engine_session_id='coord')),
        (10, 'SYNTHESIS_SUBMITTED', {}),
    ])
    result = metrics._critical_path(state.store, ch.task_id)
    assert result['wall_seconds_excluding_user_waits'] == 4
    assert result['classes']['worker_execution']['seconds'] == 3
    assert result['classes']['coordinator_turns']['seconds'] == 4
    assert result['classes']['review']['seconds'] == 2
    assert result['classes']['verification']['seconds'] == 2
    assert result['combined']['orchestration']['seconds'] == 4
    assert result['combined']['orchestration']['share_of_wall_time_excluding_user_waits'] == 1
    assert result['combined']['review_verification']['seconds'] == 2
    assert result['combined']['review_verification']['share_of_wall_time_excluding_user_waits'] == .5
    assert all(0 <= c['share_of_wall_time_excluding_user_waits'] <= 1
               for c in result['classes'].values() if c['share_of_wall_time_excluding_user_waits'] is not None)


@pytest.mark.parametrize('concurrent', [False, True])
def test_inference_gap_uses_own_execution_duration_and_excludes_review(bound, concurrent):
    store, state, coordinator, workers = bound
    task = coordinator.task_id
    events = []
    for i, w in enumerate(workers):
        with store.connection() as db:
            ref = db.execute("SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?",
                             (w.attempt_id,)).fetchone()[0]
        start = 1 if concurrent else 1 + 50 * i
        events.extend([(start - 1, 'ASSIGNMENT_CREATED', dict(assignment_ref=ref)),
                       (start, 'ENGINE_READY', dict(assignment_ref=ref, attempt_id=w.attempt_id, engine_session_id=f's{i}')),
                       (start + 50, 'WORKER_COMPLETED', dict(assignment_ref=ref, attempt_id=w.attempt_id))])
        _insert_observation(store, task, w.attempt_id, f't{i}a', 'read', 'completed', _ts(start + 10))
        _insert_observation(store, task, w.attempt_id, f't{i}b', 'read', 'completed', _ts(start + 35))
    _node(store, task, 'review', 'review')
    events.extend([(110, 'ASSIGNMENT_CREATED', dict(assignment_ref='review')),
                   (111, 'ENGINE_READY', dict(assignment_ref='review', attempt_id='rv', engine_session_id='srv')),
                   (112, 'REVIEW_STARTED', dict(assignment_ref='review')),
                   (120, 'WORKER_COMPLETED', dict(assignment_ref='review', attempt_id='rv'))])
    _insert_observation(store, task, 'rv', 'rva', 'read', 'completed', _ts(113))
    _insert_observation(store, task, 'rv', 'rvb', 'read', 'completed', _ts(118))
    _stream(store, task, events)
    result = metrics._loop_profile(store, task)
    execution = [a for a in result['per_attempt'] if a['stage'] == 'execution']
    assert len(execution) == 2
    assert [a['inference_gap_share_of_execution'] for a in execution] == [.5, .5]
    assert next(a for a in result['per_attempt'] if a['attempt_id'] == 'rv')['stage'] == 'review'


def test_external_durations_bind_actor_server_and_unique_ordered_boundaries(bound, monkeypatch):
    store, state, coordinator, workers = bound
    for i, w in enumerate(workers):
        for j, server in enumerate(('index', 'other')):
            start = 1 + 10 * i + 3 * j
            _insert_event(store, w.task_id, 'EXTERNAL_TOOL_AUTHORIZED', _ts(start),
                          dict(server=server, request_id=1), w.channel_id)
            _insert_event(store, w.task_id, 'EXTERNAL_TOOL_RESULT', _ts(start + 1),
                          dict(server=server, request_id=1), w.channel_id)
    times, truncated, invalid = metrics._call_durations(store, coordinator.task_id, external=True)
    assert times == {w.attempt_id: 2 for w in workers}
    assert not truncated and not invalid
    _insert_event(store, coordinator.task_id, 'EXTERNAL_TOOL_RESULT', _ts(30),
                  dict(server='index', request_id=1), workers[0].channel_id)
    times, _, invalid = metrics._call_durations(store, coordinator.task_id, external=True)
    assert workers[0].attempt_id not in times and workers[0].attempt_id in invalid
    monkeypatch.setattr(metrics, '_MAX_LOOP_PROFILE_ROWS', 1)
    result = metrics._loop_profile(store, coordinator.task_id)
    assert result['external_duration_rows_truncated']
    # The gateway query has its own truncation flag.
    for i in range(2):
        _insert_event(store, coordinator.task_id, 'TOOL_AUTHORIZED', _ts(31 + i),
                      dict(tool_call_ref=f'g{i}'), workers[0].channel_id)
    assert metrics._loop_profile(store, coordinator.task_id)['gateway_duration_rows_truncated']


@pytest.mark.parametrize('engine', ['claude-agent-acp', 'codex-acp', 'opencode'])
def test_gateway_edit_echo_not_native_and_project_write_population_separate(bound, engine):
    store, state, coordinator, workers = bound
    w = workers[0]
    _insert_route(store, w.task_id, w.attempt_id, engine, _ts(0))
    for i, (op, changed) in enumerate([('project_write', 10), ('project_edit', 1000)]):
        _insert_event(store, w.task_id, 'TOOL_COMPLETED', _ts(i),
                      dict(operation=op, tool_call_ref=f'g{i}', result=dict(byte_count=1000, changed_bytes=changed)),
                      w.channel_id)
        _insert_observation(store, w.task_id, w.attempt_id, f'g{i}', 'edit', 'completed', _ts(i))
    result = metrics._write_redundancy(store, w.task_id)
    assert result['project_write_redundancy_ratio'] == 100
    assert result['redundancy_ratio'] == pytest.approx(2000 / 1010)
    assert result['native_write_share']['availability'] == 'unavailable_reason'
    assert result['project_write_share_of_all_writes'] == (.5 if engine == 'claude-agent-acp' else None)


def test_repeat_text_reads_require_another_channel_and_unique_acp_duration(bound):
    store, state, coordinator, workers = bound
    task = coordinator.task_id
    for i, w in enumerate(workers):
        _insert_route(store, task, w.attempt_id, 'claude-agent-acp', _ts(0))
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET dependencies_json='[\"N0\"]' WHERE node_id='N1'")
        refs = {r['attempt_id']: r['idempotency_key'] for r in db.execute("SELECT * FROM delegation_provisioning")}
        db.commit()
    events = []
    for w in workers:
        events.extend([(0, 'ASSIGNMENT_CREATED', dict(assignment_ref=refs[w.attempt_id])),
                       (1, 'ENGINE_READY', dict(assignment_ref=refs[w.attempt_id], attempt_id=w.attempt_id,
                                                engine_session_id=w.attempt_id)),
                       (21, 'WORKER_COMPLETED', dict(assignment_ref=refs[w.attempt_id], attempt_id=w.attempt_id))])
    _stream(store, task, events)
    for i, w in enumerate(workers):
        with store.connection() as db:
            observability_events.append_event_tx(
                db, task, 'project_read', 'text', 'observed', created_at=_ts(3 + 5 * i),
                attempt_id=w.attempt_id, detail=dict(path='x', sha256='same'))
            db.commit()
    w = workers[1]
    _insert_observation(store, task, w.attempt_id, 'read1', 'read', 'in_progress', _ts(6))
    _insert_observation(store, task, w.attempt_id, 'read1', 'read', 'completed', _ts(10))
    result = metrics._reinvestigation(store, task)
    read = next(a for a in result['per_attempt'] if a['attempt_id'] == w.attempt_id)
    assert read['dependent'] is True
    assert read['repeat_ratio'] == 1
    assert read['repeat_time_share_of_execution'] == .2
    _insert_observation(store, task, w.attempt_id, 'ambiguous', 'read', 'in_progress', _ts(7))
    _insert_observation(store, task, w.attempt_id, 'ambiguous', 'read', 'completed', _ts(9))
    read = next(a for a in metrics._reinvestigation(store, task)['per_attempt'] if a['attempt_id'] == w.attempt_id)
    assert read['repeat_time_share_of_execution'] is None


def test_cross_task_repeat_ratio_uses_per_call_history_and_no_history_is_unavailable(machine):
    state, ch = machine
    assert metrics._cross_task_reads(state.store)['availability'] == 'unavailable_reason'
    _insert_read(state.store, ch.task_id, 'a', 'x', 'sha')
    assert metrics._cross_task_reads(state.store)['availability'] == 'unavailable_reason'
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    second = bootstrap_task(state.store, SourceEvent(origin='synthetic', event_id='second'), 'Synthetic', 'owner')
    for path, sha in [('x', 'sha'), ('x', 'sha'), ('new', 'different')]:
        _insert_read(state.store, second.task_id, 'b', path, sha)
    result = metrics._cross_task_reads(state.store)
    assert result['availability'] == 'available'
    assert result['sample_count'] == 4
    assert result['ratio'] == .5
    assert result['repeat_count'] == 2


def test_phase_chain_needs_completed_task_exact_review_and_verification_revision(machine, monkeypatch):
    state, ch = machine
    _node(state.store, ch.task_id, 'w')
    _node(state.store, ch.task_id, 'r', 'review')
    events = [
        (0, 'ASSIGNMENT_CREATED', dict(assignment_ref='w')),
        (1, 'ENGINE_READY', dict(assignment_ref='w', attempt_id='w', engine_session_id='sw')),
        (3, 'WORKER_COMPLETED', dict(assignment_ref='w', attempt_id='w')),
        (4, 'ASSIGNMENT_CREATED', dict(assignment_ref='r')),
        (5, 'ENGINE_READY', dict(assignment_ref='r', attempt_id='r', engine_session_id='sr')),
        (5, 'REVIEW_STARTED', dict(assignment_ref='r')),
        (6, 'WORKER_COMPLETED', dict(assignment_ref='r', attempt_id='r')),
        (7, 'TASK_STATE_CHANGED', dict(state='VERIFYING')),
        (8, 'VERIFICATION_PASSED', dict(revision=0)),
        (9, 'TASK_STATE_CHANGED', dict(state='COMPLETED'))]
    _stream(state.store, ch.task_id, events)
    result = metrics._critical_path(state.store, ch.task_id)
    assert not result['per_attempt'][0]['phase_chain_complete']
    with state.store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id=?", (ch.task_id,))
        db.commit()
    result = metrics._critical_path(state.store, ch.task_id)
    assert result['per_attempt'][0]['phase_chain_complete']
    events[-2] = (8, 'VERIFICATION_PASSED', dict(revision=99))
    _stream(state.store, ch.task_id, events)
    assert not metrics._critical_path(state.store, ch.task_id)['per_attempt'][0]['phase_chain_complete']
    events[-2] = (8, 'VERIFICATION_PASSED', dict(revision=0))
    _stream(state.store, ch.task_id, events)
    monkeypatch.setattr(metrics, '_MAX_CRITICAL_PATH_EVENTS', 3)
    result = metrics._critical_path(state.store, ch.task_id)
    assert result['truncated']
    assert not any(a['phase_chain_complete'] for a in result['per_attempt'])
    assert result['classes']['worker_execution']['seconds'] is None
