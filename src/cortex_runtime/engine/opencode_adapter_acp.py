"""OpenCode v2 adapter (``opencode acp`` 2.x): configuration, role prompt, options.

The behavior here was observed against the installed 2.0.19 binary through raw
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
  the tested 2.0.19 ignores ``session/new`` ``_meta.systemPrompt``, while an agent
  prompt was observed to replace OpenCode's own base prompt;
- ``plugin``: only removals, never a plugin to load. ``-opencode.agent`` and
  ``-opencode.plan`` keep OpenCode's built-in plugins that create ``build``,
  ``plan`` and the helper agents from ever running. OpenCode 2.0.20's
  ``opencode acp`` answers ``session/new`` from a per-directory snapshot taken
  as soon as any model and any primary agent exist; with a native login in
  ``opencode.db`` the login's models exist at once, and the built-in ``build``
  agent used to win the race against the configured ``cortex`` agent and the
  configured providers (about 25-50 ms later). The server resolves plugin
  removals from configuration before it starts any plugin, so with no built-in
  primary agent the snapshot waits for ``cortex`` and the owner's providers;
- ``permission``: ``"*": "ask"``, so every native tool call becomes an ACP
  ``session/request_permission`` (OpenCode's own default ran a shell ``rm``
  without asking), and ``task`` (its ``subagent`` tool), ``question`` and
  ``skill`` denied, which removes those tools. ``execute`` (Code Mode) is
  kept because the tested 2.0.19 reaches session MCP servers -- Cortex's own gateway
  included -- only through it, as ``tools.<server>.<tool>``. One ``execute``
  call is one permission request; the calls inside it are not asked one by
  one. Its catalog holds the session's MCP servers, ``opencode`` (session
  move/rename, model search, MCP resources) and ``browser`` (unusable
  without a connected desktop app), and no shell or file tools;
- the ``title`` agent disabled, so no extra request goes to a small model
  OpenCode would pick itself; ``autoupdate`` off; ``share`` disabled.

No model or MCP server is configured: the model is set per session through
``session/set_config_option`` (confirm-or-fail, :func:`opencode_option_mapping`)
and MCP servers are passed only on ``session/new``. With no model set,
OpenCode picks a free OpenCode Zen model, so a session is never prompted
before Cortex confirms the route's model.

Providers. Cortex imports only provider definitions and provider on/off lists
from the owner's configuration. Recognized credentials and substituted strings
use read-only in-memory jail files, never environment values. OpenCode itself
opens the owner's native opencode.db in place; Cortex never extracts its logins
or selects another database. The catalog and mode guards still reject an
observation without the ``cortex`` mode rather than running as build/plan.
"""
from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass, field
from pathlib import Path

from ..configuration.service import ConfigurationRejected

#: The one agent Cortex sessions run as; OpenCode offers it as the ``mode``.
AGENT = "cortex"
#: Where the jail mounts the agent prompt, inside the engine's scratch dir.
PROMPT_FILE_NAME = "cortex-opencode-agent.md"
#: OpenCode permission keys Cortex denies (removes the tool).
DENIED_PERMISSIONS = ("task", "question", "skill")
#: OpenCode's built-in plugins Cortex removes: they create ``build``/``plan``
#: (and the helper agents), which would otherwise be offered before ``cortex``.
REMOVED_BUILTIN_PLUGINS = ("opencode.agent", "opencode.plan")

_MODEL_OPTION_ID = "model"
_EFFORT_OPTION_ID = "effort"
_MODE_OPTION_ID = "mode"
_EFFORT_PARAM_NAMES = ("thought_level", "effort")

#: Where OpenCode reads the owner's own configuration (hidden in the jail).
USER_CONFIG_RELATIVE = (".config", "opencode")
#: The owner's global configuration files, in OpenCode's own merge order
#: (a later file overrides an earlier one).
OWNER_CONFIG_FILES = ("config.json", "opencode.json", "opencode.jsonc")
#: The only owner configuration keys Cortex carries into its own.
OWNER_PROVIDER_KEYS = ("provider", "enabled_providers", "disabled_providers")
#: Jail file names holding one owner provider credential each.
CREDENTIAL_FILE_PREFIX = "cortex-opencode-credential-"
_OWNER_CONFIG_MAX_BYTES = 1 << 20
_OWNER_FILE_REF_MAX_BYTES = 64 << 10
#: The imported configuration travels in ``OPENCODE_CONFIG_CONTENT``; one
#: environment string must stay far below the kernel's 128 KiB limit.
_OWNER_PROVIDERS_MAX_CHARS = 64 << 10
_PROVIDER_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
#: The AI SDK's own provider packages; any other package is code Cortex
#: would load on the owner's behalf, so such a provider is left out.
_ALLOWED_NPM = re.compile(r"^@ai-sdk/[a-z0-9][a-z0-9-]{0,63}$")
#: A value under such a key, or anywhere under ``headers``, is a credential.
_CREDENTIAL_KEY = re.compile(r"api[_-]?key|token|secret|password|authorization|cookie", re.IGNORECASE)
_SUBSTITUTION = re.compile(r"\{(env|file):([^}]*)\}")

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
        "plugin": ["-" + name for name in REMOVED_BUILTIN_PLUGINS],
        "agent": {
            AGENT: {"mode": "primary", "prompt": "{file:" + prompt_path + "}"},
            "build": {"disable": True},
            "plan": {"disable": True},
            "title": {"disable": True},
        },
        "permission": {"*": "ask", **{name: "deny" for name in DENIED_PERMISSIONS}},
    }


