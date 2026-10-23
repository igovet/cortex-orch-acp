"""The unified Cortex system prompt reaches both engines as the same text."""
from __future__ import annotations

import datetime
import re

import pytest

from cortex_runtime.delegation.live_adapters import _session_new_meta_for
from cortex_runtime.engine.unified_system_prompt import (
    PROMPT_EDITION_ENV, _frontmatter_field, attached_servers_section, code_index_project_name,
    cortex_system_prompt, prompt_edition, rendered_prompt_edition, sent_prompt_edition,
)


def test_prompt_is_role_specific_engine_neutral_and_carries_the_environment(tmp_path):
    (tmp_path / ".git").mkdir()
    day = datetime.date(2026, 9, 28)
    worker = cortex_system_prompt("worker", str(tmp_path), today=day, environ={})
    coordinator = cortex_system_prompt("coordinator", str(tmp_path), today=day, environ={})
    assert worker.startswith("# Cortex session\n\nYou are a Cortex worker")
    assert coordinator.startswith("# Cortex session\n\nYou are the Cortex coordinator")
    for text in (worker, coordinator):
        assert f"- Working directory: {tmp_path}" in text and "- Git repository: yes" in text
        assert "- Date (UTC): 2026-09-28" in text
        assert "`read_skill`" in text and "code index" in text
        assert "runtime's channel" in text
        assert "`cortex` MCP server" in text and "`codebase-memory-mcp`" in text
        assert "native shell, file and search tools" in text
        # Project rule file names are named; engines and products are not.
        lowered = text.lower().replace("claude.md", "")
        assert "codex" not in lowered and "claude" not in lowered
    with pytest.raises(ValueError):
        cortex_system_prompt("reviewer", None)


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_both_engines_receive_policy_specific_prompt_before_the_same_role_text(tmp_path, role):
    claude = _session_new_meta_for("claude-agent-acp", "0.78.0", "Role text.", role=role, project_root=str(tmp_path))
    codex = _session_new_meta_for("codex-acp", "1.13.0", "Role text.", role=role, project_root=str(tmp_path))
    # Codex sessions also get the schema-derived tool cheat sheet after the role text.
    assert codex["systemPrompt"].split("\n\n# Cortex tool cheat sheet")[0] == "Role text."
    assert codex["cortexNativeContext"] is False
    assert claude["systemPrompt"] == cortex_system_prompt(role, str(tmp_path), native_tools=False).rstrip() + "\n\nRole text."
    assert codex["cortexBaseInstructions"] == cortex_system_prompt(role, str(tmp_path), code_mode=True)


def _skill(base, rel, name, description=None):
    directory = base / rel / name
    directory.mkdir(parents=True)
    (directory / "SKILL.md").write_text(
        f'---\nname: {name}\ndescription: "{description or "About " + name}."\n---\n\nBody.\n')
    return directory


def test_skills_from_every_engine_directory_are_listed_once_for_every_engine(tmp_path):
    project, home = tmp_path / "project", tmp_path / "home"
    project.mkdir()
    for rel, name in ((".agents/skills", "p-agents"), (".claude/skills", "p-claude"), (".codex/skills", "p-codex"),
                      ("skills", "p-bare")):
        _skill(project, rel, name)
    for rel, name in ((".agents/skills", "h-agents"), (".claude/skills", "h-claude"), (".codex/skills", "h-codex"),
                      (".codex/skills", ".system")):
        _skill(home, rel, name)
    # Claude's project link to a shared skill, a link escaping the project, and a directory without SKILL.md.
    (project / ".claude/skills/p-agents").symlink_to("../../.agents/skills/p-agents", target_is_directory=True)
    outside = _skill(tmp_path, "outside", "leak")
    (project / ".agents/skills/leak").symlink_to(outside, target_is_directory=True)
    (project / ".agents/skills/empty").mkdir()
    many = [_skill(project, ".agents/skills", f"bulk-{index:03d}") for index in range(60)]
    text = cortex_system_prompt("worker", str(project), environ={"HOME": str(home)})
    assert "# Project and user skills" in text
    assert "- `p-agents` (.agents/skills/p-agents/SKILL.md): About p-agents." in text
    assert text.count("`p-agents`") == 1
    for name in ("p-claude", "p-codex", "h-agents", "h-claude", "h-codex"):
        assert f"`{name}`" in text
    assert f"({home}/.claude/skills/h-claude/SKILL.md)" in text
    assert all(f"`{path.name}`" in text for path in many)
    for absent in ("p-bare", ".system", "leak", "`empty`"):
        assert absent not in text
    codex_home = tmp_path / "codex-home"
    _skill(codex_home, "skills", "cx-custom")
    assert "`cx-custom`" in cortex_system_prompt("worker", str(project), environ={"HOME": str(home), "CODEX_HOME": str(codex_home)})
    assert "skills" not in cortex_system_prompt("worker", None, environ={}).split("# Environment")[1]


