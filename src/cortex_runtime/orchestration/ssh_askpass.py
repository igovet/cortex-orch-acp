"""ssh inside the sandbox: the owner's own agent, and an opt-in dialog.

ssh in an engine's native shell and in ``command_run`` uses the host's own
ssh setup: HOME (``~/.ssh/config``, ``known_hosts``, keys) is the owner's,
read-only, and ``host_agent_env`` passes the owner's running ssh-agent
(``SSH_AUTH_SOCK``) through, so keys unlocked on the host work unchanged.
Only a socket owned by the owner is passed; the private keys never leave the
agent, and the model can only ask it to sign, as any owner process can.

A sandboxed command has no terminal, so ssh cannot ask for a password and
fails. With ``ssh-askpass.json`` (``{"enabled": true}``) in the Cortex config
directory, ssh in an engine's native shell and in ``command_run`` shows its
own prompt in a desktop dialog instead: the owner types the answer and it
goes straight to ssh, never to the model, a transcript or the store.

Only when enabled, the sandbox additionally receives the owner's display
session (``DISPLAY_ENV``) and ``SSH_ASKPASS``/``SSH_ASKPASS_REQUIRE=force``.
The dialog script is Cortex-generated and mounted read-only, so the sandbox
cannot replace it. Without a display session or a dialog program nothing is
added and ssh fails as before. sudo is out of reach either way: the sandbox
sets no_new_privs, which forbids setuid programs.

Unprivileged bubblewrap maps only the owner's uid into the sandbox, so files
root owns on the host appear owned by ``nobody`` there. ssh refuses an
``Include``d config file (Ubuntu's ``/etc/ssh/ssh_config.d/*.conf``) owned by
anyone but root or the user, so every ssh failed with "Bad owner or
permissions" although the owner's terminal accepts the same file.
``trusted_config_copies`` finds the included files ssh trusts on the host
(root-owned, not group- or world-writable) and ``mount_trusted_configs``
shows the same bytes there, owned by the owner and read-only. A file ssh
rejects on the host is left alone and is rejected the same way.
"""
from pathlib import Path
import glob
import json
import re
import os
import shlex
import shutil
import stat

CONFIG_NAME = 'ssh-askpass.json'
TITLE = 'Cortex: ssh'
_SYSTEM_PATH = '/usr/local/bin:/usr/bin:/bin'
_PATH_VALUE = re.compile(r'^/[^\x00\n]*$')
DISPLAY_ENV = {
    'DISPLAY': re.compile(r'^[A-Za-z0-9._/-]*:[0-9]+(\.[0-9]+)?$'),
    'WAYLAND_DISPLAY': re.compile(r'^[A-Za-z0-9._/-]+$'),
    'XAUTHORITY': _PATH_VALUE,
    'XDG_RUNTIME_DIR': _PATH_VALUE,
    'DBUS_SESSION_BUS_ADDRESS': re.compile(r'^unix:[A-Za-z0-9=,/._%-]+$'),
}

# One shell line per dialog kind: a hidden answer, a visible answer (host key
# confirmation, a git username), a yes/no confirmation and a notice.
_DIALOGS = {
    'zenity': {
        'hidden': '--entry --hide-text --title={title} --text="$prompt"',
        'visible': '--entry --title={title} --text="$prompt"',
        'confirm': '--question --title={title} --text="$prompt"',
        'notice': '--info --title={title} --text="$prompt"',
    },
    'kdialog': {
        'hidden': '--title {title} --password "$prompt"',
        'visible': '--title {title} --inputbox "$prompt"',
        'confirm': '--title {title} --yesno "$prompt"',
        'notice': '--title {title} --msgbox "$prompt"',
    },
}

_SCRIPT = '''#!/bin/sh
# Cortex ssh askpass: ssh's own prompt in a desktop dialog. The answer is
# printed to ssh only.
prompt=${{1:-Password:}}
case ${{SSH_ASKPASS_PROMPT:-}} in
  confirm) exec {program} {confirm} ;;
  none) exec {program} {notice} ;;
esac
case $prompt in
  *"(yes/no"*|*[Uu]sername*) exec {program} {visible} ;;
esac
exec {program} {hidden}
'''


def host_agent_env(environ) -> dict[str, str]:
    """``SSH_AUTH_SOCK`` from the owner's environment when it names a socket
    the owner owns; ``{}`` otherwise (no agent, or anything suspicious)."""
    value = environ.get('SSH_AUTH_SOCK')
    if not isinstance(value, str) or not _PATH_VALUE.match(value):
        return {}
    try:
        info = os.stat(value)
    except OSError:
        return {}
    if not stat.S_ISSOCK(info.st_mode) or info.st_uid != os.getuid():
        return {}
    return {'SSH_AUTH_SOCK': value}


def enabled(environ) -> bool:
    """The owner's switch; anything but ``{"enabled": true}`` is off."""
    from ..connections import registry
    try:
        path = registry.config_root(environ) / CONFIG_NAME
        data = json.loads(path.read_text(encoding='utf-8'))
    except (OSError, ValueError, TypeError):
        return False
    return isinstance(data, dict) and data.get('enabled') is True


def display_env(environ) -> dict[str, str]:
    """The owner's display session, each value shape-checked; empty without
    a display to show a dialog on."""
    found = {name: environ[name] for name, shape in DISPLAY_ENV.items()
             if isinstance(environ.get(name), str) and shape.match(environ[name])}
    return found if 'DISPLAY' in found or 'WAYLAND_DISPLAY' in found else {}


