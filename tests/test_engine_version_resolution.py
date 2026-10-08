"""The engine version delivery/compaction registrations are looked up by."""
from __future__ import annotations

import json
from types import SimpleNamespace

from cortex_runtime.delegation.live_adapters import _pinned_version_for
from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.instruction_delivery import DeliveryQualification
from cortex_runtime.portable.roles import load_role


def _record(tmp_path, engine, package, version):
    package_dir = tmp_path / engine / version / "node_modules" / package
    (package_dir / "dist").mkdir(parents=True)
    (package_dir / "dist" / "index.js").write_text("")
    (package_dir / "package.json").write_text(json.dumps({"version": version, "bin": {engine: "dist/index.js"}}))
    return SimpleNamespace(engine=engine, launch_command=("/usr/bin/node", str(package_dir / "dist" / "index.js")))


def test_an_unregistered_claude_version_does_not_borrow_qualification(tmp_path):
    record = _record(tmp_path, "claude-agent-acp", "@agentclientprotocol/claude-agent-acp", "0.99.0")
    version = _pinned_version_for("claude-agent-acp", record)
    assert version == "0.99.0"
    plan = delivery_for("claude-agent-acp", version).plan(load_role("worker-debugger"), [])
    assert plan.qualification == DeliveryQualification.UNSUPPORTED


def test_registered_claude_versions_name_the_actual_install(tmp_path):
    for version in ("0.78.0", "0.87.0", "0.88.0"):
        record = _record(tmp_path, "claude-agent-acp", "@agentclientprotocol/claude-agent-acp", version)
        assert _pinned_version_for("claude-agent-acp", record) == version
        plan = delivery_for("claude-agent-acp", version).plan(load_role("worker-debugger"), [])
        assert plan.version == version
        assert plan.qualification == DeliveryQualification.QUALIFIED


def test_a_registered_installed_version_is_the_one_named(tmp_path):
    for version in ("1.13.0", "2.0.1", "2.1.1"):
        record = _record(tmp_path, "codex-acp", "@agentclientprotocol/codex-acp", version)
        assert _pinned_version_for("codex-acp", record) == version
        plan = delivery_for("codex-acp", version).plan(load_role("worker-debugger"), [])
        assert plan.qualification == DeliveryQualification.QUALIFIED


def test_no_record_resolves_the_pinned_version():
    assert _pinned_version_for("claude-agent-acp") == "0.88.0"
    assert _pinned_version_for("codex-acp") == "2.1.1"


def test_a_worker_failure_names_its_code_and_line():
    """Every failed Claude worker was recorded as a bare `PortableError`."""
    from cortex_runtime.acp.orchestrating_runtime import _describe_failure
    from cortex_runtime.portable.errors import PortableError, PortableErrorCategory

    try:
        raise PortableError(category=PortableErrorCategory.VALIDATION, code="instruction_delivery_unqualified",
                            remediation="secret /home/user/path must not leak", retry_eligible=False)
    except PortableError as error:
        described = _describe_failure(error)
    assert described["error"] == "PortableError:instruction_delivery_unqualified"
    assert described["where"].startswith("test_engine_version_resolution.py:")
    assert "secret" not in str(described) and "/home/user" not in str(described)


def test_discovery_uses_declared_registered_version_not_directory_label(tmp_path):
    from cortex_runtime.connections import discovery
    _record(tmp_path, "claude-agent-acp", "@agentclientprotocol/claude-agent-acp", "0.88.0")
    unknown = _record(tmp_path, "claude-agent-acp", "@agentclientprotocol/claude-agent-acp", "0.89.0")
    # A misleading directory label must not lend its package qualification.
    from pathlib import Path
    Path(unknown.launch_command[1]).parents[4].rename(tmp_path / "claude-agent-acp" / "0.78.0")
    found = discovery.discover_engine(
        "claude-agent-acp", registry_root=tmp_path / "claude-agent-acp", node_binary="/usr/bin/node",
    )
    assert found.version == "0.88.0"
