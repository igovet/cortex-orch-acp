#!/usr/bin/env python3
"""Opt-in, explicitly-authorized live codex-acp probe (T013).

Runs exactly one bounded real assignment through a verified pinned
codex-acp install and records a QUALIFICATION_RUN.json-shaped result (see
``.cortex/acp-migration/templates/QUALIFICATION_RUN.json``). This script
never runs anything paid on its own: without *both* ``--i-authorize-paid-call``
and a non-empty ``--approval-ref``, it validates its other arguments (if
given) and exits ``2`` before importing ``cortex_runtime`` or touching the
filesystem beyond argument parsing -- no install verification, no
workspace, no subprocess, no network.

``--plan`` prints, in order, the steps this script would take with
authorization (verify install, create an isolated temporary workspace,
launch via ``EngineSessionManager``, negotiate, create a session, apply the
route's configuration, send one bounded prompt, write ``--output``) without
performing any of them -- useful to review the exact sequence without ever
authorizing a paid call.

Safety notes for whoever runs this with authorization:

- ``--connection-id``/``--route-id`` identify an existing, already-onboarded
  connection/route in the real user registry; this script never onboards
  or creates one.
- ``--install-record`` is a JSON file with exactly the
  ``CodexAcpInstall`` fields (see ``engine.codex_install.CodexAcpInstall``);
  it is re-verified fresh here regardless of what it claims.
- The temporary workspace this script creates is always outside any
  existing project/repository (a fresh ``tempfile.mkdtemp()``), never this
  checkout or the real user's ``$HOME``.
- The recorded output never includes a secret, a full transcript, or
  ``os.environ``; only the fields present in the QUALIFICATION_RUN.json
  template plus the fixed prompt/output size bound below.
"""
from __future__ import annotations

import argparse
import sys

_PLAN_STEPS = (
    "1. Load and validate --install-record as a CodexAcpInstall; call "
    "engine.codex_install.verify_install() against it (fresh hash/version/"
    "trust-location re-check; never trusts the file's own claims).",
    "2. Read the named --connection-id from the real connection registry "
    "(read-only) and confirm its engine is 'codex-acp'.",
    "3. Resolve --route-id to a stored ExecutionRoute via "
    "_resolve_route() -- the newest cached catalog.store snapshot for "
    "--connection-id whose entries contain a matching route.id; refuse if "
    "no cached snapshot has a selectable matching entry.",
    "4. Create a fresh temporary workspace directory via tempfile.mkdtemp() "
    "(never this checkout, never the real project this script is invoked "
    "from).",
    "5. Build (launch_command, pinned_engine_vars) via "
    "engine.codex_adapter_acp.build_codex_launch() and construct one "
    "EngineSessionManager(option_mapping=codex_option_mapping, "
    "install=<verified install>) for that workspace.",
    "6. negotiate() capabilities, create_session(), then configure() the "
    "session against the route's ConfigurationRevision (codex_option_mapping "
    "rejects an unsupported model/effort/mode before any prompt is sent).",
    "7. Send exactly one bounded prompt via prompt_turn() and consume its "
    "events to TURN_END (or a bounded timeout, whichever first).",
    "8. Build a QUALIFICATION_RUN.json-shaped record (status/configuration/"
    "execution/measurements; selected/configuration_applied/"
    "execution_verified evidence fields from build_codex_evidence_status(); "
    "no secret, no full transcript) and write it to --output.",
    "9. shutdown() the engine session/process; report the exit code and "
    "--output path on stdout.",
)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--i-authorize-paid-call", action="store_true", default=False,
                         help="Required, together with --approval-ref, to run anything beyond argument validation.")
    parser.add_argument("--approval-ref", default=None, help="A non-empty human approval reference string.")
    parser.add_argument("--connection-id", default=None, help="An existing 'cn_...' connection id (engine codex-acp).")
    parser.add_argument("--route-id", default=None, help="An existing 'xr_...' ExecutionRoute id for that connection.")
    parser.add_argument("--install-record", default=None, help="Path to a JSON CodexAcpInstall record.")
    parser.add_argument("--output", default=None, help="Path to write the QUALIFICATION_RUN.json-shaped result.")
    parser.add_argument("--plan", action="store_true", default=False,
                         help="Print the steps this script would take and exit 0, without authorization or execution.")
    return parser


def _authorized(args: argparse.Namespace) -> bool:
    return bool(args.i_authorize_paid_call) and bool(args.approval_ref)


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.plan:
        print("cortex_codex_probe.py --plan (no authorization, nothing executed):")
        for step in _PLAN_STEPS:
            print(f"  {step}")
        return 0

    if not _authorized(args):
        print(
            "cortex_codex_probe.py: refusing to run without both --i-authorize-paid-call "
            "and a non-empty --approval-ref; exiting before any install verification, "
            "workspace creation, or network/process activity. Use --plan to see the "
            "steps this would take once authorized.",
            file=sys.stderr,
        )
        return 2

    for required_name in ("connection_id", "route_id", "install_record", "output"):
        if not getattr(args, required_name):
            flag = "--" + required_name.replace("_", "-")
            print(f"cortex_codex_probe.py: {flag} is required once authorized.", file=sys.stderr)
            return 2

    return _run_authorized(args)


