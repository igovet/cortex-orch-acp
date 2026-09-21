"""Linux check sandbox: explicit system mounts, project only, no user home/network.

This contains runtime-launched verification commands. It does not claim to
intercept engine-native actions; engines have a different process boundary.
"""
from pathlib import Path, PurePath
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
