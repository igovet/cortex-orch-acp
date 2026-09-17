"""Bounded stderr ring log and JSONL observation log for one engine process.

Both logs live under
``$XDG_STATE_HOME/cortex/engine-logs/<engine_instance_id>/`` (0700
directories via ``connections.registry.ensure_private_dir``, 0600 files).
Neither ever carries a prompt body or a secret value: the observation log
records only lifecycle transitions and method names (see
``session_manager``/``mcp_bridge`` -- the channel secret is only ever placed
into the launched engine process's *environment*, never written here).

T029: this is the single redaction choke point for both sinks -- every
``observe()`` field and every raw stderr line passes through
``observability.redact`` before it ever reaches disk (a launched engine's
own stderr is untrusted text and may itself contain a credential-shaped
substring, e.g. echoed from its own environment or a misconfigured
argument).
"""
from __future__ import annotations

import json
import os
import time
from pathlib import Path

from ..connections import registry as connection_registry
from ..observability import redact

MAX_LOG_BYTES = 256 * 1024


def engine_log_dir(engine_instance_id: str, environ: dict[str, str] | None) -> Path:
    root = connection_registry.state_root(environ)
    log_dir = root / "engine-logs" / engine_instance_id
    connection_registry.ensure_private_dir(log_dir, root=root, environ=environ)
    return log_dir


class BoundedLogFile:
    """Append-only file capped at ``MAX_LOG_BYTES``.

    Writes past the cap are silently dropped rather than growing without
    bound; a write is never partially applied mid-line beyond the cap
    (the tail simply stops).
    """

    def __init__(self, path: Path):
        self._path = path
        flags = os.O_WRONLY | os.O_APPEND | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(path, flags, 0o600)
        os.close(fd)

    def append_line(self, text: str) -> None:
        data = (text.rstrip("\n") + "\n").encode("utf-8", errors="replace")
        flags = os.O_WRONLY | os.O_APPEND | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(self._path, flags)
        try:
            size = os.fstat(fd).st_size
            if size < MAX_LOG_BYTES:
                os.write(fd, data[: max(0, MAX_LOG_BYTES - size)])
        finally:
            os.close(fd)


class EngineLogs:
    """Owns one engine instance's stderr ring log and observation log."""

    def __init__(self, engine_instance_id: str, environ: dict[str, str] | None):
        directory = engine_log_dir(engine_instance_id, environ)
        self.stderr = BoundedLogFile(directory / "stderr.log")
        self.observations = BoundedLogFile(directory / "observations.jsonl")
        self.engine_instance_id = engine_instance_id
        self.directory = directory

    def observe(self, event: str, **fields: object) -> None:
        row = dict(time_ns=time.time_ns(), engine_instance_id=self.engine_instance_id, event=event)
        row.update(redact.scrub_mapping(fields))
        try:
            self.observations.append_line(json.dumps(row, sort_keys=True, ensure_ascii=False, default=str))
        except OSError:
            pass

    def stderr_line(self, text: str) -> None:
        """Append one already-decoded stderr line, redacted (see module
        docstring). The only call site is
        ``engine.process.EngineProcess._drain_stderr``."""
        self.stderr.append_line(redact.scrub_text(text))
