"""The T026 claude-agent-acp adapter: launch mapping, option mapping, evidence.

Mirrors ``engine.codex_adapter_acp`` (T013) for the second, non-Codex
engine, proving the portable runtime is genuinely engine-independent (see
that module's own docstring for the shared design this one repeats
verbatim where nothing engine-specific differs). This module never imports
``cortex_runtime.codex_adapter`` (the legacy native Codex ``_meta``
adapter), ``server.py``, ``hooks.py``, ``host_source.py``,
``marketplace_bootstrap.py``, or anything under ``.codex-plugin`` -- see
``tests/test_engine_codex_no_native_fallback.py``, whose module glob
(``engine/*.py``) already covers this file automatically: a
claude-agent-acp configuration failure here
(``ConfigurationRejected``/``PortableError``) never triggers a legacy
native path, silently or otherwise, and no Codex-specific env var or
option id is ever referenced from here.

Everything here is scoped to the pinned release recorded in
``.cortex/acp-migration/T002/CAPABILITY_MATRIX.json`` (observed
``@agentclientprotocol/claude-agent-acp`` 0.78.0, per
``tests/fixtures/catalog/claude_agent_acp_config_options.json``'s fixture
note, itself derived from the real installed
``~/.local/share/zed/external_agents/registry/npx/claude-acp/node_modules/
@agentclientprotocol/claude-agent-acp/dist/`` bundle, lead- and
agent-verified 2026-09-17); nothing here claims a main-branch/newer-release
capability.
"""
from __future__ import annotations

from dataclasses import dataclass

from ..configuration.service import ConfigurationRejected
from ..portable.errors import PortableError, PortableErrorCategory
from . import claude_install

# session/set_config_option option ids observed in the pinned release (see
# tests/fixtures/catalog/claude_agent_acp_config_options.json's fixture
# note, citing dist file:line locations: mode id/category
# session-mode.js:2,40; model id/category session-model.js:179,182; effort
# id session-config-ids.js EFFORT_CONFIG_ID, category session-effort.js:83;
# fast id/category acp-agent.js:6768,6838). "fast" (category
# "model_config") is deliberately never mapped to here -- see module
# docstring and the lead-approved design's "never collaboration_mode or
# fast-mode" (the same excluded category codex-acp's own "fast-mode"
# belongs to).
_CLAUDE_MODEL_OPTION_ID = "model"
_CLAUDE_EFFORT_OPTION_ID = "effort"
_CLAUDE_MODE_OPTION_ID = "mode"

# ExecutionRoute.supported_parameters / revision parameter names (from
# catalog.discovery's _PARAMETER_CATEGORIES and T007/T008's
# thought_level/effort naming) that this adapter recognizes and maps to a
# claude-agent-acp option id. Any other name in supported_parameters (for
# example "model_config", how the observed "fast" boolean is categorized)
# is never folded into a mapping here.
_EFFORT_PARAM_NAMES = ("thought_level", "effort")
_MODE_PARAM_NAME = "mode"

EXECUTION_VERIFICATION_UNAVAILABLE = "unavailable_in_claude_agent_acp_0_78_0"


def _rejected(code: str) -> ConfigurationRejected:
    return ConfigurationRejected(code)


def _find_option(observed_config_options, option_id: str):
    for item in observed_config_options or ():
        if isinstance(item, dict) and item.get("id") == option_id:
            return item
    return None


def _allowed_values(option: dict) -> set | None:
    if option.get("type") == "boolean":
        return {True, False}
    options = option.get("options")
    if not isinstance(options, list):
        return None
    return {choice.get("value") for choice in options if isinstance(choice, dict)}


def _require_supported_option(option_id: str, value, observed_config_options) -> tuple[str, object]:
    option = _find_option(observed_config_options, option_id)
    if option is None:
        raise _rejected(f"claude_acp_option_not_offered:{option_id}")
    allowed = _allowed_values(option)
    if not allowed or value not in allowed:
        raise _rejected(f"claude_acp_option_value_unsupported:{option_id}")
    return (option_id, value)


