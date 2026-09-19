"""T023: coordinator session recovery/replacement.

Replaces the pre-T023 fail-closed behavior (see
``acp.orchestrating_runtime.OrchestratingRuntime._ensure_coordinator``'s
former ``coordinator_session_not_cached`` error) for the case where a
``task_orchestration`` row already exists (a coordinator was previously
provisioned for this task) but the *current* runtime process's own
in-memory cache holds neither the channel secret nor an open session for
it -- almost always because the whole Cortex process restarted.

Sequence (see the T023 task card / lead design notes; every step is its
own committed SQLite transaction, so a crash between any two of them
leaves a resumable, never-corrupt state -- the ``next_step`` recorded on
``task_orchestration`` names exactly where a *subsequent* call into
:func:`recover` should resume, mirroring ``delegation.provisioning``'s own
step-resumption model):

1. Acquire a fresh ownership epoch (fencing): every trusted channel bound
   to the task's *old* epoch is rejected from this point on (see
   ``server.Server._dispatch_channeled``) except a still-legitimate
   worker's own channel, migrated forward to the new epoch in the same
   transaction (see ``ownership.reassign_worker_channels_tx``) rather than
   fenced out along with the coordinator's.
2. Reconcile the old coordinator attempt's fate via
   ``SessionRuntime.lookup_owned`` -- "alive" is terminated and its exit
   re-verified; anything not *confirmed* dead (still alive, or "unknown")
   refuses to start a replacement outright (never two mutating
   coordinators, never guessed from a timeout). Also reconciles every
   still-open delegated worker the same way (see
   ``recovery.worker_reconciliation``), so a worker whose result already
   arrived is never re-delegated and one that cannot be safely resumed is
   marked ``recovery_worker_lost`` instead of silently relaunched.
3. Revoke the old coordinator channel and issue a fresh one, for the
   *same* task id, bound to a freshly allocated attempt id.
4. Open a new engine session for that channel (``cause="session_replacement"``
   is recorded by the injected engine/adapter itself -- see
   ``delegation.live_adapters.EngineSessionManagerAdapter.open``'s
   ``launch["recovery"]`` flag threaded through by this module).
5. Verify the replacement did not silently broaden permissions or switch
   engine/connection without an explicit new configuration revision (see
   ``recovery.registry``'s ``attempt_effective_context``); refuse (and
   tear the replacement back down) if it did.

Every step after 1 is only ever reached with a stable epoch already held,
so a raise at any point after that leaves the task correctly *owned* by
this recovery attempt (a subsequent recovery call re-reads
``task_orchestration``/``trusted_channels`` and continues, never silently
retrying step 1 with a stale expected-epoch)."""
from __future__ import annotations

from datetime import datetime, timezone

from .. import ownership
from .. import _fault_injection
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import new_id
from ..trusted_channel import issue_channel_tx, resolve_channel, revoke_channel
from . import registry as recovery_registry
from . import worker_reconciliation


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _indeterminate(code: str, remediation: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.INDETERMINATE_WRITE, code=code,
        remediation=remediation, retry_eligible=False,
    )


def _authorization_error(code: str, remediation: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.AUTHORIZATION, code=code,
        remediation=remediation, retry_eligible=False,
    )


def _resource_policy_broadens(new_policy: dict, old_policy: dict) -> bool:
    if bool(new_policy.get("fs_local")) and not bool(old_policy.get("fs_local")):
        return True
    if bool(new_policy.get("terminal")) and not bool(old_policy.get("terminal")):
        return True
    new_limit = new_policy.get("output_byte_limit_max")
    old_limit = old_policy.get("output_byte_limit_max")
    if isinstance(new_limit, int) and isinstance(old_limit, int) and new_limit > old_limit:
        return True
    return False


