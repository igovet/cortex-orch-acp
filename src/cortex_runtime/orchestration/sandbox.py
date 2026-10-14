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


def verification_command(root, scratch, argv, system_root=False, *, mask_private=True):
    """``system_root`` mounts the whole host read-only (an engine may be
    installed anywhere), with the host's real /dev and /run/user.
    ``mask_private=False`` is only for a caller that replaces the project
    mount and masks private names itself (``gateway_command``)."""
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
               '--setenv', 'TMP', str(scratch), '--setenv', 'TEMP', str(scratch),
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
    # /var/tmp has no such interop story (nothing points a real user at it),
    # and on some hosts it is a read-only mount or absent entirely once the
    # whole host is bound read-only (``system_root``) -- code that hardcodes
    # it (some tools do) still needs a real, writable, per-run-private
    # directory there, so it gets its own scratch-backed subdirectory rather
    # than the host's actual (possibly read-only, always shared) /var/tmp.
    var_tmp = Path(scratch) / 'var-tmp'
    var_tmp.mkdir(mode=0o700, exist_ok=True)
    # With the whole host visible, its real devices too (a USB phone for
    # adb, a GPU); /run/user stays visible (session bus, audio, display,
    # keyring): everything that works on the host works here.
    command += ['--proc', '/proc', *(['--dev-bind', '/dev', '/dev'] if system_root else ['--dev', '/dev']),
                '--bind', '/tmp', '/tmp', '--bind', str(scratch), str(scratch),
                '--bind', str(var_tmp), '/var/tmp',
                '--ro-bind', str(root), str(root)]
    from .gateway import PRIVATE
    for current, dirs, files in os.walk(str(root),followlinks=False) if mask_private else ():
        for name in list(dirs)+files:
            if name in PRIVATE:
                if name in dirs:
                    dirs.remove(name)
                target = _private_target(Path(current, name), root)
                if target is None:
                    continue
                if target.is_dir():
                    command += ['--tmpfs',str(target),'--remount-ro',str(target)]
                else:
                    command += ['--ro-bind','/dev/null',str(target)]
            elif name in dirs and os.path.islink(os.path.join(current, name)):
                dirs.remove(name)
    if system_root:
        # A linked worktree's runtime store lives in its main repository,
        # outside root, where the host-wide read-only mount would show it.
        store = _outside_store(root)
        if store is not None:
            command += ['--tmpfs', str(store), '--remount-ro', str(store)]
    return command + ['--chdir', str(root), '--', *argv]


def _outside_store(root: Path) -> Path | None:
    from ..project_storage import logical_project_root
    try:
        project = Path(logical_project_root(str(root)))
    except StoreError:
        return None
    store = project / '.cortex'
    if project == root or store.is_symlink() or not store.is_dir():
        return None
    return store


# Sharing the network namespace alone leaves DNS and TLS broken in the
# otherwise empty /etc; these are public system files.
_NETWORK_FILES = ('/etc/resolv.conf', '/etc/hosts', '/etc/nsswitch.conf',
                  '/etc/gai.conf', '/etc/ssl/certs/ca-certificates.crt')
# Runtime channel credentials and anything that looks like a credential never
# reach a command, even the owner's own shell. A commit author's name, email
# and date (GIT_AUTHOR_*) are identity, not credentials.
_SECRET_ENV = re.compile(r'(?i)^CORTEX_|TOKEN|SECRET|PASSW|CREDENTIAL|API_?KEY|PRIVATE_?KEY|'
                         r'AUTH(?!OR_(?:NAME|EMAIL|DATE)$)|COOKIE|SESSION')


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


def readonly_test_cache(command, scratch, environ):
    """Keep pytest's cache writable without granting writes to reviewed sources.

    Preserve owner options; explicit command-line options still take precedence.
    """
    import shlex
    options = environ.get('PYTEST_ADDOPTS', '')
    options += ' ' + shlex.join(['-o', f'cache_dir={scratch}/pytest-cache'])
    at = command.index('--chdir')
    command[at:at] = ['--setenv', 'PYTEST_ADDOPTS', options.strip()]


