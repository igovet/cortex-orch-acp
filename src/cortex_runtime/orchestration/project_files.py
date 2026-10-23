"""Bounded file pages and durable channel-local read coverage.

State uses the existing runtime_requests ledger, not model-supplied tokens.
Only digests and line ranges are retained; file bodies remain on disk.
"""
import difflib
import hashlib
import json

from ..contracts import StoreError

MAX_INLINE_BYTES = 24000
DEFAULT_LINE_LIMIT = 2000
MAX_LINE_CHARACTERS = 2000
MAX_COVERAGE_RANGES = 2048
MAX_DIFF_CHARACTERS = 32000
MAX_DIFF_LINE_PAIRS = 250000


def inline_size(result):
    """Measure the entire MCP result, including its duplicate structured data."""
    wire = dict(content=[dict(type='text', text=json.dumps(result, ensure_ascii=False))],
                structuredContent=result, isError=False)
    return len(json.dumps(wire, ensure_ascii=False).encode('utf-8'))


def text_page(body, digest, args, size_bytes):
    lines = body.splitlines(keepends=True)
    start = args.get('offset', 1)
    limit = args.get('limit', DEFAULT_LINE_LIMIT)
    numbered = args.get('line_numbers', True)
    result = dict(path=args['path'], sha256=digest, size_bytes=size_bytes,
                  total_characters=len(body), total_lines=len(lines),
                  start_line=start, end_line=start-1, content='', line_numbers=numbered,
                  complete=False, partial=True, next_offset=None, truncated_lines=[])
    pieces = []
    end = min(len(lines), start-1+limit)
    for index in range(start-1, end):
        line = lines[index]
        truncated = len(line.rstrip('\r\n')) > MAX_LINE_CHARACTERS
        shown = line[:MAX_LINE_CHARACTERS] + '… [line truncated]\n' if truncated else line
        piece = f'{index+1:6}\t{shown}' if numbered else shown
        candidate = dict(result, content=''.join(pieces)+piece, end_line=index+1,
                         truncated_lines=result['truncated_lines']+([index+1] if truncated else []))
        if inline_size(candidate) > MAX_INLINE_BYTES-1024:
            if pieces:
                break
            # JSON escaping can make even a <=2000-character line too large.
            # Return a flagged shorter prefix, never an empty non-advancing page.
            prefix = line[:MAX_LINE_CHARACTERS]
            while inline_size(candidate) > MAX_INLINE_BYTES-1024:
                prefix = prefix[:len(prefix)//2]
                shown = prefix + '… [line truncated by inline budget]\n'
                piece = f'{index+1:6}\t{shown}' if numbered else shown
                candidate.update(content=piece, truncated_lines=[index+1])
        pieces.append(piece)
        result = candidate
    result['next_offset'] = result['end_line']+1 if result['end_line'] < len(lines) else None
    result['complete'] = start == 1 and result['end_line'] == len(lines) and not result['truncated_lines']
    result['partial'] = not result['complete']
    if not lines:
        result['notice'] = 'File exists and is empty.'
    elif start > len(lines):
        result['notice'] = 'Offset is past the last line.'
    return result


def directory_page(entries, args):
    start = args.get('offset', 1)
    result = dict(entries=[], total_entries=len(entries), next_offset=None)
    for entry in entries[start-1:start-1+args.get('limit', DEFAULT_LINE_LIMIT)]:
        if inline_size(dict(result, entries=result['entries']+[entry])) > MAX_INLINE_BYTES-1024:
            break
        result['entries'].append(entry)
    next_offset = start+len(result['entries'])
    result['next_offset'] = next_offset if next_offset <= len(entries) else None
    return result


def _key(root, path):
    from .gateway import _parts
    identity = str(root) + '\0' + '/'.join(_parts(path))
    return 'file-read:' + hashlib.sha256(identity.encode()).hexdigest()


def read_state(db, channel, root, path):
    row = db.execute('SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? '
                     "AND operation='project_read_state'",
                     (channel.task_id, channel.channel_id, _key(root, path))).fetchone()
    return json.loads(row[0])['read_state'] if row else None


def record_read(db, channel, root, args, result):
    # Images, binary metadata, directory listings and missing paths are not text reads.
    if 'total_lines' not in result:
        return
    previous = read_state(db, channel, root, args['path'])
    ranges = previous['ranges'] if previous and previous['sha256'] == result['sha256'] else []
    truncated = set(result['truncated_lines'])
    # Each non-truncated contiguous range counts; line prefixes do not.
    begin = None
    for line in range(result['start_line'], result['end_line']+2):
        if line <= result['end_line'] and line not in truncated:
            if begin is None:
                begin = line
        elif begin is not None:
            ranges.append([begin, line-1])
            begin = None
    merged = []
    for begin, end in sorted(ranges):
        if merged and begin <= merged[-1][1]+1:
            merged[-1][1] = max(end, merged[-1][1])
        else:
            merged.append([begin, end])
    # Fragmented adversarial reads fail closed rather than growing state without bound.
    merged = merged[:MAX_COVERAGE_RANGES]
    complete = result['total_lines'] == 0 or merged == [[1, result['total_lines']]]
    state = dict(path=args['path'], sha256=result['sha256'], complete=complete, ranges=merged)
    payload = json.dumps(dict(complete=True, read_state=state))
    db.execute('INSERT OR REPLACE INTO runtime_requests VALUES(?,?,?,?,?,?)',
               (channel.task_id, _key(root, args['path']), channel.channel_id, 'project_read_state',
                result['sha256'], payload))
    result['read_coverage_complete'] = complete
    # A whole-file digest is an identity, NOT overwrite authority on a partial read.
    result['full_content_sha256'] = result['sha256'] if complete else None


def record_own_write(db, channel, root, path, digest, *, complete, total_lines):
    """After this channel's own successful write or edit it knows the bytes it
    produced: record them as read, so its next edit of the same file needs no
    re-read. Complete only when everything it wrote was known to it; a later
    change by anyone else still fails as a stale read."""
    ranges = [[1, total_lines]] if complete and total_lines else []
    state = dict(path=path, sha256=digest, complete=complete, ranges=ranges)
    db.execute('INSERT OR REPLACE INTO runtime_requests VALUES(?,?,?,?,?,?)',
               (channel.task_id, _key(root, path), channel.channel_id, 'project_read_state',
                digest, json.dumps(dict(complete=True, read_state=state))))


def require_read(state, digest, *, complete):
    if state is None or (complete and not state['complete']):
        raise StoreError('gateway_complete_read_required' if complete else 'gateway_read_required',
                         'path', expected='a current complete read' if complete else 'a prior text read',
                         correction='Read the file with project_read first. For whole-file replacement, read all '
                         'pages without truncated lines; a partial page sha256 never authorizes project_write. '
                         'Use project_edit for partial changes. This is a read precondition, not an access refusal; '
                         'nothing was changed. No owner question or extra approval is needed under existing authority. '
                         'Follow the tool delivery-key rules when re-proposing.')
    if state['sha256'] != digest:
        raise StoreError('gateway_stale_read', 'path', expected='the content last read by this channel',
                         correction='The file changed since this channel read it. Read the current file again '
                         'and rebuild the patch or replacement against that content; never blindly retry a stale patch. '
                         'Nothing was changed. This is a read precondition, not an access refusal; no owner question '
                         'or extra approval is needed under existing authority. Follow the tool delivery-key rules.')


def apply_edits(body, edits):
    for index, edit in enumerate(edits):
        old, new = edit['old_string'], edit['new_string']
        if old == new:
            raise StoreError('gateway_edit_no_change', f'edits[{index}]',
                             correction='old_string and new_string must differ.')
        count = body.count(old)
        if not count:
            raise StoreError('gateway_edit_not_found', f'edits[{index}].old_string',
                             correction='Match the exact current text, including whitespace; omit read line numbers.')
        if body.find(old, body.find(old)+1) != -1 and not edit.get('replace_all', False):
            raise StoreError('gateway_edit_not_unique', f'edits[{index}].old_string',
                             correction='Include more surrounding context or set replace_all to replace every occurrence.')
        replacements = count if edit.get('replace_all', False) else 1
        new_size = len(body.encode('utf-8')) + replacements * (len(new.encode('utf-8')) - len(old.encode('utf-8')))
        if new_size > 16000000:
            raise StoreError('gateway_file_too_large', f'edits[{index}]',
                             correction='The replacement would exceed the 16 MB file limit. Nothing was changed.')
        body = body.replace(old, new, -1 if edit.get('replace_all', False) else 1)
    return body


def compact_diff(before, after, path):
    # SequenceMatcher computes opcodes before yielding anything. Bound its
    # inputs as well as output, including quadratic repetitive-line matching.
    omitted = dict(diff='', diff_truncated=True, diff_omitted='input_budget')
    if len(before) + len(after) > MAX_DIFF_CHARACTERS:
        return omitted
    old_lines, new_lines = before.splitlines(keepends=True), after.splitlines(keepends=True)
    if len(old_lines) * len(new_lines) > MAX_DIFF_LINE_PAIRS:
        return omitted
    pieces, used, truncated = [], 0, False
    for line in difflib.unified_diff(old_lines, new_lines, fromfile=path, tofile=path, n=2):
        if len(line.encode('utf-8')) > 2000:
            line = line[:400] + '… [diff line truncated]\n'
            truncated = True
        size = len(line.encode('utf-8'))
        if used+size > 4000:
            truncated = True
            break
        pieces.append(line)
        used += size
    result = dict(diff=''.join(pieces), diff_truncated=truncated)
    if len(before.encode('utf-8')) + len(after.encode('utf-8')) <= 4000:
        result.update(oldText=before, newText=after)
    return result


def bound_mutation_result(result):
    """Keep the post-advisory receipt inline without losing machine evidence."""
    if inline_size(result) > MAX_INLINE_BYTES and 'required_before_result_submit' in result:
        result.pop('required_before_result_submit')
        result['skill_hint_unavailable'] = 'inline_limit; result_submit still enforces the gate'
    if inline_size(result) > MAX_INLINE_BYTES:
        result.pop('oldText', None)
        result.pop('newText', None)
    while inline_size(result) > MAX_INLINE_BYTES and result.get('diff'):
        result['diff'] = result['diff'][:len(result['diff'])//2]
        result['diff_truncated'] = True
