"""engine.codex_install: CodexAcpInstall construction and verify_install().

Every install used here is a synthetic, owned fixture built fresh under
``tmp_path`` -- never a reference to a real installed codex-acp package (see
the T013 task card's safety boundary: static reading of an installed
adapter's files is allowed for discovery, but tests construct their own
owned fixtures rather than depending on a real, possibly-absent install).
"""
from __future__ import annotations

import hashlib
import json
import os
import stat

import pytest

from cortex_runtime.engine.codex_install import CodexAcpInstall, launch_command_for, verify_install
from cortex_runtime.portable.errors import PortableError


def _sha256(path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _write_executable(path, content: bytes = b"#!/bin/sh\nexit 0\n") -> None:
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def _make_install_dir(tmp_path, *, version="1.12.0", package_name="@agentclientprotocol/codex-acp"):
    root = tmp_path / "install"
    root.mkdir()
    node_binary = root / "node"
    _write_executable(node_binary)
    adapter_entry = root / "index.js"
    adapter_entry.write_text("// fixture codex-acp adapter entry (synthetic, not real upstream)\n", encoding="utf-8")
    package_json = root / "package.json"
    package_json.write_text(json.dumps({"name": package_name, "version": version}), encoding="utf-8")
    bundled_codex_binary = root / "codex"
    _write_executable(bundled_codex_binary)
    return root, node_binary, adapter_entry, package_json, bundled_codex_binary


def _make_install(tmp_path, *, version="1.12.0"):
    root, node_binary, adapter_entry, package_json, bundled_codex_binary = _make_install_dir(tmp_path, version=version)
    install = CodexAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version=version,
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
        bundled_codex_version="0.45.0-fixture",
        bundled_codex_binary=str(bundled_codex_binary),
        bundled_codex_binary_sha256=_sha256(bundled_codex_binary),
    )
    return install, root


