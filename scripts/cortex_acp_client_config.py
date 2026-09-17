#!/usr/bin/env python3
"""Non-destructive Zed Custom Agent registration helper for Cortex (T011).

Zed 1.20.1's ``agent_servers`` setting (see ``docs/acp/zed-custom-agent.md``)
registers Cortex as a separate, explicitly-selected custom agent -- never an
automatic activation and never routed through Zed's own built-in provider
credentials (see ``INVARIANTS.md``: "No undocumented Zed-credential
inheritance"). This helper never writes anything by default: it only prints
the exact JSON entry to add and, when a settings path is given, a one-line
state summary ("would add" / "would replace" / "already identical") --
never a full-file diff or any other key's value, so an unrelated entry's
secret-looking value in the same settings file is never echoed to
stdout/stderr. A real write requires both ``--apply`` and the explicit
``--i-consent-to-write`` flag, targets only the one path given (never an
implicit default settings location, and never a symlinked path or a path
with a symlinked parent component -- pass the real target path explicitly
instead), and always writes a timestamped backup first (never overwriting
a same-second prior backup: a numeric suffix is added on collision).

Also prints the codeg 0.30.8 Custom Agent recipe text (``--print-codeg-recipe``
-- see ``docs/acp/codeg-custom-agent.md`` for the full, explicitly
unverified/unknown-marked recipe; this script never attempts to write
codeg's own configuration).
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import shutil
import stat
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
AGENT_ENTRY = REPO_ROOT / "plugins" / "cortex" / "scripts" / "cortex_acp.py"

AGENT_NAME = "Cortex"

_JSONC_MANUAL_INSTRUCTIONS = """\
{settings_path} does not parse as strict JSON (Zed settings files may use
JSONC -- comments and trailing commas -- which this helper deliberately
refuses to parse-and-rewrite, since doing so could silently drop comments
or reformat unrelated content). Add the entry manually instead:

1. Open {settings_path} in Zed's own settings editor (or any text editor).
2. Find (or create) the top-level "agent_servers" object.
3. Add this entry inside it, preserving every other key:

{entry_json}

4. Save the file. Zed will list "{agent_name}" as a separate custom agent
   the next time you open the Agent panel; it is never auto-activated.
