from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_consultation_is_conditionally_loaded_and_preserves_report_only_scope():
    root = ROOT / "skills/orchestrator"
    entry = (root / "SKILL.md").read_text()
    policy = (root / "references/senior-consultation.md").read_text()
    assert '(references/senior-consultation.md)' in entry
    assert 'concrete unresolved uncertainty' in policy
    assert 'selected published reports' in policy
    assert 'does not transfer' in policy
    assert 'repeat consultation only for changed evidence' in policy


def test_documented_overlay_has_negative_controls_and_no_runtime_gate():
    text = (ROOT / "docs/project/quality-evaluation.md").read_text()
    section = text.split("## Risk-triggered consultation overlay", 1)[1]
    assert "three transition families" in section
    assert "negative controls" in section
    assert "unavailable values\nremain null" in section
    assert "never a runtime gate, server decision or acceptance rule" in section


def test_the_advisory_lane_is_no_longer_instructed():
    """Removed on the user's decision: "мы его больше не будем использовать".

    The Advisory Lane told the coordinator to consult an `mcp-rubber-duck`
    server before an expensive assignment. Its instruction file and the
    role's link to it are gone, so the coordinator is never told to reach
    for that tool. The runtime module and its documentation still exist and
    are untouched -- removing a feature is a separate decision from
    removing an instruction.
    """
    root = ROOT / "skills/orchestrator"
    assert not (root / "references/consultation-routing.md").exists()
    entry = (root / "SKILL.md").read_text()
    assert "consultation-routing" not in entry
    assert "ask_duck" not in entry and "rubber" not in entry.lower()
