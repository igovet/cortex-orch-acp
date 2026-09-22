"""Linux check sandbox: explicit system mounts, project only, no user home/network.

This contains runtime-launched verification commands. A worker's own shell
additionally runs as the owner (`as_owner`): their name, groups, read-only
HOME, secret-free environment and network, while writes stay confined to the
granted scope. `engine_jail` puts a whole engine process in the same box, so
its native tools need no per-call interception.
"""
from pathlib import Path, PurePath
import re
import shutil
import os
import stat
from contextlib import contextmanager
from ..contracts import StoreError


# Entries walked to find grants; past it grants stop (narrower), while
# private names are still masked everywhere.
_TREE_LIMIT = 2_000_000
# Hardlinks in one granted directory past which it stays read-only.
_HARDLINK_DIR_LIMIT = 256


def _dev_index(command) -> int:
    return next(i for i, arg in enumerate(command) if arg in ('--dev', '--dev-bind'))


def _private_target(path: Path, root: Path) -> Path | None:
    """What to mask for a private name: the path itself, or -- for a
    symlink (``.claude -> shared/claude`` is common) -- the project path it
    points to. ``None``: nothing to mask (dangling, a loop, or outside the
    project, which the sandbox only ever shows read-only). A private symlink
    used to refuse the whole worker."""
    if not path.is_symlink():
        return path
    try:
        target = path.resolve(strict=True)
    except (OSError, RuntimeError):
        return None
    if target == root or root not in target.parents or target.is_symlink():
        return None
    return target


def verification_command(root, scratch, argv, system_root=False):
    """``system_root`` mounts the whole host read-only (an engine may be
    installed anywhere), with the host's real /dev and /run/user."""
    binary = shutil.which('bwrap')
    if binary is None:
        raise StoreError('verification_sandbox_unavailable')
    root = Path(root).resolve(strict=True)
    # With the whole host visible, its processes and IPC too (ps, pgrep, a
    # running app or dev server): nothing the host offers is taken away.
    namespaces = [] if system_root else ['--unshare-pid', '--unshare-ipc']
    command = [binary, '--die-with-parent', *namespaces, '--unshare-net',
               '--clearenv', '--setenv', 'PATH', '/usr/local/bin:/usr/bin:/bin',
               '--setenv', 'HOME', str(scratch), '--setenv', 'TMPDIR', str(scratch),
               '--setenv', 'LANG', 'C.UTF-8', '--setenv', 'PYTHONDONTWRITEBYTECODE', '1']
    if system_root:
        command += ['--ro-bind', '/', '/']
    for name in () if system_root else ('/usr', '/bin', '/sbin', '/lib', '/lib64'):
        path = Path(name)
        if path.is_symlink():
            command += ['--symlink', str(path.readlink()), name]
        elif path.exists():
            command += ['--ro-bind', name, name]
    for name in () if system_root else ('/etc/ld.so.cache', '/etc/ld.so.conf', '/etc/localtime'):
        if Path(name).is_file():
            command += ['--ro-bind', name, name]
    # The real /tmp (a screenshot the user points at, a file the worker leaves
    # for the user) and this process's own scratch as TMPDIR. A private /tmp
    # hid both directions.
    # With the whole host visible, its real devices too (a USB phone for
    # adb, a GPU); /run/user stays visible (session bus, audio, display,
    # keyring): everything that works on the host works here.
    command += ['--proc', '/proc', *(['--dev-bind', '/dev', '/dev'] if system_root else ['--dev', '/dev']),
                '--bind', '/tmp', '/tmp', '--bind', str(scratch), str(scratch),
                '--ro-bind', str(root), str(root)]
    from .gateway import PRIVATE
    for current, dirs, files in os.walk(root,followlinks=False):
        for name in list(dirs)+files:
            path = Path(current)/name
            if name in PRIVATE:
                if name in dirs:
                    dirs.remove(name)
                target = _private_target(path, root)
                if target is None:
                    continue
                if target.is_dir():
                    command += ['--tmpfs',str(target),'--remount-ro',str(target)]
                else:
                    command += ['--ro-bind','/dev/null',str(target)]
            elif name in dirs and path.is_symlink():
                dirs.remove(name)
    return command + ['--chdir', str(root), '--', *argv]


