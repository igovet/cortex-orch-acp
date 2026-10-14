"""routing.availability: which routes just failed for a reason about the
route, how that is decided (engine.failure_classification.route_failure),
how long it lasts, and how a node's retry treats it
(orchestration.recovery.retry_lost / orchestration.state.attempt_routing)."""
from __future__ import annotations

import time
from datetime import datetime
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.engine.client import EngineProtocolError
from cortex_runtime.engine import failure_classification
from cortex_runtime.orchestration.recovery import MAX_ATTEMPT_RETRIES, MAX_ROUTE_RETRIES, retry_lost
from cortex_runtime.orchestration.state import TaskMachine, attempt_routing
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import availability
from cortex_runtime.store import Store


# --- classification: structured engine evidence only -----------------------

@pytest.mark.parametrize("data,expected", [
    (dict(codexErrorInfo="usageLimitExceeded"), ("quota_exhausted", "connection", "codexErrorInfo=usageLimitExceeded")),
    (dict(codexErrorInfo="rateLimitExceeded"), ("rate_limited", "connection", "codexErrorInfo=rateLimitExceeded")),
    (dict(errorKind="rate_limit"), ("rate_limited", "connection", "errorKind=rate_limit")),
    (dict(errorKind="billing_error"), ("quota_exhausted", "connection", "errorKind=billing_error")),
    (dict(errorKind="model_not_found"), ("model_unavailable", "model", "errorKind=model_not_found")),
])
def test_engine_route_refusals_are_read_from_their_structured_fields(data, expected):
    assert failure_classification.route_failure(EngineProtocolError(-32603, "Internal error", data)) == expected


@pytest.mark.parametrize("error", [
    # The words alone never count: only the adapter's own enum does.
    EngineProtocolError(-32603, "You've hit your usage limit. Try again at 3:05 PM.", None),
    EngineProtocolError(-32603, "rate limit exceeded", dict(message="429 Too Many Requests")),
    EngineProtocolError(-32603, "Internal error", dict(codexErrorInfo="other")),
    EngineProtocolError(-32603, "Internal error", dict(codexErrorInfo={"httpConnectionFailed": {"httpStatusCode": 429}})),
    EngineProtocolError(-32603, "Internal error", dict(errorKind="server_error")),
    EngineProtocolError(-32000, "Authentication required", dict(errorKind="authentication_failed")),
    RuntimeError("usageLimitExceeded"),
])
def test_unrelated_errors_are_never_read_as_route_unavailability(error):
    assert failure_classification.route_failure(error) is None


def test_a_route_unavailable_turn_is_never_fallback_eligible():
    assert failure_classification.classify("route_unavailable:quota_exhausted") == failure_classification.RATE_LIMITED
    assert failure_classification.classify("route_unavailable:rate_limited") == failure_classification.RATE_LIMITED
    assert failure_classification.classify("route_unavailable:model_unavailable") == failure_classification.PROVIDER_RESTRICTION
    assert not failure_classification.is_fallback_eligible(failure_classification.classify("route_unavailable:quota_exhausted"))


# --- records and expiry ------------------------------------------------------

@pytest.fixture()
def runtime_task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="one"), "Implement feature", "owner")
    machine = TaskMachine(store)
    machine.create(task.task_id, "Implement feature")
    return store, machine, task.task_id


def _record(store, task_id, **overrides):
    values = dict(engine="codex-acp", connection_id="cn_codex", model=None, reason="quota_exhausted",
                  scope="connection", evidence="codexErrorInfo=usageLimitExceeded")
    values.update(overrides)
    return availability.record(store, task_id, **values)


def _expires(entry):
    return datetime.fromisoformat(entry["expires_at"]).timestamp()


def test_an_engine_reported_reset_is_the_expiry(runtime_task):
    store, _, task_id = runtime_task
    now = time.time()
    entry = _record(store, task_id, resets_at=now + 5 * 3600, now=now)
    assert entry["expiry_basis"] == "engine_reported"
    assert _expires(entry) == pytest.approx(now + 5 * 3600, abs=1)
    assert availability.active(store, now=now + 4 * 3600) and not availability.active(store, now=now + 6 * 3600)


