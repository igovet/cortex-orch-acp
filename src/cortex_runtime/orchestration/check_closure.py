"""Deferred check closure from runtime-owned final verification, never claims."""
import json
import os
from pathlib import Path
import re
import stat


def _retained_output_complete(db, task_id, output):
    """Read the exact task-private body without creating or repairing storage."""
    task = db.execute('SELECT project_root FROM tasks WHERE id=?', (task_id,)).fetchone()
    filename = output['filename']
    if (not task or not isinstance(filename, str)
            or filename != output['id'] + '.bin'
            or not re.fullmatch(r'[A-Za-z0-9_-]+\.bin', filename)
            or not re.fullmatch(r'[A-Za-z0-9_-]+', task_id)):
        return False
    if not isinstance(task['project_root'], str):
        return False
    root = Path(task['project_root'])
    if not root.is_absolute() or '..' in root.parts:
        return False
    descriptors = []
    try:
        directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
        directory = os.open(root.anchor, directory_flags)
        descriptors.append(directory)
        for index, part in enumerate(root.parts[1:] + ('.cortex', task_id, 'large_outputs')):
            directory = os.open(part, directory_flags, dir_fd=directory)
            descriptors.append(directory)
            if index >= len(root.parts) - 1:
                info = os.fstat(directory)
                if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
                    return False
        body = os.open(filename, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        descriptors.append(body)
        before = os.fstat(body)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or stat.S_IMODE(before.st_mode) & 0o077
                or before.st_size != output['byte_count']):
            return False
        remaining = output['byte_count']
        while remaining:
            chunk = os.read(body, min(remaining, 64 * 1024))
            if not chunk:
                return False
            remaining -= len(chunk)
        if os.read(body, 1):
            return False

        def signature(info):
            return (info.st_dev, info.st_ino, info.st_size, info.st_mode,
                    info.st_uid, info.st_nlink, info.st_mtime_ns, info.st_ctime_ns)

        return (signature(before) == signature(os.fstat(body))
                == signature(os.stat(filename, dir_fd=directory, follow_symlinks=False)))
    except (OSError, ValueError):
        return False
    finally:
        for descriptor in reversed(descriptors):
            os.close(descriptor)


