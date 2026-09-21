"""Keep read targets/status in display history, never the returned document body.

This projection does not touch the MCP response delivered to the model.
"""
import re
from collections.abc import Mapping


def metadata_only_read(title=None, kind=None, raw_input=None):
    args = raw_input if isinstance(raw_input, Mapping) else {}
    name = str(title or '').strip()
    if args.get('server') == 'cortex' and isinstance(args.get('tool'), str):
        name = 'mcp.cortex.' + args['tool']
    match = re.match(r'^mcp(?:__|\.)([^.]+?)(?:__|\.)([\w-]+)', name, re.I)
    if match:
        return match[1].lower() == 'cortex' and match[2].lower() in {'project_read','read_skill'}
    return name.lower() in {'project_read','read_skill'} or bool(re.match(r'^(read|view|cat)(?:\s|$)',name,re.I)) or kind == 'read'


def compact_read(update):
    result = {k:v for k,v in update.items() if k not in {
        'content','text','raw_output','rawOutput','output','error','diff',
    }}
    # A failed read keeps its observed status, without copying a potentially
    # large response/error envelope back into the display transcript.
    if update.get('status') == 'failed':
        result['rawOutput' if 'toolCallId' in update else 'raw_output'] = {'status':'failed'}
    return result


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
