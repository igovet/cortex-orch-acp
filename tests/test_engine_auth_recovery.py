"""Native-auth replacement uses fake engines only; no account or network I/O."""
import json
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed, PromptOutcome
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.auth_recovery import is_auth_loss
from cortex_runtime.engine.client import EngineProtocolError
from cortex_runtime.engine.retry_policy import RetryWindow
from cortex_runtime.orchestration.auth_recovery import claim_coordinator, coordinator_recovered, worker_route
from cortex_runtime.orchestration.recovery import retry_lost
from cortex_runtime.orchestration.state import attempt_routing
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.routing import store as routing_store

from test_engine_retry_window import _runtime, _router, _chat
from test_route_availability import executing, runtime_task, _fail as _base_fail, _running_node


def _fail(store, task_id, node, attempt_id, **kwargs):
    _base_fail(store, task_id, node, attempt_id, **kwargs)
    with store.connection() as db:
        db.execute("UPDATE task_events SET payload_json=json_set(payload_json, '$.auth_replay_safe', json('true')) "
                   "WHERE task_id=? AND assignment_ref=? AND event_type='assignment_failed'",
                   (task_id, node["assignment_ref"]))
        db.commit()


def test_auth_launch_restarts_once_then_succeeds(tmp_path):
    runtime, engine, store, task_id = _runtime(
        tmp_path, failure=lambda: EngineProtocolError(-32000, "Authentication required", None), failures=1)
    chunks, emit = _chat()
    runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event())
    assert engine.coordinator_opens == 2
    assert engine.terminate_calls
    assert not engine.fallback_calls
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM task_events WHERE json_extract(payload_json,'$.kind')='COORDINATOR_AUTH_RETRY'").fetchone()[0] == 1
        assert db.execute("SELECT COUNT(*) FROM task_events WHERE json_extract(payload_json,'$.kind')='COORDINATOR_AUTH_RECOVERED'").fetchone()[0] == 1


@pytest.mark.parametrize("failures", [1, 2])
def test_auth_turn_replaces_process_not_just_session_and_is_bounded(tmp_path, failures):
    failed_sessions = []
    def behavior(store, channel, session_ref, message, call_index):
        if len(failed_sessions) < failures:
            failed_sessions.append(session_ref)
            raise EngineTurnFailed("authorization_loss")
        return PromptOutcome(StopReason.END_TURN, "Understood.")
    runtime, engine, store, task_id = _runtime(tmp_path, router=_router(behavior))
    chunks, emit = _chat()
    runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event())
    assert engine.coordinator_opens == 2
    assert len(set(failed_sessions)) == failures
    assert engine.terminate_calls
    assert not engine.fallback_calls
    if failures == 2:
        assert "codex login" in "".join(chunks) and "claude /login" in "".join(chunks)
        assert len(engine.prompt_calls) == 2
    else:
        assert "engine_authentication_required" not in "".join(chunks)


def test_auth_launch_fence_survives_another_runtime_invocation(tmp_path):
    runtime, engine, store, task_id = _runtime(
        tmp_path, failure=lambda: EngineProtocolError(-32000, "Authentication required", None), failures=100)
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from orchestration_fakes import FakeConfigSnapshotSource, FakeWorkspace
    for index in range(2):
        if index:
            runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                           allow_unqualified_delivery=True, wait_timeout_seconds=30)
            runtime._engine_retry_window = RetryWindow(first_delay_seconds=0, max_delay_seconds=0)
        with pytest.raises(StoreError, match="engine_authentication_required"):
            runtime._ensure_coordinator_retrying(task_id, lambda _: True, threading.Event(), "en")
    # A recreated runtime must not even launch for the exhausted episode.
    assert engine.coordinator_opens == 2
    with pytest.raises(StoreError, match="engine_authentication_required"):
        claim_coordinator(store, task_id)
    engine.failures = 0
    runtime._ensure_coordinator_retrying(task_id, lambda _: True, threading.Event(), "en",
                                        auth_probe_source=("test", "after-login"))
    assert engine.coordinator_opens == 3
    # An explicit new input probes once; it does not restore the allowance.
    with pytest.raises(StoreError, match="engine_authentication_required"):
        runtime._ensure_coordinator_retrying(task_id, lambda _: True, threading.Event(), "en",
                                            auth_probe_source=("test", "after-login"))
    coordinator_recovered(store, task_id)
    claim_coordinator(store, task_id)


