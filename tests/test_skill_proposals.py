"""Proactive skill proposals: question, approval, decline, recovery, duplicates."""
from __future__ import annotations

import json
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.agent import Agent, _PaseoQuestionElicitor
from cortex_runtime.acp.questions_store import answer, supersede_open
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration import skill_proposals
from cortex_runtime.orchestration.controller import _skill_proposal_request
from cortex_runtime.orchestration.mcp import dispatch
from cortex_runtime.orchestration.questions import pending, resume_answered
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

OPTIONS = ['Create the skill', 'Not now']


def _task(store, label):
    task = bootstrap_task(store, SourceEvent(origin='test', event_id=label), f'Goal {label}', 'owner')
    TaskMachine(store).create(task.task_id, f'Goal {label}')
    return SimpleNamespace(task_id=task.task_id, channel_id=f'coordinator-{label}', role='coordinator')


def _report(store, task_id, title, kind='report'):
    report_id = 'r_' + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute('INSERT INTO reports VALUES (?,?,?,?,?)', (report_id, task_id, kind, report_id + '.md', store._now()))
        db.execute('INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,?,?,?,?,?,?)',
                   (report_id, title, 'summary', 'worker', store._now(), 'digest', 1))
        db.commit()
    return report_id


def _tree(root):
    return sorted((str(path.relative_to(root)), path.is_symlink()) for path in root.rglob('*')
                  if '.cortex' not in path.relative_to(root).parts)


