"""Runtime-observed review context, never worker interpretation or acceptance."""
import hashlib
import json

from ..delegation.evidence import assess_checks, current_outcomes
from ..delegation.revision import capture_revision
from ..recovery.large_output_store import persist_output

FACTS_MAX_BYTES = 4096
_VERIFICATION_NOTE = (
    "Planned commands are not results. The runtime itself runs this plan's exact "
    "verification argv, authoritatively, on the approved digest (or credits a matching "
    "speculative run only after approval); do not re-run that same argv solely to "
    "duplicate it, and never record it as your own deferred or not-run check — the "
    "runtime closes it from its own receipt. A matching deferred check closes only "
    "with identical argv, complete passing retained runtime evidence and matching "
    "author, approved and current digests. Independent review and any additional or "
    "narrower check your own analysis needs remain your responsibility."
)


def path_metadata(observed):
    """Reuse the no-follow revision capture; retain no content or link targets."""
    if observed.get('kind') != 'observed' or not isinstance(observed.get('manifest'), list):
        return None
    entries = {}
    for record in observed['manifest']:
        if not isinstance(record, str):
            return None
        kind, separator, payload = record.partition('\t')
        if not separator or kind not in {'f', 'l', 'd', 'p', 's', 'dev', 'o'}:
            return None
        try:
            if kind == 'l':
                # Both path and target are unescaped. Only a unique placeholder
                # boundary is decodable; guessing would invent observed facts.
                boundary = payload.find('\t-\t')
                if boundary < 0 or payload.find('\t-\t', boundary + 1) >= 0:
                    return None
                path, digest = payload.split('\t-\t')
                if not digest:
                    return None
                digest = hashlib.sha256(digest.encode('utf-8', 'surrogateescape')).hexdigest()
                size_bytes = None
            else:
                # Fixed right-hand fields leave tabs/newlines in the path intact.
                path, size, digest = payload.rsplit('\t', 2)
                if kind == 'f':
                    if (not size.isascii() or not size.isdecimal() or len(digest) != 64
                            or any(char not in '0123456789abcdef' for char in digest)):
                        return None
                    size_bytes = int(size)
                else:
                    if (size, digest) != ('-', '-'):
                        return None
                    digest, size_bytes = None, None
        except (ValueError, UnicodeError):
            return None
        if not path or path in entries:
            return None
        entries[path] = dict(kind=kind, sha256=digest, size_bytes=size_bytes)
    return entries


def _encoded(value):
    # ASCII escaping keeps arbitrary filenames/labels data, not Markdown instructions.
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True)


def review_runtime_facts(store, task_id, assignment_ref, root):
    """Build and retain a bounded allowlisted block for this review binding."""
    observed = capture_revision(root)
    current = path_metadata(observed)
    with store.connection() as db:
        task = db.execute('SELECT revision,plan_json FROM runtime_tasks WHERE task_id=?',
                          (task_id,)).fetchone()
        binding_row = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' "
            "AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
            (task_id, assignment_ref)).fetchone()
        binding = json.loads(binding_row[0]) if binding_row else {}
        baseline_row = db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='EXECUTION_BASELINE' "
            "AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1",
            (task_id, task['revision'])).fetchone()
        baseline = json.loads(baseline_row[0]) if baseline_row else {}
        before = baseline.get('path_metadata')
        bound = (observed.get('kind') == 'observed' and binding.get('revision_digest')
                 == observed.get('dirty_digest'))
        changed = []
        if bound and current is not None and isinstance(before, dict):
            for path in sorted(set(before) | set(current)):
                if before.get(path) != current.get(path):
                    changed.append(dict(path=path, before=before.get(path), after=current.get(path)))
        checks = []
        rows = db.execute(
            "SELECT node_id,assignment_ref,result_json FROM runtime_nodes WHERE task_id=? "
            "AND revision=? AND stage='execution' AND result_json IS NOT NULL ORDER BY rowid",
            (task_id, task['revision'])).fetchall()
        for row in rows:
            provision = db.execute(
                'SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?',
                (task_id, row['assignment_ref'])).fetchone()
            if not provision:
                continue
            reported = json.loads(row['result_json']).get('checks', [])
            assessments = assess_checks(db, task_id, provision['attempt_id'], reported)
            outcomes = current_outcomes(db, task_id, provision['attempt_id'], reported)
            for check, assessment, outcome in zip(reported, assessments, outcomes):
                if assessment['strength'] != 'verified':
                    continue
                checks.append(dict(node=row['node_id'], name=check['name'], outcome=check['outcome'],
                                   current_outcome=outcome['outcome'], closed_by=outcome['closed_by'],
                                   receipt_ref=check['tool_call_ref'],
                                   digest_binding='not established for reviewed digest'))
        verification = json.loads(task['plan_json'] or '{}').get('verification', [])

    facts = dict(kind='runtime_verified_facts', revision=task['revision'],
                 review_digest=binding.get('revision_digest'),
                 observed_digest=observed.get('dirty_digest'),
                 changed_paths_status=('review_binding_changed' if not bound else
                                       'path_metadata_unavailable' if current is None else
                                       'baseline_unavailable' if not isinstance(before, dict) else 'observed'),
                 comparison='Current capture versus this plan execution baseline; not authorship.',
                 verification_note=_VERIFICATION_NOTE,
                 check_labels='Names are submitted labels, not verified interpretations.',
                 checks=[], verification_argv=[], changed_paths=[],
                 truncated=False, omitted=dict(checks=0, verification_argv=0, changed_paths=0))
    # Reserve room for the retained-output reference and the truncation counters.
    for key, records in (('verification_argv', verification), ('checks', checks),
                         ('changed_paths', changed)):
        for record in records:
            facts[key].append(record)
            if len(_encoded(facts).encode('utf-8')) > FACTS_MAX_BYTES - 160:
                facts[key].pop()
                facts['omitted'][key] += 1
                facts['truncated'] = True
    data = _encoded(facts).encode('utf-8')
    saved = persist_output(store, task_id=task_id, owner_scope=assignment_ref,
                           source_kind='tool_result', source_ref='review_runtime_facts',
                           data=data, exit_code=0, failure=None)
    facts['large_output_id'] = saved['id']
    return facts