@pytest.mark.parametrize("error", [
    EngineTurnFailed("route_unavailable:rate_limited"),
    EngineTurnFailed("provider_restricted"),
    EngineProtocolError(-32000, "Authentication required", {"errorKind": "billing_error"}),
    StoreError("unauthorized"),
])
def test_non_native_auth_failures_do_not_spend_auth_allowance(error):
    assert not is_auth_loss(error)


def _audit(store, task_id, attempt_id, engine="claude-agent-acp", route_basis="auto"):
    routing_store.record_attempt_audit(
        store, attempt_id=attempt_id, task_id=task_id, role="worker-backend-dev", engine=engine,
        connection_id="cn_native", model="native-model", params_fingerprint="params",
        config_revision_id=None, decision="authorized", detail=dict(effort="high", route_basis=route_basis))


def test_worker_auth_retry_tears_down_then_persists_one_allowance_without_escalation(executing):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_auth", error="engine_failure:authorization_loss",
          failure_class="authorization_loss")
    _audit(store, channel.task_id, "xa_auth")
    states = {"xa_auth": "alive"}
    terminated = []
    def terminate(attempt):
        terminated.append(attempt)
        states[attempt] = "absent"
    runtime._engine = SimpleNamespace(lookup_owned=lambda a: states.get(a, "absent"), terminate_owned=terminate)
    retry_lost(runtime, channel.task_id, node)
    assert terminated == ["xa_auth"]
    second = _running_node(machine, channel)
    _fail(store, channel.task_id, second, "xa_again", error="engine_failure:authorization_loss",
          failure_class="authorization_loss")
    _audit(store, channel.task_id, "xa_again")
    assert not attempt_routing(store, channel.task_id, "xa_again").escalate
    assert worker_route(store, channel.task_id, "xa_again")["connection_id"] == "cn_native"
    # Repeated reconciliation/recovery cannot grant a second replacement.
    for _ in range(2):
        with pytest.raises(StoreError, match="engine_authentication_required.*claude /login"):
            retry_lost(runtime, channel.task_id, second)
    with store.connection() as db:
        rows = db.execute("SELECT payload_json FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'").fetchall()
    assert len(rows) == 1 and json.loads(rows[0][0])["auth_retry"]


@pytest.mark.parametrize("route_basis", ["auto", "auto:user_request", "auto:pin"])
def test_auth_retry_pins_connection_model_and_effort_without_reranking(executing, route_basis):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_auth", error="authorization_loss", failure_class="authorization_loss")
    _audit(store, channel.task_id, "xa_auth", route_basis=route_basis)
    retry_lost(runtime, channel.task_id, node)
    second = _running_node(machine, channel)
    _fail(store, channel.task_id, second, "xa_next", error="placeholder")
    adapter = object.__new__(EngineSessionManagerAdapter)
    adapter._store = store
    adapter._dispatch_configuration = lambda launch: ("revision", "profile")
    entry = SimpleNamespace(connection_id="cn_native",
                            route=SimpleNamespace(engine="claude-agent-acp", model="native-model", id="native-route"))
    adapter._automatic_route_entries = lambda *args: [entry]
    launch = dict(task_id=channel.task_id, attempt_id="xa_next", assignment_id="as_next", role_id="worker-backend-dev",
                  resolved_connection_id="cn_different", parameters=dict(effort="low"))
    adapter._resolve_runtime_route(launch, store.project_root)
    assert launch["resolved_connection_id"] == "cn_native"
    assert launch["resolved_model"] == "native-model" and launch["parameters"]["effort"] == "high"
    assert launch["resolved_route_id"] == "native-route" and "route_override" not in launch
    assert launch["route_basis"] == route_basis
    adapter._automatic_route_entries = lambda *args: []
    with pytest.raises(StoreError, match="Previous route is no longer authorized"):
        adapter._resolve_runtime_route(launch, store.project_root)