# Sharing the network namespace alone leaves DNS and TLS broken in the
# otherwise empty /etc; these are public system files.
_NETWORK_FILES = ('/etc/resolv.conf', '/etc/hosts', '/etc/nsswitch.conf',
                  '/etc/gai.conf', '/etc/ssl/certs/ca-certificates.crt')
# Runtime channel credentials and anything that looks like a credential never
# reach a command, even the owner's own shell.
_SECRET_ENV = re.compile(r'(?i)^CORTEX_|TOKEN|SECRET|PASSW|CREDENTIAL|API_?KEY|PRIVATE_?KEY|AUTH|COOKIE|SESSION')


def allow_network(command):
    """Share the host network in a sandbox command, in place."""
    command.remove('--unshare-net')
    files = []
    for name in _NETWORK_FILES:
        if Path(name).is_file():
            files += ['--ro-bind', str(Path(name).resolve(strict=True)), name]
    at = command.index('--chdir')
    command[at:at] = files


# HOME entries that stay read-only: credentials, and anything that would run
# code outside the sandbox later (autostart and systemd units live in
# .config, executables on PATH in .local/bin, desktop entries in
# .local/share/applications). The agents' own configuration is locked by the
# engine jail itself.
SENSITIVE_HOME = frozenset({'.ssh', '.gnupg', '.aws', '.azure', '.kube', '.docker', '.password-store', '.pki',
                            '.mozilla', '.thunderbird', '.config', '.local', '.claude', '.codex', '.cortex',
                            # a dotfiles repository (hooks run later) and the
                            # orchestrator's own state and runtime
                            '.git', '.paseo', '.hermes', '.cortex-dev'})
SENSITIVE_LOCAL_SHARE = frozenset({'keyrings', 'applications', 'autostart', 'systemd'})
TOOL_DIRS_CREATED = ('.cache', '.npm')


def tool_state_dirs(home: str) -> list[str]:
    """HOME directories every sandboxed process may write: each existing
    hidden directory (where package managers, SDKs and build tools of any
    language keep caches and data) except SENSITIVE_HOME, ~/go, and the
    XDG data/state directories. Not a list of tools: a list would always
    miss one, and that project would fail."""
    root = Path(home)
    for name in TOOL_DIRS_CREATED:
        path = root / name
        if not path.exists() and not path.is_symlink():
            try:
                path.mkdir(mode=0o700)
            except OSError:
                pass
    found = []
    try:
        entries = list(os.scandir(root))
    except OSError:
        return []
    for entry in entries:
        name = entry.name
        if (name.startswith('.') or name == 'go') and name not in SENSITIVE_HOME and entry.is_dir(follow_symlinks=False):
            found.append(str(root / name))
    share = root / '.local' / 'share'
    if share.is_dir() and not share.is_symlink():
        try:
            found += [str(share / entry.name) for entry in os.scandir(share)
                      if entry.name not in SENSITIVE_LOCAL_SHARE and entry.is_dir(follow_symlinks=False)]
        except OSError:
            pass
    state = root / '.local' / 'state'
    if state.is_dir() and not state.is_symlink():
        found.append(str(state))
    return sorted(found)


PROJECT_BIN_DIRS = ('.venv/bin', 'venv/bin', 'node_modules/.bin')


def project_toolchain_env(root: str, path: str, scratch: str) -> list[str]:
    """``--setenv`` args that run commands the way the owner does in the
    project: its virtualenv and node_modules/.bin first on PATH (VIRTUAL_ENV
    set), and ``python`` meaning ``python3`` where no ``python`` exists --
    a plan's ``python -m pytest`` failed on a system with only python3 and a
    project .venv."""
    dirs = [str(Path(root) / relative) for relative in PROJECT_BIN_DIRS if (Path(root) / relative).is_dir()]
    entries = dirs + [entry for entry in path.split(':') if entry]
    if not any((Path(entry) / 'python').exists() for entry in entries):
        python3 = next((Path(entry) / 'python3' for entry in entries if (Path(entry) / 'python3').exists()), None)
        if python3 is not None:
            shims = Path(scratch) / '.cortex-shims'
            try:
                shims.mkdir(exist_ok=True)
                if not (shims / 'python').exists():
                    (shims / 'python').symlink_to(python3)
                entries.append(str(shims))
            except OSError:
                pass
    env = ['--setenv', 'PATH', ':'.join(dict.fromkeys(entries))]
    venv = next((Path(root) / relative for relative in ('.venv', 'venv') if (Path(root) / relative / 'bin').is_dir()), None)
    if venv is not None:
        env += ['--setenv', 'VIRTUAL_ENV', str(venv)]
    return env


