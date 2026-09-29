"""T022: context-generation tracking and adapter compaction-support tiers.

A :class:`~engine.session_manager.EngineSessionManager` engine session
passes through a sequence of "context generations" -- distinct epochs of
whatever context the underlying engine process actually holds. Cortex never
observes the engine's internal context directly; it only ever learns a new
generation began from one of a small, closed set of *causes* (see
``generations_store.CONTEXT_GENERATIONS_SCHEMA``'s ``cause`` column):

- ``session_open`` -- the session's very first generation.
- ``observed_compaction`` -- the adapter reported a verifiable compaction
  signal over ACP (see :func:`tier_for` and :mod:`engine.session_manager`'s
  ``_translate_update``).
- ``observed_reset`` -- reserved for a future adapter signal distinct from
  compaction; no pinned adapter (codex-acp 1.12.0, claude-agent-acp) has
  been shown, by static dist evidence, to emit one -- never fabricated.
- ``session_replacement`` -- a new engine session was opened for an attempt
  that already had one (a retry/resume), whatever the pinned adapter's own
  tier.
- ``controlled_boundary_refresh`` -- reserved for a future, adapter-verified,
  Cortex-issued compaction request distinct from an ordinary session
  replacement. codex-acp 1.12.0 exposes a ``/compact`` slash command over
  the ordinary ``session/prompt`` text channel (static evidence: its
  ``available_commands_update`` advertises ``{"name": "compact", ...}``,
  dist ``index.js:30212``, handled at ``index.js:30269``) but codex-acp is
  already ``automatic`` tier via its own compaction signal (see below), so
  this cause is not currently reached by either pinned adapter -- never
  fabricated as "supported" without a real Cortex-issued-command code path.

Tier registry (:data:`_TIER_REGISTRY`, keyed like
``engine.instruction_delivery_registry._REGISTRY``): static dist evidence
for the two pinned adapters --

- **codex-acp 1.12.0** -- ``automatic``. The bundled mapper
  (``@agentclientprotocol/codex-acp`` dist ``index.js``) translates the
  native ``contextCompaction`` item into a real ``tool_call``/
  ``tool_call_update`` pair: ``createContextCompactionStartUpdate``
  (``index.js:23104-23113``, ``kind:"think"``, ``title:"Compact
  conversation"``, ``_meta:{contextCompaction:{version:1}}``) at
  ``index.js:25093-25094`` (item started) and
  ``createContextCompactionCompleteUpdate`` (``index.js:23114-23122``) at
  ``index.js:25161-25162`` (item completed) -- and never constructs an
  object with ``sessionUpdate:"compaction_update"``/``"compaction_summary_chunk"``
  anywhere (those literals exist only in the bundled SDK's unstable zod
  schema, ``index.js:19381-19389``, unused by codex-acp's own code). The
  ``toolCallId`` is the durable evidence id.
- **claude-agent-acp** (Zed dist 0.78.0 / global npm 0.77.0) -- ``automatic``.
  ``dist/context-compaction.js``'s ``ContextCompactionLifecycle`` emits the
  *same* legacy presentation by default (``kind:"think"``, ``title:"Compact
  conversation"``, ``_meta`` seeded by ``createContextCompactionMeta`` from
  ``dist/context-compaction-meta.js`` -- identical ``CONTEXT_COMPACTION_META_KEY
  = "contextCompaction"``/``version = 1`` shape to codex-acp's) unless Cortex
  advertises the ``session.compaction`` client capability at ``initialize``
  (``clientSupportsCompactionUpdates``, ``context-compaction.js``), in which
  case it emits the standardized ``compaction_update``/
  ``compaction_summary_chunk`` session updates with an explicit terminal
  ``status`` instead. Cortex's advertised ``clientCapabilities`` (see
  ``engine.resource_policy.ResourcePolicy.client_capabilities``, T014) never
  includes ``session.compaction`` today, so both pinned adapters are
  observed identically: the ``_meta.contextCompaction`` tool-call pair is
  the one portable signal this module detects (see
  :func:`observe_tool_call_meta`) -- always attempted regardless of engine
  name, so a future adapter using the identical convention is picked up
  without a registry entry, while :func:`tier_for` still names the correct
  documented tier/limitation for the two pinned adapters and ``unqualified``
  for anything else (never a guessed/interpolated tier).

Neither presentation is a *pre*-compaction hook: both arrive once the
engine has already started (``in_progress``) or finished (``completed``)
compacting, never before -- Cortex cannot intervene before a hidden
internal engine action (see ``docs/acp/context-generations.md`` and
INVARIANTS.md). ``automatic`` here means "the adapter emits a verifiable
signal Cortex can react to", not "Cortex is warned in advance".
"""
from __future__ import annotations

