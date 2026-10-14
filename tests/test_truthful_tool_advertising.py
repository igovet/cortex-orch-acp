"""Prompts advertise only what the role really has.

Exercises the fixes directly against the functions they touch, without a
live engine or subprocess.
"""
from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "src"))

from cortex_runtime.engine.mcp_bridge import host_mcp_servers, is_read_only_role, missing_grant_paths  # noqa: E402
from cortex_runtime.engine.unified_system_prompt import attached_servers_section, cortex_system_prompt  # noqa: E402
from cortex_runtime.portable.roles import NAMED_SKILLS, situational_skill_triggers  # noqa: E402


# ---------------------------------------------------------------------------
# The attached-MCP section must list only tools actually granted after
# external_proxy's own role-based filtering (external_proxy.py:124-125), and
# must not send a read-only role to run index_repository.
# ---------------------------------------------------------------------------

_FULL_INDEX_SERVER = {
    "name": "codebase-memory-mcp",
    "allowed_tools": ["list_projects", "search_graph", "trace_path", "get_code_snippet",
                       "get_architecture", "index_repository", "ingest_traces"],
    "read_only_tools": ["list_projects", "search_graph", "trace_path", "get_code_snippet", "get_architecture"],
}


def test_a_read_only_worker_role_sees_only_its_read_only_tools():
    is_read_only = attached_servers_section([_FULL_INDEX_SERVER], "/work/project",
                                            role="worker", role_id="worker-architect")
    assert "`ingest_traces`" not in is_read_only
    assert "`index_repository`" not in is_read_only
    assert "index_repository` attempt" not in is_read_only
    assert "`search_graph`, `trace_path` and `get_code_snippet`" in is_read_only
    assert is_read_only_role("worker-architect")
    assert is_read_only_role("architect")  # the short alias resolves the same way
    assert not is_read_only_role("worker-backend-dev")
    assert not is_read_only_role(None)


def test_an_ordinary_writing_worker_keeps_the_full_allowed_tools_grant():
    full = attached_servers_section([_FULL_INDEX_SERVER], "/work/project",
                                    role="worker", role_id="worker-backend-dev")
    assert "`ingest_traces`" in full
    assert "`index_repository` attempt" in full


def test_coordinator_still_uses_coordinator_tools_not_the_new_read_only_path():
    coordinator = attached_servers_section(
        [{**_FULL_INDEX_SERVER, "coordinator_tools": ["search_graph"]}], "/work/project", role="coordinator")
    assert "only these granted tools: `search_graph`" in coordinator


# ---------------------------------------------------------------------------
# An external MCP server refused because a read_paths entry is
# missing must not be advertised; the caller gets a clear diagnostic naming
# the missing path instead of a silent omission.
# ---------------------------------------------------------------------------

def test_a_server_with_a_missing_read_path_is_not_advertised(tmp_path):
    present = tmp_path / "exists"
    present.mkdir()
    absent = tmp_path / "gone"
    entry = {"name": "stale", "command": "/bin/true", "allowed_tools": ["inspect"],
              "read_paths": [str(present), str(absent)]}
    assert missing_grant_paths(entry) == [str(absent)]
    dropped: list = []
    forwarded = host_mcp_servers([entry], dropped=dropped)
    assert forwarded == []
    assert dropped == [{"name": "stale", "missing_paths": [str(absent)]}]


def test_a_server_with_every_grant_path_present_is_advertised_normally(tmp_path):
    present = tmp_path / "exists"
    present.mkdir()
    entry = {"name": "ok", "command": "/bin/true", "allowed_tools": ["inspect"], "read_paths": [str(present)]}
    dropped: list = []
    forwarded = host_mcp_servers([entry], dropped=dropped)
    assert [e["name"] for e in forwarded] == ["ok"]
    assert dropped == []


# ---------------------------------------------------------------------------
# The coordinator's own reference skills (NAMED_SKILLS["coordinator"])
# must be named with a trigger, and the base prompt's one worked example of
# `read_skill` must never name a skill the role cannot actually read.
# ---------------------------------------------------------------------------

def test_coordinator_reference_skills_are_named_with_a_trigger():
    assert "worker-routing" in NAMED_SKILLS["coordinator"]
    triggers = dict(situational_skill_triggers("coordinator"))
    assert "worker-routing" in triggers and triggers["worker-routing"]
    assert "senior-consultation" in triggers and triggers["senior-consultation"]


def test_consultant_skill_example_never_names_an_excluded_skill():
    prompt = cortex_system_prompt("worker", None, access_class="consultant")
    assert "`documentation-sync`" not in prompt
    assert "`interactive-resources`" not in prompt


def test_coordinator_skill_example_never_names_interactive_resources():
    prompt = cortex_system_prompt("coordinator", None)
    assert "`interactive-resources`" not in prompt
    assert "`documentation-sync`" in prompt


def test_write_and_read_only_worker_examples_are_unchanged():
    write = cortex_system_prompt("worker", None, access_class="write")
    read_only = cortex_system_prompt("worker", None, access_class="read_only")
    for prompt in (write, read_only):
        assert "`documentation-sync`" in prompt and "`interactive-resources`" in prompt
