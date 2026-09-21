"""catalog.refresh: prompt-free periodic catalog refresh (T035 follow-up).

Every test here is offline, against ``tests/fake_engine_process.py`` (never
a real engine) or synthetic ``codex debug models``-shaped JSON -- except
this module never launches a real engine at all; the one-time real,
prompt-free refresh against the actual pinned claude-agent-acp build was
run by hand separately (see the task's own final report), not from this
suite.
"""
from __future__ import annotations

import json
import stat
import time
from pathlib import Path

import pytest

from cortex_runtime.catalog import codex_debug_models, refresh as catalog_refresh, store as catalog_store
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.catalog.evidence import Fact, ProvenanceSource
from cortex_runtime.acp import config_options as acp_config_options
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.engine.client import EngineProtocolError
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

from engine_fakes import (
    build_codex_npm_install,
    default_scenario,
    fake_launch_command,
    isolated_environ,
    make_connection_record,
    new_connection_id,
    write_scenario,
)

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "catalog"


def _load_options(name):
    payload = json.loads((FIXTURES / name).read_text())
    return payload["configOptions"]


def _connection(connection_id, engine, *, auth_mode=AuthMode.NATIVE_ACCOUNT, state=ConnectionState.CONFIGURED):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name="Work", engine=engine, provider="acme", auth_mode=auth_mode,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"),
        approved_fingerprint="fp", state=state, qualified=True, created_at=now, updated_at=now,
    )


def _fake_record(tmp_path, scenario, *, connection_id=None, environ=None):
    environ = environ or isolated_environ(tmp_path)
    connection_id = connection_id or new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    return record, environ


# --- perform_refresh: success replaces SEED with a real DISCOVERY snapshot -

def test_perform_refresh_success_persists_discovery_snapshot(tmp_path):
    scenario = default_scenario(config_options=_load_options("codex_acp_config_options.json"))
    record, environ = _fake_record(tmp_path, scenario)
    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)
    assert outcome.success and outcome.attempted and outcome.reason is None
    assert outcome.snapshot_id is not None
    view = catalog_store.load_connection_snapshot(record.id, outcome.snapshot_id, environ)
    assert view is not None
    assert {entry.route.model for entry in view.entries} == {"model-a", "model-b"}
    for entry in view.entries:
        provenance = entry.evidence.get(Fact.CATALOG_OBTAINED)
        assert provenance.source == ProvenanceSource.DISCOVERY
        assert provenance.source != ProvenanceSource.SEED


def test_refresh_supersedes_a_previously_seeded_snapshot():
    """A caller-held seeded snapshot (see catalog.discovery.seed_observation)
    is unconditionally superseded once a real refresh succeeds -- verified
    the same way acp.config_options.selectable_entries_from_snapshots
    already picks the most recent cached file (mtime-ordered)."""
    from cortex_runtime.catalog import discovery

    record = _connection("cn_" + "3" * 12, "codex-acp")
    seed_observation = discovery.seed_observation("codex-acp")
    assert seed_observation is not None
    seed_snapshot = build_catalog(
        {record.id: record}, {record.id: seed_observation}, seeded_connection_ids=frozenset({record.id}))
    real_observation = parse_observation(_load_options("codex_acp_config_options.json"))
    real_snapshot = build_catalog({record.id: record}, {record.id: real_observation})
    for entry in seed_snapshot.entries:
        assert entry.evidence.get(Fact.CATALOG_OBTAINED).source == ProvenanceSource.SEED
    for entry in real_snapshot.entries:
        assert entry.evidence.get(Fact.CATALOG_OBTAINED).source == ProvenanceSource.DISCOVERY


# --- structural safety core: no session/prompt is ever reachable -----------

