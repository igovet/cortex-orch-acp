"""Exact quota associations, no native accounts, engine calls or template overlay."""
import copy
import json
from types import SimpleNamespace

import pytest

from cortex_runtime.connections import quota, registry
from cortex_runtime.configuration import management, model_policy_migration as migration
from cortex_runtime.routing import auto_route
from cortex_runtime.portable.errors import PortableError
from test_model_settings_template import entry


ENGINE = "codex-acp"
MODEL = "RENAMED@9"
OTHER = "RENAMED@90"
WINDOW = "Owner window / EXACT@7"


def settings(rules=None):
    return {"model_policy_version": 2, "enabled": {ENGINE: [MODEL, OTHER]},
            "models": {ENGINE: {"auto": [MODEL, OTHER], "tiers": {MODEL: "light", OTHER: "light"}}},
            "quota_windows": {ENGINE: rules if rules is not None else [
                {"window": WINDOW, "models": [MODEL]}, {"window": "shared", "models": None}]}}


def windows(monkeypatch, value):
    monkeypatch.setattr(quota, "_windows_for", lambda *args: value)
    return SimpleNamespace(engine=ENGINE, id="fake")


def test_exact_ids_labels_shared_multiple_windows_and_resets(monkeypatch):
    record = windows(monkeypatch, {WINDOW: {"used": 90, "resets_at": 200},
        "shared": {"used": 30, "resets_at": 300}, "second": {"used": 80, "resets_at": 400}})
    policy = settings([*settings()["quota_windows"][ENGINE], {"window": "second", "models": [MODEL]}])
    assert quota.used_percent(record, MODEL, now=100, routing=policy) == 90
    assert quota.resets_at(record, MODEL, now=100, routing=policy) == 200
    assert quota.used_percent(record, OTHER, now=100, routing=policy) == 30
    assert quota.resets_at(record, OTHER, now=100, routing=policy) == 300
    assert quota.used_percent(record, MODEL, now=201, routing=policy) == 80
    assert quota.used_percent(record, MODEL, now=401, routing=policy) is None
    # No case folding, suffix/family inheritance, or window substring matching.
    assert quota.used_percent(record, MODEL.lower(), now=100, routing=policy) == 30
    renamed = {"owner window / EXACT@7": {"used": 100, "resets_at": 200}}
    windows(monkeypatch, renamed)
    assert quota.used_percent(record, OTHER, now=100, routing=policy) == 100


@pytest.mark.parametrize("policy", [None, {}, {"quota_windows": {}}, settings([])])
def test_missing_empty_and_unmapped_are_conservative_not_free(monkeypatch, policy):
    record = windows(monkeypatch, {"new scoped label": {"used": 100, "resets_at": 200}})
    assert quota.used_percent(record, OTHER, now=100, routing=policy) == 100
    assert quota.resets_at(record, OTHER, now=100, routing=policy) == 200


def test_explicit_empty_scope_unknown_values_and_ignored_paid_overage(monkeypatch):
    record = windows(monkeypatch, {WINDOW: {"used": 100}, "overage": {"used": 100},
                                   "shared": {"used": None, "resets_at": 300}})
    policy = settings([{"window": WINDOW, "models": []}])
    assert quota.used_percent(record, MODEL, now=100, routing=policy) is None
    assert quota.resets_at(record, MODEL, now=100, routing=policy) is None


@pytest.mark.parametrize("value", [None, [], {"unknown-engine": []}, {ENGINE: None},
    {ENGINE: [{"window": "", "models": []}]}, {ENGINE: [{"window": "x", "models": "model"}]},
    {ENGINE: [{"window": "x", "models": [MODEL, MODEL]}]},
    {ENGINE: [{"window": "x", "models": ["model with space"]}]},
    {ENGINE: [{"window": "x", "models": None, "future": 1}]},
    {ENGINE: [{"window": "x", "models": None}, {"window": "x", "models": [MODEL]}]}])