"""


class SettingsPathUnsafe(Exception):
    """Raised when ``--settings`` itself, or one of its parent directory
    components, is a symlink."""


def _refuse_if_symlinked(path):
    """Refuse a symlinked settings path (F4): never read-then-replace
    through a symlink -- an attacker (or an unrelated concurrent process)
    could otherwise point ``settings.json`` at an arbitrary real file after
    this helper has already decided it looks safe, and an atomic
    ``os.replace`` on the symlink path would silently replace it with a
    *regular* file at that location, destroying the symlink itself. Every
    existing parent directory component is checked too, when cheap (a
    single ``lstat`` per component); a component that cannot be inspected
    (e.g. it does not exist yet) is not itself unsafe on its own.
    """
    if path.is_symlink():
        raise SettingsPathUnsafe(
            f"{path} is a symlink; refusing to read or write through it. "
            f"Pass the real target path explicitly instead (e.g. $(realpath {path}))."
        )
    current = Path(path.anchor)
    for part in path.parts[1:-1]:
        current = current / part
        try:
            is_link = current.is_symlink()
        except OSError:
            continue
        if is_link:
            raise SettingsPathUnsafe(
                f"{current} (a parent directory of {path}) is a symlink; refusing to "
                "read or write through it. Pass the real target path explicitly instead."
            )


def _default_python3():
    return shutil.which("python3") or sys.executable


def build_entry(python3_path=None):
    """The exact Zed ``agent_servers`` entry for Cortex.

    ``env`` is deliberately an empty object: Zed's custom agent launch
    never inherits Zed's own provider credentials (INVARIANTS: "No
    undocumented Zed-credential inheritance"), and Cortex reads no secret
    from its own process environment either (connections/credentials are
    resolved from the user-scoped connection registry, never an env var
    injected by the host editor).
    """
    python3_path = python3_path or _default_python3()
    return {
        AGENT_NAME: {
            "type": "custom",
            "command": str(python3_path),
            "args": ["-B", str(AGENT_ENTRY)],
            "env": {},
        }
    }


def print_entry(entry):
    print(json.dumps(entry, indent=2))


def _read_strict_json(path):
    """Return ``(data, raw_text)`` iff ``path`` parses as strict JSON;
    ``(None, raw_text)`` otherwise (including a nonexistent file, whose
    ``raw_text`` is ``""``)."""
    if not path.exists():
        return None, ""
    raw_text = path.read_text(encoding="utf-8")
    try:
        return json.loads(raw_text), raw_text
    except json.JSONDecodeError:
        return None, raw_text


def merge_entry(data, entry, *, replace):
    """Return ``(new_data, conflict)``. ``conflict`` is True iff
    ``agent_servers.Cortex`` already exists with a *different* value and
    ``replace`` is False (in which case ``new_data`` is ``data``,
    unchanged)."""
    new_data = dict(data) if isinstance(data, dict) else {}
    agent_servers = dict(new_data.get("agent_servers") or {})
    existing = agent_servers.get(AGENT_NAME)
    if existing is not None and existing != entry[AGENT_NAME] and not replace:
        return data, True
    agent_servers[AGENT_NAME] = entry[AGENT_NAME]
    new_data["agent_servers"] = agent_servers
    return new_data, False


def _render(data):
    return json.dumps(data, indent=2) + "\n"


def describe_state(data, entry):
    """One of ``"identical"``, ``"would_add"``, ``"would_replace"`` -- never
    a diff, and never touches/prints any key other than ``agent_servers.Cortex``
    (F3: a full-file diff would echo unrelated entries' values, including
    anything secret-looking, to stdout)."""
    existing = (data or {}).get("agent_servers", {}).get(AGENT_NAME) if isinstance(data, dict) else None
    if existing == entry[AGENT_NAME]:
        return "identical"
    if existing is None:
        return "would_add"
    return "would_replace"


_STATE_MESSAGES = {
    "identical": "already identical; nothing would change.",
    "would_add": "would add a new 'Cortex' agent_servers entry.",
    "would_replace": "would replace the existing, different 'Cortex' agent_servers "
                      "entry (pass --replace together with --apply to actually do this).",
}


def _write_backup(path, mode):
    """Write a timestamped backup of ``path``'s current content, never
    overwriting a prior backup created within the same second (F2): tries
    ``O_CREAT|O_EXCL`` first at the plain timestamped name, then appends a
    growing numeric suffix on each collision until an unused name is
    found."""
    timestamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%S")
    data = path.read_bytes()
    suffix = 0
    while True:
        name = f"{path.name}.bak.{timestamp}" if suffix == 0 else f"{path.name}.bak.{timestamp}.{suffix}"
        candidate = path.with_name(name)
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        try:
            fd = os.open(candidate, flags, 0o600)
        except FileExistsError:
            suffix += 1
            continue
        break
    with os.fdopen(fd, "wb") as handle:
        handle.write(data)
    if mode is not None:
        os.chmod(candidate, mode)
    return candidate


def atomic_write_with_backup(path, new_text):
    """Write ``new_text`` to ``path`` atomically, after writing a
    timestamped backup of the current content (if any), preserving the
    original file mode. Refuses (F4) if ``path`` or a parent directory
    component is a symlink -- see ``_refuse_if_symlinked``."""
    _refuse_if_symlinked(path)
    mode = None
    if path.exists():
        mode = stat.S_IMODE(path.stat().st_mode)
        _write_backup(path, mode)
    fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(new_text)
        os.replace(tmp_name, path)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise
    if mode is not None:
        os.chmod(path, mode)


CODEG_RECIPE = """\
codeg 0.30.8 Custom Agent recipe (unverified -- see docs/acp/codeg-custom-agent.md)
====================================================================================

codeg's own custom-agent registration path (Settings -> Agents) is documented
upstream as supporting npx/uvx/binary distribution channels; whether it also
accepts an arbitrary local command (the same "python3 -B <abs cortex_acp.py>"
launch Zed uses) is NOT verified by this script and is recorded as an open
question in docs/acp/codeg-custom-agent.md. Do not assume support.

