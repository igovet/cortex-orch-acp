#!/usr/bin/env python3
"""Non-destructive, idempotent first-run setup for the Cortex ACP agent (T031).

Four explicit, separately invocable steps -- never a single opaque "do
everything" action that could silently combine consent for one step with
an unrelated one:

1. ``init-storage``   -- create (or verify) the owner-private
   ``$XDG_CONFIG_HOME/cortex`` / ``$XDG_STATE_HOME/cortex`` roots that
   ``cortex_runtime.connections.registry`` already uses; never touches any
   project's own ``.cortex`` store.
2. ``add-connection`` -- register one authorized connection via the
   existing ``connections.onboarding`` API. A credential value, when
   needed, is always read from a file path or stdin -- *never* accepted as
   a plain command-line argument (so it never appears in `ps`/shell
   history) -- and is written straight to the connection's private secret
   file; this script never logs, prints, or stores it anywhere else. A
   native-account connection instead prints (non-secret) instructions
   naming the isolated directory/env var for the user's own login flow,
   exactly like ``onboarding.mark_native_login_pending``.
3. ``pin-route``      -- build a catalog snapshot from one hand-declared
   qualified ``(engine, auth_mode, model)`` combination (never a live
   engine probe -- see ``catalog.discovery.ManualDeclaration``; only a
   *documented/qualified* combination is accepted at all) and persist it
   as the plain T008 user-default route/parameters
   (``configuration.store.write_user_defaults``) -- the exact mechanism
   ``acp.agent.Agent._apply_pre_task_selection`` already uses for a normal
   in-session ``/cortex-model`` choice. A fresh task's very first prompt
   then adopts this default automatically (see
   ``acp.agent.Agent._propose_pending_selection_for_new_task``) with no
   further per-task action.
4. ``print-client-recipes`` -- delegates entirely to the existing T011
   ``cortex_acp_client_config.py`` (Zed dry-run-by-default entry +
   ``--print-codeg-recipe``); this script never re-implements or
   duplicates that write path.

``status`` (no step name, the default) only reads and reports current
state -- it is always safe to run with no flags.

Every step is idempotent: rerunning ``init-storage``/`add-connection
--reuse`/``pin-route`` with the same inputs changes nothing and reports
the existing state rather than erroring or duplicating.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(SCRIPTS.parent / "plugins" / "cortex" / "scripts"))

from cortex_runtime.catalog import discovery, store as catalog_store  # noqa: E402
from cortex_runtime.configuration import store as configuration_store  # noqa: E402
from cortex_runtime.connections import onboarding, registry  # noqa: E402
from cortex_runtime.connections.model import AuthMode  # noqa: E402
from cortex_runtime.portable.errors import PortableError  # noqa: E402

import cortex_acp_client_config  # noqa: E402


def init_storage(environ=None):
    """Create/verify the owner-private config/state roots; idempotent."""
    config_root = registry.config_root(environ)
    state_root = registry.state_root(environ)
    registry.ensure_private_dir(config_root, root=config_root, environ=environ)
    registry.ensure_private_dir(state_root, root=state_root, environ=environ)
    return dict(config_root=str(config_root), state_root=str(state_root))


def add_connection(*, name, engine, provider, auth_mode, key_kind=None, api_key_reader=None,
                    launch_command=None, reuse=True, environ=None):
    """Register (or reuse) one connection; return a JSON-safe summary.

    ``api_key_reader`` (a zero-argument callable returning ``bytes``) is
    required iff ``auth_mode == AuthMode.API_KEY`` -- the same
    reader-callback convention ``onboarding.set_api_key`` already
    enforces, so a secret is never accepted as a plain string here either.

    ``launch_command`` (an optional argv tuple) is only ever a caller-
    supplied, already-pinned/verified path -- this script never derives
    one from a live probe. A production caller would only ever pass one
    verified via ``engine.codex_install.verify_install``/
    ``engine.claude_install.verify_install`` against an operator-recorded
    install (see ``pinned-dependencies.json``'s own ``install_record_note``
    fields); wiring that verification step into this CLI is left for a
    later change, not silently skipped -- see ``docs/acp/packaging-and-onboarding.md``.
    """
    record = onboarding.create_connection(
        name=name, engine=engine, provider=provider, auth_mode=auth_mode,
        key_kind=key_kind, launch_command=launch_command, reuse=reuse, environ=environ,
    )
    login_instructions = None
    if auth_mode == AuthMode.API_KEY:
        if api_key_reader is None:
            raise SystemExit("error: --api-key-file or --api-key-stdin is required for auth-mode=api_key.")
        record = onboarding.set_api_key(record.id, reader=api_key_reader, environ=environ)
    elif auth_mode == AuthMode.NATIVE_ACCOUNT:
        login_instructions = onboarding.mark_native_login_pending(record.id, environ=environ)
    return dict(
        connection_id=record.id, name=record.name, engine=record.engine, provider=record.provider,
        auth_mode=record.auth_mode.value, state=record.state.value, qualified=record.qualified,
        login_instructions=login_instructions,
    )


def pin_route(*, connection_id, model, environ=None):
    """Declare one qualified model for ``connection_id`` and pin it as the
    persisted user-default route (see module docstring, step 3)."""
    state = registry.read_registry(environ)
    record = state.connections.get(connection_id)
    if record is None:
        raise SystemExit(f"error: unknown connection id {connection_id!r}; run add-connection first.")
    try:
        snapshot = discovery.build_catalog(
            state.connections, observations={},
            manual_declarations=(discovery.ManualDeclaration(connection_id=connection_id, model=model),),
        )
    except PortableError as error:
        raise SystemExit(f"error: {error.code}: {error.remediation}") from None
    catalog_store.save_snapshot(snapshot, environ)
    entry = next((e for e in snapshot.entries if e.connection_id == connection_id and e.route.model == model), None)
    if entry is None:
        raise SystemExit("error: the declared model was not accepted (connection auth_mode is not a "
                          "documented/qualified combination -- see connections.model.SUPPORT_MATRIX).")
    current = configuration_store.read_user_defaults(environ)
    defaults = dict(current["defaults"])
    defaults["route_id"] = entry.route.id
    defaults["parameters"] = {}
    configuration_store.write_user_defaults(defaults, current["ceiling"], environ)
    return dict(route_id=entry.route.id, connection_id=connection_id, model=model,
                catalog_snapshot_id=snapshot.id, label=entry.disambiguated_label)


def status(environ=None):
    """Read-only report of current setup state; always safe to run."""
    result = dict(storage=None, connections=[], pinned_route=None)
    try:
        result["storage"] = dict(
            config_root=str(registry.config_root(environ)), state_root=str(registry.state_root(environ)),
        )
    except PortableError as error:
        result["storage_error"] = error.code
    try:
        state = registry.read_registry(environ)
        result["connections"] = [
            dict(id=cid, name=r.name, engine=r.engine, state=r.state.value, qualified=r.qualified)
            for cid, r in sorted(state.connections.items())
        ]
    except PortableError as error:
        result["connections_error"] = error.code
    try:
        defaults = configuration_store.read_user_defaults(environ)["defaults"]
        route_id = defaults.get("route_id") if isinstance(defaults, dict) else None
        result["pinned_route"] = route_id
    except PortableError as error:
        result["pinned_route_error"] = error.code
    return result


def _read_secret(args):
    if args.api_key_file:
        return lambda: Path(args.api_key_file).read_bytes()
    if args.api_key_stdin:
        return lambda: sys.stdin.buffer.read()
    return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="step")

    sub.add_parser("status", help="Report current setup state (read-only, default).")
    sub.add_parser("init-storage", help="Create/verify owner-private config/state roots.")

    p_conn = sub.add_parser("add-connection", help="Register (or reuse) one authorized connection.")
    p_conn.add_argument("--name", required=True)
    p_conn.add_argument("--engine", required=True, choices=("codex-acp", "claude-agent-acp"))
    p_conn.add_argument("--provider", required=True)
    p_conn.add_argument("--auth-mode", required=True, choices=("native_account", "api_key"))
    p_conn.add_argument("--key-kind", default=None)
    p_conn.add_argument("--api-key-file", type=str, default=None,
                         help="Path to a file containing the API key bytes (never inline on argv).")
    p_conn.add_argument("--api-key-stdin", action="store_true",
                         help="Read the API key bytes from stdin instead of a file.")
    p_conn.add_argument("--no-reuse", dest="reuse", action="store_false", default=True)

    p_route = sub.add_parser("pin-route", help="Pin one declared model as the default route.")
    p_route.add_argument("--connection", required=True, dest="connection_id")
    p_route.add_argument("--model", required=True)

    p_recipe = sub.add_parser("print-client-recipes", help="Print Zed/codeg registration recipes (delegates to cortex_acp_client_config.py).")
    p_recipe.add_argument("--settings", type=Path, default=None)
    p_recipe.add_argument("--apply", action="store_true")
    p_recipe.add_argument("--i-consent-to-write", dest="consent", action="store_true")
    p_recipe.add_argument("--replace", action="store_true")

    args = parser.parse_args(argv)
    step = args.step or "status"

    if step == "status":
        print(_json(status()))
        return 0
    if step == "init-storage":
        print(_json(init_storage()))
        return 0
    if step == "add-connection":
        reader = _read_secret(args)
        result = add_connection(
            name=args.name, engine=args.engine, provider=args.provider,
            auth_mode=AuthMode(args.auth_mode), key_kind=args.key_kind, api_key_reader=reader,
            reuse=args.reuse,
        )
        print(_json(result))
        return 0
    if step == "pin-route":
        result = pin_route(connection_id=args.connection_id, model=args.model)
        print(_json(result))
        return 0
    if step == "print-client-recipes":
        recipe_argv = []
        if args.settings is not None:
            recipe_argv += ["--settings", str(args.settings)]
        if args.apply:
            recipe_argv.append("--apply")
        if args.consent:
            recipe_argv.append("--i-consent-to-write")
        if args.replace:
            recipe_argv.append("--replace")
        exit_code = cortex_acp_client_config.main(recipe_argv)
        print()
        cortex_acp_client_config.cmd_print_codeg_recipe()
        return exit_code

    parser.print_help()
    return 2


def _json(data):
    import json as _json_module

    return _json_module.dumps(data, indent=2, sort_keys=True)


if __name__ == "__main__":
    raise SystemExit(main())
