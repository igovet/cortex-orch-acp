"""Tests for cortex_paseo_dev.py GUI and CLI subcommands."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "cortex_paseo_dev.py"


def test_gui_validates_paseo_executable(tmp_path):
    """The gui subcommand must validate that --paseo is an executable absolute path."""
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--paseo", "/nonexistent/Paseo"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 2
    assert "executable absolute path" in result.stderr


def test_gui_validates_port_range(tmp_path):
    """The gui subcommand must validate that --port is in valid range."""
    fake_paseo = tmp_path / "subdir" / "Paseo"
    fake_paseo.parent.mkdir()
    fake_paseo.write_text("#!/bin/sh\nexit 0\n")
    fake_paseo.chmod(0o755)

    # Test port too high
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--paseo", str(fake_paseo), "--port", "99999"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 2
    assert "port must be 1-65535" in result.stderr

    # Test port too low
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--paseo", str(fake_paseo), "--port", "0"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 2
    assert "port must be 1-65535" in result.stderr


def test_gui_sets_environment_correctly(tmp_path):
    """The gui subcommand must set PASEO_HOME, PASEO_ELECTRON_USER_DATA_DIR, and PASEO_ELECTRON_FLAGS."""
    dev_home = tmp_path / "test-paseo-dev"
    dev_home.mkdir()

    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "PASEO_HOME=$PASEO_HOME"\n'
        'echo "PASEO_ELECTRON_USER_DATA_DIR=$PASEO_ELECTRON_USER_DATA_DIR"\n'
        'echo "PASEO_ELECTRON_FLAGS=$PASEO_ELECTRON_FLAGS"\n'
    )
    fake_paseo.chmod(0o755)

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--paseo", str(fake_paseo), "--home", str(dev_home)],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 0
    assert f"PASEO_HOME={dev_home}" in result.stdout
    assert "PASEO_ELECTRON_USER_DATA_DIR=" in result.stdout
    assert "--remote-debugging-address=127.0.0.1" in result.stdout
    assert "--remote-debugging-port=9222" in result.stdout


def test_gui_adds_xvfb_flag_when_requested(tmp_path):
    """The gui subcommand must add --ozone-platform=x11 when --xvfb is specified."""
    dev_home = tmp_path / "test-paseo-dev"
    dev_home.mkdir()

    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "PASEO_ELECTRON_FLAGS=$PASEO_ELECTRON_FLAGS"\n'
    )
    fake_paseo.chmod(0o755)

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--paseo", str(fake_paseo),
         "--home", str(dev_home), "--xvfb"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 0
    assert "--ozone-platform=x11" in result.stdout


def test_gui_custom_port(tmp_path):
    """The gui subcommand must use custom CDP port when specified."""
    dev_home = tmp_path / "test-paseo-dev"
    dev_home.mkdir()

    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "PASEO_ELECTRON_FLAGS=$PASEO_ELECTRON_FLAGS"\n'
    )
    fake_paseo.chmod(0o755)

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--paseo", str(fake_paseo),
         "--home", str(dev_home), "--port", "8888"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 0
    assert "--remote-debugging-port=8888" in result.stdout


def test_cli_refuses_home_override(tmp_path):
    """The CLI subcommand must refuse --home or --host overrides in the command."""
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--", "status", "--home", "/custom"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 2
    assert "may not override --home" in result.stderr


def test_cli_refuses_host_override(tmp_path):
    """The CLI subcommand must refuse --host in the command."""
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--", "status", "--host", "localhost"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 2
    assert "may not override" in result.stderr


def test_cli_check_mode_succeeds(tmp_path):
    """The --check flag must validate configuration without running commands."""
    fake_paseo = tmp_path / "subdir" / "paseo"
    fake_paseo.parent.mkdir()
    fake_paseo.write_text("#!/bin/sh\nexit 0\n")
    fake_paseo.chmod(0o755)

    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--paseo", str(fake_paseo), "--home", str(dev_home), "--check"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 0
    data = json.loads(result.stdout)
    assert data["paseo"] == str(fake_paseo)
    assert data["home"] == str(dev_home)


def test_cli_requires_command(tmp_path):
    """The CLI mode requires a command to be provided."""
    fake_paseo = tmp_path / "subdir" / "paseo"
    fake_paseo.parent.mkdir()
    fake_paseo.write_text("#!/bin/sh\nexit 0\n")
    fake_paseo.chmod(0o755)

    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--paseo", str(fake_paseo)],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 2
    assert "provide a Paseo command" in result.stderr


def test_cli_passes_current_source_root(tmp_path):
    fake_paseo = tmp_path / "paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "CORTEX_PASEO_SOURCE_ROOT=$CORTEX_PASEO_SOURCE_ROOT"\n'
    )
    fake_paseo.chmod(0o755)
    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--paseo", str(fake_paseo),
         "--home", str(dev_home), "--", "status"],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 0
    assert f"CORTEX_PASEO_SOURCE_ROOT={ROOT}" in result.stdout


def test_gui_passes_current_source_root(tmp_path):
    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "CORTEX_PASEO_SOURCE_ROOT=$CORTEX_PASEO_SOURCE_ROOT"\n'
    )
    fake_paseo.chmod(0o755)
    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--paseo", str(fake_paseo),
         "--home", str(dev_home)],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 0
    assert f"CORTEX_PASEO_SOURCE_ROOT={ROOT}" in result.stdout
