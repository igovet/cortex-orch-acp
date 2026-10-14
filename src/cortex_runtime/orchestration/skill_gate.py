"""Runtime-observed situational-skill gate at worker ``result_submit``.

Naming a situational skill with its trigger was not enough: on 2026-09-28 a
Claude worker changed runtime behavior and edited the documentation that
described it without ever calling ``read_skill`` for ``documentation-sync``
(observed live), while Codex workers read such skills. Inlining
the skill is not a substitute; the model must load it when its case arises.

So the runtime checks at a boundary it owns for both engines. Codex edits
through its native ``apply_patch`` and Claude through native Edit/Write or
Cortex ``project_write``; the project tree is the shared observation boundary.
Successful gateway writes also return an early, non-error skill reminder,
derived only from this attempt's own gateway writes (no tree observation).
A shared-root worker's pre-attempt
tree manifest is retained as ``attempt_revisions`` phase
``worker_start:<attempt_id>`` (see ``delegation.provisioning``). At
``result_submit`` the runtime observes the tree again and derives the paths
changed since that attempt started. When they match a trigger in
``SKILL_GATES`` and this attempt has no retained successful ``read_skill``
of that skill, the result is refused with ``situational_skill_required``
before anything is recorded or published, so an identical retry succeeds
once the skill was read.

Limits, stated rather than hidden:

* The tree delta is shared, but a refusal uses only this node's declared
  write reservation or its own successful gateway writes. Paths reserved by
  sibling nodes are excluded unless this attempt wrote them through the gateway.
  The early non-error hint sees only this attempt's gateway writes, so a
  native edit counts first at ``result_submit``.
* Without a complete pre-attempt manifest (isolated workspaces, a capture
  that was unavailable) nothing can be attributed; the gate does not fire
  and records a ``skill_gate``/``unobservable`` observability event instead
  of treating the absent signal as a pass.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Callable, Iterable

from ..contracts import StoreError

#: Observability category of a successful ``read_skill`` by a worker attempt.
SKILL_READ_CATEGORY = "skill_read"
#: Observability category of a gate that could not observe the attempt's changes.
SKILL_GATE_CATEGORY = "skill_gate"
ERROR_CODE = "situational_skill_required"

_DOC_BASENAMES = frozenset({"README.md", "SECURITY.md"})
#: Text formats that document rather than implement behavior.
_PROSE_SUFFIXES = (".md", ".mdx", ".markdown", ".rst", ".txt", ".adoc")
_EXAMPLE_LIMIT = 3


def is_documentation_path(path: str) -> bool:
    """``*.md`` in a ``docs`` directory, or a README.md / SECURITY.md."""
    parts = path.split("/")
    if parts[-1] in _DOC_BASENAMES:
        return True
    return path.endswith(".md") and "docs" in parts[:-1]


def is_behavior_path(path: str) -> bool:
    """A changed file that can change behavior: anything but prose,
    bytecode caches and verified engine housekeeping."""
    from ..delegation.revision import _is_benign_generated, _is_engine_volatile_path

    if _is_benign_generated(path) or _is_engine_volatile_path(path):
        return False
    return not path.lower().endswith(_PROSE_SUFFIXES)


def _docs_with_behavior(changed: set[str]) -> tuple[list[str], list[str]] | None:
    docs = sorted(path for path in changed if is_documentation_path(path))
    behavior = sorted(path for path in changed if is_behavior_path(path))
    return (docs, behavior) if docs and behavior else None


@dataclass(frozen=True)
class SkillGate:
    skill_id: str
    #: Plain statement of the observed condition, shown to the model.
    condition: str
    #: Changed paths -> (documentation paths, behavior paths) when it fires.
    match: Callable[[set[str]], tuple[list[str], list[str]] | None]


#: The one runtime table of observable situational-skill triggers. A skill
#: listed here is required only for a role whose situational catalogue
#: (``portable.roles.situational_skill_triggers``) names it.
SKILL_GATES: tuple[SkillGate, ...] = (
    SkillGate(
        skill_id="documentation-sync",
        condition="documentation (*.md under docs/, README.md or SECURITY.md) changed together with source "
                  "or other behavior files",
        match=_docs_with_behavior,
    ),
)


def gate_summary() -> str:
    """One sentence for the advertised tool descriptions."""
    return "; ".join(f"{gate.condition} requires {gate.skill_id}" for gate in SKILL_GATES)


def record_skill_read(store, channel, skill_id: str) -> None:
    """Retain one successful worker ``read_skill`` for this exact attempt."""
    if channel is None or channel.role != "worker" or not channel.attempt_id:
        return
    from ..observability.events import append_event

    append_event(store, channel.task_id, SKILL_READ_CATEGORY, "success", "observed",
                 attempt_id=channel.attempt_id, detail=dict(skill=skill_id))


def skills_read(store, task_id: str, attempt_id: str) -> set[str]:
    with store.connection() as db:
        rows = db.execute(
            "SELECT detail_json FROM observability_events WHERE task_id=? AND attempt_id=? AND category=? "
            "AND outcome='success'", (task_id, attempt_id, SKILL_READ_CATEGORY),
        ).fetchall()
    read = set()
    for row in rows:
        try:
            skill = json.loads(row[0]).get("skill")
        except (TypeError, ValueError, AttributeError):
            continue
        if isinstance(skill, str):
            read.add(skill)
    return read


def observed_changed_paths(store, task_id: str, attempt_id: str) -> tuple[set[str] | None, str | None]:
    """Paths changed in the project since this attempt started, or
    ``(None, reason)`` when that cannot be observed completely."""
    from .. import task_workspace
    from ..delegation.revision import _manifest_map, capture_revision

    with store.connection() as db:
        base = db.execute(
            "SELECT kind, manifest_json FROM attempt_revisions WHERE attempt_id=? AND phase=? "
            "ORDER BY captured_at LIMIT 1", (attempt_id, f"worker_start:{attempt_id}"),
        ).fetchone()
        if base is None:
            return None, "no_start_revision"
        try:
            root = task_workspace.task_root(db, task_id)
        except StoreError:
            return None, "no_task_root"
    try:
        manifest = json.loads(base["manifest_json"]) if base["manifest_json"] else None
    except ValueError:
        manifest = None
    before = _manifest_map(manifest) if base["kind"] == "observed" else None
    if before is None:
        return None, "start_revision_unavailable"
    current = capture_revision(str(root))
    after = _manifest_map(current.get("manifest")) if current.get("kind") == "observed" else None
    if after is None:
        return None, current.get("detail") or "current_revision_unavailable"
    # A directory entry only appears or disappears; the files in it are
    # entries of their own, so a directory is never a changed file.
    return {path for path in set(before) | set(after) if before.get(path) != after.get(path)
            and not all(entry.startswith("d\t") for entry in (before.get(path), after.get(path)) if entry)}, None


def _examples(paths: Iterable[str]) -> str:
    paths = list(paths)
    shown = ", ".join(paths[:_EXAMPLE_LIMIT])
    return shown + (f" (+{len(paths) - _EXAMPLE_LIMIT} more)" if len(paths) > _EXAMPLE_LIMIT else "")


def _own_gateway_writes(db, channel) -> set[str]:
    """Paths this attempt changed through completed gateway writes."""
    writes = set()
    for row in db.execute(
        "SELECT e.payload_json FROM runtime_events e JOIN trusted_channels c ON c.channel_id=e.actor "
        "WHERE e.task_id=? AND c.attempt_id=? AND e.type='TOOL_COMPLETED'",
        (channel.task_id, channel.attempt_id),
    ):
        payload = json.loads(row["payload_json"])
        if payload.get("operation") in {"project_write", "project_edit", "project_delete"} and isinstance(payload.get("path"), str):
            writes.add(payload["path"])
    return writes


def attributable_changed_paths(store, channel, changed: set[str], *, successful_paths: Iterable[str] = ()) -> set[str] | None:
    """Conservatively attribute a shared-tree delta; never charge sibling work.

    Declared scopes are coordination reservations, not write grants. Explicit
    successful channel writes therefore count even outside the reservation.
    Native edits in overlapping reservations cannot be attributed safely.
    """
    from .gateway import _matches
    from .contracts import READ_ONLY

    with store.connection() as db:
        own = db.execute(
            "SELECT n.scope_json,n.revision FROM runtime_nodes n JOIN delegation_provisioning p "
            "ON n.assignment_ref=p.idempotency_key AND n.task_id=p.task_id "
            "WHERE p.task_id=? AND p.attempt_id=?",
            (channel.task_id, channel.attempt_id),
        ).fetchone()
        if own is None:
            return None
        scopes = json.loads(own["scope_json"])
        other_scopes = [scope for row in db.execute(
            "SELECT scope_json,role,stage FROM runtime_nodes WHERE task_id=? AND revision=? AND assignment_ref != "
            "(SELECT idempotency_key FROM delegation_provisioning WHERE task_id=? AND attempt_id=?)",
            (channel.task_id, own["revision"], channel.task_id, channel.attempt_id),
        ) if row["role"] not in READ_ONLY and row["stage"] != "review"
            for scope in json.loads(row["scope_json"])]
        # The advisory caller may name a write that has committed but whose
        # TOOL_COMPLETED event is not published yet. These paths are runtime-owned.
        writes = set(successful_paths) | _own_gateway_writes(db, channel)
        other_writes = set()
        for row in db.execute(
            "SELECT e.payload_json FROM runtime_events e JOIN trusted_channels c ON c.channel_id=e.actor "
            "JOIN delegation_provisioning p ON p.attempt_id=c.attempt_id AND p.task_id=e.task_id "
            "JOIN runtime_nodes n ON n.assignment_ref=p.idempotency_key AND n.task_id=p.task_id "
            "WHERE e.task_id=? AND n.revision=? AND c.attempt_id!=? AND e.type='TOOL_COMPLETED'",
            (channel.task_id, own["revision"], channel.attempt_id),
        ):
            payload = json.loads(row["payload_json"])
            if payload.get("operation") in {"project_write", "project_edit", "project_delete"} and isinstance(payload.get("path"), str):
                other_writes.add(payload["path"])
    return {path for path in changed if path in writes or
            (_matches(path, scopes) and not _matches(path, other_scopes) and path not in other_writes)}


def required_before_result_submit(store, channel, *, stage: str | None,
                                  role_ids: Iterable[str | None], advisory: bool = False,
                                  successful_paths: Iterable[str] = ()) -> list[dict]:
    """Observe unread triggered skills for a gateway hint or the result gate."""
    from ..portable.roles import situational_skill_triggers
    from .contracts import READ_ONLY

    if channel is None or channel.role != "worker" or not channel.attempt_id or stage == "review":
        return []
    role_ids = [role_id for role_id in role_ids if role_id]
    if any(role_id in READ_ONLY for role_id in role_ids):
        return []
    # A role excluded from a skill under any of its names is not gated on it.
    catalogues = [{skill_id for skill_id, _ in situational_skill_triggers("worker", role_id)}
                  for role_id in role_ids or [None]]
    named = set.intersection(*catalogues)
    pending = [gate for gate in SKILL_GATES if gate.skill_id in named]
    if not pending:
        return []
    read = skills_read(store, channel.task_id, channel.attempt_id)
    pending = [gate for gate in pending if gate.skill_id not in read]
    if not pending:
        return []
    if advisory:
        # A write receipt's reminder observes no tree (a full scan per write
        # cost up to 0.6 s on a large project): it names only this attempt's
        # own gateway writes, which need no attribution. Native edits still
        # count at result_submit, which observes the tree.
        with store.connection() as db:
            changed = _own_gateway_writes(db, channel) | set(successful_paths)
        return _requirements(pending, changed)
    changed, reason = observed_changed_paths(store, channel.task_id, channel.attempt_id)
    if changed is None:
        try:
            from ..observability.events import append_event

            append_event(store, channel.task_id, SKILL_GATE_CATEGORY, "unobservable", "observed",
                         attempt_id=channel.attempt_id,
                         detail=dict(reason=reason, skills=[gate.skill_id for gate in pending]))
        except Exception:
            pass
        return []
    # Attribution narrows ``changed`` to this attempt's own paths: the gate
    # must never charge a code-only worker for a sibling's unrelated docs/**
    # edit just because the shared-tree delta includes it (see this module's
    # own docstring on attribution limits).
    attributed = attributable_changed_paths(store, channel, changed, successful_paths=successful_paths)
    if attributed is None:
        from ..observability.events import append_event

        append_event(store, channel.task_id, SKILL_GATE_CATEGORY, "unobservable", "observed",
                     attempt_id=channel.attempt_id,
                     detail=dict(reason="no_declared_scope", skills=[gate.skill_id for gate in pending]))
        return []
    return _requirements(pending, attributed)


def _requirements(pending, changed: set[str]) -> list[dict]:
    requirements = []
    for gate in pending:
        matched = gate.match(changed)
        if matched is not None:
            docs, behavior = matched
            requirements.append(dict(skill=gate.skill_id,
                                     reason=f"{gate.condition}; documentation: {_examples(docs)}; "
                                            f"behavior: {_examples(behavior)}"))
    return requirements


def enforce(store, channel, *, stage: str | None, role_ids: Iterable[str | None]) -> None:
    """Refuse before publishing when an observed trigger requires an unread skill."""
    for requirement in required_before_result_submit(store, channel, stage=stage, role_ids=role_ids):
        skill = requirement['skill']
        raise StoreError(
            ERROR_CODE, "read_skill.skill", requirement['reason'],
            f"a successful read_skill(skill='{skill}') by this attempt before result_submit",
            f"This attempt changed {requirement['reason']} and has not read '{skill}'. Call the cortex MCP "
            f"read_skill tool with skill='{skill}', re-check your documentation and source edits against "
            f"its procedure (fix what it shows is wrong), then call result_submit again. This refusal recorded "
            f"nothing, so the same request_key may be reused.",
        )
