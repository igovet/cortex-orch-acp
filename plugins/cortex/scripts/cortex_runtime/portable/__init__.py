"""Portable, engine-agnostic identity, state, and engine contracts.

This package defines value types and pure validators shared across engine
integrations.  It has no dependency on any specific engine, host, or the
rest of ``cortex_runtime``; it must remain importable with the standard
library alone and must never reference native provider/engine tokens (see
``tests/test_portable_no_native_tokens.py`` for the enforced denylist).
"""
from __future__ import annotations
