"""compact_argv's --args payload fd without memfd_create.

This host's Python build has no ``os.memfd_create`` at all, which is the
exact gap the fix covers; both branches are exercised through monkeypatch
instead of relying on host capability.
"""
import os

from cortex_runtime.orchestration.sandbox import compact_argv


def _long_command():
    # len(command) >= 512 and a '--' marker, so compact_argv takes the
    # file-descriptor branch instead of returning the command unchanged.
    grants = [f'--ro-bind-try=/opt/grant-{i}' for i in range(600)]
    return ['bwrap', *grants, '--', 'true']


def test_uses_memfd_create_when_available(monkeypatch):
    calls = []

    def fake_memfd_create(name):
        calls.append(name)
        fd, path = __import__('tempfile').mkstemp(prefix='fake-memfd-')
        os.unlink(path)
        return fd

    monkeypatch.setattr(os, 'memfd_create', fake_memfd_create, raising=False)

    original_command = _long_command()
    command, fds = compact_argv(list(original_command), ())

    assert calls == ['cortex-bwrap-args']
    assert len(fds) == 1
    payload = os.read(fds[0], 1 << 16)
    assert payload.count(b'\0') == len(original_command) - 3  # argv[0] and '--','true' excluded
    os.close(fds[0])


def test_falls_back_to_an_unlinked_temp_file_without_memfd_create(monkeypatch):
    """Simulate a host/libc without memfd_create: identical fd semantics
    (inheritable, writable, seekable, no live path) from the fallback."""
    monkeypatch.delattr(os, 'memfd_create', raising=False)

    original_command = _long_command()
    command, fds = compact_argv(list(original_command), ())

    assert len(fds) == 1
    fd = fds[0]
    payload = os.read(fd, 1 << 16)
    assert payload.count(b'\0') == len(original_command) - 3  # argv[0] and '--','true' excluded
    # the fd keeps no live path: the kernel marks it deleted
    assert '(deleted)' in os.readlink(f'/proc/self/fd/{fd}')
    os.close(fd)


def test_falls_back_when_memfd_create_raises_oserror(monkeypatch):
    """A kernel/seccomp denial (ENOSYS/EPERM) also falls back, not raises."""
    def denied(name):
        raise OSError('memfd_create denied')

    monkeypatch.setattr(os, 'memfd_create', denied, raising=False)

    command, fds = compact_argv(_long_command(), ())
    assert len(fds) == 1
    os.close(fds[0])
