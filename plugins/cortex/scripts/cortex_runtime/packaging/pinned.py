"""Pinned dependency manifest and verifier (T031).

``plugins/cortex/pinned-dependencies.json`` is the single source of truth
for the Python version floor, this package's own portable contract
version, the vendored ACP schema fixture's pinned digest, and each
supported engine adapter's pinned package version. :func:`verify_all`
re-checks every one of those *fresh* every call (never a cached verdict)
and always returns a :class:`Finding` for it -- ``"ok"``, ``"mismatch"``,
or ``"missing"`` -- naming a concrete repair action; nothing here ever
downloads, installs, or upgrades anything, and a mismatch is always
reported, never silently accepted.

This module is imported by ``cortex_acp.py`` at every launch (cheap,
in-repo checks only: Python floor + contract version) and by
``scripts/cortex_diagnose.py`` (the full set, including the schema fixture
digest and the engine pin listing) -- it never imports any paid-probe
script (``cortex_codex_probe.py``/``cortex_claude_probe.py``) and never
spawns a subprocess or opens a network socket.
"""
from __future__ import annotations

import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path

from ..portable import version as portable_version

_PLUGIN_ROOT = Path(__file__).resolve().parents[3]
_REPO_ROOT = _PLUGIN_ROOT.parents[1]
MANIFEST_PATH = _PLUGIN_ROOT / "pinned-dependencies.json"


class PinnedManifestError(Exception):
    """Raised when the pinned-dependency manifest itself cannot be read/parsed."""


@dataclass(frozen=True)
class Finding:
    component: str
    status: str  # "ok" | "mismatch" | "missing"
    detail: str
    repair_action: str | None = None

    def as_dict(self) -> dict:
        return dict(component=self.component, status=self.status, detail=self.detail,
                    repair_action=self.repair_action)


def load_manifest(path: Path = MANIFEST_PATH) -> dict:
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as error:
        raise PinnedManifestError(f"pinned dependency manifest is unreadable at {path}: {error}") from None
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as error:
        raise PinnedManifestError(f"pinned dependency manifest at {path} is not valid JSON: {error}") from None
    if not isinstance(data, dict) or data.get("schema_version") != 1:
        raise PinnedManifestError(f"pinned dependency manifest at {path} has an unrecognized schema_version.")
    return data


def verify_python_floor(manifest: dict) -> Finding:
    floor = manifest.get("python_version_floor")
    if not (isinstance(floor, list) and len(floor) == 2 and all(isinstance(n, int) for n in floor)):
        return Finding("python_version_floor", "missing", "pinned-dependencies.json has no valid python_version_floor.",
                        "Add a two-integer [major, minor] python_version_floor to pinned-dependencies.json.")
    actual = (sys.version_info.major, sys.version_info.minor)
    if actual < tuple(floor):
        return Finding("python_version_floor", "mismatch",
                        f"running Python {actual[0]}.{actual[1]}, below the pinned floor {floor[0]}.{floor[1]}.",
                        f"Install Python {floor[0]}.{floor[1]} or newer and rerun with that interpreter.")
    return Finding("python_version_floor", "ok", f"Python {actual[0]}.{actual[1]} satisfies the pinned floor.")


def verify_contract_version(manifest: dict) -> Finding:
    pinned = manifest.get("portable_contract_version")
    actual = portable_version.CONTRACT_VERSION
    if pinned != actual:
        return Finding("portable_contract_version", "mismatch",
                        f"pinned {pinned!r} does not match the installed cortex_runtime.portable.version.CONTRACT_VERSION {actual!r}.",
                        "The checkout is inconsistent with its own pinned manifest; reinstall/resync this exact revision "
                        "rather than editing either value by hand.")
    return Finding("portable_contract_version", "ok", f"contract version {actual} matches the pinned manifest.")


def verify_schema_fixture(manifest: dict, repo_root: Path = _REPO_ROOT) -> Finding:
    entry = manifest.get("acp_schema_fixture")
    if not isinstance(entry, dict) or not entry.get("path") or not entry.get("sha256"):
        return Finding("acp_schema_fixture", "missing", "pinned-dependencies.json has no valid acp_schema_fixture entry.",
                        "Record {path, version, sha256} for the vendored ACP schema fixture in pinned-dependencies.json.")
    path = repo_root / entry["path"]
    try:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError:
        return Finding("acp_schema_fixture", "missing", f"pinned fixture {path} is not present in this checkout.",
                        f"Restore {entry['path']} from version control (this checkout is incomplete).")
    if digest != entry["sha256"]:
        return Finding("acp_schema_fixture", "mismatch",
                        f"{path} sha256 {digest} does not match the pinned {entry['sha256']}.",
                        "The vendored schema fixture drifted from its pinned digest; restore the pinned revision, "
                        "never edit pinned-dependencies.json to match a changed file.")
    return Finding("acp_schema_fixture", "ok", f"{entry['path']} matches its pinned sha256 (version {entry.get('version', 'unknown')}).")


def verify_engine_pins(manifest: dict) -> tuple[Finding, ...]:
    """One informational finding per pinned engine adapter version.

    This never probes a locally installed copy (that would require running
    node/npm -- forbidden at diagnose time); it only reports what version
    this project has pinned/qualified against, so a mismatch against
    whatever an operator later installs is at least visible from the
    manifest text. A real, digest-verified install record (see
    ``cortex_runtime.engine.codex_install``/``claude_install``) is a
    separate, opt-in, operator-supplied artifact -- never auto-discovered.
    """
    engines = manifest.get("engines")
    if not isinstance(engines, dict) or not engines:
        return (Finding("engines", "missing", "pinned-dependencies.json has no engines entry.",
                         "Record at least one pinned engine adapter version."),)
    findings = []
    for name, entry in sorted(engines.items()):
        version = entry.get("pinned_package_version") if isinstance(entry, dict) else None
        if not version:
            findings.append(Finding(f"engine:{name}", "missing", "no pinned_package_version recorded.",
                                     f"Record a pinned_package_version for {name} in pinned-dependencies.json."))
            continue
        findings.append(Finding(f"engine:{name}", "ok", f"pinned adapter version {version}."))
    return tuple(findings)


def verify_all(manifest: dict | None = None) -> tuple[Finding, ...]:
    manifest = manifest if manifest is not None else load_manifest()
    return (
        verify_python_floor(manifest),
        verify_contract_version(manifest),
        verify_schema_fixture(manifest),
    ) + verify_engine_pins(manifest)


def verify_launch_critical(manifest: dict | None = None) -> tuple[Finding, ...]:
    """The subset of :func:`verify_all` cheap and safe enough to run on
    every ordinary agent launch (no filesystem walk outside the plugin
    directory itself): Python floor and portable contract version only.
    Never the schema fixture (a test-only artifact, absent from some
    deployment layouts) or engine pin listing (informational only)."""
    manifest = manifest if manifest is not None else load_manifest()
    return (verify_python_floor(manifest), verify_contract_version(manifest))
