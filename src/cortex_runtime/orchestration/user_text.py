"""Bounded inline projection of large user-originated text.

A pasted chat history or log can be megabytes. Inlined verbatim into every
coordinator turn (task goal, current input, retained events, steering) it
overflows the context budget before the engine is ever prompted, and every
later turn repeats the same overflow. The full text stays durable in the
task's source store; prompts and task_get_context carry an exact head and
tail plus the read_source reference and byte offset of the omitted middle.
"""
from ..store import digest

INLINE_LIMIT_BYTES = 48 * 1024
HEAD_BYTES = 32 * 1024
TAIL_BYTES = 8 * 1024


def source_ref_for(db, task_id, text):
    """The task's captured source whose last edition is exactly ``text``."""
    row = db.execute(
        "SELECT sr.report_id FROM source_revisions sr WHERE sr.task_id=? AND "
        "(SELECT e.digest FROM editions e WHERE e.report_id=sr.report_id ORDER BY e.sequence DESC LIMIT 1)=? "
        "ORDER BY sr.revision DESC LIMIT 1",
        (task_id, digest(text.encode('utf-8'))),
    ).fetchone()
    return f'{row[0]}:-1' if row is not None else None


def bound_text(store, task_id, text):
    if not isinstance(text, str):
        return text
    data = text.encode('utf-8')
    if len(data) <= INLINE_LIMIT_BYTES:
        return text
    head = data[:HEAD_BYTES].decode('utf-8', 'ignore')
    tail = data[-TAIL_BYTES:].decode('utf-8', 'ignore')
    head_bytes = len(head.encode('utf-8'))
    omitted = len(data) - head_bytes - len(tail.encode('utf-8'))
    with store.connection() as db:
        ref = source_ref_for(db, task_id, text)
    where = (f'read it with read_source source_ref={ref} offset={head_bytes} and follow next_offset'
             if ref is not None else 'find its source_ref with list_sources and read it with read_source')
    return (f'{head}\n[runtime: {omitted} of {len(data)} UTF-8 bytes of this user text are omitted here; '
            f'{where} before relying on the omitted part]\n{tail}')


def bound_context(store, task_id, value):
    """Copy of a task context with user-originated text bounded."""
    projected = dict(value)
    projected['goal'] = bound_text(store, task_id, value.get('goal'))
    if isinstance(value.get('steering'), list):
        projected['steering'] = [
            dict(item, text=bound_text(store, task_id, item['text']))
            if isinstance(item, dict) and isinstance(item.get('text'), str) else item
            for item in value['steering']
        ]
    if isinstance(value.get('user_answers'), list):
        projected['user_answers'] = [
            dict(item, answer_text=bound_text(store, task_id, item['answer_text']))
            if isinstance(item, dict) and isinstance(item.get('answer_text'), str) else item
            for item in value['user_answers']
        ]
    return projected


def bound_event_payload(store, task_id, payload):
    if not isinstance(payload, dict):
        return payload
    return {key: bound_text(store, task_id, item) if key in ('content', 'text', 'answer_text') else item
            for key, item in payload.items()}


def bound_prompt_blocks(store, task_id, blocks):
    return [
        dict(block, text=bound_text(store, task_id, block['text']))
        if isinstance(block, dict) and block.get('type') == 'text' and isinstance(block.get('text'), str) else block
        for block in blocks
    ]
