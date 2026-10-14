"""The truthful base MCP surface for an already-bound Cortex channel.

The public ``TOOLS`` catalogue also contains ``create_task`` because the
bootstrap process advertises it before a channel exists.  A channeled server
cannot perform that operation: task creation is completed by trusted
bootstrap, before the channel is issued.  Keep this filter in one small
dependency-free module so ``tools/list`` and recovery capsules cannot drift.

Why ``redact_values`` stays: it looked unused on every channeled
tool at first reading of ``store._call`` (only its ``create_task`` branch
reads it there). It is not: ``store.Store._transaction`` reads
``args.get("redact_values", [])`` for *any* operation's own steering-source
capture (``self._capture_steering(...)``, gated on a coordinator channel with
a live ``steering_source`` and no pending delivery) -- confirmed live by
``tests/test_storage_v11.py``'s ``write_report`` case, which actually
redacts a steering capture this way. So a live consumer exists across the
channeled surface, not only for the excluded ``create_task``/
``set_governance`` entries; it is kept, not removed.
"""
from __future__ import annotations

from .contracts import TOOLS


def channeled_base_tools() -> tuple[dict, ...]:
    """Return fresh advertised base-tool entries usable on a live channel."""
    return tuple(entry for entry in TOOLS if entry["name"] not in {"create_task", "set_governance"})


def channeled_base_tool_names() -> frozenset[str]:
    """Return the names represented by :func:`channeled_base_tools`."""
    return frozenset(entry["name"] for entry in channeled_base_tools())
