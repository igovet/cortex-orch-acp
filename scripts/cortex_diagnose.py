#!/usr/bin/env python3
"""Non-paid Cortex ACP diagnostics command (T031).

Checks storage layout/permissions, the portable schema/contract version,
pinned-dependency digests, connection/route availability, an optional
client-registration presence check, adapter capability/qualification
status, and (with ``--project``) the project's own policy-ceiling
configuration -- all read-only, all offline, and **never** a model call of
any kind. Machine-readable JSON on stdout by default (``--format text``
for a human summary); exit code distinguishes ok (0) / degraded (1, at
least one repairable gap) / failed (2, a hard/unsafe condition).

This script never imports, execs, or otherwise reaches
``cortex_claude_probe.py``/``cortex_codex_probe.py`` (the separate,
explicit, ``--i-authorize-paid-call``-gated opt-in probes) -- see
``tests/test_cortex_diagnose.py``'s static-import guard for the enforced
boundary.
"""
from __future__ import annotations

import argparse
import json
import os
import stat
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS.parent / "src"))

from cortex_runtime.acp import config_options as acp_config_options  # noqa: E402
from cortex_runtime.configuration import store as configuration_store  # noqa: E402
from cortex_runtime.connections import discovery, registry  # noqa: E402
from cortex_runtime.connections.model import AuthMode, SUPPORT_MATRIX  # noqa: E402
from cortex_runtime.engine.instruction_delivery_registry import (  # noqa: E402
    ClaudeAgentAcp0_78Delivery, ClaudeAgentAcp0_87Delivery, ClaudeAgentAcp0_88Delivery, CodexAcpDelivery, CodexAcp2_1Delivery, delivery_for,
)
from cortex_runtime.portable.errors import PortableError  # noqa: E402
from cortex_runtime.packaging import pinned  # noqa: E402

_STATUS_RANK = {"ok": 0, "skip": 0, "warn": 1, "fail": 2}
_EXIT_CODE = {"ok": 0, "warn": 1, "fail": 2}
_QUALIFICATION_BY_CLASS = {
    CodexAcpDelivery: "qualified",
    CodexAcp2_1Delivery: "qualified",
    ClaudeAgentAcp0_78Delivery: "qualified",
    ClaudeAgentAcp0_87Delivery: "qualified",
    ClaudeAgentAcp0_88Delivery: "qualified",
}


def _check(name, status, detail, repair_action=None):
    return dict(name=name, status=status, detail=detail, repair_action=repair_action)


def _pinned_checks():
    checks = []
    try:
        manifest = pinned.load_manifest()
    except pinned.PinnedManifestError as error:
        return [_check("pinned_dependency_manifest", "fail", str(error),
                        "Restore ./pinned-dependencies.json from version control.")]
    status_map = {"ok": "ok", "mismatch": "fail", "missing": "warn"}
    for finding in pinned.verify_all(manifest):
        checks.append(_check(f"pinned:{finding.component}", status_map[finding.status],
                              finding.detail, finding.repair_action))
    return checks


def _storage_checks(environ):
    checks = []
    for label, getter in (("config_root", registry.config_root), ("state_root", registry.state_root)):
        try:
            path = getter(environ)
        except PortableError as error:
            checks.append(_check(f"storage:{label}", "fail", error.remediation,
                                  "Fix HOME/XDG_CONFIG_HOME/XDG_STATE_HOME to a safe, absolute, non-project path."))
            continue
        if not path.exists():
            checks.append(_check(f"storage:{label}", "warn", f"{path} does not exist yet.",
                                  "Run scripts/cortex_acp_setup.py init-storage."))
            continue
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            checks.append(_check(f"storage:{label}", "fail", f"{path} is not an owner-private (0700) real directory.",
                                  f"Remove/recreate {path} (0700, owned by the current user, never a symlink)."))
            continue
        checks.append(_check(f"storage:{label}", "ok", f"{path} is an owner-private directory."))
    return checks


