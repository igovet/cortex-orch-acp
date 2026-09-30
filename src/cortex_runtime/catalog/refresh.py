"""Prompt-free, on-demand engine catalog refresh.

The user's own request (translated, and later simplified -- verbatim,
2026-09-18: "не делай суточное, просто обновляй каждый раз при новой
ссессии cortex", i.e. "don't make it daily, just refresh every time on a
new Cortex session"): "Why can't we start a session, refresh the
model/effort list, and close the session." That is exactly what
:func:`perform_refresh` does: ``initialize`` + ``session/new`` against one
connection's real engine, read the ``configOptions`` payload it returned,
persist it as a real ``DISCOVERY``-provenance :class:`~.model.CatalogSnapshot`
(see ``catalog.discovery.build_catalog``), and terminate the process. **No
``session/prompt`` is ever sent** -- this module never imports or
constructs an ``engine.session_manager.EngineSessionManager`` at all (its
``prompt_turn`` method must never even be reachable from here); it drives
``engine.process.EngineProcess`` + ``engine.client.EngineClient`` directly,
and uses only initialization, session creation and (for Claude) model selection
via ``session/set_config_option`` to observe each model's actual effort selector.
The initial model is restored before the disposable session is closed. This structural
property is asserted by
``tests/test_catalog_refresh.py::test_refresh_never_sends_prompt`` against a
fake engine that fails the test outright if it ever receives a
``session/prompt`` call.

**"Every new Cortex session" is already covered without this module**: a
real Cortex session actually starts an engine the moment its coordinator
opens (``delegation.live_adapters.EngineSessionManagerAdapter.open``), and
that already captures the real, just-observed ``configOptions`` into a
fresh catalog snapshot on every such open (see
``EngineSessionManagerAdapter._capture_catalog_observation``) -- for free,
with no second engine process. This module is not wired to run
automatically anywhere; it exists only as the explicit, on-demand
:func:`perform_refresh` entry point (``scripts/cortex_acp_setup.py``'s
``refresh-catalog`` step) for a connection that has no catalog yet, or
whose catalog a caller wants refreshed without waiting for/running a real
task. (An earlier revision of this module also carried an
``is_due``/``refresh_if_due`` cadence-gate scheduler for a since-abandoned
"refresh once a day" design; nothing ever called it automatically, so it
was removed rather than left as dead weight -- see T035 FINDING-05's fresh
report for the full reasoning.)

Failure is never fatal: an unauthenticated/missing/crashed/hanging engine
leaves any previously cached snapshot completely untouched and is recorded
as a failed, non-fatal :class:`RefreshOutcome` with a reason -- see
:func:`perform_refresh`'s own docstring. The engine launch itself is
bounded by ``timeout`` (default 30s, generous next to this machine's own
~1.6s real ``initialize``+``session/new`` latency for claude-agent-acp) and
``EngineProcess.release`` reaps the process within a bounded grace period
either way.

Selection is never blocked by this module: nothing in
``acp.config_options.selectable_entries_from_snapshots`` (or anywhere else
in the selection path) calls this module at all -- it only ever reads
whatever snapshot is already cached.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone

from ..portable.errors import PortableError
from . import store as catalog_store
from .discovery import build_catalog, parse_observation

DEFAULT_REFRESH_TIMEOUT_SECONDS = 30.0
_ACP_AUTHENTICATION_REQUIRED_CODE = -32000
_AUTHENTICATION_REQUIRED_TEXT = "authentication required"


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


@dataclass(frozen=True)
class RefreshOutcome:
    """The result of one :func:`perform_refresh` call.

    ``effort_enrichment`` (T035 follow-up, additive/optional -- defaults to
    ``None`` and every existing positional construction of this dataclass
    is unaffected) records, for a codex-acp connection only, whether the
    per-model reasoning-effort ceiling (see :func:`_codex_effort_ceiling`)
    was actually applied on top of the engine's own flat advertised set, or
    why it was skipped -- ``None`` for any non-codex-acp connection."""

    connection_id: str
    attempted: bool
    success: bool
    reason: str | None
    snapshot_id: str | None
    at: str
    effort_enrichment: str | None = None


def _read_catalog_from_engine(record, workspace_root, *, environ=None, timeout: float, install=None) -> list:
    """Read options and Claude's model-dependent effort selectors, without prompts."""
    from ..engine.client import EngineClient
    from ..engine.correlation import CorrelationTables
    from ..engine.process import EngineProcess

    # OpenCode is only ever launched jailed (engine.process); a catalog read
    # needs no project write, so its jail grants none.
    jail = {"scopes": [], "readonly": []} if record.engine == "opencode" else None
    process = EngineProcess(record, workspace_root, environ=environ, install=install, jail=jail)
    correlation = CorrelationTables()
    client = EngineClient(
        process,
        correlation=correlation,
        on_update=lambda params: None,
        on_client_request=lambda method, params, request_id: None,
        logs=process.logs,
    ).start()
    try:
        initialized = client.call("initialize", dict(protocolVersion=1, clientCapabilities={}), timeout=timeout)
        if record.engine == "opencode":
            from ..connections.discovery import is_qualified

            info = initialized.get("agentInfo") if isinstance(initialized, dict) else None
            if not isinstance(info, dict) or not is_qualified("opencode", info.get("version")):
                raise RuntimeError("opencode_version_unqualified: expected OpenCode 2.x.x")
        result = client.call("session/new", dict(cwd=workspace_root, mcpServers=[]), timeout=timeout)
        options = result.get("configOptions") if isinstance(result, dict) else None
        if record.engine in ('claude-agent-acp', 'opencode') and options:
            from .model_efforts import observe_model_efforts
            levels = observe_model_efforts(client.call, result['sessionId'], options, timeout=timeout)
            return parse_observation(options).with_model_thought_levels(levels)
        return options or []
    finally:
        try:
            client.close()
        finally:
            process.release(grace=min(5.0, timeout))


