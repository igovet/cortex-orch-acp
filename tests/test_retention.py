from storage_fixture import call_store, thread_for
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

import pytest
import cortex_clear
from cortex_runtime.cleanup import clear_tasks
from cortex_runtime.contracts import StoreError
from cortex_runtime.store import Store


def test_retention_deletes_artifacts_and_receipts_but_keeps_recent_active_foreign(tmp_path):
    store=Store(tmp_path/'private');project=tmp_path/'project';project.mkdir();foreign=tmp_path/'foreign';foreign.mkdir()
    now=datetime.now(timezone.utc)
    def create(path,label,days):
        time=(now-timedelta(days=days)).isoformat(timespec='microseconds')
        with patch.object(Store,'_now',return_value=time):
            return call_store(store,'create_task',dict(project_root=str(path),request=label,request_key=label))['task_id']
    old=create(project,'old',10);recent=create(project,'recent',2);active=create(project,'active',20);other=create(foreign,'other',30);updated=create(project,'updated',15);drafted=create(project,'drafted',15)
    def report(task,path,name,body,key):
        created=call_store(store,'create_draft',dict(task_id=task,template='general',request_key='draft-'+key))
        draft=Path(created['draft_path']);marker=draft.read_text().split('\n\n',1)[0]+'\n\n';draft.write_text(marker+body)
        return call_store(store,'write_report',dict(task_id=task,title='Evidence',summary='Result',author='worker',draft_id=created['draft_id'],request_key=key)),draft
    with patch.object(Store,'_now',return_value=(now-timedelta(days=10)).isoformat(timespec='microseconds')):
        _,old_source=report(old,project,'old-report.md','accepted report','old-report')
    with store.connection() as db:
        row=db.execute('SELECT id,path FROM drafts WHERE task_id=? ORDER BY created_at DESC LIMIT 1',(old,)).fetchone()
    old_source=Path(row['path']); old_source.write_text(f"Cortex draft ID: `{row['id']}`\n\naccepted report")
    report(drafted,project,'current.md','recent activity','recent-draft')
    report(updated,project,'updated.md','Done','new')
    result=clear_tasks(store,str(project),7,[thread_for(store,active)],now)
    assert result['deleted_tasks']==1 and result['skipped_protected']==1
    assert not (project/'.cortex'/old).exists() and not old_source.exists()
    for task in [recent,active,other,updated,drafted]:assert call_store(store,'list_reports',dict(task_id=task))['reports']
    with pytest.raises(StoreError,match='task_not_bound'):call_store(store,'list_reports',dict(task_id=old))
    with store.connection() as db:
        assert not db.execute('SELECT 1 FROM deliveries WHERE delivery_key=?',('old',)).fetchone()
        assert not db.execute('SELECT 1 FROM drafts WHERE task_id=?',(old,)).fetchone()
        assert db.execute('PRAGMA foreign_key_check').fetchall()==[]
    assert clear_tasks(store,str(project),7,[thread_for(store,active)],now)['deleted_tasks']==0


def test_committed_deletion_recovers_after_restart(tmp_path):
    store=Store(tmp_path/'private')
    task=call_store(store,'create_task',dict(project_root=str(tmp_path),request='Old',request_key='old'))['task_id']
    with store.connection() as db:db.execute('INSERT INTO pending_deletions VALUES (?)',(task,))
    restarted=Store(store.directory)
    with pytest.raises(StoreError,match='task_not_bound'):call_store(restarted,'list_reports',dict(task_id=task))
    assert not (tmp_path/'.cortex'/task).exists()


def test_retention_command_uses_only_selected_project_store(tmp_path, monkeypatch, capsys):
    import json
    project=tmp_path/'project';project.mkdir()
    foreign=tmp_path/'foreign';foreign.mkdir()
    local=Store(project/'.cortex',project_root=str(project))
    global_store=Store(tmp_path/'legacy-global')
    old=(datetime.now(timezone.utc)-timedelta(days=10)).isoformat(timespec='microseconds')
    with patch.object(Store,'_now',return_value=old):
        local_task=call_store(local,'create_task',dict(project_root=str(project),request='Local',request_key='local'))['task_id']
        foreign_task=call_store(global_store,'create_task',dict(project_root=str(foreign),request='Foreign',request_key='foreign'))['task_id']
    before=global_store.path.read_bytes()
    monkeypatch.setenv('CORTEX_DATA_DIR',str(global_store.directory))
    monkeypatch.setenv('CODEX_HOME',str(tmp_path/'missing-host-index'))
    monkeypatch.setattr('sys.argv',['cortex_clear.py','--project-root',str(project),'--days','7'])
    cortex_clear.main()
    assert json.loads(capsys.readouterr().out)['deleted_tasks']==1
    assert not (project/'.cortex'/local_task).exists()
    assert (foreign/'.cortex'/foreign_task).exists()
    assert global_store.path.read_bytes()==before


