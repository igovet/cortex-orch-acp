"""No-paid regression checks for the local Rust workspace prerequisites."""
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load_script(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def tree(tmp_path):
    for name in ["rust-toolchain.toml", "Cargo.toml", "Cargo.lock",
                 "crates/workspace-protocol/Cargo.toml",
                 "prototypes/cortex-workspace/desktop/src-tauri/Cargo.toml",
                 "prototypes/cortex-workspace/desktop/package.json",
                 "prototypes/cortex-workspace/desktop/package-lock.json",
                 "pinned-dependencies.json", "packages/workspace-ui/package.json", "packages/workspace-ui/package-lock.json"]:
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    return tmp_path


def replace(root, path, old, new):
    target = root / path
    content = target.read_text()
    assert old in content
    target.write_text(content.replace(old, new, 1))


def test_committed_inputs():
    assert load_script("check_workspace_pins").check(ROOT) == []


@pytest.mark.parametrize("channel", ["stable", "nightly", "1.99", "1.99.2"])
def test_toolchain_drift(tree, channel):
    replace(tree, "rust-toolchain.toml", 'channel = "1.99.1"', f'channel = "{channel}"')
    assert load_script("check_workspace_pins").check(tree)


@pytest.mark.parametrize("version", ["2", "^2.10.1", "~2.10.1", ">=2.10.1", "*", "2.10.1"])
def test_tauri_ranges(tree, version):
    replace(tree, "prototypes/cortex-workspace/desktop/src-tauri/Cargo.toml",
            'version = "=2.10.1"', f'version = "{version}"')
    assert load_script("check_workspace_pins").check(tree)


def test_lock_drift(tree):
    replace(tree, "Cargo.lock", 'name = "tauri"\nversion = "2.10.1"',
            'name = "tauri"\nversion = "2.10.0"')
    assert load_script("check_workspace_pins").check(tree)


def test_cli_drift(tree):
    path = tree / "prototypes/cortex-workspace/desktop/package.json"
    data = json.loads(path.read_text())
    data["devDependencies"]["@tauri-apps/cli"] = "2.10.2"
    path.write_text(json.dumps(data))
    assert load_script("check_workspace_pins").check(tree)


def test_cli_lock_drift(tree):
    path = tree / "prototypes/cortex-workspace/desktop/package-lock.json"
    data = json.loads(path.read_text())
    data["packages"]["node_modules/@tauri-apps/cli"]["version"] = "2.10.2"
    path.write_text(json.dumps(data))
    assert load_script("check_workspace_pins").check(tree)


@pytest.mark.parametrize("version", ["^6.43.15", "~6.43.15", "6", "6.43.14", "latest"])
def test_editor_ranges_and_drift(tree, version):
    path = tree / "packages/workspace-ui/package.json"
    data = json.loads(path.read_text())
    data["dependencies"]["@codemirror/view"] = version
    path.write_text(json.dumps(data))
    assert load_script("check_workspace_pins").check_editor(tree)


def test_editor_lock_drift(tree):
    path = tree / "packages/workspace-ui/package-lock.json"
    data = json.loads(path.read_text())
    data["packages"]["node_modules/@codemirror/view"]["version"] = "6.43.14"
    path.write_text(json.dumps(data))
    assert any("package-lock drift" in error for error in load_script("check_workspace_pins").check_editor(tree))


def test_an_unreviewed_editor_package_is_refused(tree):
    path = tree / "packages/workspace-ui/package.json"
    data = json.loads(path.read_text())
    data["dependencies"]["@codemirror/lint"] = "6.9.0"
    path.write_text(json.dumps(data))
    assert any("reviewed exact pins" in error for error in load_script("check_workspace_pins").check_editor(tree))


def test_tauri_not_a_default_member(tree):
    replace(tree, "Cargo.toml", 'default-members = ["crates/workspace-protocol"]',
            'default-members = ["prototypes/cortex-workspace/desktop/src-tauri"]')
    assert load_script("check_workspace_pins").check(tree)


def test_probe_missing_and_nonzero_are_observed():
    probe = load_script("workspace_prerequisite_probe")
    missing = probe.probe("absent", ["cortex-intentionally-nonexistent-probe"])
    assert missing["status"] == "missing"
    assert missing["exit_code"] == 127
    failed = probe.probe("failure", [sys.executable, "-c", "raise SystemExit(3)"])
    assert failed["status"] == "missing"
    assert failed["exit_code"] == 3


def test_probe_does_not_echo_child_output():
    probe = load_script("workspace_prerequisite_probe")
    result = probe.probe("quiet", [sys.executable, "-c", "print('private-' + 'output')"])
    assert result["status"] == "present"
    assert "private-output" not in json.dumps(result)


def test_probe_inventory():
    data = json.loads(subprocess.check_output(
        [sys.executable, "-B", str(ROOT / "scripts/workspace_prerequisite_probe.py")],
        text=True))
    names = {item["name"] for item in data["items"]}
    assert {"node", "npm", "python-pytest", "python-pytest-xdist",
            "user-service-manager", "rustc", "cargo", "webkit2gtk-4.1"} <= names
    assert all(item["status"] in {"present", "missing"} for item in data["items"])
    assert all(isinstance(item["exit_code"], int) and item["command"] for item in data["items"])
