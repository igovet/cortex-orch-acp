"""OpenCode v2 adapter (``opencode acp`` 2.0.19): configuration, role prompt, options.

Everything here was observed against the installed 2.0.19 binary through raw
ACP with a loopback fake model provider -- never a paid or remote model call
(see docs/acp/opencode-adapter.md and ``tests/test_engine_opencode*.py``).

Configuration. OpenCode merges the owner's ``~/.config/opencode``, the
project's ``opencode.json``/``.opencode/`` and the ``OPENCODE_CONFIG_CONTENT``
environment variable. Cortex hides the first (the engine jail's ``masked``
tmpfs), turns off the second (``OPENCODE_DISABLE_PROJECT_CONFIG=1``: a
project agent, a ``.opencode`` plugin and agent, and project ``AGENTS.md``
were all observed not to load) and supplies the third,
:func:`build_opencode_config`:

- one primary agent, ``cortex``, the default and the only one offered
  (``build`` and ``plan`` disabled). Its prompt is ``{file:<path>}``, a
  read-only file the jail mounts from memory (``engine_jail(data_files=)``);
  2.0.19 ignores ``session/new`` ``_meta.systemPrompt``, while an agent
  prompt was observed to replace OpenCode's own base prompt;
- ``permission``: ``"*": "ask"``, so every native tool call becomes an ACP
  ``session/request_permission`` (OpenCode's own default ran a shell ``rm``
  without asking), and ``task`` (its ``subagent`` tool), ``question`` and
  ``skill`` denied, which removes those tools. ``execute`` (Code Mode) is
  kept because 2.0.19 reaches session MCP servers -- Cortex's own gateway
  included -- only through it, as ``tools.<server>.<tool>``. One ``execute``
  call is one permission request; the calls inside it are not asked one by
  one. Its catalog holds the session's MCP servers, ``opencode`` (session
  move/rename, model search, MCP resources) and ``browser`` (unusable
  without a connected desktop app), and no shell or file tools;
- the ``title`` agent disabled, so no extra request goes to a small model
  OpenCode would pick itself; ``autoupdate`` off; ``share`` disabled.

No provider, model or MCP server is configured: providers come from the
owner's own ``opencode auth login``, the model is set per session through
``session/set_config_option`` (confirm-or-fail, :func:`opencode_option_mapping`)
and MCP servers are passed only on ``session/new``. With no model set,
OpenCode picks a free OpenCode Zen model, so a session is never prompted
before Cortex confirms the route's model.
"""
from __future__ import annotations

import json
import os

from ..configuration.service import ConfigurationRejected

#: The one agent Cortex sessions run as; OpenCode offers it as the ``mode``.
AGENT = "cortex"
#: Where the jail mounts the agent prompt, inside the engine's scratch dir.
PROMPT_FILE_NAME = "cortex-opencode-agent.md"
#: OpenCode permission keys Cortex denies (removes the tool).
DENIED_PERMISSIONS = ("task", "question", "skill")

_MODEL_OPTION_ID = "model"
_EFFORT_OPTION_ID = "effort"
_MODE_OPTION_ID = "mode"
_EFFORT_PARAM_NAMES = ("thought_level", "effort")

#: Where OpenCode reads the owner's own configuration (hidden in the jail).
USER_CONFIG_RELATIVE = (".config", "opencode")

#: Stands in for role instructions when the process only serves a prompt-free
#: catalog read (``catalog.refresh``); a real session always has its own.
CATALOG_PROBE_PROMPT = "This OpenCode process only answers a Cortex catalog read; no task is running."

CODE_MODE_NOTE = """\
# Tools in this engine

- MCP servers attached to this session, `cortex` included, are not separate tools here. Call them from inside the `execute` tool as `tools.<server>.<tool>(input)`, for example `tools.cortex.read_report(...)`, and await each call. Inside `execute`, `search({ namespace: "cortex" })` lists a server's tools with their exact paths and signatures; the catalog shown in this prompt is partial.
- Everything this prompt says about the `cortex` server holds unchanged: the same tools, schemas and authority, only reached through `execute`.
- The `browser` tools need OpenCode's desktop app and do not work in Cortex sessions."""