def test_refresh_never_sends_prompt(tmp_path):
    """The safety core of this feature: perform_refresh's own request log
    (recorded by the fake engine itself, not by anything perform_refresh
    controls) must show exactly initialize + session/new and nothing named
    session/prompt -- a regression that made this code path reachable would
    fail this assertion, not just "not be exercised"."""
    request_log = tmp_path / "requests.jsonl"
    scenario = default_scenario(
        config_options=_load_options("codex_acp_config_options.json"),
        request_log_path=str(request_log),
    )
    record, environ = _fake_record(tmp_path, scenario)
    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)
    assert outcome.success
    logged_methods = [json.loads(line)["method"] for line in request_log.read_text().splitlines() if line]
    assert "session/prompt" not in logged_methods
    assert "initialize" in logged_methods
    assert "session/new" in logged_methods


def test_live_catalog_capture_keeps_codex_per_model_effort_ceiling(tmp_path, monkeypatch):
    """A live session capture must not overwrite per-model Codex efforts.

    The ACP session/new payload can expose one shared fallback set, while
    Codex's own offline model catalog has the authoritative per-model set.
    This is the same enrichment used by prompt-free refresh and must also be
    applied by the ordinary worker/coordinator session capture path.
    """
    environ = isolated_environ(tmp_path)
    record = _connection("cn_" + "a" * 12, "codex-acp")
    options = _load_options("codex_acp_config_options.json")
    manager = type("Manager", (), {
        "_install": object(),
        "observed_config_options": lambda self, _session: options,
    })()
    monkeypatch.setattr(
        catalog_refresh, "_resolve_install_for_refresh", lambda _record, _environ: manager._install,
    )
    monkeypatch.setattr(
        catalog_refresh, "_codex_effort_ceiling",
        lambda _record, _install, _environ, timeout: {"model-a": ("low",), "model-b": ("high",)},
    )
    adapter = EngineSessionManagerAdapter(lambda _cid: record, lambda _tid: str(tmp_path), environ=environ)
    adapter._capture_catalog_observation(manager, record, "session-1")

    entries = acp_config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    by_model = {entry.route.model: entry.route.supported_parameters for entry in entries}
    assert dict(by_model["model-a"])["thought_level"] == "low"
    assert dict(by_model["model-b"])["thought_level"] == "high"


def test_refresh_module_never_references_session_manager():
    """perform_refresh must never even construct an EngineSessionManager
    (whose prompt_turn method exists) -- a static source check on top of
    the behavioral one above: neither name appears anywhere in the module's
    own source."""
    import cortex_runtime.catalog.refresh as refresh_module

    source = Path(refresh_module.__file__).read_text(encoding="utf-8")
    # Prose mentioning *why* EngineSessionManager/prompt_turn are avoided is
    # fine (and present, deliberately); what must never appear is an actual
    # import of it or a call to session/prompt.
    assert "import EngineSessionManager" not in source
    assert "EngineSessionManager(" not in source
    assert ".prompt_turn(" not in source
    assert '"session/prompt"' not in source
    assert "'session/prompt'" not in source


# --- failure is never fatal -------------------------------------------------

def test_failed_refresh_preserves_previous_snapshot_and_records_reason(tmp_path):
    good_scenario = default_scenario(config_options=_load_options("codex_acp_config_options.json"))
    connection_id = new_connection_id()
    environ = isolated_environ(tmp_path)
    good_record, _ = _fake_record(tmp_path, good_scenario, connection_id=connection_id, environ=environ)
    first = catalog_refresh.perform_refresh(good_record, str(tmp_path), environ=environ, timeout=5.0)
    assert first.success

    # A launch_command that starts a real process but never speaks
    # JSON-RPC at all -- initialize simply times out, exactly like an
    # unauthenticated/hanging real engine (codeg --version hung for
    # minutes on the dev machine that authorized this feature).
    hang_record = make_connection_record(
        connection_id, launch_command=("/bin/sleep", "300"), engine="codex-acp", environ=environ,
    )
    second = catalog_refresh.perform_refresh(hang_record, str(tmp_path), environ=environ, timeout=1.0)
    assert second.attempted and not second.success
    assert second.reason is not None
    assert second.snapshot_id is None

    # The previously cached snapshot is completely untouched.
    files = sorted((environ_catalog_dir(environ, connection_id)).glob("cs_*.json"))
    assert len(files) == 1
    view = catalog_store.load_connection_snapshot(connection_id, first.snapshot_id, environ)
    assert view is not None and view.entries


