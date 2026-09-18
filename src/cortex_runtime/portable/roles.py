"""Portable role instruction loading.

Loads a :class:`RoleInstructionSet` per role from the shipped canonical
per-role Markdown sources under ``skills/`` -- the same files the plugin
manifest declares and a host engine actually loads at runtime (see the
T001 baseline report's code-area map). ``agent-sources/*.md`` (the shared
per-role protocol templates) are hand-maintained parallel sources with
``{{placeholder}}`` sections that one adapter-specific generator renders,
byte-identically, into both ``skills/`` and a separate compact
adapter-specific profile format under ``agents/`` (not read by this
module); this module reads the already-rendered ``skills/`` copy, the
actually-loaded artifact.

Native-only (single-adapter-only) passages are marked in the source
Markdown with a ``<!--cortex:native ADAPTER-->`` ... ``<!--/cortex:native-->``
comment pair naming the one adapter the passage applies to;
:attr:`RoleInstructionSet.body_portable` has those spans removed regardless
of which adapter they name. Non-negotiable rules restated from the
surrounding body are marked with ``<!--cortex:critical-->`` ...
``<!--/cortex:critical-->`` and their ``- `` bullet lines (or, for an
unbulleted marked span, the whole span) become
:attr:`RoleInstructionSet.critical_rules`; the marked block is additive
documentation, never new policy.
"""
from __future__ import annotations

from dataclasses import dataclass
import hashlib
from pathlib import Path
import re

from .errors import PortableError, PortableErrorCategory

# The repository root: this project *is* the agent, so there is no
# plugin container above it any more (the name was the old one).
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
SKILLS_ROOT = REPOSITORY_ROOT / "skills"

_NATIVE_BLOCK = re.compile(
    r"<!--\s*cortex:native\s+[a-z0-9_-]+\s*-->.*?<!--\s*/cortex:native\s*-->",
    re.S,
)
_CRITICAL_BLOCK = re.compile(
    r"<!--\s*cortex:critical\s*-->(?P<body>.*?)<!--\s*/cortex:critical\s*-->",
    re.S,
)

COORDINATOR_ROLE_IDS = frozenset({"orchestrator"})
SHARED_ROLE_IDS = frozenset({
    "cortex-control",
    "coordinator-communication",
    "content-safety",
    "tool-discipline",
    "knowledge-harvest",
    "documentation-sync",
    "context-compaction",
})

# Agent-profile short names (agents/*.toml stems, one per T001 specialist)
# accepted as aliases for their skill id ("worker-" + stem). Hand-maintained;
# extend only when a specialist is added to both agents/*.toml and
# skills/worker-*.
WORKER_ALIASES: dict[str, str] = {
    "accessibility-auditor": "worker-accessibility-auditor",
    "accessibility-fixer": "worker-accessibility-fixer",
    "architect": "worker-architect",
    "backend-dev": "worker-backend-dev",
    "build-verification": "worker-build-verification",
    "code-reviewer": "worker-code-reviewer",
    "database-architect": "worker-database-architect",
    "data-engineer": "worker-data-engineer",
    "debugger": "worker-debugger",
    "devops-engineer": "worker-devops-engineer",
    "explorer": "worker-explorer",
    "frontend-dev": "worker-frontend-dev",
    "fullstack-dev": "worker-fullstack-dev",
    "general": "worker-general",
    "mobile-dev": "worker-mobile-dev",
    "performance-engineer": "worker-performance-engineer",
    "planner": "worker-planner",
    "qa-engineer": "worker-qa-engineer",
    "refactorer": "worker-refactorer",
    "security-auditor": "worker-security-auditor",
    "senior-consultant": "worker-senior-consultant",
    "technical-writer": "worker-technical-writer",
    "ux-designer": "worker-ux-designer",
}


