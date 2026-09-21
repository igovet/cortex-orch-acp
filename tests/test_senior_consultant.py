"""Current ACP consultant profile and report-only boundary regressions."""
import json

from cortex_package import PLUGIN


def test_consultant_is_the_opt_in_twenty_third_generated_profile():
    profiles = json.loads((PLUGIN / "profiles.json").read_text())["profiles"]
    consultant = next(item for item in profiles if item["name"] == "senior_consultant")
    assert len(profiles) == 23
    assert consultant["report_template"] == "general"
    assert (PLUGIN / "agents/senior-consultant.toml").is_file()
    skill = (PLUGIN / "skills/worker-senior-consultant/SKILL.md").read_text()
    for phrase in (
        "exact selected published report IDs",
        "read each explicitly selected report through bounded pages",
        "exact data request, and recommended profile",
        "never repeat an unchanged packet",
        "do not edit it",
    ):
        assert phrase in skill
    assert "page limit" not in skill


def test_consultant_has_no_general_worker_execution_permissions():
    skill = (PLUGIN / "skills/worker-senior-consultant/SKILL.md").read_text()
    assert "Do not run commands, tests or project checks" in skill
    assert "You may investigate, implement" not in skill
    assert "Your only write is your own server-issued unpublished report draft" in skill