def _policy_covers(declared: dict, actual: dict) -> bool:
    """True only if ``declared`` (a configuration revision's own explicit
    ``parameters["resource_policy_override"]``) actually grants everything
    ``actual`` (the resource policy the replacement session was really
    opened with) carries -- never inferred, never "any change is enough"."""
    if not isinstance(declared, dict):
        return False
    if bool(actual.get("fs_local")) and not bool(declared.get("fs_local")):
        return False
    if bool(actual.get("terminal")) and not bool(declared.get("terminal")):
        return False
    actual_limit = actual.get("output_byte_limit_max")
    declared_limit = declared.get("output_byte_limit_max")
    if isinstance(actual_limit, int):
        if not isinstance(declared_limit, int) or declared_limit < actual_limit:
            return False
    return True


def _fetch_revision(db, revision_id):
    if revision_id is None:
        return None
    return db.execute(
        "SELECT id, route_engine, route_connection_id, parameters FROM configuration_revisions WHERE id=?",
        (revision_id,),
    ).fetchone()


def _revision_parameters(row) -> dict:
    if row is None:
        return {}
    import json as _json

    try:
        return _json.loads(row["parameters"])
    except (TypeError, ValueError):
        return {}


def recover(store, engine, task_id: str, *, old_owner_epoch: int, old_attempt_ref: str,
            open_session) -> dict:
    """Run the full recovery sequence and return
    ``{"channel": ChannelContext, "session_ref": str, "attempt_ref": str,
    "route_changed": bool, "engine": str | None}``.

    ``open_session(task_id, attempt_ref, channel_id, secret) -> session_ref``
    is the caller's own coordinator-session-opening callable (see
    ``acp.orchestrating_runtime.OrchestratingRuntime._open_coordinator_session``),
    called with ``launch["recovery"] = True`` threaded through so a
    capacity/generation-tracking engine records ``cause="session_replacement"``
    rather than ``"session_open"``."""
    # -- 1. fresh ownership epoch (fencing) --------------------------------
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            result = ownership.acquire_task_owner_tx(db, task_id, f"recovery:{new_id('rc_')}", old_owner_epoch)
            ownership.reassign_worker_channels_tx(db, task_id, result.epoch)
            db.commit()
        except BaseException:
            db.rollback()
            raise
    new_epoch = result.epoch
    _fault_injection.checkpoint("t023_after_epoch_acquire")

    # -- 2. reconcile old coordinator attempt + every open worker ----------
    lookup = getattr(engine, "lookup_owned", None)
    terminate = getattr(engine, "terminate_owned", None)
    if lookup is not None:
        state = lookup(old_attempt_ref)
        if state == "absent":
            # Provably gone, so its capacity slot is provably free -- the
            # ordinary case after a crash or a restart. Nothing released
            # it before, and a coordinator slot nobody can reclaim wedges
            # the whole project: every later attempt queues behind it.
            worker_reconciliation._release_confirmed_capacity(
                store, old_attempt_ref, "recovery_coordinator_absent",
            )
        if state == "unknown":
            raise _indeterminate(
                "coordinator_recovery_indeterminate",
                "The prior coordinator attempt's process state could not be determined; "
                "investigate/terminate it out of band before recovery can proceed.",
            )
        if state == "alive":
            if terminate is not None:
                terminate(old_attempt_ref)
            after = lookup(old_attempt_ref)
            if after != "absent":
                raise _indeterminate(
                    "coordinator_recovery_ambiguous",
                    "The prior coordinator attempt's process could not be confirmed terminated; "
                    "capacity for it remains ambiguous, and a second mutating coordinator is refused.",
                )
    worker_reconciliation.reconcile(store, engine, task_id)
    _fault_injection.checkpoint("t023_after_old_session_reconciliation")
    # Captured *before* the replacement attempt is opened below -- reading
    # this after open() would otherwise see the just-inserted new row as
    # its own "latest" and silently skip every carry-over check.
    old_context = recovery_registry.latest_effective_context_for_task(store, task_id, "coordinator")

    # -- 3. revoke old coordinator channel, issue a fresh one --------------
    with store.connection() as db:
        old_row = db.execute(
            "SELECT channel_id FROM trusted_channels WHERE task_id=? AND role='coordinator' "
            "AND attempt_id=? AND revoked_at IS NULL",
            (task_id, old_attempt_ref),
        ).fetchone()
    if old_row is not None:
        revoke_channel(store, old_row["channel_id"])
    new_attempt_ref = new_id("co_")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            channel_id, secret = issue_channel_tx(db, task_id, "coordinator", new_epoch, attempt_id=new_attempt_ref)
            db.execute(
                "UPDATE task_orchestration SET coordinator_attempt_ref=?, state='provisioning', updated_at=? WHERE task_id=?",
                (new_attempt_ref, _now(), task_id),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    _fault_injection.checkpoint("t023_after_channel_revoke_issue")

    # -- 4. open a new engine session for the fresh channel -----------------
    session_ref = open_session(task_id, new_attempt_ref, channel_id, secret)
    _fault_injection.checkpoint("t023_after_new_session_open")

    # -- 5. verify no silent permission/route broadening --------------------
    route_changed = False
    engine_name = None
    new_context = recovery_registry.get_effective_context(store, new_attempt_ref)
    if new_context is not None:
        engine_name = new_context["route_engine"]
        if old_context is not None:
            config_changed = (
                new_context["effective_config_revision_id"] != old_context["effective_config_revision_id"]
            )
            route_differs = (
                new_context["route_engine"] != old_context["route_engine"]
                or new_context["route_connection_id"] != old_context["route_connection_id"]
            )
            policy_broadens = _resource_policy_broadens(new_context["resource_policy"], old_context["resource_policy"])
            new_revision_row = None
            if config_changed and (route_differs or policy_broadens):
                with store.connection() as db:
                    new_revision_row = _fetch_revision(db, new_context["effective_config_revision_id"])
                    prev_revision_row = _fetch_revision(db, old_context["effective_config_revision_id"])
            if route_differs:
                # A route/engine/connection switch is authorized only when
                # (a) the configuration actually changed, (b) *that specific*
                # revision transition itself changed the recorded route
                # (an effort-only/parameter-only revision never authorizes a
                # provider switch, even if some other config field changed),
                # and (c) the session actually opened on exactly the route
                # that revision recorded -- never a route that merely
                # happens to differ from the prior one for an unrelated
                # reason.
                authorized = (
                    config_changed
                    and new_revision_row is not None
                    and prev_revision_row is not None
                    and (
                        new_revision_row["route_engine"] != prev_revision_row["route_engine"]
                        or new_revision_row["route_connection_id"] != prev_revision_row["route_connection_id"]
                    )
                    and new_revision_row["route_engine"] == new_context["route_engine"]
                    and new_revision_row["route_connection_id"] == new_context["route_connection_id"]
                )
                if not authorized:
                    if terminate is not None:
                        terminate(new_attempt_ref)
                    revoke_channel(store, channel_id)
                    raise _authorization_error(
                        "unauthorized_route_change",
                        "Recovery attempted to switch engine/connection, but no configuration revision "
                        "whose own route transition (versus its immediate predecessor) actually authorizes "
                        "this exact change was found; refused. Select the new route explicitly through the "
                        "configuration flow first.",
                    )
                route_changed = True
            if policy_broadens:
                # Policy broadening is authorized only if the new revision's
                # own recorded parameters *explicitly* declare a resource
                # policy that covers the broader grant actually used --
                # never inferred merely from "some config revision changed".
                declared = _revision_parameters(new_revision_row).get("resource_policy_override")
                authorized = config_changed and _policy_covers(declared, new_context["resource_policy"])
                if not authorized:
                    if terminate is not None:
                        terminate(new_attempt_ref)
                    revoke_channel(store, channel_id)
                    raise _authorization_error(
                        "recovery_cannot_broaden_permissions",
                        "Recovery attempted to grant a broader resource policy than the recorded prior "
                        "attempt, and no configuration revision explicitly declares that broader policy "
                        "(parameters.resource_policy_override); refused.",
                    )

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "UPDATE task_orchestration SET state='running', updated_at=? WHERE task_id=?",
                (_now(), task_id),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise

    channel = resolve_channel(store, channel_id, secret)
    return dict(
        channel=channel, session_ref=session_ref, attempt_ref=new_attempt_ref,
        route_changed=route_changed, engine=engine_name,
    )
