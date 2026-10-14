"""Regression: composed worker role instructions must not require
English-only `ask_question` text/options, and must carry the
selected-language exception for them.

A review found that the restored canonical
``agent-sources/worker-protocol.md`` Communication paragraph listed
"questions" among the items a worker must keep in English, with no
exception -- contradicting the per-turn ``user_question_language`` directive
(``src/cortex_runtime/acp/orchestrating_runtime.py``) that requires a
structured question's text and selectable options to use the user's
selected chat language. This test loads every actually-composed worker role
(``load_role``/``composed_instructions``/``body_portable``) plus the base
worker system prompt and asserts the conflict cannot recur.
"""

from datetime import date

from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt
from cortex_runtime.portable.roles import composed_instructions, known_role_ids, load_role

WORKER_ROLE_IDS = tuple(role_id for role_id in known_role_ids() if role_id.startswith("worker-"))


def _sources():
    base_worker_prompt = cortex_system_prompt(
        "worker", None, today=date(2026, 10, 4), environ={}, access_class="write",
    )
    sources = [("base-worker-system-prompt", base_worker_prompt)]
    for role_id in WORKER_ROLE_IDS:
        role = load_role(role_id)
        sources.append((f"{role_id}:body_portable", role.body_portable))
        sources.append((f"{role_id}:composed_instructions", composed_instructions(role)))
    return sources


def test_no_worker_instruction_requires_english_only_questions():
    for label, text in _sources():
        lowered = text.casefold()
        # The exact regression: the English-only communication list once
        # named "questions" with no carve-out for the selected-language
        # question contract.
        assert "exclusively in english: progress, questions" not in lowered, label
        if "communicate exclusively in english" in lowered:
            # Any role instruction that states the general English-only
            # communication rule must also state its question-language
            # exception in the same text, not rely only on a later,
            # lower-priority per-turn prompt to override it.
            assert "user_question_language" in text, (label, "missing selected-language exception")
            assert "not english" in lowered, (label, "missing explicit English carve-out")


def test_selected_language_question_rule_is_present_for_every_worker_role():
    seen_exception = False
    for label, text in _sources():
        lowered = text.casefold()
        if "user_question_language" in text and "not english" in lowered:
            seen_exception = True
    assert seen_exception, "no composed worker instruction states the selected-language question exception"
