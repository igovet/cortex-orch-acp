#!/usr/bin/env python3
"""Non-destructive, idempotent first-run setup for the Cortex ACP agent (T031).

Five explicit, separately invocable steps -- never a single opaque "do
everything" action that could silently combine consent for one step with
an unrelated one:

1. ``init-storage``   -- create (or verify) the owner-private
   ``$XDG_CONFIG_HOME/cortex`` / ``$XDG_STATE_HOME/cortex`` roots that
   ``cortex_runtime.connections.registry`` already uses; never touches any
   project's own ``.cortex`` store.
2. ``add-connection`` -- register one authorized connection via the
   existing ``connections.onboarding`` API. A credential value, when
   needed, is always read from a file path or stdin -- *never* accepted as
   a plain command-line argument (so it never appears in `ps`/shell
   history) -- and is written straight to the connection's private secret
   file; this script never logs, prints, or stores it anywhere else. A
   native-account connection instead prints (non-secret) instructions
   naming the isolated directory/env var for the user's own login flow,
   exactly like ``onboarding.mark_native_login_pending``.
3. ``pin-route``      -- build a catalog snapshot from one hand-declared
   qualified ``(engine, auth_mode, model)`` combination (never a live
   engine probe -- see ``catalog.discovery.ManualDeclaration``; only a
   *documented/qualified* combination is accepted at all) and persist it
   as the plain T008 user-default route/parameters
   (``configuration.store.write_user_defaults``) -- the exact mechanism
   ``acp.agent.Agent._apply_pre_task_selection`` already uses for a normal
   in-session ``/cortex-model`` choice. A fresh task's very first prompt
   then adopts this default automatically (see
   ``acp.agent.Agent._propose_pending_selection_for_new_task``) with no
   further per-task action.
4. ``record-observation`` -- replace a ``pin-route``-declared route with
   one built from a *real* engine-observed ``configOptions`` payload (see
   ``catalog.discovery.parse_observation``/``build_catalog``): the same
   ``session/new``-shaped JSON array a live engine actually returned,
   captured elsewhere (e.g. a client's own devtools, or an explicitly
   authorized qualification run) and saved to a file -- this step never
   itself contacts an engine. Unlike ``pin-route``'s
   ``ManualDeclaration`` (which is only ever accepted with **no**
   parameters, since nothing has confirmed what it actually supports),
   this records the connection's real, evidence-qualified
   ``ExecutionRoute.supported_parameters`` (e.g. codex-acp's
   ``reasoning_effort``/``thought_level`` choices) with DISCOVERY
   provenance, and re-pins it as the user default exactly like
   ``pin-route`` does -- this is what lets a value like "high effort"
   actually appear as a selectable option in a client's message panel.
5. ``refresh-catalog`` -- a prompt-free, on-demand catalog refresh for one
   connection (see ``cortex_runtime.catalog.refresh``): opens a real,
   short-lived engine session purely to read its advertised
   ``configOptions`` (``initialize`` + ``session/new``, never a
   ``session/prompt``), persists it with real ``DISCOVERY`` provenance, and
   closes the session. A real Cortex session already does this for free on
   every engine open, so this step is only needed to seed/refresh a
   connection's catalog without running a real task first.
   Never touches the pinned user-default route -- unlike ``pin-route``/
   ``record-observation``, this only refreshes the cached catalog snapshot
   a later selection reads from.
6. ``init-route-policy`` -- write a default, hand-editable
   ``~/.config/cortex/route-policy.json`` (D-T035-03, see
   ``cortex_runtime.configuration.route_policy`` for the schema/precedence)
   pre-populated permitting every role for both supported engines at a
   sensible effort range; never overwrites an existing file, here or
   anywhere else the read-precedence chain would already find one.
7. ``install-paseo-plugin`` -- validate and install the repository-owned
   Paseo 0.8 provider plugin through the installed Paseo CLI. This is the
   single explicit Cortex-on-Paseo installation command; it is idempotent,
   preserves unrelated daemon configuration/plugins, and verifies the plugin
   reaches ``running``. It never runs implicitly when an ACP session starts.
8. ``print-client-recipes`` -- delegates entirely to the existing T011
   ``cortex_acp_client_config.py`` (Zed dry-run-by-default entry +
   ``--print-codeg-recipe``); this script never re-implements or
   duplicates that write path.

``status`` (no step name, the default) only reads and reports current
state -- it is always safe to run with no flags.

Every step is idempotent: rerunning ``init-storage``/`add-connection
--reuse`/``pin-route``/``record-observation`` with the same inputs changes
nothing and reports the existing state rather than erroring or duplicating.
"""
from __future__ import annotations

import argparse
from dataclasses import fields
import json
import os
import shutil
import subprocess
import sys
import stat
import tempfile
import time
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
_ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(SCRIPTS.parent / "src"))