def as_owner(command, environ, *, writable_home=False):
    """Run a sandbox command as the owner would in their terminal, in place.

    Their name and groups resolve, HOME is their real one (read-only, mounted
    beneath the project so project grants still apply on top), the environment
    is theirs without credentials (their ssh-agent socket stays: it holds no
    key material), and the network is shared. Writes remain
    limited to the granted scope and private runtime paths stay masked.
    """
    home = environ.get('HOME', '')
    mounts = [arg for name in ('/etc/passwd', '/etc/group') if Path(name).is_file()
              for arg in ('--ro-bind', name, name)]
    has_home = home.startswith('/') and Path(home).is_dir()
    if has_home and not writable_home:
        mounts += ['--ro-bind', home, home]
        mounts += [arg for path in tool_state_dirs(home) for arg in ('--bind', path, path)]
    # After the scratch /tmp mount (a HOME under /tmp must stay read-only),
    # before the project mounts (project grants stay writable on top).
    at = command.index('--ro-bind', _dev_index(command) + 1)
    command[at:at] = mounts
    env = [arg for name, value in environ.items()
           if name not in {'HOME', 'TMPDIR', 'PWD', 'OLDPWD'} and not _SECRET_ENV.search(name)
           for arg in ('--setenv', name, value)]
    # Their own ssh-agent (the secret filter above drops SSH_AUTH_SOCK).
    from .ssh_askpass import host_agent_env
    env += [arg for name, value in host_agent_env(environ).items() for arg in ('--setenv', name, value)]
    # HOME is read-only except the tool caches mounted above.
    scratch = command[command.index('TMPDIR', command.index('--clearenv')) + 1]
    env += ['--setenv', 'HOME', home if has_home else scratch]
    env += project_toolchain_env(command[command.index('--chdir') + 1], environ.get('PATH', ''), scratch)
    if not has_home:
        env += ['--setenv', 'XDG_CACHE_HOME', f'{scratch}/.cache']
    at = command.index('--chdir')
    command[at:at] = env
    allow_network(command)
    # ssh's root-owned config includes, as the host's ssh trusts them.
    from .ssh_askpass import mount_trusted_configs
    mount_trusted_configs(command, home if has_home else '', Path(scratch) / 'ssh-config')


# A one-shot sandboxed command (a worker's own shell, a verification check)
# runs with an explicit umask rather than inheriting whatever the Cortex
# runtime process itself set (0077, deliberately restrictive for the
# runtime's own private store -- see server.py/split.py/agent.py): 022 is
# what git, npm, make and every other tool such a command runs expect from
# a normal shell, and nothing it creates is more sensitive than the project
# tree and scratch directory it already has full read/write access to (see
# SECURITY.md, "Sandboxed command umask"). bwrap itself has no ``--umask``
# option, so the umask is set by the sandboxed process itself, right before
# it execs the real command -- never applied to a whole engine process
# (``engine_jail``), whose own state directories (credentials included)
# keep the runtime's stricter default.
COMMAND_UMASK = 0o022


def with_umask(argv, mode=COMMAND_UMASK):
    """Wrap ``argv`` to run under an explicit umask inside the sandbox."""
    return ['/bin/sh', '-c', f'umask {mode:04o}; exec "$@"', 'sh', *argv]


# What in a git metadata directory configures or redirects git runs the owner
# makes later, outside any sandbox: settings (hooks path, fsmonitor, editor,
# pager, filters, credential helpers, includes), hooks, the pointer to a
# repository's common directory (a planted ``commondir`` swaps in another
# config), a linked worktree's back-pointer, and legacy remote definitions.
# Everything else a sandboxed git writes (objects, refs, logs, index, HEAD,
# packed-refs, locks, merge/rebase state) is data.
GIT_GUARDED = ('config', 'config.worktree', 'commondir', 'gitdir', 'hooks', 'remotes', 'branches')
# Other linked worktrees' metadata and submodule repositories.
GIT_GUARDED_COMMON = ('worktrees', 'modules')
_GIT_SWEEP_SECONDS = 0.5


