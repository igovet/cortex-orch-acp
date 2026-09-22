"""Advertised model proposal API. Execution handles are runtime-owned."""
from ..contracts import StoreError
import re


def text(description, maximum=8000):
    return dict(type='string', description=description, minLength=1, maxLength=maximum)


def array(items, maximum=64, minimum=0):
    return dict(type='array', description='Bounded list of items described by the item schema.', items=items, minItems=minimum, maxItems=maximum)


def obj(properties, required=None):
    return dict(type='object', description='Structured fields; use the exact advertised properties.', properties=properties,
                required=list(properties) if required is None else required, additionalProperties=False)


NODE = obj(dict(id=text('Unique plan node name.', 64), role=text('Specialist role. Runtime resolves its profile and route.', 80),
                objective=text('One bounded implementation or investigation objective. The runtime automatically adds mandatory independent code review and final verification after all proposed tasks; do not duplicate those stages in this plan.'),
                dependencies=array(text('Predecessor node name.', 64)),
                scope=array(text('Requested project-relative path or glob; runtime policy decides access.', 512), minimum=1),
                complexity=dict(type='string', enum=['low', 'normal', 'high'],
                                description='How hard this task is, judged from the work itself: low for a narrow lookup or mechanical change, '
                                            'high for cross-cutting root-cause analysis, production incidents, concurrency, security or data-loss risk; '
                                            'omit for normal. The runtime turns it into a stronger or cheaper route; it never names a model.')),
           required=['id', 'role', 'objective', 'dependencies', 'scope'])
from ..portable.roles import WORKER_ALIASES
NODE['properties']['role']['enum'] = sorted(role for role in WORKER_ALIASES if role not in {'code-reviewer','build-verification'})
PLAN = obj(dict(tasks=array(NODE, minimum=1), verification=array(array(text('Literal verification argument. Project is read-only; write generated output only under /tmp.', 4096), minimum=1), maximum=16, minimum=1)))
COORDINATOR = {
    'task_get_context': obj({}),
    'answer_submit': obj(dict(text=text('Answer a read-only question directly, in the selected coordinator language, grounded in the project files you inspected. Use for explanations, project overviews and factual questions that need no changes or delegated investigation. Available before any plan or worker exists, or after the user answered a runtime stop question by choosing to finish. Does not certify implementation, review, or tests; use plan_submit when the user requests changes.',32000), request_key=text('Unique answer delivery key; exact retries return the same receipt.',128))),
    'plan_submit': obj(dict(plan=PLAN, request_key=text('Unique proposal delivery key; exact retries return the same receipt.', 128))),
    'synthesis_submit': obj(dict(text=text('Final synthesis grounded in current reports and runtime verification.', 32000))),
    'user_question_request': obj(dict(question=text('Question requiring user input.', 4000),
                                     options=array(text('Suggested answer.', 1000), maximum=8),
                                     request_key=text('Unique question delivery key. Exact retries preserve question identity.', 128)),
                                 required=['question', 'request_key']),
}
RESULT = obj(dict(
    report_ref=text('Exact immutable report_id from write_report for this worker attempt.', 14),
    summary=text('Summary of the result.', 4000),
    verdict=dict(type='string', enum=['APPROVE', 'REQUEST_CHANGES', 'BLOCK'], description='Required for the runtime reviewer; other roles must omit.'),
    checks=array(obj(dict(name=text('Check name.',200), outcome=dict(type='string', enum=['passed','failed','not_run','unverified']),
                         tool_call_ref=text('Exact tool_call_ref returned by command_run in this attempt. A passed claim requires the retained complete command result with exit code zero.',128)), required=['name','outcome']), maximum=20),
    request_key=text('Unique result delivery key.',128),
), required=['report_ref','summary','checks','request_key'])
RESULT['properties']['report_ref']['pattern'] = r'^r_[0-9a-f]{12}$'
REQUESTS = {
    'replan_request': obj(dict(reason=text('Evidence requiring a new decomposition or additional investigation.'),request_key=text('Unique proposal identity.',128))),
    'scope_request': obj(dict(reason=text('Why the current scope prevents completion.'),scope=array(text('Requested project-relative path or glob.',512),minimum=1),request_key=text('Unique proposal identity.',128))),
    'escalation_request': obj(dict(reason=text('Problem requiring independent senior consultation. Runtime retains configured model/effort policy.'),request_key=text('Unique proposal identity.',128))),
}
COORDINATOR.update(REQUESTS)
RESULT['properties']['checks']['description'] = 'Executed automated checks with retained command evidence. Every passed item needs its own supporting tool_call_ref; descriptive review observations belong in the report. Failed, not_run or unverified checks block completion.'
WORKER = {'result_submit': RESULT, **REQUESTS}


def validate(schema, value, field='arguments'):
    kind = schema.get('type', 'object' if 'properties' in schema else None)
    if kind == 'object':
        if not isinstance(value, dict) or set(value) - set(schema['properties']) or set(schema['required']) - set(value):
            raise StoreError('invalid_arguments', field,
                ('missing: ' + ', '.join(sorted(set(schema['required']) - set(value))) +
                 '; unexpected: ' + ', '.join(sorted(set(value) - set(schema['properties'])))) if isinstance(value, dict) else 'not an object',
                'required fields: ' + ', '.join(schema['required']), 'Use the advertised fields exactly.')
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
        if type(value) is not int or not schema['minimum'] <= value <= schema['maximum']:
            target = f"integer from {schema['minimum']} to {schema['maximum']}"
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
