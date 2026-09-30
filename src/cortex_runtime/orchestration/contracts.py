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
    'The runtime runs these commands once after review, each until it exits; this is where the project\'s whole documented suite belongs. '
    'Copy each command exactly from the project\'s documented verification (docs/project/verification.md, AGENTS.md, CLAUDE.md, '
    'Makefile or package.json scripts), interpreter included, e.g. [["python3","-B","-m","pytest","-q"]]. '
    'When the project documents no verification, send the smallest command that exits nonzero if the requested result is wrong, '
    'using only the project\'s files and standard tools, e.g. [["node","--check","src/app.js"]] for a script or '
    '[["grep","-qx","ok","notes.txt"]] for exact file content; for an investigation-only plan, one that checks the investigated files. '
    'Each command is an argv array of literal strings: no shell operators, pipes, redirection or substitution; '
    'environment variables as ["env","NAME=value",...].'
)


#: Roles that never change the project: their engine jail and tools are
#: read-only whatever scope a plan requests for them.
READ_ONLY = {'worker-architect', 'worker-ux-designer', 'worker-code-reviewer', 'worker-senior-consultant', 'senior-consultant',
             'worker-explorer', 'worker-planner', 'worker-security-auditor', 'worker-accessibility-auditor'}


def obj(properties, required=None):
    return dict(type='object', description='Structured fields; use the exact advertised properties.', properties=properties,
                required=list(properties) if required is None else required, additionalProperties=False)


NODE = obj(dict(id=text('Unique plan node name: a letter, then letters, digits, \'_\' or \'-\', at most 64 characters.', 64), role=text('Specialist role. Runtime resolves its profile and route.', 80),
                objective=text('One bounded implementation or investigation objective. After all proposed tasks the runtime automatically adds independent code review whenever it observes that the project changed, and final verification; do not duplicate those stages in this plan, and ask a worker only for tests covering its change, never the project\'s whole suite, which final verification runs. Put any scope note or rationale here: a task has no other free-text field.'),
                dependencies=array(text('Predecessor node name.', 64),
                                   'Names (the id values) of predecessor tasks in this same plan.'),
                scope=array(text('Requested project-relative path or glob; \'.\' is the whole project. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write. Runtime policy decides access.', 512),
                            'Project-relative paths or globs this task needs to read or change; required even for an investigation-only task (\'.\' is the whole project). Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. '
                            'Read-only roles (' + ', '.join(sorted({r.removeprefix('worker-') for r in READ_ONLY})) + ') never create or change files, so their scope names only existing paths to read; '
                            'when such a task must produce a file, have it return the content in its report and give the file to a writing role in a dependent task.',
                            minimum=1),
                complexity=dict(type='string', enum=['low', 'normal', 'high'],
                                description='How hard this task is, judged from the work itself: low for a narrow lookup or mechanical change, '
                                            'high for cross-cutting root-cause analysis, production incidents, concurrency, security or data-loss risk; '
                                            'omit for normal. The runtime turns it into a stronger or cheaper route; it never names a model.'),
                model=dict(type='string', minLength=1, maxLength=160,
                           description='Required routing recommendation. Accepted forms: "auto", an engine id, or "engine/model". Use auto when no work-specific reason applies, and always when list_route_options lists a model under preferred for this node\'s kind of work (role_work_kinds): the runtime ranks the owner\'s preferred models ahead of any hint. Otherwise recommend the engine list_route_options gives under affinity for that kind of work (null there means none), or another engine or an exact route with a reason, and supply model_reason. An exact engine/model route must be copied from list_route_options, which lists every configured model with its tier, policy and whether it is auto_eligible, and under preferred the models the owner prefers for each kind of work. The runtime computes tier and effort. A hint never overrides pin, deny, providers, availability or quota, and never raises tier, with one exception: when the user explicitly asked in this task for a model family that list_route_options lists under flagship, that exact flagship engine/model route runs that node on it. The runtime confirms that request only from the user\'s own recorded messages and answers in this task, never from model_reason or other model text, and applies it only to nodes carrying that route. Otherwise flagship is escalation-only; retries and fixes ignore any other hint.'),
                model_reason=dict(type='string', minLength=1, maxLength=300,
                                  description='One short sentence explaining a non-auto model recommendation; required for every non-auto hint. It explains, never authorizes: a flagship route needs the user\'s own request in this task.')),
           required=['id', 'role', 'objective', 'dependencies', 'scope', 'model'])
