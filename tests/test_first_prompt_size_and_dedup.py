"""A worker's first prompt states its objective exactly once, in its
runtime-authored assignment instructions, and the generated SKILL.md
frontmatter/footer markers never leak into delivered instructions.

A capsule fetched through ``read_capsule`` still carries the full objective
verbatim in its own protected fields (``own_assignment.title``/``brief``,
``runtime.nodes[].objective`` -- see ``recovery/capsule.py`` and
``test_worker_own_huge_brief_stays_verbatim_never_a_null_tool_reference``);
this file asserts that the literal catalog+brief tail
(``acp/orchestrating_runtime.py``'s ``_provision_and_prompt_inner``) stops
restating the title line when it adds nothing the brief does not already
carry, that a capsule rendered in front of that brief references it instead
of repeating it, that a long objective's display title is cut on a word
boundary (``orchestration/controller.py``'s ``_title_from_objective``), and
that inlined shared skills (``portable/roles.py``'s ``composed_instructions``)
carry no YAML frontmatter or generated footer comment.
"""
from __future__ import annotations

import threading
from datetime import date

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.controller import _title_from_objective
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt, prompt_access_class
from cortex_runtime.portable.roles import (
    INLINED_SKILLS,
    SHARED_ROLE_IDS,
    composed_instructions,
    known_role_ids,
    load_role,
)
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')


def test_title_from_objective_cuts_at_a_word_boundary_not_mid_word():
    objective = ("Fix the duplicate objective defect everywhere it hides " * 6).strip()
    assert len(objective) > 200
    title = _title_from_objective(objective)
    assert len(title) <= 200
    assert objective.startswith(title)
    # The old hard ``[:200]`` slice could land inside a word; the character
    # right after the kept prefix must now be a word boundary (end of
    # string or a space), never a letter continuing a cut-off word.
    next_char = objective[len(title):len(title) + 1]
    assert next_char in ('', ' ')
    assert not title.endswith(' ')


def test_title_from_objective_keeps_short_objectives_whole():
    objective = "Short objective."
    assert _title_from_objective(objective) == objective


def test_composed_instructions_strip_generated_skill_frontmatter_and_footer():
    for role_id in ("worker-backend-dev", "orchestrator"):
        role = load_role(role_id)
        assert not role.body_portable.lstrip().startswith("---\n"), role_id
        assert "END OF COMPLETE CORTEX" not in role.body_portable, role_id
    for skill_ids in INLINED_SKILLS.values():
        for skill_id in skill_ids:
            if skill_id not in SHARED_ROLE_IDS:
                continue  # a reference id (e.g. report-publication) has no frontmatter to begin with
            skill = load_role(skill_id)
            assert not skill.body_portable.lstrip().startswith("---\n"), skill_id
    worker_instructions = composed_instructions(load_role("worker-backend-dev"))
    assert "---\nname:" not in worker_instructions
    assert "END OF COMPLETE CORTEX" not in worker_instructions
    coordinator_instructions = composed_instructions(load_role("orchestrator"))
    assert "---\nname:" not in coordinator_instructions
    assert "END OF COMPLETE CORTEX" not in coordinator_instructions


