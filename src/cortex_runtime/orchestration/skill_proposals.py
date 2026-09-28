"""Proactive project-skill proposals on the durable user-question protocol.

The coordinator may notice that retained reports show the same procedure more
than once and propose a reusable project skill. It never acts on that idea by
itself: it asks through ``user_question_request`` with a ``skill_proposal``
and exactly two options (approve, decline). The runtime

- validates the proposal before the question exists: a skill name, an action
  (``create``/``update``) that matches the current ``.agents/skills/<name>``
  state, and 2-5 distinct ordinary reports of this project store as the
  repeated cases, each retained as a bounded snapshot (id, task, title);
- refuses duplicates: an open proposal for the same name anywhere in the
  project, an already answered proposal with the same name and action, or a
  second proposal in the same task;
- accepts only explicit, action-matched approval/decline label pairs in
  supported user languages; ambiguous or legacy unsafe labels never approve;
- derives the decision only from the exact recorded answer: the validated
  approval label (or its displayed number) approves, and anything else is
  declined or ``unclear``;
- records ``SKILL_PROPOSAL_DECIDED`` in the same transaction that resumes the
  task after the answer, so a restart either sees both or neither.

Nothing here writes the project. An approved proposal hands the coordinator
the same ``skill-management`` workflow instruction the explicit
``/skill-management <request>`` command uses; a decline or unclear answer
leaves the project as it was. Files alone never start this path: a proposal
needs retained report evidence and a real user answer.
"""
import json
import os

from ..contracts import StoreError
from .skill_links import check_name

MIN_EVIDENCE = 2
MAX_EVIDENCE = 5
_TITLE_LIMIT = 200
DECISIONS = ('approved', 'declined', 'unclear')

# Only these displayed labels may authorize a skill change.
# The existing English creation pair remains valid for older saved questions.
_SAFE_OPTION_PAIRS = {
    'create': (
        ('Create the skill', 'Not now'),
        ('Создать навык', 'Не сейчас'),
        ('Approve creating the skill', 'Decline creating the skill'),
        ('Одобрить создание навыка', 'Отклонить создание навыка'),
    ),
    'update': (
        ('Update the skill', 'Not now'),
        ('Обновить навык', 'Не сейчас'),
        ('Approve updating the skill', 'Decline updating the skill'),
        ('Одобрить обновление навыка', 'Отклонить обновление навыка'),
    ),
}


def workflow_instruction(source):
    """The one activation text for the skill-management workflow.

    ``source`` names what authorized it: the explicit command or an approved
    proposal. Both paths must reach the coordinator through this function."""
    return ("Read the coordinator-only shared skill skill-management using read_skill before planning. "
            "Treat the supplied request as the complete skill-management request. "
            f"This workflow is active only because {source}; ordinary prose does not activate it.")


def normalized(text):
    return ' '.join(str(text).split()).casefold()


def _safe_options(options, action):
    """Match a complete, known approval/decline pair before interpreting 1/2."""
    if not isinstance(options, (list, tuple)) or len(options) != 2:
        return False
    pair = tuple(normalized(option) for option in options)
    return pair in {tuple(normalized(label) for label in safe)
                    for safe in _SAFE_OPTION_PAIRS.get(action, ())}


def decide(options, answer_text, action):
    """Only a known, action-matched displayed approval can approve.

    Old persisted questions with arbitrary labels fail closed after restart,
    including numeric replies to a misleading first option.
    """
    if not _safe_options(options, action):
        return 'unclear'
    answer = normalized(answer_text or '')
    if answer and answer in (normalized(options[0]), '1'):
        return 'approved'
    if answer and answer in (normalized(options[1]), '2'):
        return 'declined'
    return 'unclear'


def request_text(proposal):
    """The skill-management request an approval stands for."""
    verb = 'Create' if proposal['action'] == 'create' else 'Update'
    return (f"{verb} the project skill {proposal['name']} in .agents/skills/{proposal['name']} "
            f"and expose it to Claude through project_skill_link. Purpose: {proposal['purpose']}")


