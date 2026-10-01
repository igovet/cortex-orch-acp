"""Strict v2 boundaries, exercised without accounts or paid engines."""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route, model_policy
from cortex_runtime.store import Store
from test_model_settings_template import entry


def settings(model="random-id", *, tier="light", attributes=None):
    return {"model_policy_version": 2, "enabled": {"codex-acp": [model]},
            "models": {"codex-acp": {"auto": [model], "tiers": {model: tier},
                                       "policy": {model: attributes or {}}}}}


def install(tmp_path, data):
    (tmp_path / ".cortex").mkdir(mode=0o700, exist_ok=True)
    file = tmp_path / ".cortex" / "routing.json"
    file.write_text(json.dumps(data))
    return {"HOME": str(tmp_path)}


def fake_catalog(monkeypatch, entries):
    from cortex_runtime.connections import quota, registry
    connections = {e.connection_id: SimpleNamespace(launch_command=("fake",)) for e in entries}
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(connections=connections))
    monkeypatch.setattr(auto_route, "_entries", lambda *_: entries)
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_, **_kwargs: None)


@pytest.mark.parametrize("version,code", [(None,"model_policy_migration_required"),
    (1,"model_policy_version_unsupported"), (99,"model_policy_version_unsupported"),
    (True,"model_policy_version_unsupported")])
def test_versions_require_explicit_migration_or_repair(tmp_path, version, code):
    data = settings()
    if version is None: data.pop("model_policy_version")
    else: data["model_policy_version"] = version
    env = install(tmp_path, data)
    with pytest.raises(PortableError, match=code): auto_route.load_overrides(tmp_path, env)
    assert auto_route.is_denied("codex-acp", "random-id", tmp_path, env)


@pytest.mark.parametrize("policy", [{"auto": ["random-id"]}, {"auto": ["random-id"], "tiers": {"random-id": []}}])
def test_automatic_models_need_nonempty_explicit_tiers(tmp_path, policy):
    data = settings()
    data["models"]["codex-acp"] = policy
    env = install(tmp_path, data)
    with pytest.raises(PortableError) as error: auto_route.load_overrides(tmp_path, env)
    assert error.value.code in {"policy_missing", "invalid_routing_overrides"}


@pytest.mark.parametrize("change", ["unlisted", "auto-empty", "no-auto", "engine-missing", "deny", "provider"])
def test_confirmed_but_no_policy_eligible_routes_never_inherit_coordinator(tmp_path, monkeypatch, change):
    e = entry("codex-acp", "random-id")
    fake_catalog(monkeypatch, [e])
    data = settings()
    if change == "unlisted": data["enabled"]["codex-acp"] = ["withdrawn-id"]
    elif change == "auto-empty": data["models"]["codex-acp"]["auto"] = []
    elif change == "no-auto": data["models"]["codex-acp"].pop("auto")
    elif change == "engine-missing": data["enabled"] = {}
    elif change == "deny": data["deny"] = ["random-id"]
    else: data["providers"] = ["claude-agent-acp"]
    env = install(tmp_path, data)
    with pytest.raises(PortableError, match="no_available_route"):
        auto_route.choose("worker-explorer", tmp_path, env)


def test_no_confirmed_catalog_may_retain_existing_inheritance_only(tmp_path, monkeypatch):
    fake_catalog(monkeypatch, [])
    env = install(tmp_path, settings())
    assert auto_route.choose("worker-explorer", tmp_path, env) is None


@pytest.mark.parametrize("selector,engine,model,matched", [
    ("unit", "codex-acp", "unit", True), ("unit", "codex-acp", "unit[1m]", False),
    ("unit", "codex-acp", "vendor-unit-9", False), ("unit", "codex-acp", "Unit", False),
    ("codex-acp/unit", "claude-agent-acp", "unit", False),
    ("opencode/vendor/id,thinking", "opencode", "vendor/id,thinking", True),
    ("opencode/*", "opencode", "any/new-model", True),
])
def test_selectors_are_exact_not_family_or_variant_patterns(selector, engine, model, matched):
    assert auto_route._listed([selector], engine, model) == matched