def claude_option_mapping(revision, observed_config_options) -> list[tuple[str, object]]:
    """``ConfigurationRevision`` -> ``session/set_config_option`` pairs.

    Mirrors ``codex_adapter_acp.codex_option_mapping`` exactly, against the
    pinned claude-agent-acp 0.78.0 ``configOptions`` shape instead:

    - ``model`` is always required and always mapped from
      ``revision.route.model`` (values are catalog-discovered per
      connection; this module never invents or hardcodes a real vendor
      model name -- see the fixture note's "placeholder" caveat).
    - ``effort`` is mapped only when the revision's
      ``supported_parameters`` includes a ``"thought_level"`` or
      ``"effort"`` entry (T007/T008 naming); its value must be one of the
      pinned release's observed ``effort`` options (``"default"``/
      ``"medium"`` per the fixture note -- never an invented effort name).
    - ``mode`` is mapped only when the revision's ``supported_parameters``
      includes a ``"mode"`` entry, and only when that value is one of the
      pinned release's observed ``mode`` options (``"default"``/
      ``"acceptEdits"``/``"plan"``/``"auto"``/``"bypassPermissions"``).
    - ``fast`` (category ``"model_config"``) is never mapped, regardless of
      what a revision's parameters contain -- the same excluded category as
      codex-acp's ``"fast-mode"``, per the lead-approved design.

    Every pair is pre-checked against ``observed_config_options`` before
    this function returns, exactly like ``codex_option_mapping``; see that
    function's own docstring for the shared partial-application caveat
    ``EngineSessionManager.configure`` itself re-checks against the latest
    observed options mid-call.
    """
    mapping: list[tuple[str, object]] = [
        _require_supported_option(_CLAUDE_MODEL_OPTION_ID, revision.route.model, observed_config_options),
    ]
    parameters = dict(revision.route.supported_parameters)
    for name in _EFFORT_PARAM_NAMES:
        if name in parameters:
            mapping.append(
                _require_supported_option(_CLAUDE_EFFORT_OPTION_ID, parameters[name], observed_config_options)
            )
            break
    if _MODE_PARAM_NAME in parameters:
        mapping.append(
            _require_supported_option(_CLAUDE_MODE_OPTION_ID, parameters[_MODE_PARAM_NAME], observed_config_options)
        )
    return mapping


def build_claude_launch(connection_record, install: claude_install.ClaudeAcpInstall, environ: dict[str, str] | None = None):
    """``(launch_command, pinned_engine_vars)`` for a claude-agent-acp connection.

    ``install`` is re-verified fresh here (never trusted from a
    caller-held reference without re-checking -- see
    ``claude_install.verify_install``). ``connection_record.launch_command``,
    if already set, must equal the verified install's own launch command; a
    mismatch is refused rather than silently overridden by the verified
    value.

    Unlike ``codex_adapter_acp.build_codex_launch``, ``pinned_engine_vars``
    is always the empty dict here: this pinned release bundles no separate
    spawned binary to pin an env var to (see ``claude_install``'s module
    docstring) -- this connection's own ``CLAUDE_CONFIG_DIR`` isolation
    variable is already set generically by
    ``connections.environment.build_process_env`` for every
    ``claude-agent-acp`` connection, with no per-launch escape hatch
    required. Callers must never pass this function's ``pinned_engine_vars``
    return value to ``connections.environment.build_process_env`` (that
    parameter is validated as accepted only for ``record.engine ==
    "codex-acp"``, per that module's own guarantee 6) -- passing ``None``
    (the default) is always correct for a claude-agent-acp launch.
    """
    if connection_record.engine != "claude-agent-acp":
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="unsupported_engine",
            remediation="build_claude_launch only applies to connections with engine 'claude-agent-acp'.",
            retry_eligible=False,
            field="engine",
        )
    verified = claude_install.verify_install(install, environ)
    launch_command = claude_install.launch_command_for(verified)
    if connection_record.launch_command is not None and tuple(connection_record.launch_command) != tuple(launch_command):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="engine_launch_command_mismatch",
            remediation=(
                "This connection's launch_command does not match claude_install.launch_command_for() of "
                "the verified install; re-onboard the connection against the pinned install rather than "
                "launching a different command than the one it names."
            ),
            retry_eligible=False,
            field="launch_command",
        )
    pinned_engine_vars: dict[str, str] = {}
    return launch_command, pinned_engine_vars


@dataclass(frozen=True)
class ClaudeEvidenceStatus:
    """Distinct evidence fields for one applied configuration revision.

    Mirrors ``codex_adapter_acp.CodexEvidenceStatus`` field-for-field; see
    that class's own docstring for what each field does and does not prove.

    ``execution_verified`` is always exactly
    ``EXECUTION_VERIFICATION_UNAVAILABLE`` for this pinned release: the
    installed claude-agent-acp 0.78.0 surface gives no statically-verifiable,
    non-model-authored signal that a specific turn actually executed under a
    specific confirmed model/effort/mode (a ``usage_update`` event reports
    token counts and cost per ``usage-markdown.js``, never a confirmed model
    identity, and a model's own self-report of "which model I am" is never
    treated as evidence anywhere in this runtime). This status is never
    upgraded by observing a ``usage_update``/turn-end event.
    """

    selected: str
    configuration_applied: str
    execution_verified: str = EXECUTION_VERIFICATION_UNAVAILABLE

    def __post_init__(self) -> None:
        if not isinstance(self.selected, str) or not self.selected:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="invalid_field",
                remediation="'selected' must be a non-empty ConfigurationRevision id.",
                retry_eligible=False, field="selected",
            )
        if not isinstance(self.configuration_applied, str) or not self.configuration_applied:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="invalid_field",
                remediation="'configuration_applied' must be a non-empty evidence_ref string.",
                retry_eligible=False, field="configuration_applied",
            )
        if self.execution_verified != EXECUTION_VERIFICATION_UNAVAILABLE:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="invalid_field",
                remediation=(
                    f"'execution_verified' must be exactly {EXECUTION_VERIFICATION_UNAVAILABLE!r} for this "
                    "pinned release; it is never inferred from a usage_update or any other in-band signal."
                ),
                retry_eligible=False, field="execution_verified",
            )


