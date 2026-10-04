"""Executable drift guards for role prompts and runtime tool offers."""

from datetime import date
from pathlib import Path
import re

import pytest

from cortex_runtime.channeled_tools import channeled_base_tool_names, channeled_base_tools
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt
from cortex_runtime.orchestration.contracts import READ_ONLY
from cortex_runtime.portable.roles import (
    EXAMPLE_REFERENCE_IDS,
    WORKER_REFERENCE_IDS,
    composed_instructions,
    known_role_ids,
    load_role,
    read_reference,
    resolve_role,
)
from cortex_runtime.recovery.capsule import build_capsule
from test_recovery_capsule import (
    _delegate,
    _provision_worker,
    _set_node_profile,
    _task,
)

ROOT = Path(__file__).resolve().parents[1]
WORKER_ROLE_IDS = tuple(
    role_id for role_id in known_role_ids() if role_id.startswith("worker-")
)
ROLE_IDS = ("orchestrator", *WORKER_ROLE_IDS)
ROLE_REFERENCE_LINK = re.compile(r"\]\(references/([a-z0-9-]+)\.md\)")

# Fixed-size snapshot: 20,071 rendered characters on 2026-10-04
# (6,957 coordinator base + 13,114 composed role text), with about 5% headroom.
COORDINATOR_PROMPT_BUDGET_CHARS = 21_100


def _offered_tool_names(role_kind, role_id=None, stage=None):
    return set(channeled_base_tool_names()) | {
        item["name"]
        for item in mcp_tools.tool_definitions(
            role_kind, role_id=role_id, stage=stage
        )
    }


def test_role_skill_reference_links_resolve_through_the_role_reader():
    linked_references = 0
    for role_id in ROLE_IDS:
        role = load_role(role_id)
        role_kind = "coordinator" if role_id == "orchestrator" else "worker"
        for reference_id in set(ROLE_REFERENCE_LINK.findall(role.body_portable)):
            body = read_reference(reference_id, role_kind=role_kind)
            assert body.strip(), (role_id, reference_id)
            linked_references += 1
    assert linked_references > 0


@pytest.mark.parametrize("role_id", WORKER_ROLE_IDS)
def test_recovery_capsule_tools_match_each_worker_role_and_stage(tmp_path, role_id):
    store, task_id, coordinator = _task(tmp_path)
    assignment = _delegate(
        store, coordinator, request_key=f"prompt-drift-{role_id}", role=role_id
    )
    worker = _provision_worker(
        store, task_id, coordinator, assignment["assignment_ref"]
    )

    for stage in ("execution", "review"):
        _set_node_profile(
            store, assignment["assignment_ref"], role=role_id, stage=stage
        )
        capsule = build_capsule(
            store,
            task_id,
            dict(
                role="worker",
                assignment_ref=assignment["assignment_ref"],
                attempt_id="xa_test1",
            ),
        )
        actual_role_id, actual_stage = mcp_tools.assignment_profile(store, worker)
        assert (actual_role_id, actual_stage) == (role_id, stage)
        offered = _offered_tool_names("worker", role_id, stage)
        allowed = set(
            capsule["sections"]["role_and_tool_access"]["allowed_mcp_tools"]
        )
        assert allowed == offered, (role_id, stage, sorted(allowed ^ offered))


def test_coordinator_recovery_capsule_tools_match_its_offer(tmp_path):
    store, task_id, _ = _task(tmp_path)
    capsule = build_capsule(store, task_id, dict(role="coordinator"))
    allowed = set(capsule["sections"]["role_and_tool_access"]["allowed_mcp_tools"])
    offered = _offered_tool_names("coordinator")
    assert allowed == offered, sorted(allowed ^ offered)


def test_shipped_skills_and_agent_sources_contain_no_cortex_dev_policy():
    forbidden = (
        "paseo",
        "~/.paseo-dev",
        "--i-authorize-paid-call",
        "integrations/paseo",
    )
    violations = []
    for root in (ROOT / "skills", ROOT / "agent-sources"):
        for path in root.rglob("*"):
            if not path.is_file():
                continue
            body = path.read_text(encoding="utf-8").casefold()
            for phrase in forbidden:
                if phrase.casefold() in body:
                    violations.append((path.relative_to(ROOT).as_posix(), phrase))
    assert not violations, violations


