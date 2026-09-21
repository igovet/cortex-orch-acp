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
    'read_source': obj(dict(source_ref=text('Exact source_ref from list_sources.',64),
                            offset=integer('Character offset.',0,100000000),
                            limit=integer('Maximum chunk size.',1,65536)), required=['source_ref']),
    'read_skill': obj(dict(skill=text('Exact shared skill or role-scoped reference id.',80))),
    'read_capsule': obj(dict(at_revision=integer('Optional source revision upper bound.')), required=[]),
    'read_large_output': obj(dict(large_output_id=text('Exact retained output identifier.',20),
                                 offset=integer('Byte offset.'), limit=integer('Maximum chunk size.',1,65536)),
                             required=['large_output_id']),
}


def schemas_for(role_kind):
    if role_kind not in {'coordinator','worker'}:
        raise StoreError('unknown_role_kind','role_kind',role_kind)
    extra = (COORDINATOR | {'read_diagnostics': obj({})}) if role_kind == 'coordinator' else (
        WORKER | {'ask_question': obj(dict(question=text('Information needed to finish the assignment.',4000),
                                           request_key=text('Unique request delivery key.',128)))})
    from ..orchestration.gateway import SCHEMAS
    return copy.deepcopy(SHARED | extra | SCHEMAS)


def validate(role_kind, operation, args):
    schema = schemas_for(role_kind).get(operation)
    if schema is None:
        raise StoreError('unknown_operation','operation',operation)
    validate_schema(schema,args)
    return args