def test_long_descriptions_are_cut_at_the_agent_skills_limit(tmp_path):
    _skill(tmp_path, ".agents/skills", "long", "x" * 3000)
    line = next(l for l in cortex_system_prompt("worker", str(tmp_path), environ={}).splitlines() if "`long`" in l)
    assert line.endswith(": " + "x" * 1024)



@pytest.mark.parametrize(
    ("scalar", "expected"),
    [
        ("plain scalar", "plain scalar"),
        ("'single ''quote'''", "single 'quote'"),
        (r'"double\nquote"', "double\nquote"),
    ],
)
def test_frontmatter_reader_supports_plain_and_quoted_scalars(scalar, expected):
    assert _frontmatter_field(f"---\ndescription: {scalar}\n---\n", "description") == expected


@pytest.mark.parametrize(
    ("scalar", "expected"),
    [
        ('"Inspect code and\n  tests."', "Inspect code and tests."),
        ("'Inspect code and\n  tests.'", "Inspect code and tests."),
        ('"Inspect code\\\n  and tests."', "Inspect codeand tests."),
        ('"Inspect code # and\n  tests."', "Inspect code # and tests."),
        ('"Inspect code\n\n  and tests."', "Inspect code\nand tests."),
    ],
)
def test_frontmatter_reader_supports_multiline_quoted_scalars(scalar, expected):
    assert _frontmatter_field(f"---\ndescription: {scalar}\n---\n", "description") == expected


def test_multiline_quoted_description_reaches_skill_catalog(tmp_path):
    skill = tmp_path / ".agents/skills/audit/SKILL.md"
    skill.parent.mkdir(parents=True)
    skill.write_text('---\nname: audit\ndescription: "Inspect code and\n  tests."\n---\n')
    prompt = cortex_system_prompt("worker", str(tmp_path), environ={})
    assert "- `audit` (.agents/skills/audit/SKILL.md): Inspect code and tests." in prompt


@pytest.mark.parametrize("scalar", ['"unfinished\n  value', "'unfinished\n  value"])
def test_frontmatter_reader_rejects_unclosed_multiline_quotes(scalar):
    assert _frontmatter_field(f"---\ndescription: {scalar}\n---\n", "description") == ""


def test_frontmatter_reader_rejects_unindented_quoted_continuation():
    text = '---\ndescription: "Inspect code\nname: other"\n---\n'
    assert _frontmatter_field(text, "description") == ""


def test_frontmatter_reader_treats_a_comment_only_field_as_empty():
    assert _frontmatter_field("---\ndescription: # comment\n---\n", "description") == ""


def test_frontmatter_reader_folds_plain_continuations():
    text = "---\ndescription: Inspect code\n  and tests.\n  Use for reviews.\n---\n"
    assert _frontmatter_field(text, "description") == "Inspect code and tests. Use for reviews."


@pytest.mark.parametrize(
    ("header", "expected"),
    [
        ("|", "Inspect code\nUse for reviews.\n"),
        ("|-", "Inspect code\nUse for reviews."),
        ("|+", "Inspect code\nUse for reviews.\n"),
        (">", "Inspect code Use for reviews.\n"),
        (">-", "Inspect code Use for reviews."),
        (">2+", "Inspect code Use for reviews.\n"),
    ],
)
def test_frontmatter_reader_supports_block_scalars_and_chomping(header, expected):
    text = f"---\ndescription: {header}\n  Inspect code\n  Use for reviews.\n---\n"
    assert _frontmatter_field(text, "description") == expected


@pytest.mark.parametrize("value", ["", "child: value", "- item", "[item]", "{key: value}", "*alias", "&anchor value", "!tag value"])
def test_frontmatter_reader_rejects_unsupported_yaml_nodes(value):
    text = f"---\ndescription: {value}\n  nested: value\n---\n"
    assert _frontmatter_field(text, "description") == ""


