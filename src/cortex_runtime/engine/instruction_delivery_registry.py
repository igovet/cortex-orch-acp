"""Adapter-specific ``InstructionDelivery`` mechanisms and engine+version registry.

Deliberately *not* under ``portable/`` (see
``tests/test_portable_no_native_tokens.py``: the portable package must
never name a specific engine/vendor). This module is the one place that
maps a concrete engine name/version to the
``portable.instruction_delivery.InstructionDelivery`` mechanism that
actually applies to it.
"""
from __future__ import annotations

from dataclasses import dataclass

from ..portable.instruction_delivery import DeliveryPlan, DeliveryQualification, InstructionDelivery, UnsupportedDelivery
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.roles import RoleInstructionSet


@dataclass(frozen=True)
class CodexAcpDelivery:
    """A verified, private codex-acp adapter delivers Cortex role text
    through ``session/new _meta.systemPrompt`` to Codex App Server
    ``thread/start.developerInstructions``. Its exact-source-checked copy
    preserves effective native developer instructions before the Cortex text.

    The no-paid installed-bundle trace in
    ``tests/test_codex_developer_instructions.py`` verifies the App Server
    request. Two 2026-09-21 disposable Paseo/Codex native rollouts also
    recorded the Cortex role as a developer message. Earlier releases of
    this class sent the same role again as a nonce-tagged user prompt; that
    duplicate is deliberately absent from ``blocks`` now. New sessions and
    replacements each get their own ``thread/start``. The behavior of native
    developer instructions after Codex internal compaction has not yet been
    observed in a real client and remains unverified.

    Registered for 1.12.0, 1.13.0 and 2.0.1. All use the source-checked
    session/new to thread/start channel. 2.0.1 has separate typed-reporter
    patches and passes the installed-bundle no-paid trace. The 1.x pair is
    verified against the same
    delivery mechanism: the ``session/new``/``thread/start`` call sites the
    private adapter patch rewrites are byte-identical in the two bundles
    (see ``engine.process._CODEX_SEND_PROMPT_BY_VERSION``, whose only
    version difference is an unrelated ``runTurn`` argument rename), and
    the no-paid installed-bundle trace in
    ``tests/test_codex_developer_instructions.py`` was run against each
    bundle. 1.13.0 bundles Codex CLI 0.155.1 rather than 0.154.0, which is
    what makes that CLI's newer models reachable at all.
    """

    engine: str = "codex-acp"
    version: str = "1.12.0"

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        from .codex_adapter_acp import build_codex_session_new_meta

        return DeliveryPlan(
            qualification=DeliveryQualification.QUALIFIED,
            engine=self.engine,
            version=self.version,
            channel="session/new _meta.systemPrompt to thread/start.developerInstructions",
            blocks=(),
            session_new_meta=build_codex_session_new_meta(role.body_portable),
        )


@dataclass(frozen=True)
class CodexAcp2_1Delivery(CodexAcpDelivery):
    """Exact 2.1.1 qualification, npm bundle inspected 2026-10-07.

    dist/index.js SHA-256:
    4b76310393d756a0f111687cd9df899720f36f7c59b1eb1d86429484034ff91b.
    All 14 guarded patch sites match 2.0.1 byte-for-byte: session/new at
    34087, session config at 34603, runTurn at 34345, MCP at 34214/34224,
    typed history at 39090 and compaction renderer at 27483. ACP remains
    v1; model/list-derived options and instruction metadata are unchanged.
    The installed-bundle fake App Server trace exercises this channel, not
    a paid model or rendered client. Native CLI version is install-specific
    (@openai/codex dependency is ^0.159.1), not a fixed bundled guarantee.
    """

    version: str = "2.1.1"


