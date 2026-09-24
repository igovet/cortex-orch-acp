"""ssh inside the sandbox: the owner's ssh-agent and the opt-in askpass dialog."""
import json
import shutil
import subprocess

import pytest

from cortex_runtime.orchestration import ssh_askpass
from cortex_runtime.orchestration.sandbox import as_owner, engine_jail, gateway_command

DISPLAY = {'DISPLAY': ':1', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority', 'XDG_RUNTIME_DIR': '/run/user/1000',
           'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/run/user/1000/bus'}


@pytest.fixture
def environ(tmp_path, monkeypatch):
    """An owner environment whose config enables askpass, with a fake dialog
    that prints its arguments to stderr and answers 'typed-answer'."""
    home = tmp_path / 'home'
    (home / '.config' / 'cortex').mkdir(parents=True)
    dialog = tmp_path / 'bin' / 'zenity'
    dialog.parent.mkdir()
    dialog.write_text('#!/bin/sh\necho "$*" >&2\necho typed-answer\n')
    dialog.chmod(0o755)
    monkeypatch.setattr(ssh_askpass, 'dialog_program', lambda: ('zenity', str(dialog)))
    return {'HOME': str(home), 'PATH': '/usr/bin:/bin', **DISPLAY}


def _enable(environ, value=True):
    config = f"{environ['HOME']}/.config/cortex/{ssh_askpass.CONFIG_NAME}"
    with open(config, 'w') as stream:
        json.dump({'enabled': value}, stream)


def test_disabled_by_default_adds_nothing(environ, tmp_path):
    assert ssh_askpass.prepare(environ, tmp_path / 'askpass') == {}
    assert not (tmp_path / 'askpass').exists()
    _enable(environ, 'yes')
    assert ssh_askpass.prepare(environ, tmp_path / 'askpass') == {}


def test_enabled_passes_the_display_session_and_the_script(environ, tmp_path):
    _enable(environ)
    env = ssh_askpass.prepare({**environ, 'DISPLAY': 'bad value', 'WAYLAND_DISPLAY': 'wayland-0'},
                              tmp_path / 'askpass')
    script = tmp_path / 'askpass' / 'ssh-askpass'
    assert env == {'WAYLAND_DISPLAY': 'wayland-0', 'XAUTHORITY': DISPLAY['XAUTHORITY'],
                   'XDG_RUNTIME_DIR': DISPLAY['XDG_RUNTIME_DIR'],
                   'DBUS_SESSION_BUS_ADDRESS': DISPLAY['DBUS_SESSION_BUS_ADDRESS'],
                   'SSH_ASKPASS': str(script), 'SSH_ASKPASS_REQUIRE': 'force'}
    assert script.stat().st_mode & 0o777 == 0o500


def test_no_display_session_adds_nothing(environ, tmp_path):
    _enable(environ)
    assert ssh_askpass.prepare({'HOME': environ['HOME'], 'XDG_RUNTIME_DIR': '/run/user/1000'},
                               tmp_path / 'askpass') == {}


@pytest.mark.parametrize('prompt, prompt_kind, dialog', [
    ("user@host's password: ", None, '--entry --hide-text'),
    ('Are you sure you want to continue connecting (yes/no/[fingerprint])? ', None, '--entry --title'),
    ("Username for 'https://example.com': ", None, '--entry --title'),
    ('Allow use of key?', 'confirm', '--question'),
])
def test_script_shows_the_prompt_in_the_matching_dialog(environ, tmp_path, prompt, prompt_kind, dialog):
    _enable(environ)
    script = ssh_askpass.prepare(environ, tmp_path / 'askpass')['SSH_ASKPASS']
    env = {'PATH': '/usr/bin:/bin', **({'SSH_ASKPASS_PROMPT': prompt_kind} if prompt_kind else {})}
    result = subprocess.run([script, prompt], env=env, capture_output=True, text=True, timeout=10)
    assert result.stdout == 'typed-answer\n'
    assert result.stderr.startswith(dialog) and result.stderr.rstrip().endswith(prompt.rstrip())


@pytest.mark.engine_jail
@pytest.mark.skipif(shutil.which('bwrap') is None, reason='bubblewrap is required')
def test_engine_jail_uses_the_sealed_script(environ, tmp_path):
    _enable(environ)
    project, scratch = tmp_path / 'project', tmp_path / 'scratch'
    project.mkdir(), scratch.mkdir()
    directory = tmp_path / 'askpass'
    env = {'HOME': environ['HOME'], 'PATH': '/usr/bin:/bin', **ssh_askpass.prepare(environ, directory)}
    script = '"$SSH_ASKPASS" "pw: "; echo "$SSH_ASKPASS_REQUIRE $DISPLAY"; echo x > "$SSH_ASKPASS" || echo sealed'
    with engine_jail(project, scratch, ['sh', '-c', script], [], env=env, sealed=[directory]) as (command, fds):
        result = subprocess.run(command, pass_fds=fds, env={'PATH': '/usr/bin:/bin'},
                                capture_output=True, text=True, timeout=30)
    assert result.stdout.split('\n')[:3] == ['typed-answer', 'force :1', 'sealed'], result.stderr


