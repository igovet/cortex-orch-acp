#!/usr/bin/env python3
"""Opt-in periodic login keepalive for native Claude Code / Codex CLI accounts.

This script is **not** part of the Cortex runtime and the runtime never
invokes it. It is a standalone, manually-scheduled operational tool an owner
may run (for example from a `systemd --user` timer; see
``scripts/systemd/``) so a native CLI session is less likely to expire
between uses.

For each supported engine it runs, at most, the one zero-cost
refresh-forcing command a prior investigation established does not make a
paid model call and does not require inventing capability the installed CLI
does not actually expose. As of this writing no zero-cost refresh-forcing
command is **proven** for either the installed Claude Code CLI or the
installed Codex CLI: their ``status``/``login status`` help text describes
displaying status, never that it refreshes or persists OAuth credentials,
and no command was executed to find out because doing so could have cost
real model usage or produced unproven side effects. Both engines are
therefore reported ``unsupported`` until a later investigation records a
proven command; this script must never guess one and must never fall back
to a paid call (for example a real model prompt) to "find out".

Guarantees this script holds even once a real command is configured:

- It never copies, derives, reads or prints any credential/token value. It
  only runs the configured command against the user's existing real native
  config (``CLAUDE_CONFIG_DIR``/``CODEX_HOME`` or their defaults) and looks
  at the process **exit status**.
- It never logs or prints the command's stdout/stderr bodies (they are
  discarded, not merely hidden from the default output), since that output
  could itself contain account details.
- It reports only: engine name, a sanitized status
  (``ok`` / ``needs-login`` / ``unsupported`` / ``error``), the bare
  numeric exit code (or ``null`` when the command never ran), and a short
  static detail string -- never command output.
- The overall process exit code is non-zero exactly when at least one
  configured engine reports ``needs-login`` or ``error``, so the script is
  usable as a `systemd` service `ExecStart` health signal.
"""
from __future__ import annotations

import argparse
import dataclasses
import enum
import json
import subprocess
import sys
from typing import Callable, Optional, Sequence


class Status(str, enum.Enum):
    """Sanitized per-engine outcome. Never carries command output."""

    OK = "ok"
    NEEDS_LOGIN = "needs-login"
    UNSUPPORTED = "unsupported"
    ERROR = "error"


@dataclasses.dataclass(frozen=True)
class EngineSpec:
    """One engine's keepalive configuration.

    ``command`` is ``None`` unless a later investigation records a proven
    zero-cost refresh-forcing command for this engine; until then the
    engine is reported ``unsupported`` and nothing is executed for it.
    """

    name: str
    command: Optional[tuple[str, ...]]
    unsupported_reason: Optional[str] = None


# No proven zero-cost refresh-forcing command is established for either
# installed CLI (installed-CLI --help text describes status/login display
# only, never a proven refresh or persistence). Flip
# ``command`` to the proven argv tuple only after a reviewed, source- or
# vendor-backed investigation establishes that the command: (a) performs an
# actual refresh/persistence of the native login, (b) costs no model usage,
# and (c) is safe to run concurrently with other Cortex/Claude/Codex
# sessions using the same native account.
ENGINES: tuple[EngineSpec, ...] = (
    EngineSpec(
        name="claude",
        command=None,
        unsupported_reason=(
            "no proven zero-cost refresh-forcing command for Claude Code CLI "
            "('claude auth status'/'claude setup-token' help describe status/"
            "token setup, not a proven OAuth refresh; a usage prompt was not "
            "executed because it could cost usage)"
        ),
    ),
    EngineSpec(
        name="codex",
        command=None,
        unsupported_reason=(
            "no proven zero-cost refresh-forcing command for Codex CLI "
            "('codex login status' help describes only displaying status, "
            "not a proven credential refresh/persistence)"
        ),
    ),
)

_DEFAULT_TIMEOUT_SECONDS = 30


@dataclasses.dataclass(frozen=True)
class EngineResult:
    engine: str
    status: Status
    exit_code: Optional[int]
    detail: str

    def to_dict(self) -> dict:
        return {
            "engine": self.engine,
            "status": self.status.value,
            "exit_code": self.exit_code,
            "detail": self.detail,
        }


Runner = Callable[..., "subprocess.CompletedProcess[bytes]"]


def run_engine(
    spec: EngineSpec,
    *,
    runner: Runner = subprocess.run,
    timeout_seconds: int = _DEFAULT_TIMEOUT_SECONDS,
) -> EngineResult:
    """Run one engine's configured keepalive command, or report unsupported.

    Never passes command output back to the caller: only the exit code is
    observed, and stdout/stderr are discarded (not merely captured).
    """
    if spec.command is None:
        return EngineResult(
            engine=spec.name,
            status=Status.UNSUPPORTED,
            exit_code=None,
            detail=spec.unsupported_reason or "no command configured for this engine",
        )
    try:
        completed = runner(
            list(spec.command),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            stdin=subprocess.DEVNULL,
            timeout=timeout_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return EngineResult(
            engine=spec.name,
            status=Status.ERROR,
            exit_code=None,
            detail=f"command timed out after {timeout_seconds}s",
        )
    except OSError as exc:
        return EngineResult(
            engine=spec.name,
            status=Status.ERROR,
            exit_code=None,
            detail=f"could not invoke command ({type(exc).__name__})",
        )
    if completed.returncode == 0:
        status = Status.OK
        detail = "command exited 0"
    else:
        status = Status.NEEDS_LOGIN
        detail = f"command exited {completed.returncode}"
    return EngineResult(
        engine=spec.name,
        status=status,
        exit_code=completed.returncode,
        detail=detail,
    )


def run_all(
    engines: Sequence[EngineSpec] = ENGINES,
    *,
    runner: Runner = subprocess.run,
    timeout_seconds: int = _DEFAULT_TIMEOUT_SECONDS,
) -> list[EngineResult]:
    return [
        run_engine(spec, runner=runner, timeout_seconds=timeout_seconds)
        for spec in engines
    ]


def overall_exit_code(results: Sequence[EngineResult]) -> int:
    """Non-zero exactly when a configured engine needs login or errored.

    ``unsupported`` is a skip, not a failure: it never makes the overall
    result non-zero on its own.
    """
    for result in results:
        if result.status in (Status.NEEDS_LOGIN, Status.ERROR):
            return 1
    return 0


def render(results: Sequence[EngineResult]) -> str:
    payload = {"results": [result.to_dict() for result in results]}
    return json.dumps(payload, indent=2, sort_keys=True)


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        prog="cortex_auth_keepalive.py",
        description=(
            "Opt-in periodic login keepalive check for native Claude Code "
            "and Codex CLI accounts. Never copies credentials, never prints "
            "token values, never logs command output; prints only a "
            "sanitized per-engine engine/status/exit_code/detail JSON "
            "report. Exits non-zero when an engine needs login or errored."
        ),
    )
    parser.parse_args(argv)
    results = run_all()
    print(render(results))
    return overall_exit_code(results)


if __name__ == "__main__":
    raise SystemExit(main())
