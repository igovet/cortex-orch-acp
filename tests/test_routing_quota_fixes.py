"""Routing and quota behaviour:

- A retry caused only by an infrastructure failure (transport EOF, crash,
  poisoned session, a worker lost to a restart or host disconnect) never
  raises the model tier; only a quality-caused retry (or a fix) does.
- A candidate whose *known* usage reaches 95% ranks behind every non-hot
  candidate, known or unknown usage alike.
- A rejected rate-limit window with no ``resetsAt`` still expires after a
  bounded TTL, and a CLI-sourced window the CLI stops reporting is pruned
  on the next successful refresh.
- The stretch exclusion applies only to stretch candidacy -- a model
  configured with several native tiers (e.g. both light and standard)
  keeps its own non-stretch tiers.
- A pinned effort routing.json's own policy clamps to a different step is
  recorded in the route decision.
"""
from __future__ import annotations

import time
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.connections import quota
from cortex_runtime.orchestration.state import TaskMachine, attempt_routing, event
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from cortex_runtime.store import Store

from test_auto_route import _entry, _live_all, _live_choose
from test_usage_accounting import _USAGE_TEXT, _claude_connection, _environ, _fake_claude_binary, _now, _usage_result


# --- an infrastructure-only retry never escalates the model tier -----

def _task_machine(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='one'), 'Implement feature', 'owner')
    machine = TaskMachine(store)
    machine.create(task.task_id, 'Implement feature')
    channel = SimpleNamespace(task_id=task.task_id, channel_id='coordinator', role='coordinator')
    return machine, channel


def _hard_plan():
    return dict(tasks=[dict(id='N0', role='backend-dev', objective='Implement part', dependencies=[],
                           scope=['src/**'], model='auto', complexity='high')],
                verification=[['python3', '-B', '-m', 'pytest', '-q']])


def _provisioned_node(tmp_path):
    state, channel = _task_machine(tmp_path)
    state.submit_plan(channel, _hard_plan(), 'p')
    state.move(channel.task_id, 'EXECUTING')
    node = state.claim_ready(channel.task_id, channel.channel_id)[0]
    with state.store.connection() as db:
        db.execute(
            "INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,"
            "owner_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
            (node['assignment_ref'], channel.task_id, 'xa_first', 'worker-debugger', 'ready', 'ready', 0, 'now', 'now'),
        )
        db.commit()
    return state, channel, node


@pytest.mark.parametrize("failure_class", ["engine_failure", "transport_failure", "worker_lost"])
def test_an_infrastructure_only_retry_does_not_escalate(tmp_path, failure_class):
    state, channel, node = _provisioned_node(tmp_path)
    with state.store.connection() as db:
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED',
              dict(node=node['node_id'], revision=1, retry=1, failure_class=failure_class))
        db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first').escalate is False


def test_a_quality_caused_retry_still_escalates(tmp_path):
    """A retry recorded before this fix (no failure_class at all) is still
    treated as a quality signal, exactly as before."""
    state, channel, node = _provisioned_node(tmp_path)
    with state.store.connection() as db:
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED', dict(node=node['node_id'], revision=1, retry=1))
        db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first').escalate is True


def test_a_retry_with_a_named_non_infrastructure_failure_class_still_escalates(tmp_path):
    state, channel, node = _provisioned_node(tmp_path)
    with state.store.connection() as db:
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED',
              dict(node=node['node_id'], revision=1, retry=1, failure_class='indeterminate'))
        db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first').escalate is True


def test_one_quality_retry_among_several_infrastructure_ones_still_escalates(tmp_path):
    state, channel, node = _provisioned_node(tmp_path)
    with state.store.connection() as db:
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED',
              dict(node=node['node_id'], revision=1, retry=1, failure_class='engine_failure'))
        event(db, channel.task_id, 'ASSIGNMENT_RETRIED', dict(node=node['node_id'], revision=1, retry=2))
        db.commit()
    assert attempt_routing(state.store, channel.task_id, 'xa_first').escalate is True