Candidate manual steps (verify against your installed codeg before relying on
them):

1. Open codeg -> Settings -> Agents -> Add custom agent.
2. If codeg offers a "binary"/local-command channel: point it at
   {python3} with arguments: -B {entry}
3. If codeg only offers npx/uvx/registry channels: local script launch is not
   directly supported yet; packaging a wrapper executable is a possible future
   step (see T031), not something this script does.
4. codeg injects its own "codeg-mcp" MCP server into every hosted agent's
   session; Cortex treats any injected MCP server as untrusted, inert
   metadata (see cortex_runtime/acp/protocol.py: strip_mcp_servers) and never
   routes Cortex's own identity through it.

This script never writes codeg's own configuration files.
""".format(python3=_default_python3(), entry=AGENT_ENTRY)


def cmd_print_codeg_recipe():
    print(CODEG_RECIPE)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--settings", type=Path, default=None,
                         help="Path to a Zed settings.json to preview/modify. Never a default "
                              "location -- must be given explicitly.")
    parser.add_argument("--apply", action="store_true",
                         help="Write the merged entry to --settings (requires --i-consent-to-write).")
    parser.add_argument("--i-consent-to-write", action="store_true", dest="consent",
                         help="Explicit consent required for --apply to actually write.")
    parser.add_argument("--replace", action="store_true",
                         help="Allow overwriting an existing, different 'Cortex' agent_servers entry.")
    parser.add_argument("--python3", type=str, default=None,
                         help="Override the detected absolute python3 path (for testing).")
    parser.add_argument("--print-codeg-recipe", action="store_true",
                         help="Print the codeg custom-agent recipe text and exit.")
    args = parser.parse_args(argv)

    if args.print_codeg_recipe:
        cmd_print_codeg_recipe()
        return 0

    entry = build_entry(args.python3)

    if args.settings is not None:
        try:
            _refuse_if_symlinked(args.settings)
        except SettingsPathUnsafe as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 2

    if not args.apply:
        print("Zed agent_servers entry to add for Cortex:\n")
        print_entry(entry)
        if args.settings is not None:
            data, _raw_text = _read_strict_json(args.settings)
            if data is None and args.settings.exists():
                print(f"\n{args.settings} is not strict JSON; showing manual instructions instead:\n")
                print(_JSONC_MANUAL_INSTRUCTIONS.format(
                    settings_path=args.settings, entry_json=json.dumps(entry, indent=2),
                    agent_name=AGENT_NAME))
                return 0
            state = describe_state(data, entry)
            print(f"\n{args.settings}: {_STATE_MESSAGES[state]}")
        return 0

    # --apply path
    if args.settings is None:
        print("error: --apply requires --settings PATH.", file=sys.stderr)
        return 2
    if not args.consent:
        print("error: --apply requires --i-consent-to-write (explicit consent for a real write).",
              file=sys.stderr)
        return 2

    data, raw_text = _read_strict_json(args.settings)
    if data is None and args.settings.exists():
        print(f"refusing to write: {args.settings} is not strict JSON (Zed allows JSONC). "
              "Manual instructions:\n", file=sys.stderr)
        print(_JSONC_MANUAL_INSTRUCTIONS.format(
            settings_path=args.settings, entry_json=json.dumps(entry, indent=2), agent_name=AGENT_NAME))
        return 2
    if data is None:
        data = {}

    merged, conflict = merge_entry(data, entry, replace=args.replace)
    if conflict:
        existing = data.get("agent_servers", {}).get(AGENT_NAME)
        print(f"refusing to overwrite an existing, different 'Cortex' agent_servers entry "
              f"(pass --replace to overwrite):\ncurrent: {json.dumps(existing, indent=2)}\n"
              f"proposed: {json.dumps(entry[AGENT_NAME], indent=2)}", file=sys.stderr)
        return 2

    new_text = _render(merged)
    if new_text == raw_text:
        print(f"{args.settings} already contains the exact Cortex entry; nothing to write.")
        return 0

    atomic_write_with_backup(args.settings, new_text)
    print(f"wrote {args.settings} (backup created alongside it).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
