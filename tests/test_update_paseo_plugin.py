"""No-paid checks for the repository Paseo plugin updater."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "update-paseo-plugin.sh"


def test_update_script_is_portable_and_help_is_read_only():
    assert SCRIPT.stat().st_mode & 0o111
    syntax = subprocess.run(["sh", "-n", str(SCRIPT)], capture_output=True, text=True)
    assert syntax.returncode == 0, syntax.stderr
    result = subprocess.run([str(SCRIPT), "--help"], capture_output=True, text=True)
    assert result.returncode == 0
    assert "stamps the package" in result.stdout


def test_update_script_does_not_split_or_execute_a_missing_cli_with_spaces(tmp_path):
    missing_cli = tmp_path / "Paseo install" / "paseo"
    env = os.environ.copy()
    env["CORTEX_PASEO_CLI"] = str(missing_cli)
    result = subprocess.run([str(SCRIPT)], capture_output=True, text=True, env=env)
    assert result.returncode != 0
    assert str(missing_cli) in result.stderr
    assert "not executable" in result.stderr
    # The preflight must stop before stamping or invoking the installer.
    assert "Stamping the Cortex package" not in result.stdout


def test_update_script_verifies_plugin_id_with_fake_cli(tmp_path):
    """The final check must address the manifest id, not the provider id."""
    cli = tmp_path / "Paseo install" / "paseo"
    cli.parent.mkdir()
    calls = tmp_path / "calls"
    cli.write_text(
        "#!/bin/sh\n"
        f"printf '%s\\n' \"$*\" >> '{calls}'\n"
        "if [ \"$1\" = daemon ] && [ \"$2\" = status ]; then\n"
        "  printf '%s\\n' '{\"localDaemon\":\"running\"}'\n"
        "  exit 0\n"
        "fi\n"
        "if [ \"$1\" = plugin ] && [ \"$2\" = ls ]; then\n"
        "  [ \"$3\" = cortex-acp ] || exit 42\n"
        "  printf '%s\\n' '[{\"id\":\"cortex-acp\",\"status\":\"running\"}]'\n"
        "  exit 0\n"
        "fi\n"
        "exit 43\n"
    )
    cli.chmod(0o755)

    fake_python = tmp_path / "python3"
    real_python = os.environ.get("PYTHON", "/usr/bin/python3")
    fake_python.write_text(
        "#!/bin/sh\n"
        "case \" $* \" in\n"
        "  *cortex_package.py*stamp*) exit 0 ;;\n"
        "  *cortex_acp_setup.py*install-paseo-plugin*) "
        "printf '%s\\n' '{\"status\":\"running\"}'; exit 0 ;;\n"
        f"  *) exec '{real_python}' \"$@\" ;;\n"
        "esac\n"
    )
    fake_python.chmod(0o755)

    env = os.environ.copy()
    env["CORTEX_PASEO_CLI"] = str(cli)
    env["PATH"] = f"{tmp_path}:{env.get('PATH', '')}"
    result = subprocess.run([str(SCRIPT)], capture_output=True, text=True, env=env)

    assert result.returncode == 0, result.stderr
    assert "plugin ls cortex-acp --json" in calls.read_text()
    assert "plugin ls cortex --json" not in calls.read_text()