def test_frontmatter_reader_requires_an_exact_closing_delimiter():
    missing = "---\ndescription: valid\nBody.\n"
    four_dashes = "---\ndescription: valid\n----\nBody.\n"
    assert _frontmatter_field(missing, "description") == ""
    assert _frontmatter_field(four_dashes, "description") == ""


def test_code_index_project_name_uses_the_observed_absolute_path_convention():
    assert code_index_project_name("/home/igovet/Web_Projects/cortex-orch-acp") == \
        "home-igovet-Web_Projects-cortex-orch-acp"


@pytest.mark.parametrize("workspace_root", [None, "relative/project", "/workspace/unsafe name"])
def test_code_index_project_name_falls_back_when_the_convention_does_not_apply(workspace_root):
    assert code_index_project_name(workspace_root) is None

def test_session_manager_never_guesses_the_code_index_project_from_path(tmp_path):
    """H1: a worker's first codebase-memory-mcp call against a guessed
    project name failed on an unindexed workspace (live, 2026-10-07) --
    this runtime has no way to observe whether the attached server
    actually has the workspace indexed, so engine.session_manager must
    never derive ``code_index_project`` from the path convention; it
    always passes ``None`` and lets the prompt's list_projects-first
    instruction (attached_servers_section) stand."""
    import pathlib

    source = (pathlib.Path(__file__).resolve().parents[1]
              / "src" / "cortex_runtime" / "engine" / "session_manager.py").read_text()
    assert "code_index_project_name(" not in source
    assert source.count("code_index_project=None") == 2


def test_attached_server_section_uses_a_resolved_code_index_project_directly():
    section = attached_servers_section(
        [{"name": "codebase-memory-mcp", "allowed_tools": [
            "list_projects", "search_graph", "trace_path", "get_code_snippet",
        ]}],
        "/home/igovet/Web_Projects/cortex-orch-acp",
        code_index_project="home-igovet-Web_Projects-cortex-orch-acp",
    )
    assert "The index project for this workspace is `home-igovet-Web_Projects-cortex-orch-acp`; use it directly." in section
    assert "If that project is not found, call `list_projects` once" in section
    assert "1. Call `list_projects` once" not in section


def test_attached_server_section_without_a_resolved_project_keeps_the_lookup_text():
    section = attached_servers_section(
        [{"name": "codebase-memory-mcp", "allowed_tools": [
            "list_projects", "search_graph", "trace_path", "get_code_snippet",
        ]}],
        "/workspace/project",
    )
    assert "1. Call `list_projects` once and take the project whose root path is `/workspace/project`" in section
    assert "The index project for this workspace is" not in section

def test_role_specific_cortex_tool_guidance_keeps_worker_permissions_out_of_coordinator_text():
    worker = cortex_system_prompt("worker", None, environ={})
    coordinator = cortex_system_prompt("coordinator", None, environ={})
    assert "`command_run` for worker checks" in worker
    assert "project reads and edits" in worker
    assert "command_run" not in coordinator
    assert "project reads and edits" not in coordinator
    assert "@@" not in worker + coordinator


def test_only_workers_are_told_to_work_without_narration():
    worker = cortex_system_prompt("worker", None, environ={})
    coordinator = cortex_system_prompt("coordinator", None, environ={})
    assert "Work without narration" in worker and "write one final message" in worker
    assert "including before the first tool and after a tool result" in worker
    assert "Native shell output alone is not a verified check receipt" in worker
    assert "Work without narration" not in coordinator and "@@" not in worker + coordinator


def test_refused_calls_route_workers_to_access_request_before_scope_or_replan():
    worker = cortex_system_prompt("worker", None, environ={})
    coordinator = cortex_system_prompt("coordinator", None, environ={})

    def refusal(text):
        return next(line for line in text.splitlines() if "is a decision, not a glitch" in line)

    for text in (worker, coordinator):
        line = refusal(text)
        assert "Do not retry it and do not reach the same effect another way" in line
        assert "@@" not in text
    worker_line, coordinator_line = refusal(worker), refusal(coordinator)
    # A worker asks through its assignment-local route first; scope_request
    # and replan_request stop every worker and are only for a changed plan.
    assert worker_line.index("`access_request`") < worker_line.index("scope or replan request")
    assert "only when the plan itself must change" in worker_line
    assert "runtime's scope request" not in worker_line
    # The coordinator has no access_request and changes nothing itself.
    assert "access_request" not in coordinator_line
    assert "plan it for a worker" in coordinator_line