from cortex_runtime.catalog import discovery, refresh as catalog_refresh, store as catalog_store  # noqa: E402
from cortex_runtime.configuration import route_policy as route_policy_module  # noqa: E402
from cortex_runtime.configuration import store as configuration_store  # noqa: E402
from cortex_runtime.connections import discovery as connection_discovery, onboarding, registry  # noqa: E402
from cortex_runtime.connections.model import AuthMode, EngineNativeDir  # noqa: E402
from cortex_runtime.portable.errors import PortableError  # noqa: E402

import cortex_acp_client_config  # noqa: E402
from paseo_integrity import EXCLUDED_DIRS as _PASEO_EXCLUDED_DIRS, payload_digest as _paseo_payload_digest  # noqa: E402


PASEO_PLUGIN_DIR = _ROOT / "integrations" / "paseo"
_PASEO_CLI_CANDIDATES = ("paseo", "/opt/Paseo/resources/bin/paseo")

def _paseo_payload_sha256(root: Path) -> str:
    return _paseo_payload_digest(root)


def _verify_paseo_plugin_source(plugin_dir=None):
    root = Path(plugin_dir or PASEO_PLUGIN_DIR).resolve()
    if root != PASEO_PLUGIN_DIR.resolve() or not root.is_dir():
        raise SystemExit(f"error: Paseo plugin source must be {PASEO_PLUGIN_DIR}.")
    manifest_path = root / "paseo-plugin.json"
    integrity_path = root / "plugin-integrity.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        integrity = json.loads(integrity_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SystemExit(f"error: invalid Paseo plugin source metadata: {error}") from None
    if not isinstance(manifest, dict) or manifest.get("id") != "cortex-acp":
        raise SystemExit("error: Paseo plugin manifest must declare id 'cortex-acp'.")
    requirements = manifest.get("requirements")
    if not isinstance(requirements, dict) or not str(requirements.get("paseo", "")).startswith(">=0.8"):
        raise SystemExit("error: Paseo plugin manifest must require Paseo >=0.8.0.")
    actual = _paseo_payload_sha256(root)
    if not isinstance(integrity, dict) or integrity.get("payload_sha256") != actual:
        raise SystemExit("error: Paseo plugin source hash does not match its integrity record.")
    return root, manifest, actual


def _resolve_paseo_cli(environ=None):
    env = os.environ if environ is None else environ
    candidates = (env.get("CORTEX_PASEO_CLI"),) if env.get("CORTEX_PASEO_CLI") else _PASEO_CLI_CANDIDATES
    for candidate in candidates:
        if candidate and (Path(candidate).is_file() or ("/" not in candidate and shutil.which(candidate))):
            return candidate
    raise SystemExit("error: Paseo 0.8 CLI was not found; set CORTEX_PASEO_CLI.")


def _ensure_private_directory(path: Path, *, create=False) -> Path:
    """Validate one user-owned, non-symlinked private directory."""
    path = Path(path)
    try:
        info = path.lstat()
    except FileNotFoundError:
        if not create:
            raise SystemExit(f"error: required directory does not exist: {path}") from None
        try:
            path.mkdir(mode=0o700)
            info = path.lstat()
        except OSError as error:
            raise SystemExit(f"error: cannot create private directory {path}: {error}") from None
    except OSError as error:
        raise SystemExit(f"error: cannot inspect directory {path}: {error}") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise SystemExit(f"error: refusing symlinked or non-directory private path: {path}")
    if info.st_uid != os.geteuid():
        raise SystemExit(f"error: private directory is not owned by the current user: {path}")
    try:
        if stat.S_IMODE(info.st_mode) != 0o700:
            os.chmod(path, 0o700)
            info = path.lstat()
    except OSError as error:
        raise SystemExit(f"error: cannot secure private directory {path}: {error}") from None
    if stat.S_IMODE(info.st_mode) != 0o700 or stat.S_ISLNK(info.st_mode):
        raise SystemExit(f"error: private directory is not owner-only: {path}")
    return path