# --- a known-hot candidate always loses to any non-hot candidate -----

def test_a_known_hot_candidate_loses_to_an_unknown_usage_candidate_despite_affinity():
    """Before the fix, engine affinity (checked ahead of the hot flag) let
    a known >=95%-used, *preferred-engine* candidate outrank an unknown-
    usage candidate on another engine; the quota band must decide first."""
    from model_policy_fixtures import MODELS
    from test_auto_route import CLAUDE, CODEX

    def usage(entry):
        return 97.0 if entry.connection_id == "codex" else None

    # worker-backend-dev's work_kind ("implementation") prefers codex-acp
    # (auto_route.WORK_KIND_ENGINES); without the quota band first, that preference
    # alone would pick the hot codex standard model over Claude's unknown one.
    picked = auto_route.pick("standard", CODEX + CLAUDE, usage, role="worker-backend-dev", models=MODELS)
    assert picked.route.engine == "claude-agent-acp"


def test_a_known_hot_candidate_loses_to_any_known_non_hot_candidate_too():
    from model_policy_fixtures import MODELS
    from test_auto_route import CLAUDE, CODEX

    def usage(entry):
        return 96.0 if entry.connection_id == "codex" else 40.0

    picked = auto_route.pick("standard", CODEX + CLAUDE, usage, role="worker-backend-dev", models=MODELS)
    assert picked.route.engine == "claude-agent-acp"


def test_both_hot_known_still_ranks_by_the_usual_signals():
    """The quota band only changes hot-vs-non-hot; two hot knowns still rank
    the same way (lower usage, then affinity) as before."""
    from model_policy_fixtures import MODELS
    from test_auto_route import CLAUDE, CODEX

    def usage(entry):
        return 97.0 if entry.connection_id == "codex" else 99.0

    picked = auto_route.pick("standard", CODEX + CLAUDE, usage, role="worker-backend-dev", models=MODELS)
    assert picked.route.engine == "codex-acp"


# --- quota windows always eventually expire or get pruned ------------

def test_a_rejected_window_without_resetsat_expires_after_a_bounded_ttl(tmp_path):
    environ = {"HOME": str(tmp_path), "XDG_STATE_HOME": str(tmp_path / "state")}
    connection_id = "cn_" + "f" * 12
    now = time.time()
    quota.observe(connection_id, {"status": "rejected", "rateLimitType": "five_hour"}, environ)
    record = SimpleNamespace(engine="claude-agent-acp", id=connection_id, credential_ref=None)
    assert quota.used_percent(record, "sonnet", environ, now=now) == 100.0
    ttl = quota._REJECTED_WINDOW_DEFAULT_TTL_SECONDS
    assert quota.used_percent(record, "sonnet", environ, now=now + ttl - 1) == 100.0
    assert quota.used_percent(record, "sonnet", environ, now=now + ttl + 1) is None


def test_a_rejected_window_with_a_real_resetsat_is_unaffected(tmp_path):
    environ = {"HOME": str(tmp_path), "XDG_STATE_HOME": str(tmp_path / "state")}
    connection_id = "cn_" + "g" * 12
    now = time.time()
    quota.observe(connection_id, {"status": "rejected", "rateLimitType": "five_hour", "resetsAt": now + 3600}, environ)
    record = SimpleNamespace(engine="claude-agent-acp", id=connection_id, credential_ref=None)
    assert quota.used_percent(record, "sonnet", environ, now=now + 3700) is None


