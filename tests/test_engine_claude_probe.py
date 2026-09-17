"""scripts/cortex_claude_probe.py: refusal, --plan, and route-resolution paths.

Mirrors tests/test_engine_codex_probe.py exactly. This suite NEVER passes
--i-authorize-paid-call: the authorized launch path inside
``_run_authorized`` is exercised by nothing here.
"""
from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
PROBE_SCRIPT = REPO_ROOT / "scripts" / "cortex_claude_probe.py"


def _load_probe_module():
    spec = importlib.util.spec_from_file_location("cortex_claude_probe", PROBE_SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _isolated_env(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    return {"HOME": str(home), "PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"}


def _run(args, tmp_path):
    return subprocess.run(
        [sys.executable, "-B", str(PROBE_SCRIPT), *args],
        cwd=str(tmp_path),
        env=_isolated_env(tmp_path),
        capture_output=True,
        text=True,
        timeout=15,
    )


def test_no_flags_exits_2_before_loading_anything(tmp_path):
    result = _run([], tmp_path)
    assert result.returncode == 2
    assert "refusing to run" in result.stderr


def test_authorize_flag_alone_without_approval_ref_still_exits_2(tmp_path):
    result = _run(["--i-authorize-paid-call"], tmp_path)
    assert result.returncode == 2


def test_approval_ref_alone_without_authorize_flag_still_exits_2(tmp_path):
    result = _run(["--approval-ref", "manual-review-2026-09-17"], tmp_path)
    assert result.returncode == 2


def test_empty_approval_ref_is_treated_as_unauthorized(tmp_path):
    result = _run(["--i-authorize-paid-call", "--approval-ref", ""], tmp_path)
    assert result.returncode == 2


def test_plan_mode_exits_0_and_never_requires_authorization(tmp_path):
    result = _run(["--plan"], tmp_path)
    assert result.returncode == 0
    assert "--plan (no authorization, nothing executed)" in result.stdout
    for keyword in (
        "verify_install", "EngineSessionManager", "claude_option_mapping", "delegate",
        "publish_result", "decide", "QUALIFICATION_RUN",
        "QUALIFIED",
    ):
        assert keyword in result.stdout


def test_plan_mode_states_the_instruction_delivery_status_first(tmp_path):
    result = _run(["--plan"], tmp_path)
    assert result.returncode == 0
    assert "0. STATUS" in result.stdout
    assert "QUALIFIED" in result.stdout
    assert "T035/T036" in result.stdout


def test_plan_mode_takes_precedence_over_missing_required_args(tmp_path):
    result = _run(["--plan"], tmp_path)
    assert result.returncode == 0


def test_authorized_but_missing_required_args_exits_2(tmp_path):
    result = _run(["--i-authorize-paid-call", "--approval-ref", "ref-1"], tmp_path)
    assert result.returncode == 2
    assert "--connection-id" in result.stderr


def test_probe_script_never_imports_cortex_runtime_at_module_level():
    import ast

    tree = ast.parse(PROBE_SCRIPT.read_text(encoding="utf-8"), filename=str(PROBE_SCRIPT))
    allowed_import_ids = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name in ("_run_authorized", "_resolve_route"):
            allowed_import_ids |= {id(child) for child in ast.walk(node)}
    for node in ast.walk(tree):
        if isinstance(node, (ast.Import, ast.ImportFrom)) and id(node) not in allowed_import_ids:
            names = [alias.name for alias in node.names] if isinstance(node, ast.Import) else [node.module or ""]
            for name in names:
                assert not (name == "cortex_runtime" or name.startswith("cortex_runtime.")), (
                    f"top-level/main() import of {name!r} would run before the authorization check"
                )


def _isolated_environ(tmp_path):
    home = tmp_path / "home"
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    for path in (home, config_home, state_home):
        path.mkdir(exist_ok=True)
    return {"HOME": str(home), "XDG_CONFIG_HOME": str(config_home), "XDG_STATE_HOME": str(state_home)}


def _save_one_route_snapshot(environ, connection_id, model="model-a"):
    from cortex_runtime.catalog.discovery import build_catalog, parse_observation
    from cortex_runtime.catalog.store import save_snapshot
    from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir

    now = "2026-01-01T00:00:00+00:00"
    record = ConnectionRecord(
        id=connection_id, name="probe-test", engine="claude-agent-acp", provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )
    options = [dict(id="model", name="Model", category="model", type="select", currentValue=model,
                     options=[dict(value=model, name=model)])]
    snapshot = build_catalog({connection_id: record}, {connection_id: parse_observation(options)})
    save_snapshot(snapshot, environ)
    return snapshot.entries[0].route


def test_resolve_route_finds_a_cached_matching_route(tmp_path):
    environ = _isolated_environ(tmp_path)
    connection_id = "cn_" + "a" * 12
    route = _save_one_route_snapshot(environ, connection_id)

    module = _load_probe_module()
    resolved = module._resolve_route(connection_id, route.id, environ)
    assert resolved is not None
    assert resolved.id == route.id
    assert resolved.model == "model-a"


def test_resolve_route_returns_none_for_unknown_route_id(tmp_path):
    environ = _isolated_environ(tmp_path)
    connection_id = "cn_" + "b" * 12
    _save_one_route_snapshot(environ, connection_id)

    module = _load_probe_module()
    assert module._resolve_route(connection_id, "xr_" + "0" * 24, environ) is None


def test_resolve_route_returns_none_when_no_snapshot_exists(tmp_path):
    environ = _isolated_environ(tmp_path)
    module = _load_probe_module()
    assert module._resolve_route("cn_" + "c" * 12, "xr_" + "0" * 24, environ) is None


def test_resolve_route_returns_none_for_a_different_connection(tmp_path):
    environ = _isolated_environ(tmp_path)
    connection_a = "cn_" + "d" * 12
    connection_b = "cn_" + "e" * 12
    route = _save_one_route_snapshot(environ, connection_a)

    module = _load_probe_module()
    assert module._resolve_route(connection_b, route.id, environ) is None


def test_resolve_route_never_launches_or_imports_engine_session_manager():
    import ast

    tree = ast.parse(PROBE_SCRIPT.read_text(encoding="utf-8"), filename=str(PROBE_SCRIPT))
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name == "_resolve_route":
            for child in ast.walk(node):
                if isinstance(child, ast.ImportFrom) and child.module:
                    assert "session_manager" not in child.module
                    assert "claude_install" not in child.module
