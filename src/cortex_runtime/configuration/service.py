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
from ..portable.identity import ExecutionRoute, OwnershipEpoch
from . import store as configuration_store
from .policy import COORDINATOR_MODE_FLOOR, PolicyCeiling, RouteProfile, require_narrowing
from .resolution import ConfigurationInputs, merge_layers, rederive_on_route_change, validate_dependent_settings
from ..routing import store as routing_store


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


def _parameter_defaults(defaults):
    """Separate persisted owner identity/UI fields from native parameters.

    Project defaults also support the existing flat parameter mapping.
    """
    if not defaults:
        return {}
    if not isinstance(defaults, Mapping):
        raise _fail(PortableErrorCategory.VALIDATION, "invalid_parameter_defaults",
                    "Defaults must be a mapping with a parameter mapping, not an engine envelope.", False)
    if "coordinator" in defaults:
        from .resolution import portable_from_defaults
        return portable_from_defaults(defaults)["parameters"]
    if "parameters" in defaults:
        if not isinstance(defaults["parameters"], Mapping):
            raise _fail(PortableErrorCategory.VALIDATION, "invalid_parameter_defaults",
                        "defaults.parameters must be a mapping.", False)
        return defaults["parameters"]
    return {key: value for key, value in defaults.items()
            if key not in {"route_id", "route_connection_id", "route_model", "language", "ceiling"}}


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
        route_profile: RouteProfile | None = None,
        route_details: Mapping[str, ExecutionRoute] | None = None,
    ) -> dict[str, object]:
        """Propose (select) a new configuration revision.

        ``user_layer``/``project_layer`` are ``{"defaults": {...},
        "ceiling": PolicyCeiling}`` mappings; omitted layers default to
        empty defaults and an unrestricted ceiling (never widening what a
        narrower layer already set -- an absent layer is simply skipped by
        the merge, not "unrestricted wins"). A prior task ceiling is retained
        and intersected with supplied layers; omission cannot reset it.

        ``route_profile`` (T027) is this revision's explicit mixed-team
        route authorization: omitted means retain the prior profile, or on
        the first revision inherit this revision's own ``inputs.route`` --
        see :class:`~.policy.RouteProfile`. Every route id named anywhere
        in a non-default profile must already be a member of the
        *effective* ceiling's ``allowed_routes`` (when the ceiling
        restricts it at all); a profile naming an out-of-ceiling route is
        rejected atomically with the rest of this proposal, exactly like
        any other ceiling-narrowing violation.

        ``route_details`` (T027) maps every route id named in a non-default
        ``route_profile`` to the :class:`~..portable.identity
        .ExecutionRoute` it identifies -- the durable record the fallback-
        trigger wiring (``acp.orchestrating_runtime.OrchestratingRuntime
        ._attempt_worker_route_fallback``) later reads back to resolve a
        candidate target route id to an actual engine/connection/model (a
        bare route id is an opaque content fingerprint; nothing recovers
        those from it alone). A route named in ``route_profile`` with no
        entry here is still accepted for authorization purposes, but can
        never later be *chosen* as a fallback target.
        """
        layers_omitted = user_layer is None and project_layer is None
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
                if task_layer is not None:
                    # Changing route/effort is not permission to discard the
                    # task's previously materialized narrowing.
                    effective_ceiling = (task_layer["ceiling_snapshot"] if layers_omitted else
                                         effective_ceiling.intersect(task_layer["ceiling_snapshot"]))
                parameters = merge_layers(
                    _parameter_defaults(user_layer.get("defaults")),
                    _parameter_defaults(project_layer.get("defaults")),
                    task_layer["parameters"] if task_layer is not None else None,
                    inputs.parameters,
                )
                parameters = rederive_on_route_change(previous_route_id, inputs.route, parameters)
                # T035 coordinator mode floor (see configuration.policy
                # .COORDINATOR_MODE_FLOOR's own docstring for the exact
                # values/rationale and the one disclosed gap): this is the
                # *only* place "mode" is ever set for the coordinator's own
                # revision (propose() has no other caller -- see
                # ConfigurationService's module docstring), so overriding it
                # here -- unconditionally replacing whatever "mode" value
                # (if any) the caller/user requested -- is a floor, never a
                # preference: a user-chosen mode never survives this for a
                # known live engine. Only applied when this route's own
                # catalog-observed supported_parameters already lists the
                # floor value as a supported "mode" option; when it does
                # not (never yet discovered from a live session -- always
                # true for a connection's very first proposal, since
                # catalog.model_seed seeds "model" only, never "mode"), this
                # is silently skipped rather than injecting a value nothing
                # has ever confirmed the engine actually supports -- see
                # EngineSessionManager.configure's own wire-level
                # confirm-or-fail gate (T035 FINDING-02) for the backstop
                # once "mode" *is* known.
                mode_floor = COORDINATOR_MODE_FLOOR.get(inputs.route.engine)
                if mode_floor is not None:
                    catalog_modes = {value for name, value in catalog_entry.route.supported_parameters if name == "mode"}
                    if mode_floor in catalog_modes:
                        parameters = dict(parameters)
                        parameters["mode"] = mode_floor
                candidate_ceiling = inputs.ceiling_override if inputs.ceiling_override is not None else effective_ceiling
                require_narrowing(candidate_ceiling, effective_ceiling)
                validate_dependent_settings(catalog_entry, parameters)
                if catalog_entry.route.id != inputs.route.id:
                    raise _fail(PortableErrorCategory.VALIDATION, "route_catalog_mismatch",
                                "catalog_entry.route must match inputs.route exactly.", False, field="route")
                row = configuration_store.insert_revision_tx(db, task_id, latest_number + 1, inputs.route, parameters, candidate_ceiling)
                previous_extras = routing_store.get_route_extras_tx(db, latest["id"]) if latest is not None else None
                profile = (route_profile if route_profile is not None else
                           previous_extras["route_profile"] if previous_extras is not None else RouteProfile())
                if profile.role_routes is not None and candidate_ceiling.allowed_routes is not None:
                    every_named_route = frozenset().union(*profile.role_routes.values()) if profile.role_routes else frozenset()
                    if not every_named_route.issubset(candidate_ceiling.allowed_routes):
                        raise _fail(PortableErrorCategory.VALIDATION, "ceiling_violation",
                                    "route_profile names a route outside the effective ceiling's allowed_routes.", False, field="route_profile")
                encoded_details = {
                    route_id: dict(engine=route.engine, connection_id=route.connection_id, model=route.model)
                    for route_id, route in (route_details or {}).items()
                }
                if route_profile is None and previous_extras is not None:
                    encoded_details = dict(previous_extras["route_details"], **encoded_details)
                encoded_details.setdefault(
                    inputs.route.id,
                    dict(engine=inputs.route.engine, connection_id=inputs.route.connection_id, model=inputs.route.model),
                )
                routing_store.insert_route_extras_tx(
                    db, revision_id=row["id"], task_id=task_id, route_model=inputs.route.model, route_profile=profile,
                    route_details=encoded_details,
                )
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
        """``adapter.configure(engine_session_id, revision)`` -- ``revision``
        is always the flat storage-row dict (see
        ``configuration.store._row_dict``), exactly what every existing
        ``adapter.configure()`` implementation here is tested against
        (``tests/test_configuration_service.py``'s ``_FakeAdapter``, and
        any other generic caller). A caller that needs the richer portable
        ``ConfigurationRevision``/``ExecutionRoute`` shape
        ``engine.session_manager.EngineSessionManager.configure`` (and the
        adapter-specific option-mapping callables it uses) expects must
        translate this dict itself before delegating -- see
        ``delegation.live_adapters.EngineSessionManagerAdapter
        ._maybe_apply_configuration``'s ``_PortableRevisionConfigureAdapter``,
        the only production caller of this method.
        """
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

    def current_or_latest(self, task_id: str) -> dict[str, object] | None:
        """T031: the active revision if one exists, else the single most
        recent revision regardless of state.

        Lets a caller resolve *which route to open a session against*
        before that route's configuration has ever been confirmed/applied
        -- the only legitimate use is
        ``delegation.live_adapters.ConfigurationServiceSnapshotSource``,
        immediately followed by :meth:`apply` once the session actually
        opens (see that method's own docstring); nothing here ever claims
        the returned revision is active. Never used by
        :meth:`reconnect_restore` (which still requires a genuinely active
        revision) or any other caller that must not silently reuse an
        unconfirmed/rejected configuration.
        """
        history = self.history(task_id)
        current = self.current(task_id)
        if current is not None:
            # A strictly newer live selection wins over the active
            # revision. It is what the next open will apply anyway (see
            # ``live_adapters.EngineSessionManagerAdapter
            # ._maybe_apply_configuration``, which uses ``latest_live``),
            # and preferring the active one here could keep a thread from
            # ever opening again: live (2026-09-19) a task was active on an
            # effort its route policy forbade, the user selected an allowed
            # one, and every prompt still resolved the forbidden active
            # revision, failed authorization, and so never reached the
            # apply that would have promoted the new choice. The only way
            # out was editing the policy.
            newer = self.latest_live(task_id)
            if newer is not None and newer["revision"] > current["revision"] and newer["state"] == "selected":
                return newer
            return current
        latest = history[-1] if history else None
        # A 'rejected' latest revision is never returned here: per the
        # ``configuration_revisions_guard`` trigger, 'rejected' is only ever
        # reached directly from 'selected' (a ceiling-narrowing violation
        # caught in ``_start_pending_next_use``, *before* this revision was
        # ever actually used to open a session) -- it is never a legitimate
        # "route to open a session against". 'failed', by contrast, is only
        # ever reached from 'pending_next_use'/'indeterminate', i.e. this
        # revision's route *was* already used for a real, already-open
        # session -- the engine just rejected/never confirmed the
        # configuration change itself (see
        # ``ConfigurationServiceSnapshotSource.snapshot``'s repeated calls
        # across a coordinator's later continuations, which must keep
        # resolving that same still-in-use route).
        if latest is not None and latest["state"] == "rejected":
            return None
        return latest

    def latest_live(self, task_id: str) -> dict[str, object] | None:
        """EXT-01 finding #2: the task's own current *target* configuration
        -- the highest-numbered revision that has not settled into a
        terminal negative outcome (``'rejected'``/``'failed'``) -- walking
        backward through history for the rare case where the very latest
        one has (in which case the next-highest still-live revision, almost
        always the previously-'active' one, is the honest answer). Returns
        ``None`` only when every revision this task has ever had is
        ``'rejected'``/``'failed'`` (including "no revision at all").

        Deliberately distinct from :meth:`current_or_latest`, which prefers
        an already-``'active'`` revision even when a strictly newer one has
        since been proposed -- correct for *that* method's own one
        legitimate use (resolving a route to open a brand-new session
        against, see its own docstring), wrong for reconciling an
        *existing* session's own confirmation: a mid-task selection change
        (``acp.agent._apply_mid_task_selection``) always creates a newer
        revision than whatever is currently ``'active'``, and it is that
        newer one -- not the stale ``'active'`` one -- a live session must
        be reconciled toward (see
        ``delegation.live_adapters.EngineSessionManagerAdapter
        .reconcile_configuration``, this method's only caller).
        """
        for revision in reversed(self.history(task_id)):
            if revision["state"] not in ("rejected", "failed"):
                return revision
        return None

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
        role_profile: Mapping[str, object] | None = None,
        explicit_override=None,
    ) -> dict[str, object]:
        """Coordinator's active route by default; override only within the
        active revision's own T027 :class:`~.policy.RouteProfile`.

        ``role_profile`` is accepted only for call-site backward
        compatibility (``{"allows_role_alternates": bool}``, pre-T027) and,
        when supplied with ``allows_role_alternates`` falsy, forbids any
        override outright before the route-profile check ever runs -- it
        can never *widen* what the stored route profile itself authorizes.
        The authoritative source of truth is always the active
        configuration revision's own persisted route profile (T027,
        ``routing.store.get_route_extras``): a role absent from a non-
        default profile, or every role under the default single-route
        profile, is only ever authorized for the coordinator's own route.
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
        if role_profile is not None and not role_profile.get("allows_role_alternates", True):
            raise _fail(PortableErrorCategory.AUTHORIZATION, "forbidden_worker_override",
                        f"role '{role}' is not permitted to override the coordinator's active route.", False, field="explicit_override")
        extras = routing_store.get_route_extras(self.store, coordinator["id"])
        profile = extras["route_profile"] if extras is not None else RouteProfile(role_routes=None)
        allowed = profile.allowed_routes_for(role, coordinator["route_id"])
        if coordinator["ceiling_snapshot"].allowed_routes is not None:
            allowed = allowed & coordinator["ceiling_snapshot"].allowed_routes
        if explicit_override.route.id not in allowed:
            raise _fail(PortableErrorCategory.AUTHORIZATION, "forbidden_worker_override",
                        f"role '{role}' is not authorized for route {explicit_override.route.id!r}; "
                        f"only {{{', '.join(sorted(allowed))}}} is authorized under this task's route profile.",
                        False, field="explicit_override")
        require_narrowing(explicit_override.ceiling_override or coordinator["ceiling_snapshot"], coordinator["ceiling_snapshot"])
        return dict(
            route_id=explicit_override.route.id,
            route_engine=explicit_override.route.engine,
            route_connection_id=explicit_override.route.connection_id,
            parameters=dict(explicit_override.parameters),
        )

    def resolve_worker_route(self, task_id: str, role_id: str) -> dict[str, object] | None:
        """EXT-01 carried item: the ordinary-dispatch counterpart to
        :meth:`resolve_worker`'s ``explicit_override`` branch -- auto-
        selects the one route this task's own stored mixed-team
        :class:`~.policy.RouteProfile` authorizes ``role_id`` for, when
        that authorization is unambiguous, so an ordinary ``delegate()``
        (which never carries an explicit route choice of its own) can
        still dispatch a role to a different engine/connection than the
        coordinator's own.

        Returns ``None`` -- meaning "use the coordinator's own current
        route, unchanged" -- whenever: there is no current/latest
        revision to resolve against; this revision's route profile is the
        default single-route-inheritance marker (``role_routes is None``);
        or the role's own authorized set (profile narrowed by ceiling,
        see ``routing.authorization.allowed_routes_for_role``) contains
        anything other than exactly one route id that both differs from
        the coordinator's own and has a resolvable ``(engine,
        connection_id, model)`` recorded for it (``route_details``,
        T027). Ambiguous (more than one candidate) or empty (role denied,
        or named only for the coordinator's own route) never guesses --
        the caller's own downstream ``authorize_dispatch`` still enforces
        deny-by-default for a role missing from an explicit profile.
        """
        from ..routing import authorization as routing_authorization

        coordinator = self.current_or_latest(task_id)
        if coordinator is None:
            return None
        extras = routing_store.get_route_extras(self.store, coordinator["id"])
        if extras is None:
            return None
        profile = extras["route_profile"]
        if profile.is_default_single_route():
            return None
        allowed = routing_authorization.allowed_routes_for_role(
            profile, coordinator["ceiling_snapshot"], role_id, coordinator["route_id"],
        )
        candidates = {
            route_id for route_id in allowed
            if route_id != coordinator["route_id"] and route_id in extras["route_details"]
        }
        if len(candidates) != 1:
            return None
        target_route_id = next(iter(candidates))
        detail = extras["route_details"][target_route_id]
        return dict(
            route_id=target_route_id, route_engine=detail["engine"],
            route_connection_id=detail["connection_id"], route_model=detail.get("model"),
        )

    # -- UI/command parity adapters --------------------------------------

    def ui_select(self, *args, **kwargs) -> dict[str, object]:
        return self.propose(*args, **kwargs)

    def command_set(self, *args, **kwargs) -> dict[str, object]:
        return self.propose(*args, **kwargs)
