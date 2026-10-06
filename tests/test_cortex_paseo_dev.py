"""Tests for cortex_paseo_dev.py GUI and CLI subcommands."""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import socket
import time
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "cortex_paseo_dev.py"


@pytest.fixture(autouse=True)
def _outside_cortex_sandbox(tmp_path_factory, monkeypatch):
    """These tests may themselves run inside a Cortex command sandbox; the
    daemon/GUI launch guard is exercised explicitly below."""
    empty = tmp_path_factory.mktemp("mountinfo") / "mountinfo"
    empty.write_text("")
    monkeypatch.setenv("CORTEX_PASEO_DEV_MOUNTINFO", str(empty))


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
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo), "--port", "99999"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 2
    assert "port must be 1-65535" in result.stderr

    # Test port too low
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo), "--port", "0"],
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
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo), "--home", str(dev_home)],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 0
    assert f"PASEO_HOME={dev_home}" in result.stdout
    assert f"PASEO_ELECTRON_USER_DATA_DIR={(tmp_path / 'electron-profile').resolve()}" in result.stdout
    assert "--remote-debugging-address=127.0.0.1" in result.stdout
    port_line = next(line for line in result.stdout.splitlines() if "Chrome DevTools Protocol:" in line)
    selected_port = int(port_line.rsplit(":", 1)[1])
    assert 1 <= selected_port <= 65535
    assert f"--remote-debugging-port={selected_port}" in result.stdout


def test_gui_uses_a_private_xvfb_display_when_requested(tmp_path):
    """--xvfb must replace an inherited display with a short-lived private one."""
    dev_home = tmp_path / "test-paseo-dev"
    dev_home.mkdir()

    xvfb_args = tmp_path / "xvfb-args"
    fake_xvfb = tmp_path / "Xvfb"
    fake_xvfb.write_text(
        "#!/bin/sh\n"
        'printf "%s\\n" "$@" > "$XVFB_ARGS_FILE"\n'
        "printf '123\\n'\n"
        "trap 'exit 0' TERM INT\n"
        "while :; do sleep 1; done\n"
    )
    fake_xvfb.chmod(0o755)

    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "PASEO_ELECTRON_FLAGS=$PASEO_ELECTRON_FLAGS"\n'
        'echo "DISPLAY=$DISPLAY"\n'
    )
    fake_paseo.chmod(0o755)

    environment = dict(os.environ)
    environment.update({
        "PATH": f"{tmp_path}:{environment['PATH']}",
        "XVFB_ARGS_FILE": str(xvfb_args),
        "DISPLAY": ":user-desktop",
    })
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo),
         "--home", str(dev_home), "--xvfb"],
        capture_output=True, text=True, cwd=str(ROOT), env=environment,
    )

    assert result.returncode == 0
    assert "--ozone-platform=x11" in result.stdout
    assert "DISPLAY=:123" in result.stdout
    assert "Chrome DevTools Protocol: http://127.0.0.1:" in result.stdout
    assert "DISPLAY=:user-desktop" not in result.stdout
    assert xvfb_args.read_text().splitlines() == [
        "-displayfd", "1", "-screen", "0", "1440x900x24", "-nolisten", "tcp",
    ]


def test_gui_terminates_xvfb_and_electron_children_on_sigterm(tmp_path):
    """Sending SIGTERM to the wrapper must not leak its Xvfb/Electron children."""
    dev_home = tmp_path / "test-paseo-dev"
    dev_home.mkdir()

    xvfb_pid_file = tmp_path / "xvfb.pid"
    paseo_pid_file = tmp_path / "paseo.pid"

    fake_xvfb = tmp_path / "Xvfb"
    fake_xvfb.write_text(
        "#!/bin/sh\n"
        'echo $$ > "$XVFB_PID_FILE"\n'
        "printf '123\\n'\n"
        "while :; do sleep 1; done\n"
    )
    fake_xvfb.chmod(0o755)

    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo $$ > "$PASEO_PID_FILE"\n'
        "while :; do sleep 1; done\n"
    )
    fake_paseo.chmod(0o755)

    environment = dict(os.environ)
    environment.update({
        "PATH": f"{tmp_path}:{environment['PATH']}",
        "XVFB_PID_FILE": str(xvfb_pid_file),
        "PASEO_PID_FILE": str(paseo_pid_file),
    })

    wrapper = subprocess.Popen(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo),
         "--home", str(dev_home), "--xvfb"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        cwd=str(ROOT), env=environment,
    )
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and not paseo_pid_file.exists():
            time.sleep(0.05)
        assert paseo_pid_file.exists(), "fake Paseo never started"

        wrapper.send_signal(signal.SIGTERM)
        wrapper.communicate(timeout=10)
    finally:
        if wrapper.poll() is None:
            wrapper.kill()
            wrapper.communicate()

    xvfb_pid = int(xvfb_pid_file.read_text().strip())
    paseo_pid = int(paseo_pid_file.read_text().strip())

    def _alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        return True

    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and (_alive(xvfb_pid) or _alive(paseo_pid)):
        time.sleep(0.05)

    assert not _alive(paseo_pid), "fake Paseo child survived wrapper SIGTERM"
    assert not _alive(xvfb_pid), "fake Xvfb child survived wrapper SIGTERM"