@pytest.mark.parametrize("fields,offered", [
    ({"effort_floor": "high"}, ("low", "medium")),
    ({"effort_ladder": ["high", "xhigh", "max"]}, ("high", "xhigh")),
    ({"effort_floor": "xhigh"}, ("xhigh",)),
    ({"max_effort": "medium"}, ("high",)),
])
def test_unsupported_effort_requirements_fail_visibly(fields, offered):
    p = model_policy.policy_for("codex-acp", "random-id", settings(tier="flagship", attributes=fields))
    with pytest.raises(PortableError, match="model_effort_policy_unsupported"):
        auto_route.permitted_efforts(entry("codex-acp", "random-id", offered), "flagship", p)


@pytest.mark.parametrize("pin", [False, True])
def test_ladder_step_conflicting_with_ceiling_never_silently_weakens(tmp_path, monkeypatch, pin):
    e = entry("codex-acp", "random-id")
    fake_catalog(monkeypatch, [e])
    data = settings(attributes={"effort_floor": "high", "effort_ladder": ["high", "xhigh", "max"],
                                "also_standard_for": ["testing"], "max_effort": "high"})
    if pin: data["pin"] = {"*": "codex-acp/random-id"}
    env = install(tmp_path, data)
    with pytest.raises(PortableError, match="model_effort_policy_unsupported"):
        auto_route.choose("worker-qa-engineer", tmp_path, env, escalate=True)


def test_duplicate_request_labels_refuse_even_across_engines(tmp_path):
    data = settings(attributes={"request_names": ["star"]})
    data["models"]["claude-agent-acp"] = {"policy": {"different-id": {"request_names": ["star"]}}}
    env = install(tmp_path, data)
    with pytest.raises(PortableError, match="model_request_label_ambiguous"):
        auto_route.load_overrides(tmp_path, env)


def test_overlapping_request_labels_refuse_from_real_source(tmp_path):
    data = settings(attributes={"request_names": ["star"]})
    data["models"]["claude-agent-acp"] = {"policy": {"different-id": {"request_names": ["stars"]}}}
    install(tmp_path, data)
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="initial"), "Use stars here", "owner")
    before = (tmp_path / ".cortex" / "routing.json").read_bytes()
    with store.connection() as db:
        sources = list(auto_route._user_texts(store, db, task.task_id))
    with pytest.raises(PortableError, match="model_request_label_ambiguous"):
        auto_route.user_model_request(store, task.task_id, "codex-acp/random-id")
    assert (tmp_path / ".cortex" / "routing.json").read_bytes() == before
    with store.connection() as db:
        assert list(auto_route._user_texts(store, db, task.task_id)) == sources


@pytest.mark.parametrize("text,expected", [("Use codex-acp/random-id", True), ("Use codex-acp/RANDOM-ID", False),
    ("Use random-id", False), ("Use star", True), ("Don't use star", False),
    ("Compare star with other", False), ("The tests run on star", False)])
def test_flagship_requests_bind_exact_settings_identity_and_source(tmp_path, text, expected):
    install(tmp_path, settings(tier="flagship", attributes={"request_names": ["star"]}))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="initial"), text, "owner")
    grant = auto_route.user_model_request(store, task.task_id, "codex-acp/random-id")
    assert (grant is not None) == expected
    if grant:
        assert grant["family"] == "codex-acp/random-id" and grant["source_ref"]


def test_first_existing_precedence_is_not_a_merge(tmp_path):
    env = {"HOME": str(tmp_path), "XDG_CONFIG_HOME": str(tmp_path / "xdg"), "CORTEX_ROUTING": str(tmp_path / "env.json")}
    user = tmp_path / "xdg" / "cortex" / "routing.json"
    user.parent.mkdir(parents=True)
    user.write_text(json.dumps(settings("user-id")))
    Path(env["CORTEX_ROUTING"]).write_text(json.dumps(settings("env-id")))
    assert auto_route.load_overrides(tmp_path, env)["enabled"]["codex-acp"] == ["env-id"]
    install(tmp_path, settings("project-id"))
    assert auto_route.load_overrides(tmp_path, env)["enabled"]["codex-acp"] == ["project-id"]
    (tmp_path / ".cortex" / "routing.json").write_text("{}")
    with pytest.raises(PortableError, match="model_policy_migration_required"):
        auto_route.load_overrides(tmp_path, env)