def test_auth_after_coordinator_output_is_not_blindly_replayed(tmp_path):
    def behavior(store, channel, session_ref, message, call_index, *, on_delta):
        on_delta("I have started work.")
        raise EngineTurnFailed("authorization_loss")
    runtime, engine, _, task_id = _runtime(tmp_path, router=_router(behavior))
    chunks, emit = _chat()
    runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event())
    assert engine.coordinator_opens == 1 and len(engine.prompt_calls) == 1
    assert "Turn effects prevent safe replay" in "".join(chunks)
    assert not engine.fallback_calls


def test_worker_auth_replay_refuses_unobserved_or_ambiguous_work(executing):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    _base_fail(store, channel.task_id, node, "xa_auth", error="authorization_loss", failure_class="authorization_loss")
    with pytest.raises(StoreError, match="Turn effects prevent safe replay"):
        retry_lost(runtime, channel.task_id, node)
    with store.connection() as db:
        assert not db.execute("SELECT 1 FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'").fetchall()


def test_worker_auth_replacement_requires_confirmed_process_exit(executing):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_auth", error="authorization_loss", failure_class="authorization_loss")
    runtime._engine = SimpleNamespace(lookup_owned=lambda _: "unknown", terminate_owned=lambda _: None)
    with pytest.raises(StoreError, match="lost_worker_termination_unconfirmed"):
        retry_lost(runtime, channel.task_id, node)


@pytest.mark.parametrize("auth", [True, False])
def test_adapter_maps_protocol_auth_to_the_runtime_auth_class_only(tmp_path, auth):
    adapter = EngineSessionManagerAdapter(lambda _: None, lambda _: str(tmp_path))
    failure = EngineProtocolError(-32000, "Authentication required" if auth else "Internal error", None)
    def prompt(*args, **kwargs):
        raise failure
    adapter._managers["xa_fake"] = SimpleNamespace(prompt_turn=prompt)
    adapter._session_to_attempt["wire_fake"] = "xa_fake"
    adapter._attempt_task["xa_fake"] = "task_fake"
    if auth:
        with pytest.raises(EngineTurnFailed) as raised:
            adapter.prompt("wire_fake", "hello", threading.Event())
        assert raised.value.kind == "authorization_loss"
    else:
        with pytest.raises(EngineProtocolError) as raised:
            adapter.prompt("wire_fake", "hello", threading.Event())
        assert raised.value is failure