def test_gui_xvfb_fails_closed_when_xvfb_is_unavailable(tmp_path):
    dev_home = tmp_path / "test-paseo-dev"
    dev_home.mkdir()
    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text("#!/bin/sh\nexit 99\n")
    fake_paseo.chmod(0o755)
    environment = dict(os.environ)
    environment["PATH"] = "/nonexistent"
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo),
         "--home", str(dev_home), "--xvfb"],
        capture_output=True, text=True, cwd=str(ROOT), env=environment,
    )
    assert result.returncode == 2
    assert "requires Xvfb" in result.stderr


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
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo),
         "--home", str(dev_home), "--port", "8888"],
        capture_output=True, text=True, cwd=str(ROOT)
    )

    assert result.returncode == 0
    assert "--remote-debugging-port=8888" in result.stdout
    assert "Chrome DevTools Protocol: http://127.0.0.1:8888" in result.stdout


def test_gui_refuses_busy_explicit_port_before_starting_xvfb(tmp_path):
    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()
    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text("#!/bin/sh\nexit 0\n")
    fake_paseo.chmod(0o755)
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as occupied:
        occupied.bind(("127.0.0.1", 0))
        occupied.listen()
        port = occupied.getsockname()[1]
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo),
             "--home", str(dev_home), "--port", str(port), "--xvfb"],
            capture_output=True, text=True, cwd=str(ROOT),
        )
    assert result.returncode == 2
    assert f"--port {port} is unavailable on 127.0.0.1" in result.stderr


def _live_lock_fixture(tmp_path):
    """A profile whose Electron SingletonLock names a live local process."""
    profile = tmp_path / "electron-profile"
    profile.mkdir()
    holder = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
    os.symlink(f"{socket.gethostname()}-{holder.pid}", profile / "SingletonLock")
    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text("#!/bin/sh\necho LAUNCHED\n")
    fake_paseo.chmod(0o755)
    return profile, holder, fake_paseo


def test_gui_refuses_profile_held_by_a_live_instance_before_launch(tmp_path):
    """Paseo quits with 0 when its profile is locked; the helper must fail closed instead."""
    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()
    profile, holder, fake_paseo = _live_lock_fixture(tmp_path)
    try:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(profile),
             "--paseo", str(fake_paseo), "--home", str(dev_home)],
            capture_output=True, text=True, cwd=str(ROOT),
        )
    finally:
        holder.kill()
        holder.wait()
    assert result.returncode == 3
    assert f"Paseo PID {holder.pid} already holds {profile}" in result.stderr
    assert "LAUNCHED" not in result.stdout
    assert "Chrome DevTools Protocol" not in result.stdout


def test_gui_reports_lock_taken_during_launch(tmp_path):
    """A lock acquired by another instance while launching is not reported as success."""
    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()
    profile = tmp_path / "electron-profile"
    profile.mkdir()
    holder = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        f'ln -s "{socket.gethostname()}-{holder.pid}" "$PASEO_ELECTRON_USER_DATA_DIR/SingletonLock"\n'
        "exit 0\n"
    )
    fake_paseo.chmod(0o755)
    try:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(profile),
             "--paseo", str(fake_paseo), "--home", str(dev_home)],
            capture_output=True, text=True, cwd=str(ROOT),
        )
    finally:
        holder.kill()
        holder.wait()
    assert result.returncode == 3
    assert f"Paseo exited because PID {holder.pid} holds {profile}" in result.stderr