def test_callers_cannot_supply_unconfigured_request_labels(tmp_path):
    install(tmp_path, settings(tier="flagship"))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="initial"), "Use imaginary here", "owner")
    with pytest.raises(PortableError, match="model_request_labels_changed"):
        auto_route.user_model_request(store, task.task_id, "codex-acp/random-id", names=["imaginary"])


@pytest.mark.parametrize("identity", ["codex-acp/Unit-7", "opencode/vendor/nested/Unit,7[wide]"])
@pytest.mark.parametrize("prefix,suffix", [
    ("", "-extended"), ("", ".v2"), ("", "[1m]"), ("", "/next"),
    ("", ",thinking"), ("", ":latest"), ("", "_extra"), ("", "+extra"),
    ("", "@extra"), ("", "#extra"), ("", "'s"), ("", "(extra)"),
    ("-", ""), (".", ""), ("[", "]"), ("provider/", ""), ("prefix:", ""),
    ("!", ""), ("+", ""), ("@", ""), (",", ""),
    # Without explicit quoting even terminal punctuation is ambiguous ID data.
    ("", "."), ("", ","), ("", "!"), ("", "?"), ("", ";"), ("", ":"),
    ('"', '"-extended'), ("'", "'[1m]"), ('prefix"', '"'),
    ('"', '-extended"'), ('"', '.v2"'), ('"', '[1m]"'),
    ('"', "'"), ('"', ''), ('', '"'), ('(', ')'),
])
def test_exact_request_never_truncates_an_opaque_identity(identity, prefix, suffix):
    assert auto_route.requests_family(f"Use {prefix}{identity}{suffix}", identity) is None


@pytest.mark.parametrize("identity", [
    "codex-acp/Unit-7", "codex-acp/Unit-7-extended", "codex-acp/Unit-7.v2",
    "codex-acp/Unit-7[1m]", "opencode/vendor/nested/Unit,7[wide]",
    "opencode/.Unit-7", "opencode/Unit-7.",
])
@pytest.mark.parametrize("template", [
    "Use {}", "Please use {} here", "Use: {}", 'Use "{}".', "Use '{}'!",
    "Use `{}`; then continue", "Use «{}»?", "Use “{}”, please",
])
def test_complete_exact_requests_and_explicit_quotes(identity, template):
    assert auto_route.requests_family(template.format(identity), identity) == identity


@pytest.mark.parametrize("variant", ["-extended", ".v2", "[1m]", "/nested", ",thinking"])
def test_shorter_id_does_not_authorize_its_configured_variant(variant):
    base = "opencode/vendor/Unit-7"
    assert auto_route.requests_family(f"Use {base}", base + variant) is None
    assert auto_route.requests_family(f"Use {base + variant}", base + variant) == base + variant


@pytest.mark.parametrize("text", [
    "Use codex-acp/unit-7", "Use codex-acp/UNIT-7", "Use CODEX-ACP/Unit-7",
    "Use Unit-7", "Don't use codex-acp/Unit-7", "Never use codex-acp/Unit-7",
    "Cancel the request for codex-acp/Unit-7", "Compare codex-acp/Unit-7 with another",
    "The tests run on codex-acp/Unit-7", "codex-acp/Unit-7",
    "Use:\ncodex-acp/Unit-7",  # Request words cannot cross the existing sentence boundary.
])
def test_exact_request_keeps_case_and_existing_request_grammar(text):
    assert auto_route.requests_family(text, "codex-acp/Unit-7") is None