def test_only_the_coordinator_is_told_to_write_nothing_after_its_question():
    worker = cortex_system_prompt("worker", None, environ={})
    coordinator = cortex_system_prompt("coordinator", None, environ={})
    rule = "end the turn right after it: write nothing more"
    assert rule in coordinator and rule not in worker


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_conditional_skill_instruction_is_imperative_but_not_eager(role):
    prompt = cortex_system_prompt(role, None, environ={})
    assert "call the `read_skill` tool from the `cortex` MCP server with that exact skill id" in prompt
    # Workers do not plan, so only the coordinator's example names a plan.
    example = ("before assessing documentation impact for a plan or updating affected docs" if role == "coordinator"
               else "before updating docs affected by a behavior or contract change")
    assert example in prompt
    assert ("for a plan" in prompt.split("For example,", 1)[1].split("Do not read", 1)[0]) == (role == "coordinator")
    assert "Do not read a conditional skill when its trigger has not arisen" in prompt
    assert "do not re-read them" in prompt


def test_attached_server_section_without_native_search_names_granted_literal_search_and_prefixes():
    section = attached_servers_section(
        [{"name": "codebase-memory-mcp", "allowed_tools": [
            "list_projects", "search_graph", "trace_path", "get_code_snippet", "search_code",
        ]}],
        "/workspace/project",
        native_search=False,
    )
    assert "Tools by server:" in section
    assert "`codebase-memory-mcp`: `get_code_snippet`, `list_projects`, `search_code`, `search_graph`, `trace_path`" in section
    assert "`cortex` supplies runtime tools; it has no code-search, graph, or index tools." in section
    assert "`search_code` on `codebase-memory-mcp`" in section
    assert "`project_read` on `cortex` for a known file" in section
    assert "no native shell or file search" in section
    assert "`mcp__<server>__<tool>`" in section
    assert "Use shell or file search" not in section


def test_partial_code_index_grant_keeps_resolved_project_name():
    section = attached_servers_section(
        [{"name": "codebase-memory-mcp", "allowed_tools": ["search_code"]}],
        "/workspace/project",
        code_index_project="workspace-project",
        native_search=False,
    )
    assert "The index project for this workspace is `workspace-project`; use it directly." in section
    assert "`search_code` on `codebase-memory-mcp`" in section
    assert "`project_read` on `cortex` for a known file" in section


@pytest.mark.parametrize("access_class", ["write", "read_only", "consultant"])
@pytest.mark.parametrize("native_tools", [False, True])
def test_four_role_blocks_gate_native_guidance(access_class, native_tools):
    text = cortex_system_prompt("worker", None, environ={}, access_class=access_class,
                                native_tools=native_tools)
    assert ("engine's native shell" in text) == (native_tools and access_class != "consultant")
    assert ("engine's tool search" in text) == (native_tools and access_class != "consultant")
    assert "@@" not in text
    if access_class == "write":
        assert "# Changing the project" in text and "project reads and edits" in text
        assert "Verify what you change" in text
    else:
        assert "# Changing the project" not in text and "project reads and edits" not in text
    if access_class == "read_only":
        assert "do not create or change project files" in text
        assert "authorized non-mutating checks" in text
    if access_class == "consultant":
        assert "# Working material" in text and "# Finding code" not in text
        assert "command_run" not in text
        assert ("any engine-native tool" in text) is native_tools


@pytest.mark.parametrize("native_tools", [False, True])
def test_coordinator_block_has_no_worker_command_or_edit_obligations(native_tools):
    text = cortex_system_prompt("coordinator", None, environ={}, native_tools=native_tools)
    for rule in ("command_run", "Treat shell command text", "Verify what you change",
                 "project reads and edits", "# Changing the project"):
        assert rule not in text
    assert ("engine's native shell" in text) is native_tools


@pytest.mark.parametrize("engine,version,key,native", [
    ("claude-agent-acp", "0.78.0", "systemPrompt", False),
    ("codex-acp", "1.13.0", "cortexBaseInstructions", True),
])
@pytest.mark.parametrize("role_id,access_class", [
    ("worker-refactorer", "write"), ("worker-code-reviewer", "read_only"),
    ("code-reviewer", "read_only"), ("senior-consultant", "consultant"),
])
def test_session_metadata_selects_access_class_and_native_policy(tmp_path, engine, version, key,
                                                                native, role_id, access_class):
    meta = _session_new_meta_for(engine, version, "Role text.", role="worker",
                                role_id=role_id, project_root=str(tmp_path), environ={})
    # Only Codex runs in code mode; ``native`` marks the Codex row here.
    base = cortex_system_prompt("worker", str(tmp_path), environ={}, access_class=access_class,
                                native_tools=native, code_mode=native)
    assert meta[key] == (base.rstrip() + "\n\nRole text." if not native else base)


