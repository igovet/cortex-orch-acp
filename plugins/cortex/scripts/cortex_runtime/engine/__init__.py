"""Outgoing ACP engine runtime: process/session lifecycle, JSON-RPC client,
correlation tables, and the MCP bridge (T012).

``EngineSessionManager`` is the one production entry point: it implements
``portable.engine_contract.EngineRuntime`` (structural typing) over a
per-engine-instance :class:`~.process.EngineProcess`,
:class:`~.client.EngineClient`, and :class:`~.correlation.CorrelationTables`
-- see each module's docstring for why none of those three is ever shared
across engine instances.
"""
from __future__ import annotations

from .session_manager import EngineSessionManager

__all__ = ["EngineSessionManager"]
