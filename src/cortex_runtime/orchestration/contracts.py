"""Advertised model proposal API. Execution handles are runtime-owned."""
from ..connections.model import SUPPORTED_ENGINES
from ..contracts import StoreError, expected_fields
import re


def text(description, maximum=8000):
    return dict(type='string', description=description, minLength=1, maxLength=maximum)


def array(items, description, maximum=64, minimum=0):
    # A code-mode host (e.g. Codex) renders only this array-level description
    # to the model, dropping minItems/maxItems and the item schema's own
    # description entirely, so the bounds and the item's meaning must both
    # live here, not only in ``items``. A required array opens with its
    # refusal of []: a trailing "Send 1 to 16 items." was rendered and still
    # ignored (r_236acb13fabb, first plan_submit sent verification []).
    if minimum > 0:
        return dict(type='array', description=required_bounds(minimum, maximum) + ' ' + description,
                    items=items, minItems=minimum, maxItems=maximum)
    return dict(type='array', description=f'{description} At most {maximum} items; [] when none.',
                items=items, minItems=minimum, maxItems=maximum)


def required_bounds(minimum, maximum):
    return f'Required: {minimum} to {maximum} items; [] is rejected.'


# Where verification commands come from, including the case the old text left
# open: a project that documents none, where "copy exactly" left the model
# nothing to send but [].
VERIFICATION_SOURCE = (
    'Required for every plan, including an investigation-only or trivial one: verification runs even when nothing changed. '
    'Copy each command exactly from the project\'s documented verification (docs/project/verification.md, AGENTS.md, CLAUDE.md, '
    'Makefile or package.json scripts), interpreter included, e.g. [["python3","-B","-m","pytest","-q"]]. '
    'When the project documents no verification, send the smallest command that exits nonzero if the requested result is wrong, '
    'using only the project\'s files and standard tools, e.g. [["node","--check","src/app.js"]] for a script or '
    '[["grep","-qx","ok","notes.txt"]] for exact file content; for an investigation-only plan, one that checks the investigated files. '
    'Each command is an argv array of literal strings: no shell operators, pipes, redirection or substitution; '
    'environment variables as ["env","NAME=value",...].'
)


def obj(properties, required=None):
    return dict(type='object', description='Structured fields; use the exact advertised properties.', properties=properties,
                required=list(properties) if required is None else required, additionalProperties=False)


NODE = obj(dict(id=text('Unique plan node name: a letter, then letters, digits, \'_\' or \'-\', at most 64 characters.', 64), role=text('Specialist role. Runtime resolves its profile and route.', 80),
                objective=text('One bounded implementation or investigation objective. After all proposed tasks the runtime automatically adds independent code review whenever it observes that the project changed, and final verification; do not duplicate those stages in this plan. Put any scope note or rationale here: a task has no other free-text field.'),
                dependencies=array(text('Predecessor node name.', 64),
                                   'Names (the id values) of predecessor tasks in this same plan.'),
                scope=array(text('Requested project-relative path or glob; \'.\' is the whole project. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write. Runtime policy decides access.', 512),
                            'Project-relative paths or globs this task needs to read or change; required even for an investigation-only task (\'.\' is the whole project). Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git.',
                            minimum=1),
                complexity=dict(type='string', enum=['low', 'normal', 'high'],
                                description='How hard this task is, judged from the work itself: low for a narrow lookup or mechanical change, '
                                            'high for cross-cutting root-cause analysis, production incidents, concurrency, security or data-loss risk; '
                                            'omit for normal. The runtime turns it into a stronger or cheaper route; it never names a model.'),
                model=dict(type='string', minLength=1, maxLength=160,
                           description='Required routing recommendation. Accepted forms: "auto", an engine id, or "engine/model". Use auto when no work-specific engine reason applies; otherwise recommend an engine id and supply model_reason. Use claude-agent-acp for code analysis, root-cause debugging, large-context reading, review-like judgement, design or prose; use codex-acp for mechanical edits and fast run-fix loops. An exact engine/model route must come from list_route_options. The runtime computes tier and effort; a hint never raises tier or overrides pin, deny, providers, availability or quota. Flagship is escalation-only; retries and fixes ignore the hint.'),
                model_reason=dict(type='string', minLength=1, maxLength=300,
                                  description='One short sentence explaining a non-auto model recommendation; required for every non-auto hint.')),
           required=['id', 'role', 'objective', 'dependencies', 'scope', 'model'])