def _atomic_private_json(path: Path, body: str) -> None:
    """Write a private JSON file with an exclusive, same-directory temp file."""
    fd = None
    temporary = None
    try:
        fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            fd = None
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        temporary = None
    except OSError as error:
        raise SystemExit(f"error: cannot securely write configuration {path}: {error}") from None
    finally:
        if fd is not None:
            os.close(fd)
        if temporary is not None:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def _write_paseo_launch_config(environ=None):
    """Persist the Cortex argv and user storage roots used after restart.

    Paseo daemon plugins do not inherit the shell environment that ran setup.
    Keep only non-secret, absolute path settings here so a daemon restart
    continues to address the same user registry/native-account state.
    """
    env = os.environ if environ is None else environ
    home = Path(env.get("HOME") or str(Path.home())).expanduser()
    if not home.is_absolute() or home.is_symlink() or not home.is_dir():
        raise SystemExit(f"error: HOME must be an existing, non-symlink directory: {home}")
    config_base = Path(env.get("XDG_CONFIG_HOME") or str(home / ".config")).expanduser().resolve()
    state_base = Path(env.get("XDG_STATE_HOME") or str(home / ".local" / "state")).expanduser().resolve()
    desired = {
        "argv": ["python3", "-B", str((_ROOT / "src" / "cortex_acp.py").resolve())],
        "env": {"HOME": str(home), "XDG_CONFIG_HOME": str(config_base), "XDG_STATE_HOME": str(state_base)},
        "shim": str((_ROOT / "integrations" / "paseo" / "server" / "acp-load-shim.mjs").resolve()),
    }
    # The locator must not depend on XDG_CONFIG_HOME: a GUI daemon restart can
    # start with a different XDG environment. Keep the record under the
    # user's conventional HOME/.config path and persist custom XDG roots in
    # its payload.
    config_parent = _ensure_private_directory(home / ".config", create=True)
    config_root = _ensure_private_directory(config_parent / "cortex", create=True)
    path = config_root / "paseo-plugin-launch.json"
    if path.is_symlink():
        raise SystemExit(f"error: refusing symlinked Paseo launch configuration: {path}")
    if path.exists() and (stat.S_IMODE(path.stat().st_mode) != 0o600 or not path.is_file()):
        raise SystemExit(f"error: Paseo launch configuration must be a regular 0600 file: {path}")
    if path.exists():
        try:
            current = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise SystemExit(f"error: invalid existing Paseo launch configuration: {error}") from None
        # Upgrade the old argv-only record in place. Refuse any other shape
        # so setup cannot overwrite an unrelated owner-managed file.
        legacy_argv_only = {"argv": desired["argv"]}
        legacy_argv_env = {"argv": desired["argv"], "env": desired["env"]}
        if current != desired and current not in (legacy_argv_only, legacy_argv_env):
            raise SystemExit(f"error: refusing to overwrite unrelated launch configuration: {path}")
        if current != desired:
            _atomic_private_json(path, json.dumps(desired, indent=2) + "\n")
        return dict(path=str(path), created=False, argv=desired["argv"], env=desired["env"])
    _atomic_private_json(path, json.dumps(desired, indent=2) + "\n")
    return dict(path=str(path), created=True, argv=desired["argv"], env=desired["env"])


def _remove_new_paseo_launch_config(launch_config):
    """Remove only the record created by this install attempt."""
    if not launch_config or not launch_config.get("created"):
        return
    path = Path(str(launch_config.get("path", "")))
    try:
        info = path.lstat()
        if (
            stat.S_ISLNK(info.st_mode)
            or not stat.S_ISREG(info.st_mode)
            or info.st_uid != os.geteuid()
            or stat.S_IMODE(info.st_mode) != 0o600
        ):
            return
        path.unlink()
    except FileNotFoundError:
        return
    except OSError:
        return


def _is_owned_legacy_paseo_provider(entry) -> bool:
    """Recognize only the old generic Cortex ACP registration."""
    if not isinstance(entry, dict) or entry.get("extends") != "acp":
        return False
    command = entry.get("command")
    if not isinstance(command, list) or not command or not all(isinstance(part, str) and part for part in command):
        return False
    entrypoint = str((_ROOT / "src" / "cortex_acp.py").resolve())
    if command == ["python3", "-B", entrypoint] or command == ["/usr/bin/python3", "-B", entrypoint]:
        return True
    if len(command) != 1:
        return False
    launcher = Path(command[0])
    try:
        # The legacy installer commonly registered a stable symlink under
        # ~/.local/bin.  Follow it only for identity inspection; the exact
        # resolved launcher body must still name this repository entrypoint.
        launcher = launcher.resolve(strict=True)
        if not launcher.is_file():
            return False
        body = launcher.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return False
    return entrypoint in body and "cortex_acp.py" in body


def _migrate_legacy_paseo_provider(config: dict, config_path: Path):
    """Remove only the exact old generic Cortex provider, with a backup."""
    agents = config.get("agents")
    providers = agents.get("providers") if isinstance(agents, dict) else None
    legacy = providers.get("cortex") if isinstance(providers, dict) else None
    if not _is_owned_legacy_paseo_provider(legacy):
        return {"migrated": False, "reason": "no_owned_legacy_provider"}
    backup = config_path.with_name(f"{config_path.name}.cortex-legacy-backup-{time.time_ns()}")
    try:
        backup.write_bytes(config_path.read_bytes())
        os.chmod(backup, 0o600)
        updated = json.loads(json.dumps(config))
        del updated["agents"]["providers"]["cortex"]
        _atomic_private_json(config_path, json.dumps(updated, indent=2, sort_keys=True) + "\n")
    except (OSError, ValueError) as error:
        try:
            backup.unlink()
        except OSError:
            pass
        raise SystemExit(f"error: cannot migrate the legacy Paseo Cortex provider safely: {error}") from None
    return {"migrated": True, "removed_provider": "cortex", "backup": str(backup)}


