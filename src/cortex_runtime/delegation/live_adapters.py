"""Production seams for ``acp.orchestrating_runtime.OrchestratingRuntime``.

Only the two seams that construct cleanly from existing registries/state
with no new user configuration are provided here:

- :class:`SharedProjectRootWorkspaceProvisioner` -- T016's own explicit,
  documented choice (see the T016 task's lead design notes): every
  delegated attempt shares the task's one project root instead of an
  isolated git worktree. This is intentionally **not** isolated; T025 owns
  real per-attempt worktree isolation. ``create``/``lookup``/``remove`` are
  therefore pure bookkeeping over the attempt id, never a filesystem
  mutation.
- :class:`ConfigurationServiceSnapshotSource` -- a thin read over the
  existing :class:`configuration.service.ConfigurationService` (T008),
  itself already constructed from a project ``Store`` with no new
  configuration surface.

:class:`EngineSessionManagerAdapter` is the live
:class:`~acp.orchestrating_runtime.SessionRuntime`: one
``engine.session_manager.EngineSessionManager`` per attempt (coordinator or
worker -- consistent with T012's "no two engine instances share a process"
design), constructed with that attempt's own trusted-channel
``channel_credentials`` so ``create_session`` sends the real
``mcpServers`` entry (see ``engine.mcp_bridge.build_mcp_servers``) that
launches ``cortex.py`` as the engine's own MCP child, authenticated for
exactly that channel. Route selection itself (which ``ConnectionRecord`` to
launch) is intentionally **not** this adapter's decision: it is injected as
a ``connection_record_factory(task_id) -> ConnectionRecord`` callable,
resolved by the caller (``cortex_acp.py``) from the task's current T008
configuration/catalog state.

Only ``tests/test_orchestrating_runtime_live_session.py`` exercises this
adapter, and only against the test-only, standalone
``tests/fake_engine_process.py`` subprocess (never a real engine) launched
through the real ``EngineSessionManager`` -- proving the prompt/continuation
path goes through the real session-manager machinery, not a bespoke
shortcut. No test in this repository launches a real engine.
"""
from __future__ import annotations

import hashlib
import json
import logging
import threading
import time
import os
import stat
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Mapping

# No real installed-version discovery is wired into ``ConnectionRecord``
# yet (T026 note, matching the pre-existing T016 codex-acp-only comment
# this dict replaces): each pinned, evidence-qualified engine resolves to
# its own exact pinned version here (matching
# ``engine.instruction_delivery_registry``'s/``delegation.generations``'s
# own exact registrations for the same engine names), so
# ``delivery_for``/``tier_for`` are looked up precisely rather than always
# falling through to a wildcard entry. Any other/unknown engine name still
# resolves to ``"*"`` -- never a guessed/interpolated version.
_PINNED_ENGINE_VERSION: dict[str, str] = {
    "codex-acp": "2.1.1",
    "claude-agent-acp": "0.88.0",
    # Placeholder only when a connection is not available to probe; all 2.x
    # versions are accepted, and actual sessions use their binary's version.
    "opencode": "2.0.0",
}


def unified_base_prompt_enabled(environ: Mapping[str, str] | None = None) -> bool:
    """Whether sessions replace engine base prompts with Cortex\'s unified prompt.

    This process-scoped rollout switch defaults to the current unified mode.
    Set CORTEX_UNIFIED_BASE_PROMPT=0 before launching Cortex to retain each
    engine\'s native base prompt and skill catalog for newly created sessions.
    """
    source = os.environ if environ is None else environ
    return source.get("CORTEX_UNIFIED_BASE_PROMPT") != "0"


def _installed_version_for(record) -> str | None:
    """Version at ``record``'s launch command: a sandboxed OpenCode
    ``--version`` probe or the npm adapter's own ``package.json``.
    ``None`` when no supported install-shaped command can be read."""
    command = getattr(record, "launch_command", None) or ()
    if len(command) != 2 or not isinstance(command[1], str):
        return None
    if record.engine == "opencode" and command[1] == "acp":
        from ..engine.opencode_install import probe_version

        return probe_version(command[0])
    from ..connections.discovery import _declared_bin

    declared = _declared_bin(Path(command[1]).parent.parent / "package.json")
    return declared[0] if declared else None


def _pinned_version_for(engine: str, record=None) -> str:
    """The version ``delivery_for``/``tier_for`` are looked up by.

    Prefers the version this connection will really launch when that exact
    version has its own registration, so a receipt names the registration
    that actually runs -- Cortex can have more than one qualified codex-acp
    version installed (see ``connections.discovery
    .QUALIFIED_ADAPTER_VERSIONS``). An installed version with no exact
    registration keeps resolving to the engine's pinned version, except for
    Claude: its actual installed version must be named even when unsupported,
    never borrowing another release's instruction-delivery qualification.
    """
    if record is not None:
        installed = _installed_version_for(record)
        if installed:
            from ..engine.instruction_delivery_registry import has_exact_registration

            if engine == "claude-agent-acp" or has_exact_registration(engine, installed):
                return installed
    return _PINNED_ENGINE_VERSION.get(engine, "*")


def _option_mapping_for(engine: str):
    """The adapter-specific ``option_mapping`` callable for ``engine``, or
    ``None`` for any engine with no pinned adapter mapping (falls back to
    ``engine.session_manager._default_option_mapping``, unchanged).

    T026 fix: :meth:`EngineSessionManagerAdapter.open` previously always
    used the generic default mapping (route model + supported_parameters
    round-tripped verbatim, with no adapter-specific rejection) for
    *every* engine, including codex-acp -- silently ignoring T013's own
    ``codex_option_mapping`` in the one production code path that actually
    launches a real engine. This is the single dispatch point fixing that
    for both pinned engines; a caller-supplied ``option_mapping=`` in
    ``EngineSessionManagerAdapter``'s own constructor kwargs (used by
    tests exercising a scenario against the generic default on purpose)
    always takes precedence and is never overridden here.
    """
    if engine == "codex-acp":
        from ..engine.codex_adapter_acp import codex_option_mapping

        return codex_option_mapping
    if engine == "claude-agent-acp":
        from ..engine.claude_adapter_acp import claude_option_mapping

        return claude_option_mapping
    if engine == "opencode":
        from ..engine.opencode_adapter_acp import opencode_option_mapping

        return opencode_option_mapping
    return None


# The trusted_channel.resolve_channel StoreError codes that mean this
# channel's authority is genuinely gone (see that function). Any other
# StoreError from resolve_channel is transient store/infra, never revocation.
_CHANNEL_AUTHORITY_LOST_CODES = frozenset({"channel_unknown", "channel_revoked", "channel_expired"})

# Result acceptance revokes the attempt's worker channel in the same
# transaction (orchestration.state), while the turn that called result_submit
# is still running: its final PromptResponse -- the only per-turn token report
# either installed adapter sends -- has not arrived yet. The 1 s authority
# watchdog used to SIGTERM the engine right then, so every live worker and
# reviewer call ended in transport_eof with unknown usage. For a revocation
# caused by the attempt's own published result the watchdog therefore waits at
# most this long. It exceeds EngineSessionManagerAdapter._RESULT_SETTLE_SECONDS
# by more than one watchdog poll plus controller latency, so the owned
# post-result settlement normally performs the teardown; this is only the
# backstop when no settlement runs. Explicit cancellation, stop and replanning
# still terminate at once through terminate_owned.
_RESULT_REVOCATION_GRACE_SECONDS = 20.0


def _result_published(store, task_id, attempt_id) -> bool:
    """Whether this attempt published a result that is still current."""
    with store.connection() as db:
        return db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='worker_result' AND superseded=0 LIMIT 1",
            (task_id, attempt_id),
        ).fetchone() is not None


def _jail_for(store, channel_credentials) -> dict:
    """The engine jail for one channel: exactly the scopes its gateway grants.

    The coordinator, reviewers and read-only roles get none, so their whole
    project is read-only; so does a worker without a running assignment.
    The process lives while the channel does, except that a worker channel
    revoked by its own accepted result lets the in-flight turn settle for at
    most ``_RESULT_REVOCATION_GRACE_SECONDS``.
    """
    from ..contracts import StoreError
    from ..orchestration.gateway import _authority
    from ..trusted_channel import resolve_channel

    channel_id, secret, _ = channel_credentials
    channel = resolve_channel(store, channel_id, secret)
    scopes = []
    readonly = []
    if channel.role != "coordinator":
        with store.connection() as db:
            try:
                scopes, _ = _authority(db, channel)
                from ..orchestration.gateway import write_denials
                readonly = write_denials(db, channel.task_id)
            except StoreError:
                scopes = []

    settling_since: list[float] = []

    def result_settling() -> bool:
        # Bounded from the first revoked observation, and only for this
        # worker attempt's own current result. Anything unproven fails closed.
        if channel.role != "worker" or not channel.attempt_id:
            return False
        if not settling_since:
            try:
                if not _result_published(store, channel.task_id, channel.attempt_id):
                    return False
            except Exception:  # noqa: BLE001 - an unprovable cause never extends authority
                return False
            settling_since.append(time.monotonic())
        return time.monotonic() - settling_since[0] < _RESULT_REVOCATION_GRACE_SECONDS

    def alive():
        try:
            resolve_channel(store, channel_id, secret)
            return True
        except StoreError as error:
            # Only a definitive channel invalidation means authority is lost.
            # A transient store error (``storage_busy`` under routine lock
            # contention, or a WAL/-shm sidecar mid-recreation) must never be
            # read as revocation: this runs on a 1s watchdog poll for every
            # engine, and racing it against any writer would otherwise kill a
            # perfectly live engine -- surfacing later as a spurious
            # ``transport_eof``. Stay alive and re-check on the next poll.
            code = str(error)
            if code not in _CHANNEL_AUTHORITY_LOST_CODES:
                return True
            return code == "channel_revoked" and result_settling()

    return {"scopes": list(scopes), "readonly": list(readonly), "alive": alive}


PROJECT_RULE_FILES = ("AGENTS.md", "CLAUDE.md")
_PROJECT_RULES_MAX_CHARS = 64_000


def _claude_trusts(project_root: str) -> bool:
    """Whether Claude Code's own trust record covers ``project_root``: an
    untrusted folder gets no project memory (AGENTS.md/CLAUDE.md) at all."""
    try:
        with open(os.path.join(os.path.expanduser("~"), ".claude.json"), encoding="utf-8") as stream:
            projects = json.load(stream).get("projects") or {}
    except (OSError, ValueError, AttributeError):
        return False
    root = os.path.realpath(project_root)
    while True:
        entry = projects.get(root)
        if isinstance(entry, dict) and entry.get("hasTrustDialogAccepted"):
            return True
        parent = os.path.dirname(root)
        if parent == root:
            return False
        root = parent


def project_rules_for_claude(project_root: str | None) -> str:
    """The project's own agent rules to hand a Claude session that would not
    load them itself (an untrusted folder: no project memory at all). Empty
    when Claude loads them natively."""
    if not project_root or _claude_trusts(project_root):
        return ""
    parts = []
    for name in PROJECT_RULE_FILES:
        path = os.path.join(project_root, name)
        try:
            if os.path.isfile(path) and not os.path.islink(path):
                with open(path, encoding="utf-8", errors="replace") as stream:
                    parts.append(f"### {name}\n\n{stream.read(_PROJECT_RULES_MAX_CHARS)}")
        except OSError:
            continue
    if not parts:
        return ""
    return "## Project rules (follow them)\n\n" + "\n\n".join(parts)


def _sdk_effort_for(record, launch) -> str | None:
    """The effort to hand claude-agent-acp at session/new, for a model whose
    SDK-unknown id gets no effort config option (see
    ``engine.claude_adapter_acp.SDK_EFFORTS``). ``None`` for every other
    engine/model, which configure their effort the ordinary way."""
    if record.engine != "claude-agent-acp":
        return None
    from ..engine.claude_adapter_acp import SDK_EFFORTS

    parameters = launch.get("parameters") or {}
    effort = parameters.get("effort") or parameters.get("thought_level")
    return effort if effort in SDK_EFFORTS else None


def _offered_values(observed_config_options, option_id: str) -> set:
    """The values a session's ``option_id`` select currently offers."""
    return {
        choice.get("value")
        for entry in observed_config_options or () if isinstance(entry, Mapping) and entry.get("id") == option_id
        for choice in (entry.get("options") or []) if isinstance(choice, Mapping)
    }


def _effort_within(effort: str | None, offered) -> str | None:
    """``effort`` if the session offers it, else the strongest offered
    effort below it, else the weakest above it -- lowered before raised,
    and ``None`` (leave the session's own default) when nothing offered is
    comparable."""
    from ..routing.auto_route import EFFORT_ORDER

    if not effort:
        return None
    if effort in offered:
        return effort
    if effort not in EFFORT_ORDER:
        return None
    ranked = [value for value in EFFORT_ORDER if value in offered]
    target = EFFORT_ORDER.index(effort)
    below = [value for value in ranked if EFFORT_ORDER.index(value) < target]
    return below[-1] if below else (ranked[0] if ranked else None)


def _session_new_meta_for(
    engine: str, version: str, instructions: str | None, *, role: str = "worker", project_root: str | None = None,
    effort: str | None = None, environ: dict[str, str] | None = None, role_id: str | None = None,
    stage: str | None = None,
):
    """The ``session/new`` ``_meta`` payload this engine's own qualified
    delivery mechanism builds for ``instructions``, or ``None``.

    Only ever non-``None`` for an engine+version whose
    ``engine.instruction_delivery_registry`` mechanism is QUALIFIED via a
    session/new-level ``_meta`` channel: Claude's native systemPrompt or
    Cortex's exact-source-verified private codex-acp extension.  For both
    pinned adapters the role text is absent from ordinary user prompt blocks;
    recovery adds only the task capsule through the conversation channel.
    """
    if role not in {"coordinator", "worker"}:
        raise ValueError("role must be 'coordinator' or 'worker'")
    from ..engine.instruction_delivery_registry import has_exact_registration

    if not instructions:
        if engine == "claude-agent-acp" and has_exact_registration(engine, version):
            from ..engine.claude_adapter_acp import claude_native_subagent_restriction_meta

            return claude_native_subagent_restriction_meta(role=role, effort=effort)
        return None
    unified_base_prompt = unified_base_prompt_enabled(environ)
    from ..engine.mcp_bridge import is_consultant_role

    # ``role_id`` (the worker profile) selects the senior consultant's base
    # prompt, which names its packet and reports instead of a code index.
    consultant = role == "worker" and is_consultant_role(role_id)
    logging.getLogger(__name__).info(
        "Cortex unified base prompt %s for engine session metadata",
        "enabled" if unified_base_prompt else "disabled",
    )

    if engine == "claude-agent-acp" and has_exact_registration(engine, version):
        rules = project_rules_for_claude(project_root)
        if rules:
            instructions = instructions + "\n\n" + rules
        # Keep the registry's qualification and instruction channel, but add
        # the recipient-specific native built-in policy at the runtime seam.
        # The registry is intentionally role-agnostic; this adapter is where
        # the launch role is already known.
        from ..engine.claude_adapter_acp import build_claude_session_new_meta

        base_prompt = None
        if unified_base_prompt:
            from ..engine.unified_system_prompt import cortex_system_prompt, prompt_access_class

            base_prompt = cortex_system_prompt(
                role, project_root, environ=environ, consultant=consultant,
                access_class=prompt_access_class(role, role_id), native_tools=False)
        return build_claude_session_new_meta(
            instructions, role=role, effort=effort, base_prompt=base_prompt,
            native_skills=not unified_base_prompt,
        )
    from ..engine.instruction_delivery_registry import delivery_for
    from ..portable.instruction_delivery import DeliveryQualification

    mechanism = delivery_for(engine, version)
    plan = mechanism.plan(_InstructionsOnlyRole(instructions), [])
    if plan.qualification != DeliveryQualification.QUALIFIED:
        return None
    meta = plan.session_new_meta
    if engine == "codex-acp" and meta is not None:
        # The same unified Cortex prompt the Claude branch sends, in place of
        # Codex's own base prompt and native context sections.
        from ..engine.codex_adapter_acp import build_codex_session_new_meta

        base_instructions = None
        if unified_base_prompt:
            from ..engine.unified_system_prompt import cortex_system_prompt, prompt_access_class

            base_instructions = cortex_system_prompt(
                role, project_root, environ=environ, consultant=consultant,
                access_class=prompt_access_class(role, role_id), native_tools=True)
        # Codex's code mode hides each tool's full argument schema behind
        # exec, showing only name+description; append a small, mechanically
        # schema-derived cheat sheet of required fields + an example call
        # for the most-used tools -- codex-acp sessions only.
        from .codex_tool_cheat_sheet import codex_tool_cheat_sheet

        cheat_sheet = codex_tool_cheat_sheet(role, role_id=role_id, stage=stage)
        instructions_for_codex = instructions if not cheat_sheet else instructions + "\n\n" + cheat_sheet
        meta = {**meta, **build_codex_session_new_meta(
            instructions_for_codex, base_instructions=base_instructions)}
    if engine == "codex-acp" and meta is not None:
        return {**meta, "cortexRole": role}
    return meta


def base_prompt_mode(meta) -> str:
    """Return the safely observable base-prompt mode from sent session metadata."""
    if not isinstance(meta, Mapping):
        return "unknown"
    if isinstance(meta.get("opencodeAgentPrompt"), str):
        # Recorded by _open_generation for OpenCode, whose prompt is the
        # launch-time agent prompt, not session metadata.
        return "unified" if meta["opencodeAgentPrompt"].startswith("# Cortex session\n\n") else "native"
    if "cortexBaseInstructions" in meta:
        return "unified"
    # Codex metadata always carries one of these markers.  Check them before
    # ``systemPrompt`` because that field is role text in both Codex modes.
    if "cortexNativeContext" in meta or "cortexRole" in meta:
        return "native"
    system_prompt = meta.get("systemPrompt")
    if isinstance(system_prompt, str):
        return "unified"
    if isinstance(system_prompt, Mapping) and "append" in system_prompt:
        return "native"
    return "unknown"


def _resolve_route_model_name(connection_id: str, model: str, environ: dict[str, str] | None) -> str | None:
    """T035: this route's real, engine-observed model display name -- the
    identical resolution ``acp.config_options.presented_entries`` applies to
    a fresh ``session/new``'s own ``configOptions`` -- read from this
    connection's latest cached catalog snapshot
    (``catalog.store.latest_connection_entries``, never a fresh live probe:
    a route audit runs long after the session that dispatched it opened).

    ``None`` when no snapshot has ever been cached for this connection, no
    cached entry's ``route.model`` matches ``model``, or the matching entry
    carries no evidence beyond the bare alias/id itself
    (``labels.presented_model_name``) -- callers must then show the alias
    explicitly marked as an alias, never fabricate a version.
    """
    from ..catalog import store as catalog_store
    from ..catalog.labels import presented_model_name

    entries = catalog_store.latest_connection_entries(connection_id, environ)
    if not entries:
        return None
    listed = {entry.route.model for entry in entries}
    for entry in entries:
        if entry.route.model == model:
            return presented_model_name(entry.route.model, entry.model_name, entry.model_description, listed)
    return None


class _InstructionsOnlyRole:
    """A minimal stand-in for ``portable.roles.RoleInstructionSet`` carrying
    only the one field every current ``InstructionDelivery.plan()``
    implementation actually reads (``body_portable``) -- used here because
    this call site only ever has the already-resolved instructions string
    (``delegation.provisioning``'s own ``role_set.body_portable``), never
    the full ``RoleInstructionSet`` object itself (see
    ``EngineSessionOpener.open``'s ``instructions: str`` parameter, a
    deliberately narrow protocol boundary -- see
    ``docs/features/roles-and-delegation/index.md``)."""

    def __init__(self, body_portable: str) -> None:
        self.body_portable = body_portable


class SharedProjectRootWorkspaceProvisioner:
    """Non-isolated ``WorkspaceProvisioner``: every attempt shares the task's
    one project root. See module docstring: T025 owns real isolation."""

    def create(self, attempt_id: str) -> str:
        return "shared_project_root"

    def lookup(self, attempt_id: str) -> str:
        return "present"

    def remove(self, attempt_id: str) -> None:
        return None