def test_without_a_reset_a_bounded_default_applies(runtime_task):
    store, _, task_id = runtime_task
    now = time.time()
    passed = _record(store, task_id, resets_at=now - 10, now=now)
    assert passed["expiry_basis"] == "default"
    assert _expires(passed) == pytest.approx(now + availability.DEFAULT_SECONDS["quota_exhausted"], abs=1)
    far = _record(store, task_id, resets_at=now + 90 * 24 * 3600, now=now)
    assert _expires(far) == pytest.approx(now + availability.MAX_SECONDS, abs=1)


def test_scope_decides_what_a_record_blocks(runtime_task):
    store, _, task_id = runtime_task
    _record(store, task_id)
    _record(store, task_id, engine="claude-agent-acp", connection_id="cn_claude", model="haiku",
            reason="configuration_not_confirmed", scope="model", evidence="WorkerModeNotConfirmed")
    entries = availability.active(store)
    assert availability.blocking(entries, "cn_codex", "any-model")["reason"] == "quota_exhausted"
    assert availability.blocking(entries, "cn_claude", "haiku")["reason"] == "configuration_not_confirmed"
    assert availability.blocking(entries, "cn_claude", "sonnet") is None
    assert "haiku on cn_claude: configuration_not_confirmed until" in availability.describe(entries[0])


def test_every_task_of_the_project_sees_a_record(runtime_task):
    store, _, task_id = runtime_task
    other = bootstrap_task(store, SourceEvent(origin="test", event_id="two"), "Other", "owner")
    TaskMachine(store).create(other.task_id, "Other")
    _record(store, task_id)
    assert availability.blocking(availability.active(store), "cn_codex", None) is not None


def test_nothing_is_recorded_without_a_runtime_task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="one"), "Implement feature", "owner")
    assert _record(store, task.task_id) is None
    assert availability.active(store) == []


# --- retries -----------------------------------------------------------------

def _plan():
    return dict(tasks=[dict(id="N0", role="backend-dev", objective="Implement it", dependencies=[],
                            scope=["src/**"])], verification=[["python3", "-B", "-m", "pytest", "-q"]])


def _running_node(machine, channel):
    return machine.claim_ready(channel.task_id, channel.channel_id)[0]


def _fail(store, task_id, node, attempt_id, *, error, detail=None, route=None, failure_class=None):
    """One provisioned attempt of ``node`` that failed with ``error`` --
    and, when ``route`` is given, whose route the runtime recorded."""
    with store.connection() as db:
        db.execute(
            "INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,"
            "owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
            (node["assignment_ref"], task_id, "as_" + attempt_id, attempt_id, "worker-backend-dev", "ready", "ready",
             1, "2026-09-23T00:00:00+00:00", "2026-09-23T00:00:00+00:00"),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "assignment_failed", f"assignment_failed:{node['assignment_ref']}",
        dict(error=error, **({"detail": detail} if detail else {}),
             **({"failure_class": failure_class} if failure_class else {})), origin="runtime",
        assignment_ref=node["assignment_ref"],
    )
    if route:
        _record(store, task_id, attempt_id=attempt_id, **route)


@pytest.fixture()
def executing(runtime_task):
    store, machine, task_id = runtime_task
    channel = SimpleNamespace(task_id=task_id, channel_id="coordinator", role="coordinator")
    machine.submit_plan(channel, _plan(), "p")
    machine.move(task_id, "EXECUTING")
    runtime = SimpleNamespace(_store=store, _engine=SimpleNamespace(
        lookup_owned=lambda attempt_id: "absent", terminate_owned=lambda attempt_id: None))
    return store, machine, channel, runtime


