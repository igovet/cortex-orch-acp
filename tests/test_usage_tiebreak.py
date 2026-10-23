"""Plan usage is only the last tie-breaker between otherwise equal routes.

Observed 2026-10-23: a security audit the coordinator recommended for Opus
was ranked by the owner's preferred models (Sol and Sonnet for review work),
and plan usage then picked Sol (7% against 52%). The user then chose a
question option asking for an Opus-only audit. Owner decisions: a choice
among the models the owner allows is legitimate and never refuses; an
explicit user request always wins unless its route is unavailable or
exhausted; usage only breaks ties and never overrides a coordinator-named
eligible route except when that route is hot (>=95%).
"""
import pytest

from cortex_runtime.acp import questions_store
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from cortex_runtime.store import Store
from test_model_settings_template import entry
from test_settings_only_routing import fake_catalog, install

OPUS = "claude-agent-acp/opus[1m]"
SONNET = "claude-agent-acp/claude-sonnet-5"
SOL = "codex-acp/gpt-6.1-sol"
QUESTION = "Как поступить с обязательным аудитом этой задачи?"
OPUS_OPTION = "Провести аудит только Claude Opus — claude-agent-acp/opus[1m], без замены другой моделью."
OTHER_OPTION = "Для этой задачи снять требование Opus-аудита и использовать уже выполненную проверку Sonnet."


def _settings():
    """The relevant subset of the owner's routing settings at the time."""
    return {
        "model_policy_version": 2,
        "enabled": {"claude-agent-acp": ["opus[1m]", "claude-sonnet-5"], "codex-acp": ["gpt-6.1-sol"]},
        "models": {
            "claude-agent-acp": {
                "auto": ["opus[1m]", "claude-sonnet-5"],
                "tiers": {"opus[1m]": ["flagship", "deep"], "claude-sonnet-5": ["standard", "deep"]},
                "policy": {
                    "opus[1m]": {"preferred_for": ["design", "planning", "analysis"],
                                 "preferred_when": "Use for the listed kinds.",
                                 "request_names": ["opus", "опус"]},
                    "claude-sonnet-5": {"preferred_for": ["review", "testing"]}}},
            "codex-acp": {
                "auto": ["gpt-6.1-sol"], "tiers": {"gpt-6.1-sol": ["standard", "deep"]},
                "policy": {"gpt-6.1-sol": {"preferred_for": ["review", "testing"]}}}}}


USED = {"gpt-6.1-sol": 7.0, "claude-sonnet-5": 52.0, "opus[1m]": 52.0}


@pytest.fixture
def owner(tmp_path, monkeypatch):
    from cortex_runtime.connections import quota
    env = install(tmp_path, _settings())
    fake_catalog(monkeypatch, [entry("claude-agent-acp", "opus[1m]"), entry("claude-agent-acp", "claude-sonnet-5"),
                               entry("codex-acp", "gpt-6.1-sol")])
    used = dict(USED)
    monkeypatch.setattr(quota, "used_percent", lambda _conn, model, *_, **_kw: used.get(model))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="audit"), "Проведи аудит безопасности.", "owner")
    return tmp_path, env, store, task, used


def _choose(owner, **kwargs):
    root, env, _store, _task, _used = owner
    return auto_route.choose("worker-security-auditor", root, env, **kwargs)


def test_a_choice_among_the_owners_allowed_models_never_refuses(owner):
    """The observed dispatch: the coordinator named Opus (not preferred for
    review), so the owner's preferred models rank first. The runtime selects
    one of them and dispatches; it never refuses or blocks."""
    choice = _choose(owner, model_hint=OPUS, model_reason="The project requires Opus audits.")
    decision = choice.decision
    assert f"{decision['chosen']}" in (SOL, SONNET)
    assert decision["model_hint_status"] == "ignored:owner_preferred"
    assert decision["purpose"]["status"] == "honoured"
    # Usage no longer moves Sol ahead by its 45-point lead: review work leans
    # to the Claude engine, and usage would only break a remaining tie.
    assert decision["chosen"] == SONNET and decision["decided_by"] == "affinity"
    assert decision["rank_version"] == auto_route.RANK_VERSION


