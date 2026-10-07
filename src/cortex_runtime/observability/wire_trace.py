"""Opt-in developer wire traces. Never an acceptance or command receipt."""
from __future__ import annotations

from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import re
import stat
import sys
import threading
import time


OPT_IN = "CORTEX_DEV_WIRE_TRACE"
HOME_MARKER = "CORTEX_DEV_TRACE_HOME"
MAX_BYTES = 16 * 1024 * 1024
FILE_COUNT = 4
REDACTED = "[REDACTED]"
_SECRET_KEY = re.compile(r"secret|password|passwd|authorization|(?:^|[-_])auth(?:$|[-_])|cookie|api[_.-]?key|credential|(?:^|[_-])(?:access[_-]?|refresh[_-]?)?token$", re.I)
# "Authorization"/"Proxy-Authorization" etc. are handled by _AUTH_ASSIGNMENT
# below (H2 fix S6), which skips an optional scheme word (Token, Basic, ...)
# before capturing the real credential; a bare "token"/"secret"/... key is
# still handled here.
_ASSIGNMENT = re.compile(r"""(?ix)(\b(?:[\w.-]*(?:secret|password|passwd|api[_-]?key|access[_-]?token|refresh[_-]?token|token)|cookie|token)\b\s*["']?\s*[:=]\s*["']?)([^\s"',;}]+)""")
_AUTH_ASSIGNMENT = re.compile(
    r"""(?i)(\b[\w.-]*authorization\b\s*["']?\s*[:=]\s*["']?)(?:([A-Za-z][\w.+/=~!#$%&*^`|-]*)\s+)?([^\s"',;}]+)"""
)
_AUTH = re.compile(r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9+/_.=-]+")
_KEY = re.compile(r"\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9]{12,}|AKIA[A-Z0-9]{16})\b")
_PEM = re.compile(r"-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----", re.S)


def _secret_key(key):
    return bool(_SECRET_KEY.search(str(key)))


def _diagnostic(message):
    try:
        print(message, file=sys.stderr)
    except Exception:
        pass


class Redactor:
    def __init__(self, environ):
        from .redact import scrub_wire_text
        self._scrub_wire_text = scrub_wire_text
        self.values = {value for key, value in environ.items()
                       if _secret_key(key) and isinstance(value, str) and value}
        # Do not retain an unbounded registry of message-supplied secrets.
        self.overflow = False

    def _learn(self, value):
        if isinstance(value, dict):
            if _secret_key(value.get("name", "")) and "value" in value:
                self._remember(value["value"])
            for key, item in value.items():
                if _secret_key(key):
                    self._remember(item)
                else:
                    self._learn(item)
        elif isinstance(value, list):
            for index, item in enumerate(value):
                if isinstance(item, str) and item == "--relay" and index + 2 < len(value):
                    self._remember(value[index + 2])
                if isinstance(item, str) and item.startswith("--") and _secret_key(item[2:].split("=")[0]):
                    if "=" in item:
                        self._remember(item.split("=", 1)[1])
                    elif index + 1 < len(value):
                        self._remember(value[index + 1])
                self._learn(item)
        elif isinstance(value, str):
            # H2 fix (S6): learn the real credential for an authorization
            # header -- never the bare scheme word ("token", "Basic", ...) --
            # so an ordinary later use of that word is not globally redacted.
            for match in _AUTH_ASSIGNMENT.finditer(value):
                self._remember(match[3])
            for match in _ASSIGNMENT.finditer(value):
                self._remember(match[2])
            for match in _AUTH.finditer(value):
                self._remember(match[0].split(None, 1)[1])
            for match in _KEY.finditer(value):
                self._remember(match[0])
            try:
                decoded = json.loads(value)
            except (ValueError, RecursionError):
                return
            if isinstance(decoded, (dict, list)):
                self._learn(decoded)

    def _remember(self, value):
        if isinstance(value, str) and value:
            if len(self.values) >= 4096 and value not in self.values:
                self.overflow = True
            else:
                self.values.add(value)
        elif isinstance(value, (list, dict)):
            for item in (value.values() if isinstance(value, dict) else value):
                self._remember(item)

    def _text(self, value):
        try:
            decoded = json.loads(value)
        except (ValueError, RecursionError):
            decoded = None
        if isinstance(decoded, (dict, list)):
            return json.dumps(self._clean(decoded), ensure_ascii=True)
        for secret in sorted(self.values, key=len, reverse=True):
            value = value.replace(secret, REDACTED)
        value = _PEM.sub(REDACTED, value)
        value = _AUTH.sub(lambda m: m[1] + " " + REDACTED, value)
        value = _KEY.sub(REDACTED, value)
        # Authorization-header rule (S1-equivalent) runs before the generic
        # assignment rule so a scheme word is never mistaken for the value.
        value = _AUTH_ASSIGNMENT.sub(lambda m: m[1] + REDACTED, value)
        value = _ASSIGNMENT.sub(lambda m: m[1] + REDACTED, value)
        return self._scrub_wire_text(value)

    def _clean(self, value):
        if isinstance(value, dict):
            secret_pair = _secret_key(value.get("name", ""))
            return {key: REDACTED if _secret_key(key) or (secret_pair and key == "value")
                    else self._clean(item) for key, item in value.items()}
        if isinstance(value, (list, tuple)):
            result = []
            hide_next = False
            relay_index = None
            for index, item in enumerate(value):
                if hide_next or index == relay_index:
                    result.append(REDACTED)
                    hide_next = False
                elif isinstance(item, str) and item == "--relay":
                    result.append(item)
                    relay_index = index + 2
                elif isinstance(item, str) and item.startswith("--") and _secret_key(item[2:].split("=")[0]):
                    if "=" in item:
                        result.append(item.split("=", 1)[0] + "=" + REDACTED)
                    else:
                        result.append(item)
                        hide_next = True
                else:
                    result.append(self._clean(item))
            return result
        return self._text(value) if isinstance(value, str) else value

    def redact(self, value):
        self._learn(value)
        if self.overflow:
            raise ValueError("redaction_registry_exhausted")
        return self._clean(value)


