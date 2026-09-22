"""Runtime deadlines, independent of model text and token accounting."""
import threading
import time



class Deadline:
    def __init__(self,parent,seconds,on_timeout=None,*,idle_seconds=None):
        self.parent=parent
        self.expired=threading.Event()
        self.stopped=threading.Event()
        self.cancelled=threading.Event()
        self.on_timeout=on_timeout
        self.idle_seconds=idle_seconds
        self.last_activity=time.monotonic()
        self.absolute=self.last_activity+seconds
        self.reason=None
        self.watcher=threading.Thread(target=self._watch,daemon=True,name='cortex-deadline')

    def set(self):
        self.cancelled.set()

    def touch(self):
        self.last_activity=time.monotonic()

    def _watch(self):
        while not self.stopped.wait(.025):
            if self.parent.is_set() or self.cancelled.is_set(): return
            current=time.monotonic()
            if current>=self.absolute or self.idle_seconds is not None and current-self.last_activity>=self.idle_seconds:
                self.reason='deadline' if current>=self.absolute else 'idle'
                self.expired.set()
                if self.on_timeout: self.on_timeout()
                return

    def __enter__(self):
        self.watcher.start();return self

    def __exit__(self,*args):
        self.stopped.set()
        self.watcher.join(timeout=.1)

    def is_set(self):
        return self.parent.is_set() or self.expired.is_set() or self.cancelled.is_set()

    def wait(self,timeout=None):
        end=None if timeout is None else time.monotonic()+timeout
        while not self.is_set():
            if end is not None and time.monotonic()>=end: break
            self.expired.wait(min(.025,max(0,end-time.monotonic())) if end is not None else .025)
        return self.is_set()


def assignment_current(store,task_id,assignment):
    with store.connection() as db:
        row=db.execute("SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t ON t.task_id=n.task_id AND t.revision=n.revision "
                       "WHERE n.task_id=? AND n.assignment_ref=? AND n.status='running' AND t.state IN ('EXECUTING','REVIEWING')",
                       (task_id,assignment)).fetchone()
    return row is not None
