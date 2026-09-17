"""ConfigurationService: the one API both UI and command entry points call.

``ui_select`` and ``command_set`` are thin adapters over the same
:class:`ConfigurationService` methods, kept only so a parity test can call
both and assert identical errors/results for the same inputs -- neither
adapter carries any behavior of its own.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping

from .. import ownership
from ..catalog import store as catalog_store
from ..catalog.model import CatalogEntry
from ..connections import registry as connection_registry
from ..connections.model import ConnectionState
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import OwnershipEpoch
from . import store as configuration_store
from .policy import PolicyCeiling, require_narrowing
from .resolution import ConfigurationInputs, merge_layers, rederive_on_route_change, validate_dependent_settings


def _fail(category: PortableErrorCategory, code: str, remediation: str, retry_eligible: bool, *, field: str | None = None) -> PortableError:
    return PortableError(category=category, code=code, remediation=remediation, retry_eligible=retry_eligible, field=field)


def _require_epoch(db, task_id: str, current_epoch: OwnershipEpoch | None) -> None:
    """Reject an absent or stale caller epoch against ``task_owners``.

    ``ownership.current_owner_epoch`` returns only the current epoch integer
    (never an owner reference); a caller's ``OwnershipEpoch.epoch`` must
    match it exactly, and its presence at all is required -- missing is
    never treated as authorized.
    """
    owner_epoch = ownership.current_owner_epoch(db, task_id)
    if owner_epoch is None:
        raise _fail(PortableErrorCategory.AUTHORIZATION, "missing_ownership_epoch",
                    "This task has no owner yet; acquire ownership before proposing a configuration.", False, field="current_epoch")
    if current_epoch is None:
        raise _fail(PortableErrorCategory.AUTHORIZATION, "missing_ownership_epoch",
                    "Supply the current ownership epoch before proposing a configuration; absence is rejected, never treated as authorized.", False, field="current_epoch")
    if current_epoch.epoch != owner_epoch:
        raise _fail(PortableErrorCategory.AUTHORIZATION, "stale_ownership_epoch",
                    "The supplied ownership epoch does not match this task's current epoch; re-acquire ownership and retry.", False, field="current_epoch")


@dataclass(frozen=True)
class RecoveryRequested:
    """Returned (never raised) by :meth:`ConfigurationService.apply`.

    A revision that changes engine/connection relative to the currently
    active one is never silently applied in T008; the revision stays
    ``selected`` and the caller must drive an explicit engine switch
    (out of scope here) before applying it.
    """

    task_id: str
    old_engine_session_id: str
    new_revision_id: str
    reason: str


class ConnectionUnavailable(PortableError):
    """Raised by :meth:`ConfigurationService.reconnect_restore`.

    Never substitutes another connection/route and never consults user
    defaults; ``setup_action`` (may be ``None``) is surfaced verbatim.
    """

    def __init__(self, setup_action: str | None) -> None:
        super().__init__(
            category=PortableErrorCategory.VALIDATION,
            code="connection_unavailable",
            remediation=setup_action or "Reconfigure this task's connection before restoring its route; no substitute connection is offered.",
            retry_eligible=False,
        )
        self.setup_action = setup_action


class ConfigurationRejected(Exception):
    """Raised by a fake/real adapter to signal explicit rejection (not indeterminate)."""

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class ConfigurationService:
    """Owns every configuration mutation and read for one project's ``Store``."""

    def __init__(self, store) -> None:
        self.store = store

    # -- propose --------------------------------------------------------

    def propose(
        self,
        task_id: str,
        catalog_entry: CatalogEntry,
        inputs: ConfigurationInputs,
        expected_revision: int,
        current_epoch: OwnershipEpoch | None,
        *,
        user_layer: Mapping[str, object] | None = None,
        project_layer: Mapping[str, object] | None = None,
    ) -> dict[str, object]:
        """Propose (select) a new configuration revision.

        ``user_layer``/``project_layer`` are ``{"defaults": {...},
        "ceiling": PolicyCeiling}`` mappings; omitted layers default to
        empty defaults and an unrestricted ceiling (never widening what a
        narrower layer already set -- an absent layer is simply skipped by
        the merge, not "unrestricted wins").
        """
        user_layer = user_layer or dict(defaults={}, ceiling=PolicyCeiling())
        # A project ceiling may only narrow the user ceiling; an expansion is
        # rejected outright rather than silently discarded by intersect(). An
        # *omitted* project_layer means "no project layer in play" (nothing
        # to narrow): it is never merged into the effective ceiling at all --
        # not even via intersect() with an unrestricted default -- because
        # PolicyCeiling()'s own restrictive defaults (e.g. fallback consent
        # defaults to False) are not a universal identity element for every
        # dimension, so intersecting with a phantom default would spuriously
        # narrow the user ceiling.
        if project_layer is not None:
            require_narrowing(project_layer["ceiling"], user_layer["ceiling"])
            effective_ceiling = user_layer["ceiling"].intersect(project_layer["ceiling"])
        else:
            project_layer = dict(defaults={}, ceiling=PolicyCeiling())
            effective_ceiling = user_layer["ceiling"]
        with self.store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                _require_epoch(db, task_id, current_epoch)
                latest = configuration_store.latest_revision_tx(db, task_id)
                latest_number = latest["revision"] if latest is not None else 0
                previous_route_id = latest["route_id"] if latest is not None else None
                if expected_revision != latest_number:
                    raise _fail(PortableErrorCategory.VALIDATION, "stale_configuration_revision",
                                "This is not the latest configuration revision; re-read the latest revision and retry.", True, field="expected_revision")
                task_layer = configuration_store._row_dict(latest) if latest is not None else None
                parameters = merge_layers(
                    user_layer.get("defaults"),
                    project_layer.get("defaults"),
                    task_layer["parameters"] if task_layer is not None else None,
                    inputs.parameters,
                )
                parameters = rederive_on_route_change(previous_route_id, inputs.route, parameters)
                candidate_ceiling = inputs.ceiling_override if inputs.ceiling_override is not None else effective_ceiling
                require_narrowing(candidate_ceiling, effective_ceiling)
                validate_dependent_settings(catalog_entry, parameters)
                if catalog_entry.route.id != inputs.route.id:
                    raise _fail(PortableErrorCategory.VALIDATION, "route_catalog_mismatch",
                                "catalog_entry.route must match inputs.route exactly.", False, field="route")
                row = configuration_store.insert_revision_tx(db, task_id, latest_number + 1, inputs.route, parameters, candidate_ceiling)
                db.commit()
            except BaseException:
                db.rollback()
                raise
        catalog_store.record_task_route(self.store, task_id, catalog_entry)
        return row

    # -- apply / reconcile ----------------------------------------------

    def _start_pending_next_use(self, task_id, revision_id, engine_session_id, current_epoch):
        """First transaction of :meth:`apply`: validate and mark pending.

        Returns ``(revision_dict, RecoveryRequested | None)``; the caller
        applies the adapter only when the second element is ``None``.
        Raises (never returns a broken row) on any rejection: invalid
        state, a newer revision superseding this one, or a since-tightened
        project ceiling.
        """
        with self.store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                _require_epoch(db, task_id, current_epoch)
                row = configuration_store.revision_row_tx(db, task_id, revision_id)
                revision = configuration_store._row_dict(row)
                if revision["state"] != "selected":
                    raise _fail(PortableErrorCategory.VALIDATION, "invalid_revision_state",
                                f"Only a 'selected' revision may be applied (current state: {revision['state']}).", False, field="revision_id")
                latest = configuration_store.latest_revision_tx(db, task_id)
                if latest is None or latest["id"] != revision_id:
                    raise _fail(PortableErrorCategory.VALIDATION, "stale_configuration_revision",
                                "A newer configuration revision has since been proposed; re-read the latest revision before applying.", True, field="revision_id")
                project_root = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()["project_root"]
                project_ceiling = configuration_store.project_ceiling_tx(db, project_root)
                if project_ceiling is not None:
                    # The project ceiling may have tightened since this
                    # revision was selected; recheck before ever applying it.
                    require_narrowing(revision["ceiling_snapshot"], project_ceiling)
                active = configuration_store.current_active_tx(db, task_id)
                if active is not None and (active["route_engine"] != revision["route_engine"] or active["route_connection_id"] != revision["route_connection_id"]):
                    db.commit()
                    return revision, RecoveryRequested(
                        task_id=task_id,
                        old_engine_session_id=engine_session_id,
                        new_revision_id=revision_id,
                        reason="engine_or_connection_change_requires_recovery",
                    )
                configuration_store.transition_tx(db, task_id, revision_id, "pending_next_use")
                db.commit()
                return revision, None
            except PortableError as violation:
                if violation.code == "ceiling_violation":
                    db.rollback()
                    db.execute("BEGIN IMMEDIATE")
                    configuration_store.transition_tx(db, task_id, revision_id, "rejected", error_code="ceiling_tightened")
                    db.commit()
                else:
                    db.rollback()
                raise
            except BaseException:
                db.rollback()
                raise

    def _finish_transition(self, task_id, revision_id, state, current_epoch, **fields):
        """Terminal write for :meth:`apply`.

        ``adapter.configure`` runs entirely outside any transaction; between
        the transaction that marked a revision ``pending_next_use`` and this
        one, ownership may have transferred to a new owner (the caller's
        ``current_epoch`` was only checked back then). Re-reading
        ``ownership.current_owner_epoch`` here and comparing against the
        caller's own (now possibly stale) epoch closes that window: on a
        mismatch, this never writes ``active``/``failed`` -- the engine may
        or may not actually have applied the change, so truth is unknown to
        the store -- it writes ``indeterminate``/``stale_owner_apply``
        instead and raises, leaving :meth:`current` unchanged until the
        *current* owner reconciles using engine evidence.
        """
        stale_owner_error = None
        with self.store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                owner_epoch = ownership.current_owner_epoch(db, task_id)
                if owner_epoch is None or current_epoch is None or current_epoch.epoch != owner_epoch:
                    result = configuration_store.transition_tx(db, task_id, revision_id, "indeterminate", error_code="stale_owner_apply")
                    db.commit()
                    stale_owner_error = _fail(
                        PortableErrorCategory.AUTHORIZATION, "stale_owner_apply",
                        "Ownership changed while this configuration change was in flight; the engine may or may not "
                        "have applied it. The current owner must reconcile this revision using engine evidence "
                        "(see ConfigurationService.reconcile) before it can become active.",
                        False, field="current_epoch",
                    )
                else:
                    result = configuration_store.transition_tx(db, task_id, revision_id, state, **fields)
                    db.commit()
            except BaseException:
                db.rollback()
                raise
        if stale_owner_error is not None:
            raise stale_owner_error
        return result

    def apply(self, task_id: str, revision_id: str, adapter, engine_session_id: str, current_epoch: OwnershipEpoch | None):
        revision, recovery = self._start_pending_next_use(task_id, revision_id, engine_session_id, current_epoch)
        if recovery is not None:
            return recovery
        try:
            evidence_ref = adapter.configure(engine_session_id, revision)
        except ConfigurationRejected as rejected:
            return self._finish_transition(task_id, revision_id, "failed", current_epoch, error_code=rejected.reason)
        except BaseException:
            return self._finish_transition(task_id, revision_id, "indeterminate", current_epoch, error_code="apply_exception")
        # The adapter call happens outside any transaction; a newer revision
        # may have been proposed while it was in flight. Never let a stale
        # pending apply become active regardless of the adapter's outcome.
        with self.store.connection() as db:
            latest = configuration_store.latest_revision_tx(db, task_id)
        if latest is None or latest["id"] != revision_id:
            result = self._finish_transition(task_id, revision_id, "failed", current_epoch, error_code="stale_pending_apply")
            raise _fail(PortableErrorCategory.VALIDATION, "stale_pending_apply",
                        "A newer configuration revision was proposed while this one was pending application; it was never made active.", False, field="revision_id")
        if not evidence_ref:
            return self._finish_transition(task_id, revision_id, "indeterminate", current_epoch, error_code="unknown_apply_outcome")
        try:
            return self._finish_transition(task_id, revision_id, "active", current_epoch, confirmation_evidence_ref=evidence_ref)
        except PortableError as violation:
            if violation.code != "stale_reconcile_target":
                raise
            # The pre-check above raced: a newer (still-live) revision was
            # proposed in the narrow window between that read and this
            # write. The configuration_revisions_guard trigger caught it as
            # defense in depth; report it through apply()'s own
            # 'stale_pending_apply' vocabulary rather than leaking
            # reconcile()'s 'stale_reconcile_target' code from here.
            result = self._finish_transition(task_id, revision_id, "failed", current_epoch, error_code="stale_pending_apply")
            raise _fail(PortableErrorCategory.VALIDATION, "stale_pending_apply",
                        "A newer configuration revision was proposed while this one was pending application; it was never made active.", False, field="revision_id")

    def reconcile(
        self, task_id: str, revision_id: str, current_epoch: OwnershipEpoch | None, *,
        evidence_ref: str | None = None, failed: bool = False, error_code: str | None = None,
    ) -> dict[str, object]:
        """Resolve an ``indeterminate`` revision (including one left by a
        stale-owner ``apply``, error_code ``stale_owner_apply``) to
        ``active`` or ``failed``. Only the *current* owner may do this: a
        stale owner's own epoch no longer matches and is rejected exactly
        like every other mutation here.

        A revision only reconciles to ``active`` when no *live* newer
        revision exists for its task -- "live" meaning any state other than
        ``rejected``/``failed`` (so ``selected``, ``pending_next_use``,
        ``active``, or ``indeterminate``). If one was proposed at any point
        after this one -- for example a new owner already applied rev2
        while rev1 sat indeterminate from a stale-owner apply, or rev2 was
        merely proposed (``selected``) and never even applied yet -- then
        activating rev1 now would make the activation history incoherent
        (an older revision becoming active after a newer, still-relevant
        one exists). A newer revision that is itself already
        ``rejected``/``failed`` never blocks this, since it can no longer
        become current. Resolving the stale row to ``failed``
        (``error_code='superseded'``) is always still allowed regardless;
        that just records the true outcome without ever making it current.
        A second, DB-level trigger (``configuration_revisions_guard``)
        enforces the identical ``NOT IN ('rejected','failed')`` condition
        as defense in depth even if this service-level check were ever
        bypassed (for example a direct SQL write).
        """
        with self.store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                _require_epoch(db, task_id, current_epoch)
                row = configuration_store.revision_row_tx(db, task_id, revision_id)
                current = configuration_store._row_dict(row)
                if current["state"] != "indeterminate":
                    raise _fail(PortableErrorCategory.VALIDATION, "invalid_revision_state",
                                "Only an 'indeterminate' revision may be reconciled.", False, field="revision_id")
                superseded = configuration_store.has_live_newer_revision_tx(db, task_id, current["revision"])
                if failed:
                    result = configuration_store.transition_tx(
                        db, task_id, revision_id, "failed",
                        error_code="superseded" if superseded else (error_code or "reconciled_failed"),
                    )
                else:
                    if superseded:
                        raise _fail(PortableErrorCategory.VALIDATION, "stale_reconcile_target",
                                    "A newer configuration revision already exists for this task; this revision can "
                                    "only be reconciled to 'failed' now, never 'active'.", False, field="revision_id")
                    if not evidence_ref:
                        raise _fail(PortableErrorCategory.VALIDATION, "missing_confirmation_evidence",
                                    "Reconciling to 'active' requires a non-empty evidence_ref.", False, field="evidence_ref")
                    result = configuration_store.transition_tx(db, task_id, revision_id, "active", confirmation_evidence_ref=evidence_ref)
                db.commit()
            except BaseException:
                db.rollback()
                raise
        return result

    # -- reads ------------------------------------------------------------

    def current(self, task_id: str) -> dict[str, object] | None:
        with self.store.connection() as db:
            return configuration_store.current_active_tx(db, task_id)

    def history(self, task_id: str) -> tuple[dict[str, object], ...]:
        with self.store.connection() as db:
            return configuration_store.history_tx(db, task_id)

    def reconnect_restore(self, task_id: str, environ: dict[str, str] | None = None) -> dict[str, object]:
        """Restore the task's latest active route; never substitutes another."""
        current = self.current(task_id)
        if current is None:
            raise _fail(PortableErrorCategory.VALIDATION, "no_active_configuration",
                        "This task has no active configuration revision to restore.", False, field="task_id")
        registry_state = connection_registry.read_registry(environ)
        record = registry_state.connections.get(current["route_connection_id"])
        if record is None or record.state != ConnectionState.CONFIGURED:
            setup_action = None if record is None else "Reconfigure connection " + current["route_connection_id"]
            raise ConnectionUnavailable(setup_action)
        return current

    def resolve_worker(
        self,
        task_id: str,
        role: str,
        role_profile: Mapping[str, object],
        explicit_override=None,
    ) -> dict[str, object]:
        """Coordinator's active route by default; override only if permitted.

        ``role_profile`` is ``{"allows_role_alternates": bool}``.
        ``explicit_override`` is a full :class:`ConfigurationInputs`-shaped
        route/parameters choice; never accepts any cost field (see
        :class:`~.resolution.ConfigurationInputs`, which has none).
        """
        coordinator = self.current(task_id)
        if coordinator is None:
            raise _fail(PortableErrorCategory.VALIDATION, "no_active_configuration",
                        "The coordinator has no active configuration to inherit for role '%s'." % role, False, field="task_id")
        if explicit_override is None:
            return coordinator
        if not role_profile.get("allows_role_alternates", False):
            raise _fail(PortableErrorCategory.AUTHORIZATION, "forbidden_worker_override",
                        f"role '{role}' is not permitted to override the coordinator's active route.", False, field="explicit_override")
        require_narrowing(explicit_override.ceiling_override or coordinator["ceiling_snapshot"], coordinator["ceiling_snapshot"])
        return dict(
            route_id=explicit_override.route.id,
            route_engine=explicit_override.route.engine,
            route_connection_id=explicit_override.route.connection_id,
            parameters=dict(explicit_override.parameters),
        )

    # -- UI/command parity adapters --------------------------------------

    def ui_select(self, *args, **kwargs) -> dict[str, object]:
        return self.propose(*args, **kwargs)

    def command_set(self, *args, **kwargs) -> dict[str, object]:
        return self.propose(*args, **kwargs)