def test_invalid_or_ambiguous_refuses_before_cache_or_engine_io(monkeypatch, value):
    policy = settings()
    policy["quota_windows"] = value
    monkeypatch.setattr(quota, "_windows_for", lambda *a: pytest.fail("provider IO after invalid policy"))
    record = SimpleNamespace(engine=ENGINE)
    with pytest.raises(ValueError): quota.used_percent(record, MODEL, routing=policy)
    with pytest.raises(ValueError): quota.resets_at(record, MODEL, routing=policy)
    with pytest.raises(ValueError): auto_route.validate_routing(policy)
    with pytest.raises(management.ConfigValidationError) as caught: management.validate_routing_rules(policy)
    assert any(item["code"] == "invalid_quota_windows" and item["field"].startswith("rules.quota_windows")
               for item in caught.value.errors)


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    path.write_text(json.dumps(data))
    path.chmod(0o600)


def env(tmp_path):
    return {"HOME": str(tmp_path / "home"), "XDG_CONFIG_HOME": str(tmp_path / "config"),
            "XDG_STATE_HOME": str(tmp_path / "state")}


@pytest.mark.parametrize("shuffle", [False, True])
def test_menu_and_automatic_agree_on_effective_project_policy(tmp_path, monkeypatch, shuffle):
    project = tmp_path / "project"
    environ = env(tmp_path)
    user = registry.config_root(environ) / "routing.json"
    policy = settings()
    write(user, settings([{ "window": WINDOW, "models": [OTHER]}]))
    environment = tmp_path / "environment.json"
    write(environment, settings([{ "window": WINDOW, "models": None}]))
    environ["CORTEX_ROUTING"] = str(environment)
    write(project / ".cortex" / "routing.json", policy)
    candidates = [entry(ENGINE, MODEL, "one"), entry(ENGINE, OTHER, "one")]
    if shuffle: candidates.reverse()
    record = SimpleNamespace(engine=ENGINE, launch_command=("fake",))
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(connections={ENGINE: record}))
    monkeypatch.setattr(auto_route, "_entries", lambda *a: candidates)
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    windows(monkeypatch, {WINDOW: {"used": 100, "resets_at": 10**20}, "shared": {"used": 30}})
    before = (project / ".cortex" / "routing.json").read_bytes()
    menu = auto_route.route_menu(project, environ)["tiers"]["light"]
    assert {r["model"]: r["state"] for r in menu} == {MODEL: "exhausted", OTHER: "available"}
    assert auto_route.choose("worker-explorer", project, environ).model == OTHER
    assert auto_route.route_menu(project, environ)["quota_window_policy"] == {
        "unmapped": "conservative_shared", "rules": policy["quota_windows"]}
    # First-existing-file precedence applies to quota as well as eligibility.
    project_file = project / ".cortex" / "routing.json"
    project_file.rename(project_file.with_suffix(".saved"))
    with pytest.raises(PortableError): auto_route.choose("worker-explorer", project, environ)
    environ.pop("CORTEX_ROUTING")
    assert auto_route.choose("worker-explorer", project, environ).model == MODEL
    project_file.with_suffix(".saved").rename(project_file)
    # Disabling the only open model cannot be bypassed by quota mappings.
    policy["enabled"][ENGINE] = [MODEL]
    write(project / ".cortex" / "routing.json", policy)
    with pytest.raises(PortableError): auto_route.choose("worker-explorer", project, environ)
    policy["quota_windows"][ENGINE].append(copy.deepcopy(policy["quota_windows"][ENGINE][0]))
    write(project / ".cortex" / "routing.json", policy)
    invalid = (project / ".cortex" / "routing.json").read_bytes()
    with pytest.raises(PortableError): auto_route.route_menu(project, environ)
    assert (project / ".cortex" / "routing.json").read_bytes() == invalid
    assert user.read_bytes() != before