@dataclass(frozen=True)
class ClaudeAgentAcp0_78Delivery:
    """The pinned claude-agent-acp 0.78.0 mechanism: ``session/new``'s own
    ``params._meta.systemPrompt`` -- QUALIFIED (T026, upgraded on fresh,
    rigorous re-verification; superseded the prior UNSUPPORTED reading of
    this exact (engine, version), which rested on the weaker
    leading-prompt-block candidate alone -- see the "prior finding" note at
    the end of this docstring).

    Full evidence chain (every file:line read directly against the real
    installed dist, lead- and agent-verified 2026-09-17, package.json
    version 0.78.0, sha256 ``e23a25d9...4934e2``; ``dist/index.js`` sha256
    ``9d73d1f0...b9422a``):

    1. **Who reads it / what it becomes**: ``createSession(params, ...)``
       reads ``params._meta.systemPrompt`` (``acp-agent.js:5887-5901``): a
       plain string fully replaces the SDK's own
       ``{"type": "preset", "preset": "claude_code"}`` default system
       prompt; an object form is merged with ``append``/
       ``excludeDynamicSections`` fields, locked to
       ``type: "preset"``/``preset: "claude_code"``. The resulting value is
       assigned to the local ``systemPrompt``, placed into
       ``options.systemPrompt`` (``acp-agent.js:6019``), and that same
       ``options`` object is passed as ``query({prompt: input, options})``
       (``acp-agent.js:6179-6182``) -- ``systemPrompt`` and ``prompt``
       (the actual conversation content, ``input``) are two distinct
       fields on that one call, never merged.
    2. **Passed to the SDK as a genuine system prompt, not user content**:
       the installed ``@anthropic-ai/claude-agent-sdk`` 0.3.270's own
       ``query()`` implementation (``sdk.mjs``, function ``t0(e,t)``)
       destructures ``systemPrompt`` out of its options *separately* from
       the prompt/conversation input, builds a distinct SDK-internal
       config object (locally named ``bre`` in the minified bundle)
       carrying ``systemPrompt``/``appendSystemPrompt``/
       ``systemPromptSnapshot`` as their own named fields, and forwards
       that to the spawned Claude Code CLI subprocess's own control
       protocol at query creation (``query()`` "spawns the CLI at once",
       per the adapter's own comment immediately above the call site).
       This is the SDK's own first-class, documented ``systemPrompt``
       option -- never a string concatenated into the conversation/prompt
       stream.
    3. **Survives resume/load, not only session/new**: ``resumeSession``/
       ``loadSession`` both funnel through ``getOrCreateSession(params,
       ...)`` (``acp-agent.js:993,1004``), which explicitly forwards
       ``_meta: params._meta`` into the same ``createSession()`` call
       (``acp-agent.js:5776-5782``) used by ``newSession``. A caller that
       resupplies ``_meta.systemPrompt`` on a resume/load call gets it
       honored exactly as on a fresh ``session/new`` -- the channel is not
       a one-shot, session/new-only artifact.
    4. **Distinguishable from ordinary user content**: yes, per points 1-2
       above -- it is a separate field at every layer (adapter ->
       SDK -> CLI control protocol), never text prepended to a prompt
       content block. This is strictly stronger evidence than codex-acp's
       own UNQUALIFIED leading-block channel (which has no distinct
       system/instructions role on the wire at all).

    ``engine.claude_adapter_acp.build_claude_session_new_meta`` builds
    the preset-preserving ``{"append": ...}`` form of this payload; wired into the
    real production path via
    ``delegation.live_adapters._session_new_meta_for``/``open()`` and
    ``engine.session_manager.EngineSessionManager.create_session``'s
    additive ``session_new_meta`` parameter (T026) -- never a paper-only
    registry classification with no runtime effect.

    Still not "live-executed": no real turn has run through this channel
    (as documented by the adapter's installed capability evidence); a live run could in principle
    still reveal a downstream drop/override this static reading cannot
    see. That is exactly why the enum's own QUALIFIED definition allows "a
    confirmed system-role channel" as sufficient basis (not literally
    requiring an executed live run) -- see
    ``portable.instruction_delivery.DeliveryQualification``'s own
    docstring.

    **Prior finding, corrected**: an earlier T026 pass evaluated only the
    weaker leading-prompt-block candidate (as weak as codex-acp's own
    UNQUALIFIED evidence) and kept this UNSUPPORTED to avoid silently
    overriding the then-current, already-reviewed
    ``tests/test_instruction_delivery_qualification.py`` (T015)
    expectations. Per explicit lead re-direction: those T015 tests encoded
    what was believed *then*, not the actual dist evidence, and are
    updated by this same change (T026) to assert QUALIFIED for exactly
    this (engine, version) instead, while still asserting UNSUPPORTED for
    an unrecognized engine/version (see
    :class:`ClaudeAgentAcpUnrecognizedVersionDelivery` below).
    """

    engine: str = "claude-agent-acp"
    version: str = "0.78.0"

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        from .claude_adapter_acp import build_claude_session_new_meta

        return DeliveryPlan(
            qualification=DeliveryQualification.QUALIFIED,
            engine=self.engine,
            version=self.version,
            channel="session/new _meta.systemPrompt",
            blocks=(),
            session_new_meta=build_claude_session_new_meta(role.body_portable),
        )


