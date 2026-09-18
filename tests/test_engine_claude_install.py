"""engine.claude_install: ClaudeAcpInstall construction and verify_install().

Mirrors tests/test_engine_codex_install.py; every install used here is a
synthetic, owned fixture built fresh under ``tmp_path`` -- never a reference
to a real installed claude-agent-acp package.
"""
from __future__ import annotations

import json
import stat

import pytest

from cortex_runtime.engine.claude_install import ClaudeAcpInstall, launch_command_for, verify_install
from cortex_runtime.portable.errors import PortableError


def _sha256(path) -> str:
    import hashlib

    return hashlib.sha256(path.read_bytes()).hexdigest()


def _write_executable(path, content: bytes = b"#!/bin/sh\nexit 0\n") -> None:
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def _make_install_dir(tmp_path, *, version="0.78.0", package_name="@agentclientprotocol/claude-agent-acp"):
    root = tmp_path / "install"
    root.mkdir()
    node_binary = root / "node"
    _write_executable(node_binary)
    adapter_entry = root / "index.js"
    adapter_entry.write_text("// fixture claude-agent-acp adapter entry (synthetic, not real upstream)\n", encoding="utf-8")
    package_json = root / "package.json"
    package_json.write_text(json.dumps({"name": package_name, "version": version}), encoding="utf-8")
    return root, node_binary, adapter_entry, package_json


def _make_install(tmp_path, *, version="0.78.0"):
    root, node_binary, adapter_entry, package_json = _make_install_dir(tmp_path, version=version)
    install = ClaudeAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version=version,
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
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


def test_verify_install_rejects_package_json_hash_drift(tmp_path):
    install, root = _make_install(tmp_path)
    (root / "package.json").write_text(
        json.dumps({"name": "@agentclientprotocol/claude-agent-acp", "version": "0.78.0", "extra": "tampered"}),
        encoding="utf-8",
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_drifted"


def test_verify_install_rejects_adapter_entry_not_named_by_package_bin(tmp_path):
    """Regression (T035 live handshake): a real claude-agent-acp package.json
    declares ``"bin": {"claude-agent-acp": "dist/index.js"}`` -- the only
    file that ever calls the SDK's own ``runAcp()`` and attaches ACP
    handlers to stdin/stdout. A sibling library module under the same
    package (e.g. ``dist/acp-agent.js``, which ``dist/index.js`` imports
    ``runAcp`` from but never itself invokes at module scope) is a plain
    ES module: launched directly with node it does nothing and exits
    immediately -- the transport closes before the very first
    ``initialize`` call ever gets a reply. ``verify_install`` must reject
    an ``adapter_entry`` that is not the file package.json's own ``bin``
    field names, even though such a file can otherwise pass every other
    check here (absolute, non-symlinked, trusted location, matching its
    own pinned hash)."""
    root = tmp_path / "install"
    root.mkdir()
    node_binary = root / "node"
    _write_executable(node_binary)
    dist = root / "dist"
    dist.mkdir()
    real_entry = dist / "index.js"
    real_entry.write_text("#!/usr/bin/env node\n// fixture entry that calls runAcp()\n", encoding="utf-8")
    library_module = dist / "acp-agent.js"
    library_module.write_text("// fixture library module: only exports runAcp, never calls it\n", encoding="utf-8")
    package_json = root / "package.json"
    package_json.write_text(
        json.dumps({
            "name": "@agentclientprotocol/claude-agent-acp",
            "version": "0.78.0",
            "bin": {"claude-agent-acp": "dist/index.js"},
        }),
        encoding="utf-8",
    )
    install = ClaudeAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(library_module),
        package_json=str(package_json),
        package_version="0.78.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(library_module),
    )
    environ = _environ(tmp_path)
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, environ)
    assert excinfo.value.code == "engine_install_drifted"
    assert excinfo.value.field == "adapter_entry"

    # The correct entry point (matching package.json's own "bin" field)
    # is accepted.
    correct_install = ClaudeAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(real_entry),
        package_json=str(package_json),
        package_version="0.78.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(real_entry),
    )
    verified = verify_install(correct_install, environ)
    assert verified is correct_install


