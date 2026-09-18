"""T023: OS-level process identity capture/verification for cross-process
engine-ownership reconciliation.

An :class:`~.session_manager.EngineSessionManager` (via
``delegation.live_adapters.EngineSessionManagerAdapter``) launches its
engine subprocess with ``start_new_session=True`` (see
``engine.process.EngineProcess``), so its OS pid is also its own process
group id. That pid alone is not a durable identity across a Cortex process
restart: pids are reused by the kernel, so "pid N is running" does not by
itself mean "pid N is *this* attempt's engine process". This module pairs
the pid with two additional, cheaply-observable Linux facts that together
make pid reuse detectable:

- the process's own start time (``/proc/<pid>/stat`` field 22, in clock
  ticks since boot) -- a different process that happens to reuse the same
  pid almost certainly has a different start time;
- the current boot id (``/proc/sys/kernel/random/boot_id``) -- a reboot
  between the crash and the restart makes every previously-recorded pid
  meaningless regardless of any (coincidental) start-time match.

Every read here is best-effort and side-effect-free (no signals sent, see
:mod:`recovery.registry`/``delegation.live_adapters`` for the actual
terminate path). Any read that cannot be performed at all (missing
``/proc``, permission denied, malformed content, not Linux) returns
``None`` from the capture/read helpers -- callers treat that as
"indeterminate", never as a confirmed absence or presence.
"""
from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class ProcessIdentity:
    pid: int
    pgid: int
    start_ticks: str
    boot_id: str


def read_boot_id() -> str | None:
    try:
        with open("/proc/sys/kernel/random/boot_id", "r") as handle:
            value = handle.read().strip()
    except OSError:
        return None
    return value or None


def read_start_ticks(pid: int) -> str | None:
    """Field 22 (``starttime``) of ``/proc/<pid>/stat``, as a decimal
    string. The process name field (2nd, parenthesized) may itself contain
    spaces/parentheses, so this splits on the *last* ``)`` rather than
    naive whitespace splitting (documented ``proc(5)`` parsing hazard)."""
    try:
        with open(f"/proc/{pid}/stat", "r") as handle:
            content = handle.read()
    except OSError:
        return None
    close = content.rfind(")")
    if close == -1:
        return None
    rest = content[close + 1:].split()
    # rest[0] is field 3 (state); field 22 is rest[19].
    if len(rest) < 20:
        return None
    ticks = rest[19]
    return ticks if ticks.isdigit() else None


def capture(pid: int, pgid: int | None = None) -> ProcessIdentity | None:
    """Capture this freshly-launched process's own identity, for durable
    recording at launch time (see ``recovery.registry.record_process_launch``).
    ``None`` only if ``/proc`` is unreadable right after a successful
    ``subprocess.Popen`` -- treated as "record nothing; a later lookup for
    this attempt is 'unknown', never 'absent'" by the caller."""
    boot_id = read_boot_id()
    start_ticks = read_start_ticks(pid)
    if boot_id is None or start_ticks is None:
        return None
    return ProcessIdentity(pid=pid, pgid=pgid if pgid is not None else pid, start_ticks=start_ticks, boot_id=boot_id)


def probe(pid: int, start_ticks: str, boot_id: str) -> str:
    """Return ``"alive"``, ``"absent"``, or ``"unknown"`` for a previously
    recorded identity, checked from a (possibly different) current process.

    - A different current boot id means the machine rebooted since this
      identity was recorded: every pid from before a reboot is gone --
      ``"absent"`` (this is a safe, not merely convenient, conclusion: no
      process can survive a reboot).
    - No such pid at all (``/proc/<pid>`` missing) -- ``"absent"``.
    - A pid that exists but whose own start time no longer matches --
      ``"absent"`` (the kernel has reused this pid for an unrelated
      process; the originally recorded process has already exited).
    - A pid that exists with a matching start time -- ``"alive"``.
    - Anything unreadable (permission denied, malformed, not Linux) --
      ``"unknown"``: never guessed either way.
    """
    current_boot_id = read_boot_id()
    if current_boot_id is None:
        return "unknown"
    if current_boot_id != boot_id:
        return "absent"
    current_start_ticks = read_start_ticks(pid)
    if current_start_ticks is None:
        import os

        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return "absent"
        except PermissionError:
            return "unknown"
        except OSError:
            return "unknown"
        # The pid exists (kill(pid, 0) succeeded) but /proc/<pid>/stat could
        # not be read (a container/permission boundary) -- its identity
        # cannot be verified, so a same-pid coincidence is not trusted.
        return "unknown"
    return "alive" if current_start_ticks == start_ticks else "absent"
