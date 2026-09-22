"""A jailed engine must outlive the turn thread that opened it.

The engine jail's bwrap --die-with-parent is PR_SET_PDEATHSIG, which the
kernel fires when the *spawning thread* exits: a coordinator spawned from a
turn's thread was SIGKILLed as that turn ended (cancel, completion), and the
next message hit a dead session (transport_eof, a reopened session).
"""
import shutil
import subprocess
import threading
import time

import pytest

from cortex_runtime.engine.process import _ENGINE_SPAWNER

BWRAP = shutil.which("bwrap")
ARGV = [BWRAP, "--die-with-parent", "--unshare-pid", "--ro-bind", "/", "/", "--dev", "/dev",
        "--proc", "/proc", "sleep", "30"] if BWRAP else []


def _spawn_from_a_turn_thread(spawn):
    box = {}

    def turn():
        box["proc"] = spawn()
        time.sleep(1)  # the turn outlives bwrap's own PDEATHSIG setup, then ends

    thread = threading.Thread(target=turn)
    thread.start(); thread.join()
    time.sleep(1)
    return box["proc"]


@pytest.mark.skipif(BWRAP is None, reason="bwrap unavailable")
def test_a_jailed_engine_outlives_the_turn_thread_that_opened_it():
    direct = _spawn_from_a_turn_thread(lambda: subprocess.Popen(ARGV, start_new_session=True))
    spawned = _spawn_from_a_turn_thread(
        lambda: _ENGINE_SPAWNER.submit(subprocess.Popen, ARGV, start_new_session=True).result())
    try:
        assert direct.poll() == -9  # the mechanism this guards against
        assert spawned.poll() is None
    finally:
        for proc in (direct, spawned):
            if proc.poll() is None:
                proc.kill()
            proc.wait(timeout=5)