def test_retention_deletes_configuration_revisions_with_task_but_keeps_project_defaults(tmp_path):
    from cortex_runtime.configuration import store as configuration_store
    from cortex_runtime.configuration.policy import PolicyCeiling
    from cortex_runtime.portable.identity import ExecutionRoute

    store=Store(tmp_path/'private');project=tmp_path/'project';project.mkdir()
    task=call_store(store,'create_task',dict(project_root=str(project),request='r',request_key='k'))['task_id']
    configuration_store.write_project_defaults(store,str(project),{'effort':'medium'},PolicyCeiling())
    route=ExecutionRoute(engine='codex-acp',connection_id='cn_'+'a'*12,model='model-a')
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        configuration_store.insert_revision_tx(db,task,1,route,{},PolicyCeiling())
        db.commit()
    clear_tasks(store,str(project),0)
    with store.connection() as db:
        assert not db.execute('SELECT 1 FROM configuration_revisions WHERE task_id=?',(task,)).fetchone()
    assert configuration_store.read_project_defaults(store,str(project)) is not None


def test_retention_command_inactive_project_has_no_storage_side_effects(tmp_path, monkeypatch, capsys):
    import json
    monkeypatch.setattr('sys.argv',['cortex_clear.py','--project-root',str(tmp_path),'--days','7'])
    cortex_clear.main()
    assert json.loads(capsys.readouterr().out)['deleted_tasks']==0
    assert not (tmp_path/'.codex').exists()
    monkeypatch.setattr('sys.argv',['cortex_clear.py','--project-root',str(tmp_path),'--days','-1'])
    with pytest.raises(SystemExit,match='retention cleanup failed'):
        cortex_clear.main()
    assert not (tmp_path/'.codex').exists()


def _tables_transitively_fk_to(store, root):
    """Every table with a foreign key chain (one or more hops) ending at
    ``root`` -- the authoritative set that must be emptied before ``root``
    rows can be deleted under FK enforcement."""
    with store.connection() as db:
        tables = [row[0] for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
        edges = {}
        for table in tables:
            edges[table] = {row[2] for row in db.execute(f"PRAGMA foreign_key_list({table})")}
    reach = set()
    changed = True
    while changed:
        changed = False
        for table, targets in edges.items():
            if table in reach:
                continue
            if root in targets or targets & reach:
                reach.add(table)
                changed = True
    return reach


def test_finish_deletions_covers_every_fk_to_tasks_table(tmp_path):
    """Fails closed if any table added to the composed schema (a new
    optional group, T015-T020 or later) has a foreign-key chain to
    ``tasks`` but is not handled by ``cleanup.finish_deletions`` -- the
    reproduced T020 fresh-review gap (task_stopped/task_language/
    pending_questions/pending_question_late_answers left out) is exactly
    what this guards against for every future addition too."""
    import inspect
    from cortex_runtime import cleanup as cleanup_module

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    required = _tables_transitively_fk_to(store, "tasks")
    source = inspect.getsource(cleanup_module.finish_deletions)
    missing = [table for table in sorted(required) if table not in source]
    assert not missing, f"finish_deletions does not reference: {missing}"


def test_finish_deletions_covers_fk_children_of_every_deleted_table(tmp_path):
    """Lazily created tables (e.g. the instruction-bundle ledger) carry no
    FK to ``tasks`` yet may be FK children of a table finish_deletions
    deletes from; each such child must be deleted too, or replay of a
    committed deletion intent fails with FOREIGN KEY constraint failed."""
    import inspect
    import re
    from cortex_runtime import cleanup as cleanup_module
    from cortex_runtime.portable.instruction_bundle import InstructionBundleLedger

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    with store.connection() as db:
        InstructionBundleLedger(store)._schema(db)
        tables = [row[0] for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
        edges = {table: {row[2] for row in db.execute(f"PRAGMA foreign_key_list({table})")} for table in tables}
    source = inspect.getsource(cleanup_module.finish_deletions)
    # Direct DELETEs, _delete_if_present("<table>", ...) and the quoted
    # table names iterated by f-string DELETE loops.
    deleted = set(re.findall(r'DELETE FROM (\w+)', source)) | (set(re.findall(r'"(\w+)"', source)) & set(tables))
    missing = sorted(table for table, targets in edges.items() if targets & deleted and table not in deleted)
    assert not missing, f"finish_deletions does not delete FK children: {missing}"


def test_clear_tasks_removes_instruction_bundle_deliveries(tmp_path):
    from cortex_runtime.portable.instruction_bundle import InstructionBundleLedger

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='Old', request_key='old'))['task_id']
    now = store._now()
    with store.connection() as db:
        InstructionBundleLedger(store)._schema(db)
        db.execute(
            "INSERT INTO instruction_bundles (bundle_id,task_id,assignment_ref,attempt_id,role_id,role_kind,engine,engine_version,"
            "assembled_text,assembled_sha256,sources_json,intended_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            ('ib_1', task, 'as_1', 'at_1', 'backend-dev', 'specialist', 'codex', '1', 'x', 'h', '[]', '{}', now, now))
        db.execute(
            "INSERT INTO instruction_bundle_deliveries (bundle_id,session_ref,representation,representation_sha256,engine,engine_version,"
            "attempt_id,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
            ('ib_1', 's_1', b'x', 'h', 'codex', '1', 'at_1', now, now))
    assert clear_tasks(store, str(tmp_path), 0)['deleted_tasks'] == 1
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM instruction_bundles").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM instruction_bundle_deliveries").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM pending_deletions").fetchone()[0] == 0
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []


def test_clear_tasks_end_to_end_removes_every_group_row_with_integrity_ok(tmp_path):
    """Populate one row in every T005-T020 optional-group table for a
    single task (bootstrap for identity/ownership, direct inserts for the
    rest -- fast and deterministic, matching this codebase's own
    _publish()-style fixture convention elsewhere), stop it, set its
    language, add a pending question, then clear_tasks(..., 0) must
    succeed with zero leftover rows anywhere and a clean
    PRAGMA foreign_key_check."""
    import uuid
    from cortex_runtime.acp import steering as acp_steering
    from cortex_runtime.acp import questions_store
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent

    project = tmp_path / 'project'; project.mkdir()
    store = Store(project / '.cortex', project_root=str(project))

    result = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), 'Build it.', 'owner-1')
    task = result.task_id

    now = store._now()
    with store.connection() as db:
        report_id = db.execute("SELECT id FROM reports WHERE task_id=?", (task,)).fetchone()["id"]
        db.execute("BEGIN IMMEDIATE")
        try:
            # delegation provisioning + effects + attempt_revisions + capacity + tool observations
            db.execute(
                "INSERT INTO delegation_provisioning (idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) "
                "VALUES (?,?,?,?,?,?,?,?,?)", ('dp_1', task, 'at_1', 'backend-dev', 'attempt_opened', 'running', 1, now, now))
            db.execute(
                "INSERT INTO delegation_effects (idempotency_key,step,phase,detail_json,at) VALUES (?,?,?,?,?)",
                ('dp_1', 'attempt_opened', 'intent', '{}', now))
            db.execute(
                "INSERT INTO attempt_revisions (attempt_id,captured_at,phase,kind) VALUES (?,?,?,?)",
                ('at_1', now, 'worker_result', 'git'))
            db.execute(
                "INSERT INTO capacity_reservations (reservation_id,connection_id,task_id,attempt_id,kind,state,reserved_at,updated_at) "
                "VALUES (?,?,?,?,?,?,?,?)", ('res_1', 'cn_1', task, 'at_1', 'worker', 'reserved', now, now))
            db.execute(
                "INSERT INTO tool_result_observations (task_id,attempt_id,tool_call_ref,status,created_at) VALUES (?,?,?,?,?)",
                (task, 'at_1', 'tc_1', 'completed', now))
            # task_events / task_orchestration
            db.execute(
                "INSERT INTO task_orchestration (task_id,state,updated_at) VALUES (?,?,?)",
                (task, 'running', now))
            db.execute(
                "INSERT INTO task_events (task_id,event_type,dedup_key,payload_json,origin,created_at) VALUES (?,?,?,?,?,?)",
                (task, 'user_message', 'user_message:cli:1', '{}', 'cli', now))
            # acp session + host transcript
            db.execute(
                "INSERT INTO acp_host_sessions (session_id,project_root,task_id,client_info,client_capabilities,mcp_servers,created_at) "
                "VALUES (?,?,?,?,?,?,?)", ('acps_1', str(project), task, '{}', '{}', '[]', now))
            db.execute(
                "INSERT INTO host_transcript (session_id,seq,kind,text,created_at) VALUES (?,?,?,?,?)",
                ('acps_1', 1, 'user_message', 'hi', now))
            # source_blocks + source_block_reads
            db.execute(
                "INSERT INTO source_blocks (task_id,report_id,block_index,kind,availability,trust_label,created_at) "
                "VALUES (?,?,?,?,?,?,?)", (task, report_id, 0, 'resource', 'inaccessible', 'untrusted', now))
            db.execute(
                "INSERT INTO source_block_reads (report_id,block_index,observed_at,availability) VALUES (?,?,?,?)",
                (report_id, 0, now, 'inaccessible'))
            # route selection + configuration revision
            db.execute(
                "INSERT INTO task_route_selections (task_id,sequence,route_id,label_at_selection,supported_parameters,catalog_snapshot_id,evidence,selected_at) "
                "VALUES (?,?,?,?,?,?,?,?)", (task, 1, 'rt_1', 'Model A', '[]', 'cs_1', '{}', now))
            db.execute(
                "INSERT INTO configuration_revisions (id,task_id,revision,route_id,route_engine,route_connection_id,parameters,ceiling_snapshot,state,created_at,updated_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                ('cr_1', task, 1, 'rt_1', 'codex-acp', 'cn_1', '{}', '{}', 'selected', now, now))
            # governance
            db.execute("INSERT INTO governance (id,task_id,mode,report_id) VALUES (?,?,?,?)", ('gov_1', task, 'normal', report_id))
            # T022 context_generations/compaction_owners/compaction_events_seen/capsule_deliveries
            db.execute(
                "INSERT INTO context_generations (id,engine_session_ref,task_id,assignment_ref,recipient,sequence,cause,evidence_source,evidence_id,created_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?)", ('cg_1', 'es_1', task, None, 'coordinator', 1, 'session_open', 'test', None, now))
            db.execute(
                "INSERT INTO compaction_owners (engine_session_ref,task_id,owner,tier,engine,engine_version,limitation,set_at) "
                "VALUES (?,?,?,?,?,?,?,?)", ('es_1', task, 'engine_adapter', 'automatic', 'codex-acp', '1.12.0', None, now))
            db.execute(
                "INSERT INTO compaction_events_seen (engine_session_ref,evidence_id,task_id,phase,observed_at,generation_id) "
                "VALUES (?,?,?,?,?,?)", ('es_1', 'tc_compact_1', task, 'completed', now, 'cg_1'))
            db.execute(
                "INSERT INTO capsule_deliveries (id,recipient,context_generation_id,capsule_revision,task_id,assignment_ref,engine_session_ref,state,evidence_source,uncertainty,reason,retry_count,created_at,updated_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                ('cd_1', 'coordinator', 'cg_1', 'rev_1', task, None, 'es_1', 'built', 'test', None, None, 0, now, now))
            db.commit()
        except BaseException:
            db.rollback(); raise

    # pending question + language + stop, through the real public helpers.
    question = questions_store.create_if_absent(store, task, 'ask_user:1', 'Which env?')
    questions_store.record_late_answer(store, question.id, task, 'late', 'text')
    acp_steering.set_language_override(store, task, 'ru')
    assert acp_steering.mark_stopped(store, task) is True

    # (table, where_clause, params) -- a handful of tables key off a
    # different column than task_id (see the schema notes in cleanup.py).
    _lookups = [
        ('delegation_provisioning', 'task_id=?', (task,)),
        ('delegation_effects', "idempotency_key=?", ('dp_1',)),
        ('attempt_revisions', 'attempt_id=?', ('at_1',)),
        ('capacity_reservations', 'task_id=?', (task,)),
        ('tool_result_observations', 'task_id=?', (task,)),
        ('task_orchestration', 'task_id=?', (task,)),
        ('task_events', 'task_id=?', (task,)),
        ('acp_host_sessions', 'task_id=?', (task,)),
        ('host_transcript', 'session_id=?', ('acps_1',)),
        ('source_blocks', 'task_id=?', (task,)),
        ('source_block_reads', 'report_id=?', (report_id,)),
        ('task_route_selections', 'task_id=?', (task,)),
        ('configuration_revisions', 'task_id=?', (task,)),
        ('governance', 'task_id=?', (task,)),
        ('pending_questions', 'task_id=?', (task,)),
        ('pending_question_late_answers', 'task_id=?', (task,)),
        ('task_language', 'task_id=?', (task,)),
        ('task_stopped', 'task_id=?', (task,)),
        ('trusted_channels', 'task_id=?', (task,)),
        ('source_events', 'task_id=?', (task,)),
        ('task_owners', 'task_id=?', (task,)),
        ('reports', 'task_id=?', (task,)),
        ('thread_bindings', 'task_id=?', (task,)),
        ('context_generations', 'task_id=?', (task,)),
        ('compaction_owners', 'task_id=?', (task,)),
        ('compaction_events_seen', 'task_id=?', (task,)),
        ('capsule_deliveries', 'task_id=?', (task,)),
    ]
    with store.connection() as db:
        populated = {t for t, where, params in _lookups if db.execute(f"SELECT 1 FROM {t} WHERE {where}", params).fetchone()}
    assert populated == {t for t, _, _ in _lookups}, f"fixture setup incomplete: {populated}"

    clear_tasks(store, str(project), 0)

    with store.connection() as db:
        assert not db.execute("SELECT 1 FROM tasks WHERE id=?", (task,)).fetchone()
        for table in sorted(_tables_transitively_fk_to(store, "tasks")):
            columns = {row[1] for row in db.execute(f"PRAGMA table_info({table})")}
            if "task_id" in columns:
                assert not db.execute(f"SELECT 1 FROM {table} WHERE task_id=?", (task,)).fetchone(), table
        assert not db.execute("SELECT 1 FROM pending_question_late_answers WHERE task_id=?", (task,)).fetchone()
        assert not db.execute("SELECT 1 FROM host_transcript WHERE session_id=?", ('acps_1',)).fetchone()
        assert not db.execute("SELECT 1 FROM source_block_reads WHERE report_id=?", (report_id,)).fetchone()
        assert not db.execute("SELECT 1 FROM attempt_revisions WHERE attempt_id=?", ('at_1',)).fetchone()
        assert db.execute("PRAGMA foreign_key_check").fetchall() == []
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"


def test_clear_absolute_dry_run_and_protected_ids(tmp_path):
    store = Store(tmp_path / 'private')
    cutoff = datetime(2026, 10, 1, tzinfo=timezone.utc)
    now = cutoff + timedelta(hours=1)
    def create(label, at):
        with patch.object(Store, '_now', return_value=at.isoformat(timespec='microseconds')):
            return call_store(store, 'create_task', dict(project_root=str(tmp_path), request=label, request_key=label))['task_id']
    old = create('private-old-content', cutoff - timedelta(days=1))
    kept = create('private-kept-content', cutoff - timedelta(days=1))
    boundary = create('boundary', cutoff)
    artifacts = {p: p.read_bytes() for task in (old, kept, boundary)
                 for p in (tmp_path / '.cortex' / task).rglob('*') if p.is_file()}
    result = clear_tasks(store, str(tmp_path), cutoff=cutoff, now=now,
                         dry_run=True, protected_task_ids={kept}, protect_active=True)
    assert result['deleted_tasks'] == 0
    assert result['would_delete_tasks'] == 1
    assert result['skipped_protected'] == 1
    assert all(p.read_bytes() == content for p, content in artifacts.items())
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM pending_deletions').fetchone()[0] == 0
        assert db.execute('SELECT COUNT(*) FROM tasks').fetchone()[0] == 3
    result = clear_tasks(store, str(tmp_path), cutoff=cutoff, now=now,
                         protected_task_ids={kept}, protect_active=True)
    assert result['deleted_tasks'] == 1 and result['skipped_protected'] == 1
    with store.connection() as db:
        assert {r[0] for r in db.execute('SELECT id FROM tasks')} == {kept, boundary}
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []


@pytest.mark.parametrize('state', ['CREATED', 'PLANNING', 'PLAN_READY', 'ANSWER_READY', 'EXECUTING',
                                  'REVIEWING', 'VERIFYING', 'RECONCILING', 'WAITING_USER',
                                  'RECOVERING', 'BLOCKED', 'UNKNOWN'])
def test_chat_retention_protects_every_nonterminal_state(tmp_path, state):
    from cortex_runtime.orchestration.state import SCHEMA
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    with store.connection() as db:
        db.executescript(SCHEMA)
        db.execute('INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)',
                   (task, state, 'private', store._now(), store._now()))
    result = clear_tasks(store, str(tmp_path), 0, protect_active=True)
    assert result['deleted_tasks'] == 0 and result['skipped_protected'] == 1
    assert (tmp_path / '.cortex' / task).exists()


@pytest.mark.parametrize('kind', ['worker', 'coordinator'])
@pytest.mark.parametrize('state', ['reserved', 'ambiguous'])
def test_chat_retention_protects_terminal_task_with_live_capacity(tmp_path, kind, state):
    from cortex_runtime.orchestration.state import SCHEMA
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    with store.connection() as db:
        db.executescript(SCHEMA)
        db.execute('INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)',
                   (task, 'CANCELLED', 'private', store._now(), store._now()))
        db.execute('INSERT INTO capacity_reservations(reservation_id,connection_id,task_id,attempt_id,kind,state,reserved_at,updated_at) VALUES(?,?,?,?,?,?,?,?)',
                   ('r1', 'cn1', task, 'a1', kind, state, store._now(), store._now()))
    assert clear_tasks(store, str(tmp_path), 0, protect_active=True)['skipped_protected'] == 1
    with store.connection() as db:
        db.execute("UPDATE capacity_reservations SET state='released'")
    assert clear_tasks(store, str(tmp_path), 0, protect_active=True)['deleted_tasks'] == 1


def test_chat_retention_protects_unterminated_process(tmp_path):
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    with store.connection() as db:
        db.execute('INSERT INTO attempt_process_registry(attempt_id,task_id,role,pid,pgid,start_ticks,boot_id,launch_command_digest,recorded_at) VALUES(?,?,?,?,?,?,?,?,?)',
                   ('a1', task, 'orchestrator', 1, 1, '0', 'fake', 'fake', store._now()))
    assert clear_tasks(store, str(tmp_path), 0, protect_active=True)['skipped_protected'] == 1
    with store.connection() as db:
        db.execute('UPDATE attempt_process_registry SET terminated_at=?', (store._now(),))
    assert clear_tasks(store, str(tmp_path), 0, protect_active=True)['deleted_tasks'] == 1


def test_retention_refuses_concurrent_store_access_and_protected_pending_intent(tmp_path):
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    with store.connection():
        with pytest.raises(StoreError, match='storage_busy'):
            clear_tasks(store, str(tmp_path), 0)
    with store.connection() as db:
        db.execute('INSERT INTO pending_deletions VALUES (?)', (task,))
    for dry_run in (True, False):
        with pytest.raises(StoreError, match='retention_active_conflict'):
            clear_tasks(store, str(tmp_path), 0, dry_run=dry_run, protected_task_ids={task})
    assert (tmp_path / '.cortex' / task).exists()


def test_dry_run_does_not_replay_committed_intents(tmp_path):
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    with store.connection() as db:
        db.execute('INSERT INTO pending_deletions VALUES (?)', (task,))
    result = clear_tasks(store, str(tmp_path), 7, dry_run=True)
    assert result['would_delete_tasks'] == 1 and result['deleted_tasks'] == 0
    assert (tmp_path / '.cortex' / task).exists()
    assert clear_tasks(store, str(tmp_path), 7)['deleted_tasks'] == 1


@pytest.mark.parametrize('kwargs', [dict(days=-1), dict(days=36501), dict(days=True),
                                    dict(cutoff=datetime(2999, 1, 1, tzinfo=timezone.utc)),
                                    dict(cutoff=datetime(2026, 1, 1)),
                                    dict(days=1, cutoff=datetime(2026, 1, 1, tzinfo=timezone.utc))])
def test_invalid_retention_does_not_delete(tmp_path, kwargs):
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    with pytest.raises(StoreError, match='invalid_retention'):
        clear_tasks(store, str(tmp_path), **kwargs)
    assert (tmp_path / '.cortex' / task).exists()


def test_retention_holds_exclusive_lock_through_filesystem_teardown(tmp_path, monkeypatch):
    from cortex_runtime import cleanup
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    original = cleanup.finish_deletions
    seen = []
    def fenced_finish(store, db, selected):
        with pytest.raises(StoreError, match='storage_busy'):
            with store.connection():
                pytest.fail('resuming store access must be fenced during teardown')
        seen.append(selected)
        original(store, db, selected)
    monkeypatch.setattr(cleanup, 'finish_deletions', fenced_finish)
    assert clear_tasks(store, str(tmp_path), 0)['deleted_tasks'] == 1
    assert seen == [task]


def test_retention_failure_keeps_recoverable_intent(tmp_path, monkeypatch):
    from cortex_runtime import cleanup
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='private', request_key='k'))['task_id']
    def fail(*args):
        raise OSError('simulated teardown failure')
    with monkeypatch.context() as m:
        m.setattr(cleanup, 'finish_deletions', fail)
        with pytest.raises(OSError, match='simulated'):
            clear_tasks(store, str(tmp_path), 0, protect_active=True)
    with store.connection() as db:
        assert {r[0] for r in db.execute('SELECT task_id FROM pending_deletions')} == {task}
    assert clear_tasks(store, str(tmp_path), 0, protect_active=True)['deleted_tasks'] == 1


