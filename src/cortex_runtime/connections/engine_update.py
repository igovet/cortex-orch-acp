"""Keep Cortex's own engine installs current (see ``discovery.owned_root``).

Runs in the background at startup: asks npm for each engine's latest
version and, iff it is newer than the newest owned install, installs it
beside the current one (``<root>/<engine>/<version>``). Exact qualified SDK
refreshes use ``<version>+sdk.<sdk-version>`` without replacing a running install. A new version is
used from the next session on, never swapped under a running one; one
previous version is kept to fall back to. No network, no npm, a failed
install or a concurrent update elsewhere: nothing changes and the current
version stays in use. Never raises.
"""
from __future__ import annotations

import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading

from cortex_runtime.connections import discovery

_TIMEOUT_VIEW = 20
_TIMEOUT_INSTALL = 600
_KEEP = 2  # the current version and one to fall back to
# 0.87.0 pins SDK 0.3.287 upstream, which predates Haiku 5.5. This exact
# refreshed dependency was inspected independently; never track SDK latest.
_CLAUDE_SDK_OVERRIDES = {"0.87.0": "0.3.293"}
_CLAUDE_SDK_PACKAGE = "@anthropic-ai/claude-agent-sdk"


def _npm(environ: dict[str, str] | None) -> str | None:
    node = discovery._node_binary(environ)
    beside = Path(node).parent / "npm" if node else None
    if beside is not None and beside.is_file():
        return str(beside)
    env = os.environ if environ is None else environ
    return shutil.which("npm", path=env.get("PATH"))


def _run(args: list[str], timeout: int, environ: dict[str, str] | None) -> str:
    env = dict(os.environ if environ is None else environ)
    npm_dir = str(Path(args[0]).parent)
    env["PATH"] = npm_dir + os.pathsep + env.get("PATH", "")  # npm's own node first
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout, env=env,
                          stdin=subprocess.DEVNULL, check=True).stdout


def update_engine(engine: str, *, environ: dict[str, str] | None = None) -> str | None:
    """Install the latest adapter/qualified dependency build; return the adapter version."""
    package = discovery._PACKAGE_NAME[engine]
    npm = _npm(environ)
    if npm is None:
        return None
    root = discovery.owned_root(environ) / engine
    root.mkdir(parents=True, exist_ok=True)
    with open(root / ".lock", "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return None  # another Cortex process is already updating
        for stale in root.glob(".tmp-*"):
            shutil.rmtree(stale, ignore_errors=True)
        latest = json.loads(_run([npm, "view", package, "version", "--json"], _TIMEOUT_VIEW, environ))
        if not isinstance(latest, str) or not discovery.version_key(latest):
            return None
        sdk_version = _CLAUDE_SDK_OVERRIDES.get(latest) if engine == "claude-agent-acp" else None
        install_name = f"{latest}+sdk.{sdk_version}" if sdk_version else latest
        installed = sorted((p.name for p in discovery._subdirs(root)), key=discovery.version_key)
        if installed and discovery.version_key(install_name) <= discovery.version_key(installed[-1]):
            return None
        staging = root / f".tmp-{install_name}"
        if sdk_version:
            staging.mkdir()
            (staging / "package.json").write_text(json.dumps({
                "private": True, "overrides": {_CLAUDE_SDK_PACKAGE: sdk_version},
            }), encoding="utf-8")
        _run([npm, "install", "--prefix", str(staging), "--no-audit", "--no-fund", "--omit=dev",
              "--no-save", f"{package}@{latest}"], _TIMEOUT_INSTALL, environ)
        declared = discovery._declared_bin(staging / "node_modules" / package / "package.json")
        if declared is None or declared[0] != latest:
            shutil.rmtree(staging, ignore_errors=True)
            return None
        if sdk_version:
            sdk_manifest = staging / "node_modules" / _CLAUDE_SDK_PACKAGE / "package.json"
            try:
                sdk_matches = json.loads(sdk_manifest.read_text(encoding="utf-8")).get("version") == sdk_version
            except (OSError, ValueError, AttributeError):
                sdk_matches = False
            if not sdk_matches:
                shutil.rmtree(staging, ignore_errors=True)
                return None
        staging.rename(root / install_name)
        # A pinned version is never pruned. codex-acp launches only from
        # a byte-verified, qualified bundle (see ``discovery.is_pinned``),
        # so deleting it to keep the two newest -- both possibly unqualified
        # -- would leave the engine permanently unlaunchable rather than
        # merely not-yet-current. Claude exact instruction-delivery registrations
        # (including side-by-side SDK builds) are protected for the same reason.
        keep = {path.name for path in sorted(discovery._subdirs(root), key=lambda p: discovery.version_key(p.name))[-_KEEP:]}
        for old in discovery._subdirs(root):
            if old.name in keep or discovery.is_pinned(engine, old.name):
                continue
            shutil.rmtree(old, ignore_errors=True)
        if discovery.is_qualified(engine, latest):
            from ..configuration.management import follow_approved_adapters

            follow_approved_adapters(environ=environ)
        return latest


def update_all(*, environ: dict[str, str] | None = None) -> dict[str, str]:
    updated = {}
    for engine in discovery.SUPPORTED_ENGINES:
        try:
            version = update_engine(engine, environ=environ)
        except Exception:  # noqa: BLE001 - offline, npm failure: keep the current version
            version = None
        if version:
            updated[engine] = version
    return updated


# CORTEX_ENGINE_UPDATE=off skips the startup update: an offline machine, and
# disposable runs (the no-paid smoke harness, the test suite) whose fake HOME
# would otherwise get a full npm download (~625 MB) on every launch, written
# by an npm that outlives the agent.
UPDATE_SWITCH = "CORTEX_ENGINE_UPDATE"


def start_background_update(*, environ: dict[str, str] | None = None) -> threading.Thread | None:
    env = os.environ if environ is None else environ
    if env.get(UPDATE_SWITCH, "").strip().lower() in {"off", "0", "false", "no"}:
        return None
    thread = threading.Thread(target=update_all, kwargs={"environ": environ},
                              name="cortex-engine-update", daemon=True)
    thread.start()
    return thread
