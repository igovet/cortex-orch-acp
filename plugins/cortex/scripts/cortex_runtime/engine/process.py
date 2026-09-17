"""Per-connection engine process launch, isolation, and termination.

Launches only from ``ConnectionRecord.launch_command`` -- never a
discovered/guessed executable: a missing or relative ``launch_command`` is
refused before any subprocess is started. The process environment comes
from ``connections.environment.build_process_env`` with
``channel_credentials=None`` (the trusted-channel secret is delivered to
the *MCP server* subprocess the engine itself spawns, per
``mcp_bridge.build_mcp_servers`` -- never placed in this adapter process's
own environment).
"""
from __future__ import annotations

import os
import signal
import subprocess
import threading
import uuid

from ..connections.environment import build_process_env
from ..portable.errors import PortableError, PortableErrorCategory
from . import codex_install
from .logs import EngineLogs

TERMINATE_GRACE_SECONDS = 5.0


def _validation_error(code, remediation, *, field=None):
    return PortableError(
        category=PortableErrorCategory.VALIDATION, code=code, remediation=remediation,
        retry_eligible=False, field=field,
    )


class ProcessObservation:
    STARTING = "starting"
    TRANSPORT_EOF = "transport_eof"
    TIMEOUT_ALIVE = "timeout_alive"
    PROCESS_EXITED = "process_exited"


class EngineProcess:
    """One launched engine subprocess: env isolation, stderr capture, termination.

    ``engine_instance_id`` is a fresh random identifier per launch (never
    reused, never shared): it names this process's private log directory
    and is included in ``configure()``'s evidence_ref format.
    """

    def __init__(self, record, cwd, *, environ=None, install=None):
        """``install`` -- a :class:`~.codex_install.CodexAcpInstall`, or
        ``None`` -- is only ever meaningful for ``record.engine ==
        "codex-acp"``. When supplied it is re-verified fresh (see
        ``codex_install.verify_install``) before anything is launched:
        ``record.launch_command`` must already equal
        ``codex_install.launch_command_for`` of the *verified* install (a
        mismatch is refused, never silently overridden), and the verified
        install's ``bundled_codex_binary`` is pinned into this process's own
        environment as ``CODEX_PATH`` (with ``NO_BROWSER=1``) via
        ``build_process_env``'s ``pinned_engine_vars``. Omitting ``install``
        for a codex-acp connection launches exactly as any other engine
        (no pinned vars, no install verification) -- callers that need the
        pinned-install guarantee must pass it explicitly.
        """
        if record.launch_command is None:
            raise _validation_error(
                "engine_launch_command_missing",
                "This connection has no launch_command; an engine process is never "
                "guessed or discovered by any other means.",
                field="launch_command",
            )
        command = record.launch_command
        if not command[0].startswith("/"):
            raise _validation_error(
                "engine_launch_command_relative",
                "launch_command[0] must be an absolute path; a relative command is refused.",
                field="launch_command",
            )
        if not isinstance(cwd, str) or not cwd.startswith("/"):
            raise _validation_error(
                "engine_cwd_required",
                "An explicit absolute workspace directory is required to launch an engine process.",
                field="cwd",
            )
        pinned_engine_vars = None
        if install is not None:
            if record.engine != "codex-acp":
                raise _validation_error(
                    "engine_install_not_applicable",
                    "An install record is only meaningful for engine 'codex-acp'.",
                    field="install",
                )
            verified_install = codex_install.verify_install(install, environ)
            expected_command = codex_install.launch_command_for(verified_install)
            if tuple(command) != tuple(expected_command):
                raise _validation_error(
                    "engine_launch_command_mismatch",
                    "record.launch_command does not match codex_install.launch_command_for() of the "
                    "verified install; a launch command is never silently substituted.",
                    field="launch_command",
                )
            pinned_engine_vars = {
                "CODEX_PATH": verified_install.bundled_codex_binary,
                "NO_BROWSER": "1",
            }

        base_env = dict(os.environ if environ is None else environ)
        # Pass this connection's own resolution ``environ`` through
        # explicitly (never rely on ``os.environ`` having been
        # monkeypatched by the time this runs): ``build_process_env``
        # re-verifies the connection registry against exactly this mapping
        # (default ``None`` -> real ``os.environ``, for backward
        # compatibility with every other existing caller).
        process_env = build_process_env(
            record, base_env, channel_credentials=None, environ=environ, pinned_engine_vars=pinned_engine_vars,
        )

        self.engine_instance_id = "eirt_" + uuid.uuid4().hex[:12]
        self.logs = EngineLogs(self.engine_instance_id, environ)
        self.state = ProcessObservation.STARTING
        self._exit_code = None
        self._exited_event = threading.Event()
        self.logs.observe(ProcessObservation.STARTING, command=list(command), cwd=cwd)

        self.process = subprocess.Popen(
            list(command),
            cwd=cwd,
            env=process_env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        self.state = "initialized"

        self._stderr_thread = threading.Thread(target=self._drain_stderr, name="engine-stderr", daemon=True)
        self._stderr_thread.start()
        self._wait_thread = threading.Thread(target=self._wait_for_exit, name="engine-wait", daemon=True)
        self._wait_thread.start()

    def _drain_stderr(self) -> None:
        stream = self.process.stderr
        try:
            while True:
                line = stream.readline()
                if not line:
                    break
                try:
                    text = line.decode("utf-8", errors="replace")
                except Exception:
                    text = repr(line)
                self.logs.stderr.append_line(text)
        except (OSError, ValueError):
            pass

    def _wait_for_exit(self) -> None:
        code = self.process.wait()
        self._exit_code = code
        self.state = "process_exited"
        self.logs.observe(ProcessObservation.PROCESS_EXITED, exit_code=code)
        self._exited_event.set()

    def alive(self) -> bool:
        return self.process.poll() is None

    @property
    def exit_code(self):
        return self._exit_code

    def observe_timeout_alive(self) -> None:
        """A call timed out; this does not by itself prove the process is
        dead, so capacity is not released here -- only observed."""
        self.logs.observe(ProcessObservation.TIMEOUT_ALIVE, alive=self.alive())

    def observe_transport_eof(self) -> None:
        self.logs.observe(ProcessObservation.TRANSPORT_EOF, alive=self.alive())

    def wait_exited(self, timeout=None) -> bool:
        """True once the process is *observed* to have actually exited."""
        return self._exited_event.wait(timeout)

    def terminate(self, *, grace: float = TERMINATE_GRACE_SECONDS) -> None:
        """SIGTERM the process group, grace, SIGKILL the process group, reap."""
        if self._exited_event.is_set():
            return
        try:
            self.process.stdin.close()
        except (OSError, ValueError):
            pass
        try:
            os.killpg(os.getpgid(self.process.pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        if self._exited_event.wait(grace):
            return
        try:
            os.killpg(os.getpgid(self.process.pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        self._exited_event.wait(grace)

    def release(self, *, grace: float = TERMINATE_GRACE_SECONDS) -> None:
        """Terminate and reap; capacity is only freed once exit is observed."""
        self.terminate(grace=grace)
        self._exited_event.wait(grace)
        self.state = "released"