def dialog_program() -> tuple[str, str] | None:
    for name in _DIALOGS:
        program = shutil.which(name, path=_SYSTEM_PATH)
        if program is not None:
            return name, program
    return None


def prepare(environ, directory) -> dict[str, str]:
    """Write the dialog script into ``directory`` and return the variables a
    sandboxed ssh needs to use it; ``{}`` (nothing written) when disabled or
    no display session or dialog program exists."""
    if not enabled(environ):
        return {}
    display = display_env(environ)
    dialog = dialog_program()
    if not display or dialog is None:
        return {}
    name, program = dialog
    title = shlex.quote(TITLE)
    lines = {kind: line.format(title=title) for kind, line in _DIALOGS[name].items()}
    directory = Path(directory)
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    script = directory / 'ssh-askpass'
    script.write_text(_SCRIPT.format(program=shlex.quote(program), **lines), encoding='utf-8')
    script.chmod(0o500)
    return {**display, 'SSH_ASKPASS': str(script), 'SSH_ASKPASS_REQUIRE': 'force'}


def apply(command, environ, directory) -> bool:
    """``prepare`` for a sandbox command, in place: the variables are set and
    ``directory`` is mounted read-only over itself."""
    env = prepare(environ, directory)
    if not env:
        return False
    args = [arg for name, value in env.items() for arg in ('--setenv', name, value)]
    args += ['--ro-bind', str(directory), str(directory)]
    at = command.index('--chdir')
    command[at:at] = args
    return True


SYSTEM_CONFIG = '/etc/ssh/ssh_config'
_INCLUDE = re.compile(r'^\s*include(?:\s*=\s*|\s+)(.*)$', re.IGNORECASE)
_ROOT = 0
_MAX_DEPTH = 16  # ssh's own READCONF_MAX_DEPTH
_MAX_BYTES = 1 << 20


def _includes(path, base, home):
    """The files an ssh config ``Include``s, in ssh's order; relative paths
    are under ``base`` (``~/.ssh`` or ``/etc/ssh``)."""
    try:
        lines = Path(path).read_text(encoding='utf-8', errors='replace').splitlines()
    except OSError:
        return []
    found = []
    for line in lines:
        match = _INCLUDE.match(line)
        if match is None:
            continue
        try:
            words = shlex.split(match.group(1), comments=True)
        except ValueError:
            continue
        for word in words:
            if '%' in word or '$' in word:
                continue  # A token ssh expands per connection: not reproducible here.
            if word.startswith('~/') and home:
                word = str(Path(home, word[2:]))
            elif not word.startswith('/'):
                word = str(Path(base, word))
            found += sorted(glob.glob(word))
    return found


def _trusted_bytes(path):
    """The file's bytes when ssh on the host trusts it as root's; else None."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError:
        return None
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != _ROOT or info.st_mode & 0o022
                or info.st_size > _MAX_BYTES):
            return None
        return stream.read(_MAX_BYTES)


def trusted_config_copies(home) -> dict[str, bytes]:
    """Real path -> bytes of each ssh config file ssh checks for ownership
    (``~/.ssh/config`` and every ``Include``d file) that the host's root owns
    and nobody else can write: exactly the ones the sandbox would show as
    ``nobody``'s although ssh on the host accepts them."""
    home = home if isinstance(home, str) and home.startswith('/') else ''
    user_config = str(Path(home, '.ssh', 'config')) if home else None
    pending = [(user_config, str(Path(home, '.ssh')), True, 0)] if user_config else []
    pending.append((SYSTEM_CONFIG, str(Path(SYSTEM_CONFIG).parent), False, 0))
    copies, seen = {}, set()
    while pending:
        path, base, checked, depth = pending.pop(0)
        try:
            real = str(Path(path).resolve(strict=True))
        except (OSError, RuntimeError):
            continue
        if real in seen:
            continue
        seen.add(real)
        if checked:
            data = _trusted_bytes(real)
            if data is not None:
                copies[real] = data
        if depth < _MAX_DEPTH:
            pending += [(found, base, True, depth + 1) for found in _includes(real, base, home)]
    return copies


def mount_trusted_configs(command, home, directory) -> int:
    """``trusted_config_copies`` over the host-wide mount of a sandbox
    command, in place: each copy is written under ``directory`` (the
    command's scratch), sealed read-only and mounted at its real path.
    Returns the number of files mounted."""
    whole_host = any(command[at] in ('--bind', '--ro-bind') and command[at + 1:at + 3] == ['/', '/']
                     for at in range(len(command) - 2))
    copies = trusted_config_copies(home) if whole_host else {}
    if not copies:
        return 0
    directory = Path(directory)
    directory.mkdir(mode=0o700, exist_ok=True)
    args = []
    for number, (target, data) in enumerate(copies.items()):
        copy = directory / f'{number}.conf'
        copy.unlink(missing_ok=True)  # A relaunched engine reuses its scratch.
        copy.write_bytes(data)
        copy.chmod(0o444)
        args += ['--ro-bind', str(copy), target]
    args += ['--ro-bind', str(directory), str(directory)]
    at = command.index('--chdir')
    command[at:at] = args
    return len(copies)