@pytest.mark.parametrize("variant", ["-extended", ".v2", "[1m]", "/nested", ",thinking"])
@pytest.mark.parametrize("requested_variant", [False, True])
def test_retained_request_selects_only_its_complete_configured_identity(tmp_path, monkeypatch, variant, requested_variant):
    base = "vendor/Unit-7"
    models = [base, base + variant, "vendor/ordinary"]
    data = {"model_policy_version": 2, "enabled": {"opencode": models},
            "models": {"opencode": {"auto": models, "tiers": {
                base: "flagship", base + variant: "flagship", "vendor/ordinary": "standard"}}}}
    env = install(tmp_path, data)
    fake_catalog(monkeypatch, [entry("opencode", model) for model in models])
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    requested = base + variant if requested_variant else base
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="variant"),
                          f'Use "opencode/{requested}".', "owner")
    settings_before = (tmp_path / ".cortex" / "routing.json").read_bytes()
    with store.connection() as db:
        source_before = list(auto_route._user_texts(store, db, task.task_id))
    for model in (base, base + variant):
        identity = f"opencode/{model}"
        grant = auto_route.user_model_request(store, task.task_id, identity)
        choice = auto_route.choose("worker-backend-dev", tmp_path, env, model_hint=identity, user_request=grant)
        if model == requested:
            assert grant == dict(source_ref=source_before[0][0], family=identity, matched=identity)
            assert choice.model == model and choice.decision["tier"] == "flagship"
        else:
            assert grant is None
            assert choice.model == "vendor/ordinary"
    assert (tmp_path / ".cortex" / "routing.json").read_bytes() == settings_before
    with store.connection() as db:
        assert list(auto_route._user_texts(store, db, task.task_id)) == source_before


@pytest.mark.parametrize("restriction,status", [
    ("deny", "refused:denied"), ("disabled", "refused:denied"),
    ("unselected", "refused:not_selected"), ("missing-policy", "not_flagship"),
    ("unobserved", "refused:not_offered"), ("provider", "refused:provider_not_allowed"),
    ("unavailable", "refused:unavailable"), ("exhausted", "refused:quota_exhausted"),
])
def test_exact_grant_cannot_override_eligibility(tmp_path, monkeypatch, restriction, status):
    from cortex_runtime.connections import quota

    target = "codex-acp/Unit-7"
    data = settings("Unit-7", tier="flagship")
    data["enabled"]["claude-agent-acp"] = ["ordinary-id"]
    data["models"]["claude-agent-acp"] = {"auto": ["ordinary-id"], "tiers": {"ordinary-id": "standard"}}
    observed = [entry("codex-acp", "Unit-7"), entry("claude-agent-acp", "ordinary-id")]
    if restriction == "deny": data["deny"] = [target]
    elif restriction == "disabled": data["enabled"]["codex-acp"] = []
    elif restriction == "unselected": data["models"]["codex-acp"]["auto"] = []
    elif restriction == "missing-policy": data["models"].pop("codex-acp")
    elif restriction == "unobserved": observed.pop(0)
    elif restriction == "provider": data["providers"] = ["claude-agent-acp"]
    env = install(tmp_path, data)
    fake_catalog(monkeypatch, observed)
    if restriction == "exhausted":
        monkeypatch.setattr(quota, "used_percent", lambda _connection, model, *_, **_kwargs: 100.0 if model == "Unit-7" else None)
    unavailable = [{"connection_id": "codex-acp", "model": "Unit-7"}] if restriction == "unavailable" else ()
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="exact"), f"Use {target}", "owner")
    grant = auto_route.user_model_request(store, task.task_id, target)
    assert grant is not None
    choice = auto_route.choose("worker-backend-dev", tmp_path, env, model_hint=target,
                              user_request=grant, unavailable=unavailable)
    assert choice.model == "ordinary-id"
    assert choice.decision["user_model_request"]["status"] == status


@pytest.mark.parametrize("answer,expected", [
    ('Use "codex-acp/Unit-7".', True), ("Use codex-acp/Unit-7.v2", False),
    ("Don't use codex-acp/Unit-7", False), ("Continue", False),
])
def test_exact_user_answer_keeps_answer_provenance_not_question_text(tmp_path, answer, expected):
    from cortex_runtime.acp import questions_store

    target = "codex-acp/Unit-7"
    install(tmp_path, settings("Unit-7", tier="flagship"))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="initial"), "Ordinary work", "owner")
    question = questions_store.create_if_absent(store, task.task_id, "q-exact", f"Use {target}")
    assert auto_route.user_model_request(store, task.task_id, target) is None
    assert questions_store.answer(store, question.id, answer, "text")
    grant = auto_route.user_model_request(store, task.task_id, target)
    assert (grant is not None) == expected
    if expected:
        assert grant == dict(source_ref=questions_store.answer_ref(question.id), family=target, matched=target)