def test_gui_ignores_stale_profile_lock(tmp_path):
    """A lock naming a dead process is Chromium's to reclaim, so launch proceeds."""
    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()
    profile, holder, fake_paseo = _live_lock_fixture(tmp_path)
    holder.kill()
    holder.wait()
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(profile),
         "--paseo", str(fake_paseo), "--home", str(dev_home)],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 0
    assert "LAUNCHED" in result.stdout


def test_gui_refuses_production_electron_profile(tmp_path):
    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text("#!/bin/sh\necho LAUNCHED\n")
    fake_paseo.chmod(0o755)
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(Path.home() / ".config" / "Paseo"),
         "--paseo", str(fake_paseo), "--home", str(tmp_path / "dev-home")],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 2
    assert "refusing production Paseo Electron profile" in result.stderr
    assert "LAUNCHED" not in result.stdout


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


def test_cli_uses_private_cortex_xdg_paths_and_current_source_root(tmp_path):
    fake_paseo = tmp_path / "paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "CORTEX_PASEO_SOURCE_ROOT=$CORTEX_PASEO_SOURCE_ROOT"\n'
        'echo "XDG_CONFIG_HOME=$XDG_CONFIG_HOME"\n'
        'echo "XDG_STATE_HOME=$XDG_STATE_HOME"\n'
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
    assert f"XDG_CONFIG_HOME={dev_home}/cortex-xdg/config" in result.stdout
    assert f"XDG_STATE_HOME={dev_home}/cortex-xdg/state" in result.stdout


def test_cli_drops_inherited_paseo_agent_target(tmp_path):
    fake_paseo = tmp_path / "paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'printf "agent=%s cwd=%s\\n" "${PASEO_AGENT_ID-unset}" "${PASEO_AGENT_CWD-unset}"\n'
    )
    fake_paseo.chmod(0o755)
    env = dict(os.environ, PASEO_AGENT_ID="caller-agent", PASEO_AGENT_CWD="/caller/project")
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--paseo", str(fake_paseo),
         "--home", str(tmp_path / "dev-home"), "--", "status"],
        capture_output=True, text=True, cwd=str(ROOT), env=env,
    )
    assert result.returncode == 0
    assert result.stdout.strip() == "agent=unset cwd=unset"


def test_gui_uses_private_cortex_xdg_paths_and_current_source_root(tmp_path):
    fake_paseo = tmp_path / "Paseo"
    fake_paseo.write_text(
        "#!/bin/sh\n"
        'echo "CORTEX_PASEO_SOURCE_ROOT=$CORTEX_PASEO_SOURCE_ROOT"\n'
        'echo "XDG_CONFIG_HOME=$XDG_CONFIG_HOME"\n'
        'echo "XDG_STATE_HOME=$XDG_STATE_HOME"\n'
    )
    fake_paseo.chmod(0o755)
    dev_home = tmp_path / "dev-home"
    dev_home.mkdir()
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"), "--paseo", str(fake_paseo),
         "--home", str(dev_home)],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 0
    assert f"CORTEX_PASEO_SOURCE_ROOT={ROOT}" in result.stdout
    assert f"XDG_CONFIG_HOME={dev_home}/cortex-xdg/config" in result.stdout
    assert f"XDG_STATE_HOME={dev_home}/cortex-xdg/state" in result.stdout


# /proc/self/mountinfo as seen inside a Cortex command sandbox: /var/tmp is
# the command's own scratch subdirectory (orchestration.sandbox).
SANDBOX_MOUNTINFO = (
    "36 25 259:2 / / ro,relatime - ext4 /dev/nvme0n1p2 rw\n"
    "52 36 259:2 /var/tmp /var/tmp rw,relatime - ext4 /dev/nvme0n1p2 rw\n"
    "61 36 259:2 /tmp/cortex-command-ab12cd34/var-tmp /var/tmp rw,nosuid,nodev - ext4 /dev/nvme0n1p2 rw\n"
)


def _launch_recorder(tmp_path):
    marker = tmp_path / "launched"
    fake = tmp_path / "paseo"
    fake.write_text(f'#!/bin/sh\necho "$@" >> {marker}\n')
    fake.chmod(0o755)
    return fake, marker


@pytest.mark.parametrize("command", [["start", "--timeout", "60", "--json"], ["daemon", "start"],
                                     ["daemon", "run"], ["onboard"], ["--json", "start"]])
