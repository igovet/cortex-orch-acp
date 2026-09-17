"""Cortex ACP slash commands: the documented selector equivalent for a host
client that omits ``SessionConfigOption``/``session/set_config_option``
support (see ``config_options.py``).

Four commands, never a separate configuration implementation of their own:

- ``/cortex-status`` -- report the current/pending model + thought_level and
  connection setup state; no input, never mutates anything.
- ``/cortex-model <route label or route id>`` -- select a model; routed
  through the identical ``ConfigurationService.ui_select``/``command_set``
  call a ``session/set_config_option`` for ``configId="model"`` would make
  (see ``acp.agent``'s shared selection helper).
- ``/cortex-effort <value>`` -- select a ``thought_level`` value; same
  sharing for ``configId="thought_level"``.
- ``/cortex-setup`` -- static onboarding guidance naming connection setup
  steps (``connections.onboarding.create_connection`` /
  ``set_api_key``/``mark_native_login_pending``/``validate``); never prints a
  secret, never mutates anything.

Recognition: a prompt consisting of exactly one ``text`` content block whose
*trimmed* text starts with a registered command name followed by end of
string or whitespace (see ``recognize``). Any other shape -- multiple
blocks, a non-text block, or an unrecognized ``/cortex-xyz`` name -- is not
a command and falls through to normal prompt handling.
"""
from __future__ import annotations

from typing import Mapping, Sequence

from ..catalog.model import CatalogEntry

CORTEX_STATUS = "cortex-status"
CORTEX_MODEL = "cortex-model"
CORTEX_EFFORT = "cortex-effort"
CORTEX_SETUP = "cortex-setup"
# T020 (see acp.orchestrating_runtime / acp.steering / docs/acp/steering-
# and-questions.md): explicit, exact-prefix-only commands -- never inferred
# from free text -- for the steering/urgent-stop/question-disambiguation
# channel.
CORTEX_NEW_TASK = "cortex-new-task"
CORTEX_STOP = "cortex-stop"
CORTEX_ANSWER = "cortex-answer"
CORTEX_LANGUAGE = "cortex-language"

# Trigger text as typed by a user, in recognition priority order (longest
# first so "/cortex-model" is never mistaken for a shorter prefix).
_TRIGGERS: tuple[tuple[str, str], ...] = (
    ("/cortex-status", CORTEX_STATUS),
    ("/cortex-new-task", CORTEX_NEW_TASK),
    ("/cortex-language", CORTEX_LANGUAGE),
    ("/cortex-model", CORTEX_MODEL),
    ("/cortex-effort", CORTEX_EFFORT),
    ("/cortex-setup", CORTEX_SETUP),
    ("/cortex-stop", CORTEX_STOP),
    ("/cortex-answer", CORTEX_ANSWER),
)

AVAILABLE_COMMANDS: tuple[dict, ...] = (
    dict(name=CORTEX_STATUS, description="Show the current and pending model/thought-level selection and connection setup state. Never changes anything.", input=None),
    dict(name=CORTEX_MODEL, description="Select the model by its route label or route id.", input=dict(hint="<route label or route id>")),
    dict(name=CORTEX_EFFORT, description="Select the thought_level (reasoning effort) value for the current model.", input=dict(hint="<value>")),
    dict(name=CORTEX_SETUP, description="Show onboarding guidance for connecting a model provider. Never changes anything, never prints a secret.", input=None),
    dict(name=CORTEX_NEW_TASK, description="Start a brand-new, separate task without touching the current active task.", input=dict(hint="<task text>")),
    dict(name=CORTEX_STOP, description="Urgently stop the active task: cancels every assignment, queued open, and open question. Does not restart through a later message.", input=None),
    dict(name=CORTEX_ANSWER, description="Answer one specific open coordinator question by its listed number (use when more than one question is open).", input=dict(hint="<question-number> <answer text>")),
    dict(name=CORTEX_LANGUAGE, description="Set the language Cortex uses for its own status/question/stop lines: auto, en, or ru.", input=dict(hint="auto|en|ru")),
)