def git_metadata_mounts(root: Path):
    """How a sandboxed git in ``root`` may update its repository, or ``None``
    when ``root`` itself is not a git checkout (a repository above the
    project stays read-only like the rest of the host).

    Returns ``(writable, readonly, watched)``: the git directory (and, for a
    linked worktree, its common directory) mounted writable; then read-only
    on top the other worktrees' metadata and submodules, every existing
    GIT_GUARDED entry and the effective hooks directory when it lies in the
    project (``core.hooksPath``, e.g. husky); ``watched`` the absent
    GIT_GUARDED paths, which a mount cannot cover without creating them, so
    GitGuard removes any that appear."""
    from ..delegation.revision import git_metadata_dirs
    dirs = git_metadata_dirs(str(root))
    if dirs is None:
        return None
    writable = [Path(directory) for directory in dirs]
    gitdir, common = writable[0], writable[-1]
    readonly, watched = [], []
    for name in GIT_GUARDED_COMMON:
        path = common / name
        if path.is_dir() and not path.is_symlink() and path != gitdir and path not in gitdir.parents:
            readonly.append(path)
        elif path.is_dir() and not path.is_symlink():
            # A linked worktree: its own gitdir (below) stays writable, its
            # siblings do not.
            readonly += [child for child in path.iterdir() if child != gitdir]
    for directory in writable:
        for name in GIT_GUARDED:
            path = directory / name
            if name == 'gitdir' and directory == common:
                continue  # only a linked worktree's gitdir holds this pointer
            (readonly if path.exists() or path.is_symlink() else watched).append(path)
    hooks = _configured_hooks_dir(root)
    if hooks is not None:
        readonly.append(hooks)
    return writable, readonly, watched


def _configured_hooks_dir(root: Path, *, include_external=False) -> Path | None:
    """``core.hooksPath`` when it names an existing directory inside the
    project (otherwise it is already read-only, or absent)."""
    import subprocess
    try:
        done = subprocess.run(['git', '-C', str(root), '--no-optional-locks', 'config', '--get', 'core.hooksPath'],
                              capture_output=True, timeout=10,
                              env={'PATH': '/usr/local/bin:/usr/bin:/bin', 'HOME': os.environ.get('HOME', '/'),
                                   'GIT_TERMINAL_PROMPT': '0'})
    except (OSError, subprocess.SubprocessError):
        return None
    value = done.stdout.decode('utf-8', 'surrogateescape').strip()
    if done.returncode != 0 or not value:
        return None
    path = Path(os.path.expanduser(value))
    path = Path(os.path.normpath(path if path.is_absolute() else root / path))
    if path == root or (not include_external and root not in path.parents) or not path.is_dir():
        return None
    return path


class GitGuard:
    """Removes GIT_GUARDED entries planted where none existed, while the
    sandbox lives and once more when it ends. ``removed`` lists them."""

    def __init__(self, watched):
        import threading
        self.watched = list(watched)
        self.removed = []
        self._stop = threading.Event()
        self._thread = None
        if self.watched:
            self._thread = threading.Thread(target=self._run, name='cortex-git-guard', daemon=True)
            self._thread.start()

    def _run(self):
        while not self._stop.wait(_GIT_SWEEP_SECONDS):
            self.sweep()

    def sweep(self):
        for path in self.watched:
            try:
                info = path.lstat()
            except FileNotFoundError:
                continue
            except OSError:
                continue
            try:
                if stat.S_ISDIR(info.st_mode):
                    shutil.rmtree(path)
                else:
                    path.unlink()
            except FileNotFoundError:
                pass
            except OSError:
                continue
            self.removed.append(str(path))

    def close(self):
        self._stop.set()
        if self._thread is not None:
            self._thread.join()
        self.sweep()


