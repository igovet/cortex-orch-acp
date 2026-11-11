"""No-paid title races: generation never launches a native engine."""
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "crates/workspace-service"))


def test_owner_rename_wins_late_reply_and_reload(tmp_path):
    import machine_tools as m
    import task_titles as titles
    state = tmp_path / "state"
    assert titles.claim(state, "project", "task")
    m.task_action(tmp_path, "project", state, dict(action="rename", taskId="task", title="Owner title"))
    assert not titles.complete(state, "project", "task", "Generated title")
    assert m.product_of(state, "project")["tasks"]["task"]["titleState"] == "user"
    assert m.product_of(state, "project")["tasks"]["task"]["title"] == "Owner title"
    assert not titles.claim(state, "project", "task")


def test_rename_after_generation_stays_user_even_with_same_text(tmp_path):
    import machine_tools as m
    import task_titles as titles
    state = tmp_path / "state"
    assert titles.claim(state, "project", "task")
    assert titles.complete(state, "project", "task", "Generated")
    m.task_action(tmp_path, "project", state, dict(action="rename", taskId="task", title="Generated"))
    assert not titles.complete(state, "project", "task", "Late")
    assert m.product_of(state, "project")["tasks"]["task"]["titleState"] == "user"


def test_purge_and_duplicate_completion_cannot_resurrect_title(tmp_path):
    import machine_tools as m
    import task_titles as titles
    state = tmp_path / "state"
    assert titles.claim(state, "project", "task")
    assert titles.complete(state, "project", "task", "First")
    assert not titles.complete(state, "project", "task", "Second")
    with m.Locked(state):
        m.write_json(m.product_path(state, "project"), {"tasks": {"task": {"purged": True}}})
    assert not titles.complete(state, "project", "task", "Late")
    assert not titles.claim(state, "project", "task")


def test_jobs_failure_deadline_and_replay_keep_fallback(tmp_path):
    import threading
    import time
    import machine_tools as m
    import task_titles as titles
    from test_workspace_api import api_for, create_command, query
    api = api_for(tmp_path)
    task = api.handle(create_command(description="First line\nMore detail"))["target"]
    calls = []
    def late(description, options, expires):
        calls.append(description)
        time.sleep(0.08)
        return "Too late"
    jobs = titles.TitleJobs(tmp_path / "state", late, threading.Event(), deadline=0.02)
    jobs.submit(api, task)
    deadline = time.monotonic() + 2
    while len(calls) == 0 and time.monotonic() < deadline:
        time.sleep(0.01)
    time.sleep(0.12)
    jobs.tick(api)
    time.sleep(0.03)
    assert calls == ["First line\nMore detail"]
    assert "title" not in m.product_of(jobs.state, api.project_id)["tasks"][task]
    assert m.product_of(jobs.state, api.project_id)["tasks"][task]["titleState"] == "failed"
    projected = api.handle(query("tasks.get", task))
    assert projected["description"].splitlines()[0] == "First line"
    assert projected["executionId"] is None

    other = api.handle(create_command("second"))["target"]
    def failure(*args):
        raise RuntimeError("unavailable engine")
    jobs.generate = failure
    jobs.submit(api, other)
    time.sleep(0.08)
    assert "title" not in m.product_of(jobs.state, api.project_id)["tasks"][other]
    assert m.product_of(jobs.state, api.project_id)["tasks"][other]["titleState"] == "failed"


def test_configured_route_and_bounded_fake_engine(tmp_path, monkeypatch):
    from dataclasses import dataclass
    from types import SimpleNamespace
    import threading
    import time
    import pytest
    import task_titles as titles
    import cortex_runtime.connections.onboarding as onboarding
    from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind, StopReason

    @dataclass
    class Route:
        id: str = "selected"
        model: str = "model"
        supported_parameters: tuple = ()
    entry = SimpleNamespace(connection_id="connection", route=Route())
    agent = SimpleNamespace(
        sessions={"template": {"project": str(tmp_path), "default_selection":
                              {"connection_id": "connection", "model": "model", "parameters": {}}}},
        environ={}, _workspace_shutdown=threading.Event(),
        _selectable_entries_and_connections=lambda project: ([entry], {}))
    monkeypatch.setattr(onboarding, "resolve_connection_for_route",
                        lambda *a, **kw: SimpleNamespace(engine="codex-acp"))
    made = []
    class FakeEngine:
        output = "A concise title"
        silent = False
        def __init__(self, record, cwd, **kw):
            self.closed = threading.Event()
            made.append(self)
            assert not list(Path(cwd).iterdir())
        def negotiate(self, caps):
            assert caps == ()
        def create_session(self, *args, **kw):
            assert kw["session_new_meta"]["systemPrompt"]
            return "session"
        def configure(self, session, revision):
            assert revision.route.model == "model"
            assert dict(revision.route.supported_parameters) == {"mode": "read-only"}
        def prompt_turn(self, session, prompt, prompt_blocks=None):
            self.prompt_blocks = prompt_blocks
            assert "Do the work" in prompt
            if self.silent:
                assert self.closed.wait(1), "deadline did not close a silent engine"
            yield EngineEvent(EngineEventKind.TURN_DELTA, delta_text=self.output)
            yield EngineEvent(EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN)
        def shutdown(self, **kw):
            self.closed.set()
    def generate(duration=2):
        return titles.generate(agent, "template", "Do the work", {}, time.monotonic() + duration,
                               manager_factory=FakeEngine)
    assert generate() == "A concise title"
    assert made[-1].closed.is_set()
    from planning_jobs import INSTRUCTIONS as PLAN
    files = [dict(name="screen.png", mimeType="image/png", data="eA==")]
    assert titles.generate(agent, "template", "Do the work", {"attachments": files}, time.monotonic() + 2,
                           manager_factory=FakeEngine, instructions=PLAN) == "A concise title"
    assert made[-1].prompt_blocks[1] == dict(type="image", mimeType="image/png", data="eA==")
    assert "eA==" not in made[-1].prompt_blocks[0]["text"]
    FakeEngine.output = "x" * (titles.MAX_OUTPUT + 1)
    assert generate() is None
    FakeEngine.silent = True
    assert generate(0.06) is None
    assert made[-1].closed.is_set()

    # No matching route never constructs a manager; no native-account lookup.
    before = len(made)
    agent._selectable_entries_and_connections = lambda project: ([], {})
    assert generate() is None
    assert len(made) == before
    agent._selectable_entries_and_connections = lambda project: ([entry], {})
    def no_login(*a, **kw):
        raise ValueError("login unavailable")
    monkeypatch.setattr(onboarding, "resolve_connection_for_route", no_login)
    with pytest.raises(ValueError, match="login unavailable"):
        generate()
    assert len(made) == before


def test_generated_title_validation(tmp_path):
    import task_titles as titles
    state = tmp_path / "state"
    assert titles.claim(state, "project", "task")
    for title in ("", "x" * 201, "Two\nlines", "Control\x01text", None):
        assert not titles.complete(state, "project", "task", title)
    assert titles.complete(state, "project", "task", "Заголовок")