def compact_argv(command, fds):
    """Hand bwrap a long argument list through a file descriptor (``--args``)
    instead of the command line, whose size limit a large grant list hit.
    Call last, just before starting the process; returns ``(command, fds)``."""
    if len(command) < 512 or '--' not in command:
        return command, tuple(fds)
    # bwrap takes only options from --args; the command stays on argv.
    split = command.index('--')
    fd = os.memfd_create('cortex-bwrap-args')
    os.write(fd, b''.join(arg.encode('utf-8', 'surrogateescape') + b'\0' for arg in command[1:split]))
    os.lseek(fd, 0, os.SEEK_SET)
    return [command[0], '--args', str(fd), *command[split:]], (*fds, fd)


def as_owner(command, environ):
    """Run a sandbox command as the owner would in their terminal, in place.

    Their name and groups resolve, HOME is their real one (read-only, mounted
    beneath the project so project grants still apply on top), the environment
    is theirs without credentials, and the network is shared. Writes remain
    limited to the granted scope and private runtime paths stay masked.
    """
    home = environ.get('HOME', '')
    mounts = [arg for name in ('/etc/passwd', '/etc/group') if Path(name).is_file()
              for arg in ('--ro-bind', name, name)]
    has_home = home.startswith('/') and Path(home).is_dir()
    if has_home:
        mounts += ['--ro-bind', home, home]
        mounts += [arg for path in tool_state_dirs(home) for arg in ('--bind', path, path)]
    # After the scratch /tmp mount (a HOME under /tmp must stay read-only),
    # before the project mounts (project grants stay writable on top).
    at = command.index('--ro-bind', _dev_index(command) + 1)
    command[at:at] = mounts
    env = [arg for name, value in environ.items()
           if name not in {'HOME', 'TMPDIR', 'PWD', 'OLDPWD'} and not _SECRET_ENV.search(name)
           for arg in ('--setenv', name, value)]
    # HOME is read-only except the tool caches mounted above.
    scratch = command[command.index('TMPDIR', command.index('--clearenv')) + 1]
    env += ['--setenv', 'HOME', home if has_home else scratch]
    env += project_toolchain_env(command[command.index('--chdir') + 1], environ.get('PATH', ''), scratch)
    if not has_home:
        env += ['--setenv', 'XDG_CACHE_HOME', f'{scratch}/.cache']
    at = command.index('--chdir')
    command[at:at] = env
    allow_network(command)


