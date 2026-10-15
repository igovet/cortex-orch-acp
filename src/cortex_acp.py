#!/usr/bin/env python3
"""Cortex incoming ACP v1 stdio entry point.

Constructs :class:`cortex_runtime.acp.production_runtime.ProductionOrchestratingRuntime`
(T016): every seam it needs (per-project store, T008
``ConfigurationService``, the existing connections registry) already
constructs from existing state with no new user configuration, so
production always gets a real, model-owned coordination runtime rather
than the always-refusing ``UnavailableRuntime``. A task with no
resolvable/qualified route yet still fails the prompt explicitly (an ACP
``-32001`` error naming the concrete reason), never silently and never via
a hidden provider/native fallback -- see
``cortex_runtime.acp.production_runtime`` and
``docs/features/orchestration/index.md``.
"""
import os
import sys

from cortex_runtime.tempdirs import prefer_disk_tmp

# Before anything creates a temporary file: /var/tmp, not a RAM-backed /tmp.
prefer_disk_tmp()

from cortex_runtime.acp.agent import main  # noqa: E402
from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime  # noqa: E402
from cortex_runtime.packaging.pinned import PinnedManifestError, load_manifest, verify_launch_critical  # noqa: E402


def _fail_closed_on_pinned_mismatch():
    """T031: refuse to start (stderr only -- stdout stays protocol-only)
    if this checkout does not satisfy its own pinned Python-floor/contract
    manifest. Never downloads, installs, or upgrades anything; a mismatch
    is always a concrete, named error, never a silent proceed."""
    try:
        manifest = load_manifest()
        findings = verify_launch_critical(manifest)
    except PinnedManifestError as error:
        print(f"cortex_acp: pinned dependency manifest error: {error}", file=sys.stderr)
        raise SystemExit(1) from None
    failed = [finding for finding in findings if finding.status != "ok"]
    if failed:
        for finding in failed:
            print(f"cortex_acp: pinned dependency {finding.component} {finding.status}: {finding.detail} "
                  f"Repair: {finding.repair_action}", file=sys.stderr)
        raise SystemExit(1)


def _ensure_first_run_configuration():
    """Everything a first run needs that does not need the user's consent.

    A person installing this agent configures it in their editor and
    expects it to work; they should have to run a setup step only for the
    one thing that genuinely requires them -- connecting an engine to
    their own account, which is never done silently.

    Never fails startup.
    """
    # D-T035-02: an engine the user already configured in their client (e.g.
    # Zed) needs no separate Cortex setup step -- discover it and, iff
    # Cortex has no connection for it yet, register one that references the
    # user's own native credential directory. Never overwrites or mutates a
    # connection the user already has, and a discovery problem here never
    # blocks startup -- the agent still runs with whatever connections
    # already existed.
    try:
        from cortex_runtime.connections import discovery, onboarding

        onboarding.sync_discovered_connections(discovery.discover_all())
    except Exception:  # noqa: BLE001 - startup is never blocked by this
        pass
    # Cortex's own engine copies: updated in the background, used from the
    # next session on; offline or any npm failure keeps the current version.
    try:
        from cortex_runtime.connections import engine_update

        engine_update.start_background_update()
    except Exception:  # noqa: BLE001 - startup is never blocked by this
        pass


def _print_model_efforts():
    """CORTEX_MODEL_EFFORTS=1: print each model's efforts as JSON and exit
    (read by the Paseo plugin's catalog; see config_options.model_efforts)."""
    import json

    from cortex_runtime.acp import config_options
    from cortex_runtime.connections import registry

    print(json.dumps(config_options.model_efforts(registry.read_registry().connections)))


if __name__ == '__main__':
    if os.environ.get("CORTEX_MODEL_EFFORTS") == "1":
        _print_model_efforts()
        raise SystemExit(0)
    _fail_closed_on_pinned_mismatch()
    _ensure_first_run_configuration()
    main(runtime=ProductionOrchestratingRuntime())