# worker-protocol.md's own text budget is within a few characters of the
# package's per-profile compact-source ceiling (see tests/test_package.py's
# ``< 12_500`` check applied per agents/*.toml, shared by all 22 rendered
# worker profiles): an in-place ``<!-- cortex:critical -->`` marker large
# enough to carry these rules as a distinct RoleInstructionSet.critical_rules
# list, for all 22 files, does not fit that shared budget. These rules are
# instead restated here verbatim from worker-protocol.md's existing,
# unmarked "Assignment contract" section (not new policy) and applied to
# every worker role uniformly; see docs/acp/roles-and-delegation.md.
WORKER_CRITICAL_RULES: tuple[str, ...] = (
    "Keep the assigned outcome, constraints, checks, revision, evidence and owned "
    "resources mandatory. Ask about missing/invalid conditions; do not infer scope, "
    "scan unrelated records, or finish early.",
    "You may investigate, implement, verify, and document assigned non-code "
    "artifacts. Preserve other work; do not mutate another worker's surface or use "
    "its resources. Report overlap first.",
)


@dataclass(frozen=True)
class RoleInstructionSet:
    """One role's portable instruction body, loaded from its canonical source."""

    role_id: str
    kind: str  # "worker" | "coordinator" | "shared"
    title: str
    body_portable: str
    critical_rules: tuple[str, ...]
    source_path: str
    sha256: str


def _role_kind(role_id: str) -> str:
    if role_id in COORDINATOR_ROLE_IDS:
        return "coordinator"
    if role_id in SHARED_ROLE_IDS:
        return "shared"
    if role_id.startswith("worker-"):
        return "worker"
    raise PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="unknown_role",
        remediation=(
            "'"
            + role_id
            + "' is not one of the known role ids; call known_role_ids() for "
            "the current list."
        ),
        retry_eligible=False,
        field="role_id",
    )


def known_role_ids() -> tuple[str, ...]:
    """Every loadable role id: 23 workers + orchestrator + 7 shared skills."""
    worker_ids = tuple(sorted(
        path.name for path in SKILLS_ROOT.glob("worker-*") if (path / "SKILL.md").is_file()
    ))
    return tuple(sorted(worker_ids + tuple(COORDINATOR_ROLE_IDS) + tuple(SHARED_ROLE_IDS)))


def _normalize_role_hint(hint: str) -> str:
    """Separator/case normalization only -- ``backend_dev``, ``Backend-Dev``
    and ``backend-dev`` all fold to the same lookup key. Never fuzzy/prefix
    matching: a hint that still does not name a known id or alias after this
    fold is genuinely unknown, not guessed at."""
    return hint.strip().lower().replace("_", "-")


def resolve_role(hint: object) -> str:
    """Resolve a role hint to a known role id.

    An exact role id or an exact entry of :data:`WORKER_ALIASES` is
    accepted first (no lookup performed); otherwise the hint is folded via
    :func:`_normalize_role_hint` (separators/case only) and looked up
    again -- so ``backend_dev``, ``Backend_Dev`` and ``worker-backend-dev``
    all resolve to the same role, matching what the tool schema itself
    advertises. No prefix/fuzzy matching. An unknown hint raises
    ``PortableError(code="unknown_role")`` naming every valid id so the
    caller can pick a concrete correction instead of guessing.
    """
    if not isinstance(hint, str) or not hint:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="unknown_role",
            remediation=(
                "'role' must be a non-empty string matching one of: "
                + ", ".join(known_role_ids())
            ),
            retry_eligible=False,
            field="role",
        )
    ids = known_role_ids()
    if hint in ids:
        return hint
    if hint in WORKER_ALIASES and WORKER_ALIASES[hint] in ids:
        return WORKER_ALIASES[hint]
    normalized = _normalize_role_hint(hint)
    if normalized != hint:
        if normalized in ids:
            return normalized
        if normalized in WORKER_ALIASES and WORKER_ALIASES[normalized] in ids:
            return WORKER_ALIASES[normalized]
    raise PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="unknown_role",
        remediation=(
            "'"
            + hint
            + "' is not a known role id or alias. Valid role ids: "
            + ", ".join(ids)
        ),
        retry_eligible=False,
        field="role",
    )


def _title(text: str, fallback: str) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("# "):
            return stripped[2:].strip()
    return fallback