def _skill_state(project_root, name):
    """Read-only observation of ``.agents/skills/<name>``: ``absent``,
    ``skill`` (real directory with a regular SKILL.md) or ``other``."""
    from .skill_links import _DIR_FLAGS, _verify_source
    path = os.path.join(str(project_root), '.agents', 'skills', name)
    if not os.path.lexists(path):
        return 'absent'
    root_fd = os.open(str(project_root), _DIR_FLAGS)
    try:
        _verify_source(root_fd, name)
        return 'skill'
    except (StoreError, OSError):
        return 'other'
    finally:
        os.close(root_fd)


def _status(db, question_id, options, action):
    row = db.execute('SELECT status, answer_text FROM pending_questions WHERE id=?', (question_id,)).fetchone()
    if row is None or row['status'] in ('superseded', 'cancelled'):
        return 'withdrawn'
    if row['status'] == 'open':
        return 'pending'
    return decide(options, row['answer_text'], action)


def _asked(db, *, name=None, task_id=None):
    clauses, values = ["type='USER_QUESTION_REQUESTED'", "json_extract(payload_json,'$.skill_proposal') IS NOT NULL"], []
    if name is not None:
        clauses.append("json_extract(payload_json,'$.skill_proposal.name')=?"); values.append(name)
    if task_id is not None:
        clauses.append('task_id=?'); values.append(task_id)
    rows = db.execute('SELECT id, task_id, payload_json FROM runtime_events WHERE ' + ' AND '.join(clauses) + ' ORDER BY id',
                      values).fetchall()
    for row in rows:
        payload = json.loads(row['payload_json'])
        yield row['id'], row['task_id'], payload, _status(db, payload['question_id'], payload['options'], payload['skill_proposal']['action'])


def validate(db, project_root, channel, args):
    """Check one proposal inside the question transaction; return the
    retained proposal record. Raises before anything is recorded."""
    proposal = args['skill_proposal']
    if channel.role != 'coordinator':
        raise StoreError('skill_proposal_not_allowed', 'skill_proposal', channel.role, 'the coordinator')
    if project_root is None:
        raise StoreError('skill_proposal_project_unknown', 'skill_proposal', 'no project root',
                         'a project-bound runtime store')
    options = args.get('options') or []
    if not _safe_options(options, proposal['action']):
        raise StoreError('skill_proposal_options', 'options', f'{len(options)} options',
                         'an exact, action-matched approval/decline pair',
                         'For create use [Create the skill, Not now] or [Создать навык, Не сейчас]; '
                         'for update use [Update the skill, Not now] or [Обновить навык, Не сейчас].')
    name = check_name(proposal['name'])
    refs = [item['report_id'] for item in proposal['evidence']]
    if len(set(refs)) != len(refs) or not MIN_EVIDENCE <= len(refs) <= MAX_EVIDENCE:
        raise StoreError('skill_proposal_evidence', 'skill_proposal.evidence', f'{len(set(refs))} distinct reports',
                         f'{MIN_EVIDENCE} to {MAX_EVIDENCE} distinct report ids, one per repeated case',
                         'Cite each repeated case once by the retained report that shows it.')
    evidence = []
    for item in proposal['evidence']:
        report = db.execute(
            'SELECT r.id, r.task_id, (SELECT title FROM editions e WHERE e.report_id=r.id ORDER BY sequence DESC LIMIT 1) AS title '
            "FROM reports r WHERE r.id=? AND r.kind='report'", (item['report_id'],)).fetchone()
        if report is None:
            raise StoreError('skill_proposal_evidence_unknown', 'skill_proposal.evidence.report_id', item['report_id'],
                             'an ordinary published report of this project',
                             'Copy report ids from list_reports or a worker result; a pipeline or source is not a case.')
        evidence.append(dict(report_id=report['id'], task_id=report['task_id'],
                             title=(report['title'] or '')[:_TITLE_LIMIT], case=item['case']))
    for _event, task_id, payload, status in _asked(db, task_id=channel.task_id):
        if status != 'withdrawn':
            raise StoreError('skill_proposal_limit', 'skill_proposal', payload['skill_proposal']['name'],
                             'at most one skill proposal per task',
                             'Do not propose another skill in this task; the user can run /skill-management.')
    for _event, task_id, payload, status in _asked(db, name=name):
        if status == 'pending':
            raise StoreError('skill_proposal_pending', 'skill_proposal.name', name, 'no open proposal for this skill',
                             f'Task {task_id} already asks the user about it; do not ask again.')
        if status != 'withdrawn' and payload['skill_proposal']['action'] == proposal['action']:
            raise StoreError('skill_proposal_already_asked', 'skill_proposal.name', name,
                             f'a skill the user was not already asked to {proposal["action"]}',
                             f'The user already answered ({status}); do not ask again. They can run /skill-management.')
    state = _skill_state(project_root, name)
    if proposal['action'] == 'create' and state != 'absent':
        raise StoreError('skill_proposal_exists', 'skill_proposal.name', name, 'no entry at .agents/skills/' + name,
                         'The skill already exists: propose action update, or leave it.')
    if proposal['action'] == 'update' and state != 'skill':
        raise StoreError('skill_proposal_missing', 'skill_proposal.name', name,
                         f'a real .agents/skills/{name} directory with SKILL.md', 'Propose action create instead.')
    return dict(name=name, action=proposal['action'], purpose=proposal['purpose'], evidence=evidence,
                observed_skill_state=state)