@contextmanager
def gateway_command(root, scratch, argv, scopes, reveal=frozenset(), system_root=False, readonly=(),
                    git_writable=False, git_guard=None):
    """Pin mount sources to no-follow descriptors, then constrain writes.

    ``.`` (or ``**``) grants the whole project, new top-level files included.
    Globs grant existing matches only. Explicit directory grants allow new
    children; explicit new files are first created with project_write.
    ``readonly`` (the user's deny-write paths) stays read-only on top of any
    grant. Private names in ``reveal`` are mounted read-only instead of hidden.
    ``git_writable`` with a whole-project grant lets git update the project's
    own repository (``git_metadata_mounts``); without that grant the root
    ``.git`` follows ``reveal``. ``git_guard`` (a list) receives the planted
    git configuration paths removed when the command ends.
    """
    from .gateway import PRIVATE, _parts, _parent, _matches
    root = Path(root).resolve(strict=True)
    git = None
    if git_writable and any(scope in {'.', '**', '**/*'} for scope in scopes):
        git = git_metadata_mounts(root)
    command = verification_command(root, scratch, argv, system_root, mask_private=False)
    # Replace project bind and build scoped grants before masking private data.
    start = command.index('--ro-bind', _dev_index(command) + 1)
    command = command[:start]
    fds = []
    guard = GitGuard(git[2] if git else ())
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
        # Per inode: its link count and each distinct writable link found
        # (a link is a directory entry: parent inode plus name, so a tree
        # seen twice through a bind mount is not counted twice).
        link_counts={};link_entries={}
        # Plain strings, not Path objects: a large tree (node_modules, a
        # virtualenv) made Path construction and relative_to dominate every
        # sandboxed command at several seconds each.
        root_text=str(root)
        for current,dirs,files in os.walk(root_text,followlinks=False):
            visited+=len(dirs)+len(files)
            over_limit = visited>_TREE_LIMIT  # grants stop; private masking does not
            parent_id=None
            rel_dir=current[len(root_text)+1:]
            for name in list(dirs):
                if name in PRIVATE:
                    private_paths.add(Path(current,name)); dirs.remove(name)
                elif os.path.islink(os.path.join(current,name)):
                    dirs.remove(name)
            for name in files:
                relative=f'{rel_dir}/{name}' if rel_dir else name
                if name in PRIVATE:
                    private_paths.add(Path(current,name))
                elif over_limit:
                    continue
                if denied_globs and _matches(relative,denied_globs):
                    denied_found.append(relative)
                if name in PRIVATE:
                    continue
                elif (owner:=granting_dir(relative)) is not None or _matches(relative,scopes):
                    info=os.lstat(os.path.join(current,name))
                    if stat.S_ISREG(info.st_mode) and info.st_nlink!=1:
                        # Its other link may be outside the project: writing
                        # through it would escape the scope. Read-only, not a
                        # refusal -- a build's hardlinks (gradle, pnpm) used to
                        # stop the whole worker from starting.
                        key=(info.st_dev,info.st_ino)
                        if parent_id is None:
                            parent=os.lstat(current); parent_id=(parent.st_dev,parent.st_ino)
                        if link_counts.setdefault(key,info.st_nlink)!=info.st_nlink:
                            link_counts[key]=None  # changed while walking: never trusted
                        if not (readonly and _matches(relative,readonly)):
                            link_entries.setdefault(key,{})[(parent_id,name)]=owner
                        linked.append((relative,owner,key))
                    elif owner is None and not stat.S_ISLNK(info.st_mode): candidates.add(relative)
        def internal(key,excluded=frozenset()):
            # Every link of this file is a writable path of this project
            # (esbuild links its two binaries together): writing through one
            # reaches nothing outside, and pinning it made the tree
            # impossible to reinstall (EBUSY on unlink).
            entries=link_entries.get(key,{})
            return (link_counts.get(key) is not None and len(entries)==link_counts[key]
                    and not any(owner in excluded for owner in entries.values()))
        # A directory made of hardlinks (pnpm's node_modules points into a
        # store shared by every project) stays read-only as a whole: writing
        # there corrupts other projects, and a bind per link hits the
        # kernel's mount limit.
        per_grant={}
        for _,owner,key in linked:
            if owner is not None and not internal(key): per_grant[owner]=per_grant.get(owner,0)+1
        demoted={owner for owner,count in per_grant.items() if count>_HARDLINK_DIR_LIMIT and owner!='.'}
        candidates-=demoted
        kept=[(relative,owner,key) for relative,owner,key in linked if owner not in demoted]
        linked=[relative for relative,owner,key in kept if not internal(key,demoted)]
        candidates|={relative for relative,owner,key in kept if owner is None and internal(key,demoted)}
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
        if git is not None:
            writable_git, readonly_git, _ = git
            for directory in writable_git:
                pinned=os.open(directory,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
                fds.append(pinned)
                command+=['--bind',f'/proc/self/fd/{pinned}',str(directory)]
            # Guarded entries after the writable binds, so these win.
            command+=[arg for path in readonly_git for arg in ('--ro-bind',str(path),str(path))]
        if git is None and '.git' in reveal:
            # Also pin a linked worktree's common metadata read-only, including
            # when it lives in the host's shared writable /tmp.
            from ..delegation.revision import git_metadata_dirs
            for directory in git_metadata_dirs(str(root)) or ():
                command += ['--ro-bind', directory, directory]
        for original in sorted(private_paths):
            if git is not None and original == root/'.git':
                if original.is_file():
                    # A linked worktree's gitdir pointer, visible read-only.
                    command+=['--ro-bind',str(original),str(original)]
                continue
            # Never mount through a private symlink: mask its in-project target.
            path = _private_target(original, root)
            if path is None:
                continue
            if original.name in reveal and path.exists():
                command+=['--ro-bind',str(path),str(path)]
            elif path.is_file():
                command+=['--ro-bind','/dev/null',str(path)]
            elif path.exists():
                command+=['--tmpfs',str(path)]
            if path.is_dir() and original.name not in reveal:
                command += ['--remount-ro', str(path)]
        yield command+['--chdir',str(root),'--',*argv],tuple(fds)
    finally:
        for fd in fds: os.close(fd)
        guard.close()
        if git_guard is not None:
            git_guard.extend(guard.removed)


def control_plane_paths(root, environ):
    """Known control-plane locations as named, existing or not."""
    root = Path(root).resolve(strict=True)
    home = Path(environ.get('HOME') or str(Path.home()))
    config = Path(environ.get('XDG_CONFIG_HOME') or home / '.config')
    state = Path(environ.get('XDG_STATE_HOME') or home / '.local/state')
    data = Path(environ.get('XDG_DATA_HOME') or home / '.local/share')
    paths = {config / 'cortex', state / 'cortex', home / '.paseo', config / 'Paseo',
             home / '.codex', home / '.claude', config / 'opencode', data / 'opencode',
             home / '.gitconfig', home / '.git-credentials'}
    for key in ('CODEX_HOME', 'CLAUDE_CONFIG_DIR'):
        if environ.get(key):
            path = Path(environ[key])
            if not path.is_absolute() or not path.exists():
                raise StoreError('command_protection_unavailable')
            paths.add(path)
    installed = Path(__file__).resolve().parents[3]
    if installed != root and root not in installed.parents:
        paths.add(installed)
    from ..project_storage import logical_project_root
    paths.add(Path(logical_project_root(str(root))) / '.cortex')
    hooks = _configured_hooks_dir(root, include_external=True)
    if hooks is not None:
        paths.add(hooks)
    return paths


def protected_host_paths(root, environ):
    """Known control-plane locations; not a hostile-code isolation guarantee."""
    root = Path(root).resolve(strict=True)
    paths = control_plane_paths(root, environ)
    resolved = set()
    for path in paths:
        if not path.exists() and not path.is_symlink():
            continue
        try:
            real = path.resolve(strict=True)
        except (OSError, RuntimeError) as error:
            raise StoreError('command_protection_unavailable') from error
        if real == root:
            raise StoreError('command_protection_overlaps_workspace')
        resolved.add(real)
    return sorted(resolved)


@contextmanager
def development_command(root, scratch, argv, scopes, *, environ, readonly=(), git_guard=None):
    """Host-writable commands of a Development or Auto task; the engine jail is unchanged.

    The mode decides the host (writable outside the protected control
    plane); ``scopes`` still decide the project: a writing worker's whole
    project, a check's git-ignored directories, nothing for a read-only role.
    Docker, outside aliases and same-user programs can bypass direct masks.
    """
    with gateway_command(root, scratch, argv, scopes, system_root=True, readonly=readonly,
                         reveal=frozenset({'.git'}), git_writable=True, git_guard=git_guard) as (command, fds):
        at = command.index('--ro-bind')
        if command[at:at+3] != ['--ro-bind', '/', '/']:
            raise StoreError('command_protection_unavailable')
        command[at] = '--bind'
        at = command.index('/var/tmp')
        del command[at-2:at+1]
        as_owner(command, environ, writable_home=True)
        overlays = []
        for path in protected_host_paths(root, environ):
            overlays += (['--tmpfs', str(path), '--remount-ro', str(path)]
                         if path.name == '.cortex' and path.is_dir()
                         else ['--ro-bind', str(path), str(path)])
        # Parent protection precedes task grants, so a host-created worktree
        # beneath a protected installation remains its own writable task tree.
        at = command.index('--ro-bind', _dev_index(command) + 1)
        command[at:at] = overlays
        overlays = []
        for path in protected_host_paths(root, environ):
            if Path(root).resolve() in path.parents:
                overlays += (['--tmpfs', str(path), '--remount-ro', str(path)]
                             if path.name == '.cortex' and path.is_dir()
                             else ['--ro-bind', str(path), str(path)])
        from .gateway import PRIVATE
        for current, dirs, files in os.walk(str(root), followlinks=False):
            for name in list(dirs) + files:
                if name in PRIVATE:
                    if name in dirs:
                        dirs.remove(name)
                    path = Path(current, name)
                    if path.is_symlink():
                        try:
                            target = path.resolve(strict=True)
                        except (OSError, RuntimeError) as error:
                            raise StoreError('command_protection_unavailable') from error
                        if target == Path(root).resolve():
                            raise StoreError('command_protection_overlaps_workspace')
                        overlays += (['--tmpfs', str(target), '--remount-ro', str(target)]
                                     if target.is_dir() and name != '.git'
                                     else ['--ro-bind', str(target), str(target)])
        at = command.index('--chdir')
        command[at:at] = overlays
        yield command, fds


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
    # OpenCode's data directory ($XDG_DATA_HOME/opencode) holds the session
    # database and native logins, opened in place by OpenCode, and
    # ``bin``: helper executables it downloads and later runs in the owner's
    # own interactive sessions, outside any jail. Its configuration lives in
    # ~/.config/opencode, which the OpenCode jail hides (``masked``).
    'opencode': {'bin': None},
}
# Installed plugins are not loaded in Cortex sessions.
ENGINE_HIDDEN = ('plugins',)


