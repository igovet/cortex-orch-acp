"""Per-model reasoning-effort ceiling from codex's own offline catalog dump.

``codex debug models`` (the pinned Codex CLI's own documented, read-only,
offline "Render the raw model catalog as JSON" subcommand -- see ``codex
debug --help``; the same subcommand ``catalog.model_seed`` was itself
sourced from) reports a *different* ``supported_reasoning_levels`` set per
model, e.g. ``gpt-5.5`` genuinely lacks ``xhigh``/``max``/``ultra`` while
``gpt-6-astra`` has them -- a real ceiling one flat
``ExecutionRoute.supported_parameters`` set (see ``catalog.discovery
.Observation.parameter_pairs``) cannot express on its own.

This module only ever spawns ``<codex_binary> debug models`` (never
``session/prompt`` or any other engine call -- this is not an ACP call at
all, no ACP transport/session is ever opened here) and parses its stdout;
every failure (missing binary, non-zero exit, timeout, malformed JSON)
returns ``None``, never raises -- the caller (``catalog.refresh
.perform_refresh``) treats this as a purely optional enrichment on top of
the real, required live-session observation.
"""
from __future__ import annotations

import json
import subprocess

DEFAULT_TIMEOUT_SECONDS = 20.0


def parse_effort_ceiling(raw: bytes | str) -> dict[str, tuple[str, ...]] | None:
    """Parse ``codex debug models``' own JSON stdout into
    ``{model_slug: (effort, ...)}``, restricted to ``visibility == "list"``
    entries (the same filter ``catalog.model_seed``'s own sourcing note
    documents) -- or ``None`` if ``raw`` is not that shape at all."""
    try:
        data = json.loads(raw)
    except (UnicodeDecodeError, ValueError, TypeError):
        return None
    if not isinstance(data, dict):
        return None
    models = data.get("models")
    if not isinstance(models, list):
        return None
    ceiling: dict[str, tuple[str, ...]] = {}
    for entry in models:
        if not isinstance(entry, dict) or entry.get("visibility") != "list":
            continue
        slug = entry.get("slug")
        levels = entry.get("supported_reasoning_levels")
        if not isinstance(slug, str) or not slug or not isinstance(levels, list):
            continue
        efforts = tuple(
            level.get("effort")
            for level in levels
            if isinstance(level, dict) and isinstance(level.get("effort"), str) and level.get("effort")
        )
        if efforts:
            ceiling[slug] = efforts
    return ceiling or None


def fetch_effort_ceiling(
    codex_binary: str, *, timeout: float = DEFAULT_TIMEOUT_SECONDS
) -> dict[str, tuple[str, ...]] | None:
    """Run ``<codex_binary> debug models`` (bounded by ``timeout``, which
    also reaps a hanging process -- ``subprocess.run``'s own timeout
    handling kills the child) and parse its stdout, or ``None`` on any
    failure. Never raises."""
    try:
        result = subprocess.run(
            [codex_binary, "debug", "models"],
            capture_output=True, timeout=timeout, check=False,
        )
    except (OSError, subprocess.TimeoutExpired, ValueError):
        return None
    if result.returncode != 0:
        return None
    return parse_effort_ceiling(result.stdout)
