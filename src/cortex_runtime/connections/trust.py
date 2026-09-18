"""User trust for endpoint/launch-command/provider overrides.

A project (or any project-controlled data) must never silently change a
connection's trusted endpoint, launch command, or provider label.
``fingerprint`` is a deterministic content hash of exactly those fields;
:func:`resolve_project_route` rejects any declared route whose fingerprint
does not match the connection's ``approved_fingerprint`` unless the user has
explicitly re-approved it via :func:`approve_override`, which records an
auditable history entry (previous fingerprint + timestamp) rather than
silently overwriting it.
"""
from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import json

from ..portable.errors import PortableError, PortableErrorCategory
from . import registry
from .model import ConnectionRecord


def fingerprint(engine: str, provider: str, endpoint: str | None, launch_command: tuple[str, ...] | None) -> str:
    canonical = json.dumps(
        {
            "engine": engine,
            "provider": provider,
            "endpoint": endpoint,
            "launch_command": list(launch_command) if launch_command is not None else None,
        },
        sort_keys=True,
        ensure_ascii=False,
        separators=(",", ":"),
    )
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def _authorization_error(code: str, remediation: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.AUTHORIZATION,
        code=code,
        remediation=remediation,
        retry_eligible=False,
    )


def resolve_project_route(
    connection_id: str,
    declared_endpoint: str | None,
    declared_command: tuple[str, ...] | None,
    declared_provider: str,
    *,
    environ: dict[str, str] | None = None,
) -> ConnectionRecord:
    """Return the connection iff project-declared route fields are trusted.

    Rejects any mismatch between the project-declared endpoint, launch
    command, or provider and the connection's own currently approved
    values -- a project cannot silently redirect a connection to a
    different endpoint, command, or provider label.
    """
    state = registry.read_registry(environ)
    record = state.connections.get(connection_id)
    if record is None:
        raise _authorization_error("connection_unavailable", "Unknown connection id; a project cannot reference a connection that was never registered.")
    mismatched = []
    if declared_endpoint != record.endpoint:
        mismatched.append("endpoint")
    if declared_command != record.launch_command:
        mismatched.append("launch_command")
    if declared_provider != record.provider:
        mismatched.append("provider")
    if mismatched:
        raise _authorization_error(
            "untrusted_project_override",
            "Project-controlled data declared a different "
            f"{', '.join(mismatched)} than this connection's approved values; "
            "an explicit user approval (trust.approve_override) is required "
            "before a project may change a trusted endpoint, launch command, "
            "or provider.",
        )
    declared_fingerprint = fingerprint(record.engine, declared_provider, declared_endpoint, declared_command)
    if declared_fingerprint != record.approved_fingerprint:
        raise _authorization_error(
            "untrusted_project_override",
            "The declared route no longer matches this connection's approved "
            "fingerprint; re-approve it explicitly (trust.approve_override) "
            "before using it.",
        )
    return record


def approve_override(
    connection_id: str,
    endpoint: str | None,
    launch_command: tuple[str, ...] | None,
    provider: str,
    *,
    approved_by_user: bool,
    environ: dict[str, str] | None = None,
) -> ConnectionRecord:
    """Explicitly re-approve a new endpoint/launch_command/provider.

    Requires ``approved_by_user=True`` (no default) so a caller cannot
    silently authorize an override on a user's behalf; records the previous
    fingerprint and an auditable timestamped history entry in the registry.
    """
    if approved_by_user is not True:
        raise _authorization_error(
            "untrusted_project_override",
            "approve_override requires an explicit approved_by_user=True; "
            "a project or automated caller cannot self-approve a trust change.",
        )
    with registry.transaction(environ) as state:
        record = state.connections.get(connection_id)
        if record is None:
            raise _authorization_error("connection_unavailable", "Unknown connection id.")
        new_fingerprint = fingerprint(record.engine, provider, endpoint, launch_command)
        history_entry = {
            "fingerprint": new_fingerprint,
            "previous_fingerprint": record.approved_fingerprint,
            "engine": record.engine,
            "provider": provider,
            "endpoint": endpoint,
            "launch_command": list(launch_command) if launch_command is not None else None,
            "approved_at": datetime.now(timezone.utc).isoformat(),
            "approved_by_user": True,
        }
        state.override_history.setdefault(connection_id, []).append(history_entry)
        updated = ConnectionRecord(
            id=record.id, name=record.name, engine=record.engine, provider=provider,
            auth_mode=record.auth_mode, key_kind=record.key_kind, endpoint=endpoint,
            launch_command=launch_command, credential_ref=record.credential_ref,
            approved_fingerprint=new_fingerprint, state=record.state, qualified=record.qualified,
            created_at=record.created_at, updated_at=history_entry["approved_at"],
        )
        state.connections[connection_id] = updated
    return updated


def override_history(connection_id: str, *, environ: dict[str, str] | None = None) -> tuple[dict[str, object], ...]:
    state = registry.read_registry(environ)
    return tuple(state.override_history.get(connection_id, ()))
