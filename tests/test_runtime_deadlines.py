import threading
import time
from cortex_runtime.orchestration.limits import Deadline


def test_idle_worker_is_interrupted_without_waiting_for_absolute_limit():
    observed=[]
    with Deadline(threading.Event(),10,lambda: observed.append('stopped'),idle_seconds=.05) as deadline:
        assert deadline.wait(1)
        assert deadline.reason=='idle'
    assert observed==['stopped']


def test_activity_does_not_extend_absolute_deadline():
    with Deadline(threading.Event(),.15,idle_seconds=.07) as deadline:
        while not deadline.is_set():
            deadline.touch();time.sleep(.02)
        assert deadline.reason=='deadline'


def test_parent_cancel_and_normal_exit_do_not_fire_timeout_callback():
    parent=threading.Event();observed=[]
    with Deadline(parent,.1,lambda: observed.append('timeout'),idle_seconds=.05) as deadline:
        parent.set();assert deadline.wait(.1)
    time.sleep(.15)
    assert not observed and not deadline.expired.is_set()
