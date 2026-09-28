"""The T013 codex-acp adapter: launch mapping, option mapping, evidence.

Named ``codex_adapter_acp`` (not ``codex_adapter``) to avoid clashing with
the pre-existing, unrelated legacy module
``cortex_runtime.codex_adapter`` (native Codex MCP ``_meta`` thread-context
parsing -- see that module's own docstring). This module never imports
that one, ``server.py``, ``hooks.py``, ``host_source.py``,
``marketplace_bootstrap.py``, or anything under ``.codex-plugin`` -- see
``tests/test_engine_codex_no_native_fallback.py``: a codex-acp configuration
failure here (``ConfigurationRejected``/``PortableError``) never triggers a
legacy native path, silently or otherwise.

Everything here is scoped to the pinned release recorded in
``.cortex/acp-migration/T002/CAPABILITY_MATRIX.json`` /
``ADR-001b-codex-acp-version-drift.md`` (observed
``@agentclientprotocol/codex-acp`` 1.12.0, per
``tests/fixtures/catalog/codex_acp_config_options.json``'s fixture note);
nothing here claims a main-branch/newer-release capability.
"""
from __future__ import annotations

from dataclasses import dataclass

from ..configuration.service import ConfigurationRejected
from ..portable.errors import PortableError, PortableErrorCategory
from . import codex_install

# session/set_config_option option ids observed in the pinned release (see
# tests/fixtures/catalog/codex_acp_config_options.json's fixture note,
# citing dist/index.js line numbers). "collaboration_mode" and "fast-mode"
# are deliberately never mapped to here -- see module docstring and the
# lead-approved design's "never collaboration_mode or fast-mode".
_CODEX_MODEL_OPTION_ID = "model"
_CODEX_REASONING_EFFORT_OPTION_ID = "reasoning_effort"
_CODEX_MODE_OPTION_ID = "mode"

# ExecutionRoute.supported_parameters / revision parameter names (from
# catalog.discovery's _PARAMETER_CATEGORIES and T007/T008's
# thought_level/effort naming) that this adapter recognizes and maps to a
# codex-acp option id. Any other name in supported_parameters (for example
# "model_config", which is how the observed "fast-mode" boolean is
# categorized) is never folded into a mapping here.
_REASONING_EFFORT_PARAM_NAMES = ("thought_level", "effort")
_MODE_PARAM_NAME = "mode"

EXECUTION_VERIFICATION_UNAVAILABLE = "unavailable_in_codex_acp_1_12_0"


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
        raise _rejected(f"codex_acp_option_not_offered:{option_id}")
    allowed = _allowed_values(option)
    if not allowed or value not in allowed:
        raise _rejected(f"codex_acp_option_value_unsupported:{option_id}")
    return (option_id, value)


def codex_option_mapping(revision, observed_config_options) -> list[tuple[str, object]]:
    """``ConfigurationRevision`` -> ``session/set_config_option`` pairs.

    Every pair is pre-checked against ``observed_config_options`` (this
    engine session's own ``session/new``-returned ``configOptions``, at the
    time this function is called) before this function returns: an
    unsupported model, reasoning effort, or mode raises
    ``ConfigurationRejected`` here, before ``EngineSessionManager.configure``
    sends its *first* wire call for this revision -- so a request that is
    already known-unsupported up front is refused before touching the
    engine at all.

    This precheck alone does **not** guarantee a partial application can
    never happen: the pinned release's own option set may narrow again
    *mid-call*, as a direct effect of an earlier option in the same
    ``configure()`` call actually being applied (observed for
    ``reasoning_effort``, whose offered values depend on the selected
    ``model`` -- see the fixture note this module is derived from).
    ``EngineSessionManager.configure`` itself re-checks each subsequent
    option against the *latest* observed ``configOptions`` (F3, T013
    review) and raises ``engine.session_manager.
    EngineConfigurationPartiallyApplied`` -- never a further wire call, and
    never a plain ``ConfigurationRejected`` -- if a later option in the
    same call has become unsupported after an earlier one already
    succeeded; see that method's own docstring for why a partial
    application is reported distinctly from a clean rejection.

    - ``model`` is always required and always mapped from
      ``revision.route.model``.
    - ``reasoning_effort`` is mapped only when the revision's
      ``supported_parameters`` includes a ``"thought_level"`` or
      ``"effort"`` entry (T007/T008 naming); its value must be one of the
      pinned release's observed ``reasoning_effort`` options (``"none"``/
      ``"medium"`` per the fixture note -- never an invented effort name).
    - ``mode`` is mapped only when the revision's ``supported_parameters``
      includes a ``"mode"`` entry, and only when that value is one of the
      pinned release's observed ``mode`` options.
    - ``collaboration_mode`` and ``fast-mode`` are never mapped, regardless
      of what a revision's parameters contain: they are a distinct,
      unsupported category per the lead-approved design.
    """
    mapping: list[tuple[str, object]] = [
        _require_supported_option(_CODEX_MODEL_OPTION_ID, revision.route.model, observed_config_options),
    ]
    parameters = dict(revision.route.supported_parameters)
    for name in _REASONING_EFFORT_PARAM_NAMES:
        if name in parameters:
            mapping.append(
                _require_supported_option(_CODEX_REASONING_EFFORT_OPTION_ID, parameters[name], observed_config_options)
            )
            break
    if _MODE_PARAM_NAME in parameters:
        mapping.append(
            _require_supported_option(_CODEX_MODE_OPTION_ID, parameters[_MODE_PARAM_NAME], observed_config_options)
        )
    return mapping


