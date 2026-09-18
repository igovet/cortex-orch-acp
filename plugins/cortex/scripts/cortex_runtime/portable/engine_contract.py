"""Portable engine runtime contract: capabilities, protocol, and events.

``EngineCapability`` member values are the "engine-applicable" subset of
``capabilities_to_record`` from
``.cortex/acp-migration/T002/CAPABILITY_MATRIX.json`` -- specifically every
recorded capability except the ones that JSON's
``cortex_directions.capabilities_primarily_incoming_client_offered`` marks as
primarily offered by the incoming ACP client rather than an outgoing engine
(``filesystem_execution_location``, ``unsaved_buffers``, ``terminals``,
``permissions``, ``native_tool_containment``, ``tool_schema_delivery``,
``boolean_config``).  Names are kept verbatim from that matrix.

This module carries no native engine/provider/host tokens: see
``tests/test_portable_no_native_tokens.py``.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Iterator, Mapping, Protocol, runtime_checkable

from .identity import ConfigurationRevision


class EngineCapability(str, Enum):
    """Engine-applicable capability names, verbatim from CAPABILITY_MATRIX.json."""

    PROTOCOL_VERSION = "protocol_version"
    SESSION_CREATION = "session_creation"
    PROMPT_STREAMING = "prompt_streaming"
    CONFIGURATION_OPTIONS = "configuration_options"
    MODEL_DISCOVERY = "model_discovery"
    MODEL_APPLICATION_CONFIRMATION = "model_application_confirmation"
    REASONING_LEVELS = "reasoning_levels"
    AUTH_MODES = "auth_modes"
    CREDENTIAL_CONFIG_ISOLATION = "credential_config_isolation"
    RUNTIME_BRIDGE = "runtime_bridge"
    ROLE_INSTRUCTION_DELIVERY = "role_instruction_delivery"
    CANCELLATION = "cancellation"
    CONFIRMED_PAUSE = "confirmed_pause"
    HISTORY_LOAD_WITH_REPLAY = "history_load_with_replay"
    RESUME_WITHOUT_REPLAY = "resume_without_replay"
    ELICITATION = "elicitation"
    COMPACTION_OBSERVATION = "compaction_observation"
    COMPACTION_REQUEST = "compaction_request"
    RECOVERY_DELIVERY_CHANNEL = "recovery_delivery_channel"
    CONTEXT_GENERATION_OBSERVATION = "context_generation_observation"
    USAGE = "usage"
    COST = "cost"
    SUMMARIZER_SELECTION = "summarizer_selection"
    NATIVE_SUBAGENTS_OPTIONAL = "native_subagents_optional"


class StopReason(str, Enum):
    """Why a turn ended, mirroring observed portable ACP stop reasons."""

    END_TURN = "end_turn"
    MAX_TOKENS = "max_tokens"
    MAX_TURN_REQUESTS = "max_turn_requests"
    REFUSAL = "refusal"
    CANCELLED = "cancelled"


class UsageBasis(str, Enum):
    """Provenance of a usage figure. 'unknown' is never treated as zero."""

    OBSERVED = "observed"
    ENGINE_REPORTED = "engine_reported"
    ESTIMATED = "estimated"
    UNKNOWN = "unknown"


class EngineEventKind(str, Enum):
    TURN_DELTA = "turn_delta"
    TURN_END = "turn_end"
    TOOL_CALL_OBSERVED = "tool_call_observed"
    USAGE_UPDATE = "usage_update"
    COMPACTION_SIGNAL = "compaction_signal"
    DISPLAY_UPDATE = "display_update"
    ERROR = "error"


@dataclass(frozen=True)
class EngineEvent:
    """One portable event emitted while streaming a turn.

    ``kind`` selects which of the optional payload fields is populated;
    unused fields stay ``None``.  There is deliberately no field for a
    native subagent identity, opaque provider memory blob, engine-specific
    tool name, or host metadata -- those are not portable concepts.
    """

    kind: EngineEventKind
    delta_text: str | None = None
    stop_reason: StopReason | None = None
    tool_call_ref: str | None = None
    usage_basis: UsageBasis | None = None
    usage_values: Mapping[str, int] | None = None
    compaction_reason: str | None = None
    error_message: str | None = None
    # T017: populated only for kind=TOOL_CALL_OBSERVED -- the wire tool
    # call's own kind/title/status (never raw tool output/arguments/result).
    tool_call_kind: str | None = None
    tool_call_title: str | None = None
    tool_call_status: str | None = None
    # T035: the engine's own ACP presentation for this call -- the file
    # locations it touches (ToolCallLocation[]) and what it produced
    # (ToolCallContent[], which is how a diff is carried). Protocol-shaped
    # display data defined by the schema both engines already speak, not
    # raw tool arguments or results: a client renders "Read <path>" as a
    # link and an edit as a diff from exactly these, and without them a
    # card can only say that *something* happened.
    tool_call_locations: tuple | None = None
    tool_call_content: tuple | None = None
    # T035: one engine ``session/update`` whose only purpose is to be
    # shown, forwarded verbatim (see engine.session_manager
    # ._DISPLAY_ONLY_UPDATES for exactly which kinds, and why the engine's
    # own session-state updates are deliberately not among them).
    display_update: Mapping[str, object] | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.kind, EngineEventKind):
            raise TypeError("kind must be an EngineEventKind member")
        if self.kind == EngineEventKind.TURN_END and not isinstance(self.stop_reason, StopReason):
            raise TypeError("turn_end events require a StopReason")
        if self.kind == EngineEventKind.USAGE_UPDATE and not isinstance(self.usage_basis, UsageBasis):
            raise TypeError("usage_update events require a UsageBasis")


@runtime_checkable
class EngineRuntime(Protocol):
    """Portable surface every engine integration must implement.

    Structural typing only: an integration satisfies this Protocol by
    defining matching methods, with no inheritance requirement.
    """

    def negotiate(self, requested_capabilities: tuple[EngineCapability, ...]) -> tuple[EngineCapability, ...]:
        """Return the subset of requested capabilities this engine honors."""
        ...

    def create_session(self, task_id: str, route_id: str) -> str:
        """Create a new engine session and return its portable session id."""
        ...

    def configure(self, engine_session_id: str, revision: ConfigurationRevision) -> None:
        """Apply a configuration revision to an existing engine session."""
        ...

    def prompt_turn(self, engine_session_id: str, prompt: str) -> Iterator[EngineEvent]:
        """Start a turn and yield portable events until it ends."""
        ...

    def cancel(self, engine_session_id: str) -> None:
        """Request cancellation of the engine session's in-flight turn."""
        ...

    def release(self, engine_session_id: str) -> None:
        """Release engine-side resources for a session no longer in use."""
        ...

    def recovery_observation(self, engine_session_id: str) -> Mapping[str, Any] | None:
        """Optional: return an opaque recovery observation, or None."""
        ...
