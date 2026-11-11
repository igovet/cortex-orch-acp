#!/usr/bin/env python3
"""Release-owned stdio host for the service's explicitly activated ACP child.

The authenticated Rust service supplies a project/actor binding at exec, not in
model input, prompt metadata or arbitrary ACP client JSON. Generic cortex_acp
keeps its existing, unadvertised activation semantics. The bundle installer must
ship this host alongside the exact Python runtime payload.
"""
import argparse
import os
from pathlib import Path
import signal
import sys
import threading

# Resolve only the release layout; never inherit project PYTHONPATH.
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from cortex_runtime.acp.agent import Agent
from cortex_runtime.acp.transport import StdioTransport


class ServiceAgent(Agent):
    def __init__(self, runtime, *, binding, environ=None):
        super().__init__(runtime, environ=environ)
        self._service_binding = binding
        self._service_template = None
        self._workspace_shutdown = threading.Event()
        self._title_jobs = None
        self._planning_jobs = None
        self._start_preflight = False
        self._service_log("agent_started")

    def _service_log(self, event):
        state = self.environ.get("CORTEX_WORKSPACE_STATE_DIR")
        if state:
            import machine_tools
            try:
                machine_tools.service_log(state, event)
            except OSError:
                pass

    def _session_new(self, params):
        result = super()._session_new(params)
        if self._service_template is None:
            from cortex_runtime.workspace_api.task_options import exclude_helpers
            exclude_helpers(params["cwd"])
            self._service_template = result["sessionId"]
            result["workspaceActivation"] = self.activate_workspace(
                result["sessionId"], project_id=self._service_binding.project_id,
                actor_id=self._service_binding.actor_id, request_id="service-activation")
            if self._start_preflight:
                self._workspace_apis[self._service_binding.project_id].start_blocker = self._start_blocker
            state = self.environ.get("CORTEX_WORKSPACE_STATE_DIR")
            if state:
                import sprint_recovery
                sprint_recovery.restore(self._workspace_apis[self._service_binding.project_id], state)
                import task_titles
                import planning_jobs
                self._planning_jobs = planning_jobs.PlanningJobs(state, self._generate_plan, self._workspace_shutdown)
                self._title_jobs = task_titles.TitleJobs(
                    state, self._generate_title, self._workspace_shutdown)
                threading.Thread(target=self._workspace_queues, args=(state,), name="workspace-queues", daemon=True).start()
        return result

    def _start_blocker(self):
        """Why a task cannot run on this machine now, before anything is recorded for it."""
        state = self.environ.get("CORTEX_WORKSPACE_STATE_DIR")
        if state:
            import maintenance
            if maintenance.installing(state):
                return "A package update is installing or unconfirmed. Finish or check it before starting work."
        project = self.sessions[self._service_template]["project"]
        entries, _ = self._selectable_entries_and_connections(project)
        return None if entries else ("No engine is ready, so nothing was started. Sign in with codex login or "
                                     "claude /login, wait for the models to appear, then start this task again.")

    def _generate_title(self, description, options, expires):
        import task_titles
        return task_titles.generate(self, self._service_template, description, options, expires)

    def _generate_plan(self, description, options, expires, cancel):
        import task_titles, planning_jobs
        return task_titles.generate(self, self._service_template, description, options, expires,
                                    instructions=planning_jobs.INSTRUCTIONS, max_output=32000, cancel=cancel)

    def _workspace_request(self, params):
        result = super()._workspace_request(params)
        if (self._title_jobs is not None and params.get("operation") == "tasks.create"
                and result.get("outcome") == "recorded"):
            try:
                self._title_jobs.submit(self._workspace_apis[params["projectId"]], result["target"])
            except Exception:
                pass  # The description was saved; naming must not fail its reply.
        return result

    def _maintenance_cleanup(self, state, api):
        import machine_tools
        machine_tools.cleanup(state, [{"id": api.project_id, "root": api.store.project_root, "name": "Project"}])

    def _maintenance_interval(self):
        return 300

    def _jobs_running(self):
        """A finishing job frees a slot or a queued item, which no file records."""
        return bool(self._planning_jobs is not None and self._planning_jobs.jobs) or (
            self._title_jobs is not None and self._title_jobs.running())

    def _announce_product(self, state, project, seen):
        """The screens hear about what the queues changed here, instead of rereading to find out."""
        import machine_tools
        import scheduler
        now = scheduler._stat(machine_tools.product_path(state, project))
        if seen is not None and now != seen:
            try:
                self.transport.send_notification({"jsonrpc": "2.0", "method": "_cortex/workspace/update",
                                                  "params": {"projectId": project, "kind": "product"}}, timeout=1)
            except Exception:
                pass  # A screen that misses this reads the same facts on its next look; the queue is not at stake.
        return now

    def _workspace_queues(self, state):
        import scheduler
        import time
        api = self._workspace_apis[self._service_binding.project_id]
        gate = scheduler.Gate(api, state)
        maintenance_at = 0
        announced = None
        while not self._workspace_shutdown.is_set():
            try:
                if gate.due(self._jobs_running()):
                    scheduler.tick(api, state)
                    if self._planning_jobs is not None:
                        self._planning_jobs.tick(api)
                    if self._title_jobs is not None:
                        try:
                            self._title_jobs.tick(api)
                        except Exception:
                            pass  # Naming does not pause an otherwise healthy sprint.
                    gate.done()
                if time.monotonic() >= maintenance_at:
                    self._maintenance_cleanup(state, api)
                    maintenance_at = time.monotonic() + self._maintenance_interval()
                announced = self._announce_product(state, api.project_id, announced)
            except Exception:
                self._service_log("scheduler_failed")
                import machine_tools
                with machine_tools.Locked(state):
                    product = machine_tools.product_of(state, api.project_id)
                    for sprint in product["sprints"]:
                        if sprint["queueState"] == "running":
                            sprint.update(queueState="paused", pauseReason="The service could not verify runtime data. Review and resume the queue.")
                    machine_tools.write_json(machine_tools.product_path(state, api.project_id), product)
            self._workspace_shutdown.wait(2)

    def _handle_eof(self, reason):
        self._workspace_shutdown.set()
        self._service_log("agent_stopped")
        return super()._handle_eof(reason)

    def _session_cancel(self, params):
        # Cancelling the controller during explicit service stop reaches every
        # execution session, including one whose start reply was lost.
        if params.get("sessionId") == self._service_template:
            for session in list(self.sessions):
                super()._session_cancel(dict(sessionId=session))
        else:
            super()._session_cancel(params)


