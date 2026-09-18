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
``docs/acp/orchestration.md``.
"""
import sys

from cortex_runtime.acp.agent import main
from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
from cortex_runtime.packaging.pinned import PinnedManifestError, load_manifest, verify_launch_critical


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
    their own account, which is never done silently. The route policy is
    not that: it is a file in their own configuration directory,
    permitting what is already permitted.

    Never fails startup: a user whose configuration directory cannot be
    written still gets a working agent, just without a policy file.
    """
    try:
        from cortex_runtime.configuration.route_policy import ensure_default_policy

        ensure_default_policy()
    except Exception:  # noqa: BLE001 - startup is never blocked by this
        pass

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


if __name__ == '__main__':
    _fail_closed_on_pinned_mismatch()
    _ensure_first_run_configuration()
    main(runtime=ProductionOrchestratingRuntime())
