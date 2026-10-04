"""T015: portable canonical role sources are free of native-only tokens.

Scope: ``agent-sources/**`` and the shared/coordinator skill directories
(``orchestrator``, ``cortex-control``, ``coordinator-communication``,
``content-safety``, ``tool-discipline``, ``knowledge-harvest``,
``documentation-sync``, ``context-compaction``, ``skill-management``), scanned recursively --
including ``references/*.md``. Denylisted tokens are allowed only inside a
``<!-- cortex:native ... --> ... <!-- /cortex:native -->`` marker pair; a
scan that strips those spans and still finds a denylisted token fails.
"""
from __future__ import annotations

from pathlib import Path
import re

import pytest

from cortex_runtime.portable import roles

PLUGIN_ROOT = Path(__file__).resolve().parents[1]

DENYLIST = (
    "codex_app", "mcp__codex", "apply_patch", "spawn_agent",
    "CODEX_HOME", "~/.codex", ".codex-plugin", ".codex/cortex",
)

_NATIVE_BLOCK = re.compile(
    r"<!--\s*cortex:native[^>]*-->.*?<!--\s*/cortex:native\s*-->", re.S
)

_SHARED_SKILL_DIRS = (
    "orchestrator", "cortex-control", "coordinator-communication",
    "content-safety", "tool-discipline", "knowledge-harvest",
    "documentation-sync", "context-compaction", "skill-management",
)


def _scanned_files():
    files = list((PLUGIN_ROOT / "agent-sources").rglob("*.md"))
    for name in _SHARED_SKILL_DIRS:
        files += list((PLUGIN_ROOT / "skills" / name).rglob("*.md"))
    return sorted(set(files))


@pytest.mark.parametrize("path", _scanned_files(), ids=lambda p: str(p.relative_to(PLUGIN_ROOT)))
def test_no_denylisted_native_token_outside_markers(path):
    text = path.read_text(encoding="utf-8")
    stripped = _NATIVE_BLOCK.sub("", text)
    hits = [token for token in DENYLIST if token in stripped]
    assert not hits, f"{path}: denylisted native token(s) {hits} outside <!-- cortex:native --> markers"


def test_scanned_file_set_is_nonempty():
    assert len(_scanned_files()) >= 9


def test_all_t001_specializations_load():
    """23 workers + orchestrator + 8 shared skills all load as portable roles."""
    all_roles = roles.load_all_roles()
    worker_roles = [r for r in all_roles.values() if r.kind == "worker"]
    coordinator_roles = [r for r in all_roles.values() if r.kind == "coordinator"]
    shared_roles = [r for r in all_roles.values() if r.kind == "shared"]
    assert len(worker_roles) == 23
    assert len(coordinator_roles) == 1
    assert len(shared_roles) == 8
    for role in all_roles.values():
        assert role.body_portable.strip()
        assert role.sha256
        assert role.source_path


def test_conditional_skill_catalogue_is_engine_neutral_and_has_action_triggers():
    for kind in ("coordinator", "worker"):
        role_id = "orchestrator" if kind == "coordinator" else "worker-backend-dev"
        delivered = roles.composed_instructions(roles.load_role(role_id))
        assert "call Cortex MCP `read_skill` with that id before the action" in delivered
        for skill_id, trigger in roles.situational_skill_triggers(kind, role_id):
            assert trigger.startswith(("before ", "you ", "the "))
            assert f"`{skill_id}` -- {trigger}" in delivered
        assert "apply_patch" not in delivered


def test_worker_role_bodies_are_free_of_denylisted_tokens():
    """Every worker/coordinator role's *portable* body (after stripping native
    marker spans) is free of the same denylist, even though the required
    scan above only targets agent-sources/** + shared skills directly."""
    all_roles = roles.load_all_roles()
    for role in all_roles.values():
        hits = [token for token in DENYLIST if token in role.body_portable]
        assert not hits, f"{role.role_id}: {hits} leaked into body_portable"


