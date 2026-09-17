"""Atomic delegation provisioning: one semantic operation, no hand-built chain.

``ProvisioningService.delegate`` turns one coordinator request (title,
brief, role, acceptance) into a usable, isolated execution binding:
assignment -> attempt -> workspace -> channel -> config_snapshot ->
context_snapshot -> engine_session -> ready. Every technical identifier
(task/assignment/attempt/workspace/channel/config-revision/engine-session)
is derived from the caller's trusted :class:`trusted_channel.ChannelContext`
and this service's own bookkeeping -- the model never supplies one.

Recovery model
---------------
``delegation_provisioning`` holds exactly one row per
``idempotency_key = sha256(task_id|coordinator_channel_id|request_key)``.
Its ``step`` column names the *next* step to perform; a retry with the
same key resumes from that step rather than restarting. For the three
steps that touch state outside this process's single SQLite transaction
(workspace, channel, engine_session), an "intent" row is committed to
``delegation_effects`` *before* the external call and an "outcome" row is
committed *after* it succeeds. If a retry finds an intent with no
matching outcome, it reconciles via the injected protocol's lookup
instead of blindly repeating the effect:

- workspace: ``WorkspaceProvisioner.lookup`` -- present means adopt the
  existing workspace; absent means retry the create.
- engine_session: ``EngineSessionOpener.lookup_owned`` -- "alive" means
  adopt (terminate_owned then reopen only if a *new* open is also
  required, which cannot happen once alive); "absent" means retry open;
  "unknown" means fail closed with ``provisioning_indeterminate`` rather
  than risk a second live session.
- channel: reconciled by a direct, deterministic query for a channel
  already bound to this attempt id (channel issuance is a single local
  SQLite transaction, so the only uncertain window is between that
  transaction's own commit and this service recording the outcome; no
  external lookup protocol is needed).

Concurrent ``delegate`` calls under the *same* idempotency key (for
example two threads racing a resumed retry) converge on one live
assignment via a compare-and-swap claim on the row's ``step`` column
(``UPDATE ... WHERE step=?``): the loser of a claim re-reads the row and
either adopts the winner's completed step or performs a bounded wait
before re-checking, never performing the same external effect twice.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import time
from typing import Any, Mapping, Protocol

from ..contracts import StoreError
from ..portable.errors import PortableError, PortableErrorCategory
from ..engine.instruction_delivery_registry import delivery_for
from ..portable.instruction_delivery import DeliveryQualification, qualify_delivery
from ..portable.roles import load_role, resolve_role
from ..store import identifier
from ..trusted_channel import ChannelContext
from . import contracts as delegation_contracts

STEP_ORDER = (
    # "assignment" and "attempt" are pure local id allocation (see
    # _validate_prefixed_id prefixes "a_"/"xa_" in portable.identity) with no
    # external effect, so they are performed together with row creation in
    # ProvisioningService.delegate rather than as their own persisted `step`
    # value; the remaining names below are the actual `delegation_
    # provisioning.step` values a row moves through in order.
    "assignment",
    "attempt",
    "workspace",
    "channel",
    "config_snapshot",
    "context_snapshot",
    "engine_session",
    "ready",
)
_EXTERNAL_STEPS = frozenset({"workspace", "channel", "engine_session"})


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _idempotency_key(task_id: str, coordinator_channel_id: str, request_key: str) -> str:
    payload = f"{task_id}|{coordinator_channel_id}|{request_key}".encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


class WorkspaceProvisioner(Protocol):
    """Injected external effect: an isolated workspace keyed by attempt id."""

    def create(self, attempt_id: str) -> str:
        """Create (or idempotently return) a workspace and return its binding ref."""
        ...

    def lookup(self, attempt_id: str) -> str:
        """Return 'present' or 'absent' for a workspace owned by ``attempt_id``."""
        ...

    def remove(self, attempt_id: str) -> None:
        """Release a workspace owned by ``attempt_id``; safe if already absent."""
        ...


class EngineSessionOpener(Protocol):
    """Injected external effect: a live engine session for one attempt."""

    def open(self, workspace_binding_id: str, launch: Mapping[str, Any], instructions: str) -> str:
        """Open a new engine session and return its portable session ref."""
        ...

    def lookup_owned(self, attempt_id: str) -> str:
        """Return 'alive', 'absent', or 'unknown' for a session owned by ``attempt_id``."""
        ...

    def terminate_owned(self, attempt_id: str) -> None:
        """Terminate a session owned by ``attempt_id``; safe if already absent."""
        ...


class ConfigSnapshotSource(Protocol):
    """Injected: the coordinator route's current configuration snapshot.

    Must be pure/idempotent for a given ``task_id`` -- ``delegate`` may
    call it more than once (a pre-check for delivery qualification, then
    again to persist the ``config_snapshot`` step) and requires identical
    results.
    """

    def snapshot(self, task_id: str) -> Mapping[str, Any]:
        """Return at least {'engine': str, 'version': str, 'config_revision': str}."""
        ...


@dataclass(frozen=True)
class ProvisioningResult:
    idempotency_key: str
    assignment_ref: str
    task_id: str
    role_id: str
    assignment_id: str
    attempt_id: str
    workspace_binding_id: str
    channel_id: str
    config_revision: str
    context_snapshot_ref: str
    engine_session_ref: str
    instruction_delivery: str
    status: str


def _row(db, idempotency_key: str):
    return db.execute(
        "SELECT * FROM delegation_provisioning WHERE idempotency_key=?", (idempotency_key,)
    ).fetchone()


def _record_effect(db, idempotency_key: str, step: str, phase: str, detail: Mapping[str, Any]) -> None:
    db.execute(
        "INSERT INTO delegation_effects (idempotency_key, step, phase, detail_json, at) VALUES (?,?,?,?,?)",
        (idempotency_key, step, phase, json.dumps(dict(detail), sort_keys=True, ensure_ascii=False), _now()),
    )


def _base_step(step_value: str) -> str:
    """Strip a transient ``"claimed:<step>"`` marker back to its bare step name."""
    return step_value.split(":", 1)[1] if step_value.startswith("claimed:") else step_value


def _last_effect(db, idempotency_key: str, step: str, phase: str):
    return db.execute(
        "SELECT detail_json FROM delegation_effects WHERE idempotency_key=? AND step=? AND phase=? ORDER BY id DESC LIMIT 1",
        (idempotency_key, step, phase),
    ).fetchone()


class ProvisioningService:
    """Provisions one delegation's execution binding, recoverably.

    ``allow_unqualified_delivery`` is a constructor-only escape hatch (see
    ``portable.instruction_delivery.qualify_delivery``): it is never a
    per-call/tool argument. When it is used to admit an UNQUALIFIED
    channel for a role with ``critical_rules``, the resulting row records
    ``instruction_delivery='unqualified'``.
    """

    def __init__(
        self,
        store,
        workspace_provisioner: WorkspaceProvisioner,
        engine_session_opener: EngineSessionOpener,
        config_snapshot_source: ConfigSnapshotSource,
        *,
        allow_unqualified_delivery: bool = False,
    ) -> None:
        self._store = store
        self._workspace = workspace_provisioner
        self._engine = engine_session_opener
        self._config = config_snapshot_source
        self._allow_unqualified_delivery = bool(allow_unqualified_delivery)
        # T016: a freshly-issued worker channel's plaintext secret, held only
        # in this process's memory for the span between _step_channel
        # issuing it and _step_engine_session consuming it into `launch`
        # (never persisted -- see trusted_channel's "returned exactly once"
        # security model). Absent (None) on any retry that adopts an
        # already-issued channel; a retry's engine-session step only ever
        # reconciles via lookup_owned in that case, never calling
        # EngineSessionOpener.open() again, so it never needs the secret.
        self._pending_channel_secrets: dict[str, str] = {}

    # -- claim/resume plumbing -------------------------------------------------

    def _claim(self, db, idempotency_key: str, expected_step: str, next_step: str) -> bool:
        cursor = db.execute(
            "UPDATE delegation_provisioning SET step=?, updated_at=? WHERE idempotency_key=? AND step=?",
            (next_step, _now(), idempotency_key, expected_step),
        )
        return cursor.rowcount == 1

    def _claim_exclusive(self, idempotency_key: str, step_name: str, *, steal: bool = False) -> bool:
        """Exclusively claim the right to perform ``step_name``'s external effect.

        Sets ``step`` to a transient ``"claimed:<step_name>"`` marker via a
        CAS from the bare step name (``steal=False``, the normal path: at
        most one concurrent caller wins). ``steal=True`` instead CAS-moves
        from the marker to itself and is used only by :meth:`_acquire_step`
        after a bounded wait found no progress -- modeling recovery from a
        crashed prior claimant, never a race against a still-live one (a
        live claimant finishes fast enough that the bounded wait observes
        its progress instead).
        """
        marker = "claimed:" + step_name
        source = marker if steal else step_name
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                cursor = db.execute(
                    "UPDATE delegation_provisioning SET step=?, updated_at=? "
                    "WHERE idempotency_key=? AND step=?",
                    (marker, _now(), idempotency_key, source),
                )
                won = cursor.rowcount == 1
                db.commit()
            except BaseException:
                db.rollback()
                raise
        return won

    def _acquire_step(self, idempotency_key: str, step_name: str, next_step: str) -> bool:
        """Return True iff this call must perform ``step_name``'s effect.

        False means another caller already completed it (observed via a
        bounded wait) or is stealing/completing it concurrently; the
        caller's outer loop re-reads the row and proceeds from wherever it
        actually is.
        """
        if self._claim_exclusive(idempotency_key, step_name):
            return True
        try:
            self._wait_for_step_at_least(idempotency_key, next_step, max_attempts=40)
            return False
        except PortableError as error:
            if error.code != "provisioning_contended":
                raise
            if self._claim_exclusive(idempotency_key, step_name, steal=True):
                return True
            self._wait_for_step_at_least(idempotency_key, next_step, max_attempts=40)
            return False

    def _wait_for_step_at_least(self, idempotency_key: str, target_step: str, *, max_attempts: int = 200) -> str:
        target_index = STEP_ORDER.index(target_step)
        for _ in range(max_attempts):
            with self._store.connection() as db:
                row = _row(db, idempotency_key)
            if row is None:
                raise PortableError(
                    category=PortableErrorCategory.TRANSPORT,
                    code="provisioning_row_missing",
                    remediation="The provisioning row disappeared during a concurrent claim; retry the identical delegate() call.",
                    retry_eligible=True,
                )
            if row["status"] == "failed":
                raise PortableError(
                    category=PortableErrorCategory.INDETERMINATE_WRITE,
                    code=row["error_code"] or "provisioning_failed",
                    remediation="A concurrent provisioning attempt for this idempotency key failed; reconcile before retrying.",
                    retry_eligible=False,
                )
            if STEP_ORDER.index(_base_step(row["step"])) >= target_index:
                return row["step"]
            time.sleep(0.005)
        raise PortableError(
            category=PortableErrorCategory.TRANSPORT,
            code="provisioning_contended",
            remediation="A concurrent provisioning attempt is still in progress after a bounded wait; retry the identical delegate() call.",
            retry_eligible=True,
        )

    # -- public API --------------------------------------------------------

    def delegate(
        self,
        channel: ChannelContext,
        title: str,
        brief: str,
        role: str,
        acceptance: str,
        request_key: str,
    ) -> ProvisioningResult:
        if not isinstance(channel, ChannelContext):
            raise PortableError(
                category=PortableErrorCategory.AUTHORIZATION,
                code="invalid_channel",
                remediation="'channel' must be a resolved trusted_channel.ChannelContext.",
                retry_eligible=False,
                field="channel",
            )
        if channel.role != "coordinator":
            raise PortableError(
                category=PortableErrorCategory.AUTHORIZATION,
                code="coordinator_only",
                remediation="Only the bound coordinator channel may delegate; a worker channel cannot.",
                retry_eligible=False,
                field="channel",
            )
        delegation_contracts.validate(
            "coordinator", "delegate",
            dict(title=title, brief=brief, role=role, acceptance=acceptance, request_key=request_key),
        )
        role_id = resolve_role(role)
        role_set = load_role(role_id)
        task_id = channel.task_id
        idempotency_key = _idempotency_key(task_id, channel.channel_id, request_key)

        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                current_epoch = db.execute(
                    "SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)
                ).fetchone()
                current_epoch = current_epoch["epoch"] if current_epoch is not None else None
                if current_epoch is None or current_epoch != channel.owner_epoch:
                    raise PortableError(
                        category=PortableErrorCategory.AUTHORIZATION,
                        code="stale_ownership_epoch",
                        remediation="This channel's ownership epoch no longer matches the task's current owner; re-issue a channel under the current epoch before delegating.",
                        retry_eligible=False,
                    )
                row = _row(db, idempotency_key)
                if row is None:
                    assignment_id = identifier("a_")
                    attempt_id = identifier("xa_")
                    db.execute(
                        "INSERT INTO delegation_provisioning "
                        "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
                        "VALUES (?,?,?,?,?,?,?,?,?,?)",
                        (idempotency_key, task_id, assignment_id, attempt_id, role_id, "workspace", "in_progress",
                         channel.owner_epoch, _now(), _now()),
                    )
                    row = _row(db, idempotency_key)
                elif row["task_id"] != task_id or row["role_id"] != role_id:
                    raise PortableError(
                        category=PortableErrorCategory.VALIDATION,
                        code="idempotency_key_reused",
                        remediation="This request_key was already used for a different task/role under this channel; choose a new request_key for a new delegation.",
                        retry_eligible=False,
                        field="request_key",
                    )
                db.commit()
            except BaseException:
                db.rollback()
                raise

        if row["status"] == "ready":
            return self._result_from_row(row)
        if row["status"] == "failed":
            raise PortableError(
                category=PortableErrorCategory.INDETERMINATE_WRITE,
                code=row["error_code"] or "provisioning_failed",
                remediation="A prior attempt under this idempotency key failed; reconcile the named resource before retrying with a new request_key.",
                retry_eligible=False,
            )

        # Delivery qualification is checked (and, on refusal, raises) before
        # any external effect -- see module docstring. A pure pre-check call
        # to the (idempotent) config snapshot source supplies engine/version.
        preview = self._config.snapshot(task_id)
        qualify_delivery(
            role_set, delivery_for(preview["engine"], preview["version"]), [],
            allow_unqualified_delivery=self._allow_unqualified_delivery,
        )

        return self._run_steps(idempotency_key, channel, role_set)

    def _result_from_row(self, row) -> ProvisioningResult:
        return ProvisioningResult(
            idempotency_key=row["idempotency_key"],
            assignment_ref=row["idempotency_key"],
            task_id=row["task_id"],
            role_id=row["role_id"],
            assignment_id=row["assignment_id"],
            attempt_id=row["attempt_id"],
            workspace_binding_id=row["workspace_binding_id"],
            channel_id=row["channel_id"],
            config_revision=row["config_revision"],
            context_snapshot_ref=row["context_snapshot_ref"],
            engine_session_ref=row["engine_session_ref"],
            instruction_delivery=row["instruction_delivery"],
            status=row["status"],
        )

    def _fail(self, idempotency_key: str, error_code: str) -> None:
        try:
            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                try:
                    db.execute(
                        "UPDATE delegation_provisioning SET status='failed', error_code=?, updated_at=? WHERE idempotency_key=?",
                        (error_code, _now(), idempotency_key),
                    )
                    db.commit()
                except BaseException:
                    db.rollback()
                    raise
        finally:
            # T016 review (fixup): every non-retryable failure is recorded
            # through this one method, so it is the single place that must
            # stop holding a freshly-issued channel's plaintext secret --
            # there is no future retry under this idempotency_key to hand
            # it to (a 'failed' row is never resumed; delegate() raises
            # immediately on it -- see delegate()'s own status=='failed'
            # check). Popped even if the DB update above raises, so a
            # failed *write* of the failure itself still never leaks the
            # secret past this call.
            self._pending_channel_secrets.pop(idempotency_key, None)

    def _run_steps(self, idempotency_key: str, channel: ChannelContext, role_set) -> ProvisioningResult:
        while True:
            with self._store.connection() as db:
                row = _row(db, idempotency_key)
            if row is None:
                raise PortableError(
                    category=PortableErrorCategory.TRANSPORT,
                    code="provisioning_row_missing",
                    remediation="The provisioning row is unexpectedly missing; retry the identical delegate() call.",
                    retry_eligible=True,
                )
            if row["status"] == "ready":
                return self._result_from_row(row)
            if row["status"] == "failed":
                raise PortableError(
                    category=PortableErrorCategory.INDETERMINATE_WRITE,
                    code=row["error_code"] or "provisioning_failed",
                    remediation="Provisioning failed under this idempotency key; reconcile before retrying with a new request_key.",
                    retry_eligible=False,
                )
            step = _base_step(row["step"])
            if step == "workspace":
                self._step_workspace(idempotency_key, row)
            elif step == "channel":
                self._step_channel(idempotency_key, row, channel)
            elif step == "config_snapshot":
                self._step_config_snapshot(idempotency_key, row)
            elif step == "context_snapshot":
                self._step_context_snapshot(idempotency_key, row, role_set)
            elif step == "engine_session":
                self._step_engine_session(idempotency_key, row, channel, role_set)
            else:  # pragma: no cover - defensive; STEP_ORDER is closed
                raise PortableError(
                    category=PortableErrorCategory.INDETERMINATE_WRITE,
                    code="provisioning_unknown_step",
                    remediation="Unknown provisioning step recorded; this indicates data corruption. Do not retry blindly.",
                    retry_eligible=False,
                )

    def _step_workspace(self, idempotency_key: str, row) -> None:
        attempt_id = row["attempt_id"]
        if not self._acquire_step(idempotency_key, "workspace", "channel"):
            return
        with self._store.connection() as db:
            has_intent = _last_effect(db, idempotency_key, "workspace", "intent") is not None
            has_outcome = _last_effect(db, idempotency_key, "workspace", "outcome") is not None
        if has_intent and not has_outcome:
            state = self._workspace.lookup(attempt_id)
            if state == "present":
                binding_ref = attempt_id
            elif state == "absent":
                binding_ref = None
            else:
                self._fail(idempotency_key, "provisioning_indeterminate")
                raise PortableError(
                    category=PortableErrorCategory.INDETERMINATE_WRITE,
                    code="provisioning_indeterminate",
                    remediation="Workspace existence for this attempt could not be determined; investigate the workspace provisioner out of band before starting a new delegation.",
                    retry_eligible=False,
                )
        else:
            binding_ref = None
        if binding_ref is None:
            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                _record_effect(db, idempotency_key, "workspace", "intent", dict(attempt_id=attempt_id))
                db.commit()
            binding_ref = self._workspace.create(attempt_id)
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                _record_effect(db, idempotency_key, "workspace", "outcome", dict(workspace_binding_id=binding_ref))
                db.execute(
                    "UPDATE delegation_provisioning SET workspace_binding_id=?, step='channel', updated_at=? WHERE idempotency_key=?",
                    (binding_ref, _now(), idempotency_key),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise

    def _step_channel(self, idempotency_key: str, row, coordinator_channel: ChannelContext) -> None:
        from ..trusted_channel import issue_channel_tx

        attempt_id = row["attempt_id"]
        if not self._acquire_step(idempotency_key, "channel", "config_snapshot"):
            return
        with self._store.connection() as db:
            existing = db.execute(
                "SELECT channel_id FROM trusted_channels WHERE attempt_id=? AND revoked_at IS NULL",
                (attempt_id,),
            ).fetchone()
        channel_id = existing["channel_id"] if existing is not None else None
        if channel_id is None:
            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                try:
                    _record_effect(db, idempotency_key, "channel", "intent", dict(attempt_id=attempt_id))
                    channel_id, secret = issue_channel_tx(
                        db, row["task_id"], "worker", row["owner_epoch"],
                        parent_channel_id=coordinator_channel.channel_id,
                        assignment_id=row["assignment_id"], attempt_id=attempt_id,
                        workspace_binding_id=row["workspace_binding_id"],
                    )
                    self._pending_channel_secrets[idempotency_key] = secret
                    _record_effect(db, idempotency_key, "channel", "outcome", dict(channel_id=channel_id))
                    db.execute(
                        "UPDATE delegation_provisioning SET channel_id=?, step='config_snapshot', updated_at=? WHERE idempotency_key=?",
                        (channel_id, _now(), idempotency_key),
                    )
                    db.commit()
                except BaseException:
                    db.rollback()
                    raise
        else:
            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                try:
                    db.execute(
                        "UPDATE delegation_provisioning SET channel_id=?, step='config_snapshot', updated_at=? WHERE idempotency_key=?",
                        (channel_id, _now(), idempotency_key),
                    )
                    db.commit()
                except BaseException:
                    db.rollback()
                    raise

    def _step_config_snapshot(self, idempotency_key: str, row) -> None:
        snapshot = self._config.snapshot(row["task_id"])
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                if self._claim(db, idempotency_key, "config_snapshot", "context_snapshot"):
                    db.execute(
                        "UPDATE delegation_provisioning SET config_revision=? WHERE idempotency_key=?",
                        (snapshot["config_revision"], idempotency_key),
                    )
                db.commit()
            except BaseException:
                db.rollback()
                raise

    def _step_context_snapshot(self, idempotency_key: str, row, role_set) -> None:
        ref = "cx_" + hashlib.sha256(
            f"{row['task_id']}|{row['assignment_id']}|{row['attempt_id']}|{role_set.role_id}|{role_set.sha256}".encode("utf-8")
        ).hexdigest()[:24]
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                if self._claim(db, idempotency_key, "context_snapshot", "engine_session"):
                    db.execute(
                        "UPDATE delegation_provisioning SET context_snapshot_ref=? WHERE idempotency_key=?",
                        (ref, idempotency_key),
                    )
                db.commit()
            except BaseException:
                db.rollback()
                raise

    def _step_engine_session(self, idempotency_key: str, row, coordinator_channel: ChannelContext, role_set) -> None:
        with self._store.connection() as db:
            current_epoch = db.execute(
                "SELECT epoch FROM task_owners WHERE task_id=?", (row["task_id"],)
            ).fetchone()
        current_epoch = current_epoch["epoch"] if current_epoch is not None else None
        if current_epoch != row["owner_epoch"]:
            self._fail(idempotency_key, "stale_ownership_epoch")
            raise PortableError(
                category=PortableErrorCategory.AUTHORIZATION,
                code="stale_ownership_epoch",
                remediation="Task ownership changed since this delegation started; the engine session step is refused. Start a new delegation under the current owner.",
                retry_eligible=False,
            )
        attempt_id = row["attempt_id"]
        if not self._acquire_step(idempotency_key, "engine_session", "ready"):
            return
        with self._store.connection() as db:
            has_intent = _last_effect(db, idempotency_key, "engine_session", "intent") is not None
            has_outcome = _last_effect(db, idempotency_key, "engine_session", "outcome") is not None
        session_ref = None
        if has_intent and not has_outcome:
            state = self._engine.lookup_owned(attempt_id)
            if state == "alive":
                session_ref = attempt_id
            elif state == "absent":
                session_ref = None
            else:
                # "unknown": a second live session cannot safely be risked.
                # Fail closed rather than retry (lead decision: "unknown
                # lookup -> fail with provisioning_indeterminate not retry").
                self._fail(idempotency_key, "provisioning_indeterminate")
                raise PortableError(
                    category=PortableErrorCategory.INDETERMINATE_WRITE,
                    code="provisioning_indeterminate",
                    remediation="Engine session existence for this attempt could not be determined; investigate/terminate it out of band before starting a new delegation.",
                    retry_eligible=False,
                )
        preview = self._config.snapshot(row["task_id"])
        plan = qualify_delivery(
            role_set, delivery_for(preview["engine"], preview["version"]), [],
            allow_unqualified_delivery=self._allow_unqualified_delivery,
        )
        delivery_label = plan.qualification.value
        if session_ref is None:
            # Peek (never pop yet): a worker engine session must never be
            # opened without its channel credentials (T016 review, HIGH).
            # The secret is only removed from memory after open() actually
            # succeeds, a few lines below -- so a raise from open() (this
            # attempt's crash/failure) leaves it available to a same-process
            # retry instead of silently discarding it and letting a retry
            # launch an uncredentialed worker.
            channel_secret = self._pending_channel_secrets.get(idempotency_key)
            if channel_secret is None:
                # This process no longer holds the plaintext secret (never
                # persisted -- see trusted_channel's "returned exactly once"
                # model): either this is a genuinely new process/instance
                # resuming a crashed delegation, or a same-process retry
                # after this same idempotency_key's secret was already
                # consumed by an earlier *successful* open() (which should
                # be unreachable here, since a successful open() also
                # advances status to 'ready' and this branch is only
                # reached for a still-not-ready row). Revoking the old
                # channel and issuing a fresh one for the same attempt is
                # not implemented; fail explicitly and non-retryably rather
                # than ever open a session with no credentials.
                self._fail(idempotency_key, "channel_secret_unavailable")
                raise PortableError(
                    category=PortableErrorCategory.INDETERMINATE_WRITE,
                    code="channel_secret_unavailable",
                    remediation="This process no longer holds this delegation's worker channel secret (channel revocation/reissue on retry is not implemented); start a new delegation with a new request_key instead of retrying this one.",
                    retry_eligible=False,
                )
            with self._store.connection() as db:
                db.execute("BEGIN IMMEDIATE")
                _record_effect(db, idempotency_key, "engine_session", "intent", dict(attempt_id=attempt_id))
                db.commit()
            launch = dict(
                task_id=row["task_id"], assignment_id=row["assignment_id"], attempt_id=attempt_id,
                channel_id=row["channel_id"], config_revision=row["config_revision"],
                context_snapshot_ref=row["context_snapshot_ref"],
                channel_secret=channel_secret, role_id=row["role_id"],
            )
            try:
                session_ref = self._engine.open(row["workspace_binding_id"], launch, role_set.body_portable)
            except Exception as error:
                # T017 (fresh review, MEDIUM/starvation): a queued open
                # that never reserves (cancelled, or provably
                # unsatisfiable/timed-out capacity -- see
                # delegation.live_adapters.QueuedOpenCancelled, identified
                # here only by its ``kind`` attribute so this module never
                # imports the live/adapter-specific exception type) must
                # never leave this idempotency_key's row stuck mid-claim
                # forever; fail it explicitly, non-retryably. Any *other*
                # open() exception (a genuine crash/transport failure) is
                # deliberately left exactly as before -- uncaught, so a
                # same-process retry under the identical request_key can
                # still recover via lookup_owned (see this method's
                # existing has_intent/has_outcome branch above and the
                # provisioning-recovery tests).
                kind = getattr(error, "kind", None)
                if kind not in ("capacity_unsatisfiable", "capacity_timeout", "attempt", "task", "shutdown"):
                    raise
                self._fail(idempotency_key, kind)
                raise PortableError(
                    category=PortableErrorCategory.TRANSPORT,
                    code=kind,
                    remediation="This delegation's engine session could not be opened (capacity or cancellation); start a new delegation with a new request_key if it is still needed.",
                    retry_eligible=False,
                ) from error
            # open() succeeded: the secret has now been handed to the
            # engine's own launch env; stop holding it in memory. (The
            # already-alive/lookup_owned branch above never populates or
            # needs this dict entry, so no pop is needed there; every
            # non-retryable failure path pops it via _fail() instead --
            # see that method's docstring.)
            self._pending_channel_secrets.pop(idempotency_key, None)
        with self._store.connection() as db:
            db.execute("BEGIN IMMEDIATE")
            try:
                _record_effect(db, idempotency_key, "engine_session", "outcome", dict(engine_session_ref=session_ref))
                db.execute(
                    "UPDATE delegation_provisioning SET engine_session_ref=?, instruction_delivery=?, status='ready', step='ready', updated_at=? WHERE idempotency_key=?",
                    (session_ref, delivery_label, _now(), idempotency_key),
                )
                db.commit()
            except BaseException:
                db.rollback()
                raise