def test_invalid_access_class_fails_and_consultant_keyword_keeps_compatibility():
    with pytest.raises(ValueError):
        cortex_system_prompt("worker", None, access_class="unknown")
    assert cortex_system_prompt("worker", None, environ={}, consultant=True) == cortex_system_prompt(
        "worker", None, environ={}, access_class="consultant")


@pytest.mark.parametrize("role_id,access_class", [
    ("worker-refactorer", "write"), ("worker-code-reviewer", "read_only"),
    ("senior-consultant", "consultant"),
])
def test_opencode_agent_prompt_selects_all_access_classes(tmp_path, role_id, access_class):
    from cortex_runtime.engine.opencode_adapter_acp import build_agent_prompt

    text = build_agent_prompt("Role text.", role="worker", role_id=role_id,
                              project_root=str(tmp_path), environ={})
    base = cortex_system_prompt("worker", str(tmp_path), environ={}, access_class=access_class)
    assert text.startswith(base.rstrip())


# Prompt editions. Edition 1 is today's text and stays the default; edition 2
# reorders the same rules and is selected only by the exact opt-in value.

_VARIANTS = [("coordinator", "write"), ("worker", "write"), ("worker", "read_only"), ("worker", "consultant")]
_EDITION_1_HEADINGS = {
    "coordinator": ["# Cortex session", "# Authority", "# Tools", "# Cortex skills", "# Finding code",
                    "# Project boundaries", "# Evidence and honesty", "# Safety", "# Communication",
                    "# Context", "# Environment"],
    "write": ["# Cortex session", "# Authority", "# Tools", "# Cortex skills", "# Finding code",
              "# Changing the project", "# Evidence and honesty", "# Safety", "# Communication",
              "# Context", "# Environment"],
    "read_only": ["# Cortex session", "# Authority", "# Tools", "# Cortex skills", "# Finding code",
                  "# Project boundaries", "# Evidence and honesty", "# Safety", "# Communication",
                  "# Context", "# Environment"],
    "consultant": ["# Cortex session", "# Authority", "# Tools", "# Cortex skills", "# Working material",
                   "# Project boundaries", "# Evidence and honesty", "# Safety", "# Communication",
                   "# Context", "# Environment"],
}
_EDITION_2_HEADINGS = ["# Cortex session", "# How to work", "# Checks", "# Writing for your reader",
                       "# Hard limits", "# Tools and evidence", "# Environment"]
_EDITION_2_ONLY = ("# How to work", "# Checks", "# Writing for your reader", "# Hard limits",
                   "# Tools and evidence")


def _headings(text, prefix="# "):
    return [line for line in text.splitlines() if line.startswith(prefix)]


@pytest.mark.parametrize("value", [None, "", "1", "0", "3", " 2", "2 ", "two", "true"])
def test_edition_flag_off_keeps_edition_one(value):
    environ = {} if value is None else {PROMPT_EDITION_ENV: value}
    assert prompt_edition(environ) == 1
    for role, access_class in _VARIANTS:
        text = cortex_system_prompt(role, None, environ=environ, access_class=access_class)
        assert text == cortex_system_prompt(role, None, environ={}, access_class=access_class, edition=1)
        assert not set(_EDITION_2_ONLY) & set(_headings(text))


def test_edition_flag_reads_the_process_environment_when_no_mapping_is_given(monkeypatch):
    monkeypatch.delenv(PROMPT_EDITION_ENV, raising=False)
    assert prompt_edition() == 1
    monkeypatch.setenv(PROMPT_EDITION_ENV, "2")
    assert prompt_edition() == 2
    assert rendered_prompt_edition(cortex_system_prompt("worker", None)) == 2


def test_edition_flag_two_selects_edition_two_and_unknown_editions_fail():
    environ = {PROMPT_EDITION_ENV: "2"}
    assert prompt_edition(environ) == 2
    for role, access_class in _VARIANTS:
        assert cortex_system_prompt(role, None, environ=environ, access_class=access_class) == \
            cortex_system_prompt(role, None, environ={}, access_class=access_class, edition=2)
        # An explicit edition wins over the environment.
        assert cortex_system_prompt(role, None, environ=environ, access_class=access_class, edition=1) == \
            cortex_system_prompt(role, None, environ={}, access_class=access_class)
    for edition in (0, 3, "2"):
        with pytest.raises(ValueError):
            cortex_system_prompt("worker", None, environ={}, edition=edition)


