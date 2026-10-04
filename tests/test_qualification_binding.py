"""Real guarded storage + executable wrapper + existing client 0.9.1 fake wire.

Not installed-daemon/native/paid evidence. No enforcement reader is mocked.
"""
import fcntl
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys

import pytest

from cortex_runtime.storage.inspection import discover_binding, read_binding_page
from cortex_runtime.contracts import StoreError
from cortex_runtime.ownership import acquire_task_owner
from test_task_inspection import bound, files

ROOT = Path(__file__).resolve().parents[1]
AGENT = '11111111-1111-1111-1111-111111111111'


def test_discovery_exact_current_snapshot_and_epoch_fence(bound):
    project, store, sid, task, epoch = bound
    before = files(project)
    b = discover_binding(str(project), sid)
    assert b['binding'] == dict(project_root=str(project), session_id=sid, task_id=task, owner_epoch=epoch)
    assert len(b['binding_sha256']) == 64 and 'private' not in json.dumps(b)
    assert files(project) == before
    acquire_task_owner(store, task, 'next-owner', epoch)
    assert discover_binding(str(project), sid)['binding']['owner_epoch'] == epoch + 1
    with pytest.raises(StoreError, match='inspection_binding_mismatch'):
        read_binding_page(str(project), sid, task, epoch)


@pytest.mark.parametrize('change', ['session', 'unbound', 'owner', 'session-project', 'task-project', 'invalid-epoch', 'text-epoch', 'task', 'unknown', 'mode', 'symlink', 'legacy', 'wal', 'busy'])
def test_discovery_refusals_non_mutating(bound, tmp_path, change):
    project, store, sid, task, epoch = bound
    lock = None
    if change == 'session': sid = 'acps_bbbbbbbbbbbb'
    elif change == 'unbound':
        from cortex_runtime.acp.sessions import create_session
        sid = 'acps_bbbbbbbbbbbb'
        create_session(store, sid, str(project), {}, {}, [])
    elif change in ('owner', 'session-project', 'task-project', 'invalid-epoch', 'text-epoch', 'task', 'unknown', 'legacy'):
        with store.connection() as db:
            if change == 'owner': db.execute('DELETE FROM task_owners WHERE task_id=?', (task,))
            if change == 'session-project': db.execute('UPDATE acp_host_sessions SET project_root=? WHERE session_id=?', ('/other', sid))
            if change == 'task-project': db.execute('UPDATE tasks SET project_root=? WHERE id=?', ('/other', task))
            if change == 'invalid-epoch': db.execute('UPDATE task_owners SET epoch=0 WHERE task_id=?', (task,))
            if change == 'text-epoch': db.execute("UPDATE task_owners SET epoch='ambiguous' WHERE task_id=?", (task,))
            if change == 'task':
                db.execute('PRAGMA foreign_keys=OFF')
                db.execute('DELETE FROM tasks WHERE id=?', (task,))
            if change == 'unknown': db.execute('PRAGMA user_version=99')
            if change == 'legacy': db.execute('PRAGMA user_version=11')
    elif change == 'mode': (project / '.cortex/cortex.sqlite3').chmod(0o644)
    elif change == 'symlink':
        link = tmp_path / 'alias'; link.symlink_to(project); project = link
    elif change == 'wal':
        wal = project / '.cortex/cortex.sqlite3-wal'; wal.write_bytes(b'uncheckpointed'); wal.chmod(0o600)
    elif change == 'busy':
        lock = (project / '.cortex/.access.lock').open('rb')
        fcntl.flock(lock, fcntl.LOCK_SH)
    before = files(project)
    try:
        with pytest.raises((StoreError, BlockingIOError)):
            discover_binding(str(project), sid)
        assert files(project) == before
    finally:
        if lock: lock.close()


def line(process):
    assert select.select([process.stdout], [], [], 10)[0], 'fixture did not become ready'
    return json.loads(process.stdout.readline())


