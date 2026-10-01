"""Cortex ACP slash commands: the documented selector equivalent for a host
client that omits ``SessionConfigOption``/``session/set_config_option``
support (see ``config_options.py``).

Configuration selectors share their existing implementation; explicit workflow commands use the same prompt path:

- ``/cortex-clear <retention or date> [confirm]`` -- runtime-owned task
  retention; preview by default, no model turn.
- ``/cortex-status`` -- report the current/pending model + thought_level,
  connection setup state, and the engine execution mode; no input,
  never mutates anything.
- ``/cortex-model <route label or route id>`` -- select a whole route
  (provider+model together) by its disambiguated label or route id, across
  every configured provider; routed through
  ``acp.agent.Agent._apply_route_selection``, which reaches the identical
  ``ConfigurationService.ui_select``/``command_set`` call a
  ``session/set_config_option`` request would make. T035: this is *not*
  the same as the protocol-level ``configId="model"`` axis (see
  ``config_options.py``), which is always scoped to whichever ``provider``
  is currently selected -- a client that wants provider+model as two
  separate choices uses ``session/set_config_option`` with
  ``configId="provider"`` then ``configId="model"`` instead.
- ``/cortex-effort <value>`` -- select a ``thought_level`` value; same
  sharing for ``configId="thought_level"``.
- ``/cortex-setup`` -- state-aware onboarding and repair guidance using the
  public setup CLI; never prints a secret or mutates a connection.

Recognition: a prompt consisting of exactly one ``text`` content block whose
*trimmed* text starts with a registered command name followed by end of
string or whitespace (see ``recognize``). Any other shape -- multiple
blocks, a non-text block, or an unrecognized ``/cortex-xyz`` name -- is not
a command and falls through to normal prompt handling.
"""
from __future__ import annotations

from typing import Mapping, Sequence
from datetime import datetime, timezone
import re

from ..catalog.model import CatalogEntry

CORTEX_CLEAR = "cortex-clear"
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
CORTEX_FINISH = "cortex-finish"
CORTEX_ANSWER = "cortex-answer"
CORTEX_LANGUAGE = "cortex-language"
# T035: a knowledge harvest is explicit-only by the coordinator's own role
# ("Keep `cortex:knowledge-harvest` explicit-only"). Until this command
# existed, "explicit" had no expression: a model had to infer from prose
# whether the user had really asked for one. A command is unambiguous --
# it was pressed or it was not -- and the user asked for it by this exact
# name, not a `cortex-` prefixed one.
HARVEST = "harvest"
HARVEST_REFRESH = "harvest-refresh"
SKILL_MANAGEMENT = "skill-management"

# Trigger text as typed by a user, in recognition priority order (longest first to avoid prefix overlaps).
_TRIGGERS: tuple[tuple[str, str], ...] = (
    ("/cortex-clear", CORTEX_CLEAR),
    ("/cortex-status", CORTEX_STATUS),
    ("/cortex-new-task", CORTEX_NEW_TASK),
    ("/cortex-language", CORTEX_LANGUAGE),
    ("/skill-management", SKILL_MANAGEMENT),
    ("/harvest-refresh", HARVEST_REFRESH),
    ("/harvest", HARVEST),
    ("/cortex-model", CORTEX_MODEL),
    ("/cortex-effort", CORTEX_EFFORT),
    ("/cortex-setup", CORTEX_SETUP),
    ("/cortex-stop", CORTEX_STOP),
    ("/cortex-finish", CORTEX_FINISH),
    ("/cortex-answer", CORTEX_ANSWER),
)

AVAILABLE_COMMANDS: tuple[dict, ...] = (
    dict(name=CORTEX_CLEAR, description="Preview old project task deletion; trailing confirm deletes unprotected tasks. No model call.", input=dict(hint="<N|Nd|days N|before YYYY-MM-DD|YYYY-MM-DD> [confirm]")),
    dict(name=CORTEX_STATUS, description="Show model/thought-level, connection state, and engine execution mode. Never changes anything.", input=None),
    dict(name=CORTEX_MODEL, description="Select the model by its route label or route id.", input=dict(hint="<route label or route id>")),
    dict(name=CORTEX_EFFORT, description="Select the thought_level (reasoning effort) value for the current model.", input=dict(hint="<value>")),
    dict(name=CORTEX_SETUP, description="Show onboarding guidance for connecting a model provider. Never changes anything, never prints a secret.", input=None),
    dict(name=CORTEX_NEW_TASK, description="Start a brand-new, separate task without touching the current active task.", input=dict(hint="<task text>")),
    dict(name=CORTEX_STOP, description="Urgently stop the active task: cancels every assignment, queued open, and open question. Does not restart through a later message.", input=None),
    dict(name=CORTEX_FINISH, description="Close this task now at the user's request, without claiming unfinished review, verification, or synthesis passed. Cancels active work.", input=None),
    dict(name=CORTEX_ANSWER, description="Answer one specific open coordinator question by its listed number (use when more than one question is open).", input=dict(hint="<question-number> <answer text>")),
    dict(name=CORTEX_LANGUAGE, description="Set the language Cortex uses for its own status/question/stop lines: auto, en, or ru.", input=dict(hint="auto|en|ru")),
    dict(name=HARVEST, description="Start or continue an explicit knowledge harvest. Optional text narrows the documentation scope; bare command covers the project.", input=dict(hint="<optional scope>")),
    dict(name=HARVEST_REFRESH, description="Explicitly refresh project documentation from current source. Text is an optional scope.", input=dict(hint="<optional scope>")),
    dict(name=SKILL_MANAGEMENT, description="Create, revise, or import a project skill. Use only when the user explicitly invokes this command.", input=dict(hint="<request>")),
)


