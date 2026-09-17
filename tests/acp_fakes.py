"""Test-only ACP runtime fakes.

``FakeRuntime`` is explicitly NOT Cortex orchestration: it streams a
scripted list of text chunks and returns a scripted
:class:`~cortex_runtime.portable.engine_contract.StopReason`. It exists only
to let protocol/transport tests exercise streaming updates and cancellation
without claiming to be the finished orchestration product (see
``cortex_runtime.acp.runtime.UnavailableRuntime`` for the real production
boundary, and T009's "do not mistake a protocol echo fixture for Cortex
orchestration" non-negotiable).

This module is intentionally under ``tests/`` and is never imported by any
production module (see ``cortex_runtime.acp.agent``, which only imports
``cortex_runtime.acp.runtime.UnavailableRuntime`` by default).
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

from cortex_runtime.portable.engine_contract import StopReason

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "plugins" / "cortex" / "scripts"
TESTS_DIR = ROOT / "tests"

# T010 (ACP configuration UI) added connection/catalog/user-default reads to
# the agent's session/new and session/set_config_option handling, which by
# default resolve under $XDG_CONFIG_HOME/$XDG_STATE_HOME/$HOME. Every
# spawned agent subprocess below defaults those three to a *fresh*
# process-private tmp directory (outside this repo, never reused across
# spawns) so that a test that does not care about connections/catalog state
# never reads or creates anything under the real user's actual
# ~/.config/cortex or ~/.local/state/cortex -- see the T010 task card's "no
# real ~/.config/cortex or ~/.local/state/cortex" invariant. A caller that
# *does* care (e.g. a config-options test) still passes its own
# XDG_CONFIG_HOME/XDG_STATE_HOME/HOME via ``env=`` and that value always
# wins (see the ``full_env.update`` below, applied after these defaults).


def _isolated_home_env() -> dict[str, str]:
    home = tempfile.mkdtemp(prefix="cortex-acp-fake-home-")
    return {"HOME": home, "XDG_CONFIG_HOME": os.path.join(home, "config"),
            "XDG_STATE_HOME": os.path.join(home, "state")}


class FakeRuntime:
    """Scripted, clearly-labeled stand-in for real orchestration.

    ``chunks`` is an iterable of text fragments emitted as
    ``agent_message_chunk`` updates via ``emit_update``. ``stop_reason``
    (default :class:`StopReason.END_TURN`) is returned once every chunk has
    been emitted, unless ``cancel_event`` is set first (checked between each
    chunk), in which case :class:`StopReason.CANCELLED` is returned instead.
    """

    def __init__(self, chunks=("Hello", " from", " FakeRuntime (test-only, not real orchestration)."),
                 stop_reason: StopReason = StopReason.END_TURN, delay=0.0):
        self.chunks = tuple(chunks)
        self.stop_reason = stop_reason
        self.delay = delay
        self.cancelled_sessions = []
        self._lock = threading.Lock()

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        for chunk in self.chunks:
            if cancel_event.is_set():
                return StopReason.CANCELLED
            if not emit_update(chunk):
                return StopReason.CANCELLED
            if self.delay:
                cancel_event.wait(self.delay)
        if cancel_event.is_set():
            return StopReason.CANCELLED
        return self.stop_reason

    def cancel(self, session_id):
        with self._lock:
            self.cancelled_sessions.append(session_id)


class FloodingRuntime:
    """Test-only runtime that emits updates as fast as possible, unbounded.

    Used to exercise the transport's bounded outbound queue / slow-consumer
    policy: a client that never reads stdout must not cause unbounded
    memory growth or a deadlock (see tests/test_acp_transport.py).
    """

    def __init__(self, total=100_000):
        self.total = total

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        for index in range(self.total):
            if cancel_event.is_set():
                return StopReason.CANCELLED
            if not emit_update(f"chunk-{index}"):
                return StopReason.CANCELLED
        return StopReason.END_TURN

    def cancel(self, session_id):
        return None


def spawn_fake_agent(*, chunks=("Hello", " world"), stop_reason="end_turn", delay=0.0, env=None):
    """Start ``cortex_runtime.acp.agent.main`` in a subprocess with a
    :class:`FakeRuntime` injected via an explicit, test-only ``-c`` entry
    hook. Production's ``cortex_acp.py`` never does this (see
    ``cortex_runtime.acp.agent.main``'s docstring): only a test imports
    ``acp_fakes`` and passes ``runtime=`` explicitly.
    """
    code = (
        f"import sys\n"
        f"sys.path.insert(0, {str(SCRIPTS)!r})\n"
        f"sys.path.insert(0, {str(TESTS_DIR)!r})\n"
        f"from acp_fakes import FakeRuntime\n"
        f"from cortex_runtime.portable.engine_contract import StopReason\n"
        f"from cortex_runtime.acp.agent import main\n"
        f"main(runtime=FakeRuntime(chunks={tuple(chunks)!r}, stop_reason=StopReason({stop_reason!r}), delay={delay!r}))\n"
    )
    full_env = dict(os.environ)
    full_env.update(_isolated_home_env())
    if env:
        full_env.update(env)
    return subprocess.Popen(
        [sys.executable, "-B", "-c", code],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, env=full_env)


def spawn_flooding_agent(*, total=200_000, env=None):
    """Start the agent with a :class:`FloodingRuntime` (slow-consumer tests)."""
    code = (
        f"import sys\n"
        f"sys.path.insert(0, {str(SCRIPTS)!r})\n"
        f"sys.path.insert(0, {str(TESTS_DIR)!r})\n"
        f"from acp_fakes import FloodingRuntime\n"
        f"from cortex_runtime.acp.agent import main\n"
        f"main(runtime=FloodingRuntime(total={int(total)}))\n"
    )
    full_env = dict(os.environ)
    full_env.update(_isolated_home_env())
    if env:
        full_env.update(env)
    return subprocess.Popen(
        [sys.executable, "-B", "-c", code],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, env=full_env)


def spawn_production_agent(*, env=None):
    """Start the real ``cortex_acp.py`` entry point (no fake runtime)."""
    full_env = dict(os.environ)
    full_env.update(_isolated_home_env())
    if env:
        full_env.update(env)
    return subprocess.Popen(
        [sys.executable, "-B", str(SCRIPTS / "cortex_acp.py")],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, env=full_env)
