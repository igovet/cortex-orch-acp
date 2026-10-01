"""Coordinator and worker prompts, skills and tool schemas prescribe no model.

Known identities are read from the permitted public installation template. The
guard checks current instruction/profile sources, advertised schemas, and
unified prompts; the template is data, not runtime inference or fallback.
"""
from __future__ import annotations

import datetime
import json
import re
from pathlib import Path

import pytest

from cortex_runtime.delegation import contracts as delegation_contracts
from cortex_runtime.delegation.mcp_tools import _OPERATION_DESCRIPTIONS
from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt
from cortex_runtime.orchestration.contracts import PLAN
from cortex_runtime.connections.model import SUPPORTED_ENGINES

ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = ROOT / "policy" / "model-settings-default.json"


def _names() -> set[str]:
    data = json.loads(TEMPLATE.read_text(encoding="utf-8"))["routing"]
    names: set[str] = set()
    for values in (data.get("deny", []), data.get("auto_allow", [])):
        for value in values:
            names.add(value.rsplit("/", 1)[-1])
    for engine, models in data.get("models", {}).items():
        names.update(models.get("auto", []))
        names.update(models.get("tiers", {}).keys())
        names.update(models.get("policy", {}).keys())
        names.update(data.get("enabled", {}).get(engine, []))
        for policy in models.get("policy", {}).values():
            names.update(policy.get("request_names") or [])
    # Avoid generic values such as "default" becoming false-positive words.
    return {name for name in names if any(char.isdigit() for char in name) or
            name in {"haiku", "astra", "fable", "фейбл", "фэйбл", "астр"}}

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


def test_the_guard_reads_current_template_identities():
    assert "gpt-6-luna" in NAMES
    assert "claude-fable-5-1[1m]" in NAMES
    assert "астр" in NAMES
    assert _hits("Recommend gpt-6-luna for testing; use claude-fable-5-1[1m].") == [
        "claude-fable-5-1[1m]", "gpt-6-luna"]
    assert _hits("console solution consolidates; lunar; terraform") == []

def test_skills_and_profile_sources_name_no_model():
    checked = 0
    for path in _skill_files():
        relative = path.relative_to(ROOT).as_posix()
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


def test_quota_executable_literals_do_not_embed_model_families():
    import ast
    data = json.loads(TEMPLATE.read_text())["routing"]
    families = {r["window"].removeprefix("seven_day_")
                for r in data["quota_windows"]["claude-agent-acp"]}
    tree = ast.parse((ROOT / "src/cortex_runtime/connections/quota.py").read_text())
    docstrings = {id(node.value) for node in ast.walk(tree)
                  if isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant)
                  and isinstance(node.value.value, str)}
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str) and id(node) not in docstrings:
            assert not families.intersection(re.split(r"[^\w]+|_", node.value.casefold())), node.lineno
    assert not any(isinstance(node, ast.Name) and node.id == "_MODEL_SCOPED" for node in ast.walk(tree))


def test_backend_routing_remediation_literals_name_no_model():
    import ast
    # Unlike a whole-file grep, executable strings exclude comments/docstrings.
    # Include historical backend examples absent from the installation template.
    literals = re.compile(r"(?<![\w.-])(?:claude-(?:sonnet|opus|haiku)-\d[\w.\[\]-]*|gpt-\d[\w.\[\]-]*)", re.I)
    tree = ast.parse((ROOT / "src/cortex_runtime/configuration/management.py").read_text())
    function = next(node for node in tree.body if isinstance(node, ast.FunctionDef)
                    and node.name == "validate_routing_rules")
    docstring = function.body[0].value
    for node in ast.walk(function):
        if isinstance(node, ast.Constant) and isinstance(node.value, str) and node is not docstring:
            assert _hits(node.value) == [], node.lineno
            assert not literals.search(node.value), node.lineno


def test_routing_audit_literals_use_only_neutral_policy_names():
    import ast
    tree = ast.parse((ROOT / "src/cortex_runtime/routing/auto_route.py").read_text())
    # Catch emitted mapping keys, indexed assignments and effort-basis
    # prefixes, including formatted strings. Comments/docstrings are not output.
    docstrings = {id(node.body[0].value) for node in ast.walk(tree)
                  if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef))
                  and node.body and isinstance(node.body[0], ast.Expr)
                  and isinstance(node.body[0].value, ast.Constant)
                  and isinstance(node.body[0].value.value, str)}
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str) and id(node) not in docstrings:
            assert not re.search(r"(?<![\\w])luna(?![\\w])", node.value, re.I), node.lineno
        if isinstance(node, ast.keyword):
            assert node.arg != "luna", node.lineno