@pytest.fixture
def wire(bound, tmp_path):
    project, store, sid, task, epoch = bound
    processes = []
    home = tmp_path / 'host-home'; home.mkdir(mode=0o700)
    for name in ('.paseo', '.paseo-dev'): (home / name).mkdir(mode=0o700)
    env = os.environ.copy(); env['HOME'] = str(home)
    # conftest's in-process sys.path and user-site dependencies must survive
    # the fixture HOME change in a fresh ownership-injection interpreter.
    env.pop('PYTHONHOME', None)
    env['PYTHONPATH'] = os.pathsep.join(str(Path(p).resolve()) for p in sys.path if p)
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    def start(scenario):
        process = subprocess.Popen(['node', str(ROOT / 'integrations/paseo/server/qualification-binding-fixture.mjs'),
                                    str(project), sid, AGENT, scenario, sys.executable],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, env=env)
        processes.append(process)
        port = line(process)['port']
        for name, values in [('.paseo-dev', dict(pid=process.pid, listen=f'127.0.0.1:{port}', serverId='fake-dev-server')),
                             ('.paseo', dict(pid=99999999, listen='127.0.0.1:1', serverId='fake-production-server'))]:
            record = dict(uid=os.getuid(), hostname=socket.gethostname(), startedAt='2026-10-01T00:00:00Z', **values)
            path = home / name / 'paseo.pid'; path.write_text(json.dumps(record)); path.chmod(0o664)
        return process
    try:
        yield project, home, env, start
    finally:
        exits = []
        for process in processes:
            process.terminate()
            try:
                process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate(timeout=10)
            exits.append(process.returncode)
        assert exits == [0] * len(processes), 'owned fake-wire child teardown failed'


@pytest.mark.parametrize('scenario', ['ok', 'id', 'provider', 'workspace', 'cwd', 'wrapper', 'duplicate', 'escaped', 'version', 'oversize', 'drift', 'epoch', 'server', 'timeout', 'prefix', 'endpoint', 'pid-mode', 'project-symlink', 'runtime-provider', 'session-absent', 'session-missing', 'auth', 'server-drift', 'endpoint-drift'])
def test_executable_wrapper_single_agent_real_storage_no_paid(wire, tmp_path, scenario):
    project, home, env, start = wire
    process = start(scenario)
    argv = [sys.executable, '-B', str(ROOT / 'scripts/cortex_paseo_dev.py'), 'binding', '--agent',
            AGENT if scenario != 'prefix' else AGENT[:8], '--workspace', 'workspace-owned',
            '--project', str(project), '--timeout', '2' if scenario == 'timeout' else '10']
    if scenario == 'endpoint':
        dev = home / '.paseo-dev/paseo.pid'
        dev.write_bytes((home / '.paseo/paseo.pid').read_bytes())
    if scenario == 'pid-mode': (home / '.paseo-dev/paseo.pid').chmod(0o646)
    if scenario == 'project-symlink':
        alias = tmp_path / 'alias'; alias.symlink_to(project); argv[-3] = str(alias)
    before = files(project)
    pid_before = files(home)
    r = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=15)
    assert r.returncode == (0 if scenario == 'ok' else 1), (r.stdout, r.stderr)
    result = json.loads(r.stdout)
    assert result['available'] is (scenario == 'ok')
    if scenario in ('auth', 'epoch', 'server-drift', 'endpoint-drift', 'drift'):
        assert result['error'] == {'auth':'binding_anonymous_connection_refused',
                                  'epoch':'binding_storage_drift', 'server-drift':'binding_endpoint_drift',
                                  'endpoint-drift':'binding_endpoint_drift', 'drift':'binding_host_drift'}[scenario]
    if scenario == 'ok':
        assert result['binding']['session_id'].startswith('acps_')
        assert result['binding']['owner_epoch'] == 1
        assert len(result['endpoint_sha256']) == 64
    assert 'private' not in r.stdout and 'plugin:' not in r.stdout
    process.stdin.write('counts\n'); process.stdin.flush()
    counts = line(process)
    assert set(counts) <= {'hello', 'fetch_agent_request'}, counts
    assert counts.get('fetch_agent_request', 0) == (2 if scenario in ('ok', 'drift', 'epoch', 'server-drift', 'endpoint-drift') else
                                                  0 if scenario in ('prefix', 'endpoint', 'pid-mode', 'project-symlink', 'server', 'timeout', 'auth') else 1)
    if scenario == 'epoch':
        process.stdin.write('injection\n'); process.stdin.flush()
        assert line(process) == dict(attempted=True, complete=True, before_epoch=1, after_epoch=2)
        assert discover_binding(str(project), 'acps_aaaaaaaaaaaa')['binding']['owner_epoch'] == 2
    else: assert files(project) == before
    if scenario != 'endpoint-drift': assert files(home) == pid_before


