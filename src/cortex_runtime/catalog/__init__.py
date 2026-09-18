"""Truthful model discovery and execution-route catalog (T007).

Stdlib-only, engine-agnostic value types and pure functions that build a
:class:`~cortex_runtime.catalog.model.CatalogSnapshot` of
:class:`~cortex_runtime.catalog.model.CatalogEntry` values from connection
registry records and discovery observations, never from a provider marketing
model list.  See ``cortex_runtime.portable.identity.ExecutionRoute`` for the
one identity this package reuses rather than recomputes.
"""
from __future__ import annotations