def build_claude_evidence_status(revision_id: str, evidence_ref: str) -> ClaudeEvidenceStatus:
    """The only constructor for :class:`ClaudeEvidenceStatus` -- callers
    never build one by hand, so ``execution_verified`` can never be set to
    anything but the pinned-release constant above."""
    return ClaudeEvidenceStatus(selected=revision_id, configuration_applied=evidence_ref)


def build_first_prompt_blocks(instructions_text: str, user_blocks: list) -> list[dict]:
    """Prepend one leading ``{"type": "text", ...}`` instructions block.

    A legacy leading-block helper with the same prompt-block shape:
    any ACP agent implementing the base ``session/prompt`` content-blocks
    array accepts this, so it is never itself proof of a *system-level*
    instructions channel on its own. **Not** the channel this pinned
    release's registered delivery mechanism actually uses: see
    ``engine.instruction_delivery_registry.ClaudeAgentAcp0_78Delivery``,
    which is QUALIFIED via the genuinely stronger, session/new-level
    ``_meta.systemPrompt`` channel instead (``build_claude_session_new_meta``
    below; dist evidence at ``acp-agent.js:5887-5901``, honored through
    ``@anthropic-ai/claude-agent-sdk``'s own ``query()`` into the spawned
    CLI's control protocol -- see that class's own docstring for the full
    chain). This function remains available as the same base-protocol
    fallback codex-acp uses, for a caller/future engine that has no
    session/new-level channel of its own.
    """
    if not isinstance(instructions_text, str) or not instructions_text.strip():
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation="'instructions_text' must be a non-empty string.",
            retry_eligible=False, field="instructions_text",
        )
    if not isinstance(user_blocks, list):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation="'user_blocks' must be a list of prompt content blocks.",
            retry_eligible=False, field="user_blocks",
        )
    leading_block = {"type": "text", "text": instructions_text}
    return [leading_block, *user_blocks]


def claude_native_subagent_restriction_meta() -> dict:
    """Restrict native subagents and MCP sources before a session starts.

    The installed claude-agent-acp 0.78.0 forwards
    ``_meta.claudeCode.options.disallowedTools`` into its SDK query
    (``dist/acp-agent.js:5910-5918,6065``). The bundled SDK 0.3.270
    sends these names as ``--disallowedTools`` to Claude Code; its type
    contract says they are removed from the model context and denied.
    ``TaskCreate`` and its sibling plan tools are distinct from the
    native subagent launch tools named ``Agent`` and ``Task``.

    The same installed adapter spreads ``_meta.claudeCode.options`` into
    the SDK query (``dist/acp-agent.js:6021-6036``). Its bundled SDK 0.3.270
    maps ``strictMcpConfig`` to ``--strict-mcp-config`` and documents that
    only explicitly supplied ``mcpServers`` are loaded, excluding native
    user/project/plugin MCP configuration. Keep the production
    ``CLAUDE_CONFIG_DIR`` for native-account authentication; the restriction
    applies to this session's MCP discovery, not its account state.
    """
    return {"claudeCode": {"options": {
        "disallowedTools": ["Agent", "Task"],
        "strictMcpConfig": True,
    }}}


def build_claude_session_new_meta(instructions_text: str) -> dict:
    """``_meta`` payload for ``session/new`` appending Cortex instructions.

    The real, statically-confirmed claude-agent-acp 0.78.0 instructions
    channel (dist ``acp-agent.js:5887-5901``, lead- and agent-verified
    2026-09-17): ``session/new``'s own ``params._meta.systemPrompt`` is read
    as either a plain string (a full override of the SDK's
    ``{"type": "preset", "preset": "claude_code"}`` default) or an object
    (``{"append": ..., "excludeDynamicSections": ...}``-shaped, forwarded
    verbatim except ``type``/``preset``, which the adapter always locks to
    ``"preset"``/``"claude_code"``). Use that append/object form to retain
    the engine's preset while adding this role's instructions.

    This is the real, code-confirmed, QUALIFIED channel
    ``engine.instruction_delivery_registry.ClaudeAgentAcp0_78Delivery``
    actually uses (its own ``plan()`` calls this function) -- wired end to
    end into the real production path via
    ``delegation.live_adapters._session_new_meta_for``/``open()`` and
    ``engine.session_manager.EngineSessionManager.create_session``'s
    additive ``session_new_meta`` parameter, which sends this payload as
    that ``session/new`` call's own wire-level ``_meta`` field -- never a
    paper-only registry classification (see
    ``tests/test_engine_claude_coordinator_live_session.py``, which
    observes the real dumped wire payload). Still not live-executed: no
    real turn has run through this channel, so nothing here confirms the
    spawned CLI actually *honors* the delivered text as opposed to merely
    receiving it (see T035/T036's mandatory live gate).
    """
    if not isinstance(instructions_text, str) or not instructions_text.strip():
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation="'instructions_text' must be a non-empty string.",
            retry_eligible=False, field="instructions_text",
        )
    return {
        "systemPrompt": {"append": instructions_text},
        **claude_native_subagent_restriction_meta(),
    }
