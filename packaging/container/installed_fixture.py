"""Fake engine only, loaded by the installed service's existing fixture-launcher seam.

The harness copies this file to the allowlisted tests/test_workspace_journey.py
inside the disposable container. Installed runtime and desktop files are unchanged.
"""
import importlib.util
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "src"), str(ROOT / "tests")]


def main():
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.store import Store
    from cortex_runtime.project_storage import project_store_directory
    from orchestration_fakes import FakeSessionRuntime, FakeWorkspace, FakeConfigSnapshotSource

    spec = importlib.util.spec_from_file_location("installed_agent", ROOT / "crates/workspace-service/agent_launcher.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    project = Path.cwd()
    assert str(project).startswith("/home/acceptance/"), "disposable fixture only"
    store = Store(project_store_directory(project), project_root=project)

    def router(store, channel, ref, launch, instructions):
        def behavior_impl(store, channel, ref, message, index, on_delta=None, on_tool_call=None, cancel_event=None):
            if "Owner review feedback" in str(message):
                mcp_tools.dispatch(store, channel, "answer_submit",
                                   dict(text="Installed package reviewed result.", request_key="review-result"))
                return PromptOutcome(StopReason.END_TURN, "Review applied.")
            with store.connection() as db:
                questions = db.execute("SELECT answer_text FROM pending_questions WHERE task_id=?", (channel.task_id,)).fetchall()
            if not questions:
                mcp_tools.dispatch(store, channel, "user_question_request",
                                   dict(questions=[dict(question="Choose direction?", options=["Alpha", "Beta"]),
                                                   dict(question="Choose format?", options=["Text", "JSON"])],
                                        request_key="installed-questions"))
                return PromptOutcome(StopReason.END_TURN, "Waiting for both answers.")
            assert len(questions) == 2 and all(row[0] for row in questions), "incomplete batch"
            (project / "answered").touch()
            if on_delta:
                on_delta("Installed fake engine working.")
            deadline = time.monotonic() + 180
            while not (project / "finish").exists():
                if cancel_event and cancel_event.is_set():
                    return PromptOutcome(StopReason.CANCELLED, "Stopped.")
                assert time.monotonic() < deadline, "fake finish deadline"
                time.sleep(.05)
            mcp_tools.dispatch(store, channel, "answer_submit",
                               dict(text="Installed package result.", request_key="installed-result"))
            return PromptOutcome(StopReason.END_TURN, "Done.")
        def behavior(store, channel, ref, message, index, on_delta=None, on_tool_call=None, cancel_event=None):
            try:
                return behavior_impl(store, channel, ref, message, index, on_delta, on_tool_call, cancel_event)
            except Exception as error:
                (project / "fixture-error").write_text(type(error).__name__ + ": " + str(error))
                raise
        return behavior

    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)

    class FixtureAgent(module.ServiceAgent):
        def _default_selection_from_user_defaults(self):
            return None

        def _config_options_for_session(self, *args):
            return []

        def _post_session_new_notifications(self, session_id):
            pass

    module.run(runtime=runtime, agent_type=FixtureAgent)


if __name__ == "__main__":
    main()
