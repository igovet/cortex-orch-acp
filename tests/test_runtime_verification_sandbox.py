import json
import subprocess
import sys
from pathlib import Path

import pytest
from cortex_runtime.orchestration.sandbox import verification_command, with_umask
from cortex_runtime.contracts import StoreError


@pytest.mark.requires_bwrap
def test_real_check_cannot_read_private_home_or_runtime_store(tmp_path):
    project = tmp_path / 'project'
    project.mkdir()
    (project / '.cortex').mkdir()
    (project / '.cortex' / 'private').write_text('private')
    outside = tmp_path / 'outside'
    outside.write_text('private')
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    script = ('from pathlib import Path; import os; '
              f'assert Path({str(outside)!r}).read_text() == "private"; '  # the shared /tmp, like the host
              f'assert not Path({str(project / ".cortex" / "private")!r}).exists(); '
              'assert "CORTEX_TEST_SECRET" not in os.environ; print("isolated")')
    command = verification_command(project, scratch, ['python3', '-B', '-c', script])
    result = subprocess.run(command, capture_output=True, text=True, env={'CORTEX_TEST_SECRET':'never-forward'})
    assert result.returncode == 0, result.stderr
    assert result.stdout == 'isolated\n'
    assert outside.read_text() == 'private'
    assert (project / '.cortex' / 'private').read_text() == 'private'


@pytest.mark.requires_bwrap
def test_var_tmp_is_a_private_writable_per_run_directory(tmp_path):
    """Some tools hardcode /var/tmp; on a host where the real /var/tmp is a
    read-only mount (or, under ``system_root``, simply inherits the whole
    host's read-only bind), that used to fail outright. Each run gets its
    own writable /var/tmp, backed by its own scratch, never the host's."""
    project = tmp_path / 'project'
    project.mkdir()
    scratch_a = tmp_path / 'scratch-a'
    scratch_a.mkdir()
    scratch_b = tmp_path / 'scratch-b'
    scratch_b.mkdir()
    script = 'echo "$TMPDIR" "$TMP" "$TEMP"; cat /var/tmp/marker 2>/dev/null; echo mine > /var/tmp/marker'
    command_a = verification_command(project, scratch_a, ['sh', '-c', script], system_root=True)
    result_a = subprocess.run(command_a, capture_output=True, text=True)
    assert result_a.returncode == 0, result_a.stderr
    tmpdir, tmp, temp = result_a.stdout.splitlines()[0].split()
    assert tmpdir == tmp == temp == str(scratch_a)
    # No stale marker from a run that never happened, and the host's own
    # /var/tmp (if writable at all) never sees this file either.
    assert 'mine' not in result_a.stdout
    command_b = verification_command(project, scratch_b, ['sh', '-c', script], system_root=True)
    result_b = subprocess.run(command_b, capture_output=True, text=True)
    assert result_b.returncode == 0, result_b.stderr
    assert 'mine' not in result_b.stdout, 'a later run must never see an earlier run\'s /var/tmp'
    assert not (Path('/var/tmp') / 'marker').exists()


@pytest.mark.requires_bwrap
def test_with_umask_overrides_the_ambient_process_umask(tmp_path):
    """Sandboxed commands (a worker's shell, a verification check) get an
    explicit, deliberate 022 regardless of whatever the Cortex runtime
    process itself set for its own private store (0077)."""
    project = tmp_path / 'project'
    project.mkdir()
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    command = verification_command(project, scratch, with_umask(['sh', '-c', 'umask']), system_root=True)
    result = subprocess.run(command, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == '0022'


def test_missing_sandbox_does_not_fall_back_to_unsandboxed_command(monkeypatch,tmp_path):
    monkeypatch.setattr('cortex_runtime.orchestration.sandbox.shutil.which', lambda _: None)
    with pytest.raises(StoreError,match='verification_sandbox_unavailable'):
        verification_command(tmp_path,tmp_path,['python3','-c','print(1)'])


@pytest.mark.requires_bwrap
def test_plan_verification_cannot_be_used_as_a_write_bypass(tmp_path):
    root=tmp_path/'project';root.mkdir();(root/'.cortex').mkdir()
    target=root/'protected';target.write_text('original')
    scratch=tmp_path/'scratch';scratch.mkdir()
    command=verification_command(root,scratch,['python3','-B','-c',"from pathlib import Path; Path('protected').write_text('forbidden')"])
    result=subprocess.run(command,capture_output=True,text=True)
    assert result.returncode!=0
    assert target.read_text()=='original'


@pytest.mark.requires_bwrap
def test_a_worker_shell_runs_as_the_owner_but_stays_scope_confined(tmp_path):
    """The shell a worker uses is the owner's: their name, real HOME and
    environment, network; never runtime credentials, never writes to HOME."""
    import os, pwd
    from cortex_runtime.orchestration.sandbox import as_owner, gateway_command
    home = tmp_path / 'home'
    home.mkdir()
    project = home / 'project'
    (project / 'src').mkdir(parents=True)
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    script = ('id -un; echo "$HOME"; echo "$OWNER_SETTING"; env | grep -c -E "CORTEX_|TOKEN" || true; '
              'touch "$HOME/escape" 2>/dev/null && echo home-writable; touch src/ok && echo scope-writable')
    environ = {'HOME': str(home), 'PATH': '/usr/bin:/bin', 'OWNER_SETTING': 'kept',
               'CORTEX_CHANNEL_SECRET': 'never', 'GITHUB_TOKEN': 'never'}
    with gateway_command(project, scratch, ['sh', '-c', script], ['src/**']) as (command, fds):
        as_owner(command, environ)
        assert '--unshare-net' not in command
        result = subprocess.run(command, capture_output=True, text=True, pass_fds=fds, env={'PATH': '/usr/bin:/bin'})
    assert result.returncode == 0, result.stderr
    assert result.stdout.split('\n')[:5] == [pwd.getpwuid(os.getuid()).pw_name, str(home), 'kept', '0', 'scope-writable']
    assert not (home / 'escape').exists() and (project / 'src' / 'ok').exists()