def resolve_language_argument(argument: str) -> str | None:
    """Parse a ``/cortex-language auto|en|ru`` argument (case-insensitive);
    ``None`` for anything else (never a silent guess)."""
    value = (argument or "").strip().lower()
    return value if value in ("auto", "en", "ru") else None


def recognize(blocks: object) -> tuple[str, str] | None:
    """Return ``(command_id, argument_text)`` iff ``blocks`` is a Cortex
    command prompt; ``None`` otherwise (including any multi-block or
    non-text-block prompt, which always falls through to normal handling)."""
    if not isinstance(blocks, list) or len(blocks) != 1:
        return None
    block = blocks[0]
    if not isinstance(block, dict) or block.get("type") != "text":
        return None
    text = block.get("text")
    if not isinstance(text, str):
        return None
    stripped = text.strip()
    for trigger, command_id in _TRIGGERS:
        if stripped == trigger:
            return command_id, ""
        if stripped.startswith(trigger) and stripped[len(trigger)] in (" ", "\t", "\n"):
            return command_id, stripped[len(trigger):].strip()
    return None


def resolve_model_argument(argument: str, selectable_entries: Sequence[CatalogEntry]) -> str | None:
    """Resolve a ``/cortex-model`` argument to a route id.

    Accepts either the exact route id, or a case-insensitive match against
    the entry's disambiguated label; ``None`` when nothing matches (never a
    silent best-effort guess).
    """
    argument = (argument or "").strip()
    if not argument:
        return None
    for entry in selectable_entries:
        if argument == entry.route.id:
            return entry.route.id
    lowered = argument.lower()
    for entry in selectable_entries:
        if entry.disambiguated_label.lower() == lowered or entry.label.lower() == lowered:
            return entry.route.id
    return None


def parse_answer_argument(argument: str) -> tuple[int, str] | None:
    """Parse a ``/cortex-answer <question-number> <text>`` argument into a
    1-based ``(index, text)`` pair; ``None`` when the number is missing or
    not a positive integer (never a silent guess)."""
    parts = (argument or "").split(None, 1)
    if len(parts) < 2 or not parts[0].isdigit():
        return None
    index = int(parts[0])
    if index < 1:
        return None
    text = parts[1].strip()
    if not text:
        return None
    return index, text


def build_status_text(*, config_options: Sequence[Mapping[str, object]], connection_setup_summary: str) -> str:
    """Pure, read-only ``/cortex-status`` report text."""
    lines = ["Cortex configuration status:"]
    if not config_options:
        lines.append("- model: none selectable yet (" + connection_setup_summary + ")")
        return "\n".join(lines)
    for option in config_options:
        current = option.get("currentValue")
        label = current
        for candidate in option.get("options", []):
            if candidate.get("value") == current:
                label = candidate.get("name", current)
                break
        state_note = option.get("description") or ""
        lines.append(f"- {option.get('id')}: {label}" + (f" {state_note}" if state_note else ""))
    lines.append(f"- connections: {connection_setup_summary}")
    return "\n".join(lines)


SETUP_GUIDANCE = (
    "No model is selectable yet for this project. To connect a provider:\n"
    "1. Register a connection (engine + provider + auth mode) via the Cortex "
    "connection onboarding flow (connections.onboarding.create_connection).\n"
    "2. Supply credentials out of band: for an API-key connection, set the key "
    "via set_api_key (stored only in a private, owner-only secret file or the "
    "OS keyring, never printed here); for a native-account connection, "
    "complete that engine's own native login, then finish with "
    "mark_native_login_pending's reported next step.\n"
    "3. Confirm the connection with validate(); once it reports 'configured', "
    "its models will start appearing in the model selector (or /cortex-model).\n"
    "This message never contains a secret and never mutates any connection."
)


def build_setup_text() -> str:
    return SETUP_GUIDANCE