def install_paseo_plugin(*, plugin_dir=None, paseo_cli=None, environ=None, runner=None):
    """Install/repair the repository-owned Paseo provider during explicit setup."""
    root, manifest, payload_sha256 = _verify_paseo_plugin_source(plugin_dir)
    env = dict(os.environ if environ is None else environ)
    cli = paseo_cli or _resolve_paseo_cli(env)
    run = runner or subprocess.run

    def invoke(args, check=True):
        result = run([cli, *args], env=env, text=True, capture_output=True, check=False)
        if check and result.returncode != 0:
            detail = (result.stderr or result.stdout or "").strip()
            raise SystemExit(f"error: Paseo command failed ({' '.join(args)}): {detail}")
        return result

    status = invoke(["daemon", "status", "--json"])
    try:
        daemon = json.loads(status.stdout)
    except (TypeError, ValueError):
        raise SystemExit("error: Paseo daemon status was not valid JSON.") from None
    if not isinstance(daemon, dict) or daemon.get("localDaemon") not in {"running", "reachable"}:
        raise SystemExit("error: local Paseo daemon is not reachable.")
    home = Path(str(daemon.get("home", ""))).resolve()
    config_path = home / "config.json"
    if config_path.is_symlink() or not config_path.is_file():
        raise SystemExit(f"error: refusing symlinked or non-regular Paseo daemon config: {config_path}")
    try:
        config_info = config_path.lstat()
        if config_info.st_uid != os.geteuid():
            raise SystemExit(f"error: Paseo daemon config is not owned by the current user: {config_path}")
        mode = stat.S_IMODE(config_info.st_mode)
        if not mode & 0o400:
            raise SystemExit(f"error: Paseo daemon config is not readable by its owner: {config_path}")
        if mode & 0o077:
            os.chmod(config_path, mode & 0o700)
            mode = stat.S_IMODE(config_path.lstat().st_mode)
            if mode & 0o077:
                raise SystemExit(f"error: refusing overly-readable Paseo daemon config: {config_path}")
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except SystemExit:
        raise
    except (OSError, ValueError) as error:
        raise SystemExit(f"error: cannot read Paseo daemon config: {error}") from None
    if not isinstance(config, dict):
        raise SystemExit("error: Paseo daemon config is not a JSON object.")
    if config.get("pluginsEnabled") is not True:
        updated = dict(config); updated["pluginsEnabled"] = True
        _atomic_private_json(config_path, json.dumps(updated, indent=2, sort_keys=True) + "\n")
        invoke(["reload", "--json"])
    # The plugin is evaluated as part of `plugin install`.  Persist the
    # daemon-safe launch command before that evaluation: the provider's
    # bundled server module cannot rely on import.meta.url after Paseo's
    # esbuild CommonJS compilation, and therefore needs this record on its
    # first load (before a successful install can be verified).
    launch_config = _write_paseo_launch_config(env)
    try:
        listed = invoke(["plugin", "ls", "--json"])
        try:
            plugins = json.loads(listed.stdout)
        except (TypeError, ValueError):
            raise SystemExit("error: Paseo plugin list was not valid JSON.") from None
        plugins = plugins if isinstance(plugins, list) else []
        existing = next((p for p in plugins if isinstance(p, dict) and p.get("id") == manifest["id"]), None)
        if existing is None:
            invoke(["plugin", "install", str(root)])
        else:
            if Path(str(existing.get("path", ""))).resolve() != root:
                raise SystemExit("error: Paseo plugin id 'cortex-acp' is bound to another source path.")
            if existing.get("status") != "running":
                invoke(["plugin", "enable", manifest["id"]], check=False)
        verified = invoke(["plugin", "ls", manifest["id"], "--json"])
        try:
            rows = json.loads(verified.stdout)
        except (TypeError, ValueError):
            raise SystemExit("error: Paseo plugin verification was not valid JSON.") from None
        row = rows[0] if isinstance(rows, list) and rows else rows if isinstance(rows, dict) else None
        if not isinstance(row, dict) or row.get("id") != manifest["id"] or row.get("status") != "running":
            raise SystemExit("error: Cortex Paseo plugin did not reach running status.")
        # The native plugin registers provider identity ``cortex`` for new
        # chats. Remove only our exact owned generic registration after the
        # plugin is verified; old generic chats may become inaccessible. Keep
        # an owner-only backup for rollback/audit.
        migration = _migrate_legacy_paseo_provider(config, config_path)
        # The daemon keeps provider registrations in memory.  Reload after
        # removing the old direct ACP alias so existing Paseo sessions resolve
        # through the native provider without a duplicate entry.
        if migration.get("migrated"):
            invoke(["reload", "--json"])
        return {"plugin_id": manifest["id"], "source": str(root), "status": "running", "payload_sha256": payload_sha256,
                "launch_config": launch_config, "legacy_provider_migration": migration}
    except BaseException:
        _remove_new_paseo_launch_config(launch_config)
        raise


def init_storage(environ=None):
    """Create/verify the owner-private config/state roots; idempotent."""
    config_root = registry.config_root(environ)
    state_root = registry.state_root(environ)
    registry.ensure_private_dir(config_root, root=config_root, environ=environ)
    registry.ensure_private_dir(state_root, root=state_root, environ=environ)
    return dict(config_root=str(config_root), state_root=str(state_root))