def environ_catalog_dir(environ, connection_id):
    from cortex_runtime.connections import registry

    return registry.state_root(environ) / "catalog" / connection_id


def test_empty_config_options_is_a_recorded_failure_not_a_crash(tmp_path):
    scenario = default_scenario(config_options=[])
    record, environ = _fake_record(tmp_path, scenario)
    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)
    assert outcome.attempted and not outcome.success
    assert outcome.reason == "no_config_options_advertised"
    assert outcome.snapshot_id is None


def test_authentication_required_refresh_failure_is_actionable_and_redacted(monkeypatch, tmp_path):
    record, environ = _fake_record(tmp_path, default_scenario())
    secret = "Bearer super-secret-token"

    def refuse_session(*args, **kwargs):
        raise EngineProtocolError(-32000, f"Authentication required: {secret}", {"token": secret})

    monkeypatch.setattr(catalog_refresh, "_read_catalog_from_engine", refuse_session)
    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)

    assert outcome.attempted and not outcome.success
    assert outcome.reason == "authorization_required"
    assert secret not in repr(outcome)


@pytest.mark.parametrize(
    "error",
    [
        EngineProtocolError(-32000, "provider-specific wording"),
        EngineProtocolError("Authentication required", None),
    ],
)
def test_authentication_required_classification_accepts_only_bounded_protocol_signals(monkeypatch, tmp_path, error):
    record, environ = _fake_record(tmp_path, default_scenario())
    monkeypatch.setattr(catalog_refresh, "_read_catalog_from_engine", lambda *args, **kwargs: (_ for _ in ()).throw(error))

    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)

    assert outcome.reason == "authorization_required"


def test_different_protocol_code_with_authentication_text_stays_generic(monkeypatch, tmp_path):
    record, environ = _fake_record(tmp_path, default_scenario())
    error = EngineProtocolError(-32001, "Authentication required")
    monkeypatch.setattr(catalog_refresh, "_read_catalog_from_engine", lambda *args, **kwargs: (_ for _ in ()).throw(error))

    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)

    assert outcome.reason == "engine_unreachable:EngineProtocolError"


def test_other_protocol_error_remains_generic_and_does_not_echo_engine_text(monkeypatch, tmp_path):
    record, environ = _fake_record(tmp_path, default_scenario())
    secret = "api-key=super-secret"
    error = EngineProtocolError(-32001, secret)
    monkeypatch.setattr(catalog_refresh, "_read_catalog_from_engine", lambda *args, **kwargs: (_ for _ in ()).throw(error))

    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)

    assert outcome.reason == "engine_unreachable:EngineProtocolError"
    assert secret not in repr(outcome)


# --- selection is never blocked by a refresh --------------------------------

def test_selection_never_blocked_by_a_refresh(tmp_path):
    from cortex_runtime.acp import config_options as acp_config_options

    scenario = default_scenario(config_options=_load_options("codex_acp_config_options.json"))
    record, environ = _fake_record(tmp_path, scenario)
    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)
    assert outcome.success

    catalog_dir = environ_catalog_dir(environ, record.id)
    before = sorted(catalog_dir.glob("*"))
    # Selection must work purely off the already-cached snapshot, and must
    # never itself trigger (or be blocked by) a refresh.
    entries = acp_config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    assert entries
    after = sorted(catalog_dir.glob("*"))
    assert before == after  # no new snapshot file appeared


def test_selectable_entries_never_imports_refresh():
    import cortex_runtime.acp.config_options as config_options_module

    source = Path(config_options_module.__file__).read_text(encoding="utf-8")
    assert "import refresh" not in source
    assert "catalog.refresh" not in source
    assert "catalog_refresh" not in source


