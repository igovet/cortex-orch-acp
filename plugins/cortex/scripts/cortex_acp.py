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
from cortex_runtime.acp.agent import main
from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime

if __name__ == '__main__':
    main(runtime=ProductionOrchestratingRuntime())