def add_connection(*, name, engine, provider, auth_mode, key_kind=None, api_key_reader=None,
                    launch_command=None, reuse=True, environ=None):
    """Register (or reuse) one connection; return a JSON-safe summary.

    ``api_key_reader`` (a zero-argument callable returning ``bytes``) is
    required iff ``auth_mode == AuthMode.API_KEY`` -- the same
    reader-callback convention ``onboarding.set_api_key`` already
    enforces, so a secret is never accepted as a plain string here either.

    ``launch_command`` (an optional argv tuple) is only ever a caller-
    supplied, already-pinned/verified path -- this script never derives
    one from a live probe. A production caller would only ever pass one
    verified via ``engine.codex_install.verify_install``/
    ``engine.claude_install.verify_install`` against an operator-recorded
    install (see ``pinned-dependencies.json``'s own ``install_record_note``
    fields); wiring that verification step into this CLI is left for a
    later change, not silently skipped -- see ``docs/acp/packaging-and-onboarding.md``.
    """
    record = onboarding.create_connection(
        name=name, engine=engine, provider=provider, auth_mode=auth_mode,
        key_kind=key_kind, launch_command=launch_command, reuse=reuse, environ=environ,
    )
    login_instructions = None
    if auth_mode == AuthMode.API_KEY:
        if api_key_reader is None:
            raise SystemExit("error: --api-key-file or --api-key-stdin is required for auth-mode=api_key.")
        record = onboarding.set_api_key(record.id, reader=api_key_reader, environ=environ)
    elif auth_mode == AuthMode.NATIVE_ACCOUNT:
        login_instructions = onboarding.mark_native_login_pending(record.id, environ=environ)
    return dict(
        connection_id=record.id, name=record.name, engine=record.engine, provider=record.provider,
        auth_mode=record.auth_mode.value, state=record.state.value, qualified=record.qualified,
        login_instructions=login_instructions,
    )