def build_codex_launch(connection_record, install: codex_install.CodexAcpInstall, environ: dict[str, str] | None = None):
    """``(launch_command, pinned_engine_vars)`` for a codex-acp connection.

    ``install`` is re-verified fresh here (never trusted from a caller-held
    reference without re-checking -- see ``codex_install.verify_install``).
    ``connection_record.launch_command``, if already set, must equal the
    verified install's own launch command; a mismatch is refused rather
    than silently overridden by the verified value, so a caller can never
    accidentally launch a different binary than the one its own connection
    record names.
    """
    if connection_record.engine != "codex-acp":
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="unsupported_engine",
            remediation="build_codex_launch only applies to connections with engine 'codex-acp'.",
            retry_eligible=False,
            field="engine",
        )
    verified = codex_install.verify_install(install, environ)
    launch_command = codex_install.launch_command_for(verified)
    if connection_record.launch_command is not None and tuple(connection_record.launch_command) != tuple(launch_command):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="engine_launch_command_mismatch",
            remediation=(
                "This connection's launch_command does not match codex_install.launch_command_for() of "
                "the verified install; re-onboard the connection against the pinned install rather than "
                "launching a different command than the one it names."
            ),
            retry_eligible=False,
            field="launch_command",
        )
    pinned_engine_vars = {
        "CODEX_PATH": verified.bundled_codex_binary,
        "NO_BROWSER": "1",
    }
    return launch_command, pinned_engine_vars


@dataclass(frozen=True)
class CodexEvidenceStatus:
    """Distinct evidence fields for one applied configuration revision.

    - ``selected``: the ``ConfigurationRevision.id`` that was selected --
      never itself evidence of application.
    - ``configuration_applied``: the ``evidence_ref`` string
      ``EngineSessionManager.configure`` returned (engine instance id, wire
      session id, and the exact confirmed option id=value pairs) -- proof
      the *configuration call* round-tripped and the engine echoed back the
      requested values, never proof a subsequent prompt actually used them.
    - ``execution_verified``: always exactly
      ``EXECUTION_VERIFICATION_UNAVAILABLE`` for this pinned release. The
      installed codex-acp 1.12.0 surface gives no statically-verifiable,
      non-model-authored signal that a specific turn actually executed
      under a specific confirmed model/effort/mode (a ``usage_update``
      event reports token counts, never a confirmed model identity, and a
      model's own self-report of "which model I am" is never treated as
      evidence anywhere in this runtime -- see
      ``portable.engine_contract.EngineEventKind.USAGE_UPDATE``'s
      ``UsageBasis.ENGINE_REPORTED``, which is basis for usage figures
      only). This status is never upgraded by observing a
      ``usage_update``/``TURN_END`` event; a real per-turn execution
      verification mechanism, if any exists for a later codex-acp release,
      is out of this task's scope (see T035).
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


def build_codex_evidence_status(revision_id: str, evidence_ref: str) -> CodexEvidenceStatus:
    """The only constructor for :class:`CodexEvidenceStatus` -- callers never
    build one by hand, so ``execution_verified`` can never be set to
    anything but the pinned-release constant above."""
    return CodexEvidenceStatus(selected=revision_id, configuration_applied=evidence_ref)


def build_codex_session_new_meta(instructions_text: str, *, native_prompt: bool = True,
                                 base_instructions: str | None = None) -> dict:
    """Cortex's private codex-acp 1.12.0 copy forwards this extension to
    App Server's developerInstructions on each new thread. No role text is
    inserted into a user prompt; recovery sends only the task capsule.

    ``native_prompt=False`` also asks that private adapter to drop what Codex
    itself adds: an empty ``baseInstructions`` removes the model's base
    prompt, and ``cortexNativeContext=False`` removes the permissions, apps,
    collaboration-mode, skills-catalog and environment-context sections.
    The role text then reaches the model alone.

    ``base_instructions`` (the unified Cortex prompt) does the same removal
    but puts that text in the base prompt's place, ahead of the role text.
    """
    if not isinstance(instructions_text, str) or not instructions_text.strip():
        raise PortableError(
            category=PortableErrorCategory.VALIDATION, code="invalid_field",
            remediation="'instructions_text' must be a non-empty string.",
            retry_eligible=False, field="instructions_text",
        )
    if base_instructions is not None:
        if not isinstance(base_instructions, str) or not base_instructions.strip():
            raise PortableError(
                category=PortableErrorCategory.VALIDATION, code="invalid_field",
                remediation="'base_instructions' must be a non-empty string.",
                retry_eligible=False, field="base_instructions",
            )
        return {"systemPrompt": instructions_text, "cortexBaseInstructions": base_instructions,
                "cortexNativeContext": False}
    if not native_prompt:
        return {"systemPrompt": instructions_text, "cortexBaseInstructions": "", "cortexNativeContext": False}
    return {"systemPrompt": instructions_text}
