"""T024: recovery.presentation -- budget-selected (never capability-labeled)
compact role/assignment presentation."""
from __future__ import annotations

from cortex_runtime.recovery.presentation import select_presentation
from cortex_runtime.recovery.token_budget import estimate_tokens_for_text

ROLE_TEXT = (
    "# Worker role\n\n"
    "This role explains at length the general philosophy of careful, thoughtful work "
    "and why it matters to the team and the project as a whole, in flowery prose that "
    "goes on for a while without stating any single exact rule.\n\n"
    "You must never delete another assignment's files.\n"
    "- Always run the test suite before finishing.\n"
    "Some more unrelated narrative filler text that does not state a rule at all.\n"
)
BRIEF_TEXT = "Implement the login page. Do not change the database schema."
ACCEPTANCE_TEXT = "Acceptance: all tests pass and the login page renders."


def test_full_presentation_used_when_it_fits_budget():
    full_cost = estimate_tokens_for_text("\n".join((ROLE_TEXT, BRIEF_TEXT, ACCEPTANCE_TEXT)))
    presentation = select_presentation(
        role_text=ROLE_TEXT, brief_text=BRIEF_TEXT, acceptance_text=ACCEPTANCE_TEXT,
        remaining_budget_tokens=full_cost + 10,
    )
    assert presentation.mode == "full"
    assert presentation.role_text == ROLE_TEXT


def test_compact_presentation_selected_purely_by_budget_never_by_model_name():
    presentation = select_presentation(
        role_text=ROLE_TEXT, brief_text=BRIEF_TEXT, acceptance_text=ACCEPTANCE_TEXT,
        remaining_budget_tokens=5,
    )
    assert presentation.mode == "compact"


def test_compact_presentation_preserves_critical_rules_verbatim():
    presentation = select_presentation(
        role_text=ROLE_TEXT, brief_text=BRIEF_TEXT, acceptance_text=ACCEPTANCE_TEXT,
        remaining_budget_tokens=5,
    )
    assert "You must never delete another assignment's files." in presentation.role_text
    assert "Always run the test suite before finishing." in presentation.role_text
    assert "flowery prose" not in presentation.role_text
    assert "unrelated narrative filler" not in presentation.role_text


def test_compact_presentation_never_touches_acceptance_text():
    presentation = select_presentation(
        role_text=ROLE_TEXT, brief_text=BRIEF_TEXT, acceptance_text=ACCEPTANCE_TEXT,
        remaining_budget_tokens=5,
    )
    assert presentation.acceptance_text == ACCEPTANCE_TEXT


def test_compact_presentation_keeps_brief_constraint_line():
    presentation = select_presentation(
        role_text=ROLE_TEXT, brief_text=BRIEF_TEXT, acceptance_text=ACCEPTANCE_TEXT,
        remaining_budget_tokens=5,
    )
    assert "Do not change the database schema." in presentation.brief_text
