"""Settings-only install/binding/migration/deletion, in disposable no-paid fixtures."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.acp import config_options
from cortex_runtime.catalog import discovery
from cortex_runtime.catalog.evidence import EvidenceState, Fact, Provenance, ProvenanceSource
from cortex_runtime.configuration import management as m, model_policy_migration as migration, routing_profiles
from cortex_runtime.configuration.resolution import resolve_portable_coordinator, resolve_owner_coordinator
from cortex_runtime.connections import registry
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import defaults


@pytest.fixture
def env(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    return {"HOME": str(home), "XDG_CONFIG_HOME": str(tmp_path / "config"), "XDG_STATE_HOME": str(tmp_path / "state")}


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.parent.name == "cortex": path.parent.chmod(0o700)
    path.write_text(value if isinstance(value, str) else json.dumps(value))
    path.chmod(0o600)
    return path


def bytes_of(root):
    return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob("*") if p.is_file()}


def rules():
    return {"model_policy_version": 2, "enabled": {"codex-acp": ["custom"]}, "providers": ["codex-acp"],
            "models": {"codex-acp": {"auto": ["custom"], "tiers": {"custom": "deep"}}}}


def portable():
    return {"engine": "codex-acp", "model": "custom", "parameters": {"thought_level": "high", "mode": "agent"}}


def entry(connection="one", model="custom", parameters=(("thought_level", "high"), ("mode", "agent"))):
    evidence = EvidenceState.empty().promote(Fact.CATALOG_OBTAINED, Provenance(ProvenanceSource.DISCOVERY, "test", "2026-09-30T00:00:00Z"))
    return SimpleNamespace(evidence=evidence, connection_id=connection, route=SimpleNamespace(
        engine="codex-acp", model=model, id=connection + "/" + model, supported_parameters=parameters))


@pytest.mark.parametrize("original", ['{}', '{bad', '{"defaults":{"language":"ru"},"unknown":true}',
                                      '{"defaults":{"route_id":"xr_saved","route_model":"saved"}}'])
def test_install_existing_defaults_and_profiles_remain_exact(env, original):
    root = registry.config_root(env)
    write(root / "defaults.json", original)
    write(root / "routing-profiles.json", "saved snapshot bytes")
    result = defaults.apply(env)
    assert result["routing"]["status"] == "created" and not result["defaults"]["changed"]
    assert (root / "defaults.json").read_text() == original
    assert (root / "routing-profiles.json").read_text() == "saved snapshot bytes"


def test_install_absent_portable_defaults_no_binding_or_discovery(env, monkeypatch):
    monkeypatch.setattr(registry, "inspect_registry", lambda *a, **k: pytest.fail("registry discovery forbidden during install"))
    result = defaults.apply(env)
    root = registry.config_root(env)
    assert result["defaults"]["status"] == "created"
    assert json.loads((root / "defaults.json").read_text()) == {"defaults": {
        "coordinator": {"engine": "claude-agent-acp", "model": "opus[1m]",
                        "parameters": {"thought_level": "medium"}}, "language": "ru"}}
    assert json.loads((root / "routing.json").read_text()) == defaults.default_routing()
    assert not (root / "routing-profiles.json").exists()
    assert (root / "defaults.json").stat().st_mode & 0o777 == 0o600
    assert defaults.apply(env)["defaults"]["changed"] is False
    assert not (root / "connections.json").exists()


@pytest.mark.parametrize("target", ["root", "ancestor", "defaults"])
def test_installer_refuses_symlink_storage(env, tmp_path, target):
    root = registry.config_root(env)
    external = tmp_path / "external"
    external.mkdir()
    write(external / "owner", "unchanged")
    if target == "root":
        root.parent.mkdir()
        root.symlink_to(external)
    elif target == "ancestor":
        root.parent.symlink_to(external)
    else:
        root.mkdir(parents=True)
        (root / "defaults.json").symlink_to(external / "owner")
    before = bytes_of(external)
    result = defaults.apply(env)
    assert any(v.get("status") == "unsafe_storage" for v in result.values())
    assert bytes_of(external) == before


def test_full_asset_invalid_before_setup_initialization(env, tmp_path, monkeypatch):
    import cortex_acp_setup as setup
    for key, val in env.items(): monkeypatch.setenv(key, val)
    asset = write(tmp_path / "invalid.json", {"schema_version": 1, "routing": rules(), "coordinator": {}})
    monkeypatch.setattr(defaults, "TEMPLATE_PATH", asset)
    with pytest.raises(ValueError): setup.init_storage()
    assert not registry.config_root(env).exists() and not registry.state_root(env).exists()


def test_git_bootstrap_payload_contains_public_asset_and_migration_module():
    import cortex_acp_setup as setup
    paths = {p for p, _ in setup._paseo_runtime_payload()}
    assert "policy/model-settings-default.json" in paths
    assert "src/cortex_runtime/configuration/model_policy_migration.py" in paths


def test_racing_defaults_creation_never_replaced(env, monkeypatch):
    real = defaults.os.link
    root = registry.config_root(env)
    def race(source, destination):
        if Path(destination).name == "defaults.json": write(root / "defaults.json", "owner race")
        return real(source, destination)
    monkeypatch.setattr(defaults.os, "link", race)
    result = defaults.apply(env)
    assert result["defaults"]["status"] == "existing" and not result["defaults"]["changed"]
    assert (root / "defaults.json").read_text() == "owner race"
    assert not list(root.glob("*.tmp"))


@pytest.mark.parametrize("entries,code", [([], "portable_coordinator_unavailable"),
    ([entry(model="different")], "portable_coordinator_unavailable"),
    ([entry(), entry("two")], "portable_coordinator_ambiguous"),
    ([entry(), entry()], "portable_coordinator_ambiguous"),
    ([entry(parameters=(("thought_level", "low"),))], "unsupported_dependent_setting")])
def test_portable_binding_refuses_missing_near_duplicate_and_unsupported(entries, code):
    with pytest.raises(PortableError) as error: resolve_portable_coordinator(entries, portable())
    assert error.value.code == code


def test_portable_binding_returns_only_exact_observed_route():
    selected = entry()
    assert resolve_portable_coordinator([entry(model="near"), selected], portable()) is selected


def test_mutable_portable_defaults_preserve_saved_owner_state(env):
    root = registry.config_root(env)
    write(root / "defaults.json", {"defaults": {"language": "ru", "future": 1}, "ceiling": m.ceiling_to_dict(m.PolicyCeiling()), "unknown": 2})
    original = m.read_user_defaults(env)
    result = m.update_user_defaults(original["revision"], defaults={"coordinator": portable()}, environ=env)
    assert result["defaults"]["future"] == 1 and result["defaults"]["language"] == "ru"
    assert json.loads((root / "defaults.json").read_text())["unknown"] == 2
    view = m.read_user_defaults(env)
    assert view["coordinator_resolution"]["state"] == "unavailable"
    before = (root / "defaults.json").read_bytes()
    with pytest.raises(PortableError): resolve_owner_coordinator(portable(), env)
    assert (root / "defaults.json").read_bytes() == before


def test_unavailable_portable_picker_metadata_never_claims_substitute_is_saved():
    from test_acp_config_options import _entry_with_model_thought_levels
    current = dict(route_id=None, state="default", parameters=portable()["parameters"], coordinator=portable(),
                   resolution_error="portable_coordinator_ambiguous")
    options = config_options.build_config_options([_entry_with_model_thought_levels("cn_" + "a" * 12, "some-other", ("low", "high"))], {}, current)
    for option in options:
        if option["category"] not in ("provider", "model", "thought_level"): continue
        assert option["_meta"]["_cortex"]["state"] == "default_unavailable"
        assert option["_meta"]["_cortex"]["configured_coordinator"] == portable()


def migration_source(env, tmp_path):
    root = registry.config_root(env)
    legacy = {"deny": ["withdrawn"], "providers": ["codex-acp"], "enabled": {"opencode": []},
              "auto_allow": [], "models": {"codex-acp": {"policy": {"custom": {"preferred_for": ["review"]}}}},
              "unknown": {"owner": True}}
    write(root / "routing.json", legacy)
    candidate = deepcopy(legacy)
    candidate.update(rules())
    candidate["enabled"]["opencode"] = []
    candidate["models"]["codex-acp"]["policy"] = deepcopy(legacy["models"]["codex-acp"]["policy"])
    path = write(tmp_path / "candidate.json", candidate)
    return root, path, candidate


def test_migration_preview_consent_digests_preservation_and_private_backup(env, tmp_path):
    root, path, candidate = migration_source(env, tmp_path)
    write(root / "defaults.json", {"defaults": {"route_id": "exact", "parameters": {"thought_level": "high"}}, "consent": False})
    before = bytes_of(root)
    preview = migration.preview(path, environ=env)
    assert bytes_of(root) == before and not preview["applied"]
    with pytest.raises(PortableError, match="migration_consent_required"):
        migration.apply(path, preview["expected_revision"], preview["candidate_sha256"], environ=env)
    result = migration.apply(path, preview["expected_revision"], preview["candidate_sha256"], consent=True, environ=env)
    assert result["applied"] and json.loads((root / "routing.json").read_text()) == candidate
    assert Path(result["backup"]).read_bytes() == before["routing.json"]
    assert Path(result["backup"]).stat().st_mode & 0o777 == 0o600
    assert (root / "defaults.json").read_bytes() == before["defaults.json"]


@pytest.mark.parametrize("conflict", ["source", "candidate", "deny", "preference", "disabled", "unknown"])
def test_migration_conflicts_never_overwrite_owner(env, tmp_path, conflict):
    root, path, candidate = migration_source(env, tmp_path)
    preview = migration.preview(path, environ=env)
    if conflict == "source": write(root / "routing.json", {"deny": ["new owner"]})
    elif conflict == "candidate": path.write_text(path.read_text() + "\n")
    elif conflict == "deny": candidate["deny"] = []
    elif conflict == "preference": candidate["models"]["codex-acp"]["policy"]["custom"]["preferred_for"] = []
    elif conflict == "disabled": candidate["enabled"]["opencode"] = ["provider/new"]
    elif conflict == "unknown": candidate.pop("unknown")
    if conflict not in ("source", "candidate"): write(path, candidate)
    before = (root / "routing.json").read_bytes()
    with pytest.raises(PortableError):
        migration.apply(path, preview["expected_revision"], preview["candidate_sha256"], consent=True, environ=env)
    assert (root / "routing.json").read_bytes() == before
    assert not (root / ".migration-backups").exists()


def test_migration_verification_failure_rolls_back_exact_bytes(env, tmp_path, monkeypatch):
    root, path, candidate = migration_source(env, tmp_path)
    before = (root / "routing.json").read_bytes()
    preview = migration.preview(path, environ=env)
    real = m._replace_file
    def corrupt(target, body):
        real(target, body + b" " if b'"model_policy_version"' in body else body)
    monkeypatch.setattr(m, "_replace_file", corrupt)
    with pytest.raises(PortableError, match="migration_verification_failed"):
        migration.apply(path, preview["expected_revision"], preview["candidate_sha256"], consent=True, environ=env)
    assert (root / "routing.json").read_bytes() == before


def profile_action(env, action, name=None, **extra):
    view = routing_profiles.read_profiles(environ=env)
    request = dict(surface="routing_profiles", action=action, expected_revision=view["revision"], **extra)
    if name is not None: request["name"] = name
    if action in ("create", "save", "activate"): request.setdefault("expected_revisions", view["current_revisions"])
    return m.execute(request, environ=env)


@pytest.mark.parametrize("deleting", ["inactive", "selected", "last"])
def test_delete_snapshot_only_preserves_all_working_bytes_and_divergence(env, deleting):
    root = registry.config_root(env)
    write(root / "routing.json", rules())
    write(root / "defaults.json", {"defaults": {"coordinator": portable(), "language": "ru"}, "unknown": 2})
    assert profile_action(env, "create", "first")["ok"]
    if deleting != "last": assert profile_action(env, "create", "second")["ok"]
    # Modified current defaults must stay modified; deletion cannot revert or auto-select.
    view = m.read_user_defaults(env)
    m.update_user_defaults(view["revision"], defaults={"parameters": {"thought_level": "low"}}, environ=env)
    before = bytes_of(root)
    target = "first" if deleting != "selected" else "second"
    result = profile_action(env, "delete", target)
    assert result["ok"], result
    after = bytes_of(root)
    assert {k: v for k, v in before.items() if k != "routing-profiles.json"} == {k: v for k, v in after.items() if k != "routing-profiles.json"}
    expected = "second" if deleting == "inactive" else None
    assert result["result"]["active"] == expected
    if expected is None: assert result["result"]["applied"] is None and result["result"]["state"] == "none"
    else: assert result["result"]["state"] == "modified"


@pytest.mark.parametrize("failure", ["unknown", "stale", "pending", "unsafe", "write"])
def test_delete_refusals_preserve_snapshot_and_working_state(env, failure, monkeypatch):
    root = registry.config_root(env)
    write(root / "routing.json", rules())
    assert profile_action(env, "create", "first")["ok"]
    path = root / "routing-profiles.json"
    if failure == "pending":
        doc = json.loads(path.read_text());doc["pending"] = {"operation": "activate", "profile": "first", "at": "now"};write(path, doc)
    if failure == "unsafe": path.chmod(0o644)
    before = bytes_of(root)
    if failure == "write": monkeypatch.setattr(m, "_replace_file", lambda *a: (_ for _ in ()).throw(OSError("interrupted")))
    request = dict(surface="routing_profiles", action="delete", name="missing" if failure == "unknown" else "first",
                   expected_revision="sha256:" + "0" * 64 if failure == "stale" else m._revision(path.read_bytes()))
    result = m.execute(request, environ=env)
    assert not result["ok"]
    assert result["error"]["code"] == {"unknown": "profile_not_found", "stale": "revision_conflict", "pending": "profile_interrupted", "unsafe": "unsafe_storage", "write": "profile_delete_failed"}[failure]
    assert bytes_of(root) == before


def test_legacy_profile_activation_refuses_until_its_own_explicit_candidate(env, tmp_path):
    root, candidate, _ = migration_source(env, tmp_path)
    legacy = json.loads((root / "routing.json").read_text())
    snapshot = dict(description="legacy", sections=["routing"], settings={"routing": legacy})
    write(root / "routing-profiles.json", dict(format=2, active=None, profiles={"first": snapshot, "inactive": snapshot}))
    with m._owner_config_lock(env): pass
    before = bytes_of(root)
    result = profile_action(env, "activate", "first")
    assert result["error"]["code"] == "model_policy_migration_required" and bytes_of(root) == before
    preview = migration.preview(candidate, environ=env, profile="first")
    assert bytes_of(root) == before
    migration.apply(candidate, preview["expected_revision"], preview["candidate_sha256"], consent=True, environ=env, profile="first")
    doc = json.loads((root / "routing-profiles.json").read_text())
    assert doc["profiles"]["inactive"] == json.loads(before["routing-profiles.json"])["profiles"]["inactive"]
    assert (root / "routing.json").read_bytes() == before["routing.json"]
    assert profile_action(env, "activate", "inactive")["error"]["code"] == "model_policy_migration_required"
    assert profile_action(env, "activate", "first")["ok"]


def test_generic_metadata_complete_v2_round_trip_and_named_missing_failure(env):
    root = registry.config_root(env)
    result = m.update_routing("user", "absent", rules(), environ=env)
    assert result["effective_rules"] == rules()
    assert result["builtin"]["model_policy_version"] == 2
    assert result["builtin"]["default_user_deny"] == [] and result["builtin"]["default_auto_deny_families"] == []
    with pytest.raises(PortableError, match="model_policy_migration_required"):
        m.update_routing("user", result["written_revision"], {"deny": []}, environ=env)
    assert json.loads((root / "routing.json").read_text()) == rules()


def observed_owner(env, tmp_path):
    from dataclasses import replace
    from test_acp_config_options import _configured_connection, _observation_raw
    from cortex_runtime.catalog import store as catalog_store
    record = _configured_connection(environ=env)
    record = replace(record, launch_command=("/never-run-engine",))
    with registry.transaction(env) as state: state.connections[record.id] = record
    observation = discovery.parse_observation(_observation_raw("custom") + [
        dict(id="mode", name="Mode", category="mode", type="select", currentValue="agent",
             options=[dict(value="agent", name="Agent")])])
    snapshot = discovery.build_catalog({record.id: record}, {record.id: observation})
    catalog_store.save_snapshot(snapshot, env)
    root = registry.config_root(env)
    write(root / "routing.json", rules())
    write(root / "defaults.json", {"defaults": {"coordinator": portable(), "language": "ru"}, "consent": False})
    return record, snapshot.entries[0]


def test_fresh_agent_portable_first_revision_exact_observed_identity_and_parameters(env, tmp_path, monkeypatch):
    from test_acp_config_options import _new_agent, _bootstrap_task_for_session
    from cortex_runtime.configuration.service import ConfigurationService
    record, selected = observed_owner(env, tmp_path)
    monkeypatch.setattr("cortex_runtime.catalog.refresh.perform_refresh", lambda *a, **k: pytest.fail("engine refresh forbidden"))
    root = registry.config_root(env)
    before = bytes_of(root)
    assert resolve_owner_coordinator(portable(), env).route.id == selected.route.id
    agent, _ = _new_agent(tmp_path, env)
    project = tmp_path / "project";project.mkdir()
    response = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session = agent.sessions[response["sessionId"]]
    assert session["default_selection"]["route_id"] == selected.route.id
    store, task_id = _bootstrap_task_for_session(agent, response["sessionId"], tmp_path)
    assert agent._propose_pending_selection_for_new_task(store, session, task_id)
    history = ConfigurationService(store).history(task_id)
    assert len(history) == 1 and history[0]["route_id"] == selected.route.id
    assert history[0]["route_connection_id"] == record.id and history[0]["parameters"] == portable()["parameters"]
    assert bytes_of(root) == before


def test_owner_coordinator_binds_outside_worker_providers(env, tmp_path, monkeypatch):
    """A uniquely observed, enabled, non-denied coordinator outside automatic-worker
    ``providers`` must still bind for management diagnostics, picker metadata and the
    first configuration revision -- ``providers`` scopes automatic worker routing,
    failover and pins (see ``routing.auto_route.choose``/``rank`` and the dispatch-time
    ``route_provider_excluded`` check), not manual/portable coordinator selection.
    """
    from test_acp_config_options import _new_agent, _bootstrap_task_for_session
    from cortex_runtime.configuration.service import ConfigurationService
    record, selected = observed_owner(env, tmp_path)
    root = registry.config_root(env)
    outside_providers = rules()
    outside_providers["providers"] = ["claude-agent-acp"]  # excludes codex-acp, the coordinator's own engine
    write(root / "routing.json", outside_providers)
    monkeypatch.setattr("cortex_runtime.catalog.refresh.perform_refresh", lambda *a, **k: pytest.fail("engine refresh forbidden"))
    before = bytes_of(root)
    assert resolve_owner_coordinator(portable(), env).route.id == selected.route.id
    view = m.read_user_defaults(env)
    assert view["coordinator_resolution"] == {"state": "bound", "route_id": selected.route.id, "connection_id": record.id}
    agent, _ = _new_agent(tmp_path, env)
    project = tmp_path / "project";project.mkdir()
    response = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session = agent.sessions[response["sessionId"]]
    assert session["default_selection"]["route_id"] == selected.route.id
    assert not session["default_selection"].get("resolution_error")
    store, task_id = _bootstrap_task_for_session(agent, response["sessionId"], tmp_path)
    assert agent._propose_pending_selection_for_new_task(store, session, task_id)
    history = ConfigurationService(store).history(task_id)
    assert len(history) == 1 and history[0]["route_id"] == selected.route.id
    assert bytes_of(root) == before


@pytest.mark.parametrize("restriction", ["denied", "disabled"])
def test_owner_coordinator_still_refuses_denied_or_disabled_outside_worker_providers(env, tmp_path, restriction):
    """``providers`` exclusion alone must never bind a coordinator; denial and the
    enabled-models allowlist keep refusing it regardless of ``providers`` membership."""
    record, selected = observed_owner(env, tmp_path)
    root = registry.config_root(env)
    blocked = rules()
    blocked["providers"] = ["claude-agent-acp"]
    if restriction == "denied": blocked["deny"] = ["codex-acp/*"]
    else: blocked["enabled"]["codex-acp"] = []
    write(root / "routing.json", blocked)
    with pytest.raises(PortableError, match="portable_coordinator_unavailable"):
        resolve_owner_coordinator(portable(), env)
    view = m.read_user_defaults(env)
    assert view["coordinator_resolution"]["state"] == "unavailable"


@pytest.mark.parametrize("problem", ["duplicate", "withdrawn", "unsupported", "disabled"])
def test_fresh_portable_refusal_has_no_revision_or_owner_replacement(env, tmp_path, monkeypatch, problem):
    from test_acp_config_options import _new_agent, _bootstrap_task_for_session, _configured_connection
    from cortex_runtime.configuration.service import ConfigurationService
    from cortex_runtime.catalog import store as catalog_store
    from dataclasses import replace
    record, selected = observed_owner(env, tmp_path)
    root = registry.config_root(env)
    if problem == "duplicate":
        other = _configured_connection(name="other", environ=env)
        with registry.transaction(env) as state: state.connections[other.id] = replace(other, launch_command=("/never-run-engine",))
        raw = [dict(id="model", name="Model", category="model", type="select", currentValue="custom", options=[dict(value="custom", name="Custom")])]
        catalog_store.save_snapshot(discovery.build_catalog({other.id: other}, {other.id: discovery.parse_observation(raw)}), env)
    elif problem == "withdrawn":
        snapshot = discovery.build_catalog({record.id: record}, {record.id: discovery.parse_observation([])})
        catalog_store.save_snapshot(snapshot, env)
    elif problem == "unsupported":
        value = portable();value["parameters"]["thought_level"] = "ultra"
        write(root / "defaults.json", {"defaults": {"coordinator": value}})
    else:
        disabled = rules();disabled["enabled"]["codex-acp"] = [];write(root / "routing.json", disabled)
    before = bytes_of(root)
    monkeypatch.setattr("cortex_runtime.catalog.refresh.perform_refresh", lambda *a, **k: pytest.fail("engine refresh forbidden"))
    agent, _ = _new_agent(tmp_path, env)
    project = tmp_path / "project";project.mkdir()
    response = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session = agent.sessions[response["sessionId"]]
    assert session["default_selection"]["resolution_error"]
    store, task_id = _bootstrap_task_for_session(agent, response["sessionId"], tmp_path)
    with pytest.raises(PortableError): agent._propose_pending_selection_for_new_task(store, session, task_id)
    assert ConfigurationService(store).history(task_id) == () and bytes_of(root) == before


def test_preview_apply_cli_executable_with_explicit_consent(env, tmp_path, monkeypatch, capsys):
    root, candidate, _ = migration_source(env, tmp_path)
    for key, value in env.items(): monkeypatch.setenv(key, value)
    from cortex_model_policy_migrate import main
    assert main(["preview", "--candidate", str(candidate)]) == 0
    result = json.loads(capsys.readouterr().out)["result"]
    args = ["apply", "--candidate", str(candidate), "--expected-revision", result["expected_revision"],
            "--candidate-sha256", result["candidate_sha256"]]
    assert main(args) == 1
    assert json.loads(capsys.readouterr().out)["error"]["code"] == "migration_consent_required"
    assert main(args + ["--i-consent"]) == 0
    assert json.loads(capsys.readouterr().out)["result"]["applied"]


def test_legacy_top_tier_migration_needs_explicit_unique_engine(env, tmp_path):
    root = registry.config_root(env)
    write(root / "routing.json", {"tiers": {"custom": "deep"}})
    candidate = write(tmp_path / "candidate", rules())
    assert migration.preview(candidate, environ=env)["changed"]
    ambiguous = rules();ambiguous["models"]["claude-agent-acp"] = {"tiers": {"custom": "deep"}}
    write(candidate, ambiguous)
    with pytest.raises(PortableError, match="migration_legacy_tiers_ambiguous"): migration.preview(candidate, environ=env)


def test_nonobserved_manual_identity_never_binds_portable():
    candidate = entry()
    candidate.evidence = EvidenceState.empty()
    with pytest.raises(PortableError, match="portable_coordinator_unavailable"):
        resolve_portable_coordinator([candidate], portable())


def test_delete_failed_verification_restores_requested_snapshot(env, monkeypatch):
    root = registry.config_root(env)
    write(root / "routing.json", rules())
    assert profile_action(env, "create", "first")["ok"]
    before = bytes_of(root)
    real = m._replace_file
    original = (root / "routing-profiles.json").read_bytes()
    def corrupt(path, body): real(path, body if body == original else body + b" ")
    monkeypatch.setattr(m, "_replace_file", corrupt)
    result = profile_action(env, "delete", "first")
    assert result["error"]["code"] == "profile_delete_failed" and bytes_of(root) == before


def test_partial_defaults_have_nonmutating_repair_diagnostic(env):
    root = registry.config_root(env)
    write(root / "defaults.json", {"defaults": {"language": "ru"}})
    before = (root / "defaults.json").read_bytes()
    result = defaults.apply(env)
    assert result["defaults"]["status"] == "coordinator_default_missing"
    assert (root / "defaults.json").read_bytes() == before


def test_mutable_parameters_override_portable_bundle_without_template_overlay():
    from cortex_runtime.configuration.resolution import portable_from_defaults
    value = portable()
    effective = portable_from_defaults({"coordinator": value, "parameters": {"thought_level": "low"}})
    assert effective["parameters"] == {"thought_level": "low"} and value == portable()


def test_migration_rejects_symlink_root_without_reading_private_target(env, tmp_path):
    root = registry.config_root(env)
    root.parent.mkdir()
    external = tmp_path / "external";external.mkdir(mode=0o700)
    write(external / "routing.json", {"deny": ["owner"]})
    root.symlink_to(external)
    candidate = write(tmp_path / "candidate", dict(rules(), deny=["owner"]))
    before = bytes_of(external)
    with pytest.raises(PortableError, match="unsafe_storage"): migration.preview(candidate, environ=env)
    assert bytes_of(external) == before


def test_migration_backup_must_verify_before_owner_write(env, tmp_path, monkeypatch):
    root, candidate, _ = migration_source(env, tmp_path)
    preview = migration.preview(candidate, environ=env)
    before = (root / "routing.json").read_bytes()
    real = defaults._create_private
    def corrupt(path, body): return real(path, body + " ")
    monkeypatch.setattr(defaults, "_create_private", corrupt)
    with pytest.raises(PortableError, match="migration_backup_invalid"):
        migration.apply(candidate, preview["expected_revision"], preview["candidate_sha256"], consent=True, environ=env)
    assert (root / "routing.json").read_bytes() == before


@pytest.mark.parametrize("malformation", ["boolean_format", "invalid_other_snapshot"])
def test_profile_migration_never_rewrites_invalid_retained_storage(env, tmp_path, malformation):
    root, candidate, _ = migration_source(env, tmp_path)
    profile = dict(description="", sections=["routing"], settings=dict(routing={"deny": []}))
    document = dict(format=True if malformation == "boolean_format" else 2, active=None,
                    profiles={"first": profile, "other": dict(profile)})
    if malformation == "invalid_other_snapshot": document["profiles"]["other"]["settings"] = "invalid"
    path = write(root / "routing-profiles.json", document)
    before = path.read_bytes()
    with pytest.raises(PortableError): migration.preview(candidate, environ=env, profile="first")
    assert path.read_bytes() == before


def test_routing_assertion_preview_is_complete_and_nonmutating(env, tmp_path):
    from cortex_routing_preview_check import check
    root, path, candidate = migration_source(env, tmp_path)
    write(root / "defaults.json", {"defaults": {"coordinator": portable(), "parameters": {"thought_level": "high"}}})
    # Arbitrary unknown private data is refused, never copied into the candidate.
    with pytest.raises(AssertionError):
        check(root, write(tmp_path / "template.json", {"routing": candidate}))
    source = json.loads((root / "routing.json").read_text())
    source.pop("unknown")
    candidate.pop("unknown")
    write(root / "routing.json", source)
    template = write(tmp_path / "template.json", {"routing": candidate})
    before = bytes_of(root)
    result = check(root, template)
    assert result["assertions_passed"] and result["cli_preview_exit"] == 0
    assert result["unchanged_defaults"] and result["unchanged_routing"]
    assert not result["candidate_retained"] and bytes_of(root) == before
    real_defaults = root / "defaults.json"
    body = real_defaults.read_bytes()
    real_defaults.unlink()
    outside = tmp_path / "not-defaults";outside.write_bytes(body)
    real_defaults.symlink_to(outside)
    with pytest.raises(AssertionError):
        check(root, template)


def test_routing_candidate_completion_preserves_order_and_unknown_keys():
    from cortex_routing_preview_check import complete_missing
    source = {"deny": ["second", "first"], "future": {"retained": True},
              "models": {"codex-acp": {"tiers": {"custom": ["standard", "deep"]}}}}
    reviewed = {"deny": ["different"], "models": {"codex-acp": {"tiers": {"custom": ["deep", "standard"]},
                "auto": ["custom"]}}, "model_policy_version": 2}
    result = complete_missing(source, reviewed)
    migration._preserve(source, result)
    assert result["models"]["codex-acp"]["tiers"]["custom"] == ["standard", "deep"]
    assert result["deny"] == source["deny"] and result["future"] == source["future"]
    assert "model_policy_version" not in source


def test_migration_writes_atomically_under_private_cross_process_lock(env, tmp_path, monkeypatch):
    import fcntl
    import os
    root, candidate, _ = migration_source(env, tmp_path)
    before = (root / "routing.json").read_bytes()
    preview = migration.preview(candidate, environ=env)
    real_replace = os.replace
    replacements = []
    def observed_replace(source, target):
        target = Path(target)
        if target == root / "routing.json":
            lock = root / "connections.lock"
            assert lock.stat().st_mode & 0o777 == 0o600
            descriptor = os.open(lock, os.O_RDWR)
            try:
                with pytest.raises(BlockingIOError):
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            finally:
                os.close(descriptor)
            assert target.read_bytes() == before
            assert Path(source).parent == target.parent
            assert Path(source).stat().st_mode & 0o777 == 0o600
            replacements.append(target)
        return real_replace(source, target)
    monkeypatch.setattr(os, "replace", observed_replace)
    result = migration.apply(candidate, preview["expected_revision"], preview["candidate_sha256"], consent=True, environ=env)
    assert result["applied"] and replacements == [root / "routing.json"]
    assert Path(result["backup"]).read_bytes() == before
    assert Path(result["backup"]).stat().st_mode & 0o777 == 0o600
    assert not list(root.glob(".routing.json.*.tmp"))