def test_versioned_api_profile_snapshot_conflict_and_explicit_migration(tmp_path):
    environ = env(tmp_path)
    policy = settings()
    root = registry.config_root(environ)
    view = management.update_routing("user", "absent", policy, environ=environ)
    assert view["effective_rules"]["quota_windows"] == policy["quota_windows"]
    before = (root / "routing.json").read_bytes()
    with pytest.raises(PortableError): management.update_routing("user", "absent", settings([]), environ=environ)
    assert (root / "routing.json").read_bytes() == before
    def profiles(action, **kwargs):
        result = management.execute({"api_version": 1, "surface": "routing_profiles", "action": "read"}, environ=environ)
        assert result["ok"], result
        current = result["result"]
        response = management.execute(dict(api_version=1, surface="routing_profiles", action=action,
            name="quota-snapshot", expected_revision=current["revision"],
            expected_revisions=current["current_revisions"], **kwargs), environ=environ)
        assert response["ok"], response
        return response["result"]
    profiles("create")
    saved = (root / "routing-profiles.json").read_bytes()
    management.update_routing("user", management._revision(before), settings([]), environ=environ)
    assert (root / "routing-profiles.json").read_bytes() == saved
    profiles("activate", discard_modifications=True)
    assert json.loads((root / "routing.json").read_text())["quota_windows"] == policy["quota_windows"]
    saved = (root / "routing-profiles.json").read_bytes()
    # Explicit completion of missing mappings; no template loading and no silent profile rewrite.
    legacy = settings(); legacy.pop("quota_windows")
    write(root / "routing.json", legacy)
    candidate = tmp_path / "candidate.json"; write(candidate, policy)
    preview = migration.preview(candidate, environ=environ)
    assert (root / "routing-profiles.json").read_bytes() == saved
    migration.apply(candidate, preview["expected_revision"], preview["candidate_sha256"], consent=True, environ=environ)
    assert json.loads((root / "routing.json").read_text())["quota_windows"] == policy["quota_windows"]


@pytest.mark.parametrize("invalid", [False, True])
def test_failure_reset_uses_project_policy_and_invalid_policy_retains_refusal(tmp_path, monkeypatch, invalid):
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.routing import availability
    environ = env(tmp_path)
    project = tmp_path / "project"
    policy = settings()
    if invalid: policy["quota_windows"] = None
    write(project / ".cortex" / "routing.json", policy)
    adapter = SimpleNamespace(_project_root_factory=lambda _: project, _workspace_root_factory=lambda _: pytest.fail("wrong root"),
                              _environ=environ, _store=object())
    record = windows(monkeypatch, {WINDOW: {"used": 100, "resets_at": 10**20},
                                   "shared": {"used": 30, "resets_at": 10**21}})
    captured = []
    monkeypatch.setattr(availability, "record", lambda *a, **kw: captured.append(kw))
    EngineSessionManagerAdapter._record_route_failure(adapter,
        (availability.QUOTA_EXHAUSTED, "model", "engine-reported"), record, OTHER, "task", "attempt")
    assert len(captured) == 1
    assert captured[0]["resets_at"] == (None if invalid else 10**21)
    assert captured[0]["reason"] == availability.QUOTA_EXHAUSTED


def test_install_only_rules_preserve_old_scoping_for_retained_exact_identities(monkeypatch):
    from cortex_runtime.routing import defaults
    policy = defaults.default_routing()
    configured = policy["enabled"]["claude-agent-acp"]
    families = ("opus", "sonnet", "haiku", "fable")  # Historical test oracle, not runtime rules.
    record = SimpleNamespace(engine="claude-agent-acp")
    for model in configured:
        for family in families:
            label = "seven_day_" + family
            windows(monkeypatch, {label: {"used": 90, "resets_at": 200}, "seven_day": {"used": 30, "resets_at": 300}})
            expected = 90 if family in model else 30
            assert quota.used_percent(record, model, now=100, routing=policy) == expected
            assert quota.resets_at(record, model, now=100, routing=policy) == (200 if expected == 90 else 300)


def test_associations_are_engine_specific_and_do_not_enlarge_unknown_model_eligibility(monkeypatch):
    record = windows(monkeypatch, {WINDOW: {"used": 100, "resets_at": 200}})
    policy = settings()
    assert quota.used_percent(record, "WITHDRAWN@9", now=100, routing=policy) is None
    # A different engine has no association for this exact label: shared, not exempt.
    record.engine = "claude-agent-acp"
    assert quota.used_percent(record, "WITHDRAWN@9", now=100, routing=policy) == 100