@pytest.mark.parametrize("role,access_class", _VARIANTS)
@pytest.mark.parametrize("native_tools", [False, True])
def test_edition_one_keeps_todays_section_order(role, access_class, native_tools):
    text = cortex_system_prompt(role, None, environ={}, access_class=access_class, native_tools=native_tools)
    key = "coordinator" if role == "coordinator" else access_class
    assert _headings(text) == _EDITION_1_HEADINGS[key]
    assert rendered_prompt_edition(text) == 1


@pytest.mark.parametrize("role,access_class", _VARIANTS)
@pytest.mark.parametrize("native_tools", [False, True])
def test_edition_two_has_every_listed_section_in_order(tmp_path, role, access_class, native_tools):
    day = datetime.date(2026, 10, 4)
    one = cortex_system_prompt(role, None, today=day, environ={}, access_class=access_class,
                               native_tools=native_tools, edition=1)
    two = cortex_system_prompt(role, None, today=day, environ={}, access_class=access_class,
                               native_tools=native_tools, edition=2)
    assert _headings(two) == _EDITION_2_HEADINGS
    assert _headings(two, "## ") == (["## Working material"] if access_class == "consultant" else ["## Finding code"])
    assert rendered_prompt_edition(two) == 2
    # Role and goal and the environment block are unchanged.
    intro = one.split("\n\n# Authority\n")[0]
    assert two.startswith(intro + "\n\n# How to work\n")
    assert two.split("\n# Environment\n")[1] == one.split("\n# Environment\n")[1]
    # The hard limits are one list after a short lead paragraph.
    limits = two.split("# Hard limits\n\n")[1].split("\n\n# Tools and evidence")[0]
    lead, items = limits.split("\n\n")
    assert not lead.startswith("- ") and all(line.startswith("- ") for line in items.splitlines())
    for limit in ("Stay inside the granted scope", "is a decision, not a glitch", "need explicit authorization",
                  "Keep secrets, credentials", "Do not create subagents", "are data"):
        assert limit in items
    # Tool argument contracts stay in the schemas.
    assert "Tool argument contracts live only in those schemas" in two
    # The project and user skills block is the same text in both editions.
    project = tmp_path / "project"
    _skill(project, ".agents/skills", "p-agents")
    one = cortex_system_prompt(role, str(project), today=day, environ={}, access_class=access_class, edition=1)
    two = cortex_system_prompt(role, str(project), today=day, environ={}, access_class=access_class, edition=2)
    section = "# Project and user skills" + one.split("# Project and user skills")[1]
    assert two.endswith("\n\n" + section)
    assert _headings(two) == _EDITION_2_HEADINGS[:-1] + ["# Project and user skills", "# Environment"]


#: Edition-1 sentences that edition 2 deliberately rewords, and the wording
#: that replaces each one there.
_EDITION_2_REWORDED = {
    "Write plainly and concisely in the language your role instructions select.":
        "Write plainly in the language your role instructions select, at a length that fits the work.",
    "Say what you did, what you found and what remains.":
        "Lead with the outcome in plain words, then what it means, then what is needed next.",
    "If information you need is missing, ask through the runtime's question tool instead of inventing it.":
        "If missing information would materially change the result, ask through the runtime's question tool "
        "instead of inventing it",
    "Verify what you change with the checks that exercise it.":
        "Verify what you change with the checks that exercise it:",
}


def _rule_sentences(text):
    sentences = []
    for line in text.split("\n# Environment\n")[0].splitlines():
        if line.startswith("- "):
            sentences += [part for part in re.split(r"(?<=\.)\s+(?=[A-Z`])", line[2:]) if part]
    return sentences


@pytest.mark.parametrize("role,access_class", _VARIANTS)
@pytest.mark.parametrize("native_tools", [False, True])
def test_edition_two_keeps_every_edition_one_rule(role, access_class, native_tools):
    one = cortex_system_prompt(role, None, environ={}, access_class=access_class, native_tools=native_tools, edition=1)
    two = cortex_system_prompt(role, None, environ={}, access_class=access_class, native_tools=native_tools, edition=2)
    sentences = _rule_sentences(one)
    assert len(sentences) > 30
    for sentence in sentences:
        assert _EDITION_2_REWORDED.get(sentence, sentence) in two, sentence