def verified_runtime_runs(db, task_id, plan_revision):
    """Exact final verification receipts, or None for absent/invalid evidence."""
    latest = db.execute(
        "SELECT id,type,actor,payload_json FROM runtime_events WHERE task_id=? "
        "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
        (task_id,)).fetchone()
    if not latest or latest['type'] != 'VERIFICATION_PASSED' or latest['actor'] != 'runtime':
        return None
    payload = json.loads(latest['payload_json'])
    digest = payload.get('revision_digest')
    if payload.get('revision') != plan_revision or not isinstance(digest, str) or not re.fullmatch(r'[0-9a-f]{64}', digest):
        return None
    task = db.execute('SELECT plan_json FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
    commands = json.loads(task['plan_json'] or '{}').get('verification', [])
    checks = payload.get('checks', [])
    if not commands or len(checks) != len(commands):
        return None
    refs = [check.get('command_ref') for check in checks]
    if any(not isinstance(ref, str) for ref in refs) or len(set(refs)) != len(refs):
        return None
    from .state import reviewed_digest
    from ..delegation.revision import capture_revision
    from ..task_workspace import task_root
    if reviewed_digest(db, task_id, plan_revision) != digest:
        return None
    observed = capture_revision(task_root(db, task_id))
    if observed.get('kind') != 'observed' or observed.get('dirty_digest') != digest:
        return None

    runs = []
    for index, (check, argv) in enumerate(zip(checks, commands)):
        ref = check['command_ref']
        start_id = check.get('started_event_id')
        if (not re.fullmatch(r'verification:' + str(plan_revision) + r':\d+:' + str(index), ref)
                or check.get('argv') != argv or check.get('complete') is not True
                or type(check.get('exit_code')) is not int or check['exit_code'] != 0
                or check.get('revision_digest') != digest or check.get('failure') is not None
                or check.get('truncated') or type(start_id) is not int or start_id >= latest['id']):
            return None
        starts = db.execute(
            "SELECT id,actor,payload_json FROM runtime_events WHERE task_id=? "
            "AND type='CHECK_EXECUTION_STARTED' AND json_extract(payload_json,'$.command_ref')=?",
            (task_id, ref)).fetchall()
        binding = dict(command_ref=ref, argv=argv, revision=plan_revision, revision_digest=digest, index=index)
        if 'profile' in check:
            binding['profile'] = check['profile']
        if (len(starts) != 1 or starts[0]['id'] != start_id or starts[0]['actor'] != 'runtime'
                or json.loads(starts[0]['payload_json']) != binding):
            return None
        outputs = db.execute(
            'SELECT * FROM large_outputs WHERE task_id=? AND source_ref=?',
            (task_id, ref)).fetchall()
        if (len(outputs) != 1 or outputs[0]['id'] != check.get('large_output_id')
                or outputs[0]['owner_scope'] != 'coordinator' or outputs[0]['source_kind'] != 'terminal'
                or outputs[0]['truncated'] or type(outputs[0]['exit_code']) is not int
                or outputs[0]['exit_code'] != 0 or outputs[0]['failure'] is not None
                or outputs[0]['signal'] is not None or type(outputs[0]['byte_count']) is not int
                or outputs[0]['byte_count'] < 0):
            return None
        if not _retained_output_complete(db, task_id, outputs[0]):
            return None
        runs.append((argv, ref))

    return runs


def runtime_gap_closures(db, task_id, plan_revision):
    """Rederive assignment/check-index -> command ref from exact retained evidence.

    Execution and the gap author's publication/review bind to the same approved,
    current digest, regardless of ordering. The start event authenticates the
    run, not its eligibility relative to publication. Legacy observations without
    those bindings fail closed. Stored closed_by fields are not consulted.
    """
    nodes = db.execute(
        'SELECT assignment_ref,stage,result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND result_json IS NOT NULL',
        (task_id, plan_revision)).fetchall()
    if not any(check.get('outcome') in {'not_run', 'unverified'} and check.get('command')
               for node in nodes for check in json.loads(node['result_json']).get('checks', [])):
        return {}
    runs = verified_runtime_runs(db, task_id, plan_revision)
    if runs is None:
        return {}
    from .state import reviewed_digest
    digest = reviewed_digest(db, task_id, plan_revision)

    completed = db.execute(
        "SELECT id,payload_json FROM runtime_events WHERE task_id=? "
        "AND type='WORKER_COMPLETED' AND actor='runtime'", (task_id,)).fetchall()
    by_assignment = {}
    for row in completed:
        item = json.loads(row['payload_json'])
        by_assignment.setdefault(item.get('assignment_ref'), []).append(item)
    closures = {}
    for node in nodes:
        assignment = node['assignment_ref']
        records = by_assignment.get(assignment, [])
        if len(records) != 1:
            continue
        completion = records[0]
        recorded = completion.get('result', {})
        if node['stage'] == 'review':
            author_digest = recorded.get('project_digest')
        else:
            # Publication captures are runtime-owned and bound to this exact
            # attempt and immutable report, not a model-supplied digest.
            provision = db.execute(
                'SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?',
                (task_id, assignment)).fetchone()
            if not provision or provision['attempt_id'] != completion.get('attempt_id'):
                continue
            publications = db.execute(
                'SELECT kind,dirty_digest FROM attempt_revisions WHERE attempt_id=? AND phase=?',
                (provision['attempt_id'], 'worker_publish:' + str(recorded.get('report_ref')))).fetchall()
            if len(publications) != 1 or publications[0]['kind'] != 'observed':
                continue
            author_digest = publications[0]['dirty_digest']
        if author_digest != digest:
            continue
        original = recorded.get('checks', [])
        for index, check in enumerate(json.loads(node['result_json']).get('checks', [])):
            if check.get('outcome') not in {'not_run', 'unverified'} or index >= len(original):
                continue
            # Bind to the real recorded gap, ignoring only the derived closure.
            if {k: v for k, v in check.items() if k != 'closed_by'} != original[index]:
                continue
            argv = check.get('command')
            if not isinstance(argv, list) or not argv or not all(isinstance(word, str) for word in argv):
                continue
            matches = [ref for command, ref in runs if command == argv]
            if matches:
                closures[(assignment, index)] = matches[-1]
    return closures


def apply_runtime_closures(checks, assignment, closures):
    """Keep disclosures as history and replace only rederived gap closure refs."""
    result = []
    for index, check in enumerate(checks):
        value = dict(check)
        if value.get('outcome') in {'not_run', 'unverified'}:
            value.pop('closed_by', None)
            ref = closures.get((assignment, index))
            if ref:
                value['closed_by'] = ref
        result.append(value)
    return result


def retain_runtime_closures(db, task_id, plan_revision):
    """Persist the derived references transactionally; immutable reports stay intact."""
    from .state import encoded
    closures = runtime_gap_closures(db, task_id, plan_revision)
    for node in db.execute(
            'SELECT node_id,assignment_ref,result_json FROM runtime_nodes '
            'WHERE task_id=? AND revision=? AND result_json IS NOT NULL', (task_id, plan_revision)).fetchall():
        result = json.loads(node['result_json'])
        checks = result.get('checks', [])
        updated = apply_runtime_closures(checks, node['assignment_ref'], closures)
        if updated != checks:
            result['checks'] = updated
            db.execute('UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND revision=? AND node_id=?',
                       (encoded(result), task_id, plan_revision, node['node_id']))
    return closures