def test_the_users_chosen_opus_only_option_routes_the_audit_to_opus(owner):
    """The observed answer to the observed question requests Opus; dispatch
    then uses Opus regardless of owner preferences and plan usage."""
    _root, _env, store, task, _used = owner
    question = questions_store.create_if_absent(store, task.task_id, "q-audit", QUESTION,
                                                options=[OPUS_OPTION, OTHER_OPTION])
    assert auto_route.user_model_request(store, task.task_id, OPUS) is None
    assert questions_store.answer(store, question.id, OPUS_OPTION, "elicitation")
    grant = auto_route.user_model_request(store, task.task_id, OPUS)
    assert grant is not None and grant["source_ref"] == questions_store.answer_ref(question.id)
    assert auto_route.user_model_request(store, task.task_id, SOL) is None
    assert auto_route.user_model_request(store, task.task_id, SONNET) is None
    assert auto_route.user_purpose_model_request(store, task.task_id, "worker-security-auditor") == OPUS
    choice = _choose(owner, model_hint=OPUS, model_reason="User answer.", user_request=grant)
    assert (f"claude-agent-acp/{choice.model}", choice.basis) == (OPUS, "auto:user_request")
    assert choice.decision["user_model_request"]["status"] == "honoured"


def test_the_other_option_grants_nothing(owner):
    _root, _env, store, task, _used = owner
    question = questions_store.create_if_absent(store, task.task_id, "q-audit", QUESTION,
                                                options=[OPUS_OPTION, OTHER_OPTION])
    assert questions_store.answer(store, question.id, OTHER_OPTION, "elicitation")
    for identity in (OPUS, SOL, SONNET):
        assert auto_route.user_model_request(store, task.task_id, identity) is None


def test_a_user_request_beats_usage_until_its_route_is_exhausted(owner):
    _root, _env, store, task, used = owner
    from cortex_runtime.bootstrap import append_source_event
    append_source_event(store, SourceEvent(origin="test", event_id="opus"), "Аудит только Opus.", task.task_id)
    grant = auto_route.user_model_request(store, task.task_id, OPUS)
    used["opus[1m]"] = 99.0  # hot, yet requested
    choice = _choose(owner, model_hint=OPUS, user_request=grant)
    assert (choice.model, choice.basis) == ("opus[1m]", "auto:user_request")
    used["opus[1m]"] = 100.0
    with pytest.raises(PortableError, match="user_requested_route_unavailable"):
        _choose(owner, model_hint=OPUS, user_request=grant)


# --- the ranking rule itself -------------------------------------------------

def _rank(routes, used, *, role="worker-backend-dev", hint="auto", preferred=None, tier="standard"):
    observed = [entry(*route.split("/")) for route in routes]
    models = {}
    for route in routes:
        engine, model = route.split("/")
        config = models.setdefault(engine, {"auto": [], "tiers": {}, "policy": {}})
        config["auto"].append(model)
        config["tiers"][model] = tier
        config["policy"][model] = {"preferred_for": preferred[route]} if preferred and route in preferred else {}
    return auto_route.rank(tier, observed, lambda e: used.get(f"{e.route.engine}/{e.route.model}"),
                           role=role, model_hint=hint, models=models)


CODEX, CLAUDE, CLAUDE_2 = "codex-acp/first", "claude-agent-acp/second", "claude-agent-acp/third"


@pytest.mark.parametrize("used", [{CODEX: 5.0, CLAUDE: 80.0}, {CODEX: 5.0, CLAUDE: 94.9},
                                  {CODEX: None, CLAUDE: 60.0}])
def test_a_coordinator_named_eligible_route_with_higher_usage_is_kept(used):
    picked, decision = _rank([CODEX, CLAUDE], used, hint=CLAUDE)
    assert f"{picked.route.engine}/{picked.route.model}" == CLAUDE
    assert decision["model_hint_status"] == "honoured" and decision["decided_by"] == "hint"


def test_a_large_usage_gap_no_longer_outranks_the_works_engine_affinity():
    # Backend work leans to Codex; a 75-point usage lead does not move it.
    picked, decision = _rank([CODEX, CLAUDE], {CODEX: 80.0, CLAUDE: 5.0})
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "affinity"
    assert decision["usage"]["role"] == "tie_breaker" and "margin" not in decision["usage"]
    assert all("usage_band" not in candidate for candidate in decision["candidates"])