@pytest.mark.parametrize("role,access_class", _VARIANTS)
@pytest.mark.parametrize("native_tools", [False, True])
def test_edition_two_keeps_role_boundaries(role, access_class, native_tools):
    text = cortex_system_prompt(role, None, environ={}, access_class=access_class, native_tools=native_tools,
                                edition=2)
    assert "@@" not in text
    lowered = text.lower().replace("claude.md", "")
    assert "codex" not in lowered and "claude" not in lowered
    assert "call the `read_skill` tool from the `cortex` MCP server with that exact skill id" in text
    assert "Do not read a conditional skill when its trigger has not arisen" in text
    assert "Lead with the outcome in plain words" in text and "Describe checks by their consequence" in text
    consultant = access_class == "consultant"
    assert ("engine's native shell" in text) == (native_tools and not consultant)
    assert ("any engine-native tool" in text) == (native_tools and consultant)
    worker_only = ("command_run", "Treat shell command text", "project reads and edits")
    if role == "coordinator":
        assert not any(rule in text for rule in worker_only)
        assert "Work without narration" not in text
        assert "end the turn right after it: write nothing more" in text
        assert "plan that work for workers" in text and "You run no checks yourself" in text
        assert "Do not show internal ids, codes, route names, revisions or receipts" in text
        return
    assert "Work without narration" in text and "write one final message" in text
    assert "end the turn right after it: write nothing more" not in text
    assert "exact commands, receipts and identifiers belong in the report's evidence" in text
    if access_class == "write":
        assert "project reads and edits" in text and "make the smallest coherent change" in text
        assert "rerun the same command" in text and "a re-plan stops every worker" in text
    else:
        assert "project reads and edits" not in text and "make the smallest coherent change" not in text
    if access_class == "read_only":
        assert "do not create or change project files" in text and "authorized non-mutating checks" in text
    if consultant:
        assert "command_run" not in text and "you ask the user nothing" in text
        assert "access_request" not in text


def test_edition_two_refusal_routing_stays_role_aware():
    def refusal(text):
        return next(line for line in text.splitlines() if "is a decision, not a glitch" in line)

    worker = refusal(cortex_system_prompt("worker", None, environ={}, edition=2))
    coordinator = refusal(cortex_system_prompt("coordinator", None, environ={}, edition=2))
    assert worker.index("`access_request`") < worker.index("scope or replan request")
    assert "only when the plan itself must change" in worker
    assert "access_request" not in coordinator and "plan it for a worker" in coordinator


@pytest.mark.parametrize("edition", ["1", "2"])
@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_session_metadata_carries_the_selected_edition(tmp_path, edition, role):
    environ = {PROMPT_EDITION_ENV: edition}
    claude = _session_new_meta_for("claude-agent-acp", "0.78.0", "Role text.", role=role,
                                   project_root=str(tmp_path), environ=environ)
    codex = _session_new_meta_for("codex-acp", "1.13.0", "Role text.", role=role,
                                  project_root=str(tmp_path), environ=environ)
    expected = int(edition)
    assert claude["systemPrompt"].startswith(cortex_system_prompt(
        role, str(tmp_path), environ={}, native_tools=False, edition=expected).rstrip())
    assert codex["cortexBaseInstructions"] == cortex_system_prompt(role, str(tmp_path), environ={}, edition=expected,
                                                                   code_mode=True)
    assert sent_prompt_edition(claude) == sent_prompt_edition(codex) == expected
    from cortex_runtime.engine.opencode_adapter_acp import build_agent_prompt

    agent_prompt = build_agent_prompt("Role text.", role=role, project_root=str(tmp_path), environ=environ)
    assert sent_prompt_edition({"opencodeAgentPrompt": agent_prompt}) == expected


def test_native_base_prompt_metadata_records_no_edition(tmp_path):
    environ = {"CORTEX_UNIFIED_BASE_PROMPT": "0", PROMPT_EDITION_ENV: "2"}
    for engine, version in (("claude-agent-acp", "0.78.0"), ("codex-acp", "1.13.0")):
        meta = _session_new_meta_for(engine, version, "Role text.", role="worker",
                                     project_root=str(tmp_path), environ=environ)
        assert sent_prompt_edition(meta) is None
    assert sent_prompt_edition(None) is None and sent_prompt_edition({"systemPrompt": {"append": "x"}}) is None
    assert rendered_prompt_edition("# Cortex session\n\nIntro.\n\n# Unknown\n") is None
    assert rendered_prompt_edition("Role text.\n\n# How to work\n") is None


