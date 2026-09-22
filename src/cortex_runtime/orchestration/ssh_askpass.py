"""Opt-in desktop password dialog for ssh run inside the sandbox.

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
"""
from pathlib import Path
import json
import re
import shlex
import shutil

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
