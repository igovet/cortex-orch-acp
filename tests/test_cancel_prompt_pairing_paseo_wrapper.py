"""The guarded dev Paseo wrapper drops inherited PASEO_* overrides.

Part of the same Paseo integration work as the cancel+prompt pairing: a
wrapper started from inside a production Paseo agent inherits that
installation's PASEO_HOME and related variables. No Paseo is launched.
"""
from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]


def _wrapper():
    spec = importlib.util.spec_from_file_location("paseo_dev_wrapper", ROOT / "scripts/cortex_paseo_dev.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


INHERITED = {
    "PASEO_HOME": "/home/someone/.paseo",
    "PASEO_DESKTOP_MANAGED": "1",
    "PASEO_NODE_ENV": "production",
    "PASEO_AGENT_ID": "00000000-0000-0000-0000-000000000000",
    "PASEO_AGENT_CWD": "/elsewhere",
}


def test_isolated_environment_drops_every_inherited_paseo_override(tmp_path, monkeypatch):
    wrapper = _wrapper()
    for key, value in INHERITED.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setenv("UNRELATED_SETTING", "kept")
    env = wrapper.isolated_environment(tmp_path / ".paseo-dev")
    assert not [key for key in env if key.startswith("PASEO_")]
    assert env["UNRELATED_SETTING"] == "kept"
    assert env["XDG_CONFIG_HOME"] == str(tmp_path / ".paseo-dev/cortex-xdg/config")


def test_cli_passes_only_the_isolated_home(tmp_path, monkeypatch):
    wrapper = _wrapper()
    for key, value in INHERITED.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    seen = []
    monkeypatch.setattr(wrapper.subprocess, "run",
                        lambda argv, **kwargs: seen.append((argv, kwargs["env"])) or SimpleNamespace(returncode=0))
    dev = tmp_path / ".paseo-dev"
    args = SimpleNamespace(home=str(dev), paseo="/bin/true", check=False, wire_trace=False,
                           command=["--", "status", "--json"])
    assert wrapper._run_cli(args) == 0
    argv, env = seen[0]
    assert argv[:3] == ["/bin/true", "--home", str(dev)]
    assert {key: value for key, value in env.items() if key.startswith("PASEO_")} == {"PASEO_HOME": str(dev)}