def _environ(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    return {"HOME": str(home)}


def test_verify_install_accepts_matching_fixture(tmp_path):
    install, _root = _make_install(tmp_path)
    verified = verify_install(install, _environ(tmp_path))
    assert verified is install
    assert launch_command_for(verified) == (install.node_binary, install.adapter_entry)


def test_verify_install_rejects_adapter_entry_drift(tmp_path):
    install, root = _make_install(tmp_path)
    (root / "index.js").write_text("// tampered after pinning\n", encoding="utf-8")
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_drifted"


def test_verify_install_rejects_bundled_codex_binary_drift(tmp_path):
    install, root = _make_install(tmp_path)
    binary = root / "codex"
    binary.write_bytes(b"#!/bin/sh\necho tampered\n")
    binary.chmod(binary.stat().st_mode | stat.S_IXUSR)
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_drifted"


def test_verify_install_rejects_package_json_hash_drift(tmp_path):
    install, root = _make_install(tmp_path)
    (root / "package.json").write_text(
        json.dumps({"name": "@agentclientprotocol/codex-acp", "version": "1.12.0", "extra": "tampered"}),
        encoding="utf-8",
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_drifted"


def test_verify_install_rejects_package_version_mismatch(tmp_path):
    root, node_binary, adapter_entry, package_json, bundled_codex_binary = _make_install_dir(tmp_path, version="1.12.0")
    # package_version recorded on the install disagrees with what
    # package.json itself (correctly hashed) actually says.
    install = CodexAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="1.11.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
        bundled_codex_version="0.45.0-fixture",
        bundled_codex_binary=str(bundled_codex_binary),
        bundled_codex_binary_sha256=_sha256(bundled_codex_binary),
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_drifted"


def test_verify_install_rejects_relative_path_component(tmp_path):
    install, root = _make_install(tmp_path)
    sneaky = str(root / "sub" / ".." / "index.js")
    tampered = CodexAcpInstall(
        node_binary=install.node_binary,
        adapter_entry=sneaky,
        package_json=install.package_json,
        package_version=install.package_version,
        package_json_sha256=install.package_json_sha256,
        adapter_entry_sha256=install.adapter_entry_sha256,
        bundled_codex_version=install.bundled_codex_version,
        bundled_codex_binary=install.bundled_codex_binary,
        bundled_codex_binary_sha256=install.bundled_codex_binary_sha256,
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(tampered, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_untrusted_location"


def test_verify_install_rejects_symlinked_component(tmp_path):
    install, root = _make_install(tmp_path)
    real_target = root / "index.js"
    link = root / "index-link.js"
    try:
        link.symlink_to(real_target)
    except OSError:
        pytest.skip("symlinks are not supported in this environment")
    tampered = CodexAcpInstall(
        node_binary=install.node_binary,
        adapter_entry=str(link),
        package_json=install.package_json,
        package_version=install.package_version,
        package_json_sha256=install.package_json_sha256,
        adapter_entry_sha256=install.adapter_entry_sha256,
        bundled_codex_version=install.bundled_codex_version,
        bundled_codex_binary=install.bundled_codex_binary,
        bundled_codex_binary_sha256=install.bundled_codex_binary_sha256,
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(tampered, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_untrusted_location"


@pytest.mark.parametrize(
    "relative_parts",
    [
        (".local", "share", "app.codeg", "codex-acp"),
        (".config", "app.codeg", "codex-acp"),
    ],
)
def test_verify_install_rejects_untrusted_locations(tmp_path, relative_parts):
    home = tmp_path / "home"
    home.mkdir()
    untrusted_root = home.joinpath(*relative_parts)
    untrusted_root.mkdir(parents=True)
    node_binary = untrusted_root / "node"
    _write_executable(node_binary)
    adapter_entry = untrusted_root / "index.js"
    adapter_entry.write_text("// synthetic\n", encoding="utf-8")
    package_json = untrusted_root / "package.json"
    package_json.write_text(json.dumps({"name": "@agentclientprotocol/codex-acp", "version": "1.12.0"}), encoding="utf-8")
    bundled_codex_binary = untrusted_root / "codex"
    _write_executable(bundled_codex_binary)
    install = CodexAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="1.12.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
        bundled_codex_version="0.45.0-fixture",
        bundled_codex_binary=str(bundled_codex_binary),
        bundled_codex_binary_sha256=_sha256(bundled_codex_binary),
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, {"HOME": str(home)})
    assert excinfo.value.code == "engine_install_untrusted_location"


def test_verify_install_rejects_untrusted_location_when_home_is_a_symlink(tmp_path):
    """F2 (T013 review): HOME itself may be a symlink (e.g. a real user's
    ``$HOME`` symlinked to another disk); the untrusted-root comparison
    must resolve HOME first so an install path given in already-canonical
    (resolved) form is still caught, not only one written through the
    symlink's own unresolved spelling."""
    real_home_target = tmp_path / "real-home"
    real_home_target.mkdir()
    home_symlink = tmp_path / "home-symlink"
    try:
        home_symlink.symlink_to(real_home_target, target_is_directory=True)
    except OSError:
        pytest.skip("symlinks are not supported in this environment")

    untrusted_root = real_home_target / ".local" / "share" / "app.codeg" / "codex-acp"
    untrusted_root.mkdir(parents=True)
    node_binary = untrusted_root / "node"
    _write_executable(node_binary)
    adapter_entry = untrusted_root / "index.js"
    adapter_entry.write_text("// synthetic\n", encoding="utf-8")
    package_json = untrusted_root / "package.json"
    package_json.write_text(json.dumps({"name": "@agentclientprotocol/codex-acp", "version": "1.12.0"}), encoding="utf-8")
    bundled_codex_binary = untrusted_root / "codex"
    _write_executable(bundled_codex_binary)

    # Every install path is given in its already-resolved (real_home_target)
    # form -- no symlink component in any of these fields themselves, so
    # only the HOME-resolution fix (not the existing per-field symlink
    # check) is what must catch this.
    install = CodexAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="1.12.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
        bundled_codex_version="0.45.0-fixture",
        bundled_codex_binary=str(bundled_codex_binary),
        bundled_codex_binary_sha256=_sha256(bundled_codex_binary),
    )
    # HOME is the *symlink*, unresolved -- exactly the scenario a naive
    # (unresolved) untrusted-root comparison would miss.
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, {"HOME": str(home_symlink)})
    assert excinfo.value.code == "engine_install_untrusted_location"


def test_verify_install_rejects_relative_home(tmp_path):
    install, _root = _make_install(tmp_path)
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, {"HOME": "relative/home"})
    assert excinfo.value.code == "engine_install_untrusted_location"


def test_verify_install_rejects_empty_home(tmp_path):
    install, _root = _make_install(tmp_path)
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, {"HOME": ""})
    assert excinfo.value.code == "engine_install_untrusted_location"


def test_construction_rejects_relative_path_fields(tmp_path):
    install, _root = _make_install(tmp_path)
    with pytest.raises(PortableError) as excinfo:
        CodexAcpInstall(
            node_binary="node",  # relative, never accepted
            adapter_entry=install.adapter_entry,
            package_json=install.package_json,
            package_version=install.package_version,
            package_json_sha256=install.package_json_sha256,
            adapter_entry_sha256=install.adapter_entry_sha256,
            bundled_codex_version=install.bundled_codex_version,
            bundled_codex_binary=install.bundled_codex_binary,
            bundled_codex_binary_sha256=install.bundled_codex_binary_sha256,
        )
    assert excinfo.value.code == "invalid_field"


def test_construction_rejects_malformed_hash(tmp_path):
    install, _root = _make_install(tmp_path)
    with pytest.raises(PortableError) as excinfo:
        CodexAcpInstall(
            node_binary=install.node_binary,
            adapter_entry=install.adapter_entry,
            package_json=install.package_json,
            package_version=install.package_version,
            package_json_sha256="not-a-hex-digest",
            adapter_entry_sha256=install.adapter_entry_sha256,
            bundled_codex_version=install.bundled_codex_version,
            bundled_codex_binary=install.bundled_codex_binary,
            bundled_codex_binary_sha256=install.bundled_codex_binary_sha256,
        )
    assert excinfo.value.code == "invalid_field"


def test_launch_command_for_requires_codex_acp_install_instance():
    with pytest.raises(PortableError) as excinfo:
        launch_command_for(object())
    assert excinfo.value.code == "invalid_field"