# --- per-model effort scoping is real, not cosmetic -------------------------

def test_per_model_thought_levels_stay_independent_across_siblings():
    record = _connection("cn_" + "1" * 12, "codex-acp")
    observation = parse_observation(_load_options("codex_acp_config_options.json"))
    # Both models start with the identical (copied, per user decision)
    # flat set.
    assert observation.model_thought_levels == {"model-a": ("none", "medium"), "model-b": ("none", "medium")}
    narrowed = observation.with_model_thought_levels({"model-b": ("low", "high", "max")})
    snapshot = build_catalog({record.id: record}, {record.id: narrowed})
    by_model = {entry.route.model: entry for entry in snapshot.entries}
    thought_a = {v for n, v in by_model["model-a"].route.supported_parameters if n == "thought_level"}
    thought_b = {v for n, v in by_model["model-b"].route.supported_parameters if n == "thought_level"}
    assert thought_a == {"none", "medium"}
    assert thought_b == {"low", "high", "max"}
    assert thought_a != thought_b


def test_with_model_thought_levels_never_drops_an_unmentioned_model():
    observation = parse_observation(_load_options("codex_acp_config_options.json"))
    overlaid = observation.with_model_thought_levels({"model-b": ("low",)})
    assert overlaid.model_thought_levels["model-a"] == ("none", "medium")
    assert overlaid.model_thought_levels["model-b"] == ("low",)


# --- find_route_identity: legacy-defaults migration recovery (T035 -------
# --- FINDING-05) ------------------------------------------------------------

def test_find_route_identity_recovers_a_stale_id_from_an_older_cached_snapshot(tmp_path):
    from cortex_runtime.catalog import store as catalog_store_module

    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = _connection(connection_id, "codex-acp")
    old_observation = parse_observation(_load_options("codex_acp_config_options.json"))
    old_snapshot = build_catalog({connection_id: record}, {connection_id: old_observation})
    catalog_store_module.save_snapshot(old_snapshot, environ)
    old_route_id = next(e.route.id for e in old_snapshot.entries if e.route.model == "model-a")

    # A later refresh (new snapshot id, new route ids -- same models).
    new_observation = old_observation.with_model_thought_levels({"model-a": ("none", "medium", "high")})
    new_snapshot = build_catalog({connection_id: record}, {connection_id: new_observation})
    catalog_store_module.save_snapshot(new_snapshot, environ)
    new_route_id = next(e.route.id for e in new_snapshot.entries if e.route.model == "model-a")
    assert old_route_id != new_route_id

    identity = catalog_store_module.find_route_identity([connection_id], old_route_id, environ)
    assert identity == (connection_id, "model-a")


def test_find_route_identity_returns_none_when_never_cached(tmp_path):
    from cortex_runtime.catalog import store as catalog_store_module

    environ = isolated_environ(tmp_path)
    assert catalog_store_module.find_route_identity(["cn_" + "9" * 12], "xr_" + "0" * 24, environ) is None


# --- codex_debug_models: per-model effort ceiling parsing/subprocess -------

def _sample_codex_debug_models_json():
    return json.dumps(dict(models=[
        dict(slug="gpt-5.5", visibility="list", supported_reasoning_levels=[
            dict(effort="low"), dict(effort="medium"), dict(effort="high"), dict(effort="xhigh"),
        ]),
        dict(slug="gpt-6-astra", visibility="list", supported_reasoning_levels=[
            dict(effort="low"), dict(effort="medium"), dict(effort="high"),
            dict(effort="xhigh"), dict(effort="max"), dict(effort="ultra"),
        ]),
        dict(slug="hidden-model", visibility="internal", supported_reasoning_levels=[dict(effort="low")]),
    ]))