def _normalize_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


# Any module-level tuple[str, ...] constant in portable.roles whose name ends
# in "_CRITICAL_RULES" is a hardcoded critical-rules list that bypasses the
# normal <!--cortex:critical--> marker extraction (see roles.py's module
# docstring/WORKER_CRITICAL_RULES comment for why). Each entry here must stay
# traceable to its declared canonical source: this dict is the single place a
# maintainer registers a new hardcoded constant's source, so an unregistered
# one fails loudly (KeyError-shaped assertion below) instead of silently
# escaping the drift check.
_HARDCODED_CRITICAL_RULES_SOURCES = {
    "WORKER_CRITICAL_RULES": PLUGIN_ROOT / "agent-sources" / "worker-protocol.md",
}


def _hardcoded_critical_rules_constants():
    return {
        name: value
        for name, value in vars(roles).items()
        if name.endswith("_CRITICAL_RULES") and isinstance(value, tuple)
    }


def test_every_hardcoded_critical_rules_constant_is_registered_for_drift_checking():
    found = set(_hardcoded_critical_rules_constants())
    registered = set(_HARDCODED_CRITICAL_RULES_SOURCES)
    unregistered = found - registered
    assert not unregistered, (
        f"portable.roles defines hardcoded critical-rules constant(s) {sorted(unregistered)} "
        "with no registered source in this test's _HARDCODED_CRITICAL_RULES_SOURCES; "
        "add an entry (and a traceability assertion) before shipping a new one."
    )


def test_hardcoded_worker_critical_rules_are_traceable_to_worker_protocol_source():
    """Every WORKER_CRITICAL_RULES entry must be a normalized-whitespace
    substring of agent-sources/worker-protocol.md: these rules are restated,
    not new policy, and must stay in sync if that source is edited."""
    for name, rules in _hardcoded_critical_rules_constants().items():
        source_path = _HARDCODED_CRITICAL_RULES_SOURCES[name]
        source_text = _normalize_whitespace(source_path.read_text(encoding="utf-8"))
        for rule in rules:
            normalized_rule = _normalize_whitespace(rule)
            assert normalized_rule in source_text, (
                f"portable.roles.{name} entry {rule!r} is no longer a "
                f"(whitespace-normalized) substring of {source_path}. Either "
                f"{source_path.name} was edited without re-syncing {name}, or "
                f"{name} drifted from the restated wording it claims to be "
                "quoting. Re-read the current source section and update the "
                f"constant to match verbatim (do not silently widen the match)."
            )


def test_specializations_use_engine_neutral_search_and_edit_guidance():
    source = PLUGIN_ROOT / "agent-sources"
    explorer = (source / "explorer.md").read_text()
    assert "advertised pagination" in explorer
    for stale in ("max_output_tokens", "head -80", "per wrapper"):
        assert stale not in explorer
    general = (source / "general.md").read_text()
    frontend = (source / "frontend-dev.md").read_text()
    assert "advertised contract" in general and "advertised contract" in frontend
    assert "complete envelope" not in general
    assert "Keep checks zero-exit" not in general
    assert "one update" not in frontend


def test_all_profiles_preserve_sections_and_compact_size_budget():
    from generate_agent_profiles import SPECIALIZED, sections

    for source in (PLUGIN_ROOT / "agent-sources").glob("*.md"):
        if source.name not in {"worker-protocol.md", "consultant-protocol.md"}:
            assert set(sections(source.read_text())) == set(SPECIALIZED)
    profiles = list((PLUGIN_ROOT / "skills").glob("worker-*/SKILL.md"))
    assert len(profiles) == 23
    for path in profiles:
        text = path.read_text()
        prompt = text.split('---\n', 2)[2].lstrip().removesuffix('\n<!-- END OF COMPLETE CORTEX WORKER SKILL -->\n')
        assert prompt.strip()
        assert len(text) < 12_500, (path.name, len(text))