def _resolve_install_for_refresh(record, environ):
    """The verified install for ``record``, derived from its own
    ``launch_command`` (see ``engine.install_resolution``), or ``None`` if
    it cannot be resolved/verified for any reason -- always non-fatal here:
    a refresh with no verifiable install simply skips the per-model effort
    enrichment below and keeps every model's flat, engine-advertised set
    (today's behaviour), it never fails the refresh itself."""
    from ..engine import install_resolution

    try:
        return install_resolution.resolve_verified_install(record, environ)
    except (install_resolution.InstallUnresolvable, PortableError):
        return None


def _is_authentication_required_error(error: BaseException) -> bool:
    """Return whether *error* is the ACP authentication-required response.

    The JSON-RPC code is the authoritative signal.  The exact, bounded text
    fallback also covers older/partially adapted engines that construct an
    ``EngineProtocolError`` with only the standard phrase.  We deliberately
    inspect neither ``data`` nor arbitrary exception text: those fields may
    contain credentials, provider details, or other engine-controlled data.
    """
    from ..engine.client import EngineProtocolError

    if not isinstance(error, EngineProtocolError):
        return False
    if getattr(error, "code", None) == _ACP_AUTHENTICATION_REQUIRED_CODE:
        return True
    code = getattr(error, "code", None)
    message = getattr(error, "message", None)
    # A text-only fallback is accepted only when the other protocol field is
    # absent.  A different numeric code paired with an engine-authored phrase
    # remains an unknown protocol failure and must stay fail-closed.
    if code is None and isinstance(message, str) and message.strip().casefold() == _AUTHENTICATION_REQUIRED_TEXT:
        return True
    if message is None and isinstance(code, str) and code.strip().casefold() == _AUTHENTICATION_REQUIRED_TEXT:
        return True
    return False


