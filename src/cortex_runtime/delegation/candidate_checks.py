"""Bounded host-side checks for an exact staged candidate.

This is intentionally a small execution primitive.  It does not select
checks, interpret their meaning, or expose a model-supplied command result as
proof.  The process result is captured by this module; the semantic
assessment remains an explicit, separate input to ``record_check_evidence``.
"""
from __future__ import annotations

import hashlib
import json
import os
import select
import signal
from pathlib import Path
import re
import subprocess
import time
from typing import Iterable

from .check_evidence import record_check_evidence
from .candidates import _digest_manifest, _manifest, get_candidate


MAX_ARGV = 64
MAX_ARG_BYTES = 4096
MAX_OUTPUT_BYTES = 2 * 1024 * 1024
MAX_OUTPUT_PREVIEW_CHARS = 4096
DEFAULT_TIMEOUT_SECONDS = 300.0


class CandidateCheckError(RuntimeError):
    """A candidate check cannot be run or observed consistently."""

    _CODE_RE = re.compile(r"^[a-z0-9_]+(?::[a-z0-9_]+)*$")

    @property
    def code(self) -> str:
        """Return the bounded, non-sensitive reason code for this failure."""
        value = self.args[0] if self.args else "candidate_check_failed"
        if isinstance(value, str) and self._CODE_RE.fullmatch(value):
            return value
        return "candidate_check_failed"


def _digest_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


_SECRET_RE = re.compile(
    r"(?i)(\b(?:api[_-]?key|access[_-]?token|auth(?:orization)?|token|password|passwd|secret)\b\s*[:=]\s*)([^\s,;]+)"
)
_ABSOLUTE_PATH_RE = re.compile(r"(?<![A-Za-z0-9_])/(?:[^\s\x00]+)")


def _safe_preview(data: bytes) -> str:
    """Return a small diagnostic preview without common secret/path leakage."""
    text = data[:MAX_OUTPUT_PREVIEW_CHARS * 4].decode("utf-8", errors="replace")
    text = _SECRET_RE.sub(r"\1<redacted>", text)
    text = _ABSOLUTE_PATH_RE.sub("<path>", text)
    return text[:MAX_OUTPUT_PREVIEW_CHARS]


def _validate_argv(argv: Iterable[str]) -> tuple[str, ...]:
    items = tuple(argv)
    if not items or len(items) > MAX_ARGV:
        raise CandidateCheckError("check_argv_limit")
    if any(not isinstance(item, str) or not item or "\x00" in item for item in items):
        raise CandidateCheckError("check_argv_invalid")
    if sum(len(item.encode("utf-8")) for item in items) > MAX_ARG_BYTES:
        raise CandidateCheckError("check_argv_too_large")
    # Shell interpretation would make the observed command ambiguous and can
    # escape the intended argv contract.  A command must be a direct process.
    executable = Path(items[0]).name
    if executable in {"sh", "bash", "dash", "zsh", "fish", "env"} or items[0] in {"-c", "--command"}:
        raise CandidateCheckError("check_shell_command_forbidden")
    if any(item in {"-c", "--command", "-exec", "--exec"} for item in items[1:]):
        raise CandidateCheckError("check_shell_command_forbidden")
    return items


