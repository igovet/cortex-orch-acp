"""Advertised model proposal API. Execution handles are runtime-owned."""
from ..connections.model import SUPPORTED_ENGINES
from ..contracts import StoreError, expected_fields
import re


def text(description, maximum=8000):
    return dict(type='string', description=description, minLength=1, maxLength=maximum)


def array(items, maximum=64, minimum=0):
    return dict(type='array', description='Bounded list of items described by the item schema.', items=items, minItems=minimum, maxItems=maximum)


def obj(properties, required=None):
    return dict(type='object', description='Structured fields; use the exact advertised properties.', properties=properties,
                required=list(properties) if required is None else required, additionalProperties=False)


NODE = obj(dict(id=text('Unique plan node name: a letter, then letters, digits, \'_\' or \'-\', at most 64 characters.', 64), role=text('Specialist role. Runtime resolves its profile and route.', 80),
                objective=text('One bounded implementation or investigation objective. The runtime automatically adds mandatory independent code review and final verification after all proposed tasks; do not duplicate those stages in this plan. Put any scope note or rationale here: a task has no other free-text field.'),
                dependencies=array(text('Predecessor node name.', 64)),
                scope=array(text('Requested project-relative path or glob; \'.\' is the whole project. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write. Runtime policy decides access.', 512), minimum=1),
                complexity=dict(type='string', enum=['low', 'normal', 'high'],
                                description='How hard this task is, judged from the work itself: low for a narrow lookup or mechanical change, '
                                            'high for cross-cutting root-cause analysis, production incidents, concurrency, security or data-loss risk; '
                                            'omit for normal. The runtime turns it into a stronger or cheaper route; it never names a model.'),
                model=dict(type='string', minLength=1, maxLength=160,
                           description='Required routing recommendation. Use auto to let the runtime choose, an engine id to prefer that engine, or an exact engine/model route from list_route_options. The runtime computes tier and effort; a hint never raises tier or overrides pin, deny, providers, availability or quota. Flagship is escalation-only; retries and fixes ignore the hint.'),
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
PLAN = obj(dict(tasks=array(NODE, minimum=1), verification=array(array(text('Literal argument of a verification command copied exactly from the project\'s own documented verification (docs/project/verification.md, AGENTS.md, CLAUDE.md, Makefile or package.json scripts), interpreter path included; set environment variables with ["env", "NAME=value", ...]. Runs with the owner\'s tools; only git-ignored paths are writable.', 4096), minimum=1), maximum=16, minimum=1)))
COORDINATOR = {
    'task_get_context': obj({}),
    'list_route_options': obj({}),
    'answer_submit': obj(dict(text=text('Answer a read-only question directly, in the selected coordinator language, grounded in the project files you inspected. Use for explanations, project overviews and factual questions that need no changes or delegated investigation. Available before any plan or worker exists, or after the user answered a runtime stop question or a question asked after verification by choosing to finish. Does not certify implementation, review, or tests; use plan_submit when the user requests changes.',32000), request_key=text('Unique answer delivery key; exact retries return the same receipt.',128))),
    'plan_submit': obj(dict(plan=PLAN, request_key=text('Unique proposal delivery key; exact retries return the same receipt.', 128))),
    'synthesis_submit': obj(dict(
        text=text('Final synthesis grounded in current reports and runtime verification. Only when nothing is left for the user to decide: it completes the task.', 32000),
        not_verified=array(obj(dict(
            item=text('What stayed unverified. For a retained not_run or unverified worker check, its exact check name; otherwise the changed surface.', 200),
            blocker=text('The concrete blocker that made verification impossible in this task. "Not run", "out of scope" or "not requested" is not a blocker: verify it instead.', 1000))), maximum=32)),
        required=['text']),
    'user_question_request': obj(dict(question=text('One short line naming the decision, e.g. "Approve the plan above?". Put the details, evidence and each option\'s consequences in your message text before this call, not here.', 4000),
                                     options=array(text('One short answer the user can pick, e.g. "Approve the plan" or "Do it in a separate task".', 1000), maximum=8),
                                     request_key=text('Unique question delivery key. Exact retries preserve question identity.', 128)),
                                 required=['question', 'request_key']),
}
RESULT = obj(dict(
    report_ref=text('Exact immutable report_id from write_report for this worker attempt.', 14),
    summary=text('What was found or changed, in plain words: the conclusion first, then what supports it. At most 4000 characters.', 4000),
    verdict=dict(type='string', enum=['APPROVE', 'REQUEST_CHANGES', 'BLOCK'], description='The review decision: APPROVE when the change is correct, REQUEST_CHANGES for fixable problems, BLOCK when it must not proceed.'),
    checks=array(obj(dict(name=text('What the check verified, e.g. "unit tests for services/trading" or "ruff on the changed files".',200), outcome=dict(type='string', enum=['passed','failed','not_run','unverified'], description='passed: you ran it in this attempt and it succeeded (see tool_call_ref for the evidence rule). failed: it ran and failed, which fails the assignment. not_run: a real blocker prevented running it. unverified: it ran but its result could not be established. not_run and unverified require blocker.'),
                         blocker=text('Required for not_run and unverified, omitted for passed and failed: the concrete blocker that made running or establishing this check impossible in this attempt. "Not run", "out of scope" or "not requested" is not a blocker.', 1000),
                         tool_call_ref=text('Evidence for a passed check, from this attempt only: the exact tool_call_ref a command_run returned (it must have exited 0), or the id of one of your own completed tool calls. Omit it for a check you ran with your own shell: a pass is then accepted only if this attempt ran at least one shell command that completed. A pass with no command run at all is refused.',128)), required=['name','outcome']), maximum=20),
    request_key=text('Unique result delivery key.',128),
), required=['report_ref','summary','checks','request_key'])
RESULT['properties']['report_ref']['pattern'] = r'^r_[0-9a-f]{12}$'
REQUESTS = {
    'replan_request': obj(dict(reason=text('Evidence requiring a new decomposition or additional investigation.'),request_key=text('Unique proposal identity.',128))),
    'scope_request': obj(dict(reason=text('Why the current scope prevents completion.'),scope=array(text('Requested project-relative path or glob; \'.\' is the whole project. Never absolute, never containing \'..\' or a backslash, and never inside .cortex or .git. Under .claude/.codex only the exact project notes session-history.md and lessons-learned.md are allowed; create missing notes with project_write.',512),minimum=1),request_key=text('Unique proposal identity.',128))),
    'escalation_request': obj(dict(reason=text('Problem requiring independent senior consultation. Runtime retains configured model/effort policy.'),request_key=text('Unique proposal identity.',128))),
}
COORDINATOR['verification_submit'] = obj(dict(
    verification=dict(PLAN['properties']['verification'], description='Corrected verification commands. Only after the runtime verification failed and the work itself is fine: it verifies again without redoing the reviewed work. When the code is wrong, submit a new plan instead.'),
    request_key=text('Unique proposal identity.',128)))
COORDINATOR.update(REQUESTS)
RESULT['properties']['checks']['description'] = 'Automated checks this attempt actually ran, up to 20; [] when there were none. Descriptive review observations belong in the report, not here. A failed check fails the assignment; not_run or unverified checks each need their blocker and are kept as disclosed gaps for review and synthesis.'
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


def validate(schema, value, field='arguments'):
    kind = schema.get('type', 'object' if 'properties' in schema else None)
    if kind == 'object':
        if not isinstance(value, dict) or set(value) - set(schema['properties']) or set(schema['required']) - set(value):
            raise StoreError('invalid_arguments', field, _received_fields(schema, value),
                             expected_fields(schema), 'Use the advertised fields exactly.')
        for name, child in value.items():
            validate(schema['properties'][name], child, f'{field}.{name}')
    elif kind == 'array':
        if not isinstance(value, list) or not schema.get('minItems',0) <= len(value) <= schema['maxItems']:
            target = f"array of {schema.get('minItems',0)} to {schema['maxItems']} items"
            raise StoreError('invalid_arguments', field, f'{len(value)} items' if isinstance(value, list) else type(value).__name__,
                             target, f'Send {field} as an {target}.')
        for index, child in enumerate(value):
            validate(schema['items'], child, f'{field}[{index}]')
    elif kind == 'integer':
        # A bound the schema does not advertise is not checked (reading it
        # raised KeyError, which reached the model as storage_error).
        low, high = schema.get('minimum'), schema.get('maximum')
        if type(value) is not int or (low is not None and value < low) or (high is not None and value > high):
            target = (f'integer from {low} to {high}' if low is not None and high is not None else
                      f'integer of at least {low}' if low is not None else
                      f'integer of at most {high}' if high is not None else 'integer')
            raise StoreError('invalid_arguments', field, repr(value)[:40], target, f'Send {field} as an {target}.')
    elif kind == 'string':
        if not isinstance(value,str):
            raise StoreError('invalid_arguments',field,type(value).__name__,'JSON string',f'Send {field} as a JSON string.')
        target = f"between {schema.get('minLength',0)} and {schema.get('maxLength',32000)} characters"
        if not schema.get('minLength',0) <= len(value) <= schema.get('maxLength',32000):
            raise StoreError('invalid_arguments',field,f'{len(value)} Unicode characters',target,
                             f'Rewrite {field} {target} and verify its character count.')
        if 'pattern' in schema and re.fullmatch(schema['pattern'],value) is None:
            raise StoreError('invalid_arguments',field,value[:40],schema['pattern'],
                             'Copy the server-issued identifier unchanged from its authoritative receipt.')
        if 'enum' in schema and value not in schema['enum']:
            allowed = ', '.join(schema['enum'])
            raise StoreError('invalid_arguments',field,value[:40],allowed,f'Choose exactly one advertised value: {allowed}.')
    else:
        raise StoreError('invalid_schema')