def _extract_critical_rules(text: str) -> tuple[str, ...]:
    """Extract non-negotiable rules from every marked ``cortex:critical`` block.

    A block using ``- `` bullets yields one rule per bullet (continuation
    lines without the prefix are folded into the preceding bullet). A block
    with no bullets -- typically an *existing* sentence/paragraph marked in
    place rather than duplicated as a new bullet, to keep the source
    compact -- yields exactly one rule: its non-heading lines joined.
    """
    rules: list[str] = []
    for match in _CRITICAL_BLOCK.finditer(text):
        bullets: list[str] = []
        plain: list[str] = []
        current: list[str] | None = None
        for line in match.group("body").splitlines():
            stripped = line.strip()
            if stripped.startswith("- "):
                if current is not None:
                    bullets.append(" ".join(current))
                current = [stripped[2:].strip()]
            elif stripped.startswith("#") or not stripped:
                continue
            elif current is not None:
                current.append(stripped)
            else:
                plain.append(stripped)
        if current is not None:
            bullets.append(" ".join(current))
        rules.extend(bullets)
        if not bullets and plain:
            rules.append(" ".join(plain))
    return tuple(rules)


def load_role(role_hint: object) -> RoleInstructionSet:
    """Load one role's portable instruction set from its canonical SKILL.md."""
    role_id = resolve_role(role_hint)
    kind = _role_kind(role_id)
    source_path = SKILLS_ROOT / role_id / "SKILL.md"
    try:
        raw = source_path.read_bytes()
    except OSError as error:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="role_source_unavailable",
            remediation=(
                f"Canonical source for role '{role_id}' could not be read at "
                f"{source_path}: {error}. Verify the Cortex package is complete."
            ),
            retry_eligible=False,
            field="role",
        ) from error
    text = raw.decode("utf-8")
    critical_rules = _extract_critical_rules(text)
    if kind == "worker" and not critical_rules:
        critical_rules = WORKER_CRITICAL_RULES
    body_portable = _NATIVE_BLOCK.sub("", text)
    return RoleInstructionSet(
        role_id=role_id,
        kind=kind,
        title=_title(text, role_id),
        body_portable=body_portable,
        critical_rules=critical_rules,
        source_path=str(source_path),
        sha256=hashlib.sha256(raw).hexdigest(),
    )


# Shared skills that are not situational: they describe how to behave at
# all times, so waiting for a model to decide to load them means they do
# not apply until it does. Delivered with the role itself.
#
# The rest of SHARED_ROLE_IDS is situational (documentation-sync when
# documentation is actually affected, knowledge-harvest when a harvest is
# explicitly asked for, context-compaction on a compaction, cortex-control
# for retention) and is read on demand instead -- knowledge-harvest alone
# is 18KB, which no turn should pay for unconditionally.
ALWAYS_ON_SKILLS: dict[str, tuple[str, ...]] = {
    "coordinator": ("tool-discipline", "coordinator-communication", "content-safety"),
    "worker": ("tool-discipline", "content-safety"),
    "shared": (),
}


def composed_instructions(role_set: RoleInstructionSet) -> str:
    """One role's instructions as an engine should receive them: its own
    body, then the always-on shared skills for its kind.

    Until this existed nothing delivered a shared skill at all. The role
    text says things like "load/apply `cortex:documentation-sync`" -- an
    instruction from when a plugin host could load a skill by name. In an
    ACP session there is no such host: this runtime *is* the agent, and an
    engine only ever receives the text handed to it. So the rules that
    always apply are handed over with the role, and the situational ones
    are readable by name through this runtime's own tools.

    A shared skill that cannot be read is skipped rather than failing the
    session: a worker without ``content-safety`` is worse off, but a
    worker that cannot start at all is worse still, and the omission is
    visible in what was delivered.
    """
    parts = [role_set.body_portable]
    for skill_id in ALWAYS_ON_SKILLS.get(role_set.kind, ()):
        try:
            parts.append(load_role(skill_id).body_portable)
        except PortableError:
            continue
    return "\n\n".join(part.strip() for part in parts if part and part.strip()) + "\n"


def load_all_roles() -> dict[str, RoleInstructionSet]:
    """Load every known role; used by the specialization inventory check."""
    return {role_id: load_role(role_id) for role_id in known_role_ids()}
