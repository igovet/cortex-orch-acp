"""The truthful base MCP surface for an already-bound Cortex channel.

The public ``TOOLS`` catalogue also contains ``create_task`` because the
bootstrap process advertises it before a channel exists.  A channeled server
cannot perform that operation: task creation is completed by trusted
bootstrap, before the channel is issued.  Keep this filter in one small
dependency-free module so ``tools/list`` and recovery capsules cannot drift.
"""
from __future__ import annotations

from .contracts import TOOLS


def channeled_base_tools() -> tuple[dict, ...]:
    """Return fresh advertised base-tool entries usable on a live channel."""
    return tuple(entry for entry in TOOLS if entry["name"] not in {"create_task", "set_governance"})


def channeled_base_tool_names() -> frozenset[str]:
    """Return the names represented by :func:`channeled_base_tools`."""
    return frozenset(entry["name"] for entry in channeled_base_tools())