def _ancestors_up_to(path, root):
    """``path`` and each ancestor up to and including ``root`` (inclusive).

    ``root`` must be ``path`` or one of its ancestors; a .git/.cortex marker
    anywhere *above* ``root`` (e.g. a developer's $HOME or an enclosing
    checkout) is therefore never consulted -- see W2.
    """
    current = path
    while True:
        yield current
        if current == root:
            return
        current = current.parent


def _gate(environ, runtime_root):
    home = Path.home()
    dev = home / ".paseo-dev"
    root = Path(runtime_root)
    # Source-tree launch is an existing dev mechanism; an installed payload
    # has neither .git nor the guarded wrapper. Production XDG state refuses.
    if (environ.get(HOME_MARKER) != str(dev)
            or environ.get("XDG_CONFIG_HOME") != str(dev / "cortex-xdg/config")
            or environ.get("XDG_STATE_HOME") != str(dev / "cortex-xdg/state")
            or environ.get("CORTEX_PASEO_SOURCE_ROOT") != str(root)
            or not root.is_absolute() or root.resolve() != root
            or not (root / ".git").exists()
            or not (root / "scripts/cortex_paseo_dev.py").is_file()
            or root.is_relative_to(dev)
            or root.is_relative_to(home / ".local/state")
            or root.is_relative_to(home / ".paseo")):
        raise ValueError("dev_wire_trace_refused")
    directory = dev / "cortex-xdg/state/cortex/wire-traces"
    # The .git/.cortex ancestor check is scoped to the subtree from the
    # trace directory up to and including the guarded dev home itself
    # (W2): walking further up would reach $HOME, which the developer's own
    # checkout(s) may legitimately make a Git repository, and that must
    # never disable tracing. A .git/.cortex anywhere inside the dev home's
    # own subtree still refuses.
    for parent in _ancestors_up_to(directory, dev):
        if (parent / ".git").exists() or parent.name == ".cortex":
            raise ValueError("dev_wire_trace_refused")
        if parent.exists() and parent.resolve() != parent:
            raise ValueError("dev_wire_trace_refused")
    return directory