from dataclasses import dataclass

from ..portable.identity import new_id

AUTOMATIC = "automatic"
CONTROLLED_BOUNDARY = "controlled_boundary"
UNQUALIFIED = "unqualified"

_CONTROLLED_BOUNDARY_LIMITATION = (
    "No compaction/reset signal is evidenced for this adapter+version over ACP; "
    "Cortex only refreshes context at its own controlled boundaries (a new "
    "session open or an explicit session replacement it itself performs), and "
    "never claims to intervene before, or even learn about, a hidden internal "
    "engine compaction/reset."
)
_UNQUALIFIED_LIMITATION = (
    "Unknown engine/version: no compaction-support evidence and no delivery "
    "mechanism evidence exist for it; Cortex never marks an unevidenced "
    "adapter as supporting any recovery-delivery tier."
)


@dataclass(frozen=True)
class CompactionTier:
    engine: str
    version: str
    tier: str
    limitation: str


# Both pinned adapters emit the identical `_meta.contextCompaction`
# tool_call/tool_call_update convention (see module docstring) -- 'automatic'
# tier, no fabricated pre-compaction hook.
_AUTOMATIC_LIMITATION = (
    "The adapter reports compaction only as it starts/finishes (a "
    "tool_call/tool_call_update pair carrying _meta.contextCompaction, never "
    "a distinct pre-compaction hook) -- Cortex reacts to it (a new context "
    "generation, fresh capsule/role/tool-schema redelivery on the next "
    "continuation) but can never intervene before the engine's own internal "
    "compaction runs."
)

# 0.77.0 (global npm) only ever emits the legacy tool_call presentation
# (`dist/context-compaction.js` there has no `compaction_update`/
# `clientSupportsCompactionUpdates` code path at all -- that branch was
# added in 0.78.0); 0.78.0 (Zed dist) additionally supports the
# standardized `compaction_update` presentation but only if Cortex
# advertises the `session.compaction` client capability at `initialize`,
# which it does not (T014 ResourcePolicy default). Both versions'
# `dist/context-compaction-meta.js` (the `_meta.contextCompaction` shape
# this module actually detects) are byte-identical. A ("claude-agent-acp",
# "*") wildcard entry is therefore added -- matching the same
# version-agnostic-until-evidence-differs convention
# ``engine.instruction_delivery_registry`` already uses for this same
# engine name -- so a live route resolved only by engine name (see
# ``delegation.live_adapters.ConfigurationServiceSnapshotSource``, which
# has no real installed-version discovery wired in yet) still gets the
# correct evidenced tier instead of an artificial downgrade.
# codex-acp 1.13.0 keeps the same `automatic` tier on its own dist evidence:
# `createCompletedContextCompactionUpdate` still carries
# `_meta: CONTEXT_COMPACTION_META`, which is the whole signal
# `engine.session_manager` detects. One verified difference, stated rather
# than glossed: 1.13.0 gates the *in_progress* start update behind
# `clientSupportsCompaction(capabilities) = capabilities?.session?.compaction
# != null`, and Cortex advertises no such capability (see
# `delegation.live_adapters._requested_capabilities`), so Cortex observes only
# the completed signal, never the start/complete pair 1.12.0 sends. The tier
# and its detection are unaffected; the start-phase COMPACTION_SIGNAL is not.
_TIER_REGISTRY: dict[tuple[str, str], CompactionTier] = {
    ("codex-acp", "1.12.0"): CompactionTier("codex-acp", "1.12.0", AUTOMATIC, _AUTOMATIC_LIMITATION),
    ("codex-acp", "1.13.0"): CompactionTier("codex-acp", "1.13.0", AUTOMATIC, _AUTOMATIC_LIMITATION),
    ("claude-agent-acp", "0.78.0"): CompactionTier("claude-agent-acp", "0.78.0", AUTOMATIC, _AUTOMATIC_LIMITATION),
    ("claude-agent-acp", "0.77.0"): CompactionTier("claude-agent-acp", "0.77.0", AUTOMATIC, _AUTOMATIC_LIMITATION),
    ("claude-agent-acp", "*"): CompactionTier("claude-agent-acp", "*", AUTOMATIC, _AUTOMATIC_LIMITATION),
    # OpenCode 2.0.19 documents a /compact command and advertises only the
    # init/review commands over ACP; no compaction signal was observed in
    # the no-paid probes, so Cortex claims no more than its own boundaries.
    ("opencode", "2.0.19"): CompactionTier("opencode", "2.0.19", CONTROLLED_BOUNDARY, _CONTROLLED_BOUNDARY_LIMITATION),
}