NODE['properties']['model']['pattern'] = (
    r'^(auto|' + '|'.join(re.escape(engine) for engine in SUPPORTED_ENGINES) +
    r'|(' + '|'.join(re.escape(engine) for engine in SUPPORTED_ENGINES) +
    r')/[A-Za-z0-9._:\[\]-]{1,128})$'
)
NODE['properties']['id']['pattern'] = r'^[A-Za-z][A-Za-z0-9_-]{0,63}$'
from ..portable.roles import WORKER_ALIASES
NODE['properties']['role']['enum'] = sorted(role for role in WORKER_ALIASES if role not in {'code-reviewer','build-verification'})
PLAN = obj(dict(
    tasks=array(NODE, 'Plan tasks, each a bounded implementation or investigation objective for one specialist role.', minimum=1),
    verification=array(
        array(text('Literal argument of a verification command, copied exactly from the project\'s own documented verification (docs/project/verification.md, AGENTS.md, CLAUDE.md, Makefile or package.json scripts) when it documents any, interpreter path included; set environment variables with ["env", "NAME=value", ...]. Runs as argv without a shell: do not use command substitution, backticks, redirection, pipes, shell operators or standalone parentheses. Runs with the owner\'s tools; only git-ignored paths are writable.', 4096),
              'One verification command as argv: literal string arguments, no shell.', minimum=1),
        'Verification commands the runtime runs after the work. ' + VERIFICATION_SOURCE,
        maximum=16, minimum=1)))
