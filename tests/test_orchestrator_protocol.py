"""Guard the coordinator's fresh-activation bootstrap ordering contract."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_the_bound_task_is_confirmed_before_governance():
    """The ordering this has always protected -- a task binding is
    confirmed before governance is applied -- is unchanged. What changed
    (T035) is who binds: in an ACP client the session creates and binds
    the task before the coordinator's first turn, so the coordinator
    calling `create_task` itself is refused every time, and every live run
    spent its opening words explaining that refusal to the user.
    """
    text = (ROOT / "skills/orchestrator/SKILL.md").read_text()

    start = text.index("## Apply governance to decisions")
    end = text.index("\n## Durable decisions", start)
    section = text[start:end]

    assert "One session is one task" in section
    assert "never call\n`create_task`" in section
    assert "confirm the bound task" in section
    assert section.index("bound task") < section.index("`set_governance`")
    assert "never called before that binding is confirmed" in section
    assert "not a server gate or approval machine" in section


def test_harvest_remains_explicit_only_in_orchestrator_contract():
    text = (ROOT / "skills/orchestrator/SKILL.md").read_text()
    assert "Keep `cortex:knowledge-harvest` explicit-only" in text


def test_pending_workers_require_silent_bounded_waits():
    text = (ROOT / "skills/orchestrator/SKILL.md").read_text()

    start = text.index("## Honest communication")
    end = text.index("\n## Load only relevant details", start)
    section = text[start:end]

    assert "While workers are pending/unchanged, use bounded native waits silently" in section
    assert "material state transition, blocker/question, or completion" in section