def build_opencode_config(prompt_path: str) -> dict:
    """The process-level configuration every Cortex OpenCode process runs with."""
    if not isinstance(prompt_path, str) or not prompt_path.startswith("/") or "}" in prompt_path:
        raise ValueError("prompt_path must be an absolute path without '}'")
    return {
        "autoupdate": False,
        "share": "disabled",
        "default_agent": AGENT,
        "agent": {
            AGENT: {"mode": "primary", "prompt": "{file:" + prompt_path + "}"},
            "build": {"disable": True},
            "plan": {"disable": True},
            "title": {"disable": True},
        },
        "permission": {"*": "ask", **{name: "deny" for name in DENIED_PERMISSIONS}},
    }


def process_environment(prompt_path: str) -> dict[str, str]:
    """The ``pinned_engine_vars`` ``connections.environment.build_process_env``
    accepts for an OpenCode connection."""
    return {
        "OPENCODE_CONFIG_CONTENT": json.dumps(build_opencode_config(prompt_path), sort_keys=True),
        "OPENCODE_DISABLE_PROJECT_CONFIG": "1",
        "OPENCODE_DISABLE_AUTOUPDATE": "1",
    }


def project_rules(project_root: str | None) -> str:
    """The project's own agent rules. OpenCode stops loading ``AGENTS.md``
    once project configuration is off (observed), so Cortex hands them over."""
    from ..delegation.live_adapters import PROJECT_RULE_FILES, _PROJECT_RULES_MAX_CHARS

    if not project_root:
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
    return "## Project rules (follow them)\n\n" + "\n\n".join(parts) if parts else ""


def build_agent_prompt(instructions: str, *, role: str, project_root: str | None, environ=None) -> str:
    """The whole text the ``cortex`` agent runs with: Cortex's unified base
    prompt (unless ``CORTEX_UNIFIED_BASE_PROMPT=0``), how MCP tools are reached
    here, the role instructions and the project's own rules."""
    from ..delegation.live_adapters import unified_base_prompt_enabled
    from .unified_system_prompt import cortex_system_prompt

    if role not in ("coordinator", "worker"):
        raise ValueError("role must be 'coordinator' or 'worker'")
    if not isinstance(instructions, str) or not instructions.strip():
        raise ValueError("instructions must be a non-empty string")
    parts = []
    if unified_base_prompt_enabled(environ):
        parts.append(cortex_system_prompt(role, project_root, environ=environ).rstrip())
    parts += [CODE_MODE_NOTE, instructions.strip(), project_rules(project_root)]
    return "\n\n".join(part for part in parts if part) + "\n"


def _find_option(observed_config_options, option_id: str):
    for item in observed_config_options or ():
        if isinstance(item, dict) and item.get("id") == option_id:
            return item
    return None


def _require_offered(option_id: str, value, observed_config_options) -> tuple[str, object]:
    option = _find_option(observed_config_options, option_id)
    if option is None:
        raise ConfigurationRejected(f"opencode_acp_option_not_offered:{option_id}")
    offered = {choice.get("value") for choice in option.get("options") or () if isinstance(choice, dict)}
    if value not in offered:
        raise ConfigurationRejected(f"opencode_acp_option_value_unsupported:{option_id}")
    return (option_id, value)


def opencode_option_mapping(revision, observed_config_options) -> list[tuple[str, object]]:
    """``ConfigurationRevision`` -> ``session/set_config_option`` pairs, each
    pre-checked against the session's own observed options.

    ``model`` is always set (OpenCode's own default is a free third-party
    model); ``effort`` only when the route carries one; ``mode`` only as
    :data:`AGENT`, the one agent Cortex offers.

    Which efforts OpenCode offers depends on the model (none at all for some),
    so an effort is pre-checked here only when the route's model is already
    the session's current one. Otherwise ``EngineSessionManager.configure``
    checks it against the options the model switch itself returned, before
    sending it.
    """
    mapping = [_require_offered(_MODEL_OPTION_ID, revision.route.model, observed_config_options)]
    parameters = dict(revision.route.supported_parameters)
    model_option = _find_option(observed_config_options, _MODEL_OPTION_ID) or {}
    for name in _EFFORT_PARAM_NAMES:
        if name in parameters:
            if model_option.get("currentValue") == revision.route.model:
                mapping.append(_require_offered(_EFFORT_OPTION_ID, parameters[name], observed_config_options))
            else:
                mapping.append((_EFFORT_OPTION_ID, parameters[name]))
            break
    if "mode" in parameters:
        if parameters["mode"] != AGENT:
            raise ConfigurationRejected(f"opencode_acp_option_value_unsupported:{_MODE_OPTION_ID}")
        mapping.append(_require_offered(_MODE_OPTION_ID, AGENT, observed_config_options))
    return mapping