def test_a_tie_among_equal_candidates_is_broken_by_usage():
    picked, decision = _rank([CLAUDE, CLAUDE_2], {CLAUDE: 41.0, CLAUDE_2: 40.0}, role="worker-code-reviewer")
    assert f"{picked.route.engine}/{picked.route.model}" == CLAUDE_2 and decision["decided_by"] == "used_percent"
    # Equal owner preference: still only the last tie-breaker.
    preferred = {CLAUDE: ["review"], CLAUDE_2: ["review"]}
    picked, decision = _rank([CODEX, CLAUDE, CLAUDE_2], {CODEX: 0.0, CLAUDE: 41.0, CLAUDE_2: 40.0},
                             role="worker-code-reviewer", preferred=preferred)
    assert f"{picked.route.engine}/{picked.route.model}" == CLAUDE_2 and decision["decided_by"] == "used_percent"
    assert decision["usage"]["scope"] == "preferred"


@pytest.mark.parametrize("other", [50.0, None, 94.0])
def test_a_hot_named_route_is_replaced_visibly(other):
    picked, decision = _rank([CODEX, CLAUDE], {CODEX: other, CLAUDE: 96.0}, hint=CLAUDE)
    assert picked.route.engine == "codex-acp"
    assert decision["model_hint_status"] == "ignored:near_quota_limit"
    assert decision["decided_by"] == "quota"


def test_a_hot_named_route_stays_when_every_alternative_is_hot_too():
    picked, decision = _rank([CODEX, CLAUDE], {CODEX: 95.0, CLAUDE: 99.0}, hint=CLAUDE)
    assert picked.route.engine == "claude-agent-acp" and decision["model_hint_status"] == "honoured"


# --- plan preview agrees with dispatch ---------------------------------------

def _preview(owner, model, role="security-auditor"):
    from types import SimpleNamespace
    from cortex_runtime.orchestration.state import TaskMachine
    from test_runtime_task_machine import plan
    _root, _env, store, task, _used = owner
    machine = TaskMachine(store)
    machine.create(task.task_id, "Audit")
    proposal = plan()
    proposal["tasks"][0].update(role=role, model=model, model_reason="Coordinator recommendation.")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    return machine.submit_plan(channel, proposal, "preview")["model_hints"][0]


def test_the_plan_preview_reports_the_owner_preference_dispatch_applies(owner):
    hint = _preview(owner, OPUS)
    assert hint["preview_status"] == "ignored:owner_preferred"
    assert hint["owner_preferred"] == sorted([SONNET, SOL])
    assert "not a mismatch" in hint["preview_note"]
    dispatch = _choose(owner, model_hint=OPUS, model_reason="Coordinator recommendation.").decision
    assert dispatch["model_hint_status"] == "ignored:owner_preferred"


def test_the_plan_preview_keeps_a_preferred_hint_whatever_its_usage(owner):
    hint = _preview(owner, SONNET)
    assert hint["preview_status"] == "pending_dispatch" and "owner_preferred" not in hint
    dispatch = _choose(owner, model_hint=SONNET, model_reason="Coordinator recommendation.").decision
    assert dispatch["chosen"] == SONNET and dispatch["model_hint_status"] == "honoured"


def test_the_plan_preview_reports_a_hot_hint(owner):
    owner[4]["claude-sonnet-5"] = 97.0
    hint = _preview(owner, SONNET)
    assert hint["preview_status"] == "ignored:near_quota_limit"
    dispatch = _choose(owner, model_hint=SONNET, model_reason="Coordinator recommendation.").decision
    assert dispatch["chosen"] == SOL and dispatch["model_hint_status"] == "ignored:near_quota_limit"


def test_the_plan_preview_follows_the_users_chosen_option(owner):
    _root, _env, store, task, _used = owner
    question = questions_store.create_if_absent(store, task.task_id, "q-audit", QUESTION,
                                                options=[OPUS_OPTION, OTHER_OPTION])
    assert questions_store.answer(store, question.id, OPUS_OPTION, "elicitation")
    hint = _preview(owner, OPUS)
    assert hint["preview_status"] == "pending_dispatch:user_request"
