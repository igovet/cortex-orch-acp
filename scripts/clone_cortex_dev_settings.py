#!/usr/bin/env python3
"""Copy non-secret Cortex settings into the dev channel; never copy account/state files."""
import argparse
import json
import os
from pathlib import Path
import re
import stat
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FILES = ("connections.json", "routing.json", "routing-profiles.json", "mcp-servers.json", "defaults.json")
SENSITIVE = re.compile(r"secret|token|password|credential|authorization|api.?key|cookie|headers?", re.I)
# Executable arguments, environment and URLs can contain secrets without a secret-looking key.
UNSAFE = {"env", "environment", "args", "launch_command", "endpoint", "url", "override_history",
          "pending", "history"}


def no_links(path):
    for part in (path, *path.parents):
        if part.is_symlink():
            raise ValueError("symbolic link refused")


def sanitize(value, skipped, prefix):
    if isinstance(value, dict):
        result = {}
        for key, item in value.items():
            if SENSITIVE.search(key) or key in UNSAFE:
                skipped.add(prefix + "." + key)
            else:
                result[key] = sanitize(item, skipped, prefix + "." + key)
        return result
    if isinstance(value, list):
        return [sanitize(item, skipped, prefix) for item in value]
    return value


def clone_settings(source, target, *, overwrite=False, production_paths=()):
    source, target = Path(source).absolute(), Path(target).absolute()
    if any(part in {".paseo", ".codex", ".claude"} for path in (source, target) for part in path.parts):
        raise ValueError("account and Paseo paths are not settings sources or targets")
    no_links(source)
    no_links(target)
    source, target = source.resolve(), target.resolve()
    protected = [source, *(Path(p).resolve() for p in production_paths)]
    if any(target == p or target.is_relative_to(p) or p.is_relative_to(target) for p in protected):
        raise ValueError("target overlaps production paths")
    if not source.is_dir():
        raise ValueError("production config directory missing")
    if target.exists() and not overwrite:
        raise ValueError("existing dev settings require --overwrite")
    skipped, documents = set(), {}
    # Read and sanitize everything before creating the target. Never walk secrets or state.
    for item in source.iterdir():
        if item.name not in FILES:
            skipped.add(item.name)
    for name in FILES:
        path = source / name
        no_links(path)
        if not path.exists():
            continue
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size > 4 * 1024 * 1024:
            raise ValueError("unsafe settings file")
        try:
            raw = json.loads(path.read_text())
        except (ValueError, UnicodeError):
            raise ValueError("invalid settings JSON") from None
        if not isinstance(raw, dict):
            raise ValueError("settings must be an object")
        cleaned = sanitize(raw, skipped, name)
        if name == "connections.json":
            records = {}
            for identity, record in raw.get("connections", {}).items():
                if not isinstance(record, dict) or record.get("auth_mode") != "native_account":
                    skipped.add(name + ".connections." + identity)
                    continue
                safe = sanitize(record, skipped, name + ".connections." + identity)
                # References are metadata only. Native account files are never opened or copied.
                reference = record.get("credential_ref")
                if (isinstance(reference, dict) and reference.get("kind") == "engine_native_dir"
                        and reference.get("owned") is False
                        and set(reference) <= {"kind", "path", "owned"}):
                    safe["credential_ref"] = reference
                else:
                    skipped.add(name + ".connections." + identity)
                    continue
                safe.update(launch_command=None, endpoint=None, key_kind=None, state="authorization_required",
                            approved_fingerprint="dev-review-required", qualified=False)
                records[identity] = safe
            cleaned["connections"] = records
            cleaned["override_history"] = {}
            cleaned["auto_connect_disabled"] = ["codex-acp", "claude-agent-acp", "opencode"]
        if name == "mcp-servers.json":
            for server in cleaned.get("servers", {}).values():
                if not isinstance(server, dict):
                    raise ValueError("invalid MCP settings")
                # Retain exact tool grants but require explicit review before launch.
                server.update(args=[], env={}, enabled=False)
        documents[name] = json.dumps(cleaned, indent=2) + "\n"
    if target.exists():
        if not target.is_dir():
            raise ValueError("dev target is not a directory")
        for name in documents:
            path = target / name
            no_links(path)
            if path.exists() and (not path.is_file() or path.stat().st_nlink != 1):
                raise ValueError("unsafe dev target file")
    target.mkdir(mode=0o700, parents=True, exist_ok=True)
    for name, body in documents.items():
        fd, temporary = tempfile.mkstemp(prefix=".clone-", dir=target)
        try:
            with os.fdopen(fd, "w") as stream:
                stream.write(body)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, target / name)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    return {"copied": sorted(documents), "skipped": sorted(skipped),
            "notice": "Native account references retained; connections need authorization; MCP servers disabled for review."}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--target", type=Path)
    parser.add_argument("--overwrite", action="store_true")
    args = parser.parse_args(argv)
    home = Path.home()
    config = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    state = Path(os.environ.get("XDG_STATE_HOME", home / ".local/state"))
    if not config.is_absolute() or not state.is_absolute():
        parser.error("absolute XDG paths required")
    table = json.loads((ROOT / "crates/workspace-protocol/channels.json").read_text())
    source = args.source or config / "cortex"
    target = args.target or config / table["dev"]["config_namespace"] / "cortex"
    try:
        result = clone_settings(source, target, overwrite=args.overwrite,
                                production_paths=(config / "cortex", state / "cortex",
                                                  home / ".codex", home / ".claude"))
    except (ValueError, OSError) as error:
        # Exceptions contain no source values or JSON decoder excerpts.
        parser.exit(1, ("clone refused: " + str(error) if isinstance(error, ValueError)
                        else "clone refused: filesystem error") + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
