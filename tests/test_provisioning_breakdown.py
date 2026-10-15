"""T035 FINDING-13 #1: where does ``delegation_requested`` -> ``assignment_ready``
actually go? Instruments the *real* production stages (only the fake,
subprocess ``tests/fake_engine_process.py`` engine is real/spawned -- never
a real codex-acp/claude-agent-acp process, never a paid call) and asserts a
structural regression guard, not a flaky wall-clock number.

Measured stages (see ``delegation.provisioning.STEP_ORDER`` for the outer
five, and ``delegation.live_adapters.EngineSessionManagerAdapter.open`` for
the ``engine_session`` step's own inner phases):

- ``workspace``       -- SharedProjectRootWorkspaceProvisioner.create (pure
                          bookkeeping, T025 owns real isolation -- see that
                          class's own docstring)
- ``channel``          -- trusted_channel.issue_channel_tx (one local SQLite
                          transaction)
- ``config_snapshot``  -- ConfigurationServiceSnapshotSource.snapshot (a
                          read over an already-applied revision row)
- ``context_snapshot``  -- a pure sha256 of already-known ids (no I/O)
- ``engine_session``   -- EngineSessionManagerAdapter.open, itself broken
  down further into:
    - ``negotiate``       -- spawn the process + ``initialize`` (bounded by
                              ``initialize_timeout``)
    - ``create_session``  -- ``session/new`` (bounded by ``call_timeout``)
    - ``catalog_capture`` -- local parsing of the session's own observed
                              configOptions, no wire call
    - ``mode_floor``      -- a second wire call confirming this worker's
                              own engine mode (T035, ``call_timeout``-bounded)

FINDING-13 #1's own honest caveat, repeated here rather than only in prose:
the fake engine answers every RPC near-instantly and never has a real
model to think with, so ``negotiate``/``create_session``/``mode_floor`` (the
three wire round-trips) are NOT representative of a real engine's latency
here -- only their *relative order and structure* is. ``workspace``/
``channel``/``config_snapshot``/``context_snapshot`` (pure local
bookkeeping, no external process either way) ARE representative: they are
already this cheap in production today.
"""
from __future__ import annotations

import threading
import time
import uuid
from pathlib import Path

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog import store as catalog_store
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import (
    ConfigurationServiceSnapshotSource,
    EngineSessionManagerAdapter,
    SharedProjectRootWorkspaceProvisioner,
)
from cortex_runtime.delegation.provisioning import ProvisioningService, _idempotency_key
from cortex_runtime.portable import roles
from cortex_runtime.portable.errors import PortableError, PortableErrorCategory
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel

from model_policy_fixtures import write_policy
from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario


class _FakeConfigAdapter:
    def configure(self, engine_session_id, revision):
        return "evidence-ref"


def _timed(monkeypatch, obj, method_name, sink, key):
    """Wrap ``obj.method_name`` (an already-bound method) to record its
    wall-clock duration into ``sink[key]`` (a list, appended -- a step may
    run more than once across a retried/idempotent call). Never changes
    behavior/return value."""
    original = getattr(obj, method_name)

    def _wrapped(*args, **kwargs):
        start = time.perf_counter()
        try:
            return original(*args, **kwargs)
        finally:
            sink.setdefault(key, []).append(time.perf_counter() - start)

    monkeypatch.setattr(obj, method_name, _wrapped)