def _connection_and_route_checks(environ):
    checks = []
    try:
        state = registry.read_registry(environ)
    except PortableError as error:
        return [_check("connections", "fail", error.remediation, "Repair or reinitialize the connection registry.")]
    if not state.connections:
        checks.append(_check("connections", "warn", "No connection is registered.",
                              "Run scripts/cortex_acp_setup.py add-connection."))
    else:
        for connection_id, record in sorted(state.connections.items()):
            connection_status = "ok" if record.state.value == "configured" else "warn"
            repair = None if connection_status == "ok" else (
                "Complete this connection's native login or re-set its API key "
                "(scripts/cortex_acp_setup.py add-connection with the same name/engine/provider, --no-reuse omitted)."
            )
            checks.append(_check(f"connection:{connection_id}", connection_status,
                                  f"{record.name} ({record.engine}, {record.auth_mode.value}) is {record.state.value}.",
                                  repair))
    try:
        defaults = configuration_store.read_user_defaults(environ)["defaults"]
    except PortableError as error:
        checks.append(_check("pinned_route", "fail", error.remediation, "Repair the user-default configuration file."))
        defaults = {}
    route_id = defaults.get("route_id") if isinstance(defaults, dict) else None
    if not route_id:
        checks.append(_check("pinned_route", "warn", "No default route is pinned.",
                              "Run scripts/cortex_acp_setup.py pin-route."))
    else:
        selectable = acp_config_options.selectable_entries_from_snapshots(state.connections, environ)
        if any(entry.route.id == route_id for entry in selectable):
            checks.append(_check("pinned_route", "ok", f"Default route {route_id} is currently selectable."))
        else:
            checks.append(_check("pinned_route", "warn", f"Default route {route_id} is no longer selectable.",
                                  "Run scripts/cortex_acp_setup.py pin-route again with a currently valid connection/model."))
    return checks


def _discovery_checks(environ):
    """D-T035-02: report which supported engines were actually found
    installed for the client (e.g. Zed), and which were not -- never a
    guess either way. A discovered engine's connection then shares its
    native credential directory with the client's own engine process
    (see ``connections.discovery`` module docstring): that is an accepted
    race on token refresh, not a hidden one, named here too.
    """
    checks = []
    try:
        found = discovery.discover_all(environ=environ)
    except Exception as error:  # noqa: BLE001 - diagnostics never raises
        return [_check("discovery", "warn", f"Engine discovery failed unexpectedly: {error}", None)]
    for engine in discovery.SUPPORTED_ENGINES:
        info = found.get(engine)
        if info is None:
            checks.append(_check(f"discovery:{engine}", "skip", f"{engine} is not installed for this client.",
                                  "Configure this engine in the client (e.g. Zed agent_servers) to use it."))
        else:
            checks.append(_check(
                f"discovery:{engine}", "ok",
                f"{engine} {info.version} found at {info.launch_command[1]}; shares credentials at "
                f"{info.native_credential_dir} with the client's own engine process (token-refresh race accepted, "
                "see D-T035-02).",
                None,
            ))
    return checks


def _client_registration_check(zed_settings):
    if zed_settings is None:
        return [_check("client_registration:zed", "skip",
                        "Not checked -- pass --zed-settings PATH to check a specific settings file.", None)]
    import cortex_acp_client_config as client_config

    path = Path(zed_settings)
    try:
        client_config._refuse_if_symlinked(path)
    except client_config.SettingsPathUnsafe as error:
        return [_check("client_registration:zed", "fail", str(error), "Pass the real, non-symlinked settings path.")]
    data, _raw = client_config._read_strict_json(path)
    if data is None and path.exists():
        return [_check("client_registration:zed", "warn", f"{path} is not strict JSON (JSONC); cannot be checked automatically.",
                        "Add the Cortex agent_servers entry manually; see docs/acp/zed-custom-agent.md.")]
    entry = client_config.build_entry()
    state = client_config.describe_state(data, entry)
    status_map = {"identical": "ok", "would_add": "warn", "would_replace": "warn"}
    repair = None if state == "identical" else (
        f"python3 scripts/cortex_acp_client_config.py --settings {path} --apply --i-consent-to-write"
    )
    return [_check("client_registration:zed", status_map[state], f"{path}: {state}.", repair)]


def _adapter_qualification_checks():
    checks = []
    for engine, version in (("codex-acp", "1.12.0"), ("codex-acp", "1.13.0"), ("codex-acp", "2.0.1"), ("codex-acp", "2.1.1"), ("claude-agent-acp", "0.78.0"), ("claude-agent-acp", "0.87.0"), ("claude-agent-acp", "0.88.0")):
        delivery = delivery_for(engine, version)
        qualification = _QUALIFICATION_BY_CLASS.get(type(delivery), "unsupported")
        checks.append(_check(f"adapter_qualification:{engine}:{version}", "ok" if qualification != "unsupported" else "warn",
                              f"instruction delivery is {qualification}.",
                              None if qualification != "unsupported" else "No qualified instruction-delivery channel for this engine/version."))
        for auth_mode in AuthMode:
            support = SUPPORT_MATRIX.get(engine, {}).get(auth_mode)
            if support is None:
                continue
            status = "ok" if support.qualified else "warn"
            checks.append(_check(f"auth_support:{engine}:{auth_mode.value}", status,
                                  f"{support.qualification} ({support.basis}).",
                                  None if support.qualified else "Not a qualified auth mode for this engine yet."))
    return checks