def test_a_vanished_cli_window_is_pruned_on_the_next_successful_refresh(tmp_path, monkeypatch):
    _fake_claude_binary(monkeypatch)
    environ = _environ(tmp_path)
    record = _claude_connection(environ)
    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", lambda *a, **k: _usage_result(_USAGE_TEXT))
    now = _now("2026-09-20 00:00")
    assert quota.used_percent(record, None, environ, now=now) == 50.0
    windows = quota._load(record.id, environ)
    assert "seven_day" in windows and "seven_day_fable" in windows
    # The next refresh (past the TTL) no longer reports the week windows.
    monkeypatch.setattr(
        quota, "_run_claude_usage_subprocess",
        lambda *a, **k: _usage_result("Current session: 4% used · resets Sep 26, 1:30pm (UTC)\n"))
    quota.used_percent(record, None, environ, now=now + 301)
    windows = quota._load(record.id, environ)
    assert "seven_day" not in windows and "seven_day_fable" not in windows
    assert "five_hour" in windows


def test_an_event_sourced_window_is_never_pruned_by_a_cli_refresh(tmp_path, monkeypatch):
    _fake_claude_binary(monkeypatch)
    environ = _environ(tmp_path)
    record = _claude_connection(environ)
    quota.observe(record.id, {"rateLimitType": "custom_scope", "utilization": 0.3, "resetsAt": time.time() + 3600}, environ)
    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", lambda *a, **k: _usage_result(_USAGE_TEXT))
    quota.used_percent(record, None, environ, now=time.time())
    assert "custom_scope" in quota._load(record.id, environ)


# --- a stretch exclusion never removes a model's other native tier --

def test_a_dual_tier_stretch_excluded_model_still_serves_its_own_native_tier(tmp_path, monkeypatch):
    """A model configured with both light and standard tiers, whose
    *stretch* promotion is excluded for this (deep-tier) work, must still
    be usable through its own native standard tier -- never blanket-
    excluded the way a light-only stretch model correctly is."""
    entries = [_entry("codex", "gpt-hybrid")]
    routing = {"enabled": {"codex-acp": ["gpt-hybrid"]}, "models": {"codex-acp": {
        "auto": ["gpt-hybrid"],
        "tiers": {"gpt-hybrid": ["light", "standard"]},
        "policy": {"gpt-hybrid": {"also_standard_for": ["testing"]}},
    }}}
    project, environ = _live_choose(monkeypatch, tmp_path, entries, routing)
    choice = auto_route.choose("worker-architect", project, environ)  # worker-architect is tier "deep"
    assert choice.model == "gpt-hybrid"
    assert choice.decision["tier"] == "standard"
    assert choice.decision["stretch"]["status"] == "excluded:deep_work"


def test_a_light_only_stretch_excluded_model_is_still_fully_excluded(tmp_path, monkeypatch):
    """Regression guard: the fix narrows the exclusion, it does not
    remove it for a model with no other native tier to fall back on."""
    entries = [_entry("codex", "gpt-light-only")]
    routing = {"enabled": {"codex-acp": ["gpt-light-only"]}, "models": {"codex-acp": {
        "auto": ["gpt-light-only"],
        "tiers": {"gpt-light-only": ["light"]},
        "policy": {"gpt-light-only": {"also_standard_for": ["testing"]}},
    }}}
    project, environ = _live_choose(monkeypatch, tmp_path, entries, routing)
    with pytest.raises(PortableError) as refused:
        auto_route.choose("worker-architect", project, environ)
    assert refused.value.code == "no_available_route"
    assert "codex-acp/gpt-light-only" in refused.value.remediation


# --- a clamped pin effort is recorded in the route decision ------

def test_a_clamped_pin_effort_is_recorded(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path, {"pin": {"*": "codex-acp/gpt-6-luna:low"}})
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert (choice.model, choice.effort) == ("gpt-6-luna", "high")
    assert choice.decision["pin_effort_clamped"] == dict(configured="low", applied="high")


def test_an_honoured_pin_effort_records_no_clamp(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path, {"pin": {"worker-qa-engineer": "claude-agent-acp/claude-sonnet-5"}})
    choice = auto_route.choose("worker-qa-engineer", project, environ)
    assert choice.basis == "auto:pin"
    assert "pin_effort_clamped" not in choice.decision