COORDINATOR = {
    'task_get_context': obj({}),
    'answer_submit': obj(dict(text=text('Answer a read-only question directly, in the selected coordinator language, grounded in the project files you inspected. Use for explanations, project overviews and factual questions that need no changes or delegated investigation. Available before any plan or worker exists; after the user answered a runtime stop question or a question asked after verification by choosing to finish; or when a new user message reopened a completed task and no work has been planned since. Does not certify implementation, review, or tests; use plan_submit when the user requests changes.',32000), request_key=text('Unique answer delivery key; exact retries return the same receipt.',128))),
    'plan_submit': obj(dict(plan=PLAN, request_key=text('Unique proposal delivery key; exact retries return the same receipt.', 128))),
    'synthesis_submit': obj(dict(
        text=text('Final synthesis grounded in current reports and runtime verification. Only when nothing is left for the user to decide: it completes the task.', 32000),
        not_verified=array(obj(dict(
            item=text('What stayed unverified. For a retained not_run or unverified worker check, its exact check name; otherwise the changed surface.', 200),
            blocker=text('The concrete blocker that made verification impossible in this task. "Not run", "out of scope" or "not requested" is not a blocker: verify it instead.', 1000))),
            'Items this task changed but could not verify, each with its concrete blocker.', maximum=32)),
        required=['text']),
    'user_question_request': obj(dict(question=text('One short line naming the decision, e.g. "Approve the plan above?". Put the details, evidence and each option\'s consequences in your message text before this call, not here. Write them exactly once: every message you write, including text before a tool call, is shown to the user in full and never collapsed, so after this call end the turn without restating, shortening or summarizing them.', 4000),
                                     options=array(text('One short answer the user can pick, e.g. "Approve the plan" or "Do it in a separate task".', 1000),
                                                  'Short answers the user can pick between.', maximum=8),
                                     request_key=text('Unique question delivery key. Exact retries preserve question identity.', 128)),
                                 required=['question', 'request_key']),
}
RESULT = obj(dict(
    report_ref=text('Exact immutable report_id from write_report for this worker attempt.', 14),
    summary=text('What was found or changed, in plain words: the conclusion first, then what supports it. At most 4000 characters.', 4000),
    verdict=dict(type='string', enum=['APPROVE', 'REQUEST_CHANGES', 'BLOCK'], description='The review decision: APPROVE when the change is correct, REQUEST_CHANGES for fixable problems, BLOCK when it must not proceed.'),
    checks=array(obj(dict(name=text('What the check verified, e.g. "unit tests for services/trading" or "ruff on the changed files".',200), outcome=dict(type='string', enum=['passed','failed','not_run','unverified'], description='passed: you ran it in this attempt with command_run (or a user-confirmed outside_command) and it exited 0; requires tool_call_ref (see its evidence rule). failed: it ran and failed; always kept as failed, and it fails the assignment unless a later passed check of this same result cites a rerun of exactly the same command (same argv, and for outside_command the same cwd) that is the latest run of that command in this attempt: then it stays as history with closed_by. Report a fixed failure that way, never by dropping it. not_run: a real blocker prevented running it. unverified: it ran but its result could not be established. not_run and unverified require blocker. A passed claim the runtime cannot verify is recorded as unverified with evidence_gap, or as failed when its cited command or a later run of the same command exited nonzero; the result receipt lists each such check.'),
                         blocker=text('Required for not_run and unverified, omitted for passed and failed: the concrete blocker that made running or establishing this check impossible in this attempt. "Not run", "out of scope" or "not requested" is not a blocker.', 1000),
                         tool_call_ref=text('Required for passed. The exact tool_call_ref returned by a command_run or user-confirmed outside_command of this same attempt that ran this check. It verifies passed only when that call completed (not pending, interrupted or timed out), exited 0, retained its complete untruncated output, and is not a trivial stand-in: pwd, ls, echo, cat, grep, find, true, git status or plain git diff, or a sh -c script whose exit status comes only from such commands. Refs of another attempt or task, of reads, writes, searches, listings or reports, and of your own engine shell calls (their exit code and output are not retained) never verify a check; cite each ref for one check only. A nonzero exit supports failed.',128)), required=['name','outcome']),
                  'Automated checks this attempt actually ran.', maximum=20),
    request_key=text('Unique result delivery key.',128),
), required=['report_ref','summary','checks','request_key'])
RESULT['properties']['report_ref']['pattern'] = r'^r_[0-9a-f]{12}$'
REQUESTS = {
    'replan_request': obj(dict(reason=text('Evidence requiring a new decomposition or additional investigation.'),request_key=text('Unique proposal identity.',128))),
    'scope_request': obj(dict(reason=text('Why the current scope prevents completion.'),
                              scope=array(text('Requested project-relative path or glob; \'.\' is the whole project. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write.',512),
                                          'Additional project-relative paths or globs needed to complete this assignment (\'.\' is the whole project). Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git.',
                                          minimum=1),
                              request_key=text('Unique proposal identity.',128))),
    'escalation_request': obj(dict(reason=text('Problem requiring independent senior consultation. Runtime retains configured model/effort policy.'),request_key=text('Unique proposal identity.',128))),
}
COORDINATOR['verification_submit'] = obj(dict(
    verification=dict(PLAN['properties']['verification'], description=required_bounds(1, 16) + ' Corrected verification commands. Only after the runtime verification failed and the work itself is fine: it verifies again without redoing the reviewed work. When the code is wrong, submit a new plan instead. ' + VERIFICATION_SOURCE),
    request_key=text('Unique proposal identity.',128)))
COORDINATOR.update(REQUESTS)
RESULT['properties']['checks']['description'] = 'Automated checks this attempt actually ran, up to 20; [] when there were none. Each is one executed command with an exit code, not a judgement: qualitative review remarks, read files and inspected code belong in the report body, not here. A failed check fails the assignment unless an exact later passing rerun of the same command closes it; not_run or unverified checks each need their blocker and are kept as disclosed gaps for review and synthesis.'
COORDINATOR['synthesis_submit']['properties']['not_verified']['description'] = ('Every item this task changed but could not verify, each with its concrete blocker; [] or omit when everything was verified. Every retained not_run or unverified worker check that no other result of this plan revision passed under the same name must appear here by its exact name. Prefer verifying a gap (replan_request) over listing it.')
WORKER = {'result_submit': RESULT, **REQUESTS}