NODE['properties']['model']['pattern'] = (
    r'^(auto|' + '|'.join(re.escape(engine) for engine in SUPPORTED_ENGINES) +
    r'|(' + '|'.join(re.escape(engine) for engine in SUPPORTED_ENGINES) +
    r')/[A-Za-z0-9._:\[\],/-]{1,128})$'
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
            item=text('What stayed unverified. For a retained not_run or unverified worker check, its exact check name; otherwise the changed surface. A name match from an unrelated node does not close a gap.', 200),
            blocker=text('The concrete blocker that made verification impossible in this task. "Not run", "out of scope" or "not requested" is not a blocker: verify it instead.', 1000))),
            'Items this task changed but could not verify, each with its concrete blocker.', maximum=32)),
        required=['text']),
    'user_question_request': obj(dict(question=text('One short line naming the decision, e.g. "Approve the plan above?". Put the details, evidence and each option\'s consequences in your message text before this call, not here. Write them exactly once: every message you write, including text before a tool call, is shown to the user in full and never collapsed. After this call end the turn at once and write nothing more: not restating, rewriting, shortening or summarizing anything already shown, at any length.', 4000),
                                     options=array(text('One short answer the user can pick, e.g. "Approve the plan" or "Do it in a separate task".', 1000),
                                                  'Short answers the user can pick between.', maximum=8),
                                     request_key=text('Unique question delivery key. Exact retries preserve question identity.', 128),
                                     skill_proposal=obj(dict(
                                         name=text('Lowercase kebab-case skill directory name under .agents/skills, 1-64 characters.', 64),
                                         action=dict(type='string', enum=['create', 'update'],
                                                     description='create for a skill that does not exist yet in .agents/skills; update for an existing one.'),
                                         purpose=text('What the skill does and when it applies. On approval it becomes the /skill-management request.', 1000),
                                         evidence=array(obj(dict(
                                             report_id=text('Exact report_id of one ordinary published report of this project that shows the case.', 14),
                                             case=text('One line naming the repeated case this report shows.', 300))),
                                             'The repeated cases, each a distinct retained report.', maximum=5, minimum=2)),
                                         required=['name', 'action', 'purpose', 'evidence'])),
                                 required=['question', 'request_key']),
}
_PROPOSAL = COORDINATOR['user_question_request']['properties']['skill_proposal']
_PROPOSAL['description'] = (
    'Optional. Only to propose a reusable project skill after retained reports show the same procedure repeated. '
    'Explain the cases and benefit in your message first. Use exactly one accepted action-matched pair, with approval first: '
    'create: [Create the skill, Not now], [Создать навык, Не сейчас], [Approve creating the skill, Decline creating the skill], '
    '[Одобрить создание навыка, Отклонить создание навыка]; update: [Update the skill, Not now], [Обновить навык, Не сейчас], '
    '[Approve updating the skill, Decline updating the skill], [Одобрить обновление навыка, Отклонить обновление навыка]. '
    'Only an exact pair is accepted; 1 selects its approval label, 2 its decline label, and any other answer is unclear. '
    'An arbitrary first option never approves. Nothing is created unless the user approves; a decline leaves the project unchanged.')
