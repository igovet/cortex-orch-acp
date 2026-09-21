#!/usr/bin/env python3
"""Explicitly authorized single-turn Claude ACP transport probe.

Requires both --i-authorize-paid-call and --approval-ref before runtime imports,
registry reads or subprocesses. Uses an existing registered native account and
route in a disposable project. It observes one configured engine turn only:
no worker cycle, runtime completion or GUI qualification is claimed.
Use --plan for the exact no-execution preview.
"""
from __future__ import annotations

import argparse
import sys

_PLAN_STEPS = (
    "0. STATUS: instruction delivery for pinned 0.78.0 is QUALIFIED from installed evidence; T035/T036 real-client observations are separate and unrun for this rewrite.",
    "1. Read --install-record and verify_install() the exact ClaudeAcpInstall bytes.",
    "2. Read the existing claude-agent-acp connection and cached ExecutionRoute; do not create accounts or copy credentials.",
    "3. Create a disposable temporary workspace outside the checkout.",
    "4. Construct EngineSessionManager with claude_option_mapping, negotiate, create and configure the session.",
    "5. Send one bounded prompt and observe TURN_END or failure. No plan_submit/result_submit/synthesis_submit cycle is exercised by this transport-only probe.",
    "6. Write a QUALIFICATION_RUN.json-shaped record of actual configuration and stop reason; no transcript or secrets.",
    "7. Shut down the owned engine. This is not a full orchestration or GUI qualification.",
)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--i-authorize-paid-call", action="store_true", default=False,
                         help="Required, together with --approval-ref, to run anything beyond argument validation.")
    parser.add_argument("--approval-ref", default=None, help="A non-empty human approval reference string.")
    parser.add_argument("--connection-id", default=None, help="An existing 'cn_...' connection id (engine claude-agent-acp).")
    parser.add_argument("--route-id", default=None, help="An existing 'xr_...' ExecutionRoute id for that connection.")
    parser.add_argument("--install-record", default=None, help="Path to a JSON ClaudeAcpInstall record.")
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
        print("cortex_claude_probe.py --plan (no authorization, nothing executed):")
        for step in _PLAN_STEPS:
            print(f"  {step}")
        return 0

    if not _authorized(args):
        print(
            "cortex_claude_probe.py: refusing to run without both --i-authorize-paid-call "
            "and a non-empty --approval-ref; exiting before any install verification, "
            "workspace creation, or network/process activity. Use --plan to see the "
            "steps this would take once authorized (including step 0's status of "
            "the instruction-delivery channel and the real remaining T035/T036 "
            "live-execution blocker).",
            file=sys.stderr,
        )
        return 2

    for required_name in ("connection_id", "route_id", "install_record", "output"):
        if not getattr(args, required_name):
            flag = "--" + required_name.replace("_", "-")
            print(f"cortex_claude_probe.py: {flag} is required once authorized.", file=sys.stderr)
            return 2

    return _run_authorized(args)


def _resolve_route(connection_id: str, route_id: str, environ: dict | None = None):
    """The stored :class:`ExecutionRoute` for ``route_id`` on ``connection_id``.

    Identical contract to ``cortex_codex_probe.py``'s own
    ``_resolve_route`` (see that module for the full rationale): read-only,
    never writes, never contacts an engine, never launches anything.
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
    ``tests/test_engine_claude_probe.py``, which only exercises the refusal
    and ``--plan`` paths) -- this task never runs a paid call itself.
    """
    import json
    import tempfile
    import time
    from pathlib import Path

    from cortex_runtime.connections import registry as connection_registry
    from cortex_runtime.engine.claude_adapter_acp import (
        build_claude_evidence_status,
        build_claude_launch,
        claude_option_mapping,
    )
    from cortex_runtime.engine.claude_install import ClaudeAcpInstall, verify_install
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from cortex_runtime.portable.engine_contract import EngineCapability, EngineEventKind
    from cortex_runtime.portable.identity import ConfigurationRevision

    install_data = json.loads(Path(args.install_record).read_text(encoding="utf-8"))
    install = ClaudeAcpInstall(**install_data)
    verify_install(install)

    registry_state = connection_registry.read_registry()
    record = registry_state.connections.get(args.connection_id)
    if record is None or record.engine != "claude-agent-acp":
        print(f"cortex_claude_probe.py: connection {args.connection_id!r} is not a known claude-agent-acp connection.", file=sys.stderr)
        return 1

    route = _resolve_route(args.connection_id, args.route_id)
    if route is None:
        print(
            f"cortex_claude_probe.py: route {args.route_id!r} is not a selectable, cached route for "
            f"connection {args.connection_id!r}; run discovery first.",
            file=sys.stderr,
        )
        return 1

    workspace_root = tempfile.mkdtemp(prefix="cortex-claude-probe-")
    launch_command, _pinned = build_claude_launch(record, install)
    manager = EngineSessionManager(
        record, workspace_root, option_mapping=claude_option_mapping,
    )
    result: dict = dict(status="not_run")
    try:
        manager.negotiate((EngineCapability.SESSION_CREATION, EngineCapability.CONFIGURATION_OPTIONS, EngineCapability.PROMPT_STREAMING))
        session_id = manager.create_session("t_" + "0" * 12, route.id)
        revision = ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)
        evidence_ref = manager.configure(session_id, revision)
        status = build_claude_evidence_status(revision.id, evidence_ref)
        stop_reason = None
        for event in manager.prompt_turn(session_id, "One bounded coordinator qualification prompt (see T026 probe)."):
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
            note="Single-turn transport probe only; full runtime orchestration and GUI behavior remain unverified.",
        )
    finally:
        manager.shutdown()

    Path(args.output).write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"cortex_claude_probe.py: wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