@pytest.fixture
def project(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    first = _task(store, 'one')
    reports = [_report(store, first.task_id, f'Release notes by hand {i}') for i in range(2)]
    return SimpleNamespace(root=tmp_path, store=store, channel=first, reports=reports)


def _args(project, *, key='ask-1', name='release-notes', action='create', options=None, reports=None):
    evidence = [dict(report_id=ref, case=f'Release notes assembled by hand ({index})')
                for index, ref in enumerate(reports or project.reports)]
    if options is None:
        options = OPTIONS if action == 'create' else ['Update the skill', 'Not now']
    return dict(question='Create the release-notes skill?', options=list(options), request_key=key,
                skill_proposal=dict(name=name, action=action,
                                    purpose='Assemble release notes from verified changes.', evidence=evidence))


def _decided(store, task_id):
    with store.connection() as db:
        return [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='SKILL_PROPOSAL_DECIDED' ORDER BY id", (task_id,))]


def test_proposal_is_a_durable_question_with_runtime_retained_evidence(project):
    receipt = dispatch(project.store, project.channel, 'user_question_request', _args(project))
    assert receipt['state'] == 'WAITING_USER'
    assert dispatch(project.store, project.channel, 'user_question_request', _args(project)) == receipt
    with pytest.raises(StoreError, match='request_key_conflict'):
        dispatch(project.store, project.channel, 'user_question_request', _args(project, name='other-skill'))
    saved = pending(project.store, project.channel.task_id)
    assert saved['options'] == OPTIONS
    proposal = saved['skill_proposal']
    assert (proposal['name'], proposal['action'], proposal['observed_skill_state']) == ('release-notes', 'create', 'absent')
    assert [item['report_id'] for item in proposal['evidence']] == project.reports
    assert proposal['evidence'][0]['title'] == 'Release notes by hand 0'
    assert TaskMachine(project.store).context(project.channel.task_id)['skill_proposals'][0]['status'] == 'pending'

    shown = skill_proposals.question_text(saved, 'en')['question']
    assert project.reports[0] in shown and 'Release notes assembled by hand (1)' in shown
    assert 'unless you choose the first option' in shown
    # The chat receives the evidence; the card keeps the one-line question.
    body, title = _PaseoQuestionElicitor.split_question(
        SimpleNamespace(CARD_TEXT_LIMIT=_PaseoQuestionElicitor.CARD_TEXT_LIMIT), shown)
    assert title == 'Create the release-notes skill?' and project.reports[1] in body
    assert not (project.root / '.agents').exists() and not (project.root / '.claude').exists()


@pytest.mark.parametrize('change, code', [
    (dict(options=['Yes']), 'skill_proposal_options'),
    (dict(options=['Yes', ' yes ']), 'skill_proposal_options'),
    (dict(name='Bad_Name'), 'skill_link_name_invalid'),
])
def test_invalid_proposals_ask_nothing(project, change, code):
    with pytest.raises(StoreError, match=code):
        dispatch(project.store, project.channel, 'user_question_request', _args(project, **change))
    assert TaskMachine(project.store).context(project.channel.task_id)['state'] == 'PLANNING'


def test_evidence_must_be_distinct_retained_ordinary_reports(project):
    store, channel = project.store, project.channel
    with pytest.raises(StoreError, match='invalid_arguments'):  # one case is not a repetition
        dispatch(store, channel, 'user_question_request', _args(project, reports=project.reports[:1]))
    with pytest.raises(StoreError, match='skill_proposal_evidence'):
        dispatch(store, channel, 'user_question_request', _args(project, reports=[project.reports[0]] * 2))
    with pytest.raises(StoreError, match='skill_proposal_evidence_unknown'):
        dispatch(store, channel, 'user_question_request', _args(project, reports=[project.reports[0], 'r_000000000000']))
    pipeline = _report(store, channel.task_id, 'Pipeline', kind='pipeline')
    with pytest.raises(StoreError, match='skill_proposal_evidence_unknown'):
        dispatch(store, channel, 'user_question_request', _args(project, reports=[project.reports[0], pipeline]))
    assert TaskMachine(store).context(channel.task_id)['skill_proposals'] == []


def test_action_must_match_the_project_skill_state(project):
    store, channel = project.store, project.channel
    with pytest.raises(StoreError, match='skill_proposal_missing'):
        dispatch(store, channel, 'user_question_request', _args(project, action='update'))
    skill = project.root / '.agents' / 'skills' / 'release-notes'
    skill.mkdir(parents=True)
    (skill / 'SKILL.md').write_text('---\nname: release-notes\ndescription: x\n---\n', encoding='utf-8')
    with pytest.raises(StoreError, match='skill_proposal_exists'):
        dispatch(store, channel, 'user_question_request', _args(project))
    receipt = dispatch(store, channel, 'user_question_request', _args(project, action='update', key='update'))
    assert pending(store, channel.task_id)['skill_proposal']['observed_skill_state'] == 'skill'
    assert receipt['state'] == 'WAITING_USER'


def test_workers_cannot_propose_skills(project):
    worker = SimpleNamespace(task_id=project.channel.task_id, channel_id='worker', role='worker')
    from cortex_runtime.orchestration.questions import request
    with pytest.raises(StoreError, match='skill_proposal_not_allowed'):
        request(project.store, worker, _args(project))


@pytest.mark.parametrize('reply', ['Create the skill', '  create THE skill ', '1'])
def test_only_the_approving_option_approves_and_hands_over_the_command_workflow(project, reply):
    store, channel = project.store, project.channel
    question = dispatch(store, channel, 'user_question_request', _args(project))
    assert answer(store, question['question_id'], reply, 'elicitation')
    assert resume_answered(store, channel.task_id)
    decided = _decided(store, channel.task_id)
    assert [item['decision'] for item in decided] == ['approved']
    assert decided[0]['request'].startswith('Create the project skill release-notes in .agents/skills/release-notes')

    machine = TaskMachine(store)
    context = machine.context(channel.task_id)
    assert context['state'] == 'PLANNING' and context['skill_proposals'][0]['awaiting_plan']
    instruction = _skill_proposal_request(context)
    assert '/skill-management ' + decided[0]['request'] in instruction
    assert skill_proposals.workflow_instruction('the user approved this exact skill proposal') in instruction
    # The explicit command reaches the coordinator through the same text.
    prompts = []
    agent = object.__new__(Agent)
    agent._run_prompt_body = lambda *args: prompts.append(args)
    Agent._run_skill_management(agent, 'r', 's', {'project': project.root, 'task_id': None}, 'create x', None, None)
    assert skill_proposals.workflow_instruction('the user invoked the registered command') in prompts[0][3]

    plan = dict(tasks=[dict(id='skill', role='backend-dev', objective='Create the release-notes skill',
                            dependencies=[], scope=['.'], model='auto')],
                verification=[['python3', '-B', '-m', 'pytest', '-q']])
    machine.submit_plan(channel, plan, 'plan-skill')
    context = machine.context(channel.task_id)
    assert not context['skill_proposals'][0]['awaiting_plan'] and _skill_proposal_request(context) == ''


@pytest.mark.parametrize('reply, decision', [('Not now', 'declined'), ('2', 'declined'),
                                             ('yes, but call it notes', 'unclear'), ('да', 'unclear')])
def test_decline_or_unclear_answer_leaves_the_project_unchanged(project, reply, decision):
    store, channel = project.store, project.channel
    with store.connection() as db:  # asked after verified work
        db.execute("UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id=?", (channel.task_id,)); db.commit()
    before = _tree(project.root)
    question = dispatch(store, channel, 'user_question_request', _args(project))
    assert answer(store, question['question_id'], reply, 'text')
    assert resume_answered(store, channel.task_id)
    assert [item['decision'] for item in _decided(store, channel.task_id)] == [decision]
    assert 'request' not in _decided(store, channel.task_id)[0]
    context = TaskMachine(store).context(channel.task_id)
    assert context['state'] == 'PLANNING' and not context['skill_proposals'][0]['awaiting_plan']
    instruction = _skill_proposal_request(context)
    assert 'Do not create, update or link it' in instruction and 'answer_submit' in instruction
    # The user's answer released the verified revision: a short close is accepted.
    assert TaskMachine(store).submit_answer(channel, 'Skill not created.', 'close')['state'] == 'ANSWER_READY'
    assert _tree(project.root) == before


def test_decision_survives_restart_and_is_recorded_once(project):
    store, channel = project.store, project.channel
    question = dispatch(store, channel, 'user_question_request', _args(project))
    assert answer(store, question['question_id'], 'Not now', 'text')
    # The answer is durable before the task resumes: a new process sees it
    # and duplicate checks already treat it as answered.
    restarted = Store(project.root / '.cortex', project_root=project.root)
    other = _task(restarted, 'two')
    with pytest.raises(StoreError, match='skill_proposal_already_asked'):
        dispatch(restarted, other, 'user_question_request', _args(project))
    assert TaskMachine(restarted).context(channel.task_id)['state'] == 'WAITING_USER'
    assert resume_answered(restarted, channel.task_id)
    assert not resume_answered(restarted, channel.task_id)
    assert [item['decision'] for item in _decided(restarted, channel.task_id)] == ['declined']


def test_duplicates_are_refused_across_tasks_and_within_a_task(project):
    store, channel = project.store, project.channel
    dispatch(store, channel, 'user_question_request', _args(project))
    other = _task(store, 'two')
    with pytest.raises(StoreError, match='skill_proposal_pending'):
        dispatch(store, other, 'user_question_request', _args(project, key='again'))
    with pytest.raises(StoreError, match='skill_proposal_pending'):
        dispatch(store, other, 'user_question_request', _args(project, key='again', action='update'))

    # One proposal per task, even after its answer.
    question = pending(store, channel.task_id)
    answer(store, question['question_id'], 'Create the skill', 'text')
    resume_answered(store, channel.task_id)
    with pytest.raises(StoreError, match='skill_proposal_limit'):
        dispatch(store, channel, 'user_question_request', _args(project, key='second', name='changelog'))
    # Approved once: the same name and action is not asked again anywhere.
    with pytest.raises(StoreError, match='skill_proposal_already_asked'):
        dispatch(store, other, 'user_question_request', _args(project, key='again-2'))
    assert dispatch(store, other, 'user_question_request', _args(project, key='fresh', name='changelog'))


def test_a_withdrawn_question_does_not_block_a_later_proposal(project):
    store, channel = project.store, project.channel
    dispatch(store, channel, 'user_question_request', _args(project))
    supersede_open(store, channel.task_id, 'cortex-stop')
    context = TaskMachine(store).context(channel.task_id)
    assert context['skill_proposals'][0]['status'] == 'withdrawn' and _skill_proposal_request(context) == ''
    assert _decided(store, channel.task_id) == []
    other = _task(store, 'two')
    assert dispatch(store, other, 'user_question_request', _args(project, key='later'))['state'] == 'WAITING_USER'


def test_project_skill_files_alone_never_start_a_proposal(project):
    skill = project.root / '.agents' / 'skills' / 'release-notes'
    skill.mkdir(parents=True)
    (skill / 'SKILL.md').write_text('---\nname: release-notes\ndescription: x\n---\n', encoding='utf-8')
    context = TaskMachine(project.store).context(project.channel.task_id)
    assert context['skill_proposals'] == [] and _skill_proposal_request(context) == ''
    assert _decided(project.store, project.channel.task_id) == []


@pytest.mark.parametrize('reply, expected', [('Not now', 'did not approve'), ('Create the skill', '/skill-management Create')])
def test_controller_asks_with_evidence_and_resumes_after_restart(tmp_path, reply, expected):
    """The real controller path: the proposal question is shown with the
    runtime's evidence list, the answer arrives after a restart, and the next
    planning turn carries the decision; only approval hands over the workflow."""
    import threading
    from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from test_acp_steering_and_questions import _make_runtime

    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task_id = bootstrap_task(store, SourceEvent(origin='cli', event_id='skill'), 'Write release notes.', 'owner-1').task_id
    reports = [_report(store, task_id, f'Release notes by hand {i}') for i in range(2)]
    before = _tree(tmp_path)
    planning_messages = []

    def router(store, channel, *_):
        def behavior(store, channel, _ref, message, _index):
            context = mcp_tools.dispatch(store, channel, 'task_get_context', {})
            if not context.get('skill_proposals'):
                mcp_tools.dispatch(store, channel, 'user_question_request', _args(
                    SimpleNamespace(reports=reports), key='propose'))
            else:
                planning_messages.append(message)
                mcp_tools.dispatch(store, channel, 'answer_submit', dict(text='Done.', request_key='close'))
            return PromptOutcome(StopReason.END_TURN, 'Coordinator text')
        return behavior

    first_output = []
    runtime, _engine = _make_runtime(store, router)
    try:
        runtime.start_prompt(task_id, 's1', 'Write release notes.', first_output.append, threading.Event())
    finally:
        runtime.shutdown()
    shown = '\n'.join(first_output)
    assert TaskMachine(store).context(task_id)['state'] == 'WAITING_USER'
    assert all(ref in shown for ref in reports) and 'Create the skill' in shown and 'Not now' in shown

    runtime, _engine = _make_runtime(store, router)  # a fresh process
    try:
        runtime.start_prompt(task_id, 's1', reply, lambda _: True, threading.Event())
    finally:
        runtime.shutdown()
    context = TaskMachine(store).context(task_id)
    assert context['state'] == 'COMPLETED' and len(planning_messages) == 1
    assert expected in planning_messages[0]
    assert ('read_skill' in planning_messages[0]) == (reply == 'Create the skill')
    assert len(_decided(store, task_id)) == 1
    assert _tree(tmp_path) == before


@pytest.mark.parametrize('options', [
    ['Not now', 'Create the skill'],
    ['Decline creating the skill', 'Approve creating the skill'],
    ['Do not create the skill', 'Create the skill'],
    ['Не сейчас', 'Создать навык'],
    ['Approve creating the skill', 'Not now'],
])
def test_reversed_or_misleading_labels_are_refused_before_persistence(project, options):
    with pytest.raises(StoreError, match='skill_proposal_options'):
        dispatch(project.store, project.channel, 'user_question_request',
                 _args(project, options=options))
    assert TaskMachine(project.store).context(project.channel.task_id)['state'] == 'PLANNING'
    assert TaskMachine(project.store).context(project.channel.task_id)['skill_proposals'] == []


@pytest.mark.parametrize('options, reply, decision', [
    (['Create the skill', 'Not now'], 'Create the skill', 'approved'),
    (['Create the skill', 'Not now'], '1', 'approved'),
    (['Create the skill', 'Not now'], 'Not now', 'declined'),
    (['Создать навык', 'Не сейчас'], 'Создать навык', 'approved'),
    (['Создать навык', 'Не сейчас'], '2', 'declined'),
    (['Создать навык', 'Не сейчас'], 'да, но позже', 'unclear'),
])
def test_action_matched_labels_bind_displayed_and_text_answers(project, options, reply, decision):
    store, channel = project.store, project.channel
    question = dispatch(store, channel, 'user_question_request', _args(project, options=options))
    saved = pending(store, channel.task_id)
    assert saved['options'] == options
    with store.connection() as db:
        saved_options = json.loads(db.execute('SELECT options_json FROM pending_questions WHERE id=?',
                                              (question['question_id'],)).fetchone()[0])
    assert saved_options == options
    assert answer(store, question['question_id'], reply, 'text')
    assert resume_answered(store, channel.task_id)
    assert _decided(store, channel.task_id)[0]['decision'] == decision


@pytest.mark.parametrize('options, reply', [
    (['Not now', 'Create the skill'], 'Not now'),
    (['Not now', 'Create the skill'], '1'),
    (['Create the skill', 'Not now'], '1'),
    (['Approve creating the skill', 'Not now'], '1'),
])
def test_unsafe_persisted_question_fails_closed_after_restart(project, options, reply):
    store, channel = project.store, project.channel
    question = dispatch(store, channel, 'user_question_request', _args(project))
    # Model a question saved by an older runtime. For the third case, an
    # update question has a creation label, which must not authorize update.
    action = 'update' if options == ['Create the skill', 'Not now'] else 'create'
    with store.connection() as db:
        row = db.execute(
            "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'",
            (channel.task_id,)).fetchone()
        payload = json.loads(row['payload_json'])
        payload['options'] = options
        payload['skill_proposal']['action'] = action
        db.execute('UPDATE runtime_events SET payload_json=? WHERE id=?',
                   (json.dumps(payload), row['id']))
        db.execute('UPDATE pending_questions SET options_json=? WHERE id=?',
                   (json.dumps(options), question['question_id']))
        db.commit()
    assert answer(store, question['question_id'], reply, 'text')
    restarted = Store(project.root / '.cortex', project_root=project.root)
    assert resume_answered(restarted, channel.task_id)
    assert not resume_answered(restarted, channel.task_id)
    decided = _decided(restarted, channel.task_id)
    assert [item['decision'] for item in decided] == ['unclear']
    assert 'request' not in decided[0]
    assert TaskMachine(restarted).context(channel.task_id)['skill_proposals'][0]['status'] == 'unclear'


@pytest.mark.parametrize('options, reply, expected', [
    (['Update the skill', 'Not now'], '1', 'approved'),
    (['Обновить навык', 'Не сейчас'], 'Не сейчас', 'declined'),
])
def test_update_options_match_action_and_decision(project, options, reply, expected):
    skill = project.root / '.agents' / 'skills' / 'release-notes'
    skill.mkdir(parents=True)
    (skill / 'SKILL.md').write_text('---\nname: release-notes\ndescription: x\n---\n', encoding='utf-8')
    store, channel = project.store, project.channel
    question = dispatch(store, channel, 'user_question_request',
                        _args(project, action='update', options=options))
    assert pending(store, channel.task_id)['options'] == options
    assert answer(store, question['question_id'], reply, 'text')
    assert resume_answered(store, channel.task_id)
    assert _decided(store, channel.task_id)[0]['decision'] == expected
