"""Desktop owner onboarding. Keys enter only through the private helper stdin."""
import os
from pathlib import Path
import platform
import subprocess
from types import SimpleNamespace
import machine_tools as m


def installed(engine):
    from cortex_runtime.configuration import management
    from cortex_runtime.connections import discovery
    from cortex_runtime.engine.install_resolution import resolve_verified_install
    if engine not in ("codex-acp", "claude-agent-acp"):
        raise m.Refused("unsupported_engine", "Choose Codex or Claude.")
    found = management._discover_connection_engine(engine, dict(os.environ))
    if not found or not discovery.is_qualified(engine, found.version):
        raise m.Refused("engine_unavailable", "Install the qualified bundled adapter first.")
    install = resolve_verified_install(SimpleNamespace(engine=engine, launch_command=found.launch_command))
    return found, install


def native_command(engine):
    found, install = installed(engine)
    if engine == "codex-acp":
        program = Path(install.bundled_codex_binary)
        args = ["login"]
    else:
        # The locked SDK ships a real native executable for this architecture.
        arch = {"x86_64": "x64", "aarch64": "arm64"}.get(platform.machine())
        from cortex_runtime.engine.install_resolution import _resolve_node_package_dir
        base = Path(install.package_json).parent
        package = _resolve_node_package_dir(base, "@anthropic-ai/claude-agent-sdk-linux-" + (arch or ""), "package.json")
        program = package / "claude" if package else Path("/nonexistent")
        args = ["auth", "login"]
    if not program.is_file() or not os.access(program, os.X_OK):
        raise m.Refused("native_cli_missing", "This adapter does not contain the native login executable.")
    return {"program": str(program.resolve()), "args": args, "adapter": list(found.launch_command)}


def add(params):
    from cortex_runtime.connections import onboarding
    from cortex_runtime.connections.model import AuthMode
    from cortex_runtime.portable.errors import PortableError
    engine, name, key = params.get("engine"), params.get("name"), params.get("key")
    if not isinstance(name, str) or not name.strip() or len(name) > 100:
        raise m.Refused("invalid_name", "Enter a connection name up to 100 characters.")
    if not isinstance(key, str) or not key.strip() or len(key) > 16384 or "\n" in key or "\r" in key or "\0" in key:
        raise m.Refused("invalid_key", "Enter a non-empty, single-line API key.")
    found, _ = installed(engine)
    record = None
    try:
        expected = params.get("revision")
        if not isinstance(expected, str) or not expected:
            raise m.Refused("revision_conflict", "Refresh connections before adding this connection.")
        record = onboarding.create_connection(
            name=name.strip(), engine=engine, provider="openai" if engine == "codex-acp" else "anthropic",
            auth_mode=AuthMode.API_KEY, expected_revision=expected,
            key_kind="openai_api_key" if engine == "codex-acp" else "anthropic_api_key",
            launch_command=tuple(found.launch_command))
        onboarding.set_api_key(record.id, reader=lambda: key.encode("utf-8"))
        onboarding.validate(record.id)
        return {"connectionId": record.id, "message": "Key saved privately. Refresh models to observe the available catalogue."}
    except m.Refused:
        raise
    except PortableError as error:
        if record is not None:
            onboarding.remove(record.id)
        if error.code == "revision_conflict":
            raise m.Refused("revision_conflict", "Connections changed. Refresh before retrying.") from None
        raise m.Refused("connection_failed", "The connection could not be saved. Refresh before retrying; no key is shown.") from None
    except Exception:
        if record is not None:
            onboarding.remove(record.id)
        raise m.Refused("connection_failed", "The connection could not be saved. Refresh before retrying; no key is shown.") from None


def _action(params):
    if params.get("action") == "add_key":
        return add(params)
    if params.get("action") == "login_command":
        return native_command(params.get("engine"))
    if params.get("action") == "recheck":
        from cortex_runtime.configuration import management
        engine = params.get("engine")
        command = native_command(engine)
        status_args = ["login", "status"] if engine == "codex-acp" else ["auth", "status"]
        status = subprocess.run([command["program"], *status_args], stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=8, cwd=str(Path.home()))
        if status.returncode:
            raise m.Refused("native_login_required", "The native CLI reports no active login. Finish signing in in the terminal, then recheck.")
        current = management.read_connections()["revision"]
        from cortex_runtime.connections import onboarding
        record = onboarding.create_referenced_connection(
            name=("Codex" if engine == "codex-acp" else "Claude") + " native account",
            engine=engine, provider="openai" if engine == "codex-acp" else "anthropic",
            launch_command=tuple(command["adapter"]), reuse=True, expected_revision=current)
        return {"connectionId":record.id, "message":"Native login checked. Refresh models to observe available routes."}
    raise m.Refused("unsupported_action", "Unsupported connection action.")


def action(params):
    from cortex_runtime.portable.errors import PortableError
    try:
        return _action(params)
    except m.Refused:
        raise
    except PortableError:
        raise m.Refused("engine_unavailable", "The engine install or native account is not ready. Check the adapter and login, then recheck.") from None
    except (OSError, ValueError, subprocess.SubprocessError):
        raise m.Refused("engine_unavailable", "The engine could not be checked. No login or key was copied.") from None