def record_decision(db, task_id, saved, answer_text):
    """Inside ``resume_answered``'s transaction: record the decision of an
    answered skill-proposal question. No project change happens here."""
    if not saved.get('skill_proposal'):
        return None
    from .state import event
    proposal = saved['skill_proposal']
    decision = decide(saved['options'], answer_text, proposal['action'])
    payload = dict(question_id=saved['question_id'], name=proposal['name'], action=proposal['action'],
                   decision=decision, revision=saved.get('revision'))
    if decision == 'approved':
        payload['request'] = request_text(proposal)
    event(db, task_id, 'SKILL_PROPOSAL_DECIDED', payload)
    return decision


def task_proposals(db, task_id):
    """This task's proposals with their current status, for the coordinator."""
    decided = {json.loads(row[0])['question_id']: row[1] for row in db.execute(
        "SELECT payload_json, id FROM runtime_events WHERE task_id=? AND type='SKILL_PROPOSAL_DECIDED'", (task_id,))}
    planned = db.execute("SELECT MAX(id) FROM runtime_events WHERE task_id=? AND type='PLAN_ACCEPTED'",
                         (task_id,)).fetchone()[0]
    items = []
    for _event, _task, payload, status in _asked(db, task_id=task_id):
        proposal = payload['skill_proposal']
        decided_at = decided.get(payload['question_id'])
        items.append(dict(question_id=payload['question_id'], name=proposal['name'], action=proposal['action'],
                          status=status, evidence=[e['report_id'] for e in proposal['evidence']],
                          request=request_text(proposal) if status == 'approved' else None,
                          awaiting_plan=bool(decided_at) and status == 'approved' and (planned is None or planned < decided_at)))
    return items


def question_text(question, language):
    """The asked question with the runtime's own list of the retained
    evidence first, so the explanation never rests on model prose alone."""
    from ..acp import messages
    proposal = question['skill_proposal']
    lines = [messages.render('skill_proposal_evidence', language, name=proposal['name'], action=proposal['action'])]
    for item in proposal['evidence']:
        title = f" {item['title']}" if item['title'] else ''
        lines.append(f"- `{item['report_id']}`{title}: {item['case']}")
    lines.append(messages.render('skill_proposal_note', language))
    return dict(question, question='\n'.join(lines) + '\n\n' + question['question'])


def planning_instruction(context):
    """What the next planning turn must do with this task's latest decided
    proposal; empty when there is none."""
    items = [item for item in context.get('skill_proposals') or () if item['status'] in DECISIONS]
    if not items:
        return ''
    latest = items[-1]
    if latest['awaiting_plan']:
        return ('The user approved the proposed project skill through the runtime question. That is the same '
                f'authorization as the explicit command /skill-management {latest["request"]} '
                + workflow_instruction('the user approved this exact skill proposal')
                + ' Plan only this skill work. ')
    if latest['status'] in ('declined', 'unclear'):
        return (f'The user did not approve the proposed project skill {latest["name"]} ({latest["status"]}). '
                'Do not create, update or link it and do not propose it again; leave the project unchanged for it. '
                'If the answer asks for something else, handle that; otherwise close with answer_submit. '
                'The user can still run /skill-management <request> themselves. ')
    return ''