@pytest.mark.parametrize("stage", ["launch", "turn"])
@pytest.mark.parametrize("failures", [1, 2])
def test_worker_auth_restart_through_runtime_dispatch(tmp_path, stage, failures):
    from cortex_runtime.orchestration.state import TaskMachine
    cancel = threading.Event()
    worker_sessions = []
    worker_launches = []
    successes = []
    failed = []
    def coordinator(store, channel, ref, message, call_index):
        machine = TaskMachine(store)
        if machine.get_state(channel.task_id) == "PLANNING":
            machine.submit_plan(channel, dict(
                tasks=[dict(id="worker", role="backend-dev", objective="Implement", dependencies=[], scope=["src/**"])],
                verification=[["python3", "-B", "-m", "pytest", "-q"]]), "plan")
        return PromptOutcome(StopReason.END_TURN, "")
    def worker(store, channel, ref, message, call_index):
        worker_sessions.append(ref)
        if stage == "turn" and len(failed) < failures:
            failed.append(ref)
            raise EngineTurnFailed("authorization_loss")
        successes.append(ref)
        cancel.set()
        return PromptOutcome(StopReason.END_TURN, "Finished.")
    def router(store, channel, ref, launch, instructions):
        return coordinator if channel.role == "coordinator" else worker
    runtime, engine, store, task_id = _runtime(
        tmp_path, router=router, window=RetryWindow(first_delay_seconds=0, max_delay_seconds=0))
    original_open = engine.open
    def open_engine(binding, launch, instructions):
        if launch.get("assignment_id"):
            worker_launches.append(launch["attempt_id"])
            if stage == "launch" and len(failed) < failures:
                failed.append(launch["attempt_id"])
                raise EngineProtocolError(-32000, "Authentication required", None)
        return original_open(binding, launch, instructions)
    engine.open = open_engine
    chunks, emit = _chat()
    runtime.start_prompt(task_id, "s1", "Implement.", emit, cancel)
    assert len(worker_launches) == 2 and len(set(worker_launches)) == 2
    assert not engine.fallback_calls
    with store.connection() as db:
        retries = [json.loads(r[0]) for r in db.execute(
            "SELECT payload_json FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'")]
    assert len(retries) == 1 and retries[0]["auth_retry"]
    if failures == 1:
        assert len(successes) == 1
    else:
        assert not successes
        assert "engine_authentication_required" in "".join(chunks)
    if stage == "turn":
        assert len(set(worker_sessions)) == 2
        assert worker_launches[0] in engine.terminate_calls


def test_failed_native_open_retains_identity_before_session_exists(executing, tmp_path, monkeypatch):
    from cortex_runtime.engine import session_manager
    store, machine, channel, runtime = executing
    class AuthFailedManager:
        def __init__(self, *args, **kwargs):
            pass
        def negotiate(self, capabilities):
            raise EngineProtocolError(-32000, "Authentication required", None)
    monkeypatch.setattr(session_manager, "EngineSessionManager", AuthFailedManager)
    from cortex_runtime.portable.identity import new_id
    connection_id = new_id("cn_")
    record = SimpleNamespace(id=connection_id, engine="codex-acp", provider="openai")
    adapter = EngineSessionManagerAdapter(
        lambda _: record, lambda _: str(tmp_path), store=store,
        environ={"HOME": str(tmp_path), "XDG_CONFIG_HOME": str(tmp_path / "config"),
                 "XDG_STATE_HOME": str(tmp_path / "state")},
        install=SimpleNamespace())
    # This test exercises the failed-open boundary, not route policy/capacity.
    adapter._resolve_runtime_route = lambda *args: None
    adapter._authorize_dispatch = lambda *args, **kwargs: None
    adapter._reserve_capacity_blocking = lambda *args, **kwargs: None
    adapter._record_process_identity = lambda *args: None
    adapter._record_dispatch_route_failure = lambda *args: None
    adapter._cleanup_failed_open = lambda *args: True
    with pytest.raises(EngineProtocolError):
        adapter.open("shared_project_root", dict(task_id=channel.task_id, attempt_id="xa_auth_open",
                                                assignment_id="as_auth", role_id="worker-backend-dev",
                                                resolved_model="native-model"), "instructions")
    audits = routing_store.list_audit_for_attempt(store, "xa_auth_open")
    assert len(audits) == 1
    assert audits[0]["connection_id"] == connection_id and audits[0]["model"] == "native-model"


def test_worker_auth_retry_uses_shared_streak_deadline(executing):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_auth", error="authorization_loss", failure_class="authorization_loss")
    # No positive retry window remains.
    runtime._engine_retry_window = RetryWindow(window_seconds=0, first_delay_seconds=0, max_delay_seconds=0)
    with pytest.raises(StoreError, match="engine_authentication_required"):
        retry_lost(runtime, channel.task_id, node)