class ConfigurationServiceSnapshotSource:
    """``ConfigSnapshotSource`` over an existing ``ConfigurationService``.

    Requires a task to already have a current configuration revision (set
    by the normal T008 route-selection flow) before a coordinator or
    delegated worker can be provisioned; this is a read-only seam, it never
    selects a route on the caller's behalf.
    """

    def __init__(self, configuration_service) -> None:
        self._service = configuration_service

    def snapshot(self, task_id: str, role_id: str | None = None) -> Mapping[str, object]:
        # T031: the active revision if one exists, else the latest
        # proposed-but-not-yet-applied ('selected') one -- resolving which
        # route to *open a session against* no longer requires that
        # session to already exist (see ``EngineSessionManagerAdapter
        # .open``'s own post-open ``ConfigurationService.apply()`` call,
        # the only place that ever promotes a 'selected' revision to
        # 'active', using the exact same route this snapshot named).
        current = self._service.current_or_latest(task_id)
        if not current:
            from ..portable.errors import PortableError, PortableErrorCategory

            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="no_configuration_revision",
                remediation="This task has no current configuration revision; select a route before starting orchestration.",
                retry_eligible=False,
            )
        engine = current.get("route_engine")
        resolved_route = None
        # EXT-01 (mixed-team delegation via the ordinary path, not only
        # fallback): a role passed by ``delegation.provisioning.delegate``
        # may be authorized for a different route than the coordinator's
        # own (``configuration.service.ConfigurationService
        # .resolve_worker_route``, T027's stored mixed-team profile).
        # ``resolved_route`` is ``None`` for every call with no ``role_id``
        # and for every role that resolves to the coordinator's own route
        # unchanged -- so an ordinary (non-mixed-team) delegation is
        # byte-for-byte the same snapshot as before this fix, and
        # ``delegation.provisioning`` threads a non-``None`` value into the
        # launch dict only for an actual override, so ``open`` still uses
        # its own validated ``connection_record_factory`` (live
        # login/credential state, T009 finding 09) for every attempt that
        # never overrides the coordinator's route.
        if role_id is not None:
            override = self._service.resolve_worker_route(task_id, role_id)
            if override is not None:
                engine = override["route_engine"]
                resolved_route = dict(
                    route_id=override["route_id"], connection_id=override["route_connection_id"],
                    model=override.get("route_model"),
                )
        return dict(
            engine=engine,
            # The exact evidence-qualified version per engine (see
            # engine.instruction_delivery_registry's ("codex-acp","2.1.1")
            # pin, T013); configuration_revisions does not itself track a
            # downstream engine version column, so this mirrors the one
            # pinned mapping instead of inventing a second source of truth.
            version=_pinned_version_for(engine),
            config_revision=current.get("id"),
            # T023: the connection actually selected by this revision --
            # recovery.coordinator_recovery/registry compares this against
            # a recorded attempt's own route to detect an unauthorized
            # silent provider switch. Always the coordinator's own
            # connection, unaffected by ``resolved_route`` above (existing
            # callers/consumers of this field are unchanged).
            route_connection_id=current.get("route_connection_id"),
            # The single resolved override route id/connection/model for
            # this attempt (T027 carried item), or ``None`` -- threaded
            # verbatim into the launch dict by ``delegation.provisioning``
            # so ``open``'s own dispatch authorization, connection
            # selection, worker-mode confirmation, and audit all use the
            # exact same already-resolved value instead of each re-
            # deriving a different one.
            resolved_route=resolved_route,
        )


_ENGINE_CAPABILITIES = None


def _requested_capabilities():
    global _ENGINE_CAPABILITIES
    if _ENGINE_CAPABILITIES is None:
        from ..portable.engine_contract import EngineCapability

        _ENGINE_CAPABILITIES = (
            EngineCapability.SESSION_CREATION,
            EngineCapability.PROMPT_STREAMING,
            EngineCapability.CANCELLATION,
        )
    return _ENGINE_CAPABILITIES


class _PortableRevisionConfigureAdapter:
    """T031: the ``adapter`` :meth:`~..configuration.service
    .ConfigurationService.apply` calls -- translates the flat storage-row
    ``revision`` dict ``apply()`` always passes into the portable
    ``ConfigurationRevision``/``ExecutionRoute`` shape
    ``engine.session_manager.EngineSessionManager.configure`` (and the
    adapter-specific ``codex_option_mapping``/``claude_option_mapping``
    callables it uses) actually expects, joining the route's model string
    from ``configuration_route_extras`` (``routing.store.get_route_extras``)
    since ``configuration_revisions`` itself never stores it in plain
    text. Constructed fresh per apply attempt; never cached."""

    def __init__(self, store, manager, *, route_model_override: str | None = None) -> None:
        self._store = store
        self._manager = manager
        # EXT-01 (fallback source-vs-target confusion): ``configuration_
        # route_extras.route_model`` is keyed by ``revision["id"]`` alone
        # and always names *that revision's own* (coordinator's) model --
        # never any different route a specific attempt actually opened
        # against (a fallback's ``route_override["model"]``, or an
        # ordinary mixed-team attempt's own ``resolved_model``, T027).
        # ``_apply_worker_mode_floor`` supplies this attempt's own actual
        # model here so :meth:`configure` never falls back to reading the
        # wrong (source) model out of ``configuration_route_extras`` for
        # it; every other caller (``ConfigurationService.apply``, always
        # the coordinator's own not-yet-overridden route) leaves this
        # ``None`` and keeps the original extras lookup, unchanged.
        self._route_model_override = route_model_override

    def configure(self, engine_session_id: str, revision: Mapping[str, object]) -> str:
        from ..portable.identity import ConfigurationRevision, ExecutionRoute
        from ..routing import store as routing_store

        if self._route_model_override is not None:
            route_model = self._route_model_override
        else:
            extras = routing_store.get_route_extras(self._store, revision["id"])
            route_model = extras["route_model"] if extras is not None else revision["route_engine"]
        route = ExecutionRoute(
            engine=revision["route_engine"], connection_id=revision["route_connection_id"],
            model=route_model, supported_parameters=tuple(sorted(revision["parameters"].items())),
        )
        portable_revision = ConfigurationRevision(
            id=revision["id"], task_id=revision["task_id"], revision=revision["revision"], route=route,
        )
        return self._manager.configure(engine_session_id, portable_revision)


class WorkerModeNotConfirmed(RuntimeError):
    """Raised by :meth:`EngineSessionManagerAdapter._apply_worker_mode_floor`
    (T035): this worker's own engine mode (``configuration.policy
    .WORKER_MODE_FLOOR``) could not be confirmed over the wire -- either the
    engine never offered it at all, or its own confirm-or-fail
    ``session/set_config_option`` call rejected/only partially applied it.
    Always caught by :meth:`EngineSessionManagerAdapter.open`, which tears
    down the just-opened session/process (never leaks it) before
    re-raising -- see that call site's own comment for why only this one
    attempt is refused, never the whole task."""


class QueuedOpenCancelled(RuntimeError):
    """Raised by :meth:`EngineSessionManagerAdapter.open` (from
    ``_reserve_capacity_blocking``) when a queued-for-capacity open is
    cancelled before ever reserving -- never a leaked reservation, never a
    live engine process started for it. ``kind`` is ``"attempt"`` (a
    targeted ``cancel_attempt``), ``"task"`` (``cancel_task``), or
    ``"shutdown"`` (:meth:`EngineSessionManagerAdapter.shutdown`)."""

    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(kind)


# Catalog observations already saved: connection id -> (observation
# fingerprint, saved at). Every Paseo chat is its own Cortex process, so the
# fingerprint is also kept beside that connection's catalog snapshots.
_CATALOG_OBSERVED: dict[str, tuple[str, float]] = {}
_CATALOG_OBSERVED_LOCK = threading.Lock()
_CATALOG_OBSERVATION_TTL_SECONDS = 6 * 3600
_CATALOG_FINGERPRINT_FILE = "observed-fingerprint.json"


def _catalog_observation_fingerprint(record, install, raw) -> str:
    """Identity of one session's configOptions for one install, without the
    session's own current selections."""
    def strip(value):
        if isinstance(value, dict):
            return {k: strip(v) for k, v in value.items() if k != "currentValue"}
        if isinstance(value, list):
            return [strip(v) for v in value]
        return value
    body = json.dumps(dict(record=record.id, engine=record.engine, install=repr(install), options=strip(raw)),
                      sort_keys=True, default=str, separators=(",", ":"))
    return hashlib.sha256(body.encode("utf-8")).hexdigest()


def _catalog_fingerprint_path(connection_id: str, environ) -> Path:
    from ..connections import registry as connection_registry
    return connection_registry.state_root(environ) / "catalog" / connection_id / _CATALOG_FINGERPRINT_FILE


def _catalog_observation_fresh(connection_id: str, fingerprint: str, environ) -> bool:
    from ..catalog import store as catalog_store
    from ..connections import registry as connection_registry
    with _CATALOG_OBSERVED_LOCK:
        seen = _CATALOG_OBSERVED.get(connection_id)
    fresh = (seen is not None and seen[0] == fingerprint
             and time.monotonic() - seen[1] <= _CATALOG_OBSERVATION_TTL_SECONDS)
    try:
        if not fresh:
            data = connection_registry.read_owner_file(_catalog_fingerprint_path(connection_id, environ))
            saved = json.loads(data.decode("utf-8")) if data else {}
            fresh = (isinstance(saved, dict) and saved.get("fingerprint") == fingerprint
                     and isinstance(saved.get("saved_at"), (int, float))
                     and 0 <= time.time() - saved["saved_at"] <= _CATALOG_OBSERVATION_TTL_SECONDS)
        return fresh and catalog_store.latest_connection_snapshot(connection_id, environ) is not None
    except Exception:  # noqa: BLE001 - an unreadable cache is observed again
        return False


def _remember_catalog_observation(connection_id: str, fingerprint: str, environ) -> None:
    from ..connections import registry as connection_registry
    with _CATALOG_OBSERVED_LOCK:
        _CATALOG_OBSERVED[connection_id] = (fingerprint, time.monotonic())
    try:
        path = _catalog_fingerprint_path(connection_id, environ)
        body = json.dumps(dict(fingerprint=fingerprint, saved_at=time.time())).encode("utf-8")
        connection_registry.atomic_write_private(path, body, directory=path.parent)
    except Exception:  # noqa: BLE001 - only the next process observes again
        pass