CLEAR_USAGE = "Usage: /cortex-clear <N|Nd|days N|before YYYY-MM-DD|YYYY-MM-DD> [confirm] (N: 0..36500; date: non-future, 00:00 UTC). Omit confirm to preview."


def parse_clear_argument(argument: str, *, now=None) -> dict | None:
    """Strict owner command grammar; no user text is echoed on failure."""
    words = argument.split()
    confirm = bool(words and words[-1] == "confirm")
    if confirm:
        words.pop()
    value = " ".join(words)
    match = re.fullmatch(r"(?:days )?([0-9]{1,5})(d)?", value)
    if match and not (value.startswith("days ") and match[2]):
        days = int(match[1])
        if days <= 36500:
            return dict(days=days, cutoff=None, confirm=confirm,
                        confirm_command=f"/cortex-clear days {days} confirm")
    match = re.fullmatch(r"(?:before )?([0-9]{4}-[0-9]{2}-[0-9]{2})", value)
    if match:
        try:
            cutoff = datetime.strptime(match[1], "%Y-%m-%d").replace(tzinfo=timezone.utc)
        except ValueError:
            return None
        if cutoff <= (now or datetime.now(timezone.utc)):
            return dict(days=None, cutoff=cutoff, confirm=confirm,
                        confirm_command=f"/cortex-clear before {match[1]} confirm")
    return None


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
    exact = [
        entry for entry in selectable_entries
        if entry.disambiguated_label.lower() == lowered or entry.label.lower() == lowered
    ]
    if len(exact) == 1:
        return exact[0].route.id
    # The ACP selector exposes the engine model id as the value users see in
    # Zed. Accept that same id in the slash command when it identifies one
    # offered route; retain the full label/id requirement when providers
    # offer the same model more than once.
    model_matches = [entry for entry in selectable_entries if entry.route.model.lower() == lowered]
    if len(model_matches) == 1:
        return model_matches[0].route.id
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


def build_status_text(*, config_options: Sequence[Mapping[str, object]],
                      connection_setup_summary: str, execution_mode: str | None = None) -> str:
    """Pure, read-only ``/cortex-status`` report text."""
    lines = ["Cortex configuration status:"]
    if not config_options:
        lines.append("- model: none selectable yet (" + connection_setup_summary + ")")
        if execution_mode is not None:
            lines.append(f"- execution: {execution_mode}")
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
    if execution_mode is not None:
        lines.append(f"- execution: {execution_mode}")
    return "\n".join(lines)


SETUP_GUIDANCE = (
    "No model is selectable yet for this project. To connect an engine:\n"
    "1. Follow docs/acp/packaging-and-onboarding.md to register a pinned install "
    "with the add-connection command and its --install-record option.\n"
    "2. Complete the engine's native login in the isolated directory shown by "
    "setup, or provide an API key through a private file or stdin.\n"
    "3. Run `python3 -B scripts/cortex_acp_setup.py validate-connection "
    "--connection ID` using the ID returned by setup. Once it reports "
    "'configured', start a new Cortex session to refresh models.\n"
    "This message never contains a secret and never mutates any connection."
)


def build_setup_text(*, connection_states: Mapping[str, int] | None = None) -> str:
    """Return setup guidance that reflects the registered connection state.

    A native connection is registered before its engine login is complete.  In
    that state the user needs to authorize the existing connection, not create
    a duplicate one. ``connection_states`` contains counts keyed by persisted
    state value only; it never carries credential references or filesystem
    paths.
    """
    states = connection_states or {}
    if states.get("authorization_required", 0):
        return (
            "A connection is already registered but is awaiting authorization.\n"
            "1. Complete that engine's native login in the isolated engine directory "
            "shown during connection setup.\n"
            "2. Run `python3 -B scripts/cortex_acp_setup.py validate-connection "
            "--connection ID` using its existing ID until it reports 'configured'.\n"
            "3. Once configured, its models will start appearing in the model selector "
            "(or /cortex-model).\n"
            "Do not register another connection for the same engine and account. "
            "This message never contains a secret and never mutates any connection."
        )
    if states.get("unavailable", 0):
        return (
            "A registered connection is currently unavailable.\n"
            "Check that the configured engine is installed and reachable, then run "
            "`python3 -B scripts/cortex_acp_setup.py validate-connection --connection ID`. "
            "If the connection is no longer usable, repair or remove "
            "that registration before adding a replacement.\n"
            "This message never contains a secret and never mutates any connection."
        )
    if states.get("invalid", 0):
        return (
            "A registered connection has invalid configuration.\n"
            "Review its engine, provider, and authentication settings, repair the existing "
            "registration, then run `python3 -B scripts/cortex_acp_setup.py "
            "validate-connection --connection ID` again.\n"
            "This message never contains a secret and never mutates any connection."
        )
    return SETUP_GUIDANCE