@contextmanager
def gateway_command(root, scratch, argv, scopes, reveal=frozenset(), system_root=False, readonly=()):
    """Pin mount sources to no-follow descriptors, then constrain writes.

    ``.`` (or ``**``) grants the whole project, new top-level files included.
    Globs grant existing matches only. Explicit directory grants allow new
    children; explicit new files are first created with project_write.
    ``readonly`` (the user's deny-write paths) stays read-only on top of any
    grant. Private names in ``reveal`` are mounted read-only instead of hidden.
    """
    from .gateway import PRIVATE, _parts, _parent, _matches
    root = Path(root).resolve(strict=True)
    command = verification_command(root, scratch, argv, system_root)
    # Replace project bind and build scoped grants before masking private data.
    start = command.index('--ro-bind', _dev_index(command) + 1)
    command = command[:start]
    fds = []
    try:
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        fds.append(fd)
        command += ['--ro-bind', f'/proc/self/fd/{fd}', str(root)]
        candidates = set()
        private_paths = {root / name for name in PRIVATE}
        for scope in scopes:
            _parts(scope)
            if scope in {'.','**','**/*'}:
                candidates.add('.')
            elif scope.endswith('/**') and not any(c in scope[:-3] for c in '*?['):
                candidates.add(scope[:-3])
            elif not any(c in scope for c in '*?['):
                candidates.add(scope)
        visited=0
        linked=[]
        # A file inside a granted directory needs no bind of its own (one per
        # file overflowed bwrap's argument list on a large tree).
        dir_grants={c for c in candidates if (root/c).is_dir() and not (root/c).is_symlink()}
        whole='.' in dir_grants
        def granting_dir(relative):
            parts=relative.split('/')
            found=next(('/'.join(parts[:i]) for i in range(1,len(parts)) if '/'.join(parts[:i]) in dir_grants),None)
            if found is None and whole:
                # Under the whole-project grant, a hardlink-heavy directory is
                # judged by its top-level entry (node_modules), not the project.
                return parts[0] if len(parts)>1 else '.'
            return found
        denied_exact=set();denied_globs=[]
        for pattern in readonly:
            base=pattern[:-3] if pattern.endswith('/**') else pattern
            if any(c in base for c in '*?['): denied_globs.append(pattern)
            else: denied_exact.add(base.strip('/'))
        denied_found=[]
        for current,dirs,files in os.walk(root,followlinks=False):
            visited+=len(dirs)+len(files)
            over_limit = visited>_TREE_LIMIT  # grants stop; private masking does not
            for name in list(dirs):
                path=Path(current)/name
                if name in PRIVATE:
                    private_paths.add(path); dirs.remove(name)
                elif path.is_symlink():
                    dirs.remove(name)
            for name in files:
                path=Path(current)/name
                relative=path.relative_to(root).as_posix()
                if name in PRIVATE:
                    private_paths.add(path)
                elif over_limit:
                    continue
                if denied_globs and _matches(relative,denied_globs):
                    denied_found.append(relative)
                if name in PRIVATE:
                    continue
                elif (owner:=granting_dir(relative)) is not None or _matches(relative,scopes):
                    info=path.lstat()
                    if stat.S_ISREG(info.st_mode) and info.st_nlink!=1:
                        # Its other link may be outside the project: writing
                        # through it would escape the scope. Read-only, not a
                        # refusal -- a build's hardlinks (gradle, pnpm) used to
                        # stop the whole worker from starting.
                        linked.append((relative,owner))
                    elif owner is None and not path.is_symlink(): candidates.add(relative)
        # A directory made of hardlinks (pnpm's node_modules points into a
        # store shared by every project) stays read-only as a whole: writing
        # there corrupts other projects, and a bind per link hits the
        # kernel's mount limit.
        per_grant={}
        for _,owner in linked:
            if owner is not None: per_grant[owner]=per_grant.get(owner,0)+1
        demoted={owner for owner,count in per_grant.items() if count>_HARDLINK_DIR_LIMIT and owner!='.'}
        candidates-=demoted
        linked=[relative for relative,owner in linked if owner not in demoted]
        # Read-only on top of the writable binds: demoted directories and the
        # user's deny-write paths.
        overlays=sorted(demoted|{path for path in denied_exact if path and (root/path).exists()}|set(denied_found))
        for relative in sorted(candidates,key=lambda p:(len(PurePath(p).parts),p)):
            path=root/relative
            if not path.exists(): continue
            if relative=='.':
                granted=os.dup(fds[0])
            else:
                try:
                    kind=path.lstat().st_mode
                    if not stat.S_ISREG(kind) and not stat.S_ISDIR(kind):
                        continue  # a socket/FIFO/device is never granted (see below)
                    with _parent(root,relative) as (parent,leaf):
                        granted=os.open(leaf,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK,dir_fd=parent)
                except OSError:
                    continue  # vanished or unopenable: not granted, never a refused worker
            info=os.fstat(granted)
            if not stat.S_ISREG(info.st_mode) and not stat.S_ISDIR(info.st_mode):
                # A socket, FIFO or device (a database's socket in a docker
                # volume) is never granted -- skipped, not a refused worker.
                os.close(granted); continue
            fds.append(granted)
            command+=['--bind',f'/proc/self/fd/{granted}',str(path)]
        for relative in overlays:
            try:
                with _parent(root,relative) as (parent,leaf):
                    pinned=os.open(leaf,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK,dir_fd=parent)
            except OSError:
                continue
            fds.append(pinned)
            command+=['--ro-bind',f'/proc/self/fd/{pinned}',str(root/relative)]
        for relative in linked:  # after the writable binds, so these win
            with _parent(root,relative) as (parent,leaf):
                pinned=os.open(leaf,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK,dir_fd=parent)
            if not stat.S_ISREG(os.fstat(pinned).st_mode):
                os.close(pinned); continue
            fds.append(pinned)
            command+=['--ro-bind',f'/proc/self/fd/{pinned}',str(root/relative)]
        for original in sorted(private_paths):
            # Never mount through a private symlink: mask its in-project target.
            path = _private_target(original, root)
            if path is None:
                continue
            if original.name in reveal and path.exists():
                command+=['--ro-bind',str(path),str(path)]
            elif path.is_file():
                command+=['--ro-bind','/dev/null',str(path)]
            elif path.exists():
                command+=['--tmpfs',str(path),'--remount-ro',str(path)]
        yield command+['--chdir',str(root),'--',*argv],tuple(fds)
    finally:
        for fd in fds: os.close(fd)