@pytest.mark.parametrize('target,change', [('cortex.sqlite3','symlink'), ('.access.lock','symlink'),
                                         ('cortex.sqlite3-shm','mode'), ('cortex.sqlite3','hardlink'),
                                         ('.access.lock','mode')])
def test_discovery_preserves_file_guards(bound, tmp_path, target, change):
    project, store, sid, task, epoch = bound
    path = project/'.cortex'/target
    if change == 'symlink':
        held = tmp_path/'held'; path.rename(held); path.symlink_to(held)
    elif change == 'hardlink': os.link(path, tmp_path/'second-link')
    else:
        if not path.exists(): path.write_bytes(b'fixture')
        path.chmod(0o646)
    before = files(project)
    with pytest.raises(StoreError, match='unsafe_storage'):
        discover_binding(str(project), sid)
    assert files(project) == before


def test_exact_session_ignores_newer_tasks_and_schema_refuses_duplicate_binding(bound):
    import sqlite3
    from cortex_runtime.acp import sessions
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    project, store, sid, task, epoch = bound
    other = bootstrap_task(store, SourceEvent(origin='new-fixture', event_id='new'), 'unrelated fixture', 'other-owner')
    other_sid = 'acps_bbbbbbbbbbbb'
    sessions.create_session(store, other_sid, str(project), {}, {}, [])
    sessions.bind_task(store, other_sid, other.task_id)
    for table, column, value in [('acp_host_sessions','session_id',sid), ('task_owners','task_id',task)]:
        with pytest.raises(sqlite3.IntegrityError):
            with store.connection() as db:
                db.execute(f'INSERT INTO {table} SELECT * FROM {table} WHERE {column}=?', (value,))
    before = files(project)
    assert discover_binding(str(project), sid)['binding']['task_id'] == task
    for bad in [sid[:10], 'acps_cccccccccccc', '', None, 1]:
        with pytest.raises(StoreError): discover_binding(str(project), bad)
    assert files(project) == before


