"""Human-readable projection of durable runtime state into the one pipeline."""
from pathlib import Path
import json


def publish(store, channel, context):
    state = context['state']
    key = f"runtime-pipeline:{channel.channel_id}:{context['revision']}:{context['review_cycle']}:{state}"
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
            rows.append(f"- {node['node_id']} ({node['role']}): {node['status']}; "
                        f"dependencies: {', '.join(node['dependencies']) or 'none'}; "
                        f"report: {result.get('report_ref', 'not published')}")
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