def test_a_route_failure_retry_uses_no_ordinary_retry_and_does_not_escalate(executing):
    store, machine, channel, runtime = executing
    for index in range(MAX_ROUTE_RETRIES):
        node = _running_node(machine, channel)
        _fail(store, channel.task_id, node, f"xa_{index}", error="engine_failure:route_unavailable:quota_exhausted",
              route=dict(connection_id=f"cn_{index}"))
        retry_lost(runtime, channel.task_id, node)
    node = _running_node(machine, channel)
    with store.connection() as db:
        retried = [row[0] for row in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'", (channel.task_id,))]
    assert len(retried) == MAX_ROUTE_RETRIES and all('"route_failure":true' in row for row in retried)
    assert "quota_exhausted until" in retried[0]
    # The next attempt's routing: no model fell short, so no escalation.
    _fail(store, channel.task_id, node, "xa_next", error="dispatch_failed:WorkerModeNotConfirmed",
          route=dict(connection_id="cn_next", reason="configuration_not_confirmed"))
    assert attempt_routing(store, channel.task_id, "xa_next") == (False, "normal", "auto", None, None, None, None)
    # Route retries are bounded too.
    with pytest.raises(StoreError, match="worker_retry_limit:dispatch_failed:WorkerModeNotConfirmed"):
        retry_lost(runtime, channel.task_id, node)


def test_an_ordinary_failure_still_escalates_and_stays_bounded(executing):
    store, machine, channel, runtime = executing
    for index in range(MAX_ATTEMPT_RETRIES):
        node = _running_node(machine, channel)
        _fail(store, channel.task_id, node, f"xa_{index}", error="worker_ended_without_result")
        retry_lost(runtime, channel.task_id, node)
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_last", error="worker_ended_without_result")
    assert attempt_routing(store, channel.task_id, "xa_last") == (True, "normal", "auto", None, None, None, None)
    with pytest.raises(StoreError, match="worker_retry_limit:worker_ended_without_result"):
        retry_lost(runtime, channel.task_id, node)


@pytest.mark.parametrize("code", sorted({"no_available_route", "pinned_route_unavailable",
                                         "invalid_routing_overrides", "route_denied"}))
def test_a_routing_refusal_is_a_named_blocker_never_retried(executing, code):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    detail = "No configured route can take this work now: codex-acp (all models) on cn_codex: quota_exhausted until 2026-09-26"
    _fail(store, channel.task_id, node, "xa_0", error=code, detail=detail)
    with pytest.raises(StoreError) as refused:
        retry_lost(runtime, channel.task_id, node)
    assert str(refused.value) == f"worker_failure_not_retryable:{code}: {detail}"
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'",
                          (channel.task_id,)).fetchone() is None


def test_a_fallback_refused_after_a_capacity_wait_is_retried_within_the_bound(executing):
    """2026-09-28: three queued workers ended with
    fallback_refused:no_untried_authorized_route (failure_class
    capacity_unavailable); retry_lost refused them for good although the same
    route serves again once a slot frees."""
    store, machine, channel, runtime = executing
    for index in range(MAX_ATTEMPT_RETRIES):
        node = _running_node(machine, channel)
        _fail(store, channel.task_id, node, f"xa_{index}", error="fallback_refused:no_untried_authorized_route",
              failure_class="capacity_unavailable")
        retry_lost(runtime, channel.task_id, node)
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_last", error="fallback_refused:no_untried_authorized_route",
          failure_class="capacity_unavailable")
    with pytest.raises(StoreError, match="worker_retry_limit:fallback_refused:no_untried_authorized_route"):
        retry_lost(runtime, channel.task_id, node)


def test_a_fallback_refused_for_another_failure_class_is_still_not_retried(executing):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_0", error="fallback_refused:no_untried_authorized_route",
          failure_class="rate_limited")
    with pytest.raises(StoreError, match="worker_failure_not_retryable:fallback_refused:no_untried_authorized_route"):
        retry_lost(runtime, channel.task_id, node)


def test_delivery_qualification_failure_retry_is_route_scoped_and_never_escalates(executing):
    store, machine, channel, runtime = executing
    node = _running_node(machine, channel)
    _fail(store, channel.task_id, node, "xa_unqualified",
          error="PortableError:instruction_delivery_unqualified",
          route=dict(engine="claude-agent-acp", connection_id="cn_claude",
                     reason="instruction_delivery_unqualified", evidence="claude-agent-acp@0.89.0"))
    retry_lost(runtime, channel.task_id, node)
    next_node = _running_node(machine, channel)
    assert next_node
    assert attempt_routing(store, channel.task_id, "xa_unqualified") == (False, "normal", "auto", None, None, None, None)
    entries = availability.active(store)
    assert availability.blocking(entries, "cn_claude", "haiku")
    assert availability.blocking(entries, "cn_claude", "opus")