@pytest.mark.engine_jail
@pytest.mark.skipif(shutil.which('bwrap') is None, reason='bubblewrap is required')
def test_command_sandbox_uses_the_sealed_script(environ, tmp_path):
    _enable(environ)
    project, scratch = tmp_path / 'project', tmp_path / 'scratch'
    project.mkdir(), scratch.mkdir()
    directory = tmp_path / 'askpass'
    script = '"$SSH_ASKPASS" "pw: "; echo "$XAUTHORITY"; echo x > "$SSH_ASKPASS" || echo sealed'
    with gateway_command(project, scratch, ['sh', '-c', script], [], system_root=True) as (command, fds):
        as_owner(command, environ)
        assert ssh_askpass.apply(command, environ, directory)
        result = subprocess.run(command, pass_fds=fds, env={'PATH': '/usr/bin:/bin'},
                                capture_output=True, text=True, timeout=30)
    assert result.stdout.split('\n')[:3] == ['typed-answer', DISPLAY['XAUTHORITY'], 'sealed'], result.stderr


def test_apply_changes_nothing_when_disabled(environ, tmp_path):
    command = ['bwrap', '--chdir', '/', '--', 'true']
    assert not ssh_askpass.apply(command, environ, tmp_path / 'askpass')
    assert command == ['bwrap', '--chdir', '/', '--', 'true']


@pytest.fixture
def agent(tmp_path):
    """A listening unix socket standing in for the owner's ssh-agent; each
    connection is answered with 'agent-ok'."""
    import socket
    import threading
    path = tmp_path / 'agent.sock'
    server = socket.socket(socket.AF_UNIX)
    server.bind(str(path))
    server.listen()

    def serve():
        while True:
            try:
                connection, _ = server.accept()
            except OSError:
                return
            with connection:
                connection.sendall(b'agent-ok')
    threading.Thread(target=serve, daemon=True).start()
    yield str(path)
    server.close()


def test_host_agent_passes_only_an_owned_socket(agent, tmp_path):
    assert ssh_askpass.host_agent_env({'SSH_AUTH_SOCK': agent}) == {'SSH_AUTH_SOCK': agent}
    plain = tmp_path / 'plain'
    plain.write_text('')
    for value in (None, '', 'agent.sock', str(plain), str(tmp_path / 'missing'), agent + '\n'):
        assert ssh_askpass.host_agent_env({'SSH_AUTH_SOCK': value}) == {}, value


_REACH_AGENT = ('import os, socket; s = socket.socket(socket.AF_UNIX); '
                's.connect(os.environ["SSH_AUTH_SOCK"]); print(s.recv(16).decode())')


@pytest.mark.engine_jail
@pytest.mark.skipif(shutil.which('bwrap') is None, reason='bubblewrap is required')
def test_command_sandbox_reaches_the_owner_agent(environ, agent, tmp_path):
    project, scratch = tmp_path / 'project', tmp_path / 'scratch'
    project.mkdir(), scratch.mkdir()
    with gateway_command(project, scratch, ['python3', '-c', _REACH_AGENT], [], system_root=True) as (command, fds):
        as_owner(command, {**environ, 'SSH_AUTH_SOCK': agent})
        result = subprocess.run(command, pass_fds=fds, env={'PATH': '/usr/bin:/bin'},
                                capture_output=True, text=True, timeout=30)
    assert result.stdout.strip() == 'agent-ok', result.stderr


@pytest.mark.engine_jail
@pytest.mark.skipif(shutil.which('bwrap') is None, reason='bubblewrap is required')
def test_engine_jail_reaches_the_owner_agent(environ, agent, tmp_path):
    project, scratch = tmp_path / 'project', tmp_path / 'scratch'
    project.mkdir(), scratch.mkdir()
    env = {'HOME': environ['HOME'], 'PATH': '/usr/bin:/bin', **ssh_askpass.host_agent_env({'SSH_AUTH_SOCK': agent})}
    with engine_jail(project, scratch, ['python3', '-c', _REACH_AGENT], [], env=env) as (command, fds):
        result = subprocess.run(command, pass_fds=fds, env={'PATH': '/usr/bin:/bin'},
                                capture_output=True, text=True, timeout=30)
    assert result.stdout.strip() == 'agent-ok', result.stderr