def _setup(tmp_path, monkeypatch):  # returns (store, channel, service, adapter, timings)
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", "owner-1")
    channel = resolve_channel(store, result.channel_id, result.secret)
    with store.connection() as db:
        epoch = OwnershipEpoch(epoch=current_owner_epoch(db, result.task_id), owner="owner-1")

    environ = isolated_environ(tmp_path)
    write_policy(environ, {"codex-acp": {"gpt-x": "standard"}}, automatic={"codex-acp": ["gpt-x"]})
    scenario_path = write_scenario(tmp_path, default_scenario())
    connection = make_connection_record(new_connection_id(), fake_launch_command(scenario_path), environ=environ)
    # Explicitly signed-in *fake* native directory, as required by catalog
    # selection; no production account or credentials are consulted.
    home = Path(connection.credential_ref.path)
    home.mkdir(parents=True, exist_ok=True)
    home.chmod(0o700)
    (home / "auth.json").write_text("{}")
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection

    options = [dict(id="model", name="Model", category="model", type="select", currentValue="gpt-x",
                     options=[dict(value="gpt-x", name="GPT X")])]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    catalog_store.save_snapshot(snapshot, environ)
    entry = snapshot.entries[0]
    config_service = ConfigurationService(store)
    revision = config_service.propose(
        result.task_id, entry, ConfigurationInputs(route=entry.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
    )
    config_service.apply(result.task_id, revision["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    workspace = SharedProjectRootWorkspaceProvisioner()
    config_source = ConfigurationServiceSnapshotSource(config_service)
    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(project), environ=environ, call_timeout=10.0, store=store,
    )

    timings: dict[str, list[float]] = {}
    service = ProvisioningService(store, workspace, adapter, config_source)
    for step in ("_step_workspace", "_step_channel", "_step_config_snapshot", "_step_context_snapshot", "_step_engine_session"):
        _timed(monkeypatch, service, step, timings, step)

    # EngineSessionManager instances are constructed fresh inside open(); an
    # instance method can't be pre-wrapped, so the *class* methods are
    # wrapped instead (every instance shares them) -- restored automatically
    # by monkeypatch's own teardown, exactly like every other patch here.
    monkeypatch.setattr(EngineSessionManager, "negotiate", _class_timed(EngineSessionManager.negotiate, timings, "negotiate"))
    monkeypatch.setattr(EngineSessionManager, "create_session", _class_timed(EngineSessionManager.create_session, timings, "create_session"))
    monkeypatch.setattr(
        EngineSessionManagerAdapter, "_capture_catalog_observation",
        _class_timed(EngineSessionManagerAdapter._capture_catalog_observation, timings, "catalog_capture"),
    )
    monkeypatch.setattr(
        EngineSessionManagerAdapter, "_apply_worker_mode_floor",
        _class_timed(EngineSessionManagerAdapter._apply_worker_mode_floor, timings, "mode_floor"),
    )
    return store, channel, service, adapter, timings


def _class_timed(original, sink, key):
    def _wrapped(self, *args, **kwargs):
        start = time.perf_counter()
        try:
            return original(self, *args, **kwargs)
        finally:
            sink.setdefault(key, []).append(time.perf_counter() - start)

    return _wrapped


def test_provisioning_stage_breakdown_against_the_fake_engine(tmp_path, monkeypatch, capsys):
    store, channel, service, adapter, timings = _setup(tmp_path, monkeypatch)

    wall_start = time.perf_counter()
    try:
        outcome = service.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", "req-1")
        wall_total = time.perf_counter() - wall_start
    finally:
        for attempt_id in list(adapter._managers):
            adapter.terminate_owned(attempt_id)

    assert outcome.status == "ready"
    assert not adapter._managers
    with store.connection() as db:
        connection_id = db.execute("SELECT route_connection_id FROM configuration_revisions WHERE task_id=?",
                                   (channel.task_id,)).fetchone()["route_connection_id"]
    assert capacity.count_active(store, connection_id) == 0

    report_lines = ["provisioning breakdown (fake engine, one delegate() call):"]
    for key in ("_step_workspace", "_step_channel", "_step_config_snapshot", "_step_context_snapshot", "_step_engine_session",
                "negotiate", "create_session", "catalog_capture", "mode_floor"):
        durations = timings.get(key, [])
        total = sum(durations)
        report_lines.append(f"  {key:<20s} calls={len(durations)} total={total*1000:.1f}ms")
    report_lines.append(f"  {'wall_total':<20s} total={wall_total*1000:.1f}ms")
    print("\n".join(report_lines))

    # -- structural assertions (never a bare wall-clock number: the fake
    # engine's own absolute latency is explicitly not representative -- see
    # this module's own docstring) --

    # Every one of ProvisioningService's own five named steps actually ran
    # exactly once for a fresh (non-retried) delegation.
    for step in ("_step_workspace", "_step_channel", "_step_config_snapshot", "_step_context_snapshot", "_step_engine_session"):
        assert len(timings.get(step, [])) == 1, f"{step} should run exactly once for a fresh delegate() call"

    # The three real wire/process phases inside engine_session all ran.
    assert len(timings.get("negotiate", [])) == 1
    assert len(timings.get("create_session", [])) == 1
    assert len(timings.get("mode_floor", [])) == 1

    # Regression guard: against the fake engine, the ENTIRE delegate() call
    # -- process spawn, initialize, session/new, mode-floor confirm, every
    # local DB step -- must stay well under a generous ceiling. This is not
    # a real-engine latency claim (the fake engine is deliberately far
    # faster than any real one -- see this module's docstring); it exists
    # only to catch a future change that accidentally introduces an
    # unbounded sleep/poll/retry loop into the provisioning path itself
    # (independent of engine speed). 10s is generous even under heavy CI
    # contention for what is, on the fake engine, sub-second work.
    assert wall_total < 10.0, f"provisioning against the fake engine took {wall_total:.2f}s -- investigate for a new stall"

    # The four pure-local steps (no subprocess, no wire call either way)
    # must stay a small fraction of the total -- if one of them ever grows
    # to dominate wall_total, that is itself the regression this guards
    # against (a local step should never rival a real subprocess round-trip).
    local_total = sum(sum(timings.get(s, [])) for s in ("_step_workspace", "_step_channel", "_step_config_snapshot", "_step_context_snapshot"))
    assert local_total < 1.0, f"local provisioning steps took {local_total:.2f}s -- expected near-zero (no I/O beyond SQLite)"


@pytest.mark.parametrize("through_runtime", [False, True])
def test_required_skill_failure_blocks_session_and_reports_diagnostic(tmp_path, monkeypatch, through_runtime):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.delegation import events as delegation_events

    store, channel, service, adapter, timings = _setup(tmp_path, monkeypatch)
    original = roles.load_role
    cause = PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="role_source_unavailable",
        remediation="Restore the content-safety skill source.",
        retry_eligible=False,
    )

    def fail_required(skill_id):
        if skill_id == "content-safety":
            raise cause
        return original(skill_id)

    monkeypatch.setattr(roles, "load_role", fail_required)
    assignment_ref = _idempotency_key(channel.task_id, channel.channel_id, "missing-skill")
    if through_runtime:
        runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
        runtime._store = store
        runtime._provisioning = service
        runtime._provision_and_prompt_inner(
            channel.task_id, channel,
            dict(assignment_ref=assignment_ref, title="Title", brief="Brief",
                 role="backend-dev", acceptance="Acceptance", request_key="missing-skill"),
            threading.Event(),
        )
        failures = [event for event in delegation_events.list_since(store, channel.task_id)
                    if event.event_type == "assignment_failed"]
        assert len(failures) == 1
        assert failures[0].assignment_ref == assignment_ref
        assert failures[0].payload["error"] == "required_skill_unavailable"
        diagnostic = failures[0].payload["detail"]
        assert not any(event.event_type == "assignment_ready"
                       for event in delegation_events.list_since(store, channel.task_id))
    else:
        with pytest.raises(PortableError) as excinfo:
            service.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", "missing-skill")
        assert excinfo.value.code == "required_skill_unavailable"
        assert excinfo.value.retry_eligible is False
        assert excinfo.value.__cause__ is cause
        diagnostic = excinfo.value.remediation

    assert "content-safety" in diagnostic
    assert "worker-backend-dev" in diagnostic
    assert cause.code in diagnostic and cause.remediation in diagnostic
    with store.connection() as db:
        row = db.execute("SELECT * FROM delegation_provisioning WHERE idempotency_key=?",
                         (assignment_ref,)).fetchone()
        effects = db.execute(
            "SELECT * FROM delegation_effects WHERE idempotency_key=? AND step='engine_session'",
            (assignment_ref,),
        ).fetchall()
    assert row["status"] == "failed"
    assert row["error_code"] == "required_skill_unavailable"
    assert row["step"] == "claimed:engine_session"
    assert not effects
    assert not adapter._managers
    assert "negotiate" not in timings
    assert not service._pending_channel_secrets

    # The same failed request cannot reopen, even after restoring the source.
    monkeypatch.setattr(roles, "load_role", original)
    with pytest.raises(PortableError, match="required_skill_unavailable"):
        service.delegate(channel, "Title", "Brief", "backend-dev", "Acceptance", "missing-skill")
    assert not adapter._managers
