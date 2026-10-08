"""Engine model display names survive discovery, persistence, and ACP menus."""
from __future__ import annotations

from cortex_runtime.acp import config_options
from cortex_runtime.catalog import store as catalog_store
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir


def _connection(connection_id: str, engine: str = "codex-acp") -> ConnectionRecord:
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name="Work", engine=engine, provider="acme",
        auth_mode=AuthMode.NATIVE_ACCOUNT, key_kind=None, endpoint=None,
        launch_command=None, credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"),
        approved_fingerprint="fp", state=ConnectionState.CONFIGURED,
        qualified=True, created_at=now, updated_at=now,
    )


def _snapshot(connection: ConnectionRecord):
    options = [{
        "id": "model", "name": "Model", "type": "select", "category": "model",
        "currentValue": "gpt-5.6-luna",
        "options": [{"value": "gpt-5.6-luna", "name": "GPT-5.6 Luna (Fast)",
                     "description": "Fast reasoning model"}],
    }]
    observation = parse_observation(options)
    return build_catalog({connection.id: connection}, {connection.id: observation})


def test_engine_model_name_survives_catalog_snapshot_round_trip(tmp_path, monkeypatch):
    config = tmp_path / "config"
    state = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config, state, home):
        path.mkdir()
    environ = {"XDG_CONFIG_HOME": str(config), "XDG_STATE_HOME": str(state), "HOME": str(home)}
    connection = _connection("cn_" + "a" * 12)
    snapshot = _snapshot(connection)
    assert snapshot.entries[0].model_name == "GPT-5.6 Luna (Fast)"
    assert snapshot.entries[0].model_description == "Fast reasoning model"
    catalog_store.save_snapshot(snapshot, environ)
    loaded = catalog_store.load_connection_snapshot(connection.id, snapshot.id, environ)
    assert loaded is not None
    assert loaded.entries[0].model_name == "GPT-5.6 Luna (Fast)"
    assert loaded.entries[0].model_description == "Fast reasoning model"


def test_model_option_uses_engine_name_and_secondary_provenance():
    connection = _connection("cn_" + "b" * 12)
    entry = _snapshot(connection).entries[0]
    option = config_options.build_model_option(
        [entry], entry.route.model, None, None,
        connections={connection.id: connection}, environ={},
    )
    assert option is not None
    assert option["options"][0]["value"] == entry.route.model
    assert option["options"][0]["name"] == "GPT-5.6 Luna (Fast)"
    assert option["options"][0]["description"] == "codex"


def test_combined_paseo_model_option_uses_unique_model_ids_and_friendly_names():
    codex = _connection("cn_" + "c" * 12, "codex-acp")
    claude = _connection("cn_" + "d" * 12, "claude-agent-acp")
    codex_entry = _snapshot(codex).entries[0]
    claude_options = [{
        "id": "model", "name": "Model", "type": "select", "category": "model",
        "currentValue": "sonnet", "options": [{"value": "sonnet", "name": "Claude Sonnet 4.6",
                                                 "description": "Balanced Claude model"}],
    }]
    claude_entry = build_catalog(
        {claude.id: claude}, {claude.id: parse_observation(claude_options)},
    ).entries[0]
    option = config_options.build_combined_model_option(
        [codex_entry, claude_entry], {codex.id: codex, claude.id: claude},
        codex_entry.route.id, codex.id, None, None, {},
    )
    by_value = {item["value"]: item for item in option["options"]}
    assert by_value[codex_entry.route.model]["name"] == "GPT-5.6 Luna (Fast)"
    assert by_value[claude_entry.route.model]["name"] == "Claude Sonnet 4.6"
    assert by_value[codex_entry.route.model]["description"] == "codex"
    assert by_value[claude_entry.route.model]["description"] == "claude"
    assert option["currentValue"] == codex_entry.route.model


def test_two_haiku_versions_keep_distinct_engine_names_and_ids(tmp_path):
    connection = _connection("cn_" + "e" * 12, "claude-agent-acp")
    options = [{
        "id": "model", "name": "Model", "type": "select", "category": "model",
        "currentValue": "haiku", "options": [
            {"value": "haiku", "name": "Haiku 5.5", "description": "Fastest for quick answers"},
            {"value": "claude-haiku-4-5-20251001", "name": "Haiku 4.5"},
        ],
    }]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    environ = {"HOME": str(tmp_path), "XDG_STATE_HOME": str(tmp_path / "state"),
               "XDG_CONFIG_HOME": str(tmp_path / "config")}
    catalog_store.save_snapshot(snapshot, environ)
    loaded = catalog_store.load_connection_snapshot(connection.id, snapshot.id, environ)
    assert loaded is not None
    assert {entry.route.model: entry.model_name for entry in loaded.entries} == {
        "haiku": "Haiku 5.5", "claude-haiku-4-5-20251001": "Haiku 4.5"}
