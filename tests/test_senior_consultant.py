"""Current ACP consultant profile and report-only boundary regressions."""
import json

from cortex_package import PLUGIN


def test_consultant_is_the_opt_in_twenty_third_generated_profile():
    profiles = json.loads((PLUGIN / "profiles.json").read_text())["profiles"]
    consultant = next(item for item in profiles if item["name"] == "senior_consultant")
    assert len(profiles) == 23
    assert consultant["report_template"] == "general"
    assert (PLUGIN / "skills/worker-senior-consultant/SKILL.md").is_file()
    skill = (PLUGIN / "skills/worker-senior-consultant/SKILL.md").read_text()
    for phrase in (
        "exact selected published report IDs",
        "read each explicitly selected report through bounded pages",
        "exact data request, and recommended profile",
        "do not edit it",
        "Communicate exclusively in English",
    ):
        assert phrase in skill
    assert "page limit" not in skill
    # Whether to consult again is the escalator's decision, not the consultant's.
    assert "never repeat an unchanged packet" not in skill
    assert "deeper model" not in skill
    reference = (PLUGIN / "skills/orchestrator/references/senior-consultation.md").read_text()
    assert "never repeat an unchanged packet" in reference


def test_consultant_has_no_general_worker_execution_permissions():
    skill = (PLUGIN / "skills/worker-senior-consultant/SKILL.md").read_text()
    assert "Do not run commands, tests or project checks" in skill
    assert "You may investigate, implement" not in skill
    from cortex_runtime.orchestration.gateway import READ_ONLY
    assert 'worker-senior-consultant' in READ_ONLY


def test_consultant_is_offered_reports_but_no_project_source_or_commands():
    """The consultant works only from its consultation packet and retained
    reports (owner decision, 2026-10-04)."""
    from cortex_runtime.channeled_tools import channeled_base_tool_names
    from cortex_runtime.delegation import mcp_tools
    def offered(role_id):
        tools = mcp_tools.tool_definitions("worker", role_id=role_id, stage="execution")
        return {tool["name"] for tool in tools} | set(channeled_base_tool_names())
    consultant = offered("worker-senior-consultant")
    assert not consultant & {"project_read", "project_git", "command_run", "project_write", "project_mkdir",
                             "project_delete", "project_skill_link", "outside_write", "outside_command"}
    assert {"read_report", "list_reports", "list_sources", "read_source", "create_draft",
            "write_report", "result_submit"} <= consultant
    assert {"project_read", "project_git", "command_run"} <= offered("worker-architect")


def test_consultant_protocol_limits_evidence_to_packet_and_retained_reports():
    skill = (PLUGIN / "skills/worker-senior-consultant/SKILL.md").read_text()
    text = " ".join(skill.split())
    assert "## Evidence limits" in skill
    assert "Do not read project source" in text
    assert "Run no commands, tests, installs, authorized calls or project checks" in text
    assert "state what those materials cannot establish" in text
    assert "do not perform the check yourself" in text
    for stale in (
        "## Evidence and verification", "read the current source directly",
        "before nontrivial changes", "Verify every behavior your assignment changes",
        "the tests you added or modified", "through the advertised skill reader",
    ):
        assert stale not in text
