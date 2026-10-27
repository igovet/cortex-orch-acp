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

Supported instruction-delivery releases are 0.78.0, 0.87.0, 0.88.0 and 0.97.0
(current fallback), each inspected independently; see instruction_delivery_registry.
0.97.0 bundles Claude Agent SDK 0.3.310 and ACP SDK 2.0.2, while retaining
ACP v1 and the model/effort/mode option ids. Model names and effort choices
remain session-observed, not inferred from the package version.
"""
from __future__ import annotations

import json
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

EXECUTION_VERIFICATION_UNAVAILABLE = "unavailable_in_claude_agent_acp"


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
    values = set()
    for choice in options:
        if not isinstance(choice, dict):
            continue
        # ACP select options can be grouped (0.87.0 accepts both shapes).
        nested = choice.get("options")
        if isinstance(nested, list):
            values.update(item.get("value") for item in nested if isinstance(item, dict))
        elif "value" in choice:
            values.add(choice["value"])
    # 0.87.0 explicitly permits an out-of-picker current model. It is an
    # observed identity, not permission to invent another model or effort.
    if option.get("id") == _CLAUDE_MODEL_OPTION_ID and isinstance(option.get("currentValue"), str):
        values.add(option["currentValue"])
    return values


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
    observed claude-agent-acp ``configOptions`` shape (0.78.0/0.87.0/0.88.0):

    - ``model`` is always required and always mapped from
      ``revision.route.model`` (values are catalog-discovered per
      connection; this module never invents or hardcodes a real vendor
      model name -- see the fixture note's "placeholder" caveat).
    - ``effort`` is mapped only when the revision's
      ``supported_parameters`` includes a ``"thought_level"`` or
      ``"effort"`` entry (T007/T008 naming); its value must be one of the
      session's observed ``effort`` options. 0.87.0 derives these from
      SDK supportedEffortLevels; "default" may be absent with recommended
      values enabled. No model family or effort is invented here.
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
    # A model the engine's SDK does not know has no effort option to set at
    # all; the catalog routes its effort through session/new's _meta instead
    # (see SDK_EFFORTS and catalog.discovery._with_sdk_efforts).
    via_meta = parameters.get(EFFORT_CHANNEL_PARAM) == SESSION_NEW_META_CHANNEL
    for name in _EFFORT_PARAM_NAMES:
        if name in parameters:
            if not via_meta:
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

    ``execution_verified`` remains EXECUTION_VERIFICATION_UNAVAILABLE.
    0.87.0 can attach a model to usage updates, unlike the original 0.78.0
    evidence basis. Cortex does not bind that metadata to an exact
    confirmed model/effort/mode for a turn, so it cannot certify execution.
    Configuration application, usage and a model's self-report never
    upgrade this status.
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


# Native tools that start another agent. The owner excluded native subagents.
CLAUDE_SUBAGENT_TOOLS = ("Agent", "Task", "Workflow", "TeamCreate", "TeamDelete", "SendMessage",
                         # Deferred wake-ups need an interactive session to
                         # fire; in a Cortex turn the model waits on one, ends
                         # its turn, and never resumes.
                         "ScheduleWakeup", "Monitor", "CronCreate", "CronDelete", "CronList", "RemoteTrigger")

# The Cortex gateway supplies scoped file access, code discovery and command
# receipts. Keep Claude's native alternatives out of Cortex sessions so they
# cannot bypass the workflow while the corresponding MCP tools remain available.
CLAUDE_GATEWAY_ALTERNATIVES = ("Bash", "Read", "Edit", "Write", "Grep", "Glob", "Skill", "NotebookEdit")

# Native lifecycle/notification tools whose outcome the Cortex runtime, not
# the model, already owns: EnterWorktree/ExitWorktree bypass the runtime's own
# workspace scope and git-worktree handling; EnterPlanMode/ExitPlanMode are
# Claude Code's own plan-mode gate, superseded by Cortex's own plan/execution
# lifecycle; TaskStop is the engine's native task-stop, superseded by Cortex's
# own cancellation and completion path; PushNotification reaches the host
# outward (desktop/system notifications) outside any Cortex-mediated channel.
# WebFetch/WebSearch stay available -- they are ordinary read-only tools, not
# lifecycle/notification surfaces.
CLAUDE_NATIVE_LIFECYCLE_TOOLS = ("EnterWorktree", "ExitWorktree", "PushNotification",
                                 "EnterPlanMode", "ExitPlanMode", "TaskStop")


# A model claude-agent-acp's bundled SDK does not know (an id an
# ``availableModels`` allowlist names, e.g. ``claude-opus-5[1m]``) keeps its
# bare id as its display name and gets no ``effort`` config option at all --
# yet the SDK still sends that session's effort for it. Observed against the
# installed 0.81.0 engine's own bundled CLI, with the request captured
# locally: ``claude-opus-5`` and ``claude-fable-5`` carry
# ``output_config.effort`` (low/high/max), while ``haiku`` -- a model the SDK
# does know and reports no efforts for -- carries none. So these efforts are
# offered for an unknown model only, and delivered through session/new's
# ``_meta.claudeCode.options.effort`` (acp-agent.js ``userProvidedOptions``)
# instead of the config option the engine never offers.
SDK_EFFORTS: tuple[str, ...] = ("low", "medium", "high", "xhigh", "max")


# The catalog marks such a route with this pair, so a mapping never has to
# re-derive "the SDK does not know this model" from a live session whose
# model option still describes the session's *current* model.
EFFORT_CHANNEL_PARAM = "effort_channel"
SESSION_NEW_META_CHANNEL = "session_new_meta"


def sdk_unknown_model(model: str, model_name: str | None) -> bool:
    """True when the engine gave ``model`` no display name of its own."""
    return bool(model) and (model_name or model) == model


def claude_native_subagent_restriction_meta(
    *, role: str = "worker", effort: str | None = None, native_skills: bool = False,
) -> dict:
    """Build the Claude tool policy for either Cortex role.

    Native shell, file and skill alternatives are disallowed in favor of the
    scoped Cortex gateway and external MCP index. Native subagents stay
    disallowed, along with :data:`CLAUDE_NATIVE_LIFECYCLE_TOOLS` (worktree,
    plan-mode, task-stop and push-notification tools whose outcome the
    runtime, not the model, already owns -- WebFetch/WebSearch stay
    available). The engine jail remains a backstop for any remaining tools.
    The installed adapter forwards ``disallowedTools`` to its SDK; MCP tools
    are not in this list. Native context loading remains at the adapter's
    user/project/local default; the installed SDK's ``managedSettings``
    disables native auto-memory read/write for this session alongside hooks
    (confirmed against the pinned/qualified ``claude-agent-acp`` 0.78.0,
    0.87.0 and 0.88.0 ``Settings.autoMemoryEnabled`` field, forwarded the
    same way ``disableAllHooks`` already is). User/project MCP, hooks and
    plugins are still disabled by their separate options.
    """
    if role not in {"coordinator", "worker"}:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation="'role' must be 'coordinator' or 'worker'.",
            retry_eligible=False, field="role",
        )

    if effort is not None and (not isinstance(effort, str) or effort not in SDK_EFFORTS):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation=f"'effort' must be one of {SDK_EFFORTS}.", retry_eligible=False, field="effort",
        )
    gateway_alternatives = CLAUDE_GATEWAY_ALTERNATIVES
    if native_skills:
        gateway_alternatives = tuple(tool for tool in gateway_alternatives if tool != "Skill")
    options = {"strictMcpConfig": True,
               "plugins": [], "managedSettings": {"disableAllHooks": True, "autoMemoryEnabled": False},
               "disallowedTools": list(CLAUDE_SUBAGENT_TOOLS + CLAUDE_NATIVE_LIFECYCLE_TOOLS + gateway_alternatives)}
    if not native_skills:
        options["skills"] = []
    if effort is not None:
        options["effort"] = effort
    return {"claudeCode": {"options": options}}


# The attached-server note is engine-neutral now; kept importable here.
from .unified_system_prompt import (  # noqa: E402
    CODE_INDEX_SERVER as CODEBASE_MEMORY_SERVER,
    attached_servers_section as external_mcp_instructions,
    with_attached_servers as with_external_mcp_instructions,
)


#: What the unified Cortex prompt replaces that Claude Code would still add on
#: its own with a string system prompt, each through an option or setting of
#: the installed SDK: its native skill listing (``skills: []``), its commit
#: and pull-request attribution reminder and its git workflow instructions
#: (the ``settings`` layer; ``managedSettings`` drops these keys).
CLAUDE_UNIFIED_PROMPT_SETTINGS = {"attribution": {"commit": "", "pr": ""}, "includeGitInstructions": False}


def build_claude_session_new_meta(instructions_text: str, *, role: str = "worker",
                                  effort: str | None = None, base_prompt: str | None = None,
                                  native_skills: bool = False) -> dict:
    """``_meta`` payload for ``session/new`` appending Cortex instructions.

    The statically-confirmed claude-agent-acp 0.78.0/0.87.0/0.88.0 instructions
    channel (dist ``acp-agent.js:5887-5901`` / ``7047-7063`` / ``7091-7107``): ``session/new``'s own ``params._meta.systemPrompt`` is read
    as either a plain string (a full override of the SDK's
    ``{"type": "preset", "preset": "claude_code"}`` default) or an object
    (``{"append": ..., "excludeDynamicSections": ...}``-shaped, forwarded
    verbatim except ``type``/``preset``, which the adapter always locks to
    ``"preset"``/``"claude_code"``). Use that append/object form to retain
    the engine's preset while adding this role's instructions.

    This is the real, code-confirmed, QUALIFIED channel
    the exact registrations in ``engine.instruction_delivery_registry``
    actually use (its own ``plan()`` calls this function) -- wired end to
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
    receiving it (as documented by the adapter's installed capability evidence).

    ``base_prompt`` (the unified Cortex prompt) uses the plain-string form
    instead: base prompt then role instructions become the whole system
    prompt, replacing Claude Code's preset, and the native additions listed
    in :data:`CLAUDE_UNIFIED_PROMPT_SETTINGS` are switched off.
    """
    if not isinstance(instructions_text, str) or not instructions_text.strip():
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation="'instructions_text' must be a non-empty string.",
            retry_eligible=False, field="instructions_text",
        )
    meta = claude_native_subagent_restriction_meta(
        role=role, effort=effort, native_skills=native_skills,
    )
    if base_prompt is None:
        return {"systemPrompt": {"append": instructions_text}, **meta}
    if not isinstance(base_prompt, str) or not base_prompt.strip():
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation="'base_prompt' must be a non-empty string.",
            retry_eligible=False, field="base_prompt",
        )
    options = meta["claudeCode"]["options"]
    options["skills"] = []
    options["settings"] = json.loads(json.dumps(CLAUDE_UNIFIED_PROMPT_SETTINGS))
    return {"systemPrompt": base_prompt.rstrip() + "\n\n" + instructions_text, **meta}