class WireTrace:
    def __init__(self, directory, environ, *, max_bytes=MAX_BYTES, file_count=FILE_COUNT):
        self.directory = Path(directory)
        self.max_bytes = max_bytes
        self.file_count = file_count
        self.redactor = Redactor(environ)
        self._lock = threading.RLock()
        self.failed = False

    def _directory_fd(self):
        # Every descendant of the fixed dev home must be a private, owned
        # directory. No chmod of an existing target, no symlink traversal.
        # Same W2 scoping as _gate: only the trace directory's own subtree
        # up to and including the dev home is checked for .git/.cortex.
        dev = self.directory.parents[3]
        if any((p / ".git").exists() or p.name == ".cortex"
               for p in _ancestors_up_to(self.directory, dev)):
            raise ValueError("unsafe_trace_directory")
        if dev.parent.resolve() != dev.parent:
            raise ValueError("unsafe_trace_directory")
        fd = os.open(dev.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            for name in (dev.name, "cortex-xdg", "state", "cortex", "wire-traces"):
                try:
                    os.mkdir(name, mode=0o700, dir_fd=fd)
                except FileExistsError:
                    pass
                child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                os.close(fd)
                fd = child
                info = os.fstat(fd)
                if (info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700):
                    raise ValueError("unsafe_trace_directory")
            return fd
        except BaseException:
            os.close(fd)
            raise

    def _file(self, fd, name):
        opened = os.open(name, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW
                         | os.O_NONBLOCK | os.O_CLOEXEC, 0o600, dir_fd=fd)
        info = os.fstat(opened)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600
                or info.st_size > self.max_bytes or (name == "trace.lock" and info.st_size != 0)):
            os.close(opened)
            raise ValueError("unsafe_trace_file")
        return opened

    def learn_environment(self, environ):
        """Retain credential literals in memory only, before an engine starts."""
        if self.failed:
            return
        try:
            with self._lock:
                for key, value in environ.items():
                    if _secret_key(key):
                        self.redactor._remember(value)
                if self.redactor.overflow:
                    raise ValueError("redaction_registry_exhausted")
        except Exception:
            self.failed = True
            _diagnostic("cortex: dev wire trace stopped (environment redaction failure)")

    def record(self, channel, direction, payload, **identity):
        if self.failed:
            return
        try:
            with self._lock:
                row = dict(timestamp=datetime.now(timezone.utc).isoformat(),
                           monotonic_ns=time.monotonic_ns(), pid=os.getpid(),
                           channel=channel, direction=direction,
                           session_id=None, task_id=None, assignment_ref=None,
                           attempt_id=None, channel_id=None)
                row.update({key: value for key, value in identity.items()
                            if key in row or key in ("connection_id", "engine_session_id", "command_ref")})
                row["payload"] = payload
                body = (json.dumps(self.redactor.redact(row), ensure_ascii=True) + "\n").encode()
                if len(body) > self.max_bytes:
                    row["payload"] = {"trace_error": "record_exceeds_rotation_size",
                                      "record_bytes": len(body)}
                    for key, value in list(row.items()):
                        if isinstance(value, str) and len(value) > 512:
                            row[key] = None
                            row["payload"]["identity_exceeds_limit"] = True
                    body = (json.dumps(self.redactor.redact(row), ensure_ascii=True) + "\n").encode()
                directory_fd = self._directory_fd()
                try:
                    lock_fd = self._file(directory_fd, "trace.lock")
                    try:
                        fcntl.flock(lock_fd, fcntl.LOCK_EX)
                        file_fd = self._file(directory_fd, "trace.jsonl")
                        if os.fstat(file_fd).st_size + len(body) > self.max_bytes:
                            os.close(file_fd)
                            # Validate every existing rotation target before renaming.
                            for index in range(1, self.file_count):
                                name = f"trace.{index}.jsonl"
                                try:
                                    info = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
                                except FileNotFoundError:
                                    continue
                                if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                                        or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600
                                        or info.st_size > self.max_bytes):
                                    raise ValueError("unsafe_trace_file")
                            for index in range(self.file_count - 1, 0, -1):
                                source = "trace.jsonl" if index == 1 else f"trace.{index - 1}.jsonl"
                                try:
                                    os.rename(source, f"trace.{index}.jsonl",
                                              src_dir_fd=directory_fd, dst_dir_fd=directory_fd)
                                except FileNotFoundError:
                                    pass
                            file_fd = self._file(directory_fd, "trace.jsonl")
                        try:
                            view = memoryview(body)
                            while view:
                                count = os.write(file_fd, view)
                                if count <= 0:
                                    raise OSError("trace_write_failed")
                                view = view[count:]
                        finally:
                            os.close(file_fd)
                    finally:
                        os.close(lock_fd)
                finally:
                    os.close(directory_fd)
        except Exception:
            self.failed = True
            # A failed passive trace cannot alter protocol/storage outcomes.
            _diagnostic("cortex: dev wire trace stopped (unsafe path or write/redaction failure)")


_tracer = None


def configure(environ=None):
    global _tracer
    environ = os.environ if environ is None else environ
    _tracer = None
    if environ.get(OPT_IN) != "1":
        return None
    try:
        directory = _gate(environ, Path(__file__).resolve().parents[3])
        _tracer = WireTrace(directory, environ)
    except Exception:
        _diagnostic("cortex: dev wire trace refused (requires guarded dev source/state)")
    return _tracer


def get_trace():
    return _tracer


configure()