def test_cli_refuses_daemon_launch_inside_cortex_sandbox(tmp_path, monkeypatch, command):
    """A daemon started there keeps the sandbox's /var/tmp mount, whose
    scratch Cortex deletes when the command ends; every engine jail it
    launches then fails in bwrap (live, 2026-10-06)."""
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text(SANDBOX_MOUNTINFO)
    monkeypatch.setenv("CORTEX_PASEO_DEV_MOUNTINFO", str(mountinfo))
    fake, marker = _launch_recorder(tmp_path)
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--paseo", str(fake), "--home", str(tmp_path / "dev-home"), "--", *command],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 2
    assert "inside a Cortex sandbox" in result.stderr
    assert "/tmp/cortex-command-ab12cd34/var-tmp" in result.stderr
    assert not marker.exists()


@pytest.mark.parametrize("command", [["status", "--json"], ["daemon", "stop"], ["ls", "--json"], ["logs", "start"]])
def test_cli_keeps_short_lived_commands_inside_cortex_sandbox(tmp_path, monkeypatch, command):
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text(SANDBOX_MOUNTINFO)
    monkeypatch.setenv("CORTEX_PASEO_DEV_MOUNTINFO", str(mountinfo))
    fake, marker = _launch_recorder(tmp_path)
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--paseo", str(fake), "--home", str(tmp_path / "dev-home"), "--", *command],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 0
    assert marker.read_text().split() == ["--home", str(tmp_path / "dev-home"), *command]


def test_cli_refuses_daemon_launch_inside_an_engine_jail(tmp_path, monkeypatch):
    """An engine's native shell sees its jail's scratch over /var/tmp, possibly
    already deleted."""
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text("36 25 0:41 / / rw - tmpfs tmpfs rw\n"
                         "70 36 0:41 /cortex-engine-x1y2/tmp/var-tmp//deleted /var/tmp rw - tmpfs tmpfs rw\n")
    monkeypatch.setenv("CORTEX_PASEO_DEV_MOUNTINFO", str(mountinfo))
    fake, marker = _launch_recorder(tmp_path)
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--paseo", str(fake), "--home", str(tmp_path / "dev-home"), "--", "start"],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 2
    assert "inside a Cortex sandbox" in result.stderr
    assert not marker.exists()


def test_gui_refuses_launch_inside_cortex_sandbox(tmp_path, monkeypatch):
    mountinfo = tmp_path / "mountinfo"
    mountinfo.write_text(SANDBOX_MOUNTINFO)
    monkeypatch.setenv("CORTEX_PASEO_DEV_MOUNTINFO", str(mountinfo))
    fake, marker = _launch_recorder(tmp_path)
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "gui", "--user-data-dir", str(tmp_path / "electron-profile"),
         "--paseo", str(fake), "--home", str(tmp_path / "dev-home")],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    assert result.returncode == 2
    assert "inside a Cortex sandbox" in result.stderr
    assert not marker.exists()


@pytest.mark.skipif(shutil.which("bwrap") is None, reason="bubblewrap is not installed")
def test_cli_reads_the_real_sandbox_var_tmp_mount(tmp_path, monkeypatch):
    """The real /proc/self/mountinfo of a bwrap namespace laid out like a Cortex
    command sandbox is refused; the same command with a plain /var/tmp runs."""
    monkeypatch.delenv("CORTEX_PASEO_DEV_MOUNTINFO")
    scratch = tmp_path / "cortex-command-real1234" / "var-tmp"
    scratch.mkdir(parents=True)
    fake, marker = _launch_recorder(tmp_path)
    wrapper = [sys.executable, str(SCRIPT), "--paseo", str(fake), "--home", str(tmp_path / "dev-home"), "--", "start"]
    plain = subprocess.run(["bwrap", "--dev-bind", "/", "/", "--tmpfs", "/var/tmp", "--", *wrapper],
                           capture_output=True, text=True, cwd=str(ROOT))
    if plain.returncode != 0 and plain.stderr.startswith("bwrap:"):
        pytest.skip("bubblewrap cannot create a namespace here: " + plain.stderr.strip())
    assert plain.returncode == 0
    assert marker.read_text().split() == ["--home", str(tmp_path / "dev-home"), "start"]
    marker.unlink()
    sandboxed = subprocess.run(["bwrap", "--dev-bind", "/", "/", "--bind", str(scratch), "/var/tmp", "--", *wrapper],
                               capture_output=True, text=True, cwd=str(ROOT))
    assert sandboxed.returncode == 2
    assert str(scratch) in sandboxed.stderr
    assert not marker.exists()
