"""Tests for scripts/cortex_diagnose.py (T031 no-paid diagnostics)."""
from __future__ import annotations

import inspect
import json
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import cortex_diagnose  # noqa: E402
import cortex_acp_setup as setup  # noqa: E402
from cortex_runtime.connections.model import AuthMode  # noqa: E402

DIAGNOSE_SCRIPT = SCRIPTS / "cortex_diagnose.py"


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))


def test_never_imports_or_references_paid_probe_scripts():
    """Static guard: diagnostics must never import, exec, or subprocess a
    paid-probe entry point (mentioning the name in a docstring explaining
    the boundary is fine; an actual import/call is not)."""
    source = inspect.getsource(cortex_diagnose)
    for forbidden in ("import cortex_codex_probe", "import cortex_claude_probe",
                       "cortex_codex_probe.py'", 'cortex_codex_probe.py"',
                       "cortex_claude_probe.py'", 'cortex_claude_probe.py"'):
        assert forbidden not in source
    assert "subprocess" not in source
    assert "os.exec" not in source


def test_fresh_environment_is_degraded_not_failed(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    report = cortex_diagnose.run_diagnostics()
    assert report["overall"] in ("degraded", "ok")
    names = {c["name"] for c in report["checks"]}
    assert "storage:config_root" in names
    assert "pinned_route" in names
    every_warn_has_repair = all(
        c["repair_action"] for c in report["checks"] if c["status"] in ("warn", "fail")
    )
    assert every_warn_has_repair


def test_after_full_setup_diagnostics_is_ok(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    setup.init_storage()
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(name="work", engine="codex-acp", provider="openai",
                                 auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
                                 api_key_reader=lambda: secret_file.read_bytes())
    setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")
    report = cortex_diagnose.run_diagnostics()
    # Exclude the always-present static "gateway auth mode is unqualified"
    # informational warnings -- those describe engine capability limits,
    # never something a setup step fixes.
    setup_relevant = [c for c in report["checks"] if not c["name"].startswith("auth_support:")
                       and not c["name"].startswith("adapter_qualification:")]
    non_ok = [c for c in setup_relevant if c["status"] not in ("ok", "skip")]
    assert non_ok == [], non_ok


def test_broken_registry_is_reported_failed(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    setup.init_storage()
    connections_path = Path(setup.registry.config_root()) / "connections.json"
    connections_path.write_text("not json", encoding="utf-8")
    report = cortex_diagnose.run_diagnostics()
    assert report["overall"] == "failed"


def test_zed_settings_not_checked_by_default(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    report = cortex_diagnose.run_diagnostics()
    zed_checks = [c for c in report["checks"] if c["name"].startswith("client_registration")]
    assert zed_checks[0]["status"] == "skip"


def test_zed_settings_identical_reports_ok(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    import cortex_acp_client_config as client_config

    settings = tmp_path / "settings.json"
    entry = client_config.build_entry()
    settings.write_text(json.dumps({"agent_servers": entry}), encoding="utf-8")
    report = cortex_diagnose.run_diagnostics(zed_settings=settings)
    zed_checks = [c for c in report["checks"] if c["name"] == "client_registration:zed"]
    assert zed_checks[0]["status"] == "ok"


def test_cli_exit_codes(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    proc = subprocess.run([sys.executable, "-B", str(DIAGNOSE_SCRIPT)], capture_output=True, text=True,
                           env={**{"HOME": str(tmp_path / "home"), "XDG_CONFIG_HOME": str(tmp_path / "config"),
                                    "XDG_STATE_HOME": str(tmp_path / "state"), "PATH": "/usr/bin:/bin"}})
    assert proc.returncode in (0, 1)
    data = json.loads(proc.stdout)
    assert data["overall"] in ("ok", "degraded")


def test_never_spawns_a_subprocess(monkeypatch, tmp_path):
    """Dynamic guard: run_diagnostics must never spawn a subprocess (which
    a probe or a live engine capability check would require)."""
    _isolate(tmp_path, monkeypatch)
    import subprocess as subprocess_module

    def _forbidden(*_a, **_kw):
        raise AssertionError("cortex_diagnose must never spawn a subprocess")

    monkeypatch.setattr(subprocess_module, "run", _forbidden)
    monkeypatch.setattr(subprocess_module, "Popen", _forbidden)
    cortex_diagnose.run_diagnostics()


def test_diagnostics_launches_zero_engine_processes_after_full_setup(monkeypatch, tmp_path):
    """DEP-02: instrument the actual engine-invocation path
    (``engine.process.EngineProcess``, the class whose constructor is the
    only ``subprocess.Popen`` call in the engine-launch code path -- see
    ``engine/process.py``) with a call counter, run ``run_diagnostics``
    after a full connection/route setup (the config state most likely to
    tempt a "helpful" capability probe), and assert exactly zero engine
    launches -- not just that the diagnostics script's own source text
    never mentions ``subprocess`` (the existing static guard), but that no
    transitively-imported code path actually launches one at runtime."""
    _isolate(tmp_path, monkeypatch)
    setup.init_storage()
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(name="work", engine="codex-acp", provider="openai",
                                 auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
                                 api_key_reader=lambda: secret_file.read_bytes())
    setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")

    from cortex_runtime.engine import process as engine_process

    launch_count = {"n": 0}
    real_init = engine_process.EngineProcess.__init__

    def _counting_init(self, *args, **kwargs):
        launch_count["n"] += 1
        return real_init(self, *args, **kwargs)

    monkeypatch.setattr(engine_process.EngineProcess, "__init__", _counting_init)

    report = cortex_diagnose.run_diagnostics()

    assert launch_count["n"] == 0
    assert report["overall"] in ("ok", "degraded")