@pytest.mark.parametrize('state', ['EXECUTING', 'UNKNOWN'])
@pytest.mark.parametrize('pending', [False, True])
def test_force_cleans_live_records_but_never_explicit_keeps(tmp_path, monkeypatch, state, pending):
    from cortex_runtime import cleanup
    from cortex_runtime.orchestration.state import SCHEMA
    store = Store(tmp_path / 'private')
    tasks = [call_store(store, 'create_task', dict(project_root=str(tmp_path), request=key, request_key=key))['task_id']
             for key in ('stale', 'invoking')]
    stale, invoking = tasks
    with store.connection() as db:
        db.executescript(SCHEMA)
        for task in tasks:
            db.execute('INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)',
                       (task, state, 'private', store._now(), store._now()))
            db.execute('INSERT INTO capacity_reservations(reservation_id,connection_id,task_id,attempt_id,kind,state,reserved_at,updated_at) VALUES(?,?,?,?,?,?,?,?)',
                       (task, 'cn1', task, task, 'worker', 'ambiguous', store._now(), store._now()))
            db.execute('INSERT INTO attempt_process_registry(attempt_id,task_id,role,pid,pgid,start_ticks,boot_id,launch_command_digest,recorded_at) VALUES(?,?,?,?,?,?,?,?,?)',
                       (task, task, 'orchestrator', 1, 1, '0', 'fake', 'fake', store._now()))
        if pending:
            db.execute('INSERT INTO pending_deletions VALUES (?)', (stale,))
    options = dict(force=True, protect_active=True, protected_task_ids=tasks,
                   keep_threads=(thread_for(store, invoking),))
    preview = clear_tasks(store, str(tmp_path), 0, dry_run=True, **options)
    assert preview['would_delete_tasks'] == 1 and preview['skipped_protected'] == 1
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM capacity_reservations WHERE state='ambiguous'").fetchone()[0] == 2
        assert db.execute('SELECT COUNT(*) FROM attempt_process_registry WHERE terminated_at IS NULL').fetchone()[0] == 2
    original = cleanup.finish_deletions
    def checked_finish(store, db, task):
        assert task == stale
        row = db.execute('SELECT state,release_observation FROM capacity_reservations WHERE task_id=?', (task,)).fetchone()
        assert tuple(row) == ('released', 'owner_force_retention_assumed_dead')
        row = db.execute('SELECT terminated_at,termination_reason FROM attempt_process_registry WHERE task_id=?', (task,)).fetchone()
        assert row[0] and row[1] == 'owner_force_retention_assumed_dead'
        original(store, db, task)
    monkeypatch.setattr(cleanup, 'finish_deletions', checked_finish)
    result = clear_tasks(store, str(tmp_path), 0, **options)
    assert result['deleted_tasks'] == 1 and result['force']
    with store.connection() as db:
        assert {row[0] for row in db.execute('SELECT id FROM tasks')} == {invoking}
        assert db.execute('SELECT terminated_at FROM attempt_process_registry').fetchone()[0] is None
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []


def test_force_still_refuses_kept_pending_intent(tmp_path):
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='k', request_key='k'))['task_id']
    with store.connection() as db:
        db.execute('INSERT INTO pending_deletions VALUES (?)', (task,))
    for dry_run in (True, False):
        with pytest.raises(StoreError, match='retention_active_conflict'):
            clear_tasks(store, str(tmp_path), 0, force=True, dry_run=dry_run, keep_task_ids=(task,))
    assert (tmp_path / '.cortex' / task).exists()


def _retention_session(store, project, sid, created, *, transcript=None, update=None, closed=None):
    from cortex_runtime.acp.sessions import create_session
    create_session(store, sid, str(project), {}, {}, [], workspace_root=str(project))
    with store.connection() as db:
        db.execute('UPDATE acp_host_sessions SET created_at=?,closed_at=? WHERE session_id=?', (created, closed, sid))
        if transcript:
            db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,created_at) VALUES(?,1,'user_message',?,?)",
                       (sid, 'private' * 200000, transcript))
        if update:
            db.execute('INSERT INTO host_updates(session_id,seq,update_json,created_at) VALUES(?,2,?,?)',
                       (sid, '{"private":"' + 'x' * 1000000 + '"}', update))


@pytest.mark.parametrize('force', [False, True])
@pytest.mark.parametrize('journal_mode', ['DELETE', 'WAL'])
def test_orphan_transcripts_use_last_activity_and_reclaim_file(tmp_path, force, journal_mode):
    store = Store(tmp_path / 'private')
    with store.connection() as db:
        assert db.execute(f'PRAGMA journal_mode={journal_mode}').fetchone()[0].upper() == journal_mode
    foreign = tmp_path / 'foreign'; foreign.mkdir()
    old = '2025-01-01T00:00:00.000000+00:00'
    cutoff = datetime(2026, 1, 1, tzinfo=timezone.utc)
    recent = '2026-01-01T00:00:00.000000+00:00'
    for sid in ('old', 'loaded', 'invoking', 'missing-task'):
        _retention_session(store, tmp_path, sid, old, transcript=old, update=old)
    _retention_session(store, tmp_path, 'recent-text', old, transcript=recent)
    _retention_session(store, tmp_path, 'recent-update', old, update=recent)
    _retention_session(store, tmp_path, 'recent-close', old, closed=recent)
    _retention_session(store, foreign, 'foreign', old, transcript=old)
    # Legacy corruption: a host session can reference a task already absent.
    import sqlite3
    with sqlite3.connect(store.path) as db:
        db.execute("UPDATE acp_host_sessions SET task_id='missing' WHERE session_id='missing-task'")
    options = dict(cutoff=cutoff, force=force, keep_session_ids=('invoking',), protected_session_ids=('loaded',))
    before = store.path.read_bytes()
    preview = clear_tasks(store, str(tmp_path), dry_run=True, **options)
    assert preview['would_delete_orphan_sessions'] == (3 if force else 2)
    assert preview['would_delete_tasks'] == 0 and store.path.read_bytes() == before
    result = clear_tasks(store, str(tmp_path), **options)
    assert result['deleted_tasks'] == 0
    assert result['deleted_orphan_sessions'] == (3 if force else 2)
    assert result['vacuum_status'] == 'completed'
    assert result['db_bytes_after'] < result['db_bytes_before']
    assert result['reclaimed_bytes'] == result['db_bytes_before'] - result['db_bytes_after']
    assert store.path.stat().st_size == result['db_bytes_after']
    with store.connection() as db:
        remaining = {row[0] for row in db.execute('SELECT session_id FROM acp_host_sessions')}
        expected = {'invoking', 'recent-text', 'recent-update', 'recent-close', 'foreign'} | (set() if force else {'loaded'})
        assert remaining == expected
        for table in ('host_transcript', 'host_updates'):
            assert not {row[0] for row in db.execute(f'SELECT session_id FROM {table}')} - expected
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []
    assert clear_tasks(store, str(tmp_path), **options)['vacuum_status'] == 'not_needed'


