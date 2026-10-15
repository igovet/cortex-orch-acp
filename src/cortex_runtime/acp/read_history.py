"""Keep read targets/status in display history, never the returned document body.

This projection does not touch the MCP response delivered to the model.
"""
import re
from collections.abc import Mapping


def shell_command_call(title=None, kind=None, raw_input=None):
    """Whether a call is a shell command rather than a native read.

    Claude's Bash is titled with its own command line, so ``cat a | grep b``
    must not be taken for a Read by its first word. Only the engine's own
    ``read`` kind makes a command a read.
    """
    args = raw_input if isinstance(raw_input, Mapping) else {}
    command = args.get('command', args.get('cmd'))
    if kind == 'read' or not isinstance(command, str) or not command.strip():
        return False
    return kind in {'execute', 'terminal'} or ' '.join(str(title or '').split()) == ' '.join(command.split())


def metadata_only_read(title=None, kind=None, raw_input=None):
    args = raw_input if isinstance(raw_input, Mapping) else {}
    name = str(title or '').strip()
    if args.get('server') == 'cortex' and isinstance(args.get('tool'), str):
        name = 'mcp.cortex.' + args['tool']
    match = re.match(r'^mcp(?:__|\.)([^.]+?)(?:__|\.)([\w-]+)', name, re.I)
    if match:
        return match[1].lower() == 'cortex' and match[2].lower() in {'project_read','read_skill'}
    if shell_command_call(title, kind, raw_input):
        return False
    return name.lower() in {'project_read','read_skill'} or bool(re.match(r'^(read|view|cat)(?:\s|$)',name,re.I)) or kind == 'read'


# Fields of a Cortex tool error that never carry a submitted value; ``received``
# can quote part of one and is left out.
_SAFE_ERROR_FIELDS = ('error', 'field', 'expected', 'correction')
_MAX_ERROR_TEXT = 4000
_MAX_ERROR_FIELD = 300


def _cortex_error(value, depth=0):
    """The first Cortex tool-error object inside a tool output, reduced to
    its value-free fields, or None."""
    if depth > 6:
        return None
    if isinstance(value, str):
        if len(value) > _MAX_ERROR_TEXT or not value.lstrip().startswith('{'):
            return None
        try:
            import json
            parsed = json.loads(value)
        except ValueError:
            return None
        if isinstance(parsed, Mapping) and isinstance(parsed.get('error'), str):
            return {key: parsed[key][:_MAX_ERROR_FIELD] for key in _SAFE_ERROR_FIELDS
                    if isinstance(parsed.get(key), str)}
        return None
    if isinstance(value, Mapping):
        items = value.values()
    elif isinstance(value, list):
        items = value
    else:
        return None
    for item in items:
        found = _cortex_error(item, depth + 1)
        if found:
            return found
    return None


def compact_read(update):
    result = {k:v for k,v in update.items() if k not in {
        'content','text','raw_output','rawOutput','output','error','diff',
    }}
    # A failed read keeps its observed status, without copying a potentially
    # large response/error envelope back into the display transcript. A
    # Cortex refusal keeps its value-free reason so the card says why.
    if update.get('status') == 'failed':
        result['rawOutput' if 'toolCallId' in update else 'raw_output'] = failed_read_output(
            *(update.get(key) for key in ('rawOutput','raw_output','content','output')))
    return result


def failed_read_output(*outputs):
    """What a failed metadata-only read keeps: its status, plus the
    value-free reason when the output is a Cortex tool refusal."""
    failed = {'status':'failed'}
    reason = _cortex_error(list(outputs))
    if reason:
        failed['error'] = reason
    return failed


class ReadHistoryFilter:
    def __init__(self):
        self.reads = set()

    def project(self, update):
        ref = update.get('toolCallId') or update.get('tool_call_id')
        if not ref or str(ref).startswith('worker:'):
            return update
        if metadata_only_read(update.get('title'),update.get('kind'),update.get('rawInput',update.get('raw_input'))):
            self.reads.add(ref)
        return compact_read(update) if ref in self.reads else update
