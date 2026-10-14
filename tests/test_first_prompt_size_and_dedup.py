"""C-30 (consolidated audit r_9731f0fc6d64, WP9): a worker's first prompt no
longer repeats its objective as often, and the generated SKILL.md
frontmatter/footer markers never leak into delivered instructions.

Recovery capsules still carry the full objective verbatim in their own
protected fields (``own_assignment.title``/``brief``, ``runtime.nodes[].
objective`` -- see ``recovery/capsule.py`` and the pre-existing
``test_worker_own_huge_brief_stays_verbatim_never_a_null_tool_reference``);
this file only asserts that the *literal* catalog+brief tail
(``acp/orchestrating_runtime.py``'s ``_provision_and_prompt_inner``) stops
restating the title line when it adds nothing the brief does not already
carry, that a long objective's display title is cut on a word boundary
(``orchestration/controller.py``'s ``_title_from_objective``), and that
inlined shared skills (``portable/roles.py``'s ``composed_instructions``)
carry no YAML frontmatter or generated footer comment.
"""
from __future__ import annotations

import threading

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.controller import _title_from_objective
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import (
    INLINED_SKILLS,
    SHARED_ROLE_IDS,
    composed_instructions,
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
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='c30'), 'Audit remediation', 'owner')
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
    """C-30 (full fix, see r_99646044798e's own qualification contradiction:
    the narrower test above never exercised a capsule-attached delivery at
    all, so it stayed green at a ``<=4`` ceiling that no longer matched the
    stated "objective once" goal).

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
    kept verbatim only at
    ``pipeline_and_assignments.own_assignment.brief`` (see
    ``recovery.capsule._apply_inline_delivery_dedup``, and the literal-
    brief strip in ``_begin_generation_delivery`` itself)."""
    from test_context_generations import GenerationAwareSessionRuntime

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='c30-inline'), 'Audit remediation', 'owner')
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
        assert '---\nname:' not in message
        assert 'END OF COMPLETE CORTEX' not in message
    finally:
        runtime.shutdown()
