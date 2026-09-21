"""Linux check sandbox: explicit system mounts, project only, no user home/network.

This contains runtime-launched verification commands. It does not claim to
intercept engine-native actions; engines have a different process boundary.
A worker's own shell additionally runs as the owner (`as_owner`): their name,
groups, read-only HOME, secret-free environment and network, while writes
stay confined to the granted scope.
"""
from pathlib import Path, PurePath
import re
import shutil
import os
import stat
from contextlib import contextmanager
from ..contracts import StoreError


def verification_command(root, scratch, argv):
    binary = shutil.which('bwrap')
    if binary is None:
        raise StoreError('verification_sandbox_unavailable')
    root = Path(root).resolve(strict=True)
    command = [binary, '--die-with-parent', '--unshare-pid', '--unshare-ipc', '--unshare-net',
               '--clearenv', '--setenv', 'PATH', '/usr/local/bin:/usr/bin:/bin',
               '--setenv', 'HOME', '/tmp', '--setenv', 'TMPDIR', '/tmp',
               '--setenv', 'LANG', 'C.UTF-8', '--setenv', 'PYTHONDONTWRITEBYTECODE', '1']
    for name in ('/usr', '/bin', '/sbin', '/lib', '/lib64'):
        path = Path(name)
        if path.is_symlink():
            command += ['--symlink', str(path.readlink()), name]
        elif path.exists():
            command += ['--ro-bind', name, name]
    for name in ('/etc/ld.so.cache', '/etc/ld.so.conf', '/etc/localtime'):
        if Path(name).is_file():
            command += ['--ro-bind', name, name]
    command += ['--proc', '/proc', '--dev', '/dev', '--bind', str(scratch), '/tmp',
                '--ro-bind', str(root), str(root)]
    from .gateway import PRIVATE
    visited = 0
    for current, dirs, files in os.walk(root,followlinks=False):
        visited += len(dirs)+len(files)
        if visited > 200000: raise StoreError('gateway_tree_limit')
        for name in list(dirs)+files:
            path = Path(current)/name
            if name in PRIVATE:
                if path.is_symlink(): raise StoreError('gateway_private_path_symlink')
                if name in dirs:
                    dirs.remove(name)
                    command += ['--tmpfs',str(path),'--remount-ro',str(path)]
                else:
                    command += ['--ro-bind','/dev/null',str(path)]
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
    # After the scratch /tmp mount (a HOME under /tmp must stay read-only),
    # before the project mounts (project grants stay writable on top).
    at = command.index('--ro-bind', command.index('--dev') + 1)
    command[at:at] = mounts
    env = [arg for name, value in environ.items()
           if name not in {'HOME', 'TMPDIR', 'PWD', 'OLDPWD'} and not _SECRET_ENV.search(name)
           for arg in ('--setenv', name, value)]
    # HOME is read-only; caches go to the command's writable scratch /tmp.
    env += ['--setenv', 'HOME', home if has_home else '/tmp', '--setenv', 'XDG_CACHE_HOME', '/tmp/.cache']
    at = command.index('--chdir')
    command[at:at] = env
    allow_network(command)


@contextmanager
def gateway_command(root, scratch, argv, scopes):
    """Pin mount sources to no-follow descriptors, then constrain writes.

    Globs grant existing matches only. Explicit directory grants allow new
    children; explicit new files are first created with project_write.
    """
    from .gateway import PRIVATE, _parts, _parent, _matches
    root = Path(root).resolve(strict=True)
    command = verification_command(root, scratch, argv)
    # Replace project bind and build scoped grants before masking private data.
    start = command.index('--ro-bind', command.index('--dev') + 1)
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
                candidates.update(n for n in os.listdir(root) if n not in PRIVATE)
            elif scope.endswith('/**') and not any(c in scope[:-3] for c in '*?['):
                candidates.add(scope[:-3])
            elif not any(c in scope for c in '*?['):
                candidates.add(scope)
        visited=0
        for current,dirs,files in os.walk(root,followlinks=False):
            visited+=len(dirs)+len(files)
            if visited>200000:
                raise StoreError('gateway_tree_limit')
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
                elif _matches(relative,scopes):
                    info=path.lstat()
                    if stat.S_ISREG(info.st_mode) and info.st_nlink!=1:
                        raise StoreError('gateway_hardlink_denied')
                    if not path.is_symlink(): candidates.add(relative)
        for relative in sorted(candidates,key=lambda p:(len(PurePath(p).parts),p)):
            path=root/relative
            if not path.exists(): continue
            if relative=='.':
                granted=os.dup(fds[0])
            else:
                with _parent(root,relative) as (parent,leaf):
                    granted=os.open(leaf,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK,dir_fd=parent)
            info=os.fstat(granted)
            if not stat.S_ISREG(info.st_mode) and not stat.S_ISDIR(info.st_mode):
                os.close(granted); raise StoreError('gateway_regular_file_required')
            fds.append(granted)
            command+=['--bind',f'/proc/self/fd/{granted}',str(path)]
        for path in sorted(private_paths):
            if path.is_symlink():
                # Never follow a private-path symlink when applying a mount.
                raise StoreError('gateway_private_path_symlink')
            if path.is_file():
                command+=['--ro-bind','/dev/null',str(path)]
            elif path.exists():
                command+=['--tmpfs',str(path),'--remount-ro',str(path)]
        yield command+['--chdir',str(root),'--',*argv],tuple(fds)
    finally:
        for fd in fds: os.close(fd)