@dataclass(frozen=True)
class ClaudeAgentAcp0_87Delivery(ClaudeAgentAcp0_78Delivery):
    """Exact 0.87.0 registration, inspected from the npm tarball 2026-10-07.

    ACP stays v1 (dist/acp-agent.js:1342). createSession reads the string or
    preset-append systemPrompt at 7047-7063, puts it into options at
    7177-7181, and calls query({prompt: input, options}) at 7337-7340.
    The bundled SDK 0.3.287 sdk.mjs BR separately converts the string or
    preset append into systemPrompt/appendSystemPrompt, forwarded by its
    initialize control request, never conversation content. Load/resume
    resupply _meta at acp-agent.js:1442,6928.

    Cortex also installs an exact side-by-side SDK 0.3.293 refresh for
    Haiku 5.5. Its sdk.mjs fT keeps systemPrompt/preset append separate;
    initialize forwards initConfig.systemPrompt/appendSystemPrompt, and
    CLI argument construction retains strictMcpConfig/disallowedTools.
    No-prompt ACP model/effort controls were exercised with both SDKs;
    model execution and native-tool enforcement remain unverified.

    The wire payload is unchanged, so plan is shared with 0.78.0. Option
    forwarding (including strictMcpConfig and disallowedTools) was re-read
    at 7079,7181,7223; compaction presentation is NOT unchanged (see
    session_manager and generations). Static qualification is not a paid
    model-turn or GUI enforcement claim.
    """

    version: str = "0.87.0"


@dataclass(frozen=True)
class ClaudeAgentAcp0_88Delivery(ClaudeAgentAcp0_78Delivery):
    """Exact installed 0.88.0 qualification, inspected read-only 2026-10-08.

    dist/acp-agent.js SHA-256:
    790992f7c7387d78a5324fcfcbf9eb79a811ebd68eb3d6161a93de34b28327a0.
    ACP v1 at 1368; string/preset append at 7091-7107, options at
    7221-7225, query({prompt: input, options}) at 7381-7384. Resume/load
    resupply _meta at 1468,1499-1523,6968-6976 (existing queries are reused
    only when their session-defining params match).
    Its bundled SDK 0.3.293 sdk.mjs:142 fT separates systemPrompt/preset
    append from prompt; line 138 sends initConfig.systemPrompt and
    appendSystemPrompt in the initialize control request.

    strictMcpConfig/options spread at acp-agent.js:7225; disallowedTools
    merged at 7267, tools resolved at 7140-7145 and forwarded at 7268.
    SDK sdk.mjs:136 builds --strict-mcp-config, --disallowedTools and
    --tools (a preset becomes "default", not an explicit allowlist).
    Cortex must supply its own disallowedTools; AskUserQuestion alone
    is only the adapter's no-form-elicitation default, not Cortex policy.

    Model/effort/mode retain select shapes: session-model.js:179-192,
    session-effort.js:64-100, session-mode.js:68-75. Usage attaches
    _claude/model at acp-agent.js:2614-2625,3565,4377,4835,5206; this
    is not complete model/effort/mode execution verification.
    Non-AIR compaction uses claudeCode.toolName=compact at
    dist/tool-calls/reporters/compaction.js:17-65, reactively only.
    Shared plan is justified by these paths, not version interpolation.
    No paid turn, effective native-tool enforcement or GUI is qualified.
    """

    version: str = "0.88.0"


@dataclass(frozen=True)
class ClaudeAgentAcpUnrecognizedVersionDelivery:
    """Any claude-agent-acp version without an exact registration: no static
    evidence has been read for it, so it is always UNSUPPORTED -- never a
    guessed/interpolated qualification borrowed from the pinned version
    (the ``_meta.systemPrompt`` channel's exact call chain above was only
    confirmed against 0.78.0's own dist; a different version could change
    or drop it without warning)."""

    engine: str = "claude-agent-acp"
    version: str = "*"

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        return DeliveryPlan(
            qualification=DeliveryQualification.UNSUPPORTED,
            engine=self.engine,
            version=self.version,
            channel="none",
            blocks=(),
        )