def tier_for(engine: str, version: str) -> CompactionTier:
    """Static, evidence-based tier for ``(engine, version)``.

    Falls back, in order: exact ``(engine, version)``; ``(engine, "*")``
    wildcard; ``controlled_boundary`` for a *known* engine name at an
    otherwise-unrecognized version (conservative: never claims automatic
    support without exact/wildcard evidence); ``unqualified`` for a
    completely unknown engine -- never a guessed/interpolated tier.
    """
    exact = _TIER_REGISTRY.get((engine, version))
    if exact is not None:
        return exact
    wildcard = _TIER_REGISTRY.get((engine, "*"))
    if wildcard is not None:
        return wildcard
    if engine in {name for name, _ in _TIER_REGISTRY}:
        return CompactionTier(engine, version, CONTROLLED_BOUNDARY, _CONTROLLED_BOUNDARY_LIMITATION)
    return CompactionTier(engine, version, UNQUALIFIED, _UNQUALIFIED_LIMITATION)


_OWNER_FOR_TIER = {AUTOMATIC: "engine_adapter", CONTROLLED_BOUNDARY: "cortex_controlled", UNQUALIFIED: "none"}


def ensure_owner(store, *, task_id: str, engine_session_ref: str, engine: str, version: str) -> CompactionTier:
    """Idempotently record the single compaction owner for
    ``engine_session_ref`` (``INSERT OR IGNORE`` on the primary key: the
    first call for a given session wins, matching "exactly one compaction
    owner per engine session"; a later call for the same session is a
    read-only no-op even if ``engine``/``version`` differ, since a session
    is bound to one adapter instance for its whole lifetime).
    """
    tier = tier_for(engine, version)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT OR IGNORE INTO compaction_owners "
                "(engine_session_ref, task_id, owner, tier, engine, engine_version, limitation, set_at) "
                "VALUES (?,?,?,?,?,?,?,?)",
                (engine_session_ref, task_id, _OWNER_FOR_TIER[tier.tier], tier.tier, engine, version,
                 tier.limitation, store._now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
        row = db.execute(
            "SELECT owner, tier, engine, engine_version, limitation FROM compaction_owners WHERE engine_session_ref=?",
            (engine_session_ref,),
        ).fetchone()
    return CompactionTier(row["engine"], row["engine_version"], row["tier"], row["limitation"])


def owner_row(store, engine_session_ref: str):
    with store.connection() as db:
        return db.execute(
            "SELECT owner, tier, engine, engine_version, limitation FROM compaction_owners WHERE engine_session_ref=?",
            (engine_session_ref,),
        ).fetchone()


def open_generation(
    store, *, task_id: str, engine_session_ref: str, recipient: str, cause: str,
    evidence_source: str, assignment_ref: str | None = None, evidence_id: str | None = None,
) -> dict:
    """Create the next (1-based, strictly increasing) generation for
    ``engine_session_ref``. Never deduplicated here -- callers that must
    dedup by evidence id (an observed compaction) go through
    :func:`record_compaction_event` instead, which only calls this once
    per distinct evidence id."""
    generation_id = new_id("cg_")
    now = store._now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            next_sequence = db.execute(
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS n FROM context_generations WHERE engine_session_ref=?",
                (engine_session_ref,),
            ).fetchone()["n"]
            db.execute(
                "INSERT INTO context_generations "
                "(id, engine_session_ref, task_id, assignment_ref, recipient, sequence, cause, "
                " evidence_source, evidence_id, created_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
                (generation_id, engine_session_ref, task_id, assignment_ref, recipient, next_sequence,
                 cause, evidence_source, evidence_id, now),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return dict(
        id=generation_id, engine_session_ref=engine_session_ref, task_id=task_id, assignment_ref=assignment_ref,
        recipient=recipient, sequence=next_sequence, cause=cause, evidence_source=evidence_source,
        evidence_id=evidence_id, created_at=now,
    )


# A duplicated/re-delivered compaction signal for the exact same evidence id
# can never open more than one generation (the dedup ledger's primary key
# already guarantees this); this is a defense-in-depth cap on how many
# distinct evidence ids a single session's ledger is expected to carry
# before a caller should stop trusting fresh triggers without investigation
# (never enforced as a hard failure -- a legitimately long-lived session can
# compact many times -- only surfaced via :func:`compaction_event_count`).
BOUNDED_EVENTS_PER_SESSION = 10_000


def record_compaction_event(
    store, *, task_id: str, engine_session_ref: str, evidence_id: str, phase: str,
    evidence_source: str, recipient: str, assignment_ref: str | None = None, cause: str = "observed_compaction",
) -> dict | None:
    """Dedup one observed compaction/reset event by
    ``(engine_session_ref, evidence_id)``. Only the *first* sighting of
    ``phase="completed"`` for a given evidence id opens a new generation
    (recorded by setting that row's ``generation_id``); an earlier
    non-terminal sighting (``"in_progress"``) updates the row's phase but
    never itself opens one, so the following terminal sighting for the
    same id still does. A truly duplicated terminal phase (this id's row
    already has a ``generation_id``) returns ``None``: "duplicate triggers
    ... are deduplicated ... and bounded" holds even if the adapter
    redelivers the exact same ``tool_call_update`` twice.
    """
    now = store._now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            existing = db.execute(
                "SELECT generation_id FROM compaction_events_seen WHERE engine_session_ref=? AND evidence_id=?",
                (engine_session_ref, evidence_id),
            ).fetchone()
            if existing is None:
                db.execute(
                    "INSERT INTO compaction_events_seen "
                    "(engine_session_ref, evidence_id, task_id, phase, observed_at, generation_id) "
                    "VALUES (?,?,?,?,?,NULL)",
                    (engine_session_ref, evidence_id, task_id, phase, now),
                )
                already_opened = False
            else:
                # An in-progress sighting of this same evidence id (or a
                # duplicated non-terminal phase) never claims the id
                # permanently -- only reaching (and staying at) a terminal
                # phase does, via generation_id being set below. A later
                # terminal phase for an id whose earlier sighting was only
                # ever non-terminal must still open exactly one generation.
                db.execute(
                    "UPDATE compaction_events_seen SET phase=?, observed_at=? WHERE engine_session_ref=? AND evidence_id=?",
                    (phase, now, engine_session_ref, evidence_id),
                )
                already_opened = existing["generation_id"] is not None
            db.commit()
        except BaseException:
            db.rollback()
            raise
    if already_opened or phase != "completed":
        return None
    generation = open_generation(
        store, task_id=task_id, engine_session_ref=engine_session_ref, recipient=recipient,
        cause=cause, evidence_source=evidence_source, assignment_ref=assignment_ref, evidence_id=evidence_id,
    )
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "UPDATE compaction_events_seen SET generation_id=? WHERE engine_session_ref=? AND evidence_id=?",
                (generation["id"], engine_session_ref, evidence_id),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return generation


def compaction_event_count(store, engine_session_ref: str) -> int:
    with store.connection() as db:
        return db.execute(
            "SELECT COUNT(*) AS n FROM compaction_events_seen WHERE engine_session_ref=?", (engine_session_ref,),
        ).fetchone()["n"]


def latest_generation(store, engine_session_ref: str) -> dict | None:
    with store.connection() as db:
        row = db.execute(
            "SELECT * FROM context_generations WHERE engine_session_ref=? ORDER BY sequence DESC LIMIT 1",
            (engine_session_ref,),
        ).fetchone()
    return dict(row) if row is not None else None


def generations_for_session(store, engine_session_ref: str) -> list[dict]:
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM context_generations WHERE engine_session_ref=? ORDER BY sequence",
            (engine_session_ref,),
        ).fetchall()
    return [dict(row) for row in rows]