def process_environment(prompt_path: str, owner_config: dict | None = None) -> dict[str, str]:
    """The ``pinned_engine_vars`` ``connections.environment.build_process_env``
    accepts for an OpenCode connection.

    ``owner_config`` is :attr:`OwnerProviders.config`: the owner's provider
    definitions join Cortex's configuration, where a provider Cortex itself
    names wins, and nothing else of the owner's is taken.
    """
    config = build_opencode_config(prompt_path)
    if owner_config:
        providers = dict(owner_config.get("provider") or {})
        providers.update(config.get("provider") or {})
        if providers:
            config["provider"] = providers
        for key in ("enabled_providers", "disabled_providers"):
            if key in owner_config and key not in config:
                config[key] = list(owner_config[key])
    return {
        "OPENCODE_CONFIG_CONTENT": json.dumps(config, sort_keys=True),
        "OPENCODE_DISABLE_PROJECT_CONFIG": "1",
        "OPENCODE_DISABLE_AUTOUPDATE": "1",
    }


# --- the owner's own providers -------------------------------------------------

@dataclass(frozen=True)
class OwnerProviders:
    """What :func:`owner_provider_config` carries into a Cortex process.

    ``config`` holds only :data:`OWNER_PROVIDER_KEYS`, with every recognized credential or substituted
    string replaced by ``{file:<path>}``; ``files`` maps those jail paths to
    the credential bytes (``engine_jail(data_files=)``). ``imported`` (from
    the configuration files) and
    ``skipped`` (``(provider, reason)``) name providers only, never values.
    """

    config: dict = field(default_factory=dict)
    files: dict = field(default_factory=dict)
    imported: tuple = ()
    skipped: tuple = ()


class _Skip(Exception):
    """One provider cannot be carried over faithfully; its reason code."""


class _CredentialRef(int):
    """Index of one credential in the provider's ``credentials`` list."""


_OMIT = object()


def _strip_jsonc(text: str) -> str:
    """JSONC (comments, trailing commas) to JSON, leaving strings intact."""
    def scan(source, handle):
        out, i, n = [], 0, len(source)
        while i < n:
            if source[i] == '"':
                j = i + 1
                while j < n and source[j] != '"':
                    j += 2 if source[j] == "\\" else 1
                out.append(source[i:j + 1])
                i = j + 1
            else:
                i = handle(source, i, out)
        return "".join(out)

    def comments(source, i, out):
        if source.startswith("//", i):
            end = source.find("\n", i)
            return len(source) if end < 0 else end
        if source.startswith("/*", i):
            end = source.find("*/", i + 2)
            if end < 0:
                raise ValueError("unterminated comment")
            return end + 2
        out.append(source[i])
        return i + 1

    def trailing_commas(source, i, out):
        if source[i] == ",":
            j = i + 1
            while j < len(source) and source[j] in " \t\r\n":
                j += 1
            if j < len(source) and source[j] in "}]":
                return i + 1
        out.append(source[i])
        return i + 1

    return scan(scan(text, comments), trailing_commas)


def _merge(base: dict, extra: dict) -> dict:
    merged = dict(base)
    for key, value in extra.items():
        if isinstance(value, dict) and isinstance(merged.get(key), dict):
            merged[key] = _merge(merged[key], value)
        else:
            merged[key] = value
    return merged


def _read_bounded(path: Path, limit: int) -> bytes | None:
    try:
        if not path.is_file():
            return None
        with open(path, "rb") as stream:
            data = stream.read(limit + 1)
    except OSError:
        return None
    return None if len(data) > limit else data


def _read_owner_configuration(directory: Path) -> tuple[dict, list[str]]:
    merged, problems = {}, []
    for name in OWNER_CONFIG_FILES:
        path = directory / name
        if not path.exists():
            continue
        data = _read_bounded(path, _OWNER_CONFIG_MAX_BYTES)
        try:
            parsed = json.loads(_strip_jsonc(data.decode("utf-8"))) if data is not None else None
        except (UnicodeDecodeError, ValueError):
            parsed = None
        if not isinstance(parsed, dict):
            problems.append(f"{name}:unreadable")
            continue
        merged = _merge(merged, {key: parsed[key] for key in OWNER_PROVIDER_KEYS if key in parsed})
    return merged, problems