@dataclass(frozen=True)
class OpenCodeAgentPromptDelivery:
    """OpenCode v2: the role text is the prompt of the ``cortex`` agent
    the process is started with. The channel was observed on 2.0.19;
    version policy now accepts any 2.x release.

    2.0.19 ignores ``session/new`` ``_meta.systemPrompt`` (a marker sent there
    never reached the model), and has no other per-session instruction
    channel. An agent ``prompt`` given in ``OPENCODE_CONFIG_CONTENT`` as
    ``{file:<path>}`` was observed, through a loopback fake provider, as the
    first system message of every model request, replacing OpenCode's own
    base prompt; OpenCode appends its environment block and Code Mode tool
    catalog after it. ``engine.opencode_adapter_acp`` builds that prompt and
    ``engine.process`` mounts it read-only into the jail, one process per
    session, so ``session_new_meta`` is ``None`` and no prompt block carries
    the role (``blocks`` stays empty, as for the other engines). Checked by
    ``tests/test_engine_opencode_qualification.py`` against the installed
    binary when it is present.
    """

    engine: str = "opencode"
    version: str = "2.0.0"

    def plan(self, role: RoleInstructionSet, user_blocks: list) -> DeliveryPlan:
        return DeliveryPlan(
            qualification=DeliveryQualification.QUALIFIED,
            engine=self.engine,
            version=self.version,
            channel="OPENCODE_CONFIG_CONTENT agent prompt {file:} at process launch",
            blocks=(),
            session_new_meta=None,
        )


# Exact registrations for the npm adapters. OpenCode uses the supported
# major range in delivery_for rather than a patch-specific entry.
_REGISTRY: dict[tuple[str, str], InstructionDelivery] = {
    ("codex-acp", "1.12.0"): CodexAcpDelivery(version="1.12.0"),
    ("codex-acp", "1.13.0"): CodexAcpDelivery(version="1.13.0"),
    ("codex-acp", "2.0.1"): CodexAcpDelivery(version="2.0.1"),
    ("codex-acp", "2.1.1"): CodexAcp2_1Delivery(),
    ("claude-agent-acp", "0.78.0"): ClaudeAgentAcp0_78Delivery(),
    ("claude-agent-acp", "0.87.0"): ClaudeAgentAcp0_87Delivery(),
    ("claude-agent-acp", "0.88.0"): ClaudeAgentAcp0_88Delivery(),
    ("claude-agent-acp", "*"): ClaudeAgentAcpUnrecognizedVersionDelivery(),
}


def has_exact_registration(engine: str, version: str) -> bool:
    """True for an exact npm registration or a supported OpenCode v2
    version. Wildcards do not count as qualified registrations."""
    if version == "*":
        return False
    if engine == "opencode":
        from ..connections.discovery import is_qualified

        return is_qualified(engine, version)
    return (engine, version) in _REGISTRY


def delivery_for(engine: str, version: str) -> InstructionDelivery:
    """Look up the delivery mechanism for an exact engine+version.

    OpenCode v2 is selected by major range. Otherwise falls back in order:
    exact (engine, version); (engine, "*") wildcard
    registration (used for claude-agent-acp versions without exact evidence);
    otherwise :class:`UnsupportedDelivery`.
    """
    if not isinstance(engine, str) or not engine:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation="'engine' must be a non-empty string.",
            retry_eligible=False,
            field="engine",
        )
    if not isinstance(version, str) or not version:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation="'version' must be a non-empty string.",
            retry_eligible=False,
            field="version",
        )
    exact = _REGISTRY.get((engine, version))
    if exact is not None:
        return exact
    if engine == "opencode":
        from ..connections.discovery import is_qualified

        if is_qualified(engine, version):
            return OpenCodeAgentPromptDelivery(version=version)
    wildcard = _REGISTRY.get((engine, "*"))
    if wildcard is not None:
        return wildcard
    return UnsupportedDelivery(engine=engine, version=version)