def _received_fields(schema, value):
    if not isinstance(value, dict):
        return 'not an object'
    missing = sorted(set(schema['required']) - set(value))
    unexpected = sorted(set(value) - set(schema['properties']))
    return '; '.join(part for part in (
        'missing: ' + ', '.join(missing) if missing else '',
        'unexpected: ' + ', '.join(unexpected) if unexpected else '') if part)


MAX_VIOLATIONS = 8


def _collect(schema, value, field, violations):
    """Append every violation found in one pass, up to MAX_VIOLATIONS.

    A single call previously raised on the model's first mistake, costing a
    retry per remaining mistake (observed: two round trips for one
    plan_submit with four simultaneous violations). Collecting them here lets
    ``validate`` report every field at once.
    """
    if len(violations) >= MAX_VIOLATIONS:
        return
    kind = schema.get('type', 'object' if 'properties' in schema else None)
    if kind == 'object':
        if not isinstance(value, dict) or set(value) - set(schema['properties']) or set(schema['required']) - set(value):
            violations.append(dict(field=field, received=_received_fields(schema, value),
                                   expected=expected_fields(schema), correction='Use the advertised fields exactly.'))
            return
        for name, child in value.items():
            _collect(schema['properties'][name], child, f'{field}.{name}', violations)
            if len(violations) >= MAX_VIOLATIONS:
                return
    elif kind == 'array':
        if not isinstance(value, list) or not schema.get('minItems',0) <= len(value) <= schema['maxItems']:
            target = f"array of {schema.get('minItems',0)} to {schema['maxItems']} items"
            violations.append(dict(field=field, received=f'{len(value)} items' if isinstance(value, list) else type(value).__name__,
                                   expected=target, correction=f'Send {field} as an {target}. {schema.get("description", "")}'.rstrip()))
            return
        for index, child in enumerate(value):
            _collect(schema['items'], child, f'{field}[{index}]', violations)
            if len(violations) >= MAX_VIOLATIONS:
                return
    elif kind == 'integer':
        # A bound the schema does not advertise is not checked (reading it
        # raised KeyError, which reached the model as storage_error).
        low, high = schema.get('minimum'), schema.get('maximum')
        if type(value) is not int or (low is not None and value < low) or (high is not None and value > high):
            target = (f'integer from {low} to {high}' if low is not None and high is not None else
                      f'integer of at least {low}' if low is not None else
                      f'integer of at most {high}' if high is not None else 'integer')
            violations.append(dict(field=field, received=repr(value)[:40], expected=target, correction=f'Send {field} as an {target}.'))
    elif kind == 'string':
        if not isinstance(value,str):
            violations.append(dict(field=field, received=type(value).__name__, expected='JSON string', correction=f'Send {field} as a JSON string.'))
            return
        target = f"between {schema.get('minLength',0)} and {schema.get('maxLength',32000)} characters"
        if not schema.get('minLength',0) <= len(value) <= schema.get('maxLength',32000):
            violations.append(dict(field=field, received=f'{len(value)} Unicode characters', expected=target,
                                   correction=f'Rewrite {field} {target} and verify its character count.'))
            return
        if 'pattern' in schema and re.fullmatch(schema['pattern'],value) is None:
            violations.append(dict(field=field, received=value[:40], expected=schema['pattern'],
                                   correction='Copy the server-issued identifier unchanged from its authoritative receipt.'))
            return
        if 'enum' in schema and value not in schema['enum']:
            allowed = ', '.join(schema['enum'])
            violations.append(dict(field=field, received=value[:40], expected=allowed, correction=f'Choose exactly one advertised value: {allowed}.'))
    else:
        raise StoreError('invalid_schema')


def validate(schema, value, field='arguments'):
    violations = []
    _collect(schema, value, field, violations)
    if violations:
        first = violations[0]
        raise StoreError('invalid_arguments', first['field'], first['received'], first['expected'], first['correction'],
                         violations=violations)