_PROPOSAL['properties']['evidence']['items']['properties']['report_id']['pattern'] = r'^r_[0-9a-f]{12}$'
FINDING = obj(dict(
    node=text('Exact id of the one original plan task whose work must change, as written in the accepted plan; never a runtime id such as _review_1 or _fix_1_api.', 64),
    files=array(text('Project-relative path of one file the fix changes, inside that plan task\'s scope.', 512),
                'Project-relative files the fix changes, each inside the scope of the plan task named in node. The runtime checks every path against that scope itself.',
                maximum=16, minimum=1),
    fix=text('What must change and how the change is confirmed, in plain words.', 2000),
    cross_cutting=dict(type='boolean', description='true when the problem spans several plan tasks or changes an interface, contract or file other tasks depend on; the runtime then sends a fix to every original plan task. false for a problem local to the named task.'),
), required=['node', 'files', 'fix', 'cross_cutting'])
FINDING['properties']['node']['pattern'] = r'^[A-Za-z][A-Za-z0-9_-]{0,63}$'
INLINE_REPORT = obj(dict(
    title=text('Readable English report title.', 200),
    markdown=text('Complete report Markdown: the opening decision brief (conclusion, observed evidence, checks, contradictions, limits, next action), then the detailed evidence. No draft marker and no template placeholder comments; the runtime adds the marker.', 1000000),
))
RESULT = obj(dict(
    report_ref=text('Send exactly one of report_ref or report. Use report_ref only after this worker publishes its own filled draft with write_report: copy that call\'s report_id exactly (r_ plus 12 lowercase hex characters) into result_submit.report_ref. Do not use draft_id, a report from another attempt, or a tool_call_ref. Omit report when sending report_ref.', 14),
    report=dict(INLINE_REPORT, description='Send exactly one of report or report_ref. Simpler alternative to the draft flow: send this title and complete Markdown as result_submit.report, with no report_ref. The runtime publishes it as this attempt\'s immutable report before recording the result. Retry an uncertain delivery with exactly the same request_key and entire payload; a changed report needs a new logical delivery.'),
    summary=text('What was found or changed, in plain words: the conclusion first, then what supports it. At most 4000 characters.', 4000),
    verdict=dict(type='string', enum=['APPROVE', 'REQUEST_CHANGES', 'BLOCK', 'approve', 'request_changes', 'block'], description='Required for a review-stage worker, omitted for other workers. Send APPROVE (or approve) when correct, REQUEST_CHANGES (or request_changes) for fixable problems, BLOCK (or block) when the task must not proceed. Lowercase forms normalize to the uppercase decision.'),
    findings=array(FINDING, 'Review only, optional, used only with verdict REQUEST_CHANGES and ignored with any other verdict. One entry per problem, each naming the original plan task that must fix it. When every finding names a task of the accepted plan, is not cross_cutting and lists only files inside that task\'s scope, the runtime creates fixes only for the named tasks, each receiving only its own findings and this review\'s report_ref, ordered as in the plan. Otherwise, or when omitted, every original plan task receives a fix with this summary.', maximum=16),
    checks=array(obj(dict(name=text('Specific stable name of this automated command check, e.g. "unit tests for services/trading" or "ruff on changed files". Reuse exactly the same name for a later passing rerun that closes this check; do not use a report title, task name, or qualitative review remark.',200), outcome=dict(type='string', enum=['passed','failed','not_run','unverified'], description='passed: you ran it in this attempt with command_run (or a user-confirmed outside_command) and it exited 0; requires tool_call_ref (see its evidence rule). failed: it ran and failed; always kept as failed, and it fails the assignment unless a later passed check of this same result cites a rerun of exactly the same command (same argv, and for outside_command the same cwd) that is the latest run of that command in this attempt: then it stays as history with closed_by. Report a fixed failure that way, never by dropping it. not_run: a real blocker prevented running it. unverified: it ran but its result could not be established. not_run and unverified require blocker. A passed claim the runtime cannot verify is recorded as unverified with evidence_gap, or as failed when its cited command or a later run of the same command exited nonzero; the result receipt lists each such check.'),
                         blocker=text('Required and non-whitespace for not_run and unverified; forbidden for passed and failed. State the concrete condition that prevented running or establishing this specific check (for example, a named unavailable host). "Not run", "out of scope" or "not requested" alone is not a blocker.', 1000),
                         command=array(dict(type='string', description='One literal argument of the command this check ran or would run.', maxLength=4096),
                                       'Optional literal argv. This command is retained only for not_run or unverified gap checks; it identifies the blocked command and lets a later verified identical run close the gap. For passed or failed, it is descriptive only: the runtime derives the actual command and pass/fail evidence exclusively from tool_call_ref, never from this array.',
                                       maximum=128, minimum=1),
                         tool_call_ref=text('For a verified passed check, copy the exact tool_call_ref field from the completed command_run (or user-confirmed outside_command) result of this same attempt; engine shell calls never verify a check, and their exit code or output is not retained. Never derive the receipt from a command, draft, report or engine shell call. A real gateway receipt has gateway: followed by 32 lowercase hex characters, but its shape alone proves nothing. Cite one receipt for one check only, after confirming complete=true, exit_code=0 and untruncated retained output. A missing, foreign, pending, failed, truncated, replayed or trivial receipt makes a claimed pass unverified or failed, not accepted as passed. A failed check may cite its nonzero command receipt; a later same-command passing rerun closes it and the runtime supplies closed_by in the result (do not send closed_by).',128)), required=['name','outcome']),
                  'Automated checks this attempt actually ran.', maximum=20),
    request_key=text('New unique key for this logical result_submit delivery, distinct from keys used for prior mutations on this channel. Generate it once and retain it with the exact report/summary/checks payload; reuse it only to retry an uncertain delivery with identical arguments. Changed content requires a new key and, for a by-reference result, a new owned report.',128),
), required=['summary','checks','request_key'])
RESULT['properties']['report_ref']['pattern'] = r'^r_[0-9a-f]{12}$'
RESULT['properties']['checks']['items']['properties']['blocker']['pattern'] = r'[\s\S]*\S[\s\S]*'
RESULT['oneOf'] = [
    {'required': ['report_ref'], 'not': {'required': ['report']}},
    {'required': ['report'], 'not': {'required': ['report_ref']}},
]
RESULT['properties']['checks']['items']['allOf'] = [{
    'if': {'properties': {'outcome': {'enum': ['not_run', 'unverified']}}, 'required': ['outcome']},
    'then': {'required': ['blocker']},
    'else': {'not': {'required': ['blocker']}},
}]
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
RESULT['properties']['checks']['description'] = 'Automated command checks, up to 20; [] when no command ran and no check was blocked. Each item needs name and outcome (passed, failed, not_run, unverified). A passed check needs tool_call_ref copied from an exit-0 command_run or confirmed outside_command of this attempt; command argv alone never verifies it. A failed check may cite its nonzero receipt. not_run or unverified require a concrete blocker; command argv is retained only for these gap checks and may identify the exact blocked command. A supplied command on passed/failed is ignored as evidence; the runtime derives the command from the receipt. Do not send closed_by: the runtime calculates it. These are not qualitative review remarks: read files and inspected code belong in the report body, not checks.'
COORDINATOR['synthesis_submit']['properties']['not_verified']['description'] = ('Every item this task changed but could not verify, each with its concrete blocker; [] or omit when everything was verified. Every retained not_run or unverified worker check must appear here by its exact name unless closed as follows: a gap with a non-empty declared command closes only by a verified command_run whose argv exactly matches that command; a gap without a command may close by a verified same-name pass in its own node or a later fix of that node. A same-name pass alone never closes a gap with a declared command, and same-name passes from unrelated nodes do not close command-less gaps. Prefer verifying a gap (replan_request) over listing it.')
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


