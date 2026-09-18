"""T031: the standalone production entrypoint fails closed on a pinned
mismatch (never silently proceeds, never attempts a download), and a
network-less empty-environment launch stays protocol-only on stdout."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
import cortex_acp  # noqa: E402
from cortex_runtime.packaging import pinned  # noqa: E402

SCRIPTS = Path(__file__).resolve().parents[1] / "src"


def test_fail_closed_on_manifest_error(monkeypatch, capsys):
    def _raise(*_a, **_kw):
        raise pinned.PinnedManifestError("boom")

    monkeypatch.setattr(cortex_acp, "load_manifest", _raise)
    with pytest.raises(SystemExit) as excinfo:
        cortex_acp._fail_closed_on_pinned_mismatch()
    assert excinfo.value.code == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "boom" in captured.err


def test_fail_closed_on_python_floor_mismatch(monkeypatch, capsys):
    manifest = {"python_version_floor": [99, 0], "portable_contract_version": 1}
    monkeypatch.setattr(cortex_acp, "load_manifest", lambda: manifest)
    with pytest.raises(SystemExit) as excinfo:
        cortex_acp._fail_closed_on_pinned_mismatch()
    assert excinfo.value.code == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "python_version_floor" in captured.err
    assert "Repair:" in captured.err


def test_passes_with_real_pinned_manifest(capsys):
    cortex_acp._fail_closed_on_pinned_mismatch()
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == ""


def test_network_less_empty_environment_launch_is_protocol_only(tmp_path):
    """Spawn the real entrypoint with a stripped, network-less environment
    (no proxy vars, minimal PATH) and confirm stdout carries only
    JSON-RPC lines -- no banner, no download/update attempt, no traceback
    text leaking onto stdout."""
    home = tmp_path / "home"
    home.mkdir()
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_STATE_HOME": str(home / "state"),
        "TMPDIR": str(tmp_path / "tmp"),
    }
    (tmp_path / "tmp").mkdir()
    proc = subprocess.Popen(
        [sys.executable, "-B", str(SCRIPTS / "cortex_acp.py")],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    try:
        proc.stdin.write(json.dumps(dict(
            jsonrpc="2.0", id=1, method="initialize",
            params=dict(protocolVersion=1, clientCapabilities={}, clientInfo=dict(name="t", version="1")),
        )) + "\n")
        proc.stdin.flush()
        proc.stdin.close()
        proc.stdin = None
        out, err = proc.communicate(timeout=15)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=5)
    lines = [line for line in out.splitlines() if line]
    assert lines, f"no stdout at all; stderr={err}"
    for line in lines:
        message = json.loads(line)
        assert message.get("jsonrpc") == "2.0"
    assert not (home / ".config" / "cortex" / "connections.json").exists()
    # No fresh top-level download/deps tree was created anywhere under HOME.
    assert not any(home.rglob("*deps*"))
