"""Tests for ``scripts/cortex_acp_client_config.py`` (T011).

Every settings file used here is a synthetic tmp fixture (never the real
``~/.config/zed/settings.json``); "secret-looking" values are fake, owned
fixture strings only, never real credentials.
"""
from __future__ import annotations

import json
import os
import stat
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import cortex_acp_client_config as helper  # noqa: E402


FAKE_SECRET = "sk-fake-not-a-real-key-1234567890"


def _write(path, data):
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def test_build_entry_shape():
    entry = helper.build_entry(python3_path="/usr/bin/python3")
    assert entry["Cortex"]["type"] == "custom"
    assert entry["Cortex"]["command"] == "/usr/bin/python3"
    assert entry["Cortex"]["args"][0] == "-B"
    assert entry["Cortex"]["args"][1] == str(helper.AGENT_ENTRY)
    assert Path(entry["Cortex"]["args"][1]).is_absolute()
    assert entry["Cortex"]["env"] == {}


def test_default_dry_run_prints_entry_and_no_write(tmp_path, capsys):
    settings = tmp_path / "settings.json"
    _write(settings, {"theme": "x"})
    original_text = settings.read_text()
    exit_code = helper.main(["--settings", str(settings), "--python3", "/usr/bin/python3"])
    assert exit_code == 0
    assert settings.read_text() == original_text  # never written without --apply
    out = capsys.readouterr().out
    assert '"Cortex"' in out
    assert "would add" in out


def test_dry_run_reports_would_replace_and_identical_states(tmp_path, capsys):
    settings = tmp_path / "settings.json"
    _write(settings, {"agent_servers": {"Cortex": {"type": "custom", "command": "/different/python3",
                                                     "args": ["-B", "/different/path.py"], "env": {}}}})
    exit_code = helper.main(["--settings", str(settings), "--python3", "/usr/bin/python3"])
    assert exit_code == 0
    out = capsys.readouterr().out
    assert "would replace" in out

    entry = helper.build_entry(python3_path="/usr/bin/python3")
    _write(settings, {"agent_servers": entry})
    exit_code = helper.main(["--settings", str(settings), "--python3", "/usr/bin/python3"])
    assert exit_code == 0
    out = capsys.readouterr().out
    assert "already identical" in out


def test_dry_run_never_prints_an_unrelated_secret_looking_value(tmp_path, capsys):
    """F3: the dry-run preview must never echo a full-file diff/context --
    only the Cortex entry itself and a one-line summary."""
    settings = tmp_path / "settings.json"
    _write(settings, {
        "theme": "One Dark",
        "agent_servers": {
            "other-agent": {
                "type": "custom", "command": "/usr/bin/other",
                "args": ["--flag"], "env": {"API_KEY": FAKE_SECRET},
            },
        },
    })
    exit_code = helper.main(["--settings", str(settings), "--python3", "/usr/bin/python3"])
    assert exit_code == 0
    combined = capsys.readouterr()
    assert FAKE_SECRET not in combined.out
    assert FAKE_SECRET not in combined.err
    assert "other-agent" not in combined.out
    assert "other-agent" not in combined.err


def test_jsonc_settings_refuses_apply_and_prints_manual_instructions(tmp_path, capsys):
    settings = tmp_path / "settings.json"
    settings.write_text('{\n  // a comment\n  "theme": "x",\n}\n', encoding="utf-8")
    original_text = settings.read_text()
    exit_code = helper.main(
        ["--settings", str(settings), "--apply", "--i-consent-to-write", "--python3", "/usr/bin/python3"])
    assert exit_code != 0
    assert settings.read_text() == original_text
    combined = capsys.readouterr()
    assert "manual" in (combined.out + combined.err).lower()


def test_apply_without_consent_refuses(tmp_path):
    settings = tmp_path / "settings.json"
    _write(settings, {"theme": "x"})
    original_text = settings.read_text()
    exit_code = helper.main(["--settings", str(settings), "--apply", "--python3", "/usr/bin/python3"])
    assert exit_code != 0
    assert settings.read_text() == original_text


def test_apply_writes_backup_and_preserves_other_keys_and_secret_values(tmp_path):
    settings = tmp_path / "settings.json"
    original = {
        "theme": "One Dark",
        "agent_servers": {
            "other-agent": {
                "type": "custom", "command": "/usr/bin/other",
                "args": ["--flag"], "env": {"API_KEY": FAKE_SECRET},
            },
        },
        "z_trailing_key": True,
    }
    _write(settings, original)
    settings.chmod(0o600)

    exit_code = helper.main(
        ["--settings", str(settings), "--apply", "--i-consent-to-write", "--python3", "/usr/bin/python3"])
    assert exit_code == 0

    backups = list(tmp_path.glob("settings.json.bak.*"))
    assert len(backups) == 1
    assert json.loads(backups[0].read_text()) == original

    new_data = json.loads(settings.read_text())
    assert new_data["theme"] == "One Dark"
    assert new_data["z_trailing_key"] is True
    # Unrelated agent entry, including its "secret-looking" value, is
    # byte-identical -- untouched, never inspected or logged.
    assert new_data["agent_servers"]["other-agent"] == original["agent_servers"]["other-agent"]
    assert new_data["agent_servers"]["Cortex"]["type"] == "custom"
    assert new_data["agent_servers"]["Cortex"]["env"] == {}

    assert stat.S_IMODE(settings.stat().st_mode) == 0o600
    # Key order preserved: Cortex is appended after the pre-existing agent.
    assert list(new_data["agent_servers"].keys()) == ["other-agent", "Cortex"]


