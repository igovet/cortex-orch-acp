"""Coordinator and worker prompts, skills and tool schemas name no model.

Model names, tiers and policy reach a model only through the live
``list_route_options`` result, derived from routing.json ``models`` and the
built-in defaults in :mod:`routing.model_policy`. This guard fails when a
built-in model or family name, or a seed catalog id, reappears in text a
coordinator or worker reads.
"""
from __future__ import annotations

import datetime
import json
import re
from pathlib import Path

import pytest

from cortex_runtime.catalog.model_seed import SEED_MODELS
from cortex_runtime.delegation import contracts as delegation_contracts
from cortex_runtime.delegation.mcp_tools import _OPERATION_DESCRIPTIONS
from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt
from cortex_runtime.orchestration.contracts import PLAN
from cortex_runtime.routing import model_policy
from cortex_runtime.connections.model import SUPPORTED_ENGINES

ROOT = Path(__file__).resolve().parents[1]
# These quote the owner's AGENTS.md live-qualification rule (the model a
# paid live check must use), not a routing preference.
ALLOWED = {
    "skills/orchestrator/references/live-qualification.md",
    "skills/orchestrator/references/verification-coverage.md",
}


def _names() -> set[str]:
    names = set(model_policy.BUILTIN_FAMILY_TIERS) | set(model_policy.BUILTIN_MODEL_TIERS)
    names |= set(model_policy.BUILTIN_LINE_TIERS) | set(model_policy.BUILTIN_FLAGSHIP_FAMILIES)
    names |= set(model_policy.BUILTIN_STRETCH_LINES) | set(model_policy.BUILTIN_AUTO_DENY_FAMILIES)
    names |= set(model_policy.BUILTIN_REQUEST_NAMES)
    names |= {stem for stems in model_policy.BUILTIN_REQUEST_NAMES.values() for stem in stems}
    names |= {model for models in SEED_MODELS.values() for model, _name in models}
    return names


NAMES = sorted(_names(), key=len, reverse=True)
_CYRILLIC = re.compile("[\u0430-\u044f\u0451]")
# A Latin name is a whole word; a Russian stem takes any ending.
PATTERN = re.compile(
    r"(?<![\w.-])(" + "|".join(re.escape(name) + ("" if _CYRILLIC.search(name) else r"(?![\w])")
                                 for name in NAMES) + ")",
    re.IGNORECASE)


def _hits(text: str) -> list[str]:
    return sorted({match.group(1) for match in PATTERN.finditer(text)})


def _skill_files():
    patterns = ("skills/orchestrator/**/*", "skills/worker-*/**/*", "agent-sources/**/*")
    for pattern in patterns:
        for path in sorted(ROOT.glob(pattern)):
            if path.is_file() and path.suffix in {".md", ".json", ".yaml", ".yml", ".toml", ".txt"}:
                yield path


def test_the_guard_knows_the_names_it_looks_for():
    for name in ("opus", "sonnet", "haiku", "fable", "astra", "luna", "sol", "terra", "gpt-5.5", "астр"):
        assert name in NAMES, name
    assert _hits("Recommend Opus for design and gpt-6-luna for tests; используй Астру.") == [
        "Opus", "gpt-6-luna", "Астр"]
    assert _hits("console solution consolidates; lunar; terraform; opuses") == []


def test_skills_and_profile_sources_name_no_model():
    checked = 0
    for path in _skill_files():
        relative = path.relative_to(ROOT).as_posix()
        if relative in ALLOWED:
            continue
        checked += 1
        assert _hits(path.read_text(encoding="utf-8")) == [], relative
    assert checked > 20


@pytest.mark.parametrize("role_kind", ["coordinator", "worker"])
def test_advertised_tool_schemas_name_no_model(role_kind):
    schemas = delegation_contracts.schemas_for(role_kind)
    assert _hits(json.dumps(schemas, ensure_ascii=False)) == []
    assert _hits(json.dumps(PLAN, ensure_ascii=False)) == []
    assert _hits(json.dumps(_OPERATION_DESCRIPTIONS, ensure_ascii=False)) == []


@pytest.mark.parametrize("role_kind", ["coordinator", "worker"])
def test_the_unified_system_prompt_names_no_model(tmp_path, role_kind):
    text = cortex_system_prompt(role_kind, str(tmp_path), today=datetime.date(2026, 9, 29), environ={})
    assert _hits(text) == []


_ENGINE_WORDS = tuple(sorted({*SUPPORTED_ENGINES, "codex", "claude", "opencode", "kimi", "glm"},
                             key=len, reverse=True))
_PURPOSE_WORDS = r"design|architecture|implementation|coding|analysis|investigation|review|testing|debugging|writing|planning|documentation|operations"
_PREFERENCE = re.compile(
    rf"\b(?:use|prefer|choose|recommend|route|assign)\s+(?:\w+\s+){{0,3}}"
    rf"(?:{'|'.join(re.escape(word) for word in _ENGINE_WORDS)})\b[^.!?\n]{{0,80}}\b(?:for|on)\s+"
    rf"(?:\w+\s+){{0,3}}(?:{_PURPOSE_WORDS})\b",
    re.IGNORECASE,
)


def test_prompts_name_no_engine_preference(tmp_path):
    assert _PREFERENCE.search("Use Claude for design work")
    assert not _PREFERENCE.search("Claude Code account requires a login")
    texts = [path.read_text(encoding="utf-8") for path in _skill_files()]
    texts += [cortex_system_prompt(role, str(tmp_path), today=datetime.date(2026, 9, 29), environ={})
              for role in ("coordinator", "worker")]
    texts += list(_OPERATION_DESCRIPTIONS.values())
    def descriptions(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key == "description" and isinstance(item, str):
                    yield item
                else:
                    yield from descriptions(item)
        elif isinstance(value, list):
            for item in value:
                yield from descriptions(item)
    texts += list(descriptions(delegation_contracts.schemas_for("coordinator")))
    texts += list(descriptions(delegation_contracts.schemas_for("worker")))
    texts += list(descriptions(PLAN))
    assert all(_PREFERENCE.search(value) is None for value in texts)