def test_discovery_cli_and_complete_unchanged_pagination(bound):
    project, store, sid, task, epoch = bound
    script = ROOT/'scripts/cortex_inspect_task.py'
    before = files(project)
    # Modification timestamps/inodes also remain unchanged, not just body hashes.
    metadata = {str(p): (p.stat().st_ino, p.stat().st_mtime_ns, p.stat().st_ctime_ns)
                for p in project.rglob('*')}
    argv = [sys.executable, '-B', str(script), '--discover', '--project', str(project), '--session', sid]
    result = subprocess.run(argv, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    binding = json.loads(result.stdout)['binding']
    page = read_binding_page(str(project), sid, binding['task_id'], binding['owner_epoch'], limit=1)
    rows = []
    while True:
        rows.extend(page['rows'])
        if page['next_cursor'] is None: break
        assert not page['complete']
        page = read_binding_page(str(project), sid, task, epoch, cursor=page['next_cursor'], limit=1)
    assert page['complete'] and len(rows) == page['total']
    for extra in [['--task', task], ['--owner-epoch','1'], ['--cursor','opaque'], ['--limit','1']]:
        refused = subprocess.run(argv+extra, capture_output=True, text=True, timeout=10)
        assert refused.returncode == 2
    assert files(project) == before
    assert metadata == {str(p): (p.stat().st_ino, p.stat().st_mtime_ns, p.stat().st_ctime_ns)
                        for p in project.rglob('*')}


@pytest.mark.parametrize('option', ['--home', '--host', '--url', '--paseo'])
def test_binding_cli_disallows_endpoint_home_executable_overrides(wire, option):
    project, home, env, start = wire
    process = start('ok')
    result = subprocess.run([sys.executable, '-B', str(ROOT/'scripts/cortex_paseo_dev.py'), 'binding',
                             '--agent', AGENT, '--workspace','workspace-owned','--project',str(project),
                             option, '/unrelated'], env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 2
    process.stdin.write('counts\n'); process.stdin.flush()
    assert line(process) == {}


@pytest.mark.parametrize('behavior,code', [('overflow','binding_output_overflow'), ('hang','binding_timeout')])
def test_wrapper_bounds_child_stdout_and_deadline_and_stops_owned_helper(bound, tmp_path, monkeypatch, capsys, behavior, code):
    import argparse
    import cortex_paseo_dev as wrapper
    project, store, sid, task, epoch = bound
    # Test pipe supervision itself with a newly owned subprocess, no wire/daemon.
    monkeypatch.setattr(wrapper, '_binding_endpoint', lambda home: ('ws://127.0.0.1:1/ws', 'a'*64))
    for key in ('PASEO_HOME', 'PASEO_HOST', 'PASEO_AGENT_ID', 'PASEO_AGENT_CWD'):
        monkeypatch.setenv(key, 'unrelated-fixture')
    real_popen = subprocess.Popen
    owned = []
    def popen(argv, **kwargs):
        assert not any(key.startswith('PASEO_') for key in kwargs['env'])
        body = ('import os,time; os.write(1,b"x"*9000); time.sleep(10)' if behavior == 'overflow'
                else 'import time; time.sleep(10)')
        child = real_popen([sys.executable,'-B','-c',body], **kwargs)
        owned.append(child)
        return child
    monkeypatch.setattr(wrapper.subprocess, 'Popen', popen)
    assert wrapper._run_binding(argparse.Namespace(agent=AGENT, workspace='owned', project=str(project), timeout=1)) == 1
    assert json.loads(capsys.readouterr().out) == {'available':False, 'error':code,
        'diagnostic': {'stage': 'helper-output', 'reason': 'output_overflow' if behavior == 'overflow' else 'timeout'}}
    assert len(owned) == 1 and owned[0].poll() is not None
    owned[0].stdout.close()


@pytest.mark.parametrize('change', ['duplicate','large','symlink','hardlink','mode'])
def test_endpoint_metadata_is_bounded_unique_private_and_nonmutating(tmp_path, change):
    import cortex_paseo_dev as wrapper
    path = tmp_path/'paseo.pid'
    path.write_text('{"serverId":"one"}'); path.chmod(0o600)
    if change == 'duplicate': path.write_text('{"serverId":"one","serverId":"two"}')
    if change == 'large': path.write_text('x'*4097)
    if change == 'symlink':
        actual = tmp_path/'actual'; path.rename(actual); path.symlink_to(actual)
    if change == 'hardlink': os.link(path, tmp_path/'other')
    if change == 'mode': path.chmod(0o646)
    before = files(tmp_path)
    with pytest.raises(ValueError, match='binding_endpoint_refused'): wrapper._private_json(path)
    assert files(tmp_path) == before


@pytest.mark.parametrize('change', ['mode', 'replacement'])
def test_endpoint_metadata_change_during_read_refuses(tmp_path, monkeypatch, change):
    import cortex_paseo_dev as wrapper
    path = tmp_path/'paseo.pid'; path.write_text('{"serverId":"fixture"}'); path.chmod(0o600)
    read = os.read
    def changed_read(fd, size):
        raw = read(fd, size)
        if change == 'mode': path.chmod(0o646)
        else:
            replacement = tmp_path/'replacement'
            replacement.write_bytes(raw); replacement.chmod(0o600); replacement.replace(path)
        return raw
    monkeypatch.setattr(wrapper.os, 'read', changed_read)
    with pytest.raises(ValueError, match='binding_endpoint_refused'):
        wrapper._private_json(path)


@pytest.mark.parametrize('change', ['ok','production-absent','production-empty','production-uid',
                                   'production-host','same-pid','same-server','same-listen','non-loopback',
                                   'invalid-port','dev-uid','dev-host'])
def test_both_fake_pid_records_required_and_distinct(tmp_path, monkeypatch, change):
    import cortex_paseo_dev as wrapper
    home = tmp_path/'fake-home'; home.mkdir(mode=0o700)
    monkeypatch.setenv('HOME', str(home))
    dev = dict(pid=11, listen='127.0.0.1:1234', serverId='dev-fixture',
               startedAt='fixture-start', uid=os.getuid(), hostname=socket.gethostname())
    prod = dict(dev, pid=22, listen='127.0.0.1:4321', serverId='prod-fixture')
    if change == 'production-empty': prod = {}
    if change == 'production-uid': prod['uid'] += 1
    if change == 'production-host': prod['hostname'] = 'unrelated'
    if change == 'same-pid': dev['pid'] = prod['pid']
    if change == 'same-server': dev['serverId'] = prod['serverId']
    if change == 'same-listen': dev['listen'] = prod['listen']
    if change == 'non-loopback': dev['listen'] = '192.0.2.1:1234'
    if change == 'invalid-port': dev['listen'] = '127.0.0.1:65536'
    if change == 'dev-uid': dev['uid'] += 1
    if change == 'dev-host': dev['hostname'] = 'unrelated'
    for name, values in [('.paseo-dev',dev), ('.paseo',prod)]:
        folder = home/name; folder.mkdir(mode=0o700)
        if change == 'production-absent' and name == '.paseo': continue
        path = folder/'paseo.pid'; path.write_text(json.dumps(values)); path.chmod(0o664)
    before = files(home)
    if change == 'ok':
        url, digest = wrapper._binding_endpoint(home/'.paseo-dev')
        assert url == 'ws://127.0.0.1:1234/ws' and len(digest) == 64
    else:
        with pytest.raises((ValueError, FileNotFoundError)):
            wrapper._binding_endpoint(home/'.paseo-dev')
    assert files(home) == before


def test_epoch_injection_failure_is_reported_without_crashing_or_mutating(wire):
    project, home, env, start = wire
    process = start('epoch-failure')
    before = files(project)
    result = subprocess.run([sys.executable, '-B', str(ROOT/'scripts/cortex_paseo_dev.py'),
                             'binding', '--agent', AGENT, '--workspace', 'workspace-owned',
                             '--project', str(project), '--timeout', '10'],
                            env=env, capture_output=True, text=True, timeout=15)
    assert result.returncode == 1
    assert json.loads(result.stdout) == {'available': False,
                                       'error': 'binding_anonymous_host_or_storage_refused'}
    process.stdin.write('injection\n'); process.stdin.flush()
    assert line(process) == dict(attempted=True, complete=False,
                                 error='fixture_epoch_injection_failed', exit_code=7)
    process.stdin.write('counts\n'); process.stdin.flush()
    assert line(process) == {'hello': 1, 'fetch_agent_request': 2}
    assert process.poll() is None
    assert files(project) == before


@pytest.mark.parametrize('condition,reason', [
    ('busy', 'inspection_busy'), ('wal', 'inspection_uncheckpointed_wal'),
    ('legacy', 'legacy_storage_migration_required'), ('mode', 'unsafe_storage'),
    ('owner', 'inspection_binding_mismatch'), ('unsupported', 'unsupported_storage'),
])
def test_wrapper_preserves_guarded_inspector_refusal_without_binding(wire, condition, reason):
    project, home, env, start = wire
    process = start('ok')
    lock = None
    if condition == 'busy':
        lock = (project / '.cortex/.access.lock').open('rb')
        fcntl.flock(lock, fcntl.LOCK_SH)
    elif condition == 'wal':
        wal = project / '.cortex/cortex.sqlite3-wal'
        wal.write_bytes(b'owned-fixture-uncheckpointed')
        wal.chmod(0o600)
    elif condition == 'mode':
        (project / '.cortex/cortex.sqlite3').chmod(0o644)
    else:
        from cortex_runtime.store import Store
        store = Store(project / '.cortex', initialize=False, project_root=project)
        with store.connection() as db:
            if condition == 'owner':
                db.execute('DELETE FROM task_owners')
            else:
                db.execute('PRAGMA user_version=' + ('11' if condition == 'legacy' else '99'))
    before = files(project)
    try:
        inspector = subprocess.run([sys.executable, '-B', str(ROOT/'scripts/cortex_inspect_task.py'),
                                    '--discover', '--project', str(project), '--session', 'acps_aaaaaaaaaaaa'],
                                   env=env, capture_output=True, text=True, timeout=10)
        assert inspector.returncode == 1
        assert json.loads(inspector.stdout) == {'available': False, 'error': reason}
        assert files(project) == before
        result = subprocess.run([sys.executable, '-B', str(ROOT/'scripts/cortex_paseo_dev.py'),
                                 'binding', '--agent', AGENT, '--workspace', 'workspace-owned',
                                 '--project', str(project), '--timeout', '10'],
                                env=env, capture_output=True, text=True, timeout=15)
        assert result.returncode == 1
        assert json.loads(result.stdout) == {
            'available': False, 'error': 'binding_storage_refused',
            'diagnostic': {'stage': 'task-discovery', 'reason': reason},
        }
        assert files(project) == before
        process.stdin.write('counts\n'); process.stdin.flush()
        assert line(process) == {'hello': 1, 'fetch_agent_request': 1}
    finally:
        if lock:
            lock.close()


@pytest.mark.parametrize('change', ['valid', 'reason', 'stage', 'extra', 'duplicate', 'escaped', 'truncated', 'signal'])
def test_wrapper_only_forwards_complete_allowlisted_refusal_diagnostic(bound, monkeypatch, capsys, change):
    import argparse
    import cortex_paseo_dev as wrapper
    project, store, sid, task, epoch = bound
    monkeypatch.setattr(wrapper, '_binding_endpoint', lambda home: ('ws://127.0.0.1:1/ws', 'a'*64))
    body = {'available': False, 'error': 'binding_storage_refused',
            'diagnostic': {'stage': 'task-discovery', 'reason': 'inspection_busy'}}
    if change == 'reason': body['diagnostic']['reason'] = 'private sentinel'
    if change == 'stage': body['diagnostic']['stage'] = 'private sentinel'
    if change == 'extra': body['diagnostic']['private'] = 'sentinel'
    payload = json.dumps(body, separators=(',', ':')) + '\n'
    if change == 'duplicate': payload = payload.replace('"available":false', '"available":false,"available":false')
    if change == 'escaped': payload = payload.replace('"reason"', '"\\u0072eason"')
    if change == 'truncated': payload = payload[:-10]
    real_popen = subprocess.Popen
    children = []
    def popen(argv, **kwargs):
        code = 'import os,signal; os.write(1,' + repr(payload.encode()) + '); '
        code += ('os.kill(os.getpid(),signal.SIGTERM)' if change == 'signal' else 'raise SystemExit(1)')
        child = real_popen([sys.executable, '-B', '-c', code], **kwargs)
        children.append(child)
        return child
    monkeypatch.setattr(wrapper.subprocess, 'Popen', popen)
    before = files(project)
    assert wrapper._run_binding(argparse.Namespace(agent=AGENT, workspace='owned',
                                                   project=str(project), timeout=2)) == 1
    result = json.loads(capsys.readouterr().out)
    assert result['available'] is False and 'binding' not in result
    if change == 'valid':
        assert result == body
    else:
        assert result['diagnostic'] == {'stage': 'helper-output',
            'reason': 'subprocess_terminated' if change == 'signal' else 'subprocess_failed'}
        assert 'private' not in json.dumps(result)
    assert files(project) == before
    assert len(children) == 1 and children[0].poll() is not None
    children[0].stdout.close()


def test_missing_owned_endpoint_metadata_is_diagnosed_without_spawn(wire):
    project, home, env, start = wire
    before = files(home), files(project)
    result = subprocess.run([sys.executable, '-B', str(ROOT/'scripts/cortex_paseo_dev.py'),
                             'binding', '--agent', AGENT, '--workspace', 'workspace-owned',
                             '--project', str(project), '--timeout', '2'],
                            env=env, capture_output=True, text=True, timeout=5)
    assert result.returncode == 1
    assert json.loads(result.stdout) == {'available': False, 'error': 'binding_refused',
                                        'diagnostic': {'stage': 'endpoint-provenance',
                                                       'reason': 'metadata_unavailable'}}
    assert (files(home), files(project)) == before


@pytest.mark.parametrize('behavior,stage,reason', [
    ('spawn', 'helper-spawn', 'spawn_failed'),
    ('runtime', 'helper-output', 'subprocess_failed'),
    ('signal', 'helper-output', 'subprocess_terminated'),
    ('invalid', 'helper-output', 'invalid_output'),
    ('incomplete', 'helper-output', 'invalid_output'),
    ('empty', 'helper-output', 'invalid_output'),
])
def test_wrapper_local_failure_diagnostics_never_expose_child_text(bound, monkeypatch, capsys,
                                                                behavior, stage, reason):
    import argparse
    import cortex_paseo_dev as wrapper
    project, store, sid, task, epoch = bound
    monkeypatch.setattr(wrapper, '_binding_endpoint', lambda home: ('ws://127.0.0.1:1/ws', 'a'*64))
    real_popen = subprocess.Popen
    children = []
    def popen(argv, **kwargs):
        if behavior == 'spawn':
            raise FileNotFoundError('private secret sentinel')
        body = {'runtime': 'import sys; sys.stderr.write("private secret sentinel"); sys.exit(7)',
                'signal': 'import os,signal; os.kill(os.getpid(),signal.SIGTERM)',
                'invalid': 'print("private secret sentinel")',
                'incomplete': 'print("{}")', 'empty': 'pass'}[behavior]
        child = real_popen([sys.executable, '-B', '-c', body], **kwargs)
        children.append(child)
        return child
    monkeypatch.setattr(wrapper.subprocess, 'Popen', popen)
    before = files(project)
    assert wrapper._run_binding(argparse.Namespace(agent=AGENT, workspace='owned',
                                                   project=str(project), timeout=2)) == 1
    result = json.loads(capsys.readouterr().out)
    assert result['available'] is False and 'binding' not in result
    assert result['diagnostic'] == {'stage': stage, 'reason': reason}
    assert 'private' not in json.dumps(result) and 'secret' not in json.dumps(result)
    assert files(project) == before
    for child in children:
        assert child.poll() is not None
        child.stdout.close()


def test_stopped_owned_fake_endpoint_refuses_without_binding(wire):
    project, home, env, start = wire
    process = start('ok')
    process.terminate(); process.communicate(timeout=10)
    assert process.returncode == 0
    before = files(project), files(home)
    result = subprocess.run([sys.executable, '-B', str(ROOT/'scripts/cortex_paseo_dev.py'),
                             'binding', '--agent', AGENT, '--workspace', 'workspace-owned',
                             '--project', str(project), '--timeout', '2'],
                            env=env, capture_output=True, text=True, timeout=5)
    assert result.returncode == 1
    assert json.loads(result.stdout)['error'] in ('binding_anonymous_connection_refused', 'binding_timeout')
    assert json.loads(result.stdout)['available'] is False and 'binding' not in json.loads(result.stdout)
    assert (files(project), files(home)) == before


@pytest.mark.parametrize('stage', ['endpoint-provenance', 'task-discovery'])
@pytest.mark.parametrize('behavior,reason', [('runtime', 'subprocess_failed'),
    ('signal', 'subprocess_terminated'), ('invalid', 'invalid_output'),
    ('malicious', 'subprocess_failed'), ('overflow', 'output_overflow')])
def test_real_helper_python_boundary_retains_only_local_safe_process_reason(wire, tmp_path,
        monkeypatch, capsys, stage, behavior, reason):
    import argparse
    import cortex_paseo_dev as wrapper
    project, home, env, start = wire
    process = start('ok')
    monkeypatch.setenv('HOME', str(home))
    monkeypatch.setenv('PYTHONPATH', env['PYTHONPATH'])
    shim = tmp_path/'owned-python'
    payload = json.dumps({'available': False, 'error': 'private secret sentinel'}) + '\n'
    body = {'runtime': 'sys.stderr.write("private secret sentinel"); sys.exit(7)',
            'signal': 'os.kill(os.getpid(),signal.SIGTERM)',
            'invalid': 'print("private secret sentinel")',
            'malicious': 'sys.stdout.write(' + repr(payload) + '); sys.exit(1)',
            'overflow': 'os.write(1,b"x"*20000)'}[behavior]
    # Change only the owned helper's Python child; production helper, endpoint
    # gate and wrapper remain executable, with no success-producing substitute.
    shim.write_text('#!' + sys.executable + '\nimport os,sys,signal\n'
                    + 'endpoint = "-c" in sys.argv\n'
                    + 'if endpoint == ' + repr(stage == 'endpoint-provenance') + ':\n    ' + body + '\n'
                    + 'else:\n    os.execv(' + repr(sys.executable) + ', [' + repr(sys.executable) + '] + sys.argv[1:])\n')
    shim.chmod(0o700)
    real_popen = subprocess.Popen
    children = []
    def popen(argv, **kwargs):
        assert argv[0] == 'node' and argv[-2] == sys.executable
        argv = [*argv]; argv[-2] = str(shim)
        child = real_popen(argv, **kwargs); children.append(child); return child
    monkeypatch.setattr(wrapper.subprocess, 'Popen', popen)
    before = files(project), files(home)
    assert wrapper._run_binding(argparse.Namespace(agent=AGENT, workspace='workspace-owned',
                                                   project=str(project), timeout=5)) == 1
    result = json.loads(capsys.readouterr().out)
    assert result == {'available': False, 'error': 'binding_storage_refused',
                      'diagnostic': {'stage': stage, 'reason': reason}}
    assert (files(project), files(home)) == before
    for child in children:
        assert child.poll() is not None
        child.stdout.close()
    process.stdin.write('counts\n'); process.stdin.flush()
    assert line(process) == ({} if stage == 'endpoint-provenance' else {'hello': 1, 'fetch_agent_request': 1})


@pytest.mark.parametrize('change', ['ok', 'endpoint-missing', 'identity', 'digest', 'epoch', 'extra', 'duplicate',
                                    'escaped', 'truncated', 'error'])
def test_zero_exit_requires_complete_canonical_exact_binding(bound, monkeypatch, capsys, change):
    import argparse
    import hashlib
    import cortex_paseo_dev as wrapper
    project, store, sid, task, epoch = bound
    calls = []
    def endpoint(home):
        calls.append(home)
        if change == 'endpoint-missing' and len(calls) == 2:
            raise FileNotFoundError('private secret sentinel')
        return 'ws://127.0.0.1:1/ws', 'a'*64
    monkeypatch.setattr(wrapper, '_binding_endpoint', endpoint)
    binding = dict(owner_epoch=epoch, project_root=str(project), session_id=sid, task_id=task)
    result = {'available': True,
              'host': dict(agent_id=AGENT, workspace_id='owned', project_root=str(project),
                           session_id=sid, wrapper_sha256='a'*64),
              'binding': binding,
              'binding_sha256': hashlib.sha256(json.dumps(binding, separators=(',', ':')).encode()).hexdigest(),
              'provenance': 'single-agent-api/refetch + guarded-immutable-session-task-current-owner'}
    if change == 'identity': result['host']['agent_id'] = '22222222-2222-2222-2222-222222222222'
    if change == 'digest': result['binding_sha256'] = 'b'*64
    if change == 'epoch': binding['owner_epoch'] = True
    if change == 'extra': result['private'] = 'secret sentinel'
    if change == 'error': result = {'available': False, 'error': 'binding_storage_refused'}
    payload = json.dumps(result, separators=(',', ':')) + '\n'
    if change == 'duplicate': payload = payload.replace('"available":true', '"available":true,"available":true')
    if change == 'escaped': payload = payload.replace('"available"', '"\\u0061vailable"')
    if change == 'truncated': payload = payload[:-1]
    real_popen = subprocess.Popen
    children = []
    def popen(argv, **kwargs):
        child = real_popen([sys.executable, '-B', '-c', 'import os; os.write(1,' + repr(payload.encode()) + ')'], **kwargs)
        children.append(child); return child
    monkeypatch.setattr(wrapper.subprocess, 'Popen', popen)
    before = files(project)
    assert wrapper._run_binding(argparse.Namespace(agent=AGENT, workspace='owned',
        project=str(project), timeout=2)) == (0 if change == 'ok' else 1)
    observed = json.loads(capsys.readouterr().out)
    if change == 'ok': assert observed == dict(result, endpoint_sha256='a'*64)
    elif change == 'endpoint-missing':
        assert observed == {'available': False, 'error': 'binding_refused',
                            'diagnostic': {'stage': 'endpoint-recheck', 'reason': 'metadata_unavailable'}}
    else:
        assert observed == {'available': False, 'error': 'binding_refused',
                            'diagnostic': {'stage': 'helper-output', 'reason': 'invalid_output'}}
    assert files(project) == before
    for child in children:
        assert child.poll() is not None
        child.stdout.close()


def test_unknown_exception_code_is_not_forwarded(bound, monkeypatch, capsys):
    import argparse
    import cortex_paseo_dev as wrapper
    project, store, sid, task, epoch = bound
    def refused(home): raise ValueError('binding_private_secret_sentinel')
    monkeypatch.setattr(wrapper, '_binding_endpoint', refused)
    assert wrapper._run_binding(argparse.Namespace(agent=AGENT, workspace='owned',
                                                   project=str(project), timeout=2)) == 1
    assert json.loads(capsys.readouterr().out) == {'available': False, 'error': 'binding_refused'}


def test_unavailable_fresh_input_path_refuses_before_endpoint_read(tmp_path, monkeypatch, capsys):
    import argparse
    import cortex_paseo_dev as wrapper
    monkeypatch.setattr(wrapper, '_binding_endpoint', lambda home: pytest.fail('endpoint before input validation'))
    assert wrapper._run_binding(argparse.Namespace(agent=AGENT, workspace='owned',
        project=str(tmp_path/'absent-project'), timeout=2)) == 1
    assert json.loads(capsys.readouterr().out) == {'available': False, 'error': 'binding_refused',
        'diagnostic': {'stage': 'input-validation', 'reason': 'path_unavailable'}}