# What an engine state directory holds that configures later sessions, the
# owner's own interactive ones included. The engine may write its state, not
# these: a hook written here would later run outside any jail. Each maps to
# its neutral content (None: a directory), created when absent.
ENGINE_CONFIG = {
    'codex-acp': {'config.toml': '', 'hooks.json': '{}\n', 'AGENTS.md': '', 'AGENTS.override.md': '',
                  'skills': None, 'agents': None, 'prompts': None, 'rules': None},
    'claude-agent-acp': {'settings.json': '{}\n', 'settings.local.json': '{}\n', 'CLAUDE.md': '',
                         'AGENTS.md': '', 'skills': None, 'agents': None, 'commands': None,
                         'hooks': None, 'output-styles': None, 'rules': None},
}
# Installed plugins are not loaded in Cortex sessions.
ENGINE_HIDDEN = ('plugins',)


@contextmanager
def engine_jail(root, scratch, argv, scopes, *, env, state_dirs=None, writable=(), readonly=(), sealed=()):
    """Run a whole engine process the way a worker's shell runs.

    The project is read-only except the granted scopes, `.cortex` is hidden
    and the other private names are read-only. HOME is read-only; each engine
    state directory (``state_dirs``: path -> engine) is writable except its
    ENGINE_CONFIG entries, which are created neutral when absent so none can
    be planted. `writable` adds plain
    writable directories (the MCP broker sockets); `sealed` host paths are
    mounted read-only over themselves (the ssh askpass script, which lives
    under the otherwise writable /tmp). `env` is the engine's own.
    """
    from .gateway import PRIVATE
    with gateway_command(root, scratch, argv, scopes, reveal=frozenset(PRIVATE) - {'.cortex'},
                         system_root=True, readonly=readonly) as (command, fds):
        as_owner(command, {'HOME': env.get('HOME', '')})
        extra = [arg for name, value in env.items() if name != 'PATH' for arg in ('--setenv', name, value)]
        extra += ['--setenv', 'TMPDIR', str(scratch)]
        # The worker's shell finds the project's own toolchain first.
        extra += project_toolchain_env(str(Path(root).resolve()), env.get('PATH', ''), str(scratch))
        state_dirs = state_dirs or {}
        real_dirs = {}
        for directory in (*state_dirs, *writable):
            path = Path(directory)
            if not path.exists() and not path.is_symlink() and directory in state_dirs:
                path.mkdir(mode=0o700, parents=True)
            # A dotfile manager links ~/.claude or ~/.codex elsewhere: bind the
            # real directory (the link then resolves into it). Refusing it
            # stopped every engine for such an owner.
            real = path.resolve()
            if directory in state_dirs and not real.exists():
                real.mkdir(mode=0o700, parents=True)
            if not real.is_dir():
                raise StoreError('engine_state_dir_invalid')
            real_dirs[directory] = real
            extra += ['--bind', str(real), str(real)]
        for directory, engine in state_dirs.items():
            for name, neutral in ENGINE_CONFIG[engine].items():
                path = real_dirs[directory] / name
                if path.is_symlink():
                    # Lock what it points to instead; the rest of the host is
                    # read-only already, so a target elsewhere needs nothing.
                    try:
                        target = path.resolve(strict=True)
                    except (OSError, RuntimeError):
                        continue
                    if real_dirs[directory] in target.parents:
                        extra += ['--ro-bind', str(target), str(target)]
                    continue
                if not path.exists():
                    if neutral is None:
                        path.mkdir(mode=0o700)
                    else:
                        path.write_text(neutral)
                extra += ['--ro-bind', str(path), str(path)]
            for name in ENGINE_HIDDEN:
                path = real_dirs[directory] / name
                if path.is_dir() and not path.is_symlink():
                    extra += ['--tmpfs', str(path), '--remount-ro', str(path)]
        for path in sealed:
            extra += ['--ro-bind', str(path), str(path)]
        at = command.index('--chdir')
        command[at:at] = extra
        launch, launch_fds = compact_argv(command, fds)
        try:
            yield launch, launch_fds
        finally:
            for extra in set(launch_fds) - set(fds):
                os.close(extra)