def _matches_presence_rule(rule, value):
    """Evaluate the small presence/enum subset used by RESULT combinators."""
    if not isinstance(value, dict):
        return False
    if not set(rule.get('required', ())) <= set(value):
        return False
    if any(name in value and value[name] not in child['enum']
           for name, child in rule.get('properties', {}).items() if 'enum' in child):
        return False
    if 'not' in rule and _matches_presence_rule(rule['not'], value):
        return False
    if 'anyOf' in rule and not any(_matches_presence_rule(part, value) for part in rule['anyOf']):
        return False
    return True


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
        if not isinstance(value, dict):
            violations.append(dict(field=field, received=_received_fields(schema, value),
                                   expected=expected_fields(schema), correction='Use the advertised fields exactly.'))
            return
        for name in sorted(set(schema['required']) - set(value)):
            violations.append(dict(field=f'{field}.{name}', received='missing',
                                   expected=f'required field: {schema["properties"][name].get("description", name)}',
                                   correction=f'Add {name} in the advertised form.'))
            if len(violations) >= MAX_VIOLATIONS:
                return
        for name in sorted(set(value) - set(schema['properties'])):
            violations.append(dict(field=f'{field}.{name}', received='unexpected field',
                                   expected='one of ' + ', '.join(schema['properties']),
                                   correction=f'Omit {name}; use only fields listed in this tool\'s live schema.'))
            if len(violations) >= MAX_VIOLATIONS:
                return
        for name, child in value.items():
            if name not in schema['properties']:
                continue
            _collect(schema['properties'][name], child, f'{field}.{name}', violations)
            if len(violations) >= MAX_VIOLATIONS:
                return
        if 'oneOf' in schema and sum(_matches_presence_rule(part, value) for part in schema['oneOf']) != 1:
            present = [name for name in ('report_ref', 'report') if name in value]
            violations.append(dict(
                field=f'{field}.report_ref, {field}.report',
                received='both supplied' if len(present) == 2 else 'neither supplied',
                expected='exactly one of report_ref or report',
                correction='Send report={title, markdown} inline, or copy report_id from this worker\'s write_report into report_ref; omit the other field.',
            ))
        for conditional in schema.get('allOf', ()):  # checks[].blocker/command combinations
            branch = conditional['then'] if _matches_presence_rule(conditional['if'], value) else conditional['else']
            if _matches_presence_rule(branch, value):
                continue
            if 'required' in branch:
                violations.append(dict(field=f'{field}.blocker', received='missing',
                                       expected='non-whitespace blocker for not_run or unverified',
                                       correction='Run this check and report its outcome, or name the concrete condition that prevented it.'))
            else:
                forbidden = 'blocker' if 'blocker' in value else 'command'
                violations.append(dict(field=f'{field}.{forbidden}', received=f'present for {value.get("outcome")}',
                                       expected=f'{forbidden} omitted for passed or failed',
                                       correction=f'Remove {forbidden}; a passed or failed check gets command evidence from tool_call_ref.'))
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
    elif kind == 'boolean':
        if type(value) is not bool:
            violations.append(dict(field=field, received=type(value).__name__, expected='JSON boolean', correction=f'Send {field} as true or false.'))
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
            if field.endswith('.blocker'):
                violations.append(dict(field=field, received='whitespace-only string',
                                       expected='a non-whitespace concrete blocker',
                                       correction='Name the specific condition that prevented this check from running or being established.'))
            else:
                violations.append(dict(field=field, received=f'malformed string ({len(value)} characters)', expected=schema['pattern'],
                                       correction='Copy the server-issued identifier unchanged from its authoritative receipt; do not alter its prefix or length.'))
            return
        if 'enum' in schema and value not in schema['enum']:
            allowed = ', '.join(schema['enum'])
            violations.append(dict(field=field, received='unrecognized string value', expected=allowed, correction=f'Choose exactly one advertised value: {allowed}.'))
    else:
        raise StoreError('invalid_schema')


def validate(schema, value, field='arguments'):
    violations = []
    _collect(schema, value, field, violations)
    if violations:
        first = violations[0]
        raise StoreError('invalid_arguments', first['field'], first['received'], first['expected'], first['correction'],
                         violations=violations)