def test_verify_install_rejects_package_version_mismatch(tmp_path):
    root, node_binary, adapter_entry, package_json = _make_install_dir(tmp_path, version="0.78.0")
    install = ClaudeAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="0.77.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_drifted"


def test_verify_install_rejects_relative_path_component(tmp_path):
    install, root = _make_install(tmp_path)
    sneaky = str(root / "sub" / ".." / "index.js")
    tampered = ClaudeAcpInstall(
        node_binary=install.node_binary,
        adapter_entry=sneaky,
        package_json=install.package_json,
        package_version=install.package_version,
        package_json_sha256=install.package_json_sha256,
        adapter_entry_sha256=install.adapter_entry_sha256,
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
    tampered = ClaudeAcpInstall(
        node_binary=install.node_binary,
        adapter_entry=str(link),
        package_json=install.package_json,
        package_version=install.package_version,
        package_json_sha256=install.package_json_sha256,
        adapter_entry_sha256=install.adapter_entry_sha256,
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(tampered, _environ(tmp_path))
    assert excinfo.value.code == "engine_install_untrusted_location"


@pytest.mark.parametrize(
    "relative_parts",
    [
        (".local", "share", "zed", "external_agents", "registry", "npx", "claude-acp"),
        (".local", "share", "app.codeg", "claude-acp"),
        (".config", "app.codeg", "claude-acp"),
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
    package_json.write_text(json.dumps({"name": "@agentclientprotocol/claude-agent-acp", "version": "0.78.0"}), encoding="utf-8")
    install = ClaudeAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="0.78.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
    )
    with pytest.raises(PortableError) as excinfo:
        verify_install(install, {"HOME": str(home)})
    assert excinfo.value.code == "engine_install_untrusted_location"


def test_verify_install_rejects_untrusted_location_when_home_is_a_symlink(tmp_path):
    real_home_target = tmp_path / "real-home"
    real_home_target.mkdir()
    home_symlink = tmp_path / "home-symlink"
    try:
        home_symlink.symlink_to(real_home_target, target_is_directory=True)
    except OSError:
        pytest.skip("symlinks are not supported in this environment")

    untrusted_root = real_home_target / ".local" / "share" / "zed" / "external_agents" / "registry" / "npx" / "claude-acp"
    untrusted_root.mkdir(parents=True)
    node_binary = untrusted_root / "node"
    _write_executable(node_binary)
    adapter_entry = untrusted_root / "index.js"
    adapter_entry.write_text("// synthetic\n", encoding="utf-8")
    package_json = untrusted_root / "package.json"
    package_json.write_text(json.dumps({"name": "@agentclientprotocol/claude-agent-acp", "version": "0.78.0"}), encoding="utf-8")

    install = ClaudeAcpInstall(
        node_binary=str(node_binary),
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="0.78.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
    )
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
        ClaudeAcpInstall(
            node_binary="node",  # relative, never accepted
            adapter_entry=install.adapter_entry,
            package_json=install.package_json,
            package_version=install.package_version,
            package_json_sha256=install.package_json_sha256,
            adapter_entry_sha256=install.adapter_entry_sha256,
        )
    assert excinfo.value.code == "invalid_field"


def test_construction_rejects_malformed_hash(tmp_path):
    install, _root = _make_install(tmp_path)
    with pytest.raises(PortableError) as excinfo:
        ClaudeAcpInstall(
            node_binary=install.node_binary,
            adapter_entry=install.adapter_entry,
            package_json=install.package_json,
            package_version=install.package_version,
            package_json_sha256="not-a-hex-digest",
            adapter_entry_sha256=install.adapter_entry_sha256,
        )
    assert excinfo.value.code == "invalid_field"


def test_launch_command_for_requires_claude_acp_install_instance():
    with pytest.raises(PortableError) as excinfo:
        launch_command_for(object())
    assert excinfo.value.code == "invalid_field"
