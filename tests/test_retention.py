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
            # governance + hook_command_sessions
            db.execute("INSERT INTO governance (id,task_id,mode,report_id) VALUES (?,?,?,?)", ('gov_1', task, 'normal', report_id))
            db.execute(
                "INSERT INTO hook_command_sessions (task_id,worker_thread_id,parent_thread_id,command_session_id,cwd,created_at) "
                "VALUES (?,?,?,?,?,?)", (task, 'wt_1', 'pt_1', 'cs_1', str(project), now))
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
        ('hook_command_sessions', 'task_id=?', (task,)),
        ('pending_questions', 'task_id=?', (task,)),
        ('pending_question_late_answers', 'task_id=?', (task,)),
        ('task_language', 'task_id=?', (task,)),
        ('task_stopped', 'task_id=?', (task,)),
        ('trusted_channels', 'task_id=?', (task,)),
        ('source_events', 'task_id=?', (task,)),
        ('task_owners', 'task_id=?', (task,)),
        ('reports', 'task_id=?', (task,)),
        ('thread_bindings', 'task_id=?', (task,)),
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
