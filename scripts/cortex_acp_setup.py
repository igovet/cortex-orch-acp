#!/usr/bin/env python3
"""Non-destructive, idempotent first-run setup for the Cortex ACP agent (T031).

Five explicit, separately invocable steps -- never a single opaque "do
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
4. ``record-observation`` -- replace a ``pin-route``-declared route with
   one built from a *real* engine-observed ``configOptions`` payload (see
   ``catalog.discovery.parse_observation``/``build_catalog``): the same
   ``session/new``-shaped JSON array a live engine actually returned,
   captured elsewhere (e.g. a client's own devtools, or an explicitly
   authorized qualification run) and saved to a file -- this step never
   itself contacts an engine. Unlike ``pin-route``'s
   ``ManualDeclaration`` (which is only ever accepted with **no**
   parameters, since nothing has confirmed what it actually supports),
   this records the connection's real, evidence-qualified
   ``ExecutionRoute.supported_parameters`` (e.g. codex-acp's
   ``reasoning_effort``/``thought_level`` choices) with DISCOVERY
   provenance, and re-pins it as the user default exactly like
   ``pin-route`` does -- this is what lets a value like "high effort"
   actually appear as a selectable option in a client's message panel.
5. ``refresh-catalog`` -- a prompt-free, on-demand catalog refresh for one
   connection (see ``cortex_runtime.catalog.refresh``): opens a real,
   short-lived engine session purely to read its advertised
   ``configOptions`` (``initialize`` + ``session/new``, never a
   ``session/prompt``), persists it with real ``DISCOVERY`` provenance, and
   closes the session. A real Cortex session already does this for free on
   every engine open, so this step is only needed to seed/refresh a
   connection's catalog without running a real task first.
   Never touches the pinned user-default route -- unlike ``pin-route``/
   ``record-observation``, this only refreshes the cached catalog snapshot
   a later selection reads from.
6. ``init-route-policy`` -- write a default, hand-editable
   ``~/.config/cortex/route-policy.json`` (D-T035-03, see
   ``cortex_runtime.configuration.route_policy`` for the schema/precedence)
   pre-populated permitting every role for both supported engines at a
   sensible effort range; never overwrites an existing file, here or
   anywhere else the read-precedence chain would already find one.
7. ``print-client-recipes`` -- delegates entirely to the existing T011
   ``cortex_acp_client_config.py`` (Zed dry-run-by-default entry +
   ``--print-codeg-recipe``); this script never re-implements or
   duplicates that write path.

``status`` (no step name, the default) only reads and reports current
state -- it is always safe to run with no flags.

Every step is idempotent: rerunning ``init-storage``/`add-connection
--reuse`/``pin-route``/``record-observation`` with the same inputs changes
nothing and reports the existing state rather than erroring or duplicating.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(SCRIPTS.parent / "src"))

from cortex_runtime.catalog import discovery, refresh as catalog_refresh, store as catalog_store  # noqa: E402
from cortex_runtime.configuration import route_policy as route_policy_module  # noqa: E402
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
        # T035 FINDING-07: only this one connection, never the full
        # registry -- passing every connection here (as an earlier revision
        # did) makes build_catalog mark every *other* connection
        # "discovery_unavailable" (it never observed them at all this
        # call) and save_snapshot then writes that entry-less placeholder
        # into each of their own catalog directories, newer than their real
        # cached snapshot, hiding them from selectable_entries_from_snapshots
        # (see acp.config_options).
        snapshot = discovery.build_catalog(
            {connection_id: record}, observations={},
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
    # T035 FINDING-05: the stable identity alongside the fingerprint id --
    # see cortex_runtime.configuration.resolution.resolve_saved_route.
    defaults["route_connection_id"] = connection_id
    defaults["route_model"] = model
    defaults["parameters"] = {}
    configuration_store.write_user_defaults(defaults, current["ceiling"], environ)
    return dict(route_id=entry.route.id, connection_id=connection_id, model=model,
                catalog_snapshot_id=snapshot.id, label=entry.disambiguated_label)


def record_observation(*, connection_id, model, config_options_file, environ=None):
    """Persist a real, engine-observed route for ``connection_id``/``model``
    from a captured ``session/new``-shaped ``configOptions`` JSON array (see
    module docstring, step 4), and re-pin it as the user-default route --
    the same mechanism ``pin_route`` uses.

    Unlike ``pin_route``'s ``ManualDeclaration`` (always persisted with
    ``supported_parameters=()`` since nothing confirmed what it actually
    supports), this goes through ``catalog.discovery.parse_observation`` +
    ``build_catalog`` -- the same DISCOVERY-provenance path automatic
    discovery would use -- so the saved route's
    ``ExecutionRoute.supported_parameters`` are the engine's real advertised
    values (e.g. a ``thought_level``/``reasoning_effort`` choice), never a
    fabricated or declared one.
    """
    state = registry.read_registry(environ)
    record = state.connections.get(connection_id)
    if record is None:
        raise SystemExit(f"error: unknown connection id {connection_id!r}; run add-connection first.")
    try:
        raw = json.loads(Path(config_options_file).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise SystemExit(f"error: could not read --config-options-file: {error}") from None
    try:
        observation = discovery.parse_observation(raw)
    except PortableError as error:
        raise SystemExit(f"error: {error.code}: {error.remediation}") from None
    if model not in {value for value, _name in observation.model_options()}:
        raise SystemExit(
            f"error: {model!r} is not among the observed 'model' option's values; "
            "the engine never actually advertised it."
        )
    try:
        snapshot = discovery.build_catalog({connection_id: record}, observations={connection_id: observation})
    except PortableError as error:
        raise SystemExit(f"error: {error.code}: {error.remediation}") from None
    catalog_store.save_snapshot(snapshot, environ)
    entry = next((e for e in snapshot.entries if e.connection_id == connection_id and e.route.model == model), None)
    if entry is None:
        raise SystemExit("error: the observed model was not accepted (see catalog.discovery.build_catalog).")
    current = configuration_store.read_user_defaults(environ)
    defaults = dict(current["defaults"])
    defaults["route_id"] = entry.route.id
    defaults["route_connection_id"] = connection_id
    defaults["route_model"] = model
    defaults["parameters"] = {}
    configuration_store.write_user_defaults(defaults, current["ceiling"], environ)
    return dict(
        route_id=entry.route.id, connection_id=connection_id, model=model,
        catalog_snapshot_id=snapshot.id, label=entry.disambiguated_label,
        supported_parameters=[list(pair) for pair in entry.route.supported_parameters],
    )


def refresh_catalog(*, connection_id, workspace_root=None, timeout=None, environ=None):
    """Step 5: a prompt-free, on-demand catalog refresh for
    ``connection_id`` -- see module docstring and
    ``cortex_runtime.catalog.refresh``. A real Cortex session already
    refreshes the catalog for free on every engine open (see
    ``catalog.refresh``'s own module docstring); this step is only for an
    explicit manual refresh (e.g. a connection with no catalog yet)."""
    state = registry.read_registry(environ)
    record = state.connections.get(connection_id)
    if record is None:
        raise SystemExit(f"error: unknown connection id {connection_id!r}; run add-connection first.")
    kwargs = dict(environ=environ)
    if timeout is not None:
        kwargs["timeout"] = timeout
    outcome = catalog_refresh.perform_refresh(record, workspace_root or os.getcwd(), **kwargs)
    return dict(
        connection_id=outcome.connection_id, attempted=outcome.attempted, success=outcome.success,
        reason=outcome.reason, catalog_snapshot_id=outcome.snapshot_id, at=outcome.at,
        effort_enrichment=outcome.effort_enrichment,
    )


def init_route_policy(environ=None):
    """D-T035-03: write a default, hand-editable ``route-policy.json`` at
    ``~/.config/cortex/route-policy.json`` (see
    ``cortex_runtime.configuration.route_policy`` for the exact schema and
    read precedence) -- pre-populated permitting every role for both
    engines this project supports, at a sensible effort range, so the file
    always exists in a working (unrestrictive) state the user can then
    narrow by hand. Never overwrites an existing file, at this path or any
    other location ``route_policy.load_route_policy`` would find first;
    idempotent -- rerunning after a file already exists changes nothing and
    reports ``created=False``.
    """
    existing_policy, malformed = route_policy_module.load_route_policy(project_root=None, environ=environ)
    # The same XDG rules every other Cortex config obeys (see
    # connections.registry): a user who moved their configuration with
    # XDG_CONFIG_HOME would otherwise get this one file written somewhere
    # their own setup does not look.
    env = os.environ if environ is None else environ
    config_home = env.get("XDG_CONFIG_HOME") or ""
    base = Path(config_home) if config_home.startswith("/") else Path(env.get("HOME") or Path.home()) / ".config"
    path = base / "cortex" / "route-policy.json"
    if path.is_file():
        return dict(path=str(path), created=False, reason="already_exists")
    if malformed is not None or existing_policy is not None:
        # A different location in the read-precedence chain already has a
        # policy (or a malformed one) in effect; writing a second file here
        # would never actually be read (see load_route_policy's own
        # first-match-wins docstring) and could confuse the user into
        # thinking it applies. Report where the existing one actually is
        # instead of writing a dead file.
        return dict(path=str(path), created=False, reason="policy_already_in_effect_elsewhere", detail=malformed)
    default_policy = {
        "_comment": [
            "Cortex route policy: which engine, model and effort each kind of worker may be dispatched with.",
            "Shipped as a recommendation, not a placeholder -- edit it, it is yours.",
            "'roles' maps a role id (or its short alias, e.g. 'backend-dev') to the routes that role may use.",
            "'default' applies to any role not named in 'roles'. \"*\" means any model / any effort.",
            "'default_effort' is used only when nothing else already selected an effort for that route.",
            "Effort names are the engine's own and are discovered from it, not fixed here: a value a model",
            "does not support is refused by that engine. Observed values today: low, medium, high, xhigh, max.",
            "This file can only NARROW what the permission ceiling already allows. It can never widen it,",
            "so it is a way to spend less, never a way around a limit.",
            "Roles are grouped by what the work costs to get wrong: design/security/performance/debugging get",
            "the strongest models, review sits just below, ordinary implementation uses mid models, and",
            "exploration/planning/generic work uses the cheapest. Change any line you disagree with."
        ],
        "roles": {
            "worker-accessibility-auditor": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-accessibility-fixer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-architect": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-6-astra",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "opus",
                    "efforts": [
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-backend-dev": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-build-verification": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-code-reviewer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-data-engineer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-database-architect": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-debugger": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-6-astra",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "opus",
                    "efforts": [
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-devops-engineer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-explorer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "low",
                        "medium",
                        "high"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.5",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                }
            ],
            "worker-frontend-dev": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-fullstack-dev": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-general": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "low",
                        "medium",
                        "high"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.5",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                }
            ],
            "worker-mobile-dev": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-performance-engineer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-6-astra",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "opus",
                    "efforts": [
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-planner": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "low",
                        "medium",
                        "high"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.5",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                }
            ],
            "worker-qa-engineer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-refactorer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ],
            "worker-security-auditor": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-6-astra",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "opus",
                    "efforts": [
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-senior-consultant": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-6-astra",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-sol",
                    "efforts": [
                        "high",
                        "xhigh",
                        "max"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "opus",
                    "efforts": [
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                }
            ],
            "worker-technical-writer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "low",
                        "medium",
                        "high"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.5",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "low",
                        "medium"
                    ],
                    "default_effort": "low"
                }
            ],
            "worker-ux-designer": [
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-luna",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "high"
                },
                {
                    "engine": "codex-acp",
                    "model": "gpt-5.6-terra",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                },
                {
                    "engine": "claude-agent-acp",
                    "model": "sonnet",
                    "efforts": [
                        "medium",
                        "high",
                        "xhigh"
                    ],
                    "default_effort": "medium"
                }
            ]
        },
        "default": [
            {
                "engine": "codex-acp",
                "model": "*",
                "efforts": [
                    "low",
                    "medium",
                    "high",
                    "xhigh",
                    "max"
                ],
                "default_effort": "medium"
            },
            {
                "engine": "claude-agent-acp",
                "model": "*",
                "efforts": [
                    "low",
                    "medium",
                    "high",
                    "xhigh"
                ],
                "default_effort": "medium"
            }
        ]
    }
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    tmp_path = path.with_suffix(".json.tmp")
    tmp_path.write_text(json.dumps(default_policy, indent=2, sort_keys=False) + "\n", encoding="utf-8")
    os.chmod(tmp_path, 0o600)
    os.replace(tmp_path, path)
    return dict(path=str(path), created=True)


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
    sub.add_parser("init-route-policy", help="Write a default, hand-editable ~/.config/cortex/route-policy.json (never overwrites an existing one).")

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

    p_obs = sub.add_parser(
        "record-observation",
        help="Replace a pin-route'd route with one built from a real captured engine configOptions payload.",
    )
    p_obs.add_argument("--connection", required=True, dest="connection_id")
    p_obs.add_argument("--model", required=True)
    p_obs.add_argument("--config-options-file", required=True, type=Path,
                        help="Path to a JSON file holding the engine's real session/new 'configOptions' array.")

    p_refresh = sub.add_parser(
        "refresh-catalog",
        help="Prompt-free, on-demand catalog refresh for one connection.",
    )
    p_refresh.add_argument("--connection", required=True, dest="connection_id")
    p_refresh.add_argument("--workspace", type=str, default=None,
                            help="Working directory for the refresh session (defaults to the current directory).")
    p_refresh.add_argument("--timeout", type=float, default=None, help="Engine launch/call timeout (default 30s).")

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
    if step == "init-route-policy":
        print(_json(init_route_policy()))
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
    if step == "record-observation":
        result = record_observation(
            connection_id=args.connection_id, model=args.model,
            config_options_file=args.config_options_file,
        )
        print(_json(result))
        return 0
    if step == "refresh-catalog":
        result = refresh_catalog(
            connection_id=args.connection_id, workspace_root=args.workspace, timeout=args.timeout,
        )
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