def test_vacuum_failure_reports_committed_deletion_under_lock(tmp_path, monkeypatch):
    from contextlib import contextmanager
    import sqlite3
    store = Store(tmp_path / 'private')
    task = call_store(store, 'create_task', dict(project_root=str(tmp_path), request='k', request_key='k'))['task_id']
    original = store.connection
    calls = []
    class Connection:
        def __init__(self, db):
            self.db = db
        def __getattr__(self, name):
            return getattr(self.db, name)
        def execute(self, sql, *args):
            if sql == 'VACUUM':
                calls.append(sql)
                assert not self.db.in_transaction
                with pytest.raises(StoreError, match='storage_busy'):
                    with original():
                        pytest.fail('vacuum must still hold the exclusive lock')
                raise sqlite3.OperationalError('private disk failure')
            return self.db.execute(sql, *args)
    @contextmanager
    def connection(**kwargs):
        with original(**kwargs) as db:
            yield Connection(db)
    monkeypatch.setattr(store, 'connection', connection)
    result = clear_tasks(store, str(tmp_path), 0)
    assert calls == ['VACUUM']
    assert result['deleted_tasks'] == 1 and result['vacuum_status'] == 'failed'
    assert result['reclaimed_bytes'] == 0
    assert 'private' not in result['vacuum_error'] and 'committed' in result['vacuum_error']
    with original() as db:
        assert db.execute('SELECT COUNT(*) FROM tasks').fetchone()[0] == 0
        assert db.execute('SELECT COUNT(*) FROM pending_deletions').fetchone()[0] == 0


