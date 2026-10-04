"""Human-readable projection of durable runtime state into the one pipeline."""
from pathlib import Path
import json

from ..routing.decision_display import route_decision_summary


def _recorded_route_summary(store, task_id, assignment_ref):
    if not assignment_ref:
        return None
    with store.connection() as db:
        audit = db.execute(
            "SELECT a.engine, a.model, a.decision_detail_json "
            "FROM delegation_provisioning p JOIN attempt_route_audit a ON a.attempt_id=p.attempt_id "
            "WHERE p.task_id=? AND p.idempotency_key=? ORDER BY a.recorded_at DESC LIMIT 1",
            (task_id, assignment_ref),
        ).fetchone()
    if audit is None:
        return None
    try:
        detail = json.loads(audit["decision_detail_json"] or "{}")
    except (TypeError, json.JSONDecodeError):
        return None
    return route_decision_summary(audit["engine"], audit["model"], detail)


def publish(store, channel, context):
    state = context['state']
    waits = [node.get('wait', {}).get('event_id') for node in context['nodes'] if node.get('wait')]
    wait_key = ':wait:' + ','.join(map(str, waits)) if waits else ''
    key = f"runtime-pipeline:{channel.channel_id}:{context['revision']}:{context['review_cycle']}:{state}" + wait_key
    draft = store.call('create_draft', dict(template='pipeline', request_key=key),
                       channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
    path = Path(draft['draft_path'])
    if path.exists():
        marker = path.read_text().split('\n\n', 1)[0]
        rows = [marker, '', '# Runtime pipeline', '', f"State: {state}",
                f"Plan revision: {context['revision']}", '', '## Original objective', '',
                '> ' + context['goal'].replace('\n','\n> '), '', '## Assignments', '']
        for node in context['nodes']:
            result = node['result'] or {}
            route = _recorded_route_summary(store, context["task_id"], node.get("assignment_ref"))
            wait = node.get('wait')
            wait_label = {'open': 'waiting for answer', 'ready': 'answer ready',
                          'resuming': 'answer continuation in progress'}.get(wait['status']) if wait else None
            if wait and wait.get('kind') == 'access':
                wait_label = {'open': 'waiting for access decision', 'ready': 'access decision ready',
                              'resuming': 'access continuation in progress'}.get(wait['status'])
            rows.append(f"- {node['node_id']} ({node['role']}): {node['status']}; "
                        f"dependencies: {', '.join(node['dependencies']) or 'none'}; "
                        f"report: {result.get('report_ref', 'not published')}"
                        + (f"; routing: {route}" if route else "")
                        + (f"; {wait_label}" if wait_label else ""))
            if node.get('access', {}).get('declared_scope_revision'):
                rows.append('  - Runtime declared scope: ' + ', '.join(node['access']['effective_declared_scope'])
                            + '; enforcement: already project writable (no sandbox remount).')
        waiver = context.get('review_not_required')
        if waiver:
            reference = ('the project as observed before the planned work' if waiver['reference'] == 'execution_baseline'
                         else f"the project an independent reviewer approved in plan revision {waiver['reference_revision']}")
            rows.extend(['', '## Independent review', '',
                         f"Not required ({waiver['reason']}): the runtime observed no project file changes; "
                         f"the project digest {waiver['revision_digest']} equals {reference}."])
        if context.get('verification'):
            rows.extend(['','## Runtime verification','',json.dumps(context['verification'],ensure_ascii=False,indent=2)])
        if context['synthesis']:
            rows.extend(['', '## Synthesis', '', context['synthesis']])
        if context['blocker']:
            rows.extend(['', '## Blocker', '', context['blocker']])
        path.write_text('\n'.join(rows) + '\n')
    return store.call('write_report', dict(draft_id=draft['draft_id'], title=f'Runtime {state}',
                      summary=f"Plan revision {context['revision']}; stage {state}.", author='runtime', request_key=key+':publish'),
                      channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
