"""Channel-scoped model proposal and bounded evidence-read schemas."""
from __future__ import annotations
import copy
from ..contracts import StoreError
from ..orchestration.contracts import COORDINATOR, WORKER, obj, required_bounds, text, validate as validate_schema


def integer(description, minimum=0, maximum=1000000000):
    return dict(type='integer',description=description,minimum=minimum,maximum=maximum)


SHARED = {
    'list_sources': obj(dict(after=integer('Exact cursor returned by the preceding source page.'),
                             limit=integer('Maximum page size.',1,200)), required=[]),
    'read_source': obj(dict(source_ref=text('Exact source_ref from list_sources: an entry source_ref or an answered_questions source_ref.',64),
                            offset=integer('Character offset.',0,100000000),
                            limit=integer('Maximum chunk size.',1,65536)), required=['source_ref']),
    'read_skill': obj(dict(skill=text('Exact shared skill/reference id, or exact SKILL.md path listed in this session\'s Project and user skills section (relative to the project or absolute for a user skill). No globs or other files.',4096))),
    'read_capsule': obj(dict(at_revision=integer('Optional source revision upper bound.')), required=[]),
    'read_large_output': obj(dict(large_output_id=text('Exact retained output identifier.',20),
                                 offset=integer('Byte offset.'), limit=integer('Maximum chunk size.',1,65536)),
                             required=['large_output_id']),
}


def schemas_for(role_kind):
    if role_kind not in {'coordinator','worker'}:
        raise StoreError('unknown_role_kind','role_kind',role_kind)
    extra = (COORDINATOR | {'read_diagnostics': obj({}), 'list_route_options': obj({})}) if role_kind == 'coordinator' else (
        WORKER | {'ask_question': COORDINATOR['user_question_request']})
    from ..orchestration.gateway import SCHEMAS
    result = copy.deepcopy(SHARED | extra | SCHEMAS)
    if role_kind == 'worker':
        schema = result['ask_question']
        schema['properties'].pop('skill_proposal')
        # Only the coordinator proposes skills: keep skill_proposal out of the
        # worker's presence rule and batch description too.
        schema['then'] = {'not': {'anyOf': [{'required': [name]} for name in ('question', 'options')]}}
        batch = schema['properties']['questions']
        batch['description'] = batch['description'].replace('question/options/skill_proposal', 'question/options')
        purpose = (
            'Only for a genuine user decision that cannot be resolved from the assignment, retained context or runtime tools. '
            'Never ask the user for missing project access, permissions or declared work paths: use access_request, '
            'which the runtime grants or refuses under policy. Use scope_request/replan_request only for a changed '
            'decomposition; unavailable evidence is a report limitation, not a request that the user repair tool access. ')
        language = (
            'Write the user-facing question and every selectable option in user_question_language supplied by the '
            'runtime in the current turn (en = English, ru = Russian), even though worker communication and reports '
            'stay English. The runtime forwards these strings verbatim; it does not translate them. ')
        schema['properties']['question']['description'] = (
            purpose + language + 'One self-contained question with meaningful selectable options, not a prose-only '
            'question or native engine question tool. Registers an assignment-local wait and returns promptly. '
            'End this turn without publishing an incomplete result; the runtime delivers the real answer.')
        options = schema['properties']['options']
        options['description'] = (
            required_bounds(options['minItems'], options['maxItems']) +
            ' 2 to 8 distinct meaningful answers; never a placeholder or Other. Custom free text remains available. ' + language)
        options['items']['description'] = 'One meaningful selectable answer. ' + language
        batch['description'] += ' ' + purpose + language
        form = batch['items']['properties']
        form['question']['description'] = schema['properties']['question']['description']
        form['options'] = copy.deepcopy(options)
    return result


def validate(role_kind, operation, args):
    schema = schemas_for(role_kind).get(operation)
    if schema is None:
        raise StoreError('unknown_operation','operation',operation)
    validate_schema(schema,args)
    return args