@pytest.mark.parametrize('force', [False, True])
def test_cli_keeps_manual_active_unaware_selection_and_explicit_threads(tmp_path, monkeypatch, capsys, force):
    import json
    from cortex_runtime.orchestration.state import SCHEMA
    store = Store(tmp_path / '.cortex', project_root=str(tmp_path))
    old = '2025-01-01T00:00:00.000000+00:00'
    with patch.object(Store, '_now', return_value=old):
        tasks = [call_store(store, 'create_task', dict(project_root=str(tmp_path), request=key, request_key=key))['task_id']
                 for key in ('stale', 'kept')]
    with store.connection() as db:
        db.executescript(SCHEMA)
        for task in tasks:
            db.execute('INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)',
                       (task, 'EXECUTING', 'private', old, old))
    argv = ['cortex_clear.py', '--project-root', str(tmp_path), '--days', '1', '--keep-thread', thread_for(store, tasks[1])]
    if force:
        argv.append('--force')
    monkeypatch.setattr('sys.argv', argv + ['--dry-run'])
    cortex_clear.main()
    preview = json.loads(capsys.readouterr().out)
    assert preview['deleted_tasks'] == 0 and preview['would_delete_tasks'] == 1
    assert preview['force'] == force
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM tasks').fetchone()[0] == 2
    monkeypatch.setattr('sys.argv', argv)
    cortex_clear.main()
    result = json.loads(capsys.readouterr().out)
    assert result['deleted_tasks'] == 1 and result['skipped_protected'] == 1
    assert result['vacuum_status'] == 'completed'
    with store.connection() as db:
        assert {row[0] for row in db.execute('SELECT id FROM tasks')} == {tasks[1]}