def _codex_effort_ceiling(record, install, environ, timeout):
    """Best-effort per-model reasoning-effort supplement (T035 follow-up
    "per-model efforts") for a codex-acp connection with a verified
    ``install`` -- ``None`` for any other engine, a missing/unverifiable
    install, or any failure of ``codex debug models`` itself. Never raises,
    never itself an ACP call (see ``catalog.codex_debug_models``'s own
    docstring)."""
    if record.engine != "codex-acp" or install is None:
        return None
    try:
        from ..engine import codex_install
        from . import codex_debug_models

        verified = codex_install.verify_install(install, environ)
        return codex_debug_models.fetch_effort_ceiling(verified.bundled_codex_binary, timeout=timeout)
    except Exception:  # noqa: BLE001 - purely optional enrichment
        return None


def _opencode_cortex_mode_advertised(observation) -> bool:
    """Models alone cannot establish that Cortex's required agent is usable."""
    from ..engine.opencode_adapter_acp import AGENT

    mode = next((option for option in observation.options if option.id == "mode"), None)
    return mode is not None and mode.type == "select" and AGENT in dict(mode.select_options)


def perform_refresh(
    record, workspace_root, *, environ=None, timeout: float = DEFAULT_REFRESH_TIMEOUT_SECONDS, install=None,
) -> RefreshOutcome:
    """One prompt-free catalog refresh attempt for ``record`` -- never
    raises. Every failure (unauthenticated/missing/crashed/hanging engine,
    an empty/malformed ``configOptions`` payload) is caught here and
    reported as a failed, non-fatal :class:`RefreshOutcome` that leaves any
    previously cached snapshot for this connection completely untouched
    (this function only ever calls ``catalog_store.save_snapshot`` on the
    success path, and ``save_snapshot`` itself never rewrites/deletes an
    existing snapshot file -- see ``catalog.store``'s own docstring).
    """
    at = _now()
    # T035 follow-up ("codex per-model efforts are never enriched in
    # practice"): a caller-supplied ``install`` always wins (existing
    # behaviour, e.g. the qualification probe scripts' own explicit
    # install); otherwise this is the one production path
    # (``scripts/cortex_acp_setup.py``'s ``refresh-catalog`` step) that
    # actually calls this function, and it never built one -- so resolve it
    # here, from the connection's own recorded launch_command, instead of
    # requiring every caller to. Resolution failure is never fatal to the
    # refresh itself (see ``_resolve_install_for_refresh``'s own docstring).
    if install is None:
        install = _resolve_install_for_refresh(record, environ)
    try:
        raw_options = _read_catalog_from_engine(
            record, workspace_root, environ=environ, timeout=timeout, install=install,
        )
    except Exception as error:  # noqa: BLE001 - an engine launch/call failure is never fatal
        if _is_authentication_required_error(error):
            return RefreshOutcome(record.id, True, False, "authorization_required", None, at)
        return RefreshOutcome(record.id, True, False, f"engine_unreachable:{type(error).__name__}", None, at)
    if not raw_options:
        return RefreshOutcome(record.id, True, False, "no_config_options_advertised", None, at)
    try:
        from .discovery import Observation
        observation = raw_options if isinstance(raw_options, Observation) else parse_observation(raw_options)
    except PortableError as error:
        return RefreshOutcome(record.id, True, False, f"invalid_observation:{error.code}", None, at)
    if not observation.model_options():
        return RefreshOutcome(record.id, True, False, "no_model_options_advertised", None, at)
    if record.engine == "opencode" and not _opencode_cortex_mode_advertised(observation):
        return RefreshOutcome(record.id, True, False, "opencode_cortex_mode_not_advertised", None, at)
    ceiling = _codex_effort_ceiling(record, install, environ, timeout)
    effort_enrichment = None
    if record.engine == "codex-acp":
        if ceiling:
            observation = observation.with_model_thought_levels(ceiling)
            effort_enrichment = "applied"
        elif install is None:
            effort_enrichment = "skipped:no_verifiable_install"
        else:
            effort_enrichment = "skipped:codex_debug_models_unavailable"
    try:
        snapshot = build_catalog({record.id: record}, {record.id: observation})
        catalog_store.save_snapshot(snapshot, environ)
    except PortableError as error:
        return RefreshOutcome(record.id, True, False, f"invalid_catalog:{error.code}", None, at)
    return RefreshOutcome(record.id, True, True, None, snapshot.id, at, effort_enrichment)
