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


def test_an_unregistered_installed_version_keeps_the_pinned_registration(tmp_path):
    """claude-agent-acp is installed at 0.81.0 while only 0.78.0 has its own
    registration. Resolving the installed version made every Claude worker
    refuse its first prompt with an UNSUPPORTED delivery (a bare
    `PortableError`, live 2026-09-22) while the coordinator kept working."""
    record = _record(tmp_path, "claude-agent-acp", "@agentclientprotocol/claude-agent-acp", "0.81.0")
    version = _pinned_version_for("claude-agent-acp", record)
    assert version == "0.78.0"
    plan = delivery_for("claude-agent-acp", version).plan(load_role("worker-debugger"), [])
    assert plan.qualification == DeliveryQualification.QUALIFIED


def test_a_registered_installed_version_is_the_one_named(tmp_path):
    for version in ("1.13.0", "2.0.1"):
        record = _record(tmp_path, "codex-acp", "@agentclientprotocol/codex-acp", version)
        assert _pinned_version_for("codex-acp", record) == version
        plan = delivery_for("codex-acp", version).plan(load_role("worker-debugger"), [])
        assert plan.qualification == DeliveryQualification.QUALIFIED


def test_no_record_resolves_the_pinned_version():
    assert _pinned_version_for("claude-agent-acp") == "0.78.0"
    assert _pinned_version_for("codex-acp") == "1.12.0"


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
