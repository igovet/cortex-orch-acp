"""Effective configuration policy: layered defaults, ceilings, and revisions.

Sub-modules:

- ``policy``: :class:`PolicyCeiling`/:class:`FallbackPolicy` value types and
  narrowing/intersection rules (T008).
- ``resolution``: per-field most-specific-wins layer merge, route-change
  re-derivation, and dependent-settings bundle validation.
- ``store``: ``configuration_revisions`` (task-scoped, SQLite) and
  ``project_configuration_defaults`` (project-scoped, SQLite) persistence.
- ``service``: :class:`ConfigurationService`, the one API both the UI and
  command entry points call through thin adapters (``ui_select``/
  ``command_set``) for parity.
"""
from __future__ import annotations