def test_parse_effort_ceiling_filters_visibility_and_extracts_per_model_efforts():
    ceiling = codex_debug_models.parse_effort_ceiling(_sample_codex_debug_models_json())
    assert ceiling == {
        "gpt-5.5": ("low", "medium", "high", "xhigh"),
        "gpt-6-astra": ("low", "medium", "high", "xhigh", "max", "ultra"),
    }
    assert "hidden-model" not in ceiling
    assert ceiling["gpt-5.5"] != ceiling["gpt-6-astra"]


def test_parse_effort_ceiling_malformed_returns_none():
    assert codex_debug_models.parse_effort_ceiling("not json") is None
    assert codex_debug_models.parse_effort_ceiling(json.dumps([])) is None
    assert codex_debug_models.parse_effort_ceiling(json.dumps({"models": "nope"})) is None


def _write_fake_codex_binary(path: Path, body: str) -> None:
    path.write_text(f"#!/usr/bin/env python3\n{body}\n", encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def test_fetch_effort_ceiling_runs_the_binary_and_parses_stdout(tmp_path):
    payload_path = tmp_path / "payload.json"
    payload_path.write_text(_sample_codex_debug_models_json(), encoding="utf-8")
    binary = tmp_path / "fake_codex"
    _write_fake_codex_binary(binary, f"import sys; sys.stdout.write(open({str(payload_path)!r}).read())")
    ceiling = codex_debug_models.fetch_effort_ceiling(str(binary), timeout=5.0)
    assert ceiling is not None
    assert ceiling["gpt-5.5"] == ("low", "medium", "high", "xhigh")


def test_fetch_effort_ceiling_bounded_by_timeout_and_reaps(tmp_path):
    binary = tmp_path / "hanging_codex"
    _write_fake_codex_binary(binary, "import time; time.sleep(300)")
    start = time.monotonic()
    ceiling = codex_debug_models.fetch_effort_ceiling(str(binary), timeout=1.0)
    elapsed = time.monotonic() - start
    assert ceiling is None
    assert elapsed < 5.0


def test_fetch_effort_ceiling_missing_binary_returns_none(tmp_path):
    missing = tmp_path / "no_such_binary"
    assert codex_debug_models.fetch_effort_ceiling(str(missing), timeout=2.0) is None


# --- _codex_effort_ceiling gating (never called for a non-codex engine) ----

def test_codex_effort_ceiling_is_none_for_non_codex_engine_or_missing_install():
    record = _connection("cn_" + "c" * 12, "claude-agent-acp")
    assert catalog_refresh._codex_effort_ceiling(record, install=None, environ=None, timeout=5.0) is None
    codex_record = _connection("cn_" + "d" * 12, "codex-acp")
    assert catalog_refresh._codex_effort_ceiling(codex_record, install=None, environ=None, timeout=5.0) is None


# --- Gap A (T035 live qualification): a refresh with a verifiable install --
# --- actually narrows per-model efforts, end to end ------------------------


def _codex_debug_models_payload_two_models():
    return json.dumps(dict(models=[
        dict(slug="model-a", visibility="list", supported_reasoning_levels=[
            dict(effort="low"), dict(effort="medium"), dict(effort="high"), dict(effort="xhigh"),
        ]),
        dict(slug="model-b", visibility="list", supported_reasoning_levels=[
            dict(effort="low"), dict(effort="medium"), dict(effort="high"),
            dict(effort="xhigh"), dict(effort="max"), dict(effort="ultra"),
        ]),
    ]))


def _thought_levels(entry):
    return {value for name, value in entry.route.supported_parameters if name == "thought_level"}


def test_refresh_narrows_per_model_efforts_when_install_is_real_and_verifiable(tmp_path):
    """The actual bug this task closes: previously
    scripts/cortex_acp_setup.py's refresh-catalog step never built an
    install, so catalog.refresh._codex_effort_ceiling's own ``install is
    None`` guard made this enrichment unreachable in practice even though
    the per-model narrowing machinery itself worked -- see this module's
    other Observation.with_model_thought_levels tests above. Now
    perform_refresh resolves+verifies the install itself, straight from
    the connection's own launch_command (engine.install_resolution), with
    no caller-supplied install= at all -- exactly the codepath
    refresh-catalog uses."""
    environ = isolated_environ(tmp_path)
    engine_scenario_path = write_scenario(
        tmp_path, default_scenario(config_options=_load_options("codex_acp_config_options.json")),
    )
    payload = _codex_debug_models_payload_two_models()
    debug_models_body = f"import sys; sys.stdout.write({payload!r})"
    launch_command, _binary = build_codex_npm_install(
        tmp_path / "install", engine_scenario_path, codex_debug_models_body=debug_models_body,
        private_adapter_shape=True,
    )
    record = make_connection_record(new_connection_id(), launch_command=launch_command, engine="codex-acp", environ=environ)

    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)

    assert outcome.success and outcome.effort_enrichment == "applied"
    view = catalog_store.load_connection_snapshot(record.id, outcome.snapshot_id, environ)
    by_model = {entry.route.model: entry for entry in view.entries}
    efforts_a = _thought_levels(by_model["model-a"])
    efforts_b = _thought_levels(by_model["model-b"])
    assert efforts_a == {"low", "medium", "high", "xhigh"}
    assert "max" not in efforts_a and "ultra" not in efforts_a
    assert efforts_b == {"low", "medium", "high", "xhigh", "max", "ultra"}
    assert efforts_a != efforts_b