def test_worker_readable_instructions_never_say_ask_the_coordinator():
    worker_texts = [
        (role_id, composed_instructions(load_role(role_id)))
        for role_id in WORKER_ROLE_IDS
    ]
    for reference_id in sorted(WORKER_REFERENCE_IDS | EXAMPLE_REFERENCE_IDS):
        worker_texts.append(
            (
                reference_id,
                read_reference(reference_id, role_kind="worker"),
            )
        )
    for source_id, body in worker_texts:
        assert "ask the coordinator" not in body.casefold(), source_id


def test_composed_instructions_do_not_reload_a_skill_already_inlined():
    delimiter = chr(96)
    for role_id in ROLE_IDS:
        instructions = composed_instructions(load_role(role_id))
        inline_heading = "## Skills already included"
        loadable_heading = "## Skills, and when to read one"

        inline_ids = set()
        if inline_heading in instructions:
            inline_section = instructions.split(inline_heading, 1)[1]
            if loadable_heading in inline_section:
                inline_section = inline_section.split(loadable_heading, 1)[0]
            for line in inline_section.splitlines():
                if line.startswith("### Skill "):
                    inline_ids.add(line.split(delimiter)[1])

        loadable_ids = set()
        if loadable_heading in instructions:
            loadable_section = instructions.split(loadable_heading, 1)[1]
            for line in loadable_section.splitlines():
                if line.startswith("- " + delimiter):
                    loadable_ids.add(line.split(delimiter)[1])

        assert inline_ids.isdisjoint(loadable_ids), (
            role_id,
            sorted(inline_ids & loadable_ids),
        )


def test_base_tool_descriptions_have_no_codex_store_or_hook_residue():
    descriptions = "\n".join(
        tool["description"] for tool in channeled_base_tools()
    ).casefold()
    for phrase in (".codex/cortex", "codex_home", "hook"):
        assert phrase not in descriptions, phrase


def test_prompt_read_only_roles_equal_the_runtime_read_only_set():
    prompt_read_only = set()
    for role_id in WORKER_ROLE_IDS:
        role = load_role(role_id)
        match = re.search(
            r"(?ms)^## Role and responsibility\s*\n(.*?)(?=^## |\Z)",
            role.body_portable,
        )
        assert match, role_id
        responsibility = match.group(1)
        if re.search(
            r"\brole is read-only\b|\breport-only\b",
            responsibility,
            flags=re.IGNORECASE,
        ):
            prompt_read_only.add(role_id)

    runtime_read_only = {resolve_role(role_id) for role_id in READ_ONLY}
    assert prompt_read_only == runtime_read_only, (
        "Prompt/runtime read-only mismatch: "
        f"prompt-only={sorted(prompt_read_only - runtime_read_only)}, "
        f"runtime-only={sorted(runtime_read_only - prompt_read_only)}"
    )


def test_coordinator_base_prompt_plus_role_text_stays_within_budget():
    base_prompt = cortex_system_prompt(
        "coordinator", None, today=date(2026, 10, 4), environ={}
    )
    role_text = composed_instructions(load_role("orchestrator"))
    rendered_size = len(base_prompt) + len(role_text)
    assert rendered_size <= COORDINATOR_PROMPT_BUDGET_CHARS, (
        f"coordinator prompt grew to {rendered_size} characters "
        f"(base={len(base_prompt)}, role={len(role_text)}, "
        f"budget={COORDINATOR_PROMPT_BUDGET_CHARS})"
    )


def test_consultant_has_no_project_source_or_command_tools():
    consultant_tools = _offered_tool_names(
        "worker", "worker-senior-consultant", "execution"
    )
    prohibited = {
        "project_read",
        "project_git",
        "project_write",
        "project_mkdir",
        "project_delete",
        "project_skill_link",
        "command_run",
        "outside_write",
        "outside_command",
    }
    assert not consultant_tools & prohibited, sorted(consultant_tools & prohibited)
