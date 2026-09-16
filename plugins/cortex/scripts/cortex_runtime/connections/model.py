"""Connection value types: identity, auth mode, credential reference, state.

A :class:`ConnectionRecord` never carries a secret value. ``credential_ref``
is a closed union of three opaque references (:class:`EngineNativeDir`,
:class:`SecretFile`, :class:`OsKeyring`); the secret bytes themselves live
outside this package's in-memory model, in the engine's own native
configuration directory, a Cortex-managed secret file, or the OS keyring.

The ``SUPPORT_MATRIX`` below is the only source of truth for which
``(engine, auth_mode)`` combinations, and for ``api_key`` which
``key_kind`` values, are supported.  Nothing here claims those modes were
live-qualified: every entry is annotated with the evidence basis recorded
in ``.cortex/acp-migration/evidence/T002/static-capability-findings.md``
(DOC = release notes/README; STATIC = read in installed adapter code,
never executed), and ``gateway`` mode is always registered with
``qualified=False`` because no gateway launch path is implemented here.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
import re
from typing import Union

from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import Connection, _validate_prefixed_id


class AuthMode(str, Enum):
    """Closed set of supported, mutually non-interchangeable auth modes."""

    NATIVE_ACCOUNT = "native_account"
    API_KEY = "api_key"
    GATEWAY = "gateway"


class ConnectionState(str, Enum):
    """The only states a connection may report; never a silent substitution."""

    CONFIGURED = "configured"
    AUTHORIZATION_REQUIRED = "authorization_required"
    UNAVAILABLE = "unavailable"
    INVALID = "invalid"


# Each key kind is a distinct credential shape that maps to exactly one
# engine environment variable; kinds are never interchangeable and a
# connection may only use a key_kind its engine's SUPPORT_MATRIX entry
# lists for api_key auth.
#
# codex-acp: CODEX_API_KEY takes documented precedence over OPENAI_API_KEY
# (T002 evidence: "auth: ChatGPT login, API key (CODEX_API_KEY/OPENAI_API_KEY)
# ... (README) DOC"); this module only ever sets CODEX_API_KEY, never
# OPENAI_API_KEY, so no ambiguous precedence exists in a built environment.
KEY_KIND_ENV_VAR: dict[str, str] = {
    "openai_api_key": "CODEX_API_KEY",
    "anthropic_api_key": "ANTHROPIC_API_KEY",
    "claude_oauth_token": "CLAUDE_CODE_OAUTH_TOKEN",
}

SUPPORTED_ENGINES: tuple[str, ...] = ("codex-acp", "claude-agent-acp")


@dataclass(frozen=True)
class _AuthSupport:
    qualified: bool
    qualification: str
    key_kinds: tuple[str, ...] | None
    basis: str


# See module docstring: "qualified" only ever describes whether this module
# permits registering the mode at all (i.e. whether it is a documented or
# statically observed engine capability), never a live/paid qualification
# result. gateway is always qualified=False here because no gateway launch
# is implemented.
SUPPORT_MATRIX: dict[str, dict[AuthMode, _AuthSupport]] = {
    "codex-acp": {
        AuthMode.NATIVE_ACCOUNT: _AuthSupport(
            qualified=True,
            qualification="documented_unqualified",
            key_kinds=None,
            basis="DOC: ChatGPT login (T002 codex-acp auth line)",
        ),
        AuthMode.API_KEY: _AuthSupport(
            qualified=True,
            qualification="documented_unqualified",
            key_kinds=("openai_api_key",),
            basis="DOC: CODEX_API_KEY/OPENAI_API_KEY (T002 codex-acp auth line)",
        ),
        AuthMode.GATEWAY: _AuthSupport(
            qualified=False,
            qualification="documented_unqualified",
            key_kinds=None,
            basis="DOC: custom gateway (T002 codex-acp auth line); no launch implemented",
        ),
    },
    "claude-agent-acp": {
        AuthMode.NATIVE_ACCOUNT: _AuthSupport(
            qualified=True,
            qualification="static_unqualified",
            key_kinds=None,
            basis="STATIC: terminal login (T002 claude-agent-acp auth line, acp-agent.js:950)",
        ),
        AuthMode.API_KEY: _AuthSupport(
            qualified=True,
            qualification="static_unqualified",
            key_kinds=("anthropic_api_key", "claude_oauth_token"),
            basis=(
                "STATIC: ANTHROPIC_API_KEY, CLAUDE_CODE_OAUTH_TOKEN "
                "(T002 claude-agent-acp auth line, hide-claude-auth.js:62, "
                "acp-agent.js:7847-7849)"
            ),
        ),
        AuthMode.GATEWAY: _AuthSupport(
            qualified=False,
            qualification="static_unqualified",
            key_kinds=None,
            basis="STATIC: gateway, gateway-bedrock (T002 claude-agent-acp auth line, acp-agent.js:950); no launch implemented",
        ),
    },
}


def _unsupported(engine: str | None = None, auth_mode: str | None = None, key_kind: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="unsupported_auth_mode",
        remediation=(
            "Use one of the documented (engine, auth_mode[, key_kind]) "
            "combinations in connections.model.SUPPORT_MATRIX; native "
            "account, API key, and gateway modes are distinct and only "
            "enabled where the engine documents or statically exposes them."
        ),
        retry_eligible=False,
        field="auth_mode",
        details=f"engine={engine!r} auth_mode={auth_mode!r} key_kind={key_kind!r}",
    )


def support_for(engine: str, auth_mode: AuthMode) -> _AuthSupport:
    if engine not in SUPPORT_MATRIX:
        raise _unsupported(engine=engine)
    entry = SUPPORT_MATRIX[engine].get(auth_mode)
    if entry is None:
        raise _unsupported(engine=engine, auth_mode=auth_mode.value if isinstance(auth_mode, AuthMode) else auth_mode)
    return entry


def validate_auth_selection(engine: str, auth_mode: AuthMode, key_kind: str | None) -> _AuthSupport:
    """Fail closed on any unsupported (engine, auth_mode, key_kind) triple."""
    support = support_for(engine, auth_mode)
    if auth_mode == AuthMode.API_KEY:
        if key_kind is None or support.key_kinds is None or key_kind not in support.key_kinds:
            raise _unsupported(engine=engine, auth_mode=auth_mode.value, key_kind=key_kind)
    elif key_kind is not None:
        raise _unsupported(engine=engine, auth_mode=auth_mode.value, key_kind=key_kind)
    return support


def _require_absolute_path(value: object, field_name: str) -> str:
    text = _require_str(value, field_name)
    if not text.startswith("/"):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation=f"'{field_name}' must be an absolute path.",
            retry_eligible=False,
            field=field_name,
        )
    return text


# Keyring service/account identifiers are opaque labels, never filesystem
# paths: no "/", no "..", no leading dot, bounded length. This is enforced
# unconditionally here (not only by callers) so no code path -- including a
# hand-tampered connections.json deserialized back into an OsKeyring -- can
# smuggle a path-shaped value into what must stay a pure keyring lookup key.
_KEYRING_IDENTIFIER = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


def _require_keyring_identifier(value: object, field_name: str) -> str:
    text = _require_str(value, field_name)
    if "/" in text or ".." in text or not _KEYRING_IDENTIFIER.match(text):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation=f"'{field_name}' must be a plain identifier (letters/digits/'.'/'_'/'-' only, no path separators).",
            retry_eligible=False,
            field=field_name,
        )
    return text


@dataclass(frozen=True)
class EngineNativeDir:
    """Reference to an engine's own protected native config directory.

    ``path`` is never caller-supplied for a real connection: onboarding
    always derives it from ``registry.connection_state_dir(id, engine)``.
    """

    path: str
    kind: str = field(default="engine_native_dir", init=False)

    def __post_init__(self) -> None:
        _require_absolute_path(self.path, "path")


@dataclass(frozen=True)
class SecretFile:
    """Reference to a Cortex-managed, owner-only secret file (0600).

    ``path`` is never caller-supplied for a real connection: onboarding
    always derives it from ``registry.secret_path(id)``.
    """

    path: str
    kind: str = field(default="secret_file", init=False)

    def __post_init__(self) -> None:
        _require_absolute_path(self.path, "path")


@dataclass(frozen=True)
class OsKeyring:
    """Reference to an OS keyring entry; never a Cortex-held secret."""

    service: str
    account: str
    kind: str = field(default="os_keyring", init=False)

    def __post_init__(self) -> None:
        _require_keyring_identifier(self.service, "service")
        _require_keyring_identifier(self.account, "account")


CredentialRef = Union[EngineNativeDir, SecretFile, OsKeyring]

_CREDENTIAL_REF_KINDS = ("engine_native_dir", "secret_file", "os_keyring")


def serialize_credential_ref(ref: CredentialRef) -> dict[str, object]:
    if isinstance(ref, EngineNativeDir):
        return {"kind": ref.kind, "path": ref.path}
    if isinstance(ref, SecretFile):
        return {"kind": ref.kind, "path": ref.path}
    if isinstance(ref, OsKeyring):
        return {"kind": ref.kind, "service": ref.service, "account": ref.account}
    raise PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="invalid_field",
        remediation="credential_ref must be an EngineNativeDir, SecretFile, or OsKeyring instance.",
        retry_eligible=False,
        field="credential_ref",
    )


def deserialize_credential_ref(data: object) -> CredentialRef:
    if not isinstance(data, dict):
        raise _invalid_credential_ref()
    kind = data.get("kind")
    if kind == "engine_native_dir":
        path = data.get("path")
        if not isinstance(path, str) or not path or set(data) != {"kind", "path"}:
            raise _invalid_credential_ref()
        return EngineNativeDir(path=path)
    if kind == "secret_file":
        path = data.get("path")
        if not isinstance(path, str) or not path or set(data) != {"kind", "path"}:
            raise _invalid_credential_ref()
        return SecretFile(path=path)
    if kind == "os_keyring":
        service = data.get("service")
        account = data.get("account")
        if (
            not isinstance(service, str)
            or not service
            or not isinstance(account, str)
            or not account
            or set(data) != {"kind", "service", "account"}
        ):
            raise _invalid_credential_ref()
        return OsKeyring(service=service, account=account)
    raise _invalid_credential_ref()


def _invalid_credential_ref() -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="invalid_credential_ref",
        remediation=(
            f"'credential_ref.kind' must be one of {_CREDENTIAL_REF_KINDS} with "
            "exactly the fields that kind requires; unknown shapes are rejected "
            "rather than guessed at."
        ),
        retry_eligible=False,
        field="credential_ref",
    )


def _require_str(value: object, field_name: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation=f"'{field_name}' must be a non-empty string.",
            retry_eligible=False,
            field=field_name,
        )
    return value


@dataclass(frozen=True)
class ConnectionRecord:
    """Full, registry-persisted connection state. Never carries a secret."""

    id: str
    name: str
    engine: str
    provider: str
    auth_mode: AuthMode
    key_kind: str | None
    endpoint: str | None
    launch_command: tuple[str, ...] | None
    credential_ref: CredentialRef
    approved_fingerprint: str
    state: ConnectionState
    qualified: bool
    created_at: str
    updated_at: str

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "cn_")
        _require_str(self.name, "name")
        _require_str(self.provider, "provider")
        _require_str(self.approved_fingerprint, "approved_fingerprint")
        _require_str(self.created_at, "created_at")
        _require_str(self.updated_at, "updated_at")
        if not isinstance(self.auth_mode, AuthMode):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'auth_mode' must be an AuthMode member.",
                retry_eligible=False,
                field="auth_mode",
            )
        if not isinstance(self.state, ConnectionState):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'state' must be a ConnectionState member.",
                retry_eligible=False,
                field="state",
            )
        validate_auth_selection(self.engine, self.auth_mode, self.key_kind)
        if self.endpoint is not None:
            _require_str(self.endpoint, "endpoint")
        if self.launch_command is not None:
            if not isinstance(self.launch_command, tuple) or not all(isinstance(part, str) and part for part in self.launch_command):
                raise PortableError(
                    category=PortableErrorCategory.VALIDATION,
                    code="invalid_field",
                    remediation="'launch_command' must be a tuple of non-empty strings, or None.",
                    retry_eligible=False,
                    field="launch_command",
                )
        if not isinstance(self.credential_ref, (EngineNativeDir, SecretFile, OsKeyring)):
            raise _invalid_credential_ref()
        if not isinstance(self.qualified, bool):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'qualified' must be an explicit bool.",
                retry_eligible=False,
                field="qualified",
            )


def credential_ref_opaque_reference(ref: CredentialRef) -> str:
    """A non-secret opaque string identifying the ref shape, never its value."""
    if isinstance(ref, EngineNativeDir):
        return f"engine_native_dir:{ref.path}"
    if isinstance(ref, SecretFile):
        return f"secret_file:{ref.path}"
    if isinstance(ref, OsKeyring):
        return f"os_keyring:{ref.service}:{ref.account}"
    raise _invalid_credential_ref()


def to_portable_connection(record: ConnectionRecord) -> Connection:
    """Project a full record down to the portable, engine-agnostic view."""
    return Connection(
        id=record.id,
        engine=record.engine,
        provider=record.provider,
        endpoint=record.endpoint,
        auth_mode=record.auth_mode.value,
        credential_ref=credential_ref_opaque_reference(record.credential_ref),
    )