#: The rules every engine receives in both editions: Claude kept to them
#: unprompted, Codex did not once its own base prompt was replaced.
_SHARED_ECONOMY_RULES = (
    "Act once you know enough for the next step.",
    "Do not re-read a file, report page or skill you already read in this session unless it may have changed",
    "A trigger matches when your next action is the one it names, not when a skill's topic merely looks related",
    "Reading a matching skill is part of the work, not an optional extra.",
    "Compaction does not end the work: continue from the summarized state without redoing completed steps.",
    "Do not add approval steps, warnings or checklists for hypothetical risks.",
)
_CODE_MODE_ONLY = ("`ALL_TOOLS`", "Promise.allSettled", "Cortex tool cheat sheet")


@pytest.mark.parametrize("edition", [1, 2])
@pytest.mark.parametrize("role,access_class", _VARIANTS)
@pytest.mark.parametrize("native_tools,code_mode", [(False, False), (True, False), (True, True)])
def test_every_engine_gets_the_shared_working_economy_rules(edition, role, access_class, native_tools, code_mode):
    text = cortex_system_prompt(role, None, environ={}, access_class=access_class, native_tools=native_tools,
                                edition=edition, code_mode=code_mode)
    for rule in _SHARED_ECONOMY_RULES:
        assert rule in text, rule
    assert "make routine" in text.lower()
    assert "record the assumption" in text or "state each assumption" in text
    assert ("Orient only as far as the plan needs" in text) == (role == "coordinator")
    assert ("Do not add tests for low-impact, reversible changes" in text) == (role == "worker" and access_class == "write")
    assert "Afterwards, recover the exact assignment" not in text
    for marker in _CODE_MODE_ONLY:
        assert (marker in text) == code_mode, marker


@pytest.mark.parametrize("edition", [1, 2])
@pytest.mark.parametrize("role,access_class", _VARIANTS)
def test_code_mode_swaps_only_the_schema_tool_search_and_parallel_lines(edition, role, access_class):
    plain = cortex_system_prompt(role, None, environ={}, access_class=access_class, edition=edition)
    code = cortex_system_prompt(role, None, environ={}, access_class=access_class, edition=edition, code_mode=True)
    assert "read its full advertised description and argument schema" in plain
    assert "read its full advertised description and argument schema" not in code
    assert "engine's tool search" not in code
    assert "If a call fails validation, correct the arguments from the error once." in code
    assert "Put independent calls in one `exec` with `await Promise.allSettled([...])`" in code
    plain_lines = set(plain.splitlines())
    code_lines = set(code.splitlines())
    changed = {line for line in plain_lines ^ code_lines}
    swapped = ("Before the first call to any tool", "If a tool you need is not in your list", "Code mode lists",
               "Put independent calls in one `exec`")
    assert all(line.startswith(tuple("- " + prefix for prefix in swapped)) for line in changed), changed


@pytest.mark.parametrize("engine,version,key", [
    ("claude-agent-acp", "0.78.0", "systemPrompt"),
    ("codex-acp", "1.13.0", "cortexBaseInstructions"),
])
def test_only_codex_sessions_receive_the_code_mode_lines(tmp_path, engine, version, key):
    meta = _session_new_meta_for(engine, version, "Role text.", role="worker", project_root=str(tmp_path),
                                 environ={})
    for marker in ("`ALL_TOOLS`", "Promise.allSettled"):
        assert (marker in meta[key]) == (engine == "codex-acp")
    from cortex_runtime.engine.opencode_adapter_acp import build_agent_prompt

    agent_prompt = build_agent_prompt("Role text.", role="worker", project_root=str(tmp_path), environ={})
    assert "`ALL_TOOLS`" not in agent_prompt and "Promise.allSettled" not in agent_prompt
    assert "Compaction does not end the work" in agent_prompt


def test_code_mode_attached_server_section_drops_the_tool_search_line():
    servers = [{"name": "codebase-memory-mcp",
                "allowed_tools": ["list_projects", "search_graph", "trace_path", "get_code_snippet"]}]
    plain = attached_servers_section(servers, "/workspace/project")
    code = attached_servers_section(servers, "/workspace/project", code_mode=True)
    assert "engine's tool search" in plain
    assert "engine's tool search" not in code
    assert code == plain.split("\n\nIf a tool of an attached server")[0]
