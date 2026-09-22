"""Static, per-engine seed model catalogs (T035).

Ships a usable ``model`` select before any live session has ever been
observed for a connection, so a first-time user is never shown an empty
selector -- but a seed value is never claimed as engine-confirmed: it is
promoted with :class:`catalog.evidence.ProvenanceSource.SEED`, rendered as
"seeded -- not yet confirmed by an engine" (see
``acp.config_options._evidence_note``), and unconditionally superseded the
moment a real observation is captured for that connection (see
``catalog.discovery.selectable_entries_from_snapshots``, which only ever
falls back to this seed when no real cached snapshot file exists yet for
the connection).

Sourced from this development machine's own already-installed vendor
tooling, not invented/recalled from training data:

- ``codex-acp``: ``codex debug models`` (the pinned Codex CLI's own
  documented "Render the raw model catalog as JSON" read-only, offline,
  free subcommand -- see ``codex debug --help``), filtered to
  ``visibility == "list"``. This is the authoritative catalog the codex-acp
  adapter itself is built on, not a guess.
- ``claude-agent-acp``: no equivalent catalog subcommand exists on the
  installed ``claude`` CLI (``claude --help`` lists no ``models``
  subcommand). The pinned ``claude-agent-acp`` engine build ships no
  static model list either -- only alias tokens the SDK resolves at
  runtime. Seeded here with just the two aliases directly evidenced on
  this machine: ``sonnet`` (this machine's own
  ``~/.config/zed/settings.json`` ``agent_servers.claude-acp
  .default_config_options.model``) and ``opus`` (a literal alias string,
  and the resolution target of ``opusplan``, in the pinned engine's own
  ``dist/session-model.js``). ``haiku`` was not directly evidenced on this
  machine and is deliberately left out -- a narrower, honestly-sourced set
  rather than a guessed one; add it once actually observed.

Each per-engine tuple is ``((model_id, display_name), ...)``.

Known ceiling (ponytail): codex's own catalog reports a *different*
``supported_reasoning_levels`` (thought_level/effort) set per model (e.g.
``gpt-5.5`` lacks ``max``/``ultra``), but one ``catalog.discovery
.Observation`` today applies one flat ``supported_parameters`` set to
*every* model it lists (see ``build_catalog``'s per-observation loop) --
the same simplification real engine observations already have, not one
introduced here. Seeding a model-keyed effort set would need
``ExecutionRoute``/``Observation`` to carry per-model parameter sets, a
real architecture change out of scope for this task; this module therefore
seeds ``model`` only, never ``thought_level`` -- a seeded model's
thought_level stays unavailable until a live session actually observes it
for that connection. Upgrade path: key ``Observation.parameter_pairs()`` by
model instead of by connection if/when this is needed.
"""
from __future__ import annotations

SEED_MODELS: dict[str, tuple[tuple[str, str], ...]] = {
    "codex-acp": (
        ("gpt-6-astra", "GPT-6-Astra"),
        ("gpt-6-sol", "GPT-6-Sol"),
        ("gpt-6-luna", "GPT-6-Luna"),
        ("gpt-5.6-sol", "GPT-5.6-Sol"),
        ("gpt-5.6-terra", "GPT-5.6-Terra"),
        ("gpt-5.6-luna", "GPT-5.6-Luna"),
        ("gpt-5.5", "GPT-5.5"),
    ),
    "claude-agent-acp": (
        ("sonnet", "sonnet"),
        ("opus", "opus"),
    ),
}