def _substitute(value: str, *, home: str, config_dir: Path, environ) -> str:
    """OpenCode's own ``{env:NAME}``/``{file:path}`` substitution, done by
    Cortex: inside the jail neither the owner's environment nor, for a file
    under the hidden configuration directory, the file is visible."""
    def replace(match):
        kind, target = match.group(1), match.group(2).strip()
        if kind == "env":
            return str((environ or {}).get(target, ""))
        if target.startswith("~/"):
            path = Path(home) / target[2:]
        else:
            path = Path(target) if target.startswith("/") else config_dir / target
        data = _read_bounded(path, _OWNER_FILE_REF_MAX_BYTES)
        if data is None:
            raise _Skip("file_reference_unreadable")
        try:
            return data.decode("utf-8").strip()
        except UnicodeDecodeError:
            raise _Skip("file_reference_unreadable") from None

    return _SUBSTITUTION.sub(replace, value)


def _sanitize_provider(definition, *, home, config_dir, environ, credentials: list) -> dict:
    """The provider as Cortex passes it on: substitutions resolved, every
    substituted value and recognized credential moved into ``credentials`` (replaced by a
    :class:`_CredentialRef`), an empty credential dropped. Refused
    (:class:`_Skip`) when it would load a non-``@ai-sdk/`` package or keep a
    substitution Cortex cannot resolve inline."""
    if not isinstance(definition, dict):
        raise _Skip("invalid_definition")

    def walk(node, secret):
        if isinstance(node, dict):
            out = {}
            for name, value in node.items():
                if not isinstance(name, str) or _SUBSTITUTION.search(name):
                    raise _Skip("unsupported_substitution")
                if name == "npm" and not (isinstance(value, str) and _ALLOWED_NPM.match(value)):
                    raise _Skip("package_not_allowed")
                resolved = walk(value, secret or name == "headers" or bool(_CREDENTIAL_KEY.search(name)))
                if resolved is not _OMIT:
                    out[name] = resolved
            return out
        if isinstance(node, list):
            return [item for item in (walk(value, secret) for value in node) if item is not _OMIT]
        if isinstance(node, str):
            value = _substitute(node, home=home, config_dir=config_dir, environ=environ)
            # Provider-specific credential names cannot be enumerated. Never
            # serialize an expanded owner value into OPENCODE_CONFIG_CONTENT.
            if secret or _SUBSTITUTION.search(node):
                if not value:
                    return _OMIT
                credentials.append(value.encode("utf-8"))
                return _CredentialRef(len(credentials) - 1)
            if _SUBSTITUTION.search(value):
                raise _Skip("unsupported_substitution")
            return value
        return node

    return walk(definition, False)


def owner_provider_config(home: str | None, credential_dir: str, environ=None) -> OwnerProviders:
    """The owner's OpenCode providers, ready for a Cortex process.

    ``home`` is the owner's home (``~/.config/opencode`` is read there, the
    directory the jail hides); ``credential_dir`` is the absolute jail
    directory the credential files are mounted into; ``environ`` resolves an
    ``{env:NAME}`` reference. OpenCode reads native database logins itself.
    Unreadable configuration yields nothing; one provider Cortex cannot carry
    faithfully is skipped, never guessed.
    """
    if not isinstance(credential_dir, str) or not credential_dir.startswith("/") or "}" in credential_dir:
        raise ValueError("credential_dir must be an absolute path without '}'")
    if home and str(home).startswith("/"):
        config_dir = Path(home).joinpath(*USER_CONFIG_RELATIVE)
        owner, problems = _read_owner_configuration(config_dir)
    else:
        config_dir, owner, problems = None, {}, []
    skipped = [("*", problem) for problem in problems]
    config, files, imported = {}, {}, []
    definitions = owner.get("provider")
    if definitions is not None and not isinstance(definitions, dict):
        skipped.append(("*", "invalid_provider_section"))
        definitions = {}
    providers = {}
    for provider_id, definition in sorted((definitions or {}).items()):
        if not _PROVIDER_ID.match(provider_id):
            skipped.append(("?", "invalid_provider_id"))
            continue
        credentials: list[bytes] = []
        try:
            sanitized = _sanitize_provider(definition, home=str(home), config_dir=config_dir, environ=environ,
                                           credentials=credentials)
        except _Skip as reason:
            skipped.append((provider_id, str(reason)))
            continue
        paths = [f"{credential_dir}/{CREDENTIAL_FILE_PREFIX}{len(files) + index}" for index in range(len(credentials))]

        def refer(node):
            if isinstance(node, _CredentialRef):
                return "{file:" + paths[int(node)] + "}"
            if isinstance(node, dict):
                return {name: refer(value) for name, value in node.items()}
            if isinstance(node, list):
                return [refer(item) for item in node]
            return node

        providers[provider_id] = refer(sanitized)
        files.update(zip(paths, credentials))
        imported.append(provider_id)
    if providers:
        config["provider"] = providers
    for key in ("enabled_providers", "disabled_providers"):
        value = owner.get(key)
        if value is None:
            continue
        if isinstance(value, list) and all(isinstance(item, str) and _PROVIDER_ID.match(item) for item in value):
            config[key] = list(value)
        else:
            skipped.append(("*", f"invalid_{key}"))
    if len(json.dumps(config, sort_keys=True)) > _OWNER_PROVIDERS_MAX_CHARS:
        return OwnerProviders(skipped=tuple(skipped) + (("*", "configuration_too_large"),))
    return OwnerProviders(config=config, files=files, imported=tuple(imported), skipped=tuple(skipped))


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