def _checkout_package_version():
    """This checkout's own stamped version (``scripts/cortex_package.py
    stamp`` writes it) -- may differ from what a running Paseo agent
    process actually loaded; see :func:`_package_version_checks`."""
    try:
        return json.loads((SCRIPTS.parent / "package-identity.json").read_text(encoding="utf-8"))["version"]
    except (OSError, ValueError, KeyError, TypeError):
        return None


def _installed_paseo_runtime_versions(environ):
    """Every version directory ``scripts/cortex_acp_setup.py
    bootstrap-paseo-git-install`` has ever staged under this host's
    ``$XDG_STATE_HOME/cortex/paseo-runtime/`` -- the actual code a live
    Paseo agent process runs, independent of whatever this checkout
    currently contains (e.g. after a checkout is advanced past the last
    install). ``[]`` (never an exception) when the directory does not
    exist yet or cannot be listed."""
    from cortex_runtime.connections import registry

    try:
        root = registry.state_root(environ) / "paseo-runtime"
    except PortableError:
        return []
    if not root.is_dir():
        return []
    versions = []
    for entry in root.iterdir():
        try:
            if entry.is_dir() and not entry.is_symlink():
                versions.append((entry.stat().st_mtime, entry.name))
        except OSError:
            continue
    versions.sort()
    return [name for _mtime, name in versions]


def _package_version_checks(environ):
    """P1 audit follow-up: report the installed runtime package version
    separately from this checkout's own version -- a live Paseo agent
    process runs whatever was last staged under ``paseo-runtime/``, which
    can lag behind (or, mid-development, lead) the checkout this script is
    invoked from (2026-09-25 audit: production ran commit 9a70dee9 while
    the checkout under test already had newer fixes)."""
    checkout_version = _checkout_package_version()
    if checkout_version is None:
        return [_check("package_version", "fail", "This checkout's package-identity.json is missing or invalid.",
                        "Run scripts/cortex_package.py stamp from a clean checkout.")]
    installed = _installed_paseo_runtime_versions(environ)
    if not installed:
        return [_check("package_version", "warn",
                        f"This checkout is stamped {checkout_version}; no Paseo runtime is installed yet for this host.",
                        "Run scripts/cortex_acp_setup.py bootstrap-paseo-git-install to install it.")]
    newest_installed = installed[-1]
    if newest_installed == checkout_version:
        return [_check("package_version", "ok",
                        f"The installed Paseo runtime ({newest_installed}) matches this checkout's stamped version.")]
    return [_check(
        "package_version", "warn",
        f"The installed Paseo runtime ({newest_installed}) differs from this checkout's stamped version "
        f"({checkout_version}); a live Paseo agent process runs the installed version, not this checkout.",
        "Run scripts/cortex_acp_setup.py bootstrap-paseo-git-install after stamping this checkout "
        "(scripts/cortex_package.py stamp) to install the current code.",
    )]


def _policy_checks(project, environ):
    if project is None:
        return [_check("policy:project_ceiling", "skip", "Not checked -- pass --project PATH to check a project's ceiling.", None)]
    from cortex_runtime.project_storage import canonical_project, project_store_directory

    try:
        canonical = canonical_project(project)
    except Exception as error:
        return [_check("policy:project_ceiling", "fail", f"invalid project path: {error}", "Pass an absolute, existing project directory.")]
    directory = project_store_directory(canonical)
    if not (directory / "cortex.sqlite3").exists():
        return [_check("policy:project_ceiling", "warn", f"No Cortex project store bound yet at {directory}.",
                        "No action needed until this project's first task is created.")]
    return [_check("policy:project_ceiling", "ok", f"Project store schema present at {directory}.", None)]


def run_diagnostics(*, environ=None, zed_settings=None, project=None):
    checks = []
    checks += _pinned_checks()
    checks += _package_version_checks(environ)
    checks += _storage_checks(environ)
    checks += _connection_and_route_checks(environ)
    checks += _discovery_checks(environ)
    checks += _client_registration_check(zed_settings)
    checks += _adapter_qualification_checks()
    checks += _policy_checks(project, environ)
    worst = max((_STATUS_RANK[c["status"]] for c in checks), default=0)
    overall = {0: "ok", 1: "degraded", 2: "failed"}[worst]
    return dict(overall=overall, checks=checks)


def _print_text(report):
    print(f"overall: {report['overall']}")
    for check in report["checks"]:
        line = f"[{check['status']:>4}] {check['name']}: {check['detail']}"
        if check["repair_action"]:
            line += f"  (repair: {check['repair_action']})"
        print(line)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--format", choices=("json", "text"), default="json")
    parser.add_argument("--zed-settings", type=Path, default=None)
    parser.add_argument("--project", type=Path, default=None)
    args = parser.parse_args(argv)

    report = run_diagnostics(zed_settings=args.zed_settings, project=args.project)
    if args.format == "json":
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        _print_text(report)
    exit_code = {"ok": 0, "degraded": 1, "failed": 2}[report["overall"]]
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
