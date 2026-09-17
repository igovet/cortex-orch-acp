"""Tests for the T031 pinned-dependency manifest/verifier."""
from __future__ import annotations

import json

import pytest

from cortex_runtime.packaging import pinned


def test_real_manifest_verifies_clean():
    manifest = pinned.load_manifest()
    findings = pinned.verify_all(manifest)
    bad = [f for f in findings if f.status != "ok"]
    assert not bad, bad


def test_launch_critical_is_a_strict_subset_of_verify_all():
    manifest = pinned.load_manifest()
    launch = {f.component for f in pinned.verify_launch_critical(manifest)}
    full = {f.component for f in pinned.verify_all(manifest)}
    assert launch <= full
    assert launch == {"python_version_floor", "portable_contract_version"}


def test_missing_manifest_file_raises(tmp_path):
    with pytest.raises(pinned.PinnedManifestError):
        pinned.load_manifest(tmp_path / "nope.json")


def test_malformed_manifest_raises(tmp_path):
    path = tmp_path / "pinned.json"
    path.write_text("not json", encoding="utf-8")
    with pytest.raises(pinned.PinnedManifestError):
        pinned.load_manifest(path)


def test_wrong_schema_version_raises(tmp_path):
    path = tmp_path / "pinned.json"
    path.write_text(json.dumps({"schema_version": 99}), encoding="utf-8")
    with pytest.raises(pinned.PinnedManifestError):
        pinned.load_manifest(path)


def test_python_floor_mismatch_is_reported_not_raised():
    manifest = {"python_version_floor": [99, 0]}
    finding = pinned.verify_python_floor(manifest)
    assert finding.status == "mismatch"
    assert finding.repair_action is not None


def test_python_floor_missing_in_manifest():
    finding = pinned.verify_python_floor({})
    assert finding.status == "missing"


def test_contract_version_mismatch_reported():
    finding = pinned.verify_contract_version({"portable_contract_version": 12345})
    assert finding.status == "mismatch"


def test_schema_fixture_missing_file(tmp_path):
    manifest = {"acp_schema_fixture": {"path": "no/such/file.json", "sha256": "x" * 64, "version": "0"}}
    finding = pinned.verify_schema_fixture(manifest, repo_root=tmp_path)
    assert finding.status == "missing"


def test_schema_fixture_digest_mismatch(tmp_path):
    fixture = tmp_path / "schema.json"
    fixture.write_text("{}", encoding="utf-8")
    manifest = {"acp_schema_fixture": {"path": "schema.json", "sha256": "0" * 64, "version": "0"}}
    finding = pinned.verify_schema_fixture(manifest, repo_root=tmp_path)
    assert finding.status == "mismatch"


def test_engine_pins_missing_entry_reported():
    findings = pinned.verify_engine_pins({"engines": {"codex-acp": {}}})
    assert findings[0].status == "missing"


def test_engine_pins_empty_manifest_reported():
    findings = pinned.verify_engine_pins({})
    assert findings[0].status == "missing"


def test_never_reads_any_credential_or_secret_path():
    """Static guard: this module must never reference connection secrets,
    credential registries, or engine install verification (that stays a
    separate, operator-driven, opt-in path -- see the module docstring)."""
    import inspect

    source = inspect.getsource(pinned)
    for forbidden in ("credential", "secret", "keyring", "CODEX_API_KEY", "ANTHROPIC_API_KEY"):
        assert forbidden not in source