def test_refresh_falls_back_to_flat_set_and_records_reason_when_install_is_unverifiable(tmp_path):
    """A connection with no verifiable install (this repository's own
    fake-engine test double: a 4-element launch_command is never
    install-shaped -- see engine.install_resolution's own docstring) keeps
    today's flat, engine-advertised set for every model and records why
    enrichment was skipped, rather than failing the refresh."""
    scenario = default_scenario(config_options=_load_options("codex_acp_config_options.json"))
    record, environ = _fake_record(tmp_path, scenario)

    outcome = catalog_refresh.perform_refresh(record, str(tmp_path), environ=environ, timeout=5.0)

    assert outcome.success
    assert outcome.effort_enrichment == "skipped:no_verifiable_install"
    view = catalog_store.load_connection_snapshot(record.id, outcome.snapshot_id, environ)
    by_model = {entry.route.model: entry for entry in view.entries}
    assert _thought_levels(by_model["model-a"]) == _thought_levels(by_model["model-b"])


def test_model_efforts_observes_absent_selector_and_restores_initial_model():
    from cortex_runtime.catalog.model_efforts import observe_model_efforts
    options = _load_options('codex_acp_config_options.json')
    model_option = next(o for o in options if o.get('category') == 'model')
    calls = []
    def call(method, params, *, timeout):
        assert method == 'session/set_config_option'
        calls.append(params)
        raw = json.loads(json.dumps(options))
        next(o for o in raw if o.get('category') == 'model')['currentValue'] = params['value']
        if params['value'] == 'model-b':
            raw = [o for o in raw if o.get('category') != 'thought_level']
        return {'configOptions': raw}
    levels = observe_model_efforts(call, 'exact-session', options, timeout=2)
    assert levels['model-a']
    assert levels['model-b'] == ()
    assert calls[-1]['value'] == model_option['currentValue']
    enriched = parse_observation(options).with_model_thought_levels(levels)
    assert not [p for p in enriched.parameter_pairs_for('model-b') if p[0] == 'thought_level']
    assert all(p['sessionId'] == 'exact-session' for p in calls)


def test_model_efforts_does_not_accept_unconfirmed_selection():
    from cortex_runtime.catalog.model_efforts import observe_model_efforts
    options = _load_options('codex_acp_config_options.json')
    calls = []
    def call(method, params, *, timeout):
        calls.append(params)
        return {'configOptions': options}
    with pytest.raises(ValueError, match='not confirmed'):
        observe_model_efforts(call, 'exact-session', options, timeout=2)
    assert calls[-1]['value'] == next(o['currentValue'] for o in options if o.get('category') == 'model')