def test_apply_refuses_to_overwrite_existing_different_entry_without_replace(tmp_path):
    settings = tmp_path / "settings.json"
    _write(settings, {"agent_servers": {"Cortex": {"type": "custom", "command": "/different/python3",
                                                     "args": ["-B", "/different/path.py"], "env": {}}}})
    original_text = settings.read_text()
    exit_code = helper.main(
        ["--settings", str(settings), "--apply", "--i-consent-to-write", "--python3", "/usr/bin/python3"])
    assert exit_code != 0
    assert settings.read_text() == original_text


def test_apply_replace_overwrites_existing_different_entry(tmp_path):
    settings = tmp_path / "settings.json"
    _write(settings, {"agent_servers": {"Cortex": {"type": "custom", "command": "/different/python3",
                                                     "args": ["-B", "/different/path.py"], "env": {}}}})
    exit_code = helper.main(
        ["--settings", str(settings), "--apply", "--i-consent-to-write", "--replace", "--python3", "/usr/bin/python3"])
    assert exit_code == 0
    new_data = json.loads(settings.read_text())
    assert new_data["agent_servers"]["Cortex"]["command"] == "/usr/bin/python3"


def test_apply_same_entry_is_idempotent_no_duplicate_backup_noise(tmp_path):
    settings = tmp_path / "settings.json"
    _write(settings, {"theme": "x"})
    helper.main(["--settings", str(settings), "--apply", "--i-consent-to-write", "--python3", "/usr/bin/python3"])
    backups_after_first = list(tmp_path.glob("settings.json.bak.*"))
    exit_code = helper.main(
        ["--settings", str(settings), "--apply", "--i-consent-to-write", "--python3", "/usr/bin/python3"])
    assert exit_code == 0
    backups_after_second = list(tmp_path.glob("settings.json.bak.*"))
    assert backups_after_second == backups_after_first  # no-op write creates no new backup


def test_apply_requires_settings_path():
    exit_code = helper.main(["--apply", "--i-consent-to-write"])
    assert exit_code != 0


def test_print_codeg_recipe_runs_and_never_writes_anything(tmp_path, capsys, monkeypatch):
    monkeypatch.chdir(tmp_path)
    exit_code = helper.main(["--print-codeg-recipe"])
    assert exit_code == 0
    out = capsys.readouterr().out
    assert "codeg" in out.lower()
    assert list(tmp_path.iterdir()) == []


def test_only_operates_on_given_path_no_implicit_default(tmp_path, monkeypatch):
    """No --settings at all: prints the entry only, touches nothing on disk."""
    monkeypatch.chdir(tmp_path)
    exit_code = helper.main(["--python3", "/usr/bin/python3"])
    assert exit_code == 0
    assert list(tmp_path.iterdir()) == []


# --- F2: same-second backup collision must never overwrite a prior backup --

def test_two_applies_in_the_same_second_never_overwrite_the_prior_backup(tmp_path, monkeypatch):
    settings = tmp_path / "settings.json"
    _write(settings, {"theme": "first"})

    fixed_now = __import__("datetime").datetime(2026, 1, 1, 12, 0, 0)

    class _FixedDateTime(__import__("datetime").datetime):
        @classmethod
        def now(cls, tz=None):
            return fixed_now

    monkeypatch.setattr(helper.datetime, "datetime", _FixedDateTime)

    exit_code = helper.main(
        ["--settings", str(settings), "--apply", "--i-consent-to-write", "--replace", "--python3", "/a"])
    assert exit_code == 0
    _write(settings, {"theme": "second"})
    exit_code = helper.main(
        ["--settings", str(settings), "--apply", "--i-consent-to-write", "--replace", "--python3", "/b"])
    assert exit_code == 0

    backups = sorted(tmp_path.glob("settings.json.bak.*"))
    assert len(backups) == 2
    contents = {json.loads(path.read_text())["theme"] for path in backups}
    assert contents == {"first", "second"}  # neither backup was overwritten


# --- F4: a symlinked settings path (or a symlinked parent) is refused ------

def test_symlinked_settings_path_is_refused(tmp_path):
    real = tmp_path / "real-settings.json"
    _write(real, {"theme": "x"})
    link = tmp_path / "settings.json"
    link.symlink_to(real)
    original_text = real.read_text()

    exit_code = helper.main(
        ["--settings", str(link), "--apply", "--i-consent-to-write", "--python3", "/usr/bin/python3"])
    assert exit_code != 0
    assert real.read_text() == original_text
    assert not real.with_name("real-settings.json.bak.0").exists()


def test_symlinked_settings_path_refused_even_in_dry_run(tmp_path, capsys):
    real = tmp_path / "real-settings.json"
    _write(real, {"theme": "x"})
    link = tmp_path / "settings.json"
    link.symlink_to(real)

    exit_code = helper.main(["--settings", str(link), "--python3", "/usr/bin/python3"])
    assert exit_code != 0
    combined = capsys.readouterr()
    assert "symlink" in (combined.out + combined.err).lower()


def test_symlinked_parent_directory_is_refused(tmp_path):
    real_dir = tmp_path / "real-dir"
    real_dir.mkdir()
    settings_in_real_dir = real_dir / "settings.json"
    _write(settings_in_real_dir, {"theme": "x"})
    link_dir = tmp_path / "link-dir"
    link_dir.symlink_to(real_dir)
    settings_via_link = link_dir / "settings.json"

    exit_code = helper.main(
        ["--settings", str(settings_via_link), "--apply", "--i-consent-to-write", "--python3", "/usr/bin/python3"])
    assert exit_code != 0
    assert settings_in_real_dir.read_text() == json.dumps({"theme": "x"}, indent=2) + "\n"
