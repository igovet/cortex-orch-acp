"""Stopping a command together with everything it started.

``killpg`` reaches only the command's own process group. A child that puts
itself in a new group is missed: Playwright starts its ``webServer`` detached,
and after a timed-out test run that server kept its port, so the next run and
the runtime's final verification failed with "port already used" (observed
2026-10-22). The tree is read from ``/proc`` before anything is killed, while
every descendant is still attached to it.
"""
from __future__ import annotations

import os
import signal


def descendants(pid: int) -> list[int]:
    """Every live descendant of ``pid``, parents before children."""
    found: list[int] = []
    pending = [pid]
    seen = {pid}
    while pending:
        current = pending.pop(0)
        try:
            tasks = os.listdir(f'/proc/{current}/task')
        except OSError:
            continue
        for task in tasks:
            try:
                with open(f'/proc/{current}/task/{task}/children') as handle:
                    children = [int(value) for value in handle.read().split()]
            except (OSError, ValueError):
                continue
            for child in children:
                if child not in seen:
                    seen.add(child)
                    found.append(child)
                    pending.append(child)
    return found


def kill_process_tree(pid: int) -> None:
    """SIGKILL ``pid``'s process group, every descendant and each process
    group a descendant leads; never this runtime's own group."""
    tree = descendants(pid)
    own = os.getpgrp()
    groups = {pid}
    for member in tree:
        try:
            groups.add(os.getpgid(member))
        except OSError:
            pass
    for group in groups:
        if group > 1 and group != own:
            try:
                os.killpg(group, signal.SIGKILL)
            except OSError:
                pass
    for member in (pid, *tree):
        try:
            os.kill(member, signal.SIGKILL)
        except OSError:
            pass
