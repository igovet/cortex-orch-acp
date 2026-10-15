"""Where Cortex keeps its temporary files: ``/var/tmp``, on disk.

On many hosts ``/tmp`` is a tmpfs: its files live in RAM and, under
pressure, in swap. Engine jails, verification scratch, git observations and
the test suite create gigabytes there, so a RAM-backed ``/tmp`` filled up
(live, 2026-10-15: 13 of 16 GB, then ``disk I/O error``) and pushed the rest
of the machine into swap. Every Cortex entry point calls
:func:`prefer_disk_tmp` first, which points ``TMPDIR`` (and :mod:`tempfile`)
at ``/var/tmp``. Only an unset ``TMPDIR`` or the bare RAM root ``/tmp`` is
replaced: an explicit directory (a sandbox's own scratch, a test run's own
root, a caller's choice) is kept. Child processes inherit the setting.
"""
from __future__ import annotations

import os
import tempfile
from typing import MutableMapping

DISK_TMP = "/var/tmp"


def prefer_disk_tmp(environ: MutableMapping[str, str] | None = None) -> str:
    """Point ``TMPDIR`` at :data:`DISK_TMP` unless it already names an
    explicit directory other than ``/tmp`` itself; return the temporary
    root now in effect."""
    env = os.environ if environ is None else environ
    current = env.get("TMPDIR")
    if current and os.path.isdir(current) and os.path.realpath(current) != "/tmp":
        chosen = current
    elif os.path.isdir(DISK_TMP) and os.access(DISK_TMP, os.W_OK | os.X_OK):
        chosen = DISK_TMP
    else:
        # No writable /var/tmp (a minimal container): the system default.
        return tempfile.gettempdir()
    env["TMPDIR"] = chosen
    if environ is None:
        tempfile.tempdir = chosen
    return chosen
