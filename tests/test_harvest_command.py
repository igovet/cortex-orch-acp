"""`/harvest` makes an explicit request expressible.

The coordinator's role keeps a knowledge harvest explicit-only. Until this
command existed, "explicit" had nothing to express it: a model had to decide
from prose whether the user had really asked for one, which is exactly the
inference the rule exists to prevent. A command was pressed or it was not.
"""
from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "src"))

from cortex_runtime.acp import commands as acp_commands  # noqa: E402
from cortex_runtime.acp import messages as acp_messages  # noqa: E402


def test_the_command_is_named_what_the_user_types():
    names = [c["name"] for c in acp_commands.AVAILABLE_COMMANDS]
    assert "harvest" in names, names
    entry = next(c for c in acp_commands.AVAILABLE_COMMANDS if c["name"] == "harvest")
    assert entry["input"], "a harvest may be scoped, so the command takes optional text"
    assert "optional" in entry["input"]["hint"]


def test_it_is_recognized_by_exact_prefix_only():
    assert acp_commands.recognize([{"type": "text", "text": "/harvest"}]) == ("harvest", "")
    assert acp_commands.recognize([{"type": "text", "text": "/harvest auth module"}]) == ("harvest", "auth module")
    assert acp_commands.recognize([{"type": "text", "text": "/harvest-refresh"}]) == ("harvest-refresh", "")
    assert acp_commands.recognize([{"type": "text", "text": "/harvest-refresh auth module"}]) == ("harvest-refresh", "auth module")
    # Never inferred from prose -- which is the whole point of the command.
    assert acp_commands.recognize([{"type": "text", "text": "please harvest the knowledge"}]) is None
    assert acp_commands.recognize([{"type": "text", "text": "/harvesting"}]) is None


def test_the_request_names_itself_as_explicit_and_says_how_to_read_the_skill():
    for language in ("en", "ru"):
        text = acp_messages.render("harvest_requested", language, scope="")
        assert "knowledge-harvest" in text
        assert "read_skill" in text, "the skill is read by name; nothing else can load it"
        lowered = text.lower()
        assert ("explicit" in lowered) or ("явн" in lowered), text

    scoped = acp_messages.render("harvest_requested", "en", scope=" (auth module)")
    assert "(auth module)" in scoped


def test_harvest_request_does_not_require_a_prior_task_or_goal():
    for language in ("en", "ru"):
        text = acp_messages.render("harvest_requested", language, scope="")
        assert "prior" not in text.lower()
        assert "goal" not in text.lower()


def test_harvest_review_is_runtime_owned_and_census_has_one_body():
    from cortex_runtime.portable.roles import load_role

    text = load_role("knowledge-harvest").body_portable
    scope = load_role("knowledge-harvest-scope").body_portable
    assert "For harvests only" in scope
    assert "`technical-writer`" in scope
    assert "technical_writer" not in text
    assert "index-driven documentation routing" not in text
    assert "worker's source-backed review" not in text
    assert "runtime's mandatory review checks material claims" in text
    assert "advertised question\nroute directly to the user" in text
    assert "load skill `coordinator-communication`" not in text
    assert "assign a separate reviewer" not in text
    assert text.count("# Feature census and completeness contract") == 1
    reference = (REPO_ROOT / "skills/knowledge-harvest/references/feature-census.md").read_text()
    assert "../SKILL.md#feature-census-and-completeness-contract" in reference
    assert "## Coverage matrix" not in reference
    sync = load_role("documentation-sync").body_portable
    assert "runtime's mandatory review checks material claims" in sync
    assert "Independent reviewer when warranted" not in sync


def test_profile_generation_preserves_the_single_canonical_harvest_body(tmp_path):
    import shutil
    from generate_agent_profiles import check, write

    plugin = tmp_path / "plugin"
    plugin.mkdir()
    shutil.copytree(REPO_ROOT / "agent-sources", plugin / "agent-sources")
    shutil.copyfile(REPO_ROOT / "profiles.json", plugin / "profiles.json")
    harvest = plugin / "skills/knowledge-harvest"
    shutil.copytree(REPO_ROOT / "skills/knowledge-harvest", harvest)
    before = {path.relative_to(harvest): path.read_bytes()
              for path in harvest.rglob("*") if path.is_file()}
    write(plugin)
    check(plugin)
    after = {path.relative_to(harvest): path.read_bytes()
             for path in harvest.rglob("*") if path.is_file()}
    assert before == after
    assert len(list((plugin / "skills").glob("worker-*/SKILL.md"))) == 23
    assert not (plugin / "agents").exists()
