"""Reported provider windows reach Workspace without estimates or a CLI install on PATH."""
import importlib.util
import json
import pytest
from pathlib import Path
from types import SimpleNamespace

from cortex_runtime.connections import quota, registry

ROOT = Path(__file__).resolve().parents[1]


def helper():
    spec = importlib.util.spec_from_file_location("weekly_machine", ROOT / "crates/workspace-service/machine_tools.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_weekly_display_uses_shared_source_and_never_last_scope_wins(monkeypatch):
    records = [SimpleNamespace(id="a", engine="claude-agent-acp"), SimpleNamespace(id="b", engine="codex-acp"),
               SimpleNamespace(id="c", engine="opencode")]
    monkeypatch.setattr(registry, "inspect_registry", lambda _: SimpleNamespace(connections={r.id:r for r in records}))
    windows = {
        "a": [{"name":"seven_day", "usedPercent":70, "resetsAt":None},
              {"name":"seven_day_sonnet", "usedPercent":2, "resetsAt":None},
              {"name":"five_hour", "usedPercent":99, "resetsAt":None}],
        "b": [{"name":"secondary", "usedPercent":48, "resetsAt":None, "windowMinutes":10080}],
    }
    monkeypatch.setattr(quota, "visible_windows", lambda r: windows[r.id])
    rows, seen = helper().plan_usage()
    assert {r["engine"]: r["usedPercent"] for r in rows} == {"claude":70, "codex":48}
    assert all(r["observedAt"] is None for r in rows)  # no invented observation timestamp
    assert all(w["window"] != "five_hour" for ws in seen.values() for w in ws)


def test_bundled_claude_cli_works_without_global_cli(tmp_path, monkeypatch):
    from cortex_runtime.engine import install_resolution
    from test_usage_accounting import _environ, _claude_connection, _usage_result, _USAGE_TEXT, _now
    env = _environ(tmp_path)
    record = _claude_connection(env)
    package = tmp_path / "adapter"
    package.mkdir()
    (package / "package.json").write_text("{}")
    sdk = tmp_path / "sdk"
    sdk.mkdir()
    (sdk / "claude").write_text("fixture executable, never launched")
    (sdk / "claude").chmod(0o700)
    monkeypatch.setattr(quota.shutil, "which", lambda *a, **k: None)
    monkeypatch.setattr(install_resolution, "resolve_verified_install", lambda r: SimpleNamespace(package_json=str(package / "package.json")))
    monkeypatch.setattr(install_resolution, "_resolve_node_package_dir", lambda *a: sdk)
    calls = []
    def run(argv, env, cwd, timeout):
        calls.append(argv)
        return _usage_result(_USAGE_TEXT)
    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", run)
    windows = quota.visible_windows(record, env, now=_now("2026-09-20 00:00"))
    assert any(w["name"] == "seven_day" and w["usedPercent"] == 50 for w in windows)
    assert calls[0][0] == str(sdk / "claude")


def test_codex_latest_secondary_reaches_weekly_display(tmp_path, monkeypatch):
    now = 1794000000
    native = tmp_path / "native"
    day = native / "sessions/2026/11/06"
    day.mkdir(parents=True)
    (day / "rollout.jsonl").write_text(json.dumps({"payload":{"rate_limits":{
        "primary":{"used_percent":99,"resets_at":now+100,"window_minutes":300},
        "secondary":{"used_percent":46,"resets_at":now+604800,"window_minutes":10080}
    }}}) + "\n")
    record = SimpleNamespace(id="b",engine="codex-acp",credential_ref=SimpleNamespace(path=str(native)))
    monkeypatch.setattr(registry, "inspect_registry", lambda _: SimpleNamespace(connections={"b":record}))
    monkeypatch.setattr(quota.time, "time", lambda:now)
    rows, seen = helper().plan_usage()
    assert rows[0]["usedPercent"] == 46
    assert seen["codex-acp"] == [{"window":"secondary","percent":46.0}]


@pytest.mark.parametrize("slot,minutes,expired,expected", [
    ("primary", 10080, False, 35.0),
    ("secondary", 10080, False, 35.0),
    ("secondary", 300, False, None),
    ("primary", None, False, None),
    ("secondary", None, False, None),
    ("primary", True, False, None),
    ("primary", 10080, True, None),
])
def test_codex_weekly_window_uses_reported_duration_not_slot(tmp_path, monkeypatch, slot, minutes, expired, expected):
    now = 1794000000
    native = tmp_path / "native"
    day = native / "sessions/2026/11/07"
    day.mkdir(parents=True)
    window = {"used_percent":35, "resets_at":now - 1 if expired else now + 604800}
    if minutes is not None:
        window["window_minutes"] = minutes
    (day / "rollout.jsonl").write_text(json.dumps({"payload":{"rate_limits":{slot:window}}}) + "\n")
    record = SimpleNamespace(id="b", engine="codex-acp", credential_ref=SimpleNamespace(path=str(native)))
    monkeypatch.setattr(registry, "inspect_registry", lambda _: SimpleNamespace(connections={"b":record}))
    monkeypatch.setattr(quota.time, "time", lambda:now)
    rows, seen = helper().plan_usage()
    assert rows[0]["usedPercent"] == expected
    assert seen["codex-acp"] == ([] if expected is None else [{"window":slot, "percent":expected}])