def test_worker_first_prompt_repeats_the_objective_exactly_once(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='objective-once'), 'Audit remediation', 'owner')
    objective = 'Deliver the objective once per worker first prompt in this regression test.'
    captured: dict = {}

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == 'coordinator':
                context = mcp_tools.dispatch(store, channel, 'task_get_context', {})
                if context['state'] == 'PLANNING':
                    proposed = dict(
                        tasks=[dict(id='N0', role='backend-dev', objective=objective,
                                    dependencies=[], scope=['src/part0/**'], model='auto')],
                        verification=[['python3', '-B', '-c', 'print("ok")']],
                    )
                    mcp_tools.dispatch(store, channel, 'plan_submit',
                                       dict(plan=proposed, request_key='plan-' + str(context['revision'])))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit',
                                       dict(text='Result reviewed and verified.'))
                return PromptOutcome(StopReason.END_TURN, '')
            captured['message'] = message
            mcp_tools.dispatch(store, channel, 'result_submit', dict(
                report='# Observed\n\nDone.', report_title='Observed', summary='Done.',
                checks=[], request_key='result'))
            return PromptOutcome(StopReason.END_TURN, 'Finished.')
        return behavior

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True,
    )
    try:
        cancel_event = threading.Event()
        runtime.start_prompt(task.task_id, 's1', 'Audit remediation', lambda text: True, cancel_event)
        assert 'message' in captured, 'the worker was never prompted'
        message = captured['message']
        # This fake session runtime has no ``pending_delivery`` at all (see
        # ``orchestration_fakes.FakeSessionRuntime``'s own class
        # docstring), so this exact scenario never gets a recovery capsule
        # attached -- the literal brief JSON's own ``objective`` field is
        # the only copy. The scenario that used to repeat the objective up
        # to 5 times (capsule own_assignment.title, capsule
        # own_assignment.brief, capsule runtime.nodes[].objective, the
        # literal title line, and this literal brief's objective field)
        # requires a capsule actually attached to this exact delivery --
        # see
        # ``test_worker_first_prompt_with_inline_capsule_delivers_the_objective_exactly_once``
        # below, which exercises that real, capsule-attached path end to
        # end and asserts the same exact-once contract there.
        assert message.count(objective) == 1, message.count(objective)
        assert '---\nname:' not in message
        assert 'END OF COMPLETE CORTEX' not in message
    finally:
        runtime.shutdown()


def test_worker_first_prompt_with_inline_capsule_delivers_the_objective_exactly_once(tmp_path):
    """The narrower test above never exercises a capsule-attached delivery.

    Every real ``session_open``/post-compaction-continuation whose engine
    adapter supports delivery tracking (``delegation.live_adapters.
    EngineSessionManagerAdapter`` -- every production engine) has this
    exact generation's recovery capsule prefixed onto the worker's first
    prompt (see ``acp.orchestrating_runtime._begin_generation_delivery``).
    Reuse ``test_context_generations.GenerationAwareSessionRuntime`` (the
    real ``delegation.generations``/``deliveries`` ledger wired behind the
    same in-process fake, never a reimplementation of that tracking) to
    exercise that real path, with the worst case this module's own prior
    test used (a short objective, so its display ``title`` -- see
    ``orchestration.controller._title_from_objective`` -- equals the
    objective exactly, the hardest case to collapse to one copy): the
    delivered message still carries the full objective exactly once --
    in the literal brief of the worker's own assignment instructions, never
    only inside a data-labelled capsule block (see
    ``recovery.capsule.reference_brief_delivered_inline``)."""
    from test_context_generations import GenerationAwareSessionRuntime

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='objective-once-inline'), 'Audit remediation', 'owner')
    objective = 'Deliver the objective once per worker first prompt, even with a capsule attached.'
    captured: dict = {}

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == 'coordinator':
                context = mcp_tools.dispatch(store, channel, 'task_get_context', {})
                if context['state'] == 'PLANNING':
                    proposed = dict(
                        tasks=[dict(id='N0', role='backend-dev', objective=objective,
                                    dependencies=[], scope=['src/part0/**'], model='auto')],
                        verification=[['python3', '-B', '-c', 'print("ok")']],
                    )
                    mcp_tools.dispatch(store, channel, 'plan_submit',
                                       dict(plan=proposed, request_key='plan-' + str(context['revision'])))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit',
                                       dict(text='Result reviewed and verified.'))
                return PromptOutcome(StopReason.END_TURN, '')
            captured['message'] = message
            mcp_tools.dispatch(store, channel, 'result_submit', dict(
                report='# Observed\n\nDone.', report_title='Observed', summary='Done.',
                checks=[], request_key='result'))
            return PromptOutcome(StopReason.END_TURN, 'Finished.')
        return behavior

    engine = GenerationAwareSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(), allow_unqualified_delivery=True,
    )
    try:
        cancel_event = threading.Event()
        runtime.start_prompt(task.task_id, 's1', 'Audit remediation', lambda text: True, cancel_event)
        assert 'message' in captured, 'the worker was never prompted'
        message = captured['message']
        # Confirms the capsule really was attached to this exact delivery
        # (otherwise this test would silently degrade into a duplicate of
        # the no-capsule scenario above and prove nothing new).
        assert 'Cortex recovery capsule' in message
        assert message.count(objective) == 1, message.count(objective)
        # The one copy is the assignment's own instruction text after the
        # capsule, never inside a data-labelled capsule block.
        at = message.index(objective)
        assert at > message.rindex('CORTEX_CAPSULE_DATA_END'), 'objective only inside the capsule'
        assert 'Delivered once, verbatim, as this assignment' in message[:at]
        assert '---\nname:' not in message
        assert 'END OF COMPLETE CORTEX' not in message
    finally:
        runtime.shutdown()