def _execute_bounded(command: tuple[str, ...], *, cwd: str, env: dict[str, str], timeout: float) -> dict:
    """Execute a process group while retaining at most MAX_OUTPUT_BYTES."""
    try:
        process = subprocess.Popen(
            command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise CandidateCheckError(f"check_execution_failed:{type(error).__name__}") from error
    if process.stdout is None:
        raise CandidateCheckError("check_output_unavailable")
    output = bytearray()
    truncated = False
    total_output = 0
    deadline = time.monotonic() + timeout
    try:
        while True:
            remaining = max(0.0, deadline - time.monotonic())
            if remaining == 0.0:
                raise TimeoutError
            readable, _, _ = select.select([process.stdout], [], [], min(remaining, 0.25))
            if readable:
                chunk = os.read(process.stdout.fileno(), 65536)
                if chunk:
                    total_output += len(chunk)
                    if len(output) < MAX_OUTPUT_BYTES:
                        output.extend(chunk[:MAX_OUTPUT_BYTES - len(output)])
                    if total_output > MAX_OUTPUT_BYTES:
                        truncated = True
                else:
                    process.stdout.close()
                    break
            if process.poll() is not None and not readable:
                # A process can exit while the pipe still has buffered data;
                # continue until EOF so the digest covers all available output.
                continue
        returncode = process.wait(timeout=max(0.1, min(2.0, max(0.0, deadline - time.monotonic()))))
    except TimeoutError:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError as error:
            raise CandidateCheckError("check_outcome_unknown:process_group_kill_failed") from error
        try:
            process.wait(timeout=2.0)
        except subprocess.TimeoutExpired as error:
            raise CandidateCheckError("check_outcome_unknown:process_reap_failed") from error
        return {"status": "timeout", "exit_code": None,
                "output_digest": _digest_bytes(bytes(output)), "output_bytes": len(output),
                "output_truncated": truncated, "output_preview": _safe_preview(bytes(output))}
    except (OSError, subprocess.SubprocessError) as error:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            process.wait(timeout=2.0)
        except Exception:
            pass
        raise CandidateCheckError("check_outcome_unknown:execution_read_failed") from error
    return {"status": "truncated" if truncated else "completed", "exit_code": returncode,
            "output_digest": _digest_bytes(bytes(output)), "output_bytes": len(output),
            "output_truncated": truncated, "output_preview": _safe_preview(bytes(output))}


def run_candidate_check(
    store,
    task_id: str,
    attempt_id: str,
    candidate_id: str,
    candidate_content_digest: str,
    argv: Iterable[str],
    *,
    environment: dict[str, str] | None = None,
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
    assessment: dict | None = None,
    requirement_bindings: dict | None = None,
    checked_files: Iterable[str] = (),
    idempotency_key: str | None = None,
) -> dict:
    """Run a direct argv against an exact candidate and optionally persist it.

    The returned ``observed_result`` is produced from the host subprocess,
    including exit code, timeout state, and output digest.  A record is only
    written when ``assessment`` is supplied explicitly; it is never inferred
    from process completion or exit code.
    """
    candidate = get_candidate(store, candidate_id)
    if candidate is None or candidate.get("task_id") != task_id:
        raise CandidateCheckError("candidate_task_mismatch")
    if candidate.get("status") != "ready":
        raise CandidateCheckError("candidate_not_ready")
    if candidate.get("content_digest") != candidate_content_digest:
        raise CandidateCheckError("candidate_digest_mismatch")
    stage = Path(candidate.get("stage_path", ""))
    project = Path(candidate.get("project_root", ""))
    cortex = project / ".cortex"
    if not stage.is_dir() or not project.is_dir() or not cortex.is_dir():
        raise CandidateCheckError("candidate_paths_unavailable")
    if not stage.is_absolute() or not project.is_absolute() or not cortex.is_absolute():
        raise CandidateCheckError("candidate_paths_not_absolute")
    command = _validate_argv(argv)
    if type(timeout_seconds) not in (int, float) or not 0 < timeout_seconds <= 1800:
        raise CandidateCheckError("check_timeout_invalid")
    try:
        before_digest = _digest_manifest(_manifest(str(stage)))
    except Exception as error:
        raise CandidateCheckError("candidate_manifest_unavailable") from error
    if before_digest != candidate_content_digest:
        raise CandidateCheckError("candidate_digest_mismatch")
    safe_env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LC_ALL": "C", "LANG": "C"}
    for key, value in (environment or {}).items():
        if not isinstance(key, str) or not key or not key.isidentifier() or not isinstance(value, str):
            raise CandidateCheckError("check_environment_invalid")
        safe_env[key] = value
    # Python checks commonly create __pycache__ as a side effect.  The staged
    # candidate is content-addressed, so make the default check environment
    # explicitly bytecode-free; callers cannot opt out of this invariant.
    safe_env["PYTHONDONTWRITEBYTECODE"] = "1"
    # The native check runs with the user's ordinary OS permissions. The
    # before/after manifest detects changes to the staged candidate, but is
    # not a filesystem or network containment boundary for the check.
    environment_identity = {"variables": safe_env, "execution": "direct-user-v2"}
    observed = _execute_bounded(command, cwd=str(stage), env=safe_env, timeout=timeout_seconds)
    try:
        after_digest = _digest_manifest(_manifest(str(stage)))
    except Exception as error:
        raise CandidateCheckError("candidate_manifest_unavailable_after_check") from error
    if after_digest != candidate_content_digest or after_digest != before_digest:
        raise CandidateCheckError("candidate_changed_during_check")
    if assessment is not None and assessment.get("status") == "pass" and (
            observed["status"] != "completed" or observed["exit_code"] != 0 or observed["output_truncated"]):
        raise CandidateCheckError("check_observation_not_successful")
    result = {"candidate_id": candidate_id, "candidate_content_digest": candidate_content_digest,
              "attempt_id": attempt_id, "argv": list(command), "environment": environment_identity,
              "observed_result": observed}
    if assessment is not None:
        result["evidence"] = record_check_evidence(
            store, task_id, attempt_id, candidate_id, candidate_content_digest,
            requirement_bindings or {}, environment_identity, {"argv": list(command)}, observed,
            assessment, checked_files=checked_files, idempotency_key=idempotency_key,
        )
    return result


__all__ = ["CandidateCheckError", "run_candidate_check"]
