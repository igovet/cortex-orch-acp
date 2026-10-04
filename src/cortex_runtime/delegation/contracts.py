"""Channel-scoped model proposal and bounded evidence-read schemas."""
from __future__ import annotations
import copy
from ..contracts import StoreError
from ..orchestration.contracts import COORDINATOR, WORKER, obj, text, validate as validate_schema


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
        schema['properties']['question']['description'] = (
            'Information needed to finish the assignment. Always use this structured tool with meaningful selectable options, '
            'not a prose-only question or native engine question tool. Registers an assignment-local wait and returns promptly. '
            'End this turn without publishing an incomplete result; all real answers arrive in the same healthy session.')
    return result


def validate(role_kind, operation, args):
    schema = schemas_for(role_kind).get(operation)
    if schema is None:
        raise StoreError('unknown_operation','operation',operation)
    validate_schema(schema,args)
    return args