def _worker_capsule(brief):
    return dict(
        recipient=dict(role='worker', assignment_ref='a', attempt_id='xa'),
        sections=dict(pipeline_and_assignments=dict(
            own_assignment=dict(title='Do it', brief=brief, acceptance='Done'),
            runtime=dict(nodes=[dict(node_id='N0', objective='Do it')]),
        )),
    )


def test_a_capsule_without_the_brief_in_its_message_keeps_the_full_brief():
    """A continuation prompt that does not carry the literal brief keeps the
    capsule's own verbatim brief: the objective is never lost."""
    from cortex_runtime.recovery.capsule import reference_brief_delivered_inline
    capsule = _worker_capsule('{"objective": "Do it"}')
    assert reference_brief_delivered_inline(capsule, 'Continue the work.') is capsule
    referenced = reference_brief_delivered_inline(capsule, 'header\n\n{"objective": "Do it"}\n\nAcceptance: Done')
    own = referenced['sections']['pipeline_and_assignments']['own_assignment']
    assert own['brief']['status'] == 'referenced' and own['title']['status'] == 'referenced'
    assert own['acceptance'] == 'Done'
    assert referenced['sections']['pipeline_and_assignments']['runtime']['nodes'][0]['objective']['status'] == 'referenced'
    # Rendering only: the built capsule itself is unchanged.
    assert capsule['sections']['pipeline_and_assignments']['own_assignment']['brief'] == '{"objective": "Do it"}'
    coordinator = dict(_worker_capsule('x'), recipient=dict(role='coordinator'))
    assert reference_brief_delivered_inline(coordinator, 'x') is coordinator


# Edition-2 base prompt ceilings, the analogue of the edition-1 ceilings in
# ``test_prompt_drift.test_s3_rendered_size_ceilings_and_shared_report_ownership``
# (19,800 coordinator / 17,000 consultant / 25,800 other workers for base +
# composed role text). Edition 2 adds its new sections without yet removing the
# skill text they overlap, so its ceilings are higher. Measured on 2026-10-15
# (base prompt rendered with no project skills and today=2026-10-04):
# coordinator 20,985 (7,678 base), senior consultant 17,343 (7,318 base),
# largest other worker 27,658 (worker-qa-engineer, 9,568 base).
_EDITION_2_TOTAL_BUDGET = {'coordinator': 21_200, 'consultant': 17_600, 'worker': 27_900}
_EDITION_2_BASE_BUDGET = {'coordinator': 7_900, 'consultant': 7_500, 'worker': 9_800}


def test_edition_2_rendered_size_ceilings():
    role_ids = ('orchestrator', *(role_id for role_id in known_role_ids() if role_id.startswith('worker-')))
    assert len(role_ids) == 24  # the coordinator and all 23 worker profiles
    for role_id in role_ids:
        kind = 'coordinator' if role_id == 'orchestrator' else 'worker'
        access_class = prompt_access_class(kind, role_id)
        budget_key = 'coordinator' if kind == 'coordinator' else 'consultant' if access_class == 'consultant' else 'worker'
        role = composed_instructions(load_role(role_id))
        for native_tools in (False, True):
            base = cortex_system_prompt(kind, None, today=date(2026, 10, 4), environ={}, access_class=access_class,
                                        native_tools=native_tools, edition=2)
            assert base.splitlines()[4] == '# How to work', role_id
            assert len(base) <= _EDITION_2_BASE_BUDGET[budget_key], (role_id, native_tools, len(base))
            assert len(role) + len(base) <= _EDITION_2_TOTAL_BUDGET[budget_key], (
                role_id, native_tools, len(role) + len(base))