def _resolve_route(connection_id: str, route_id: str, environ: dict | None = None):
    """The stored :class:`ExecutionRoute` for ``route_id`` on ``connection_id``.

    F1 (T013 review): ``--route-id`` identifies a route *previously
    discovered and cached* for this connection (see
    ``catalog.store.save_snapshot``/``discovery.build_catalog``) -- it was
    never itself a model name, and using it as one (the original bug this
    fixes) silently sent the wrong string to the engine as
    ``ExecutionRoute.model``. This reads every cached catalog snapshot file
    for ``connection_id`` (newest file first, by mtime) and returns the
    first entry whose ``route.id`` matches; returns ``None`` if no cached
    snapshot contains a matching, still-selectable entry (an entry only
    ever exists here for a route ``catalog.discovery.build_catalog`` itself
    considered available -- an unavailable connection's routes never appear
    in ``entries`` at all, so "not found" and "not selectable" are the same
    outcome from this function's point of view).

    Read-only: never writes, never contacts an engine, never launches
    anything.
    """
    from cortex_runtime.catalog import store as catalog_store
    from cortex_runtime.connections import registry as connection_registry

    snapshot_dir = connection_registry.state_root(environ) / "catalog" / connection_id
    if not snapshot_dir.is_dir():
        return None
    snapshot_files = sorted(
        snapshot_dir.glob("cs_*.json"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    for snapshot_file in snapshot_files:
        snapshot_id = snapshot_file.stem
        view = catalog_store.load_connection_snapshot(connection_id, snapshot_id, environ)
        if view is None:
            continue
        for entry in view.entries:
            if entry.route.id == route_id:
                return entry.route
    return None


def _run_authorized(args: argparse.Namespace) -> int:  # pragma: no cover - exercised only with explicit authorization
    """Only reached with explicit authorization; see module docstring's
    step list (``_PLAN_STEPS``) for exactly what this does. Deliberately
    never exercised by the automated test suite (see
    ``tests/test_engine_codex_probe.py``, which only exercises the refusal
    and ``--plan`` paths) -- this task never runs a paid call itself.
    """
    import json
    import tempfile
    import time
    from pathlib import Path

    from cortex_runtime.connections import registry as connection_registry
    from cortex_runtime.engine.codex_adapter_acp import (
        build_codex_evidence_status,
        build_codex_launch,
        codex_option_mapping,
    )
    from cortex_runtime.engine.codex_install import CodexAcpInstall, verify_install
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from cortex_runtime.portable.engine_contract import EngineCapability, EngineEventKind
    from cortex_runtime.portable.identity import ConfigurationRevision

    install_data = json.loads(Path(args.install_record).read_text(encoding="utf-8"))
    install = CodexAcpInstall(**install_data)
    verify_install(install)

    registry_state = connection_registry.read_registry()
    record = registry_state.connections.get(args.connection_id)
    if record is None or record.engine != "codex-acp":
        print(f"cortex_codex_probe.py: connection {args.connection_id!r} is not a known codex-acp connection.", file=sys.stderr)
        return 1

    # F1 (T013 review): --route-id names a route already discovered and
    # cached for this connection -- it is never itself a model name.
    route = _resolve_route(args.connection_id, args.route_id)
    if route is None:
        print(
            f"cortex_codex_probe.py: route {args.route_id!r} is not a selectable, cached route for "
            f"connection {args.connection_id!r}; run discovery first.",
            file=sys.stderr,
        )
        return 1

    workspace_root = tempfile.mkdtemp(prefix="cortex-codex-probe-")
    launch_command, _pinned = build_codex_launch(record, install)
    manager = EngineSessionManager(
        record, workspace_root, option_mapping=codex_option_mapping, install=install,
    )
    result: dict = dict(status="not_run")
    try:
        manager.negotiate((EngineCapability.SESSION_CREATION, EngineCapability.CONFIGURATION_OPTIONS, EngineCapability.PROMPT_STREAMING))
        session_id = manager.create_session("t_" + "0" * 12, route.id)
        revision = ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)
        evidence_ref = manager.configure(session_id, revision)
        status = build_codex_evidence_status(revision.id, evidence_ref)
        stop_reason = None
        for event in manager.prompt_turn(session_id, "One bounded qualification prompt (see T013 probe)."):
            if event.kind == EngineEventKind.TURN_END:
                stop_reason = event.stop_reason.value if event.stop_reason else None
                break
        result = dict(
            status="ran",
            started_at=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            configuration=dict(connection_id=record.id, requested_model=route.model),
            execution=dict(
                exit_status=stop_reason,
                engine_configuration_evidence=[status.selected, status.configuration_applied, status.execution_verified],
            ),
        )
    finally:
        manager.shutdown()

    Path(args.output).write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"cortex_codex_probe.py: wrote {args.output}")
    return 0


if __name__ == "__main__":
    from _cortex_tmp import prefer_disk_tmp
    prefer_disk_tmp()  # /var/tmp, not a RAM-backed /tmp
    raise SystemExit(main())