def run(*, runtime=None, agent_type=ServiceAgent):
    parser = argparse.ArgumentParser()
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--actor-id", required=True)
    binding = parser.parse_args()
    os.umask(0o077)
    raw_stdout = sys.stdout.buffer
    sys.stdout = sys.stderr
    preflight = runtime is None
    if runtime is None:
        from cortex_acp import _fail_closed_on_pinned_mismatch, _ensure_first_run_configuration
        from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
        _fail_closed_on_pinned_mismatch()
        _ensure_first_run_configuration()
        runtime = ProductionOrchestratingRuntime()
    agent = agent_type(runtime, binding=binding, environ=os.environ)
    agent._start_preflight = preflight
    transport = StdioTransport(agent.handle_message, raw_stdout,
                               on_writer_closed=agent.handle_writer_closed)
    agent.bind_transport(transport)
    transport.start()
    if threading.current_thread() is threading.main_thread():
        signal.signal(signal.SIGTERM, lambda _s, _f: transport.request_shutdown())
    reason = transport.wait_for_shutdown_signal()
    if reason == "signal":
        agent._handle_eof("sigterm")
    if reason in ("writer_closed", "signal"):
        if reason == "writer_closed":
            transport.stop_writer()
        agent.join_workers()
        agent.shutdown_runtime()
        if reason == "signal":
            transport.stop_writer()
        os._exit(0)
    transport.join_reader()
    agent.join_workers()
    agent.shutdown_runtime()
    transport.stop_writer()
    return 0


if __name__ == "__main__":
    raise SystemExit(run())
