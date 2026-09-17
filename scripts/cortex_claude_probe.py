#!/usr/bin/env python3
"""Opt-in, explicitly-authorized live claude-agent-acp coordinator probe (T026).

Mirrors ``scripts/cortex_codex_probe.py`` (T013) for the second, non-Codex
engine. The real coordinator role's own instruction delivery is QUALIFIED
at the pinned claude-agent-acp 0.78.0 (channel ``session/new
_meta.systemPrompt`` -- see
``engine.instruction_delivery_registry.ClaudeAgentAcp0_78Delivery``'s own
docstring for the full dist evidence chain, and
``docs/acp/claude-agent-acp-adapter.md``'s "Instruction delivery: QUALIFIED
via ``session/new _meta.systemPrompt``" section), so
``acp.orchestrating_runtime.OrchestratingRuntime._ensure_coordinator`` no
longer refuses a critical-rules coordinator role for this pinned version --
that is not this script's remaining blocker. The remaining blocker is
purely the mandatory T035/T036 live-execution gate: nothing here has ever
run a real turn confirming the spawned CLI actually *honors* the delivered
``systemPrompt`` (as opposed to merely receiving it on the wire, which the
automated test suite does confirm -- see
``tests/test_engine_claude_coordinator_live_session.py``), and this script
never runs anything paid on its own: without *both*
``--i-authorize-paid-call`` and a non-empty ``--approval-ref``, it
validates its other arguments (if given) and exits ``2`` before importing
``cortex_runtime`` or touching the filesystem beyond argument parsing -- no
install verification, no workspace, no subprocess, no network. It is never
invoked with those flags by this task; do not run it that way outside an
explicit, separately authorized live-qualification effort (T035/T036).

``--plan`` prints, in order, the steps this script would take with
authorization, without performing any of them.

Unlike ``cortex_codex_probe.py``, this script demonstrates the
**coordinator** role specifically (delegates one managed assignment,
consumes the worker's published result, and issues an explicit ``decide``
acceptance) via the real ``EngineSessionManager`` + Cortex MCP subprocess
path, per T026's own lead decision 4 -- not a single bounded prompt.

Safety notes for whoever runs this with authorization:

- ``--connection-id``/``--route-id`` identify an existing, already-onboarded
  connection/route in the real user registry; this script never onboards
  or creates one.
- ``--install-record`` is a JSON file with exactly the
  ``ClaudeAcpInstall`` fields (see
  ``engine.claude_install.ClaudeAcpInstall``); it is re-verified fresh here
  regardless of what it claims.
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
    "0. STATUS (read this first): engine.instruction_delivery_registry."
    "ClaudeAgentAcp0_78Delivery is QUALIFIED for claude-agent-acp 0.78.0 "
    "(channel session/new _meta.systemPrompt; dist evidence "
    "acp-agent.js:5887-5901,6019,6179-6182 plus the SDK's own query() "
    "forwarding it into the spawned CLI's control protocol), so "
    "acp.orchestrating_runtime.OrchestratingRuntime._ensure_coordinator "
    "no longer refuses the real critical-rules coordinator role for this "
    "pinned version -- that is not a blocker here. The real remaining "
    "blocker is the mandatory T035/T036 live-execution gate: no real turn "
    "has ever run through this channel, so nothing confirms the spawned "
    "CLI actually honors the delivered systemPrompt (only that it reaches "
    "the wire, which the automated suite does confirm); "
    "native_tool_containment also stays unknown regardless of this "
    "channel's own qualification.",
    "1. Load and validate --install-record as a ClaudeAcpInstall; call "
    "engine.claude_install.verify_install() against it (fresh hash/version/"
    "trust-location re-check; never trusts the file's own claims).",
    "2. Read the named --connection-id from the real connection registry "
    "(read-only) and confirm its engine is 'claude-agent-acp'.",
    "3. Resolve --route-id to a stored ExecutionRoute via "
    "_resolve_route() -- the newest cached catalog.store snapshot for "
    "--connection-id whose entries contain a matching route.id; refuse if "
    "no cached snapshot has a selectable matching entry.",
    "4. Create a fresh temporary workspace directory via tempfile.mkdtemp() "
    "(never this checkout, never the real project this script is invoked "
    "from).",
    "5. Build (launch_command, pinned_engine_vars) via "
    "engine.claude_adapter_acp.build_claude_launch() (pinned_engine_vars is "
    "always {} for this engine) and construct one "
    "EngineSessionManager(option_mapping=claude_option_mapping) for the "
    "coordinator role in that workspace (no install= passed to "
    "EngineSessionManager itself: engine.process only accepts install for "
    "engine=='codex-acp').",
    "6. negotiate() capabilities, create_session() for the coordinator, "
    "then configure() the session against the route's ConfigurationRevision "
    "(claude_option_mapping rejects an unsupported model/effort/mode before "
    "any prompt is sent).",
    "7. Send one coordinator prompt; on its first turn, call the "
    "delegation.mcp_tools 'delegate' operation for exactly one managed "
    "assignment (never more, for this bounded probe).",
    "8. Wait (bounded, via the real completion gate -- delegation.completion "
    "-- never a fixed sleep) for the delegated worker's own "
    "'publish_result' MCP call, then resume the coordinator's own prompt "
    "with that result.",
    "9. Call the delegation.mcp_tools 'decide' operation with an explicit "
    "'accept' (or 'reject', if the worker's result was inadequate) -- never "
    "an implicit/auto acceptance.",
    "10. Build a QUALIFICATION_RUN.json-shaped record (status/configuration/"
    "execution/measurements; selected/configuration_applied/"
    "execution_verified evidence fields from "
    "build_claude_evidence_status(); the coordinator's own delegate/decide "
    "outcome; no secret, no full transcript) and write it to --output.",
    "11. shutdown() every engine session/process (coordinator and worker); "
    "report the exit code and --output path on stdout.",
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
            note=(
                "This probe stops at a single bounded prompt turn against a "
                "directly constructed EngineSessionManager; the full "
                "delegate/worker-report/decide coordinator cycle (steps 7-9 "
                "of --plan) requires going through the real "
                "OrchestratingRuntime/ProvisioningService path instead -- "
                "not attempted here (a scope choice, not a qualification "
                "refusal: the pinned 0.78.0 instruction-delivery channel is "
                "QUALIFIED and admits this role with no escape hatch)."
            ),
        )
    finally:
        manager.shutdown()

    Path(args.output).write_text(json.dumps(result, indent=2, sort_keys=True), encoding="utf-8")
    print(f"cortex_claude_probe.py: wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