def _verified_install_launch_command(*, engine, install_record, environ=None):
    """Load and verify one operator-pinned engine install before onboarding.

    The install record is deliberately a file input rather than an argv JSON
    value: it contains absolute paths and integrity digests, but never a
    credential.  Verification is completed before ``create_connection`` is
    called, so a bad or drifted record cannot leave a half-created connection.
    """
    if engine == "codex-acp":
        from cortex_runtime.engine import codex_install

        install_type = codex_install.CodexAcpInstall
    elif engine == "claude-agent-acp":
        from cortex_runtime.engine import claude_install

        install_type = claude_install.ClaudeAcpInstall
    else:  # argparse normally prevents this; keep the helper closed for API callers.
        raise SystemExit(f"error: unsupported engine {engine!r}.")

    try:
        raw = json.loads(Path(install_record).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SystemExit(f"error: could not read --install-record: {error}") from None
    if not isinstance(raw, dict):
        raise SystemExit("error: --install-record must contain one JSON object.")

    expected = {field.name for field in fields(install_type)}
    actual = set(raw)
    missing = sorted(expected - actual)
    unknown = sorted(actual - expected)
    if missing or unknown:
        details = []
        if missing:
            details.append("missing fields: " + ", ".join(missing))
        if unknown:
            details.append("unknown fields: " + ", ".join(unknown))
        raise SystemExit("error: invalid --install-record (" + "; ".join(details) + ").")
    try:
        install = install_type(**raw)
        verified = (
            codex_install.verify_install(install, environ)
            if engine == "codex-acp"
            else claude_install.verify_install(install, environ)
        )
        return (
            codex_install.launch_command_for(verified)
            if engine == "codex-acp"
            else claude_install.launch_command_for(verified)
        )
    except PortableError as error:
        raise SystemExit(f"error: --install-record is not trusted: {error.code}: {error.remediation}") from None
    except (TypeError, ValueError) as error:
        raise SystemExit(f"error: invalid --install-record: {error}") from None


def pin_route(*, connection_id, model, environ=None):
    """Declare one qualified model for ``connection_id`` and pin it as the
    persisted user-default route (see module docstring, step 3)."""
    state = registry.read_registry(environ)
    record = state.connections.get(connection_id)
    if record is None:
        raise SystemExit(f"error: unknown connection id {connection_id!r}; run add-connection first.")
    try:
        # T035 FINDING-07: only this one connection, never the full
        # registry -- passing every connection here (as an earlier revision
        # did) makes build_catalog mark every *other* connection
        # "discovery_unavailable" (it never observed them at all this
        # call) and save_snapshot then writes that entry-less placeholder
        # into each of their own catalog directories, newer than their real
        # cached snapshot, hiding them from selectable_entries_from_snapshots
        # (see acp.config_options).
        snapshot = discovery.build_catalog(
            {connection_id: record}, observations={},
            manual_declarations=(discovery.ManualDeclaration(connection_id=connection_id, model=model),),
        )
    except PortableError as error:
        raise SystemExit(f"error: {error.code}: {error.remediation}") from None
    catalog_store.save_snapshot(snapshot, environ)
    entry = next((e for e in snapshot.entries if e.connection_id == connection_id and e.route.model == model), None)
    if entry is None:
        raise SystemExit("error: the declared model was not accepted (connection auth_mode is not a "
                          "documented/qualified combination -- see connections.model.SUPPORT_MATRIX).")
    current = configuration_store.read_user_defaults(environ)
    defaults = dict(current["defaults"])
    defaults["route_id"] = entry.route.id
    # T035 FINDING-05: the stable identity alongside the fingerprint id --
    # see cortex_runtime.configuration.resolution.resolve_saved_route.
    defaults["route_connection_id"] = connection_id
    defaults["route_model"] = model
    defaults["parameters"] = {}
    configuration_store.write_user_defaults(defaults, current["ceiling"], environ)
    return dict(route_id=entry.route.id, connection_id=connection_id, model=model,
                catalog_snapshot_id=snapshot.id, label=entry.disambiguated_label)


def record_observation(*, connection_id, model, config_options_file, environ=None):
    """Persist a real, engine-observed route for ``connection_id``/``model``
    from a captured ``session/new``-shaped ``configOptions`` JSON array (see
    module docstring, step 4), and re-pin it as the user-default route --
    the same mechanism ``pin_route`` uses.

    Unlike ``pin_route``'s ``ManualDeclaration`` (always persisted with
    ``supported_parameters=()`` since nothing confirmed what it actually
    supports), this goes through ``catalog.discovery.parse_observation`` +
    ``build_catalog`` -- the same DISCOVERY-provenance path automatic
    discovery would use -- so the saved route's
    ``ExecutionRoute.supported_parameters`` are the engine's real advertised
    values (e.g. a ``thought_level``/``reasoning_effort`` choice), never a
    fabricated or declared one.
    """
    state = registry.read_registry(environ)
    record = state.connections.get(connection_id)
    if record is None:
        raise SystemExit(f"error: unknown connection id {connection_id!r}; run add-connection first.")
    try:
        raw = json.loads(Path(config_options_file).read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise SystemExit(f"error: could not read --config-options-file: {error}") from None
    try:
        observation = discovery.parse_observation(raw)
    except PortableError as error:
        raise SystemExit(f"error: {error.code}: {error.remediation}") from None
    if model not in {value for value, _name in observation.model_options()}:
        raise SystemExit(
            f"error: {model!r} is not among the observed 'model' option's values; "
            "the engine never actually advertised it."
        )
    try:
        snapshot = discovery.build_catalog({connection_id: record}, observations={connection_id: observation})
    except PortableError as error:
        raise SystemExit(f"error: {error.code}: {error.remediation}") from None
    catalog_store.save_snapshot(snapshot, environ)
    entry = next((e for e in snapshot.entries if e.connection_id == connection_id and e.route.model == model), None)
    if entry is None:
        raise SystemExit("error: the observed model was not accepted (see catalog.discovery.build_catalog).")
    current = configuration_store.read_user_defaults(environ)
    defaults = dict(current["defaults"])
    defaults["route_id"] = entry.route.id
    defaults["route_connection_id"] = connection_id
    defaults["route_model"] = model
    defaults["parameters"] = {}
    configuration_store.write_user_defaults(defaults, current["ceiling"], environ)
    return dict(
        route_id=entry.route.id, connection_id=connection_id, model=model,
        catalog_snapshot_id=snapshot.id, label=entry.disambiguated_label,
        supported_parameters=[list(pair) for pair in entry.route.supported_parameters],
    )


def refresh_catalog(*, connection_id, workspace_root=None, timeout=None, environ=None):
    """Step 5: a prompt-free, on-demand catalog refresh for
    ``connection_id`` -- see module docstring and
    ``cortex_runtime.catalog.refresh``. A real Cortex session already
    refreshes the catalog for free on every engine open (see
    ``catalog.refresh``'s own module docstring); this step is only for an
    explicit manual refresh (e.g. a connection with no catalog yet)."""
    state = registry.read_registry(environ)
    record = state.connections.get(connection_id)
    if record is None:
        raise SystemExit(f"error: unknown connection id {connection_id!r}; run add-connection first.")
    kwargs = dict(environ=environ)
    if timeout is not None:
        kwargs["timeout"] = timeout
    outcome = catalog_refresh.perform_refresh(record, workspace_root or os.getcwd(), **kwargs)
    return dict(
        connection_id=outcome.connection_id, attempted=outcome.attempted, success=outcome.success,
        reason=outcome.reason, catalog_snapshot_id=outcome.snapshot_id, at=outcome.at,
        effort_enrichment=outcome.effort_enrichment,
    )


def init_route_policy(environ=None):
    """D-T035-03: write a default, hand-editable ``route-policy.json`` at
    ``~/.config/cortex/route-policy.json`` (see
    ``cortex_runtime.configuration.route_policy`` for the exact schema and
    read precedence) -- pre-populated permitting every role for both
    engines this project supports, at a sensible effort range, so the file
    always exists in a working (unrestrictive) state the user can then
    narrow by hand. Never overwrites an existing file, at this path or any
    other location ``route_policy.load_route_policy`` would find first;
    idempotent -- rerunning after a file already exists changes nothing and
    reports ``created=False``.
    """
    existing_policy, malformed = route_policy_module.load_route_policy(project_root=None, environ=environ)
    # The same XDG rules every other Cortex config obeys (see
    # connections.registry): a user who moved their configuration with
    # XDG_CONFIG_HOME would otherwise get this one file written somewhere
    # their own setup does not look.
    env = os.environ if environ is None else environ
    config_home = env.get("XDG_CONFIG_HOME") or ""
    base = Path(config_home) if config_home.startswith("/") else Path(env.get("HOME") or Path.home()) / ".config"
    path = base / "cortex" / "route-policy.json"
    if path.is_file():
        return dict(path=str(path), created=False, reason="already_exists")
    if malformed is not None or existing_policy is not None:
        # A different location in the read-precedence chain already has a
        # policy (or a malformed one) in effect; writing a second file here
        # would never actually be read (see load_route_policy's own
        # first-match-wins docstring) and could confuse the user into
        # thinking it applies. Report where the existing one actually is
        # instead of writing a dead file.
        return dict(path=str(path), created=False, reason="policy_already_in_effect_elsewhere", detail=malformed)
    # The file the user actually maintains, shipped verbatim rather than
    # rebuilt from code. It is a document with their own judgement in it --
    # which effort levels each model is worth using at -- and its exact
    # shape and formatting are theirs. Regenerating it from a dict would
    # reformat it on every change and quietly lose anything expressed only
    # in its layout.
    template = _ROOT / "policy" / "route-policy.default.json"
    try:
        body = template.read_text(encoding="utf-8")
    except OSError as error:
        return dict(path=str(path), created=False, reason="template_unavailable", detail=str(error))
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    tmp_path = path.with_suffix(".json.tmp")
    tmp_path.write_text(body, encoding="utf-8")
    os.chmod(tmp_path, 0o600)
    os.replace(tmp_path, path)
    return dict(path=str(path), created=True)


def status(environ=None):
    """Read-only report of current setup state; always safe to run."""
    result = dict(storage=None, connections=[], pinned_route=None, discovered_engines=None)
    try:
        found = connection_discovery.discover_all(environ=environ)
        # D-T035-02: a discovered engine shares its native credential
        # directory with the client's own engine process, so a token
        # refresh can race between the two -- an accepted cost, not a
        # hidden one; "referenced" (never "owned"/copied) names that here.
        result["discovered_engines"] = {
            engine: dict(
                version=info.version,
                launch_command=list(info.launch_command),
                native_credential_dir=info.native_credential_dir,
                credential_dir_kind="referenced",
            )
            for engine, info in found.items()
        }
        for engine in connection_discovery.SUPPORTED_ENGINES:
            result["discovered_engines"].setdefault(engine, None)
    except Exception as error:  # noqa: BLE001 - status is always safe to run
        result["discovery_error"] = str(error)
    try:
        result["storage"] = dict(
            config_root=str(registry.config_root(environ)), state_root=str(registry.state_root(environ)),
        )
    except PortableError as error:
        result["storage_error"] = error.code
    try:
        state = registry.read_registry(environ)
        def _credential_kind(record):
            ref = record.credential_ref
            if isinstance(ref, EngineNativeDir):
                return "owned" if ref.owned else "referenced"
            return ref.kind

        result["connections"] = [
            dict(id=cid, name=r.name, engine=r.engine, state=r.state.value, qualified=r.qualified,
                 credential_kind=_credential_kind(r))
            for cid, r in sorted(state.connections.items())
        ]
    except PortableError as error:
        result["connections_error"] = error.code
    try:
        defaults = configuration_store.read_user_defaults(environ)["defaults"]
        route_id = defaults.get("route_id") if isinstance(defaults, dict) else None
        result["pinned_route"] = route_id
    except PortableError as error:
        result["pinned_route_error"] = error.code
    return result


def validate_connection(*, connection_id, environ=None):
    """Recompute one connection's state and return only safe public fields."""
    try:
        record = onboarding.validate(connection_id, environ=environ)
    except PortableError as error:
        raise SystemExit(f"error: {error.code}: {error.remediation}") from None
    return dict(
        connection_id=record.id,
        engine=record.engine,
        state=record.state.value,
        qualified=record.qualified,
    )


def _read_secret(args):
    if args.api_key_file:
        return lambda: Path(args.api_key_file).read_bytes()
    if args.api_key_stdin:
        return lambda: sys.stdin.buffer.read()
    return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="step")

    sub.add_parser("status", help="Report current setup state (read-only, default).")
    sub.add_parser("init-storage", help="Create/verify owner-private config/state roots.")
    sub.add_parser("init-route-policy", help="Write a default, hand-editable ~/.config/cortex/route-policy.json (never overwrites an existing one).")
    sub.add_parser("install-paseo-plugin", help="Install and verify the Cortex provider plugin in Paseo (explicit setup only).")

    p_validate = sub.add_parser(
        "validate-connection", help="Recompute one connection's authorization state (no credentials shown).",
    )
    p_validate.add_argument("--connection", required=True, dest="connection_id")

    p_conn = sub.add_parser("add-connection", help="Register (or reuse) one authorized connection.")
    p_conn.add_argument("--name", required=True)
    p_conn.add_argument("--engine", required=True, choices=("codex-acp", "claude-agent-acp"))
    p_conn.add_argument("--provider", required=True)
    p_conn.add_argument("--auth-mode", required=True, choices=("native_account", "api_key"))
    p_conn.add_argument("--key-kind", default=None)
    p_conn.add_argument("--api-key-file", type=str, default=None,
                         help="Path to a file containing the API key bytes (never inline on argv).")
    p_conn.add_argument("--api-key-stdin", action="store_true",
                         help="Read the API key bytes from stdin instead of a file.")
    p_conn.add_argument(
        "--install-record", required=True, type=Path,
        help=("Path to the exact JSON install record for the selected engine. "
              "It is verified before the connection is written; credentials never belong in this file."),
    )
    p_conn.add_argument("--no-reuse", dest="reuse", action="store_false", default=True)

    p_route = sub.add_parser("pin-route", help="Pin one declared model as the default route.")
    p_route.add_argument("--connection", required=True, dest="connection_id")
    p_route.add_argument("--model", required=True)

    p_obs = sub.add_parser(
        "record-observation",
        help="Replace a pin-route'd route with one built from a real captured engine configOptions payload.",
    )
    p_obs.add_argument("--connection", required=True, dest="connection_id")
    p_obs.add_argument("--model", required=True)
    p_obs.add_argument("--config-options-file", required=True, type=Path,
                        help="Path to a JSON file holding the engine's real session/new 'configOptions' array.")

    p_refresh = sub.add_parser(
        "refresh-catalog",
        help="Prompt-free, on-demand catalog refresh for one connection.",
    )
    p_refresh.add_argument("--connection", required=True, dest="connection_id")
    p_refresh.add_argument("--workspace", type=str, default=None,
                            help="Working directory for the refresh session (defaults to the current directory).")
    p_refresh.add_argument("--timeout", type=float, default=None, help="Engine launch/call timeout (default 30s).")

    p_recipe = sub.add_parser("print-client-recipes", help="Print Zed/codeg registration recipes (delegates to cortex_acp_client_config.py).")
    p_recipe.add_argument("--settings", type=Path, default=None)
    p_recipe.add_argument("--apply", action="store_true")
    p_recipe.add_argument("--i-consent-to-write", dest="consent", action="store_true")
    p_recipe.add_argument("--replace", action="store_true")

    args = parser.parse_args(argv)
    step = args.step or "status"

    if step == "status":
        print(_json(status()))
        return 0
    if step == "init-storage":
        print(_json(init_storage()))
        return 0
    if step == "init-route-policy":
        print(_json(init_route_policy()))
        return 0
    if step == "install-paseo-plugin":
        print(_json(install_paseo_plugin()))
        return 0
    if step == "validate-connection":
        print(_json(validate_connection(connection_id=args.connection_id)))
        return 0
    if step == "add-connection":
        reader = _read_secret(args)
        launch_command = _verified_install_launch_command(
            engine=args.engine, install_record=args.install_record,
        )
        result = add_connection(
            name=args.name, engine=args.engine, provider=args.provider,
            auth_mode=AuthMode(args.auth_mode), key_kind=args.key_kind, api_key_reader=reader,
            launch_command=launch_command, reuse=args.reuse,
        )
        print(_json(result))
        return 0
    if step == "pin-route":
        result = pin_route(connection_id=args.connection_id, model=args.model)
        print(_json(result))
        return 0
    if step == "record-observation":
        result = record_observation(
            connection_id=args.connection_id, model=args.model,
            config_options_file=args.config_options_file,
        )
        print(_json(result))
        return 0
    if step == "refresh-catalog":
        result = refresh_catalog(
            connection_id=args.connection_id, workspace_root=args.workspace, timeout=args.timeout,
        )
        print(_json(result))
        return 0
    if step == "print-client-recipes":
        recipe_argv = []
        if args.settings is not None:
            recipe_argv += ["--settings", str(args.settings)]
        if args.apply:
            recipe_argv.append("--apply")
        if args.consent:
            recipe_argv.append("--i-consent-to-write")
        if args.replace:
            recipe_argv.append("--replace")
        exit_code = cortex_acp_client_config.main(recipe_argv)
        print()
        cortex_acp_client_config.cmd_print_codeg_recipe()
        return exit_code

    parser.print_help()
    return 2


def _json(data):
    import json as _json_module

    return _json_module.dumps(data, indent=2, sort_keys=True)


if __name__ == "__main__":
    raise SystemExit(main())