class EngineSessionManagerAdapter:
    """Live ``acp.orchestrating_runtime.SessionRuntime`` over one
    ``EngineSessionManager`` per attempt. See module docstring.

    ``store``/``capacity_limit`` (T017, optional, off by default): when a
    ``store`` is supplied, every :meth:`open` atomically reserves one
    capacity unit for the resolved connection (see ``delegation.capacity``)
    before launching the process, and :meth:`terminate_owned` releases it
    only once the underlying process's exit is *positively observed*
    (``EngineProcess.wait_exited``); an unconfirmed termination (grace
    expired with the process still not observed exited) is marked
    ``ambiguous`` instead and keeps counting against the limit. A worker
    runs only while its chat session (the task) holds fewer than
    ``delegation.capacity.MAX_WORKERS_PER_SESSION`` workers on every
    connection together; the resolved connection's own
    ``max_concurrent_sessions`` (default 4) can only lower how many of that
    session's workers use the connection. Coordinators keep their own
    per-connection count; an explicit ``capacity_limit`` can only lower it.
    A queued open reclaims a slot whose holder is observed gone
    (:meth:`reclaim_stale_capacity`) instead of waiting on it forever.
    """

    def __init__(
        self,
        connection_record_factory: Callable[[str], object],
        workspace_root_factory: Callable[[str], str],
        *,
        environ: Mapping[str, str] | None = None,
        store=None,
        capacity_limit: int | None = None,
        capacity_queue_timeout_seconds: float = 600.0,
        workspace_resolver: Callable[[str, Mapping[str, object]], str] | None = None,
        project_root_factory: Callable[[str], str] | None = None,
        workspace_lock_factory: Callable[[], object] | None = None,
        workspace_lock_timeout_seconds: float = 600.0,
        **manager_kwargs,
    ) -> None:
        self._connection_record_factory = connection_record_factory
        self._workspace_root_factory = workspace_root_factory
        # EXT-01 finding #1 fix: the *execution* workspace root (this
        # attempt's cwd -- a throwaway isolated copy under
        # ``workspace_resolver``, excluding both ``.git`` and ``.cortex``)
        # and the *logical* project root (where the one real
        # ``.cortex/cortex.sqlite3`` store actually lives, which
        # ``engine.mcp_bridge.build_mcp_servers``/``server.py`` need in
        # ``channel_credentials``/``CORTEX_CHANNEL_PROJECT_ROOT``) are not
        # the same path once isolation is in play. ``project_root_factory``
        # -- ``task_id -> project_root`` -- is the explicit, opt-in seam for
        # the caller's already-known real project root; when ``None`` (every
        # pre-existing caller/test, all of which only ever use the shared,
        # non-isolated ``workspace_root_factory`` where the two paths are
        # identical), behaviour is unchanged: the resolved workspace root is
        # reused as the project root, exactly as before.
        self._project_root_factory = project_root_factory
        # T025 (optional, opt-in -- every existing caller keeps its
        # single-argument ``workspace_root_factory(task_id)`` behaviour
        # unchanged): when supplied, resolves the *actual* per-attempt
        # workspace root from this open()'s own ``workspace_binding_id``
        # (the ref ``delegation.provisioning.WorkspaceProvisioner.create``
        # returned -- see ``delegation.workspace_isolation
        # .IsolatedSnapshotWorkspaceProvisioner``) instead of the
        # task-wide shared root, so two attempts of the same task can
        # actually run against two different, non-overlapping directories.
        self._workspace_resolver = workspace_resolver
        self._workspace_lock_factory = workspace_lock_factory
        self._workspace_lock_timeout_seconds = workspace_lock_timeout_seconds
        self._environ = environ
        self._manager_kwargs = manager_kwargs
        self._store = store
        self._capacity_limit = capacity_limit
        self._queue_timeout_seconds = capacity_queue_timeout_seconds
        self._lock = threading.Lock()
        self._managers: dict[str, object] = {}
        self._sessions: dict[str, str] = {}
        self._session_to_attempt: dict[str, str] = {}
        self._reservations: dict[str, str] = {}
        self._attempt_task: dict[str, str] = {}
        # The (connection record, model) each worker attempt opened on, so
        # a later turn refused for the route can name it (routing.availability).
        self._attempt_routes: dict[str, tuple[object, str | None]] = {}
        # A delivery refusal is version-wide, not a reason to try another model.
        self._unqualified_instruction_routes: set[tuple[str, str, str]] = set()
        self._workspace_locks: dict[str, object] = {}
        self._termination_locks: dict[str, threading.Lock] = {}
        self._result_settlement_managers: dict[str, object] = {}
        # T017 (fresh review, MEDIUM): a queued open must observe
        # cancellation promptly instead of polling forever.
        self._cancelled_attempts: set[str] = set()
        self._cancelled_tasks: set[str] = set()
        # Attempts between their capacity reservation and their session
        # registration: their reservation has no process record yet and is
        # never reclaimed as stale (see reclaim_stale_capacity).
        self._opening: set[str] = set()
        self._shutdown_event = threading.Event()
        # T022: context-generation/delivery tracking, active only when
        # ``store`` is supplied (same gate T017's capacity tracking uses --
        # a caller/test with no store gets the pre-T022 behavior
        # unchanged). ``_ever_opened_attempts`` distinguishes a brand new
        # attempt's first session (cause=session_open) from a retried
        # attempt reopening a session (cause=session_replacement).
        # ``_pending_generation``/``_session_meta`` are keyed by portable
        # ``session_ref``.
        self._ever_opened_attempts: set[str] = set()
        self._pending_generation: dict[str, dict] = {}
        self._session_meta: dict[str, dict] = {}
        # EXT-01 finding #2: per-*session* configuration confirmation record
        # -- keyed by this adapter's own portable session_ref, populated and
        # consulted only by :meth:`reconcile_configuration`. Deliberately
        # in-memory, never a durable store row: no session_ref this adapter
        # ever hands out survives this process's own lifetime (a restart or
        # a coordinator recovery always opens a genuinely new session -- see
        # :meth:`open`'s ``launch['recovery']`` path), so this record always
        # starts empty for a new session and is therefore always
        # reconfirmed, regardless of what ``configuration_revisions`` (a
        # *task*-level, cross-session row) already says. See
        # :meth:`reconcile_configuration`'s own docstring for the full
        # rationale -- this is deliberately never merged into
        # ``configuration_revisions`` itself, whose ``active`` state is
        # immutable by DB trigger (``configuration_revisions_guard``) once
        # reached, precisely because it is cross-session task history, not
        # one session's own confirmation.
        self._session_configured_revision: dict[str, str] = {}
        # T017 (load-test fix): notified on every confirmed/ambiguous
        # release and every cancel/shutdown signal, so a queued open wakes
        # promptly instead of relying purely on its own backoff poll
        # interval (up to 1s) under real scheduling contention.
        self._activity_cond = threading.Condition()
        # EXT-01 #5: optional back-reference to the owning
        # ``acp.orchestrating_runtime.OrchestratingRuntime`` (set once, after
        # both are constructed -- see ``set_runtime``'s own docstring for why
        # the two cannot be constructed in the other order). ``None`` means
        # "no runtime wired" (every pre-EXT-01 caller/test): every engine
        # session this adapter opens then gets no ``host_permission_requester``
        # at all, the exact same fail-closed-to-cancelled behaviour as before
        # this fix existed.
        self._runtime = None

    def set_runtime(self, runtime) -> None:
        """EXT-01 #5: wires this adapter to the
        ``acp.orchestrating_runtime.OrchestratingRuntime`` it is the
        ``SessionRuntime`` for -- called once by the caller
        (``acp.production_runtime.ProductionOrchestratingRuntime``) right
        after constructing that runtime with this adapter, which is itself
        constructed first (the runtime's own constructor requires an
        already-built ``SessionRuntime``); this setter is the seam that
        closes that ordering gap. Every ``EngineSessionManager`` this
        adapter opens *after* this call gets a real
        ``host_permission_requester`` (see :meth:`_host_permission_requester`)
        that maps its own ``engine_session_id`` back to this adapter's own
        already-tracked owning ``task_id`` (``_session_to_attempt`` +
        ``_attempt_task``, both populated by :meth:`open` itself -- never a
        second, independent identity scheme) and then asks the runtime for
        *that task's* currently-registered, connection-bound requester (see
        ``OrchestratingRuntime.permission_requester_for_task``)."""
        self._runtime = runtime

    def _host_permission_requester(self, engine_session_id, params, timeout):
        """EXT-01 #5: the actual ``host_permission_requester`` passed to
        every ``EngineSessionManager`` this adapter opens (see :meth:`open`)
        -- resolves ``engine_session_id`` to the one task it belongs to
        using this adapter's own bookkeeping, then delegates to that task's
        currently-registered requester. Returns ``None`` (mapped by
        ``engine.permissions.PermissionBridge`` to the same ``cancelled``
        outcome as "no host requester at all") whenever no runtime is wired,
        the session is unknown to this adapter, or no requester is
        currently registered for its task (no ``start_prompt`` call for that
        task is in flight on any connection right now) -- never guesses,
        never reuses a different task's requester."""
        if self._runtime is None:
            return None
        with self._lock:
            attempt_id = self._session_to_attempt.get(engine_session_id)
            task_id = self._attempt_task.get(attempt_id) if attempt_id is not None else None
        if task_id is None:
            return None
        requester = self._runtime.permission_requester_for_task(task_id)
        if requester is None:
            return None
        return requester(engine_session_id, params, timeout)


    def _resolve_launch_install(self, record):
        """T035 follow-up ("a connection's launch command is trusted
        verbatim, never re-verified"): before a real engine is spawned,
        re-derive and re-verify this connection's own install straight from
        its recorded ``launch_command`` (see
        ``engine.install_resolution.resolve_verified_install``) -- for
        *both* engines, not just codex-acp (previously this constructor
        never passed ``install`` at all, for either engine, so
        ``EngineProcess`` never cross-checked ``launch_command`` against
        anything; that gap is exactly how a wrong entry point -- a library
        module with no stdin listener instead of the package's own declared
        ``bin`` -- reached live testing).

        A ``launch_command`` that is not install-shaped at all (this
        repository's own fake-engine test doubles, whose launch_command is
        never the exact ``(node_binary, adapter_entry)`` a real pinned
        install produces) is reported as
        ``install_resolution.InstallUnresolvable`` and launches unchanged
        (``None``, the same as every caller before this check existed).
        Anything install-shaped that then fails verification (hash drift,
        wrong declared entry point, untrusted location) is **not** caught
        here -- it propagates out of :meth:`open` and refuses the launch,
        unlike ``catalog.refresh``'s own non-fatal use of the same
        resolver, which only ever supplements an already-successful
        refresh."""
        from ..engine import install_resolution

        try:
            return install_resolution.resolve_verified_install(record, self._environ)
        except install_resolution.InstallUnresolvable:
            return None

    def _assignment_stage(self, task_id: str, attempt_id: str) -> str | None:
        """Resolve the latest node stage with the same binding as tools/list."""
        if self._store is None:
            return None
        with self._store.connection() as db:
            row = db.execute(
                "SELECT n.stage FROM delegation_provisioning p JOIN runtime_nodes n "
                "ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key "
                "WHERE p.task_id=? AND p.attempt_id=? ORDER BY n.revision DESC LIMIT 1",
                (task_id, attempt_id),
            ).fetchone()
        return row["stage"] if row is not None else None

    def open(self, workspace_binding_id: str, launch: Mapping[str, object], instructions: str) -> str:
        from ..engine.session_manager import EngineSessionManager

        attempt_id = launch["attempt_id"]
        task_id = launch["task_id"]
        channel_id = launch.get("channel_id")
        channel_secret = launch.get("channel_secret")
        if channel_id and not channel_secret:
            # T016 review (HIGH): never silently launch a worker/coordinator
            # engine session with a channel_id but no secret -- that would
            # start a live engine process with no way to authenticate its
            # own Cortex MCP subprocess, reaching 'ready' with a session
            # that can never actually call a delegation tool.
            raise RuntimeError(
                "EngineSessionManagerAdapter.open refuses to open an engine session for "
                f"channel_id={channel_id!r} with no channel_secret; the caller must supply "
                "the freshly-issued secret or fail the delegation explicitly instead of "
                "calling open()."
            )
        if self._workspace_resolver is not None:
            workspace_root = self._workspace_resolver(workspace_binding_id, launch)
        else:
            workspace_root = self._workspace_root_factory(task_id)
        # EXT-01 finding #1: the MCP server's ``project_root`` must be this
        # task's one real logical project root, never the (possibly
        # isolated, throwaway) execution ``workspace_root`` above -- see
        # ``_project_root_factory``'s own docstring in ``__init__``.
        project_root = (
            self._project_root_factory(task_id)
            if self._project_root_factory is not None
            else workspace_root
        )
        channel_credentials = (
            (channel_id, channel_secret, project_root)
            if channel_id and channel_secret else None
        )
        # D-T035-04: the coordinator's own explicit (or omitted, meaning
        # "pick the role's first allowed policy entry") route choice for
        # this one delegation, resolved here -- into ``launch["resolved_
        # connection_id"]``/``["resolved_model"]``/``["parameters"]
        # ["effort"]`` -- against the route policy and connections
        # registry this method already loads (see ``_authorize_dispatch``)
        # before the override-connection-id selection just below ever
        # runs, so a coordinator-chosen route reaches ``record`` exactly
        # like a T027 mixed-team one. A no-op for the coordinator's own
        # session (``launch`` carries no ``assignment_id``) and whenever a
        # fallback ``route_override`` is already present (fallback always
        # wins, unchanged).
        is_worker = bool(launch.get("assignment_id")) and self._store is not None
        unavailable = ()
        if is_worker:
            unavailable = self._worker_unavailable_routes(launch)
        if self._store is not None:
            self._resolve_runtime_route(launch, project_root, unavailable)
        # EXT-01 carried item: exactly one resolved connection id for this
        # attempt, from exactly one of two sources, both already-authorized
        # or already-role-resolved *before* open() ever runs -- never both,
        # never neither re-derived independently downstream. A fallback
        # attempt's ``route_override`` (already authorized by
        # ``routing.authorization.authorize_fallback``, see
        # :meth:`attempt_fallback`) always wins when present; an ordinary
        # mixed-team delegation's own ``resolved_connection_id`` (T027
        # stored route profile, resolved once by
        # ``configuration.service.ConfigurationService.resolve_worker_route``
        # and threaded through unchanged by
        # ``delegation.provisioning.ProvisioningService``) is used only when
        # no fallback override is present. Neither is authorized here --
        # ``_authorize_dispatch`` below still enforces deny-by-default
        # against this exact same id via ``launch["resolved_route_id"]``.
        # Absent both (every non-mixed-team, non-fallback attempt -- the
        # overwhelming majority), the task's own validated
        # ``connection_record_factory`` (live login/credential state, T009
        # finding 09) resolves the connection exactly as before this fix.
        route_override = launch.get("route_override")
        override_connection_id = (
            route_override["connection_id"] if route_override is not None
            else launch.get("resolved_connection_id")
        )
        if override_connection_id is not None:
            from ..connections import registry as connection_registry

            state = connection_registry.read_registry(self._environ)
            record = state.connections.get(override_connection_id)
            if record is None:
                raise RuntimeError(
                    f"EngineSessionManagerAdapter.open: resolved connection "
                    f"{override_connection_id!r} no longer exists."
                )
        else:
            record = self._connection_record_factory(task_id)
        route_model = self._launch_model(launch) if is_worker else None
        if is_worker:
            self._refuse_unavailable_route(record, route_model, unavailable)
        if self._store is not None:
            self._authorize_dispatch(record, task_id, launch, project_root=project_root)
        reservation_id = None
        with self._lock:
            self._opening.add(attempt_id)
            self._attempt_task[attempt_id] = task_id
        if self._store is not None:
            from . import capacity as delegation_capacity

            try:
                reservation_id = self._reserve_capacity_blocking(
                    record.id, task_id, attempt_id, launch,
                    limit=delegation_capacity.limit_for_connection(record, self._capacity_limit),
                )
            except BaseException:
                with self._lock:
                    self._opening.discard(attempt_id)
                raise
        # EXT-01 #8 ("startup cleanup"): from here on a real process may
        # exist (``EngineSessionManager._ensure_started`` spawns it lazily,
        # inside ``negotiate()``) before this method ever reaches its own
        # atomic bookkeeping below -- a failure in install verification,
        # negotiate (``initialize``), or ``session/new`` must never leak
        # the capacity slot just reserved above, nor leave that process
        # running with nothing in this adapter (or the durable registry)
        # able to find and stop it later. ``manager`` is the one local
        # reference to it for exactly that reason: this same stack frame
        # both spawned it and is the first to know it failed, so recovery
        # never has to guess. See ``_cleanup_failed_open`` for the
        # confirmed-vs-ambiguous distinction this reuses verbatim from
        # ``terminate_owned``.
        manager = None
        session_id = None
        workspace_lock = None
        workspace_lock_acquired = False
        try:
            # A direct project worker owns the project writer lock for the
            # complete engine lifetime.  Taking it only around launch would
            # allow two workers to edit the same ACP project concurrently.
            if launch.get("assignment_id") and self._workspace_lock_factory is not None:
                workspace_lock = self._workspace_lock_factory(
                    timeout=self._workspace_lock_timeout_seconds
                )
                workspace_lock.__enter__()
                workspace_lock_acquired = True
            # T035 FINDING-13 #3: a cancellation that lands while this
            # attempt was only queued for capacity is already handled inside
            # _reserve_capacity_blocking (never reaches here at all); this
            # and the two checks below are the same ``_cancelled_attempts``/
            # ``_cancelled_tasks`` bookkeeping re-checked at each subsequent
            # blocking wire round-trip (``negotiate``'s ``initialize``,
            # ``create_session``'s ``session/new`` -- each individually
            # bounded by ``initialize_timeout``/``call_timeout``, never
            # unbounded), so a cancellation is never left to wait for the
            # *whole* open() to run to completion (see
            # ``EngineSessionOpener.open``'s Protocol docstring): the worst
            # case is whichever single round-trip is already in flight,
            # not the full multi-step launch. A cancellation observed here
            # raises the exact same ``QueuedOpenCancelled`` the capacity
            # wait already raises, so the ``except BaseException`` below
            # tears the just-spawned process down and releases capacity
            # through the one existing cleanup path -- no separate cleanup
            # code needed for this case.
            self._check_not_cancelled(attempt_id, task_id)
            manager_kwargs = dict(self._manager_kwargs)
            # T026: route each connection's own pinned adapter option_mapping
            # (never a caller-supplied one, which always wins) -- see
            # _option_mapping_for's own docstring for why this was previously
            # always the generic default for every engine including codex-acp.
            if "option_mapping" not in manager_kwargs:
                mapping = _option_mapping_for(record.engine)
                if mapping is not None:
                    manager_kwargs["option_mapping"] = mapping
            if "install" not in manager_kwargs:
                manager_kwargs["install"] = self._resolve_launch_install(record)
            if "coordinator_refusal_notifier" not in manager_kwargs:
                manager_kwargs["coordinator_refusal_notifier"] = (
                    self._notify_coordinator_refusal if self._store is not None else None
                )
            # EXT-01 #5: a caller-supplied ``host_permission_requester`` (every
            # existing direct test -- see e.g.
            # ``test_delegation_worker_mode_floor.py``) always wins unchanged;
            # only when none was given does this adapter wire its own
            # runtime-backed lookup (a no-op -- returns ``None``, same as before
            # this fix -- until :meth:`set_runtime` has actually been called).
            if "host_permission_requester" not in manager_kwargs:
                manager_kwargs["host_permission_requester"] = self._host_permission_requester
            if "host_mcp_servers" not in manager_kwargs:
                # The client's own MCP servers, so a coordinator and every
                # worker see the same tools the user configured in their
                # editor. ``None`` until set_runtime has been called, which
                # is every direct test -- unchanged for them.
                runtime = getattr(self, "_runtime", None)
                getter = getattr(runtime, "host_mcp_servers_for_task", None)
                if getter is not None:
                    manager_kwargs["host_mcp_servers"] = getter(task_id)
            recipient = "worker" if launch.get("assignment_id") else "coordinator"
            if "mcp_role" not in manager_kwargs:
                manager_kwargs["mcp_role"] = recipient
            if "mcp_role_id" not in manager_kwargs:
                # The worker profile decides per-role attachment: the senior
                # consultant gets no external MCP server (code index included)
                # and no native tool approvals (engine.mcp_bridge
                # .receives_external_mcp, engine.permissions.PermissionBridge).
                manager_kwargs["mcp_role_id"] = launch.get("role_id")
            if record.engine == "opencode" and "engine_instructions" not in manager_kwargs:
                # OpenCode v2 takes its role prompt at process launch
                # only (engine.instruction_delivery_registry
                # .OpenCodeAgentPromptDelivery); session/new gets no _meta.
                from ..engine.mcp_bridge import is_consultant_role
                from ..engine.opencode_adapter_acp import build_agent_prompt

                manager_kwargs["engine_instructions"] = build_agent_prompt(
                    instructions, role=recipient, project_root=workspace_root, environ=self._environ,
                    consultant=recipient == "worker" and is_consultant_role(launch.get("role_id")),
                    role_id=launch.get("role_id"))
            if "jail" not in manager_kwargs and self._store is not None and channel_credentials is not None:
                manager_kwargs["jail"] = _jail_for(self._store, channel_credentials)
            if recipient == "worker" and self._store is not None:
                from ..orchestration.measurement import attempt_event
                manager_kwargs["lifecycle_observer"] = (
                    lambda kind, session, payload=None: attempt_event(
                        self._store, task_id, attempt_id, kind, session, payload=payload))
            from ..observability.wire_trace import get_trace
            if get_trace() is not None:
                manager_kwargs["trace_identity"] = dict(
                    task_id=task_id, assignment_ref=launch.get("assignment_id"),
                    attempt_id=attempt_id)
            manager = EngineSessionManager(
                record, workspace_root, environ=self._environ,
                channel_credentials=channel_credentials,
                terminal_output_persist=self._persist_terminal_output,
                **manager_kwargs,
            )
            manager.negotiate(_requested_capabilities())
            self._check_not_cancelled(attempt_id, task_id)
            # Both pinned adapters now receive their qualified session/new
            # metadata. Codex also receives its tagged generation prompt.
            session_new_meta = _session_new_meta_for(
                record.engine, _pinned_version_for(record.engine, record), instructions, role=recipient,
                project_root=workspace_root, effort=_sdk_effort_for(record, launch), environ=self._environ,
                role_id=launch.get("role_id"), stage=self._assignment_stage(task_id, attempt_id),
            )
            session_id = manager.create_session(
                task_id, launch.get("assignment_id") or attempt_id, session_new_meta=session_new_meta,
            )
            self._check_not_cancelled(attempt_id, task_id)
            # T035 (additive, best-effort): feed this real session's own
            # actually-observed configOptions into catalog.discovery, so the
            # provider/model/thought_level selects reflect the engine's real
            # advertised set from here on -- never blocking/failing the engine
            # session itself on a capture problem.
            self._capture_catalog_observation(manager, record, session_id)
        except BaseException as error:
            # Retain ownership while teardown runs, even before session/new.
            with self._lock:
                if manager is not None:
                    self._managers[attempt_id] = manager
                if reservation_id is not None:
                    self._reservations[attempt_id] = reservation_id
                if workspace_lock_acquired:
                    self._workspace_locks[attempt_id] = workspace_lock
            if self._store is not None and manager is not None:
                try:
                    # A failed initialize/session/new still needs a durable
                    # owner identity if its bounded shutdown is unconfirmed.
                    self._record_process_identity(record, session_id, task_id, attempt_id, launch)
                except Exception:
                    pass
            exited = False
            try:
                exited = self._cleanup_failed_open(manager, reservation_id)
            except Exception:
                # Preserve the launch error and quarantine the owned resources.
                pass
            if exited and workspace_lock_acquired:
                workspace_lock.__exit__(None, None, None)
            with self._lock:
                self._opening.discard(attempt_id)
                if exited:
                    self._managers.pop(attempt_id, None)
                    self._reservations.pop(attempt_id, None)
                    self._workspace_locks.pop(attempt_id, None)
            if is_worker and manager is not None:
                self._record_dispatch_route_failure(error, record, route_model, task_id, attempt_id)
            raise
        with self._lock:
            self._managers[attempt_id] = manager
            self._sessions[attempt_id] = session_id
            self._opening.discard(attempt_id)
            self._session_to_attempt[session_id] = attempt_id
            self._attempt_task[attempt_id] = task_id
            if is_worker:
                self._attempt_routes[attempt_id] = (record, route_model)
            if reservation_id is not None:
                self._reservations[attempt_id] = reservation_id
            if workspace_lock is not None:
                self._workspace_locks[attempt_id] = workspace_lock
            replacement = attempt_id in self._ever_opened_attempts or bool(launch.get("recovery"))
            self._ever_opened_attempts.add(attempt_id)
        if self._store is not None:
            self._record_process_identity(record, session_id, task_id, attempt_id, launch)
            self._record_effective_context(record, task_id, attempt_id, launch)
            self._record_route_audit(record, task_id, attempt_id, launch)
            try:
                self._open_generation(
                    manager, record, session_id, task_id, attempt_id, launch,
                    instructions=instructions,
                    cause="session_replacement" if replacement else "session_open",
                )
            except BaseException as error:
                if is_worker:
                    self._record_dispatch_route_failure(error, record, route_model, task_id, attempt_id)
                # A revoked/mismatched channel must not leave a newly opened
                # engine or its skill snapshot live after binding fails.
                self.terminate_owned(attempt_id)
                raise
            self._bind_resource(manager, session_id, task_id, attempt_id, workspace_binding_id, workspace_root, launch)
            if not launch.get("assignment_id"):
                # EXT-01 #2: only the coordinator's own session ever
                # confirms/activates a task's configuration revision as a
                # whole (a worker inherits the task's route but never
                # re-confirms that same revision row here -- see
                # ``_apply_worker_mode_floor`` for the distinct, narrower
                # thing a worker's own open() *does* confirm: just its own
                # engine mode, T035). See ``reconcile_configuration``'s own
                # docstring: this is also the session's *first* turn boundary
                # (``acp.orchestrating_runtime.OrchestratingRuntime
                # ._ensure_coordinator`` calls the same method again on every
                # later turn, cached session or not).
                self.reconcile_configuration(task_id, session_id)
            if launch.get("assignment_id"):
                # T035 gap closure: a worker's own engine mode, unlike the
                # coordinator's, is never part of ``reconcile_configuration``
                # (see that method's own docstring -- it skips every worker
                # launch outright). Unconfirmed, "full working capability
                # except deletion, which must be confirmed" silently does not
                # bind (see ``configuration.policy.WORKER_MODE_FLOOR``'s own
                # docstring). A worker whose mode this cannot confirm must
                # never run under it -- see ``_apply_worker_mode_floor``'s own
                # docstring for why that means aborting this one attempt's
                # ``open()`` (never the whole task), with the just-opened
                # session/process torn down rather than leaked.
                try:
                    self._apply_worker_mode_floor(manager, task_id, session_id, launch, record)
                except WorkerModeNotConfirmed as error:
                    self.terminate_owned(attempt_id)
                    self._record_dispatch_route_failure(error, record, route_model, task_id, attempt_id)
                    raise
        try:
            self._check_not_cancelled(attempt_id, task_id)
        except QueuedOpenCancelled:
            self.terminate_owned(attempt_id)
            raise
        return session_id

    def _cleanup_failed_open(self, manager, reservation_id) -> bool:
        """EXT-01 #8: undo a partially-completed :meth:`open` -- capacity
        already reserved, and possibly a real ``EngineProcess`` already
        spawned by ``manager.negotiate``/``manager.create_session`` -- that
        failed before reaching :meth:`open`'s own atomic registration into
        ``self._managers``/``self._sessions``/``self._reservations``.
        Called from :meth:`open`'s own exception handler with the same
        local ``manager`` reference that call was about to register, so
        this never depends on that registration having happened (the exact
        gap the finding this fixes describes) -- this process is the only
        one that ever knew about the attempt, and it is still on the
        stack.

        Mirrors ``terminate_owned``'s own confirmed-vs-ambiguous split
        (see ``delegation.capacity``'s module docstring) rather than
        inventing a third state: a process that never actually spawned
        (``manager is None``, or ``manager._process is None`` because the
        failure happened before/during ``EngineSessionManager
        ._ensure_started``) provably never started, so its reservation is
        released outright the same as a confirmed exit. A process that did
        spawn is shut down (``EngineProcess.release``, the same call
        ``terminate_owned`` makes -- never a cross-process signal, since
        this process is the one that just launched it) and only released
        if that shutdown positively observes the exit; otherwise marked
        ``ambiguous`` -- still counted against the connection's limit,
        never silently dropped -- for the same reconciliation path an
        ordinary unconfirmed ``terminate_owned`` uses."""
        if manager is not None:
            manager.shutdown(grace=2.0)
        process = getattr(manager, "_process", None) if manager is not None else None
        exited = process is None or bool(process.wait_exited(0))
        if reservation_id is None or self._store is None:
            return exited
        from . import capacity as delegation_capacity

        if process is None:
            delegation_capacity.release_confirmed(self._store, reservation_id, "never_started")
        elif exited:
            delegation_capacity.release_confirmed(self._store, reservation_id, "process_exited")
        else:
            delegation_capacity.mark_ambiguous(self._store, reservation_id, "timeout_alive")
        self._notify_activity()
        return exited

    def _capture_catalog_observation(self, manager, record, session_id) -> None:
        """T035: parse this real session's own observed ``configOptions``
        (see ``EngineSessionManager.observed_config_options``) and persist
        it as a real, ``DISCOVERY``-provenance catalog snapshot for
        ``record`` -- exactly the same ``parse_observation``/
        ``build_catalog``/``catalog_store.save_snapshot`` path
        ``cortex_acp_setup.py record_observation`` already uses by hand,
        just triggered automatically by a real session opening. Best
        effort only: a missing/empty/malformed payload, or any failure
        here, is silently ignored -- the catalog simply keeps its previous
        (possibly seeded, see ``catalog.model_seed``) state until a valid
        observation is captured, and engine session creation itself must
        never fail because of this.
        """
        try:
            raw = manager.observed_config_options(session_id)
            if not raw:
                return
            from ..catalog import discovery as catalog_discovery
            from ..catalog import store as catalog_store

            # The per-model probe (one set_config_option per Claude model, a
            # ``codex debug models`` run for Codex) ran before every first
            # prompt and cost seconds per session. An identical observation
            # of the same install is already saved: skip it.
            fingerprint = _catalog_observation_fingerprint(record, getattr(manager, "_install", None), raw)
            if _catalog_observation_fresh(record.id, fingerprint, self._environ):
                return

            observation = catalog_discovery.parse_observation(raw)
            if record.engine == "opencode":
                from ..catalog.refresh import _opencode_cortex_mode_advertised

                if not _opencode_cortex_mode_advertised(observation):
                    return
            # ACP's initial configOptions is a connection-level snapshot.  In
            # Codex, the engine's effort selector is model-dependent, while
            # the initial payload can contain one shared fallback set.  The
            # prompt-free offline catalog is the only supported source that
            # supplies the exact per-model ceilings; apply it here too (the
            # normal refresh path already does this).  Without this merge a
            # real worker session overwrote an enriched snapshot with the
            # flat initial set, making Cortex's selector disagree with the
            # native Paseo/Zed selector after every task.
            if record.engine == "codex-acp":
                from ..catalog import refresh as catalog_refresh

                install = getattr(manager, "_install", None)
                if install is None:
                    install = catalog_refresh._resolve_install_for_refresh(record, self._environ)
                ceilings = catalog_refresh._codex_effort_ceiling(
                    record, install, self._environ, timeout=20.0,
                )
                if ceilings:
                    observation = observation.with_model_thought_levels(ceilings)
            if record.engine in ('claude-agent-acp', 'opencode'):
                # Both offer an effort selector only for some models, and a
                # different one per model (observed on opencode 2.0.19 too).
                from ..catalog.model_efforts import observe_model_efforts
                levels = observe_model_efforts(
                    manager._client.call, manager.correlation.wire_session_id(session_id), raw, timeout=20.0,
                )
                observation = observation.with_model_thought_levels(levels)
            snapshot = catalog_discovery.build_catalog({record.id: record}, {record.id: observation})
            catalog_store.save_snapshot(snapshot, self._environ)
            _remember_catalog_observation(record.id, fingerprint, self._environ)
        except Exception:
            pass

    def reconcile_configuration(self, task_id: str, session_ref: str):
        """EXT-01 finding #2 (per-session configuration confirmation).

        Replaces the pre-EXT-01 ``_maybe_apply_configuration`` (T031),
        which read *only* ``configuration_revisions.state`` and skipped
        entirely once it was already ``'active'`` -- correct for "has this
        task's configuration been decided and confirmed by *some* session
        ever", wrong for "is *this* concrete engine session, right now,
        actually running under it". A task-level ``'active'`` row is real,
        durable evidence that some session once confirmed it -- never
        evidence that this ``session_ref`` did. Three concrete gaps that
        follows from conflating the two (see the EXT-01 review):

        (a) a mid-task model change (``acp.agent._apply_mid_task_selection``)
            only ever proposes a fresh ``'selected'`` revision; nothing
            re-opens the coordinator's already-cached session, so the old
            state-only check never fired again for it. Closed by having
            this method run on *every* turn (see
            ``acp.orchestrating_runtime.OrchestratingRuntime
            ._ensure_coordinator``, which calls this every time it resolves
            a session -- cached, freshly opened, or recovered alike -- at
            that method's own safe per-turn boundary, never mid-turn), not
            only once at session-open time.
        (b) a new session after a restart/recovery: the *task's* revision
            may already be ``'active'`` (a prior, now-dead session
            confirmed it) while this brand-new session has never itself
            received a single ``session/set_config_option`` call and is
            silently running the engine's own defaults. Closed by
            ``self._session_configured_revision`` below: a per-*session*
            record, never read from or written into
            ``configuration_revisions`` (whose ``'active'`` state is
            immutable once reached -- see ``configuration_revisions_guard``
            -- precisely because it is cross-session task history, not any
            one session's own confirmation). It lives in memory on this
            adapter, not the durable store, because no ``session_ref`` this
            adapter ever hands out outlives this process: a restart or
            recovery always opens a genuinely new one (``open()``'s own
            ``launch['recovery']`` path), so the record for it always
            starts empty and this method always reconfirms, regardless of
            the task-level row's state.
        (c) a partial/indeterminate application: this method never marks a
            session confirmed except on a real ``'active'`` outcome: a
            benign "engine has no configuration surface" is recorded as
            confirmed (there is nothing to have configured), but a
            failed/rejected/partially-applied/unknown outcome returns a
            :class:`~..portable.errors.PortableError` instead -- the
            caller (``_ensure_coordinator``) raises it, blocking the turn,
            never proceeding on an unconfirmed combination. Reuses the
            existing ``failed``/``rejected``/``indeterminate`` vocabulary;
            no fourth state is added anywhere.

        Returns ``None`` when this session is (now) confirmed for the
        task's current revision, when that revision is still mid-flight
        (``'selected'``/``'pending_next_use'`` -- another call already has
        it in hand, or the very next reconciliation will), or when there is
        no store/session/current revision to reconcile at all (a fake/test
        double with no real configuration behind it, or no session live for
        ``session_ref`` -- unchanged pre-EXT-01 no-op behavior). Never
        raises itself -- ``open()`` calls this and discards the result (an
        already-opened session is not itself torn down over a refused
        configuration; see ``_open_coordinator_session``'s own comment for
        why *that* is ``_ensure_coordinator``'s job, using this same
        return value).
        """
        if self._store is None:
            return None
        with self._lock:
            attempt_id = self._session_to_attempt.get(session_ref)
            manager = self._managers.get(attempt_id) if attempt_id is not None else None
        if manager is None:
            return None
        from ..configuration import store as configuration_store
        from ..configuration.policy import BENIGN_CONFIGURATION_ERROR_CODES
        from ..configuration.service import ConfigurationRejected, ConfigurationService
        from ..portable.errors import PortableError, PortableErrorCategory
        from ..portable.identity import OwnershipEpoch

        service = ConfigurationService(self._store)
        # EXT-01 #2a: ``latest_live`` (never ``current_or_latest``, which
        # prefers an already-'active' revision even when a newer one has
        # since been proposed -- see that method's own docstring for why
        # that is correct for *its* one caller and wrong here) so a
        # mid-task selection change is actually the target this
        # reconciles a live session toward, not whatever was active before
        # it was ever proposed.
        current = service.latest_live(task_id)
        if current is None:
            return None
        revision_id = current["id"]
        with self._lock:
            already_confirmed = self._session_configured_revision.get(session_ref) == revision_id
        if already_confirmed and current["state"] == "active":
            return None
        state, error_code = current["state"], current.get("error_code")
        if state == "selected":
            with self._store.connection() as db:
                owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
            if owner is None:
                return None
            current_epoch = OwnershipEpoch(epoch=owner["epoch"], owner=task_id)
            adapter = _PortableRevisionConfigureAdapter(self._store, manager)
            try:
                service.apply(task_id, revision_id, adapter, session_ref, current_epoch)
            except PortableError:
                # stale_pending_apply (a newer revision was proposed
                # mid-flight) or a stale/mismatched owner epoch race -- the
                # next reconciliation (this same method, next turn) will
                # simply re-read whatever is current then; never treated as
                # a confirmed failure of *this* attempt.
                return None
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT state, error_code FROM configuration_revisions WHERE id=?", (revision_id,),
                ).fetchone()
            state, error_code = (row["state"], row["error_code"]) if row is not None else ("indeterminate", None)
        elif state == "active":
            # EXT-01 #2b: already active at the task level, but never
            # confirmed by *this* session -- confirm it directly over the
            # wire (never a second ``ConfigurationService.apply`` on an
            # already-'active' row: that requires 'selected' and would
            # simply reject it) using the identical confirm-or-fail
            # ``_PortableRevisionConfigureAdapter``/``EngineSessionManager
            # .configure`` gate ``_apply_worker_mode_floor`` already relies
            # on for the analogous worker-mode case. Only this in-memory
            # per-session record changes; the store's own 'active' row and
            # its original confirmation evidence are untouched.
            with self._store.connection() as db:
                stored = configuration_store.revision_row_tx(db, task_id, revision_id)
            revision = configuration_store._row_dict(stored)
            adapter = _PortableRevisionConfigureAdapter(self._store, manager)
            try:
                adapter.configure(session_ref, revision)
            except ConfigurationRejected as rejected:
                state, error_code = "failed", rejected.reason
            except Exception:
                state, error_code = "indeterminate", "apply_exception"
            else:
                state, error_code = "active", None
        if state == "active" or (state in ("failed", "rejected") and error_code in BENIGN_CONFIGURATION_ERROR_CODES):
            with self._lock:
                self._session_configured_revision[session_ref] = revision_id
            return None
        if state in ("selected", "pending_next_use"):
            # Another in-flight apply (a concurrent reconciliation, or this
            # one's own stale_pending_apply race above) already owns
            # settling this revision; the next reconciliation re-reads it.
            return None
        return PortableError(
            category=PortableErrorCategory.VALIDATION,
            code=error_code or "configuration_apply_failed",
            remediation="This task's configuration revision was refused by, or never confirmed by, the engine "
                        "for this session; select and apply a working configuration before starting a new turn.",
            retry_eligible=False,
        )

    def _apply_worker_mode_floor(self, manager, task_id, session_id, launch, record) -> None:
        """T035 gap closure: confirm -- not merely request -- this worker's
        own engine mode against ``configuration.policy.WORKER_MODE_FLOOR``
        (see that constant's own docstring for the pinned-build evidence
        and exactly why those two values, never any other offered mode).

        A no-op, exactly like ``_maybe_apply_configuration``'s own
        documented no-op cases, when: this ``launch`` carries no
        ``config_revision`` at all (a fake/test double with no real
        configuration behind it -- every existing worker-path test that
        never wires a real ``ConfigurationService`` stays unaffected); or
        ``route_engine`` names an engine this floor has no entry for (an
        unrecognized engine has no known-safe mode to confirm -- the same
        honest gap ``COORDINATOR_MODE_FLOOR`` already discloses).

        Deliberately does **not** call ``ConfigurationService.apply`` a
        second time on the *same* ``config_revision`` row: by the time a
        worker opens, that row is almost always already ``'active'`` (the
        coordinator's own open already confirmed it), and ``apply``'s own
        turn-gating (``_start_pending_next_use`` requiring a settled
        ``'selected'`` row) would simply reject it outright -- this is not
        the coordinator's own task-wide route/model confirmation, only one
        worker session's own mode. It instead reuses that same method's
        actual wire-level confirm-or-fail machinery directly: the identical
        ``_PortableRevisionConfigureAdapter`` (and therefore the identical
        ``EngineSessionManager.configure`` confirm-or-fail gate and
        ``confirmation_evidence_ref``-shaped evidence string ``apply``
        itself would have recorded) against an in-memory copy of the
        coordinator's own stored revision row with only ``parameters["mode"]``
        overridden -- never written back to ``configuration_revisions``,
        since a worker's own mode is not part of that task-wide audit
        trail.

        Checked directly against this worker session's own live
        ``session/new``-observed ``configOptions`` (ground truth already
        available -- unlike ``ConfigurationService.propose``'s coordinator-
        side floor check, which must fall back to a possibly-stale catalog
        snapshot because no session exists yet at that point).

        Raises :class:`WorkerModeNotConfirmed` -- never silently continues
        with an unverified mode -- when: the floor value is not among this
        session's own observed ``mode`` options at all (before or after its
        model is switched), the model switch itself is not confirmed, or the
        wire confirm-or-fail call itself rejects/only partially applies it. This
        is a different judgement call than ``COORDINATOR_MODE_FLOOR``'s own
        "must not run" (which blocks the entire task's configuration from
        ever becoming active): blocking *all* delegated work over one
        worker's mode confirmation failing would be its own outage, but
        silently letting a worker run in an unconfirmed mode is exactly the
        gap this closes -- so only *this one worker attempt* is refused
        (see ``open()``'s own caller, which tears down the just-opened
        session/process rather than leaking it); other attempts, and the
        coordinator itself, are unaffected.
        """
        config_revision = launch.get("config_revision")
        if not config_revision:
            return
        from ..configuration import access_mode as configuration_access_mode
        from ..configuration import store as configuration_store
        from ..configuration.policy import WORKER_MODE_FLOOR

        with self._store.connection() as db:
            row = configuration_store.revision_row_tx(db, task_id, config_revision)
            revision = configuration_store._row_dict(row)
        # EXT-01 (fallback/mixed-team source-vs-target confusion): the
        # engine mode floor -- and, below, the model this confirm-or-fail
        # call itself sends -- must be checked/applied against the route
        # this attempt was *actually opened against* (``record``, the same
        # already-resolved connection ``open()`` just launched, whether
        # that is the coordinator's own default route, an ordinary
        # mixed-team ``resolved_connection_id`` override, or a fallback
        # ``route_override``), never re-read fresh from
        # ``config_revision``'s own stored row -- which always names the
        # *source* route, not this attempt's target, whenever the two
        # differ.
        mode_floor = WORKER_MODE_FLOOR.get(record.engine)
        if mode_floor is None:
            return
        # The user's access mode (configuration.access_mode): in Auto the
        # worker is confirmed in the most permissive mode its session
        # offers instead of the floor -- still confirm-or-fail below.
        access = configuration_access_mode.current(self._store, task_id)
        mode_floor = configuration_access_mode.worker_mode(
            record.engine, _offered_values(manager.observed_config_options(session_id), "mode"),
            access=access, floor=mode_floor)
        if mode_floor not in _offered_values(manager.observed_config_options(session_id), "mode"):
            raise WorkerModeNotConfirmed(
                f"this worker's own engine session never offered mode {mode_floor!r} "
                f"(engine {record.engine!r}); its mode cannot be confirmed, "
                "so it must not run with an unverified permission-request boundary."
            )
        route_override = launch.get("route_override")
        resolved_model = (route_override or {}).get("model") or launch.get("resolved_model")
        revision = dict(revision, route_engine=record.engine, route_connection_id=record.id)
        adapter = _PortableRevisionConfigureAdapter(self._store, manager, route_model_override=resolved_model)
        # session/new describes the engine's default model, and which effort
        # values a session offers depends on its model: claude-agent-acp
        # drops its effort option entirely once the model is Haiku. Judged
        # against the default model, the effort below would be sent after
        # the model switch had already removed it, refusing the worker. So
        # the model is switched on its own first, and everything after it
        # judged against the options the switched session actually offers.
        target_model = resolved_model
        if target_model is None:
            from ..routing import store as routing_store

            extras = routing_store.get_route_extras(self._store, revision["id"])
            target_model = extras["route_model"] if extras is not None else None
        current_model = next(
            (entry.get("currentValue") for entry in manager.observed_config_options(session_id) or ()
             if isinstance(entry, Mapping) and entry.get("id") == "model"),
            None,
        )
        if target_model is not None and current_model != target_model:
            try:
                adapter.configure(session_id, dict(revision, parameters={}))
            except Exception as rejected:  # noqa: BLE001 - see the confirm call below
                raise WorkerModeNotConfirmed(
                    f"the engine did not confirm this worker's model {target_model!r}, "
                    f"so its mode {mode_floor!r} was never set: {rejected}"
                ) from rejected
        observed = manager.observed_config_options(session_id) or []
        mode_floor = configuration_access_mode.worker_mode(
            record.engine, _offered_values(observed, "mode"), access=access,
            floor=WORKER_MODE_FLOOR.get(record.engine))
        if mode_floor not in _offered_values(observed, "mode"):
            raise WorkerModeNotConfirmed(
                f"this worker's engine session stopped offering mode {mode_floor!r} once its model "
                f"was set to {target_model!r}; its mode cannot be confirmed."
            )
        # This worker's own parameters, not the coordinator's: its own
        # effort (else the coordinator's), carried by the same in-memory
        # revision copy as the mode floor, under whichever of the two
        # parameter names this revision already uses (see
        # configuration.resolution's own set), so nothing invents a key the
        # engine's option mapping does not know.
        effort_keys = ("thought_level", "effort")
        worker_parameters = {key: value for key, value in revision["parameters"].items() if key not in effort_keys}
        worker_parameters["mode"] = mode_floor
        launched_parameters = launch.get("parameters") or {}
        worker_effort = (launched_parameters.get("effort") or launched_parameters.get("thought_level")
                         or revision["parameters"].get("thought_level") or revision["parameters"].get("effort"))
        # Only an effort this session offers for its model now. The
        # configure call below is confirm-or-fail by design (an unconfirmed
        # mode must never run), so asking an engine for an option it does
        # not have would refuse the worker outright -- a policy about which
        # effort a role may use must narrow what it runs with, never stop
        # it running on an engine or model that has no such setting.
        offered_efforts = set().union(*(
            _offered_values(observed, option_id) for option_id in ("reasoning_effort", "thought_level", "effort")
        ))
        effort = _effort_within(worker_effort, offered_efforts)
        if effort is not None:
            key = "thought_level" if "thought_level" in revision["parameters"] else "effort"
            worker_parameters[key] = effort
        revision = dict(revision, parameters=worker_parameters)
        try:
            confirmation = adapter.configure(session_id, revision)
            if confirmation:
                # Dispatch audit is intent, not proof of the native settings
                # the engine confirmed (effort may be narrowed after opening).
                from .events import append_event

                append_event(
                    self._store, task_id, "progress",
                    f"worker-config:{launch['attempt_id']}:{session_id}:{confirmation}",
                    dict(kind="worker_configuration_confirmed", attempt_id=launch["attempt_id"],
                         engine_session_ref=session_id, config_revision_id=launch.get("config_revision"),
                         engine=record.engine, connection_id=record.id, model=target_model,
                         parameters={key: value for key, value in worker_parameters.items()
                                     if key in {"thought_level", "effort", "mode"}},
                         confirmation_evidence_ref=confirmation),
                    # The capsule's recipient carries this worker's own
                    # idempotency_key as "assignment_ref" (see
                    # ProvisioningResult.assignment_ref); "launch" never had
                    # an "assignment_ref" key (it has "assignment_id", the
                    # unrelated work-graph id), so this event used to be
                    # stored with assignment_ref=None and recovery.capsule's
                    # exact-match lookup by the real assignment_ref could
                    # never find it.
                    "engine_configuration", assignment_ref=launch.get("idempotency_key"),
                )
        except Exception as rejected:  # noqa: BLE001 - ConfigurationRejected,
            # EngineConfigurationPartiallyApplied, EngineConfigurationOutcomeUnknown
            # (a genuinely indeterminate wire outcome), and any other
            # unexpected failure of the same wire call all mean the same
            # thing here: this worker's mode was never actually confirmed,
            # so it must never run -- see this method's own docstring.
            raise WorkerModeNotConfirmed(
                f"the engine did not confirm worker mode {mode_floor!r}: {rejected}"
            ) from rejected

    def attempt_fallback(
        self, *, old_attempt_id: str, new_attempt_id: str, task_id: str, role: str, failure_class: str,
        target_connection_id: str, target_route_id: str, workspace_binding_id: str, launch: Mapping[str, object],
        instructions: str, target_model: str | None = None,
    ) -> str:
        """T027: the only production path that ever opens a *different*
        route/connection for the same task than the one its active
        configuration currently selects.

        Requires ``self._store`` (raises otherwise -- never authorizes a
        fallback with no durable state to check policy/consent/budget
        against). Authorizes strictly via
        ``routing.authorization.authorize_fallback`` against this task's
        *current* effective ceiling/route-profile before ever calling
        :meth:`open`; on any authorization failure this raises the
        resulting :class:`~..portable.errors.PortableError` and opens
        nothing. On success, reconciliation of ``old_attempt_id`` is the
        caller's own responsibility (see ``recovery.worker_reconciliation``/
        ``recovery.coordinator_recovery``) -- this method only ever
        dispatches the *new* attempt, recorded with
        ``decision='fallback'`` and ``predecessor_attempt_id=old_attempt_id``
        (see :meth:`_record_route_audit`).
        """
        if self._store is None:
            raise RuntimeError("attempt_fallback requires a real Store (self._store is None).")
        from ..configuration import store as configuration_store
        from ..configuration.policy import RouteProfile
        from ..connections import registry as connection_registry
        from ..routing import authorization as routing_authorization
        from ..routing import store as routing_store

        config_revision = launch.get("config_revision")
        if not config_revision:
            raise RuntimeError("attempt_fallback requires launch['config_revision'].")
        with self._store.connection() as db:
            row = db.execute("SELECT * FROM configuration_revisions WHERE id=?", (config_revision,)).fetchone()
        if row is None:
            raise RuntimeError("attempt_fallback: config_revision not found.")
        revision = configuration_store._row_dict(row)
        extras = routing_store.get_route_extras(self._store, config_revision)
        route_profile = extras["route_profile"] if extras is not None else RouteProfile(role_routes=None)
        state = connection_registry.read_registry(self._environ)
        current_connection = state.connections.get(revision["route_connection_id"])
        target_connection = state.connections.get(target_connection_id)
        if current_connection is None or target_connection is None:
            raise RuntimeError("attempt_fallback: current or target connection no longer exists.")
        routing_authorization.authorize_fallback(
            self._store, task_id=task_id, ceiling=revision["ceiling_snapshot"], failure_class=failure_class,
            role=role, route_profile=route_profile, coordinator_route_id=revision["route_id"],
            current_connection=current_connection, target_connection=target_connection, target_route_id=target_route_id,
        )
        fallback_launch = dict(launch)
        fallback_launch["attempt_id"] = new_attempt_id
        fallback_launch["route_override"] = dict(connection_id=target_connection_id, model=target_model)
        fallback_launch["fallback_of"] = old_attempt_id
        fallback_launch["fallback_failure_class"] = failure_class
        fallback_launch["resolved_route_id"] = target_route_id
        return self.open(workspace_binding_id, fallback_launch, instructions)

    def _record_process_identity(self, record, session_id, task_id, attempt_id, launch) -> None:
        """T023: durably record enough OS-level identity for this attempt's
        freshly-launched engine process that a *different* future adapter
        instance (a Cortex process restart) can tell "still running" apart
        from "exited" apart from "cannot tell" for a process it never
        itself launched (see ``engine.process_identity``/
        ``recovery.registry``) -- otherwise :meth:`lookup_owned` always
        answers "absent" after a restart regardless of the real OS state.
        Best-effort only: a manager with no real ``EngineProcess`` (a fake
        test double) or an unreadable ``/proc`` records nothing, and a
        later :meth:`lookup_owned` for that attempt then falls back to
        "unknown" rather than a guessed "alive"/"absent"."""
        from ..engine import process_identity
        from ..recovery import registry as recovery_registry

        manager = self._managers.get(attempt_id)
        process = getattr(manager, "_process", None)
        popen = getattr(process, "process", None)
        pid = getattr(popen, "pid", None)
        if pid is None:
            return
        identity = getattr(process, "_group_identity", None) or process_identity.capture(pid)
        if identity is None:
            return
        import hashlib

        digest = hashlib.sha256("\x00".join(record.launch_command or []).encode("utf-8")).hexdigest()
        role = "worker" if launch.get("assignment_id") else "coordinator"
        recovery_registry.record_process_launch(
            self._store, attempt_id=attempt_id, task_id=task_id, role=role,
            pid=identity.pid, pgid=identity.pgid, start_ticks=identity.start_ticks, boot_id=identity.boot_id,
            launch_command_digest=digest, connection_id=record.id, engine_session_ref=session_id,
        )

    def _record_effective_context(self, record, task_id, attempt_id, launch) -> None:
        """T023: durably record the configuration revision and
        :class:`~..engine.resource_policy.ResourcePolicy` this attempt was
        actually opened with -- ``recovery.coordinator_recovery`` reads
        this back to refuse a replacement that would silently broaden
        rights or switch engine/connection with no new, explicitly
        selected configuration revision authorizing it."""
        from ..engine.resource_policy import ResourcePolicy
        from ..recovery import registry as recovery_registry

        policy = self._manager_kwargs.get("policy") or ResourcePolicy()
        role = "worker" if launch.get("assignment_id") else "coordinator"
        config_revision = launch.get("config_revision")
        recovery_registry.record_effective_context(
            self._store, attempt_id=attempt_id, task_id=task_id, role=role,
            selected_config_revision_id=config_revision, effective_config_revision_id=config_revision,
            route_engine=record.engine, route_connection_id=record.id, resource_policy=policy,
        )

    def _launch_model(self, launch) -> str | None:
        """This attempt's model: a fallback's override, an auto/mixed-team
        resolution, else the configuration revision's own route model."""
        model = (launch.get("route_override") or {}).get("model") or launch.get("resolved_model")
        if model is None and launch.get("config_revision") and self._store is not None:
            from ..routing import store as routing_store

            extras = routing_store.get_route_extras(self._store, launch["config_revision"])
            if extras is not None:
                model = extras["route_model"]
        return model

    def _worker_unavailable_routes(self, launch) -> list[dict]:
        """Project route failures plus exact delivery failures for critical roles.

        Project-wide account failures keep their ordinary expiry. Instruction
        failures apply only to critical roles and the actual engine version,
        for this task's dispatch lifetime; installing a qualified version on
        the same connection does not inherit the refusal.
        """
        from ..connections import registry as connection_registry
        from ..engine.instruction_delivery_registry import has_exact_registration
        from ..portable.errors import PortableError
        from ..portable.roles import load_role
        from ..routing import availability

        try:
            critical = bool(load_role(launch.get("role_id") or "worker-general").critical_rules)
        except PortableError:
            # Role validity belongs to route authorization and instruction
            # loading, which report their own refusal; an unloadable role
            # is fenced conservatively here rather than refused early.
            critical = True
        entries = availability.active(self._store)
        state = connection_registry.read_registry(self._environ)
        records = dict(state.connections)
        # Profile inheritance can resolve a connection outside an automatic
        # catalog, but still needs the identical version/role fence.
        if not launch.get("route_override") and not launch.get("resolved_connection_id"):
            record = self._connection_record_factory(launch["task_id"])
            records.setdefault(record.id, record)
        current = {key: _pinned_version_for(record.engine, record) for key, record in records.items()}
        entries = [entry for entry in entries
                   if entry.get("reason") != "instruction_delivery_unqualified"
                   or (critical and entry.get("evidence") ==
                       f"{entry.get('engine')}@{current.get(entry.get('connection_id'))}")]
        if not critical:
            return entries
        with self._lock:
            refused = set(self._unqualified_instruction_routes)
        for record in records.values():
            version = current[record.id]
            if ((launch["task_id"], record.engine, version) in refused
                    or (record.engine == "claude-agent-acp"
                        and not has_exact_registration(record.engine, version))):
                entries.append(dict(
                    engine=record.engine, connection_id=record.id, model=None, scope="connection",
                    reason="instruction_delivery_unqualified", evidence=f"{record.engine}@{version}",
                    expires_at="engine version qualification required",
                ))
        return entries

    def record_instruction_delivery_failure(self, session_ref: str, error: BaseException) -> None:
        """Retain a non-retryable generation-delivery failure before it is reported."""
        from ..portable.errors import PortableError

        if not (isinstance(error, PortableError)
                and error.code == "instruction_delivery_unqualified" and not error.retry_eligible):
            return
        with self._lock:
            meta = self._session_meta.get(session_ref)
            route = self._attempt_routes.get(meta["attempt_id"]) if meta is not None else None
        if meta is not None and meta["recipient"] == "worker" and route is not None:
            self._record_dispatch_route_failure(
                error, route[0], route[1], meta["task_id"], meta["attempt_id"],
            )

    def _refuse_unavailable_route(self, record, model, unavailable) -> None:
        """A worker is never opened on a route ``routing.availability``
        still records as unavailable -- whichever way the route was fixed
        (the coordinator's own route, a mixed-team profile, a fallback)."""
        from ..portable.errors import PortableError, PortableErrorCategory
        from ..routing import availability

        blocked = availability.blocking(unavailable, record.id, model)
        if blocked is not None:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="no_available_route",
                remediation=(f"The route this worker would open on is unavailable ({availability.describe(blocked)}) "
                             "and no other route was chosen for it; wait for it, connect another account, "
                             "or change routing.json."),
                retry_eligible=False,
            )

    def _record_dispatch_route_failure(self, error, record, model, task_id, attempt_id) -> None:
        """Record the route as unavailable when opening a worker on it
        failed for a reason about the route: the engine would not start or
        answer (``engine_start_failed``, the whole connection), would not
        confirm the worker's configuration (``configuration_not_confirmed``,
        this model), or refused it with a structured quota/rate-limit/model
        error. Anything else -- a cancellation, a policy refusal, a
        workspace or channel problem -- records nothing. Never raises."""
        from ..engine import failure_classification
        from ..engine.client import EngineProtocolError, EngineTimeout, EngineTransportClosed

        if attempt_id in self._cancelled_attempts or task_id in self._cancelled_tasks:
            return
        from ..portable.errors import PortableError

        if (isinstance(error, PortableError) and error.code == "instruction_delivery_unqualified"
                and not error.retry_eligible):
            version = _pinned_version_for(record.engine, record)
            with self._lock:
                self._unqualified_instruction_routes.add((task_id, record.engine, version))
            failure = ("instruction_delivery_unqualified", "connection", f"{record.engine}@{version}")
        elif isinstance(error, WorkerModeNotConfirmed):
            failure = ("configuration_not_confirmed", "model", "WorkerModeNotConfirmed")
        elif isinstance(error, EngineProtocolError):
            failure = failure_classification.route_failure(error)
        elif isinstance(error, (EngineTransportClosed, EngineTimeout, OSError)):
            failure = ("engine_start_failed", "connection", type(error).__name__)
        else:
            failure = None
        if failure is not None:
            self._record_route_failure(failure, record, model, task_id, attempt_id)

    def _record_route_failure(self, failure, record, model, task_id, attempt_id) -> None:
        from ..connections import quota
        from ..routing import availability, auto_route

        reason, scope, evidence = failure
        try:
            resets = None
            if reason in (availability.QUOTA_EXHAUSTED, availability.RATE_LIMITED):
                try:
                    root = (self._project_root_factory(task_id) if self._project_root_factory is not None
                            else self._workspace_root_factory(task_id))
                    rules = auto_route.load_overrides(root, self._environ)
                    resets = quota.resets_at(record, model, self._environ, routing=rules)
                except Exception:
                    # Invalid settings cannot erase a real engine refusal.
                    # Retain it with availability's bounded default expiry.
                    pass
            availability.record(
                self._store, task_id, engine=record.engine, connection_id=record.id, model=model,
                reason=reason, scope=scope, evidence=evidence, attempt_id=attempt_id, resets_at=resets,
            )
        except Exception:  # noqa: BLE001 - the original failure is what the caller reports
            pass

    def _resolve_runtime_route(self, launch, project_root, unavailable=()) -> None:
        """Choose this worker's connection, model and effort automatically
        (``routing.auto_route``): the role's tier, the models the engines
        actually advertised, how much of each account's plan is left, and
        which routes ``routing.availability`` records as unavailable.
        An already-resolved route (mixed-team profile, fallback) wins."""
        if not launch.get('assignment_id') or launch.get('route_override') or launch.get('resolved_connection_id'):
            return
        role = launch.get('role_id')
        if not role:
            return
        from ..routing import auto_route

        from ..orchestration.state import attempt_routing, review_provider_preference

        route = (attempt_routing(self._store, launch['task_id'], launch['attempt_id'])
                 if launch.get('task_id') and launch.get('attempt_id') else None)

        configuration = self._dispatch_configuration(launch)
        eligible_ids = None
        if configuration is not None:
            revision, profile = configuration
            if profile.role_routes is not None:
                role_routes = profile.allowed_routes_for(role, revision["route_id"])
                if not role_routes:
                    raise self._route_authorization_error("role_route_not_authorized")
                if role_routes == {revision["route_id"]} and not (route and route.user_request):
                    # An explicit coordinator-only role is ordinary profile
                    # inheritance, not an automatic alternate selection.
                    from ..routing import store as routing_store
                    extras = routing_store.get_route_extras(self._store, revision["id"])
                    launch["resolved_route_id"] = revision["route_id"]
                    launch["resolved_connection_id"] = revision["route_connection_id"]
                    launch["resolved_model"] = extras["route_model"]
                    launch["route_basis"] = "profile"
                    return
            eligible_ids = frozenset(entry.route.id for entry in
                                     self._automatic_route_entries(revision, profile, role))

        escalate = route.escalate if route else False
        complexity = route.complexity if route else 'normal'
        model_hint = route.model_hint if route else 'auto'
        model_reason = route.model_reason if route else None
        previous_engine = route.previous_engine if route else None
        user_request = route.user_request if route else None
        user_revocation = route.user_revocation if route else None
        provider = None
        review_reason = None
        if role == "worker-code-reviewer" and launch.get('task_id') and launch.get('attempt_id'):
            provider, review_reason = review_provider_preference(
                self._store, launch["task_id"], launch["attempt_id"],
            )
        # The choice depends only on the work and each plan's current usage,
        # never on how many attempts an engine received before.
        choice = auto_route.choose(
            role, project_root, self._environ, escalate=escalate, complexity=complexity,
            unavailable=unavailable, preference=provider, model_hint=model_hint,
            model_reason=model_reason, previous_engine=previous_engine, user_request=user_request,
            eligible_route_ids=eligible_ids, user_revocation=user_revocation,
            work_kind_override=route.work_kind if route else None,
        )
        if choice is None:
            if configuration is not None:
                raise self._route_authorization_error("automatic_route_identity_unavailable")
            return
        if review_reason and choice.decision is not None:
            choice.decision["cross_provider_review"] = review_reason
        launch["resolved_connection_id"] = choice.connection_id
        launch["resolved_model"] = choice.model
        launch["resolved_route_id"] = choice.route_id
        # What authorized this route, recorded with the attempt. The task's
        # own route profile did not: absent an explicit mixed-team profile
        # it authorizes only the coordinator's route.
        launch["route_basis"] = choice.basis
        if choice.decision:
            launch["route_decision"] = dict(choice.decision)
        if choice.effort:
            launch["parameters"] = dict(launch.get("parameters") or {}, effort=choice.effort)
            launch["effort_source"] = "auto_route"

    @staticmethod
    def _route_authorization_error(code):
        from ..portable.errors import PortableError, PortableErrorCategory
        return PortableError(category=PortableErrorCategory.AUTHORIZATION, code=code,
                             remediation="The actual dispatch route must match one current observed identity within this task's profile/ceiling.",
                             retry_eligible=False)

    def _dispatch_configuration(self, launch, task_id=None):
        from ..configuration import store as configuration_store
        from ..configuration.policy import RouteProfile
        from ..routing import store as routing_store
        revision_id = launch.get("config_revision")
        if not revision_id:
            return None
        with self._store.connection() as db:
            row = db.execute("SELECT * FROM configuration_revisions WHERE id=?", (revision_id,)).fetchone()
        if row is None or row["task_id"] != (task_id if task_id is not None else launch.get("task_id")):
            raise self._route_authorization_error("dispatch_configuration_unavailable")
        extras = routing_store.get_route_extras(self._store, revision_id)
        return configuration_store._row_dict(row), extras["route_profile"] if extras else RouteProfile()

    def _automatic_route_entries(self, revision, profile, role):
        from ..connections import registry as connection_registry
        from ..routing import auto_route, authorization
        connections = connection_registry.read_registry(self._environ).connections
        ceiling = revision["ceiling_snapshot"]
        # Only automatic routing may use the owner settings' mixed-engine
        # candidates when no explicit task profile was supplied.
        allowed = (authorization.allowed_routes_for_role(profile, ceiling, role, revision["route_id"])
                   if profile.role_routes is not None else ceiling.allowed_routes)
        return [entry for entry in auto_route._entries({}, connections, self._environ)
                if not auto_route._is_seed(entry)
                and (allowed is None or entry.route.id in allowed)
                and (ceiling.allowed_connections is None or entry.connection_id in ceiling.allowed_connections)
                and (ceiling.allowed_data_recipients is None or
                     connections[entry.connection_id].provider in ceiling.allowed_data_recipients)]

    def _authorize_dispatch(self, record, task_id, launch, project_root=None) -> None:
        """T027: refuse *before* any capacity reservation/process launch
        when this attempt's role is not authorized for the route it is
        about to use, or this task has already exhausted a tracked
        ``PolicyCeiling.limits['attempts']`` budget. A no-op when this
        launch carries no ``config_revision`` (a fake/test double with no
        real configuration behind it) -- never a substitute for the real
        checks in ``configuration.service.ConfigurationService.propose``/
        ``resolve_worker``, which already ran before this launch was ever
        constructed.

        Once the route itself is authorized, a model the user listed under
        ``deny`` in ``routing.json`` (see ``routing.auto_route``) is still
        refused -- checked strictly *after* the route/ceiling authorization,
        so it can only ever narrow further.
        """
        config_revision = launch.get("config_revision")
        if not config_revision:
            return
        from ..configuration.policy import RouteProfile
        from ..routing import authorization as routing_authorization
        from ..routing import store as routing_store

        revision, route_profile = self._dispatch_configuration(launch, task_id)
        extras = routing_store.get_route_extras(self._store, config_revision)
        # T027 review fix (HIGH): the real, resolved role id (e.g.
        # "reviewer") when the caller threaded one through
        # (``delegation.provisioning``'s launch, or a fallback launch --
        # see ``attempt_fallback``/``_attempt_worker_route_fallback``),
        # never just the coarse "worker"/"coordinator" bucket -- a profile
        # that names (and denies) a specific role must actually bind that
        # role's own name, or a denied role would silently fall through to
        # RouteProfile's generic-bucket default and dispatch anyway.
        role = launch.get("role_id") or ("worker" if launch.get("assignment_id") else "coordinator")
        # A fallback attempt (route_override present) is authorized
        # up-front by the caller via ``routing.authorization
        # .authorize_fallback`` -- never re-derived/re-checked here against
        # the *original* route's profile, which would incorrectly forbid
        # the very route change fallback just authorized.
        route_override = launch.get("route_override")
        ceiling = revision["ceiling_snapshot"]
        if ((ceiling.allowed_connections is not None and record.id not in ceiling.allowed_connections) or
                (ceiling.allowed_data_recipients is not None and record.provider not in ceiling.allowed_data_recipients)):
            raise self._route_authorization_error("role_route_not_authorized")
        if route_override is None:
            candidate_route_id = launch.get("resolved_route_id") or revision["route_id"]
            if launch.get("route_basis") in ("auto", "auto:pin", "auto:user_request"):
                entries = self._automatic_route_entries(revision, route_profile, role)
                matches = [entry for entry in entries if entry.connection_id == record.id
                           and entry.route.engine == record.engine and entry.route.model == launch.get("resolved_model")]
                if len(matches) != 1 or matches[0].route.id != launch.get("resolved_route_id"):
                    raise self._route_authorization_error("automatic_route_identity_unavailable")
                if route_profile.role_routes is None:
                    route_profile = RouteProfile(role_routes={role: {matches[0].route.id}})
            routing_authorization.authorize_dispatch(
                self._store, task_id=task_id, role=role, route_profile=route_profile,
                coordinator_route_id=revision["route_id"], candidate_route_id=candidate_route_id,
                ceiling=revision["ceiling_snapshot"],
            )
        model = (route_override or {}).get("model") or launch.get("resolved_model")
        if model is None and extras is not None:
            model = extras["route_model"]
        parameters = launch.get("parameters") or {}
        requested_effort = parameters.get("effort") or parameters.get("thought_level")
        if requested_effort is None:
            requested_effort = revision["parameters"].get("effort") or revision["parameters"].get("thought_level")
        from ..routing import auto_route

        auto_route.refuse_if_denied(record.engine, model, project_root, self._environ)
        if launch.get("assignment_id"):
            from ..portable.errors import PortableError, PortableErrorCategory

            rules = auto_route.load_overrides(project_root, self._environ)
            if rules.get("providers") is not None and record.engine not in rules["providers"]:
                raise PortableError(category=PortableErrorCategory.VALIDATION, code="route_provider_excluded",
                                    remediation="The selected worker/reviewer engine is excluded by current settings.",
                                    retry_eligible=False)
        # The tier effort rule is for automatic worker routing; the
        # coordinator runs at the effort chosen in the editor.
        allowed = (auto_route.allowed_efforts(record.engine, model, project_root, self._environ)
                   if launch.get("assignment_id") else None)
        if requested_effort is not None and allowed is not None and requested_effort not in allowed:
            # Lowered, never refused: a saved effort from before the rule
            # (e.g. ultra) must not make the thread unusable. Recorded as
            # this attempt's effective effort, with its provenance.
            clamped = auto_route.nearest(requested_effort, allowed)
            if clamped is not None:
                launch["parameters"] = dict(parameters, effort=clamped)
                launch["effort_source"] = "clamped_from_" + requested_effort
        if requested_effort is not None:
            launch.setdefault("effort_source", "configuration_or_request")

    def _record_route_audit(self, record, task_id, attempt_id, launch) -> None:
        """T027: durably audit the actual engine/connection/model/params
        fingerprint/config-revision every attempt (coordinator, worker, or
        fallback) was dispatched with -- see ``routing.store
        .record_attempt_audit``. A fallback attempt (``route_override``
        present) is audited with ``decision='fallback'`` and its
        predecessor attempt id; every other attempt is audited
        ``'authorized'``."""
        from ..routing import authorization as routing_authorization
        from ..routing import store as routing_store

        config_revision = launch.get("config_revision")
        role = launch.get("role_id") or ("worker" if launch.get("assignment_id") else "coordinator")
        route_override = launch.get("route_override")
        # EXT-01: a fallback's own ``route_override["model"]`` and an
        # ordinary mixed-team attempt's own ``resolved_model`` both name
        # this attempt's actual target model; only when neither is present
        # (the coordinator's own route, unoverridden) does the source
        # revision's own stored ``route_model`` -- correct for that case,
        # since source and target are the same route -- apply.
        model = self._launch_model(launch) or "unknown"
        fingerprint = routing_authorization.fingerprint_params(launch.get("parameters") or {})
        is_fallback = launch.get("route_override") is not None
        detail = dict(role=role, route_connection_id=record.id)
        route_basis = launch.get("route_basis")
        if route_basis:
            detail["route_basis"] = route_basis
        # Why automatic routing chose this route among the tier's
        # candidates (``auto_route.rank``): tier, ranked candidates with
        # their plan usage/affinity, cross-tier stand-ins, what decided it, and
        # the coordinator's provider preference.
        if launch.get("route_decision"):
            detail["route_decision"] = launch["route_decision"]
        # T035: this attempt's real, engine-observed model name (never a
        # guessed version) alongside the routing id already recorded in
        # ``model`` above -- see ``_resolve_route_model_name``. Absent when
        # this connection's own cached catalog snapshot has no matching
        # entry, or that entry gives no evidence more concrete than the
        # bare alias/id itself: every reader must then show the alias
        # explicitly marked as an alias, never fabricate a name.
        model_name = _resolve_route_model_name(record.id, model, self._environ)
        if model_name:
            detail["model_name"] = model_name
        # The effort this attempt actually ran with. It exists nowhere else
        # in the store: ``configuration_revisions`` holds the coordinator's
        # parameters, and this attempt's own value lives only in ``launch``
        # (applied to the worker by ``_apply_worker_mode_floor``). Without
        # it the card naming a worker's route could say provider and model
        # but never the third thing the user asked to see.
        attempt_parameters = launch.get("parameters") or {}
        effort = attempt_parameters.get("effort") or attempt_parameters.get("thought_level")
        if effort:
            detail["effort"] = effort
            detail["effort_source"] = launch.get("effort_source") or "launch_parameters"
        if is_fallback:
            detail["failure_class"] = launch.get("fallback_failure_class")
        routing_store.record_attempt_audit(
            self._store, attempt_id=attempt_id, task_id=task_id, role=role, engine=record.engine,
            connection_id=record.id, model=model, params_fingerprint=fingerprint,
            config_revision_id=config_revision, decision="fallback" if is_fallback else "authorized",
            detail=detail, predecessor_attempt_id=launch.get("fallback_of"),
        )

    def _launch_model(self, launch) -> str | None:
        """The route model this attempt actually targets: a fallback's own
        ``route_override["model"]``, a mixed-team attempt's
        ``resolved_model``, else the coordinator revision's stored
        ``route_model``; ``None`` when none is recorded."""
        from ..routing import store as routing_store

        model = (launch.get("route_override") or {}).get("model") or launch.get("resolved_model")
        config_revision = launch.get("config_revision")
        if model is None and config_revision and self._store is not None:
            extras = routing_store.get_route_extras(self._store, config_revision)
            if extras is not None:
                model = extras["route_model"]
        return model

    def _open_generation(self, manager, record, session_id, task_id, attempt_id, launch, *, instructions: str, cause: str) -> None:
        from . import generations as delegation_generations

        recipient = "worker" if launch.get("assignment_id") else "coordinator"
        # NOTE: ``launch["assignment_id"]`` is ``delegation_provisioning.
        # assignment_id`` (a distinct column from its primary key). Every
        # other consumer of a generation's own recorded ``assignment_ref``
        # (``recovery.capsule``'s ``recipient["assignment_ref"]``,
        # ``task_events.assignment_ref``, ...) means the provisioning row's
        # ``idempotency_key`` instead (see
        # ``acp.orchestrating_runtime.DelegateResult.assignment_ref`` /
        # ``ProvisioningService.delegate``) -- resolved here by
        # ``attempt_id`` so the generation's own ``assignment_ref`` column
        # actually matches what a later capsule build looks up.
        assignment_ref = None
        if recipient == "worker":
            with self._store.connection() as db:
                row = db.execute(
                    "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (attempt_id,),
                ).fetchone()
            assignment_ref = row["idempotency_key"] if row is not None else None
        # Same documented pinned-version convention as
        # ``ConfigurationServiceSnapshotSource.snapshot`` (T016/T026, see
        # ``_PINNED_ENGINE_VERSION`` above): no real installed-version
        # discovery is wired into ``ConnectionRecord`` yet, so each pinned,
        # evidence-qualified engine resolves to its own exact pinned
        # version and any other engine resolves to the wildcard tier entry
        # (see ``delegation.generations.tier_for``'s own fallback -- an
        # engine with no wildcard evidence still correctly falls to
        # ``controlled_boundary``/``unqualified``, never a guessed tier).
        version = _pinned_version_for(record.engine, record)
        tier = delegation_generations.ensure_owner(
            self._store, task_id=task_id, engine_session_ref=session_id, engine=record.engine, version=version,
        )
        generation = delegation_generations.open_generation(
            self._store, task_id=task_id, engine_session_ref=session_id, recipient=recipient,
            cause=cause, evidence_source=f"engine_session_manager:{cause}", assignment_ref=assignment_ref,
        )
        from ..portable.instruction_bundle import (
            InstructionBundleLedger, InstructionSource, capture_instruction_bundle,
        )

        role_id = str(launch.get("role_id") or ("orchestrator" if recipient == "coordinator" else "worker-general"))
        from ..portable.roles import load_role

        # The canonical role is an independently versioned source.  The
        # assembled text below records exactly what this adapter selected;
        # engine- or client-loaded instructions outside Cortex are not
        # observable through this ledger.
        role = load_role(role_id)
        bundle = capture_instruction_bundle(
            task_id=task_id, assignment_ref=assignment_ref or "task", attempt_id=attempt_id,
            role_id=role_id, role_kind=recipient, engine=record.engine, engine_version=version,
            assembled_text=instructions,
            sources=(
                InstructionSource.from_content(f"role:{role.role_id}", "canonical_role",
                                               role.body_portable, version=role.sha256,
                                               provenance=role.source_path),
                InstructionSource.from_content("assembled_role_instructions", "adapter_assembly",
                                               instructions, provenance="runtime_role_assembly"),
                InstructionSource.from_content("context_generation", "runtime_generation", generation["id"],
                                               version=generation["id"], provenance="context_generations"),
            ),
        )
        ledger = InstructionBundleLedger(self._store)
        ledger.save_intended(bundle)
        # Record what the manager really sent: it may append to this meta
        # (the Claude note naming attached external MCP servers).
        sent = getattr(manager, "sent_session_new_meta", None)
        meta = sent(session_id) if sent is not None else None
        launched = getattr(manager, "sent_engine_instructions", None)
        launched_prompt = launched() if launched is not None else None
        if meta is None and isinstance(launched_prompt, str):
            # OpenCode: what the model receives is the agent prompt its
            # process started with (no session/new _meta at all).
            meta = {"opencodeAgentPrompt": launched_prompt}
        # A reconstructed payload is useful for the instruction ledger, but
        # cannot authorize paths unless the manager observed sending it.
        skill_meta = meta
        if meta is None:
            meta = _session_new_meta_for(record.engine, version, instructions, role=recipient,
                                         project_root=getattr(manager, "_workspace_root", None),
                                         effort=_sdk_effort_for(record, launch), environ=self._environ,
                                         role_id=launch.get("role_id"),
                                         stage=self._assignment_stage(task_id, attempt_id))
        if meta is not None:
            import json

            ledger.record_sent(bundle, json.dumps(meta, sort_keys=True, ensure_ascii=False),
                               engine=record.engine, engine_version=version,
                               attempt_id=attempt_id, session_ref=session_id)
            try:
                from ..engine.unified_system_prompt import sent_prompt_edition
                from ..observability import events as observability_events

                # The edition is read back from the sent text, so a native
                # (non-unified) session records none.
                edition = sent_prompt_edition(meta)
                # Aggregate-only, secret-free: catalog ids this bundle's
                # role would currently fail to resolve through `read_skill`
                # (see `instruction_bundle.catalog_resolution_gaps`). Purely
                # advisory -- it never blocked and never blocks this
                # session; the required-inlined-skill guard in
                # `portable.roles.composed_instructions` already handles
                # the blocking case for the always-included skills.
                unresolved = bundle.unresolved_catalog_entries
                detail = {}
                if edition is not None:
                    detail["prompt_edition"] = edition
                if unresolved:
                    detail["unresolved_catalog_count"] = len(unresolved)
                    detail["unresolved_catalog_ids"] = sorted(unresolved)
                observability_events.append_event(
                    self._store, task_id, "base_prompt", base_prompt_mode(meta), "observed",
                    attempt_id=attempt_id, engine_session_ref=session_id, route_engine=record.engine,
                    detail=detail or None,
                )
            except Exception:
                pass
        from .session_skills import bind_session_skill_generation, record_session_skills

        record_session_skills(
            self._store, task_id=task_id, attempt_id=attempt_id, session_ref=session_id,
            project_root=getattr(manager, "_workspace_root", None), environ=self._environ,
            sent_meta=skill_meta,
        )
        if launch.get("channel_id"):
            bind_session_skill_generation(
                self._store, channel_id=launch["channel_id"], task_id=task_id,
                attempt_id=attempt_id, generation_id=generation["id"], session_ref=session_id,
            )
        with self._lock:
            self._session_meta[session_id] = dict(
                task_id=task_id, attempt_id=attempt_id, engine=record.engine, version=version,
                recipient=recipient, assignment_ref=assignment_ref, tier=tier.tier,
                instructions=instructions, connection_id=record.id, instruction_bundle_id=bundle.bundle_id,
                model=self._launch_model(launch),
                channel_id=launch.get("channel_id"),
            )
            self._pending_generation[session_id] = generation

    def _bind_resource(
        self, manager, session_id, task_id, attempt_id, workspace_binding_id, workspace_root, launch,
    ) -> None:
        """T035 gap closure: register this session's own
        ``engine.resource_binding.ResourceBinding`` so
        ``engine.permissions.PermissionBridge`` can classify anything at
        all -- previously nothing in production ever called
        ``EngineSessionManager.bind_resource`` (see
        ``resource_binding``'s own module docstring), so every permission
        request failed closed to ``cancelled`` regardless of role or
        classification, silently disabling the "workers keep full
        capability except deletion" rule's auto-approval half in
        production.

        Store-gated (same convention every other T022/T023/T027 feature in
        this method already follows): a caller with no ``store`` -- a
        bespoke test double with no durable state to read
        ``context_generation_id``/``ownership_epoch`` from -- gets no
        binding here and keeps failing closed exactly as before this fix;
        this never weakens the fail-closed default, it only activates the
        auto-approval half for real production sessions, which always
        supply a store (see ``acp.production_runtime``).

        ``role`` is never defaulted or inferred from a shape: it is the
        same ``"worker" if launch.get("assignment_id") else "coordinator"``
        rule every other role computation in this file already uses (see
        ``_record_process_identity``/``_record_effective_context``/
        ``_open_generation``), so a coordinator can never be misclassified
        as a worker and auto-approved.
        """
        import os

        from .. import ownership
        from ..engine.resource_binding import ResourceBinding

        with self._lock:
            generation = self._pending_generation.get(session_id)
        if generation is None:
            return
        with self._store.connection() as db:
            epoch = ownership.current_owner_epoch(db, task_id)
        if epoch is None:
            return
        role = "worker" if launch.get("assignment_id") else "coordinator"
        binding = ResourceBinding(
            engine_instance_id=manager._process.engine_instance_id,
            engine_session_id=session_id,
            context_generation_id=generation["id"],
            assignment_id=launch.get("assignment_id") or attempt_id,
            attempt_id=attempt_id,
            workspace_binding_id=workspace_binding_id,
            workspace_root=os.path.realpath(workspace_root),
            ownership_epoch=str(epoch),
            role=role,
        )
        manager.bind_resource(binding)

    def _persist_terminal_output(
        self, engine_session_id: str, terminal_id: str, data: bytes, *,
        truncated: bool, exit_code: int | None, signal: str | None, failure: str | None,
    ) -> str | None:
        """T024: real ``engine.terminal_bridge.TerminalBridge`` persist sink
        binding -- looks up this exact session's already-recorded
        ``task_id``/``recipient``/``assignment_ref`` (see
        ``_open_generation``, populated before any real terminal can be
        created) and persists via ``recovery.large_output_store``, scoped
        by owner exactly like a capsule recipient (``"coordinator"`` or the
        worker's own ``assignment_ref`` -- never ambiguous, never another
        assignment's scope). Best-effort: returns ``None`` (no
        ``large_output_id`` surfaced) rather than raising, if this session's
        meta or the store is unavailable."""
        if self._store is None:
            return None
        with self._lock:
            meta = self._session_meta.get(engine_session_id)
        if meta is None:
            return None
        owner_scope = "coordinator" if meta["recipient"] == "coordinator" else meta.get("assignment_ref")
        if not owner_scope:
            return None
        from ..recovery import large_output_store

        record = large_output_store.persist_output(
            self._store, task_id=meta["task_id"], owner_scope=owner_scope, source_kind="terminal",
            data=data, source_ref=terminal_id, exit_code=exit_code, signal=signal,
            failure=failure, truncated=truncated,
        )
        return record["id"]



    def _notify_coordinator_refusal(self, engine_session_id: str, reason: str) -> None:
        """T035 FINDING-12 closure: ``engine.permissions.PermissionBridge``'s
        ``coordinator_refusal_notifier`` seam. ACP's ``RequestPermissionResponse``
        has no text field (see that module's own docstring), so this is the
        only way the refusal reason ever reaches the coordinator model at
        all: appended as an ordinary ``"progress"`` task event (the exact
        same idempotent, informative-only shape ``acp.orchestrating_runtime
        ._emit_localized`` already uses for every other runtime-authored
        status line), which ``_render_continuation_text`` renders as
        ``"Progress: <note>"`` the next time this task's coordinator session
        is prompted. Role refusals carry a meaningful kind so the
        orchestration loop starts that continuation in the same host turn;
        other informational refusal notices remain non-meaningful. Nothing
        is injected synchronously into the in-flight ACP permission reply.
        Best-effort, same as
        :meth:`_persist_terminal_output`: a session with no recorded meta
        (or no store at all) is silently a no-op rather than raising into
        the permission bridge, which must never let a notifier failure
        change the refusal it already decided."""
        if self._store is None:
            return
        with self._lock:
            meta = self._session_meta.get(engine_session_id)
        if meta is None:
            return
        from . import events as delegation_events
        from ..portable.identity import new_id
        from ..engine.permissions import (
            COORDINATOR_EDIT_REFUSAL_REASON,
            COORDINATOR_PRIVATE_OR_CHECK_REFUSAL_REASON,
        )

        delegation_events.append_event(
            self._store, meta["task_id"], "progress", f"progress:{new_id('p_')}",
            dict(
                note=reason,
                kind=(
                    "coordinator_role_refusal"
                    if reason in (
                        COORDINATOR_EDIT_REFUSAL_REASON,
                        COORDINATOR_PRIVATE_OR_CHECK_REFUSAL_REASON,
                    ) else None
                ),
                message_key="coordinator_mutation_refused", message_params={},
            ),
            origin="runtime",
        )

    def _reserve_capacity_blocking(
        self, connection_id: str, task_id: str, attempt_id: str, launch: Mapping[str, object],
        *, limit: int | None = None,
    ) -> str:
        """Block (bounded backoff poll, never a model prompt) until a
        capacity unit is reserved. Deliberately *inside* ``open()`` -- below
        ``delegation.provisioning``'s idempotency-key claim/steal protocol
        -- so a queued delegation is indistinguishable from "the engine is
        just taking a while to launch", never interacting with that
        protocol's own contention recovery (see
        ``acp.orchestrating_runtime.OrchestratingRuntime._provision_and_prompt``'s
        docstring for why retrying ``delegate()`` itself would be unsafe).

        ``limit`` is the resolved connection's own ``max_concurrent_sessions``
        (:func:`delegation.capacity.limit_for_connection`, which :meth:`open`
        passes); when omitted, only this adapter's explicit ``capacity_limit``
        or the default applies. A worker also waits while its session
        already runs ``MAX_WORKERS_PER_SESSION`` workers on any connection.
        A coordinator is never limited, so its reserve never waits, and it
        never takes a worker slot. A refused reserve first reclaims the
        session's slots whose holders are observed gone."""
        from . import capacity as delegation_capacity
        from . import events as delegation_events

        kind = "worker" if launch.get("assignment_id") else "coordinator"
        if limit is None:
            limit = delegation_capacity.limit_for_connection(None, self._capacity_limit)
        queued_note_recorded = False
        backoff = 0.05
        deadline = time.monotonic() + self._queue_timeout_seconds
        while True:
            if self._shutdown_event.is_set():
                raise QueuedOpenCancelled("shutdown")
            if attempt_id in self._cancelled_attempts:
                raise QueuedOpenCancelled("attempt")
            if task_id in self._cancelled_tasks:
                raise QueuedOpenCancelled("task")
            reservation_id = delegation_capacity.reserve(
                self._store, connection_id, task_id, attempt_id, kind, limit=limit,
            )
            if reservation_id is not None:
                if queued_note_recorded and kind == "worker":
                    from ..orchestration.measurement import attempt_wait
                    attempt_wait(self._store, task_id, attempt_id, None, connection_id)
                return reservation_id
            if kind == "worker" and self.reclaim_stale_capacity(task_id):
                continue
            if not queued_note_recorded:
                if kind == "worker":
                    from ..orchestration.measurement import attempt_wait
                    attempt_wait(self._store, task_id, attempt_id, 'route_capacity', connection_id)
                note = (
                    f"Attempt {attempt_id} is queued: this session already runs "
                    f"{delegation_capacity.MAX_WORKERS_PER_SESSION} workers, or connection "
                    f"{connection_id!r} is at its own limit."
                )
                delegation_events.append_event(
                    self._store, task_id, "progress", f"capacity_queued:{attempt_id}",
                    dict(note=note), origin="runtime",
                )
                queued_note_recorded = True
            # Only a worker ever gets here (a coordinator is never limited),
            # and it only waits for another worker's release, the bounded
            # queue deadline, or cancellation.
            if time.monotonic() > deadline:
                # Slots held by sessions still observed alive are real work
                # that will release them: keep waiting another period. Seven
                # parallel audits on a four-slot connection failed three
                # workers here after 600 s while four live siblings worked
                # (2026-09-28). Only a slot whose holder is not observed
                # alive (ambiguous, leaked) ends the wait.
                if self._capacity_held_by_live_sessions(connection_id, kind, task_id):
                    deadline = time.monotonic() + self._queue_timeout_seconds
                    continue
                raise QueuedOpenCancelled("capacity_timeout")
            with self._activity_cond:
                self._activity_cond.wait(timeout=backoff)
            backoff = min(backoff * 2, 1.0)

    def _capacity_held_by_live_sessions(self, connection_id: str, kind: str, task_id: str | None = None) -> bool:
        """True when every active reservation the queued open waits behind
        (its session's workers) is ``reserved`` by a session :meth:`lookup_owned`
        observes alive. An ambiguous reservation or a holder not observed
        alive (absent, unknown) is not progress to wait for."""
        from . import capacity as delegation_capacity

        if self._store is None:
            return False
        holders = delegation_capacity.active_holders(self._store, connection_id, kind, task_id)
        return bool(holders) and all(
            state == "reserved" and self.lookup_owned(attempt_id) == "alive"
            for attempt_id, state in holders
        )

    def reclaim_stale_capacity(self, task_id: str) -> int:
        """Release session ``task_id``'s worker slots whose holder is
        observed gone; returns how many were released.

        A slot is reclaimed only on evidence that its worker no longer runs:
        this adapter's own engine process observed exited; a durable process
        record (``recovery.registry``) probed absent; or, with no process
        record at all, a reservation older than
        ``UNRECORDED_RESERVATION_EXPIRY_SECONDS`` that no open of this
        adapter is still working on. A holder observed alive, or unknown,
        keeps its slot -- ambiguous included. Each reclaim appends a
        ``capacity_reclaimed`` progress event naming the attempt,
        reservation and evidence (no engine output)."""
        from . import capacity as delegation_capacity
        from . import events as delegation_events
        from ..recovery import registry as recovery_registry

        if self._store is None:
            return 0
        opening = getattr(self, "_opening", set())
        reclaimed = 0
        for row in delegation_capacity.active_reservations(self._store, task_id, "worker"):
            attempt_id = row["attempt_id"]
            with self._lock:
                if attempt_id in opening:
                    continue
                owned = attempt_id in self._sessions
                manager = self._managers.get(attempt_id) if owned else None
            if owned:
                process = getattr(manager, "_process", None)
                if process is None or not process.wait_exited(0):
                    continue
                evidence = "owned_process_exited"
            else:
                process_row = recovery_registry.lookup_process(self._store, attempt_id)
                if process_row is None:
                    try:
                        reserved_at = datetime.fromisoformat(row["reserved_at"])
                    except (TypeError, ValueError):
                        continue
                    age = (datetime.now(timezone.utc) - reserved_at).total_seconds()
                    if age < delegation_capacity.UNRECORDED_RESERVATION_EXPIRY_SECONDS:
                        continue
                    evidence = "unrecorded_reservation_expired"
                elif recovery_registry.probe_liveness(process_row) != "absent":
                    continue
                else:
                    evidence = "process_absent"
            if not delegation_capacity.release_confirmed(
                self._store, row["reservation_id"], f"reclaimed:{evidence}",
            ):
                continue
            reclaimed += 1
            delegation_events.append_event(
                self._store, task_id, "progress", f"capacity_reclaimed:{row['reservation_id']}",
                dict(
                    note=f"Worker slot of attempt {attempt_id} reclaimed: {evidence}.",
                    kind="capacity_reclaimed", attempt_id=attempt_id,
                    reservation_id=row["reservation_id"], prior_state=row["state"], evidence=evidence,
                ),
                origin="runtime",
            )
        if reclaimed:
            self._notify_activity()
        return reclaimed

    def _notify_activity(self) -> None:
        with self._activity_cond:
            self._activity_cond.notify_all()

    def cancel_attempt(self, attempt_id: str) -> None:
        """Cancel a specific queued (not-yet-opened) attempt promptly."""
        self._cancelled_attempts.add(attempt_id)
        self._notify_activity()

    def _check_not_cancelled(self, attempt_id: str, task_id: str) -> None:
        """Raise :class:`QueuedOpenCancelled` if ``attempt_id`` (or its
        whole task) was cancelled -- called by :meth:`open` at each
        natural boundary between its own blocking steps (see that
        method's own comment on why this bounds cancellation latency to
        one in-flight round-trip instead of the whole launch)."""
        if attempt_id in self._cancelled_attempts:
            raise QueuedOpenCancelled("attempt")
        if task_id in self._cancelled_tasks:
            raise QueuedOpenCancelled("task")

    def owned_attempts(self, task_id: str) -> tuple[str, ...]:
        """Exact task-owned opens, sessions and quarantined workspace locks."""
        with self._lock:
            active = self._opening | set(self._managers) | set(self._workspace_locks)
            return tuple(a for a in active if self._attempt_task.get(a) == task_id)

    def cancel_task(self, task_id: str) -> None:
        """Cancel every queued open for ``task_id`` promptly."""
        self._cancelled_tasks.add(task_id)
        self._notify_activity()

    def invalidate_session(self, engine_session_ref: str, reason: str | None = None) -> None:
        """T028: duck-typed optional hook (see
        ``acp.orchestrating_runtime.OrchestratingRuntime.cancel``) --
        forwards to the exact ``EngineSessionManager`` that owns
        ``engine_session_ref`` (via ``_session_to_attempt``/``_managers``,
        the same lookup :meth:`prompt` itself uses), invalidating only that
        one session's ``BindingRegistry`` entry (see
        ``engine.resource_binding.BindingRegistry.invalidate_session``) so
        a permission/fs/terminal request already in flight for it is
        answered as cancelled rather than relayed. A session this adapter
        instance never opened (unknown ref, or opened by a different
        process before a restart) is a silent no-op -- there is no
        in-memory manager to invalidate a binding on."""
        with self._lock:
            attempt_id = self._session_to_attempt.get(engine_session_ref)
            manager = self._managers.get(attempt_id) if attempt_id is not None else None
        if manager is not None:
            manager.invalidate_session(engine_session_ref, reason)

    def invalidate_turn(self, engine_session_ref: str) -> None:
        """Like :meth:`invalidate_session`, but scoped to the turn in flight:
        the session keeps working for the next turn (see
        ``EngineSessionManager.invalidate_turn``)."""
        with self._lock:
            attempt_id = self._session_to_attempt.get(engine_session_ref)
            manager = self._managers.get(attempt_id) if attempt_id is not None else None
        if manager is not None:
            manager.invalidate_turn(engine_session_ref)

    def shutdown(self) -> None:
        """Signal every queued open to abort, then terminate every owned
        engine process with a confirmed/ambiguous release (never a
        reservation leak)."""
        with self._lock:
            self._shutdown_event.set()
            # A started settlement already owns teardown of its exact manager.
            # Do not retry that manager during this shutdown if exit is unknown.
            settling = getattr(self, "_result_settlement_managers", {})
            attempts = [(attempt, manager) for attempt, manager in self._managers.items()
                        if settling.get(attempt) is not manager]
        self._notify_activity()
        for attempt_id, manager in attempts:
            try:
                self.terminate_owned(attempt_id, expected_manager=manager)
            except Exception:
                # One ambiguous child must not strand other owned sessions.
                continue
        self._join_result_settlements()

    def _join_result_settlements(self) -> None:
        """Let post-result teardown already in progress finish before shutdown
        returns. The shared wait is bounded; a teardown still stuck after it
        is left as a daemon rather than awaited forever."""
        with self._lock:
            threads = list(getattr(self, "_result_settlements", {}).values())
        deadline = time.monotonic() + self._SETTLEMENT_SHUTDOWN_JOIN_SECONDS
        for thread in threads:
            if (not isinstance(thread, threading.Thread) or thread.ident is None
                    or thread is threading.current_thread()):
                continue
            thread.join(max(0.0, deadline - time.monotonic()))

    def lookup_owned(self, attempt_id: str) -> str:
        """"alive"/"absent"/"unknown" for the process owned by
        ``attempt_id``. A same-process manager (this exact adapter instance
        launched it) is always authoritative and answers "alive"
        immediately. Otherwise (this adapter never launched it -- almost
        always because a *different* process/adapter instance did, before a
        Cortex process restart) falls back to the durable
        ``recovery.registry`` identity captured at launch time (T023: see
        that module and ``engine.process_identity`` -- a bare "no record in
        memory" no longer means "absent" the way it did pre-T023)."""
        with self._lock:
            if attempt_id in self._sessions or attempt_id in getattr(self, "_managers", {}):
                return "alive"
        if self._store is None:
            return "absent"
        from ..recovery import registry as recovery_registry

        row = recovery_registry.lookup_process(self._store, attempt_id)
        if row is None:
            return "absent"
        state = recovery_registry.probe_liveness(row)
        if state == "absent":
            recovery_registry.mark_terminated(self._store, attempt_id, "lookup_owned_absent")
        return state

    def lookup_owned_tx(self, attempt_id: str, db) -> str:
        """The same liveness gate using the caller's SQLite transaction.

        Absence bookkeeping must not acquire a second writer while synthesis
        holds BEGIN IMMEDIATE. The caller owns commit/rollback.
        """
        with self._lock:
            if attempt_id in self._sessions or attempt_id in getattr(self, "_managers", {}):
                return "alive"
        if self._store is None:
            return "absent"
        from ..recovery import registry as recovery_registry

        row = db.execute("SELECT * FROM attempt_process_registry WHERE attempt_id=?",
                         (attempt_id,)).fetchone()
        if row is None:
            return "absent"
        state = recovery_registry.probe_liveness(dict(row))
        if state == "absent":
            recovery_registry.mark_terminated_tx(db, attempt_id, "lookup_owned_absent")
        return state

    def invalidate_owner_epoch(self, attempt_id: str, new_epoch) -> None:
        """T035 EXT-01 ownership-transfer gap closure:
        ``engine.resource_binding.BindingRegistry.invalidate_epoch`` had no
        production caller because nothing on the ownership-transfer side
        (``ownership.acquire_task_owner_tx``/``reassign_worker_channels_tx``,
        called from ``recovery.coordinator_recovery``) holds a live
        ``ResourceBinding`` or its ``workspace_binding_id`` -- only this
        adapter's own ``attempt_id -> manager -> session_id -> binding``
        chain does. Resolves ``workspace_binding_id`` from that chain
        itself, so a caller only ever needs the ``attempt_id`` it already
        has and the freshly-acquired epoch -- never invented or re-derived
        data.

        A no-op if this process has no live manager for ``attempt_id``
        (already terminated -- see :meth:`terminate_owned`, which already
        invalidates unconditionally via ``bindings.release`` -- or never
        opened in this process at all, e.g. after a restart; see
        ``recovery.registry``), since there is then nothing in this
        process's memory left to invalidate.

        NOT YET CALLED from ``recovery.coordinator_recovery.recover``: that
        module is outside this change's edit scope. The missing wiring is
        exactly one call, right after that module's own step 1 commits
        (``result = ownership.acquire_task_owner_tx(...)``):
        ``engine.invalidate_owner_epoch(old_attempt_ref, result.epoch)``
        (``engine`` there is this adapter, already passed into
        ``recover()``). See this repository's task card for why that one
        line was left out of this change rather than added speculatively
        to a file outside the approved edit set."""
        with self._lock:
            manager = self._managers.get(attempt_id)
            session_id = self._sessions.get(attempt_id)
        if manager is None or session_id is None:
            return
        binding = manager.bindings.get(session_id)
        if binding is None:
            return
        manager.invalidate_epoch(binding.workspace_binding_id, str(new_epoch))

    # The turn that called result_submit usually still writes a closing
    # message; its PromptResponse is the only token carrier and nothing can
    # deliver it after teardown. Live v5/v6 dev runs: natural tails 2.5-4.9 s
    # after the result, and 4 of 25 turns were cut by the former 5 s bound.
    # A turn that ends sooner settles at once; this only bounds a hung one.
    _RESULT_SETTLE_SECONDS = 15.0
    # How often a settlement wait re-checks shutdown, and how long shutdown
    # waits in total for settlement teardown already in progress.
    _SETTLE_POLL_SECONDS = 0.05
    _SETTLEMENT_SHUTDOWN_JOIN_SECONDS = 5.0

    def finish_owned(self, attempt_id: str) -> None:
        """Bank a result-bearing prompt off the scheduler path before teardown.

        The prompt consumer owns usage recording. Capacity and the writer lock
        remain held until the usual confirmed-exit cleanup; explicit cancellation
        still uses terminate_owned immediately. Shutdown interrupts the wait.
        """
        shutdown = getattr(self, "_shutdown_event", None)
        with self._lock:
            done = getattr(self, "_prompt_done", {}).get(attempt_id)
            manager = self._managers.get(attempt_id)
            settling = getattr(self, "_result_settlements", None)
            if settling is None:
                settling = self._result_settlements = {}
            if attempt_id in settling:
                return
            if (done is not None and not done.is_set() and manager is not None
                    and not (shutdown is not None and shutdown.is_set())):
                thread = threading.Thread(
                    target=self._finish_result_prompt, args=(attempt_id, manager, done),
                    name="engine-result-settlement", daemon=True,
                )
                owners = getattr(self, "_result_settlement_managers", None)
                if owners is None:
                    owners = self._result_settlement_managers = {}
                settling[attempt_id] = thread
                owners[attempt_id] = manager
                # Publish and start under the same lock: shutdown must never
                # delegate cleanup to a thread that has not started yet.
                try:
                    thread.start()
                except BaseException:
                    settling.pop(attempt_id, None)
                    owners.pop(attempt_id, None)
                    raise
            else:
                thread = None
        if thread is None:
            self.terminate_owned(attempt_id)

    def result_settlement_pending(self, attempt_id: str) -> bool:
        """Whether this adapter still owns post-result usage/exit cleanup.

        This is only a wait hint, never proof of engine absence or usage credit.
        Keep it true through teardown; lookup_owned remains the liveness gate.
        """
        with self._lock:
            return attempt_id in getattr(self, "_result_settlements", {})

    def _await_result_prompt(self, done) -> tuple[bool, bool]:
        """Wait for the consumer to record usage: (settled, interrupted by shutdown)."""
        shutdown = getattr(self, "_shutdown_event", None)
        deadline = time.monotonic() + self._RESULT_SETTLE_SECONDS
        while not done.is_set():
            if shutdown is not None and shutdown.is_set():
                return False, True
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            done.wait(min(remaining, self._SETTLE_POLL_SECONDS))
        return done.is_set(), False

    def _finish_result_prompt(self, attempt_id, manager, done) -> None:
        settled, interrupted = self._await_result_prompt(done)
        try:
            # Cancellation/replacement may already have terminated this manager.
            # Never act on another incarnation or invoke cross-process cleanup.
            with self._lock:
                still_owned = self._managers.get(attempt_id) is manager
                task_id = self._attempt_task.get(attempt_id)
                session_id = self._sessions.get(attempt_id)
            if still_owned:
                try:
                    # Shutdown is not a settlement timeout: ordinary teardown
                    # only; the consumer's own row stays unknown if unrecorded.
                    if self._store is not None and task_id is not None and not interrupted:
                        from . import events
                        events.append_event(
                            self._store, task_id, "progress",
                            f"usage_settlement:{attempt_id}:{session_id}",
                            dict(kind="usage_settlement", attempt_id=attempt_id,
                                 engine_session_ref=session_id,
                                 outcome="prompt_recorded" if settled else "timeout",
                                 reason=None if settled else "post_result_prompt_settlement_timeout",
                                 # Teardown ends the engine before its only token
                                 # carrier: this call's usage cannot arrive.
                                 usage_coverage=None if settled else "unavailable",
                                 timeout_seconds=self._RESULT_SETTLE_SECONDS),
                            origin="runtime", assignment_ref=attempt_id,
                        )
                finally:
                    self.terminate_owned(attempt_id, expected_manager=manager)
        finally:
            with self._lock:
                self._result_settlements.pop(attempt_id, None)
                owners = getattr(self, "_result_settlement_managers", {})
                if owners.get(attempt_id) is manager:
                    owners.pop(attempt_id, None)
            self._notify_activity()

    def terminate_owned(self, attempt_id: str, *, expected_manager=None) -> None:
        # Repeated stop/result/disconnect cleanup must not race shutdown and
        # unlock a writer while another cleanup still owns its manager.
        with self._lock:
            locks = getattr(self, "_termination_locks", None)
            if locks is None:
                self._termination_locks = locks = {}
            lock = locks.setdefault(attempt_id, threading.Lock())
        with lock:
            self._terminate_owned(attempt_id, expected_manager=expected_manager)

    def _terminate_owned(self, attempt_id: str, *, expected_manager=None) -> None:
        with self._lock:
            if expected_manager is not None and self._managers.get(attempt_id) is not expected_manager:
                return
            manager = self._managers.pop(attempt_id, None)
            session_id = self._sessions.pop(attempt_id, None)
            reservation_id = self._reservations.pop(attempt_id, None)
            if session_id is not None:
                self._session_to_attempt.pop(session_id, None)
            self._session_meta.pop(session_id, None) if session_id is not None else None
            self._pending_generation.pop(session_id, None) if session_id is not None else None
            self._session_configured_revision.pop(session_id, None) if session_id is not None else None
            # Keep the lock associated with the attempt until shutdown has
            # been positively observed.  An ambiguous/timeout termination
            # must quarantine the project against a new writer in this
            # adapter; releasing here would let a still-running engine edit
            # concurrently with the next worker.
            # A few recovery-only test/compatibility adapters are built via
            # ``__new__`` and predate direct-project locking. Missing state
            # means there is no lock to retain, never an error during
            # termination reconciliation.
            workspace_locks = getattr(self, "_workspace_locks", None)
            workspace_lock = workspace_locks.get(attempt_id) if workspace_locks is not None else None
        if self._store is not None:
            try:
                from .session_skills import revoke_session_skills

                skill_session = session_id
                if skill_session is None:
                    from ..recovery import registry as recovery_registry

                    process_row = recovery_registry.lookup_process(self._store, attempt_id)
                    skill_session = process_row["engine_session_ref"] if process_row is not None else None
                if skill_session is not None:
                    revoke_session_skills(self._store, skill_session)
            except Exception:
                # Failed metadata invalidation must not bypass process teardown.
                pass
        if manager is not None:
            try:
                manager.shutdown(grace=2.0)
            except Exception:
                with self._lock:
                    self._managers[attempt_id] = manager
                    if reservation_id is not None:
                        self._reservations[attempt_id] = reservation_id
                raise
            exited = None
            process = getattr(manager, "_process", None)
            # shutdown() above already waited up to its own grace for exit;
            # a non-blocking check here just reads whatever it observed --
            # never a second wait, never assumed just because shutdown()
            # returned (see EngineProcess.release/wait_exited).
            if process is not None:
                exited = bool(process.wait_exited(0))
            if self._store is not None:
                from ..recovery import registry as recovery_registry

                if exited:
                    recovery_registry.mark_terminated(self._store, attempt_id, "process_exited")
        elif self._store is not None:
            # T023: no same-process manager owns this attempt -- either it
            # was launched by a *different* process (a restart), or it was
            # never launched at all. Terminate it directly from the
            # durable registry record: never a second .wait() (this
            # process is not that child's parent and cannot reap it), so
            # exit is confirmed only by re-probing OS-level identity after
            # signalling, exactly like the ambiguous/confirmed distinction
            # ``EngineProcess.terminate`` already makes for a same-process
            # child.
            exited = self._terminate_cross_process(attempt_id)
            if reservation_id is None:
                from . import capacity as delegation_capacity

                # T023: this adapter instance never itself reserved capacity
                # for this attempt (a different process's own adapter did,
                # before a restart) -- its reservation id lives only in
                # capacity_reservations, never in this instance's own
                # in-memory _reservations dict.
                reservation_id = delegation_capacity.reservation_for_attempt(self._store, attempt_id)
        else:
            exited = None
        if manager is not None and not exited:
            with self._lock:
                self._managers[attempt_id] = manager
                if reservation_id is not None:
                    self._reservations[attempt_id] = reservation_id
        if workspace_lock is not None:
            if exited:
                workspace_lock.__exit__(None, None, None)
                if workspace_locks is not None:
                    with self._lock:
                        workspace_locks.pop(attempt_id, None)
            else:
                # Deliberately retain the open advisory lock as a local
                # quarantine.  A later open will fail closed at the bounded
                # acquisition timeout until the ambiguous process is
                # reconciled and this attempt is explicitly released.
                if workspace_locks is not None:
                    with self._lock:
                        workspace_locks[attempt_id] = workspace_lock
        if self._store is not None and session_id is not None:
            from . import deliveries as delegation_deliveries

            # T022: a released session's not-yet-``continuation_observed``
            # deliveries can never later report confirmation from a stale
            # in-flight call -- "stale session (released/replaced) -> never
            # confirmed".
            delegation_deliveries.supersede_session_deliveries(self._store, session_id, "attempt_terminated")
        if reservation_id is not None and self._store is not None:
            from . import capacity as delegation_capacity
            from .. import _fault_injection

            _fault_injection.checkpoint("t028_between_terminate_and_capacity_release")
            if exited:
                delegation_capacity.release_confirmed(self._store, reservation_id, "process_exited")
            else:
                delegation_capacity.mark_ambiguous(self._store, reservation_id, "timeout_alive")
            # Confirmed release frees a slot a queued open is waiting on;
            # ambiguous does not free anything, but still wakes queued
            # waiters promptly so they can re-check current capacity
            # instead of sitting on their own backoff interval.
            self._notify_activity()

    def _terminate_cross_process(self, attempt_id: str) -> bool:
        """Best-effort SIGTERM-then-SIGKILL of an attempt's process group
        this adapter instance never itself launched, by durable pid/pgid
        identity alone (see ``recovery.registry``). Returns ``True`` only
        once its exit is actually re-observed (never assumed from the
        signal alone); ``False`` (ambiguous, never freed) otherwise --
        including when there was never a registry record to act on (no
        capacity was ever reserved for it in that case either)."""
        import os
        import signal
        import time

        from ..engine.process import TERMINATE_GRACE_SECONDS
        from ..recovery import registry as recovery_registry

        row = recovery_registry.lookup_process(self._store, attempt_id)
        if row is None:
            return False
        state = recovery_registry.probe_liveness(row)
        if state == "absent":
            recovery_registry.mark_terminated(self._store, attempt_id, "already_absent")
            return True
        if state != "alive":
            return False
        try:
            os.killpg(row["pgid"], signal.SIGTERM)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
        while time.monotonic() < deadline:
            if recovery_registry.probe_liveness(row) == "absent":
                recovery_registry.mark_terminated(self._store, attempt_id, "process_exited")
                return True
            time.sleep(0.05)
        state = recovery_registry.probe_liveness(row)
        if state == "absent":
            recovery_registry.mark_terminated(self._store, attempt_id, "process_exited")
            return True
        if state != "alive":
            return False
        try:
            os.killpg(row["pgid"], signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
        while time.monotonic() < deadline:
            if recovery_registry.probe_liveness(row) == "absent":
                recovery_registry.mark_terminated(self._store, attempt_id, "process_exited")
                return True
            time.sleep(0.05)
        return False

    def _observe_compaction_signal(self, manager, session_ref: str, event) -> None:
        """T022: an observed compaction reaching its terminal phase opens
        (at most once per evidence id -- see
        ``delegation.generations.record_compaction_event``) a fresh
        generation, queued the same way ``open()`` queues its own initial
        one -- picked up by :meth:`pending_delivery` on the *next*
        continuation, never mid-turn (Cortex cannot rewrite a turn already
        in flight).

        T035 gap closure: also invalidates this session's own
        ``ResourceBinding`` context generation (see
        ``engine.session_manager.EngineSessionManager.invalidate_generation``
        / ``engine.resource_binding.ResourceBinding.is_current``) the same
        instant the fresh generation is opened -- previously nothing in
        production ever called this either, so a permission request whose
        host round-trip straddled a mid-session compaction would still be
        relayed as if nothing had changed underneath it, exactly the
        staleness ``is_current`` exists to catch.
        """
        from . import generations as delegation_generations

        with self._lock:
            meta = self._session_meta.get(session_ref)
            if meta is not None and event.tool_call_ref is not None and meta.get("compaction_evidence") != event.tool_call_ref:
                # The first signal of this compaction: the context fill the
                # engine last reported is its pre-compaction size. Cleared
                # so a later compaction never reuses this stale figure.
                meta["compaction_evidence"] = event.tool_call_ref
                meta["pre_compaction_used"] = meta.pop("context_used", None)
        if meta is None or event.tool_call_ref is None:
            return
        phase = "completed" if event.compaction_reason == "completed" else (
            "failed" if event.compaction_reason in ("failed",) else "in_progress"
        )
        generation = delegation_generations.record_compaction_event(
            self._store, task_id=meta["task_id"], engine_session_ref=session_ref,
            evidence_id=event.tool_call_ref, phase=phase,
            evidence_source="tool_call_meta_contextCompaction", recipient=meta["recipient"],
            assignment_ref=meta["assignment_ref"],
        )
        if generation is not None:
            if meta.get("channel_id"):
                from .session_skills import bind_session_skill_generation

                bind_session_skill_generation(
                    self._store, channel_id=meta["channel_id"], task_id=meta["task_id"],
                    attempt_id=meta["attempt_id"], generation_id=generation["id"], session_ref=session_ref,
                )
            with self._lock:
                self._pending_generation[session_ref] = generation
            manager.invalidate_generation(session_ref, generation["id"])

    def _observe_context_window(self, session_ref: str, size: object) -> None:
        """Keep the context window the engine just reported (``usage_update
        .size``) for this session and, durably, for its connection's route
        model -- see :mod:`connections.context_window`."""
        from ..connections import context_window

        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            return
        sessions = getattr(self, "_session_meta", None)
        if sessions is None:
            return
        with self._lock:
            meta = sessions.get(session_ref)
            if meta is None or not meta.get("connection_id"):
                return
            meta["context_window"] = size
        context_window.observe(meta["connection_id"], meta.get("model"), size, self._environ)

    def _observe_context_used(self, session_ref: str, used: object) -> None:
        """Keep the context fill the engine just reported (``usage_update
        .used``) for this session, in memory only."""
        if not isinstance(used, int) or isinstance(used, bool) or used < 0:
            return
        sessions = getattr(self, "_session_meta", None)
        if sessions is None:
            return
        with self._lock:
            meta = sessions.get(session_ref)
            if meta is not None:
                meta["context_used"] = used

    def last_usage(self, session_ref: str) -> dict | None:
        """The context fill the engine reported for ``session_ref`` right
        before its most recent observed compaction, or ``None`` when none
        was reported in this process (see ``recovery.compaction_guard``)."""
        from ..recovery import token_budget

        sessions = getattr(self, "_session_meta", None)
        if sessions is None:
            return None
        with self._lock:
            used = (sessions.get(session_ref) or {}).get("pre_compaction_used")
        if used is None:
            return None
        return {"tokens": used, "basis": token_budget.ENGINE_REPORTED_BASIS}

    def context_window(self, session_ref: str) -> int | None:
        """The real context window for ``session_ref``: what this session's
        engine last reported, else what the same connection's engine last
        reported for the same route model. ``None`` when never reported."""
        from ..connections import context_window

        with self._lock:
            meta = dict(self._session_meta.get(session_ref) or {})
        if not meta.get("connection_id"):
            return None
        if meta.get("context_window"):
            return meta["context_window"]
        return context_window.observed(meta["connection_id"], meta.get("model"), self._environ)

    def pending_delivery(self, session_ref: str) -> Mapping[str, object] | None:
        """T022 (optional ``SessionRuntime`` extension, checked via
        ``getattr`` by ``acp.orchestrating_runtime`` -- never required):
        the queued fresh :class:`context_generations` row for
        ``session_ref``, if any, that has not yet been acknowledged via
        :meth:`begin_delivery`/:meth:`clear_pending_delivery`."""
        with self._lock:
            generation = self._pending_generation.get(session_ref)
            meta = self._session_meta.get(session_ref)
        if generation is None or meta is None:
            return None
        return dict(
            generation=generation, recipient=meta["recipient"], task_id=meta["task_id"],
            attempt_id=meta.get("attempt_id"), engine=meta.get("engine"),
            version=meta.get("version"), instructions=meta.get("instructions"),
            connection_id=meta.get("connection_id"),
        )

    def begin_delivery(self, session_ref: str, capsule_revision: str) -> dict | None:
        from . import deliveries as delegation_deliveries

        with self._lock:
            generation = self._pending_generation.get(session_ref)
            meta = self._session_meta.get(session_ref)
        if generation is None or meta is None or self._store is None:
            return None
        return delegation_deliveries.record_built(
            self._store, recipient=meta["recipient"], context_generation_id=generation["id"],
            capsule_revision=capsule_revision, task_id=meta["task_id"], engine_session_ref=session_ref,
            evidence_source="orchestrating_runtime:generation_delivery_build",
            assignment_ref=meta["assignment_ref"],
        )

    def mark_delivery_transmitted(self, session_ref: str, delivery_id: str) -> None:
        from . import deliveries as delegation_deliveries

        if self._store is not None:
            delegation_deliveries.mark_transmitted(self._store, delivery_id, "session_prompt_call_issued")

    def mark_delivery_confirmed(self, session_ref: str, delivery_id: str) -> None:
        from . import deliveries as delegation_deliveries

        row = None
        if self._store is not None:
            row = delegation_deliveries.mark_delivery_confirmed(
                self._store, delivery_id, "session_prompt_response_received",
            )
        with self._lock:
            # T035 EXT-01 #3 gap closure: this pop is a conditional
            # transition, not an unconditional delete keyed only on
            # ``session_ref``. A new compaction can already have queued a
            # *later* generation (and started its own delivery) while this
            # delivery -- built against an *older* generation -- was still
            # in flight; confirming the older one must never erase the
            # pending requirement for the newer one still owed on this
            # session.
            pending = self._pending_generation.get(session_ref)
            if pending is not None and row is not None and pending.get("id") == row.get("context_generation_id"):
                self._pending_generation.pop(session_ref, None)

    def mark_delivery_uncertain(self, session_ref: str, delivery_id: str, reason: str) -> None:
        from . import deliveries as delegation_deliveries

        if self._store is not None:
            delegation_deliveries.mark_receipt_uncertain(self._store, delivery_id, reason)

    def mark_delivery_blocked(self, session_ref: str, delivery_id: str, reason: str, *, permanent: bool = False) -> None:
        from . import deliveries as delegation_deliveries

        if self._store is not None:
            delegation_deliveries.mark_blocked(self._store, delivery_id, reason, "orchestrating_runtime:generation_delivery_blocked")
        if permanent:
            with self._lock:
                self._pending_generation.pop(session_ref, None)

    # EXT-01 #4 fix: how often the independent cancel-watcher below re-checks
    # ``cancel_event`` while ``manager.prompt_turn()`` is blocked with
    # nothing to yield (silent engine, transport still open). This is
    # deliberately *not* a wall-clock bound on the turn itself (the turn's
    # own iteration below is never given a timeout -- see
    # ``engine.session_manager``'s own T035 FINDING-06 comment); it only
    # bounds how quickly a cancel gets *sent*, independent of whether the
    # engine ever emits another event.
    _CANCEL_WATCH_POLL_SECONDS = 0.05
    # A turn is never killed for being quiet: a model may think silently
    # for many minutes. After this much silence from a live engine the
    # turn is interrupted with session/cancel and the same session is
    # re-prompted to continue, context intact. Doubles after each nudge.
    # ponytail: fixed first threshold; make it per-engine config if a
    # provider routinely thinks longer than this without streaming.
    _STALL_NUDGE_SECONDS = 900.0
    _STALL_NUDGE_PROMPT = (
        "Runtime liveness check: nothing was received from you for a long time, so your "
        "previous step was interrupted. Continue the same work from where you stopped; "
        "do not restart or repeat completed steps."
    )
    # OpenCode 2.0.19 ends the whole turn with stopReason "cancelled" when a
    # permission request is answered with a reject option (observed), where
    # the other engines only fail that one tool call. A refusal is a runtime
    # decision, not a cancellation: the same session is told so and
    # continues, at most this many times per prompt() call.
    _MAX_REFUSAL_CONTINUATIONS = 3
    _REFUSAL_CONTINUE_PROMPT = (
        "Runtime notice: your last tool call was refused by a permission decision; it is not an error. "
        "Do not retry it or reach the same effect another way. Continue your work without it, or report "
        "it as a blocker."
    )

    _WORKER_WAIT_GRACE_SECONDS = 30.0
    _WORKER_WAIT_CANCEL_SECONDS = 30.0

    def prompt(
        self, session_ref: str, message: str, cancel_event: threading.Event, *,
        on_delta: Callable[[str], bool] | None = None,
        on_tool_call: Callable[..., None] | None = None,
        on_display_update: Callable[[dict], None] | None = None,
        prompt_blocks=None,
    ):
        """T035 visibility seam: ``on_delta``, when given, receives each
        provider ``TURN_DELTA`` synchronously so the coordinator can assemble
        complete message blocks at tool/display/turn boundaries. The
        coordinator is the only caller that supplies it; delegated-worker
        prompts pass none, so worker prose is never streamed to chat. The
        callback is best-effort like ``emit_update`` itself: an exception
        never aborts an otherwise successful engine turn."""
        from ..acp.orchestrating_runtime import EngineTurnFailed, PromptOutcome
        from ..engine import failure_classification
        from ..engine.client import EngineProtocolError
        from ..portable.engine_contract import EngineEventKind, StopReason, UsageBasis
        from ..portable.errors import PortableError

        with self._lock:
            attempt_id = self._session_to_attempt.get(session_ref)
            manager = self._managers.get(attempt_id) if attempt_id is not None else None
        if manager is None:
            raise RuntimeError(f"prompt() called on an unopened/terminated session {session_ref!r}.")
        task_id = self._attempt_task.get(attempt_id)
        prompt_done = threading.Event()
        with self._lock:
            if not hasattr(self, "_prompt_done"):
                self._prompt_done = {}
            self._prompt_done[attempt_id] = prompt_done
        if self._store is not None:
            from . import deliveries as delegation_deliveries

            # T022: initiating a new turn on this session is itself the
            # "subsequent engine turn" evidence for any delivery this
            # session already confirmed but had not yet observed a
            # continuation for.
            delegation_deliveries.mark_continuation_observed_if_pending(
                self._store, session_ref, "next_session_prompt_call_initiated",
            )
        text_parts: list[str] = []
        stop_reason = StopReason.END_TURN
        usage_basis = None
        usage_values: dict = {}
        # G7: a stall-nudge or refusal continuation calls
        # ``manager.prompt_turn`` again on this same ``prompt()`` call (see
        # the ``continue`` sites below); each inner turn's usage_update
        # counters are cumulative only within that one turn and restart at
        # zero on the next, so usage_basis/usage_values above (the current
        # turn's latest report) is banked into these accumulators at every
        # turn boundary instead of being overwritten by the next turn.
        accumulated_usage_values: dict = {}
        accumulated_usage_basis = None
        # P1 fix: whether any inner turn of this call ended with no
        # usage_update at all (usage_basis still None when _bank_turn_usage
        # ran for it). One turn reporting does not make a later silent turn's
        # contribution known -- the combined total must then never be
        # recorded as fully 'engine_reported' (see the finally block below).
        any_turn_missing_usage = False
        turn_events = None
        call_failed = True
        # EXT-01 #4 fix (independent-review P1 #4): the per-event
        # ``cancel_event.is_set()`` check below only ever runs when this
        # loop's own ``for`` statement advances -- i.e. only once
        # ``manager.prompt_turn()`` actually yields another event. A silent
        # engine (accepted the prompt, transport still open, never emits
        # another event) parks that generator on its own internal
        # ``update_queue.get()`` indefinitely (by design -- T035 FINDING-06
        # removed the wall-clock bound on the turn itself), so a cancel
        # requested while parked there was never sent downstream at all
        # until *something else* drove another iteration. This watcher
        # thread sends ``manager.cancel(session_ref)`` the moment
        # ``cancel_event`` is set, entirely independent of the event
        # stream -- it never waits for, and is never driven by, the next
        # event. The per-event check right below is kept as a fast path
        # (an event that happens to arrive at the same moment as the cancel
        # need not wait even one poll interval) and to preserve its
        # existing fault-injection checkpoint; ``_cancel_sent`` makes the
        # two paths race-safe without ever sending the notification twice.
        _cancel_sent = threading.Event()
        _stop_watch = threading.Event()
        # A tool call separates prose within one turn. Telemetry and display
        # updates can arrive between chunks of the same sentence.
        _saw_tool_boundary = [False]
        # Stall watchdog state: when the engine last said anything, the
        # current silence limit, and whether this turn was nudged.
        _last_event = [time.monotonic()]
        _stall_limit = [self._STALL_NUDGE_SECONDS]
        _nudged = threading.Event()
        # Tool calls the engine started and has not finished: a shell or
        # MCP call may legitimately run for hours in silence, and nudging
        # would cancel it. Silence only counts while nothing is running.
        _open_tools: set = set()
        _parked = threading.Event()
        _park_unconfirmed = threading.Event()
        _park_started = [None]
        _wait_started = [None]

        def _watch_for_cancel() -> None:
            while not _stop_watch.is_set():
                if cancel_event.wait(timeout=self._CANCEL_WATCH_POLL_SECONDS):
                    if not _cancel_sent.is_set():
                        _cancel_sent.set()
                        manager.cancel(session_ref)
                    return
                # Human waits acknowledge immediately. An uncooperative question
                # turn gets a cancellation nudge after a grace period, never a Stop.
                wait = None
                if self._store is not None and task_id and attempt_id:
                    from ..orchestration import worker_waits
                    with self._store.connection() as db:
                        provision = db.execute(
                            'SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                            (task_id, attempt_id),
                        ).fetchone()
                        if provision:
                            wait = worker_waits.current(db, task_id, provision[0])
                if wait and wait['status'] in {'open', 'ready'}:
                    _wait_started[0] = _wait_started[0] or time.monotonic()
                    if not _parked.is_set() and time.monotonic() - _wait_started[0] >= self._WORKER_WAIT_GRACE_SECONDS:
                        _parked.set()
                        _park_started[0] = time.monotonic()
                        manager.cancel(session_ref)
                    elif (_parked.is_set() and time.monotonic() - _park_started[0] >= self._WORKER_WAIT_CANCEL_SECONDS):
                        _park_unconfirmed.set()
                        # No observed turn end: this session cannot be reused.
                        # Terminate only this owned manager, then normal recovery
                        # requires observed absence and reconciles action receipts.
                        manager.shutdown()
                        return
                    continue
                if (not _nudged.is_set() and not _open_tools
                        and time.monotonic() - _last_event[0] >= _stall_limit[0]):
                    _nudged.set()
                    manager.cancel(session_ref)

        watcher = threading.Thread(target=_watch_for_cancel, name="engine-cancel-watch", daemon=True)
        watcher.start()

        def _bank_turn_usage() -> None:
            # G7: fold the current inner turn's latest reported usage into
            # the running accumulators, then reset for the next turn.
            # A turn ending with absent or UNKNOWN token usage contributes
            # an unknown amount -- flagging it keeps the combined
            # total from being recorded as fully 'engine_reported' just
            # because some *other* turn of this same call did report.
            nonlocal usage_basis, usage_values, accumulated_usage_basis, any_turn_missing_usage
            if usage_basis is None or usage_basis == UsageBasis.UNKNOWN:
                any_turn_missing_usage = True
            if usage_basis is not None:
                accumulated_usage_basis = usage_basis
                for key, value in usage_values.items():
                    accumulated_usage_values[key] = accumulated_usage_values.get(key, 0) + value
            usage_basis = None
            usage_values = {}

        try:
            instruction_delivery_observed = False
            refusal_continuations = 0
            turn_message, turn_blocks = message, prompt_blocks
            while True:
                _last_event[0] = time.monotonic()
                _open_tools.clear()
                saw_turn_end = False
                turn_events = manager.prompt_turn(session_ref, turn_message, prompt_blocks=turn_blocks)
                for event in turn_events:
                    _last_event[0] = time.monotonic()
                    if not instruction_delivery_observed and self._store is not None:
                        instruction_delivery_observed = True
                        with self._lock:
                            bundle_id = (getattr(self, "_session_meta", {}).get(session_ref) or {}).get(
                                "instruction_bundle_id"
                            )
                        if bundle_id:
                            from ..portable.instruction_bundle import InstructionBundleLedger

                            ledger = InstructionBundleLedger(self._store)
                            bundle = ledger.get(bundle_id)
                            if bundle is not None and not bundle.delivery_bound:
                                ledger.record_sent(bundle, message, engine=bundle.engine,
                                                   engine_version=bundle.engine_version,
                                                   attempt_id=bundle.attempt_id, session_ref=session_ref)
                    if cancel_event.is_set() and not _cancel_sent.is_set():
                        from .. import _fault_injection

                        # T028: any event still arriving on this iteration
                        # (including a TURN_END success) after cancel_event was
                        # observed set is a "late success after cancel" -- the
                        # caller (acp.agent.Agent._run_prompt_body) always
                        # forces StopReason.CANCELLED regardless of what this
                        # loop returns once cancel_event is set, so a late
                        # success here is recorded (its own durable events/
                        # deliveries still land normally) but never reported as
                        # having completed the turn or used to revive it.
                        _fault_injection.checkpoint("t028_late_event_after_cancel")
                        _cancel_sent.set()
                        manager.cancel(session_ref)
                    if event.kind == EngineEventKind.TOOL_CALL_OBSERVED:
                        _saw_tool_boundary[0] = True
                        # A worker's terminal answer is only the text after
                        # its last tool. Keep earlier deltas in engine event
                        # evidence, not in PromptOutcome handed to the card.
                        if (getattr(self, "_session_meta", {}).get(session_ref) or {}).get("recipient") == "worker":
                            text_parts.clear()
                    if event.kind == EngineEventKind.TURN_DELTA and event.delta_text:
                        text_parts.append(event.delta_text)
                        if on_delta is not None:
                            try:
                                # A model that speaks, calls a tool, then speaks
                                # again has produced two messages, but
                                # ``agent_message_chunk`` carries no message
                                # identity at all (see
                                # engine.session_manager._translate_update), so
                                # the two arrive as one unbroken run of chunks
                                # and render as "...verify the change.The Cortex
                                # draft was created...". The tool call between
                                # them IS the observable boundary: a delta that
                                # is the first one after a tool call
                                # starts a new message, and gets a blank line
                                # ahead of it. Never before the turn's own first
                                # delta -- the caller owns the separation
                                # between turns (see
                                # acp.orchestrating_runtime._continuation_loop's
                                # rebound emit_update).
                                if _saw_tool_boundary[0] and text_parts[:-1]:
                                    on_delta("\n\n")
                                _saw_tool_boundary[0] = False
                                on_delta(event.delta_text)
                            except Exception:  # noqa: BLE001 - best-effort, never fails the turn
                                pass
                    elif event.kind == EngineEventKind.TOOL_CALL_OBSERVED:
                        if event.tool_call_status in ("completed", "failed"):
                            _open_tools.discard(event.tool_call_ref)
                        elif event.tool_call_ref:
                            _open_tools.add(event.tool_call_ref)
                        # The same observation the durable evidence below
                        # records, also handed to the caller so a person can
                        # watch what the model is doing while it does it --
                        # ref/kind/title/status only, exactly the portable
                        # fields EngineEvent carries (never raw arguments,
                        # output or result; see its own docstring for why
                        # those are deliberately not portable concepts).
                        if on_tool_call is not None and event.tool_call_ref:
                            try:
                                on_tool_call(
                                    event.tool_call_ref, event.tool_call_kind,
                                    event.tool_call_title, event.tool_call_status,
                                    event.tool_call_locations, event.tool_call_content,
                                    event.tool_call_raw_input, event.tool_call_raw_output,
                                )
                            except Exception:  # noqa: BLE001 - best-effort, never fails the turn
                                pass
                        # T017: durable evidence, never raw tool output/args --
                        # see delegation.evidence module docstring.
                        if self._store is not None and task_id is not None:
                            from . import evidence as delegation_evidence

                            delegation_evidence.record_tool_result_observed(
                                self._store, task_id, attempt_id, event.tool_call_ref,
                                event.tool_call_kind, event.tool_call_title, event.tool_call_status,
                            )
                    elif event.kind == EngineEventKind.DISPLAY_UPDATE:
                        # Shown, never interpreted: this adapter attaches no
                        # meaning to a thought or a plan, it only makes sure the
                        # person watching sees what the engine said it is doing.
                        if on_display_update is not None and event.display_update:
                            try:
                                on_display_update(dict(event.display_update))
                            except Exception:  # noqa: BLE001 - best-effort, never fails the turn
                                pass
                    elif event.kind == EngineEventKind.COMPACTION_SIGNAL:
                        if self._store is not None:
                            self._observe_compaction_signal(manager, session_ref, event)
                    elif event.kind == EngineEventKind.USAGE_UPDATE:
                        # T029: the engine's own most-recent usage_update for
                        # this call (cumulative counters, per
                        # engine.session_manager) -- attributed below once the
                        # call finishes, whether it succeeds or fails.
                        # A context-only update carries no counts and must not
                        # erase counts already reported for this call.
                        if event.usage_values or usage_basis is None:
                            usage_basis = event.usage_basis
                            usage_values = dict(event.usage_values or {})
                        if event.display_update:
                            self._observe_context_window(session_ref, event.display_update.get("size"))
                            self._observe_context_used(session_ref, event.display_update.get("used"))
                        # The context fill is shown like any display update.
                        if on_display_update is not None and event.display_update:
                            try:
                                on_display_update(dict(event.display_update))
                            except Exception:  # noqa: BLE001 - best-effort, never fails the turn
                                pass
                    elif event.kind == EngineEventKind.ERROR:
                        # T017: a yielded EngineEventKind.ERROR (transport_eof,
                        # timeout_alive -- see engine.session_manager's
                        # _prompt_turn_locked) must never be silently treated as
                        # a normal end_turn; the caller (OrchestratingRuntime)
                        # decides the durable outcome, never this adapter.
                        raise EngineTurnFailed(event.error_message or "engine_error")
                    elif event.kind == EngineEventKind.TURN_END:
                        saw_turn_end = True
                        stop_reason = event.stop_reason or StopReason.END_TURN
                if _park_unconfirmed.is_set() or (_parked.is_set() and not saw_turn_end):
                    raise EngineTurnFailed("worker_wait_cancel_unconfirmed")
                if (_parked.is_set() and not cancel_event.is_set()
                        and stop_reason == StopReason.CANCELLED):
                    # Only a real cancelled TURN_END proves the old turn finished.
                    # A poisoned/failed manager raises instead and cannot continue.
                    stop_reason = StopReason.END_TURN
                    break
                if (_nudged.is_set() and not cancel_event.is_set()
                        and stop_reason == StopReason.CANCELLED):
                    # The engine answered the nudge's cancel, so it is alive:
                    # wake it in the same session and keep waiting.
                    _stall_limit[0] *= 2
                    _nudged.clear()
                    stop_reason = StopReason.END_TURN
                    turn_message, turn_blocks = self._STALL_NUDGE_PROMPT, None
                    _bank_turn_usage()
                    continue
                take_refusal = getattr(manager, "take_permission_refusal", None)
                if (not cancel_event.is_set() and stop_reason == StopReason.CANCELLED
                        and refusal_continuations < self._MAX_REFUSAL_CONTINUATIONS
                        and take_refusal is not None and take_refusal(session_ref) is True):
                    refusal_continuations += 1
                    stop_reason = StopReason.END_TURN
                    turn_message, turn_blocks = self._REFUSAL_CONTINUE_PROMPT, None
                    _bank_turn_usage()
                    continue
                break
            call_failed = False
        except EngineProtocolError as error:
            # The engine refused this turn for the route itself (exhausted
            # or rate-limited account, model unavailable -- only its own
            # structured error fields decide, see
            # engine.failure_classification.route_failure): record the
            # route so the node's retry is routed elsewhere, and fail the
            # turn under a name that says so. Any other protocol error
            # propagates exactly as before.
            failure = failure_classification.route_failure(error)
            route = self._attempt_routes.get(attempt_id)
            if failure is None or route is None or self._store is None or task_id is None:
                raise
            self._record_route_failure(failure, route[0], route[1], task_id, attempt_id)
            raise EngineTurnFailed(f"route_unavailable:{failure[0]}") from error
        except PortableError as error:
            if error.code == "engine_session_unusable":
                # session_manager marks a session "poisoned" after an
                # unfinished cancellation race (see its module docstring);
                # a poisoned session can never usefully be prompted again.
                raise EngineTurnFailed("poisoned") from error
            raise
        finally:
            # EXT-01 #4 fix: stop the watcher promptly once this call is
            # done for any reason (normal end, failure, or already handled
            # its own cancel) -- it is a bounded, short poll (never longer
            # than ``_CANCEL_WATCH_POLL_SECONDS``), so this join never waits
            # on anything unbounded, and it never leaves a stray thread
            # polling a ``cancel_event`` no one will ever set again.
            _stop_watch.set()
            watcher.join(timeout=self._CANCEL_WATCH_POLL_SECONDS * 4)
            # Close explicitly: abandonment may settle the exact wire call
            # within the manager's existing cleanup bound. Its final response
            # can contain usage that never reached this event iterator.
            if turn_events is not None:
                close = getattr(turn_events, "close", None)
                if close is not None:
                    close()
            take_usage = getattr(manager, "take_abandoned_usage", None)
            final_usage = take_usage(session_ref) if take_usage is not None else None
            if final_usage:
                usage_basis = UsageBasis.ENGINE_REPORTED
                usage_values = final_usage
            # T029/G7: exactly one usage_records row per prompt() call --
            # retry/fallback hops and recovery/clarification deliveries
            # each call prompt() again and therefore each get their own
            # row, whether this call succeeded or raised. basis='unknown'
            # (never zero/free) when no usage_update event was ever seen in
            # *any* inner turn of this call. A stall-nudge or refusal
            # continuation runs more than one inner turn inside this same
            # call; each turn's final usage was already folded into
            # accumulated_usage_values/accumulated_usage_basis at its own
            # turn boundary (see _bank_turn_usage), so banking the still-open
            # final turn here and recording the accumulators sums every
            # inner turn instead of keeping only the last one's count.
            _bank_turn_usage()
            if any_turn_missing_usage and accumulated_usage_basis is not None:
                # P1 fix: at least one inner turn of this call reported no
                # usage at all while another did -- the combined total is
                # therefore incomplete and must never be recorded as fully
                # 'engine_reported'. The basis vocabulary stays closed (no
                # CHECK-constraint migration): 'unknown', never zero/free,
                # same as a call that never saw any usage_update at all.
                accumulated_usage_basis = UsageBasis.UNKNOWN
            if self._store is not None and task_id is not None:
                from ..observability import usage as observability_usage

                # H3 fix: a call that raised (e.g. EngineTurnFailed from a
                # transport closed by the runtime's own post-acceptance
                # release) after this exact attempt already published its
                # worker_result is the runtime ending an already-successful
                # call, not a failed one -- retain its distinct stop reason
                # even when the engine never supplied a usage figure.
                recorded_failed = call_failed
                recorded_stop_reason = stop_reason.value if not call_failed else None
                if call_failed and attempt_id is not None:
                    with self._store.connection() as db:
                        already_reported = db.execute(
                            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
                            "AND event_type='worker_result' LIMIT 1",
                            (task_id, attempt_id),
                        ).fetchone() is not None
                    if already_reported:
                        recorded_failed = False
                        recorded_stop_reason = "released_after_result"
                        # Keep the original closed basis vocabulary so existing
                        # stores need no CHECK-constraint migration. The stop
                        # reason and failed=False distinguish this release;
                        # real reported usage is kept, never overwritten.
                        if accumulated_usage_basis != UsageBasis.ENGINE_REPORTED:
                            accumulated_usage_basis = UsageBasis.UNKNOWN
                observability_usage.record_call_usage(
                    self._store, task_id=task_id, attempt_id=attempt_id, engine_session_ref=session_ref,
                    usage_basis=accumulated_usage_basis, usage_values=accumulated_usage_values,
                    stop_reason=recorded_stop_reason, failed=recorded_failed,
                )
            # Teardown waits for recording, not merely for the wire response.
            prompt_done.set()
            with self._lock:
                if self._prompt_done.get(attempt_id) is prompt_done:
                    self._prompt_done.pop(attempt_id, None)
        return PromptOutcome(stop_reason, "".join(text_parts))