@contextmanager
def engine_jail(root, scratch, argv, scopes, *, env, state_dirs=None, writable=(), readonly=(), sealed=(),
                isolate_processes=False, masked=(), data_files=None):
    """Run a whole engine process the way a worker's shell runs.

    The project is read-only except the granted scopes, `.cortex` is hidden
    and the other private names are read-only -- except that with a
    whole-project grant git may update the project's own repository
    (``git_metadata_mounts``). HOME is read-only; each engine
    state directory (``state_dirs``: path -> engine) is writable except its
    ENGINE_CONFIG entries, which are created neutral when absent so none can
    be planted. `writable` adds plain
    writable directories (the MCP broker sockets); `sealed` host paths are
    mounted read-only over themselves (the ssh askpass script, which lives
    under the otherwise writable /tmp). `env` is the engine's own.

    ``isolate_processes`` gives the engine its own user, PID and IPC
    namespaces. Another jail runs in a sibling user namespace, so the kernel
    refuses it ``/proc/<pid>/environ`` of this engine's processes (the
    ptrace access check needs CAP_SYS_PTRACE in the target's user
    namespace), and this engine sees none of the host's processes. OpenCode
    needs it: its private control server keeps its password in its
    environment. ``masked`` host directories are hidden behind an empty,
    jail-private tmpfs (OpenCode's own user configuration). ``data_files``
    (path -> bytes) appear read-only at those paths, copied by bwrap from a
    memory file: no other process can change them on the host, although the
    empty mount point itself is created there.
    """
    from .gateway import PRIVATE
    with gateway_command(root, scratch, argv, scopes, reveal=frozenset(PRIVATE) - {'.cortex'},
                         system_root=True, readonly=readonly, git_writable=True) as (command, fds):
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
        for directory in masked:
            path = Path(directory)
            if path.is_symlink():
                # A dotfile manager links it elsewhere: hide what it points to.
                try:
                    path = path.resolve(strict=True)
                except (OSError, RuntimeError):
                    continue
            if path.is_dir():
                extra += ['--tmpfs', str(path)]
        data_fds = []
        try:
            for destination, content in (data_files or {}).items():
                fd = os.memfd_create('cortex-engine-data')
                data_fds.append(fd)
                view = memoryview(bytes(content))
                while view:
                    view = view[os.write(fd, view):]
                os.lseek(fd, 0, os.SEEK_SET)
                extra += ['--perms', '0400', '--ro-bind-data', str(fd), str(destination)]
            if isolate_processes:
                at = command.index('--die-with-parent') + 1
                command[at:at] = ['--unshare-user', '--unshare-pid', '--unshare-ipc']
            at = command.index('--chdir')
            command[at:at] = extra
            if not scopes:
                readonly_test_cache(command, scratch, env)
            launch, launch_fds = compact_argv(command, (*fds, *data_fds))
        except BaseException:
            for fd in data_fds:
                os.close(fd)
            raise
        try:
            yield launch, launch_fds
        finally:
            for extra in set(launch_fds) - set(fds):
                os.close(extra)
