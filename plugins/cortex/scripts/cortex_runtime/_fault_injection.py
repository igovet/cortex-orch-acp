"""Deterministic fault-injection checkpoints for T019 crash-recovery tests.

Test-only mechanism, never reached in production unless the env var below
is explicitly set (which no packaged/production entry point ever does).
A subprocess test harness sets ``CORTEX_FAULT_INJECTION_POINT`` to the
exact checkpoint name it wants to crash at (see call sites in ``store.py``);
``checkpoint()`` then calls ``os._exit(137)`` immediately -- no Python
cleanup, no ``except``/``finally`` runs, exactly modeling an OS-level kill
(SIGKILL/host crash) between two filesystem/SQLite operations. A no-op
when the env var is unset or does not match the current point.
"""
from __future__ import annotations

import os


def checkpoint(name: str) -> None:
    if os.environ.get("CORTEX_FAULT_INJECTION_POINT") == name:
        os._exit(137)
