// Fake wire endpoint for no-paid tests only, NOT the installed Paseo daemon.
import { WebSocketServer } from 'ws';
import { readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { FetchAgentResponseMessageSchema } from '@getpaseo/protocol/messages';
const [project, sid, agent, scenario, python] = process.argv.slice(2);
const counts = {};
let injection = { attempted: false };
const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
server.on('listening', () => console.log(JSON.stringify({ port: server.address().port })));
server.on('connection', (ws) => {
  ws.on('message', (raw) => {
    const envelope = JSON.parse(raw.toString());
    const m = envelope.type === 'session' ? envelope.message : envelope;
    counts[m.type] = (counts[m.type] ?? 0) + 1;
    const send = (message) => ws.send(JSON.stringify({ type: 'session', message }));
    if (m.type === 'hello') {
      if (scenario === 'auth') { ws.close(1008, 'authentication required'); return; }
      if (scenario !== 'timeout') send({ type: 'status', payload: { status: 'server_info', serverId: scenario === 'server' ? 'wrong-server' : 'fake-dev-server' } });
      return;
    }
    if (m.type !== 'fetch_agent_request') return;
    const a = { id: agent, provider: 'cortex', cwd: project, workspaceId: 'workspace-owned',
      model: null, createdAt: '2026-10-01T00:00:00Z', updatedAt: '2026-10-01T00:00:00Z',
      lastUserMessageAt: null, status: 'running', capabilities: { supportsStreaming: true,
        supportsSessionPersistence: true, supportsDynamicModes: false, supportsMcpServers: false,
        supportsReasoningStream: false, supportsToolInvocations: false }, currentModeId: null,
      availableModes: [], pendingPermissions: [], persistence: null, title: null, labels: {},
      runtimeInfo: { provider: 'cortex', sessionId: 'plugin:' + JSON.stringify({version:1,data:{sessionId:sid}}) } };
    if (scenario === 'runtime-provider') a.runtimeInfo.provider = 'codex';
    if (scenario === 'session-absent') a.runtimeInfo.sessionId = null;
    if (scenario === 'session-missing') a.runtimeInfo.sessionId = 'plugin:' + JSON.stringify({version:1,data:{sessionId:'acps_bbbbbbbbbbbb'}});
    if (scenario === 'id') a.id = '22222222-2222-2222-2222-222222222222';
    if (scenario === 'provider') a.provider = 'codex';
    if (scenario === 'workspace') a.workspaceId = 'workspace-other';
    if (scenario === 'cwd') a.cwd = '/unrelated';
    if (scenario === 'wrapper') a.runtimeInfo.sessionId = 'plugin:invalid';
    if (scenario === 'duplicate') a.runtimeInfo.sessionId = 'plugin:{"version":1,"version":1,"data":{"sessionId":"' + sid + '"}}';
    if (scenario === 'escaped') a.runtimeInfo.sessionId = 'plugin:{"version":1,"data":{"\\u0073essionId":"' + sid + '"}}';
    if (scenario === 'version') a.runtimeInfo.sessionId = 'plugin:{"version":2,"data":{"sessionId":"' + sid + '"}}';
    if (scenario === 'oversize') a.title = 'x'.repeat(70000);
    if (scenario === 'drift' && counts[m.type] === 2) a.runtimeInfo.sessionId = 'plugin:{"version":1,"data":{"sessionId":"acps_bbbbbbbbbbbb"}}';
    if ((scenario === 'epoch' || scenario === 'epoch-failure') && counts[m.type] === 2) {
      try {
        const output = execFileSync(python, ['-B', '-c',
          'import json,sys; from pathlib import Path; from cortex_runtime.store import Store; from cortex_runtime.storage.inspection import discover_binding; from cortex_runtime.ownership import acquire_task_owner; p=sys.argv[1]; sid=sys.argv[2]; b=discover_binding(p,sid)["binding"]; sys.exit(7) if sys.argv[3]=="epoch-failure" else None; s=Store(Path(p)/".cortex",project_root=p); acquire_task_owner(s,b["task_id"],"new-fixture-owner",b["owner_epoch"]); after=discover_binding(p,sid)["binding"]; assert after["task_id"]==b["task_id"] and after["owner_epoch"]==b["owner_epoch"]+1; print(json.dumps({"attempted":True,"complete":True,"before_epoch":b["owner_epoch"],"after_epoch":after["owner_epoch"]}))',
          project, sid, scenario], { env: process.env, timeout: 5000, maxBuffer: 8192, stdio: 'pipe' });
        injection = JSON.parse(output.toString());
      } catch (error) {
        // Fixture failure is not observed storage drift. Keep the server alive
        // for deterministic diagnostics and supervised teardown, without logs.
        injection = { attempted: true, complete: false, error: 'fixture_epoch_injection_failed',
          exit_code: Number.isInteger(error.status) ? error.status : null };
        send(FetchAgentResponseMessageSchema.parse({ type: 'fetch_agent_response',
          payload: { requestId: m.requestId, agent: null, project: null, error: injection.error } }));
        return;
      }
    }
    if (scenario === 'server-drift' && counts[m.type] === 2)
      send({ type: 'status', payload: { status: 'server_info', serverId: 'drifted-server' } });
    if (scenario === 'endpoint-drift' && counts[m.type] === 2) {
      const file = join(homedir(), '.paseo-dev/paseo.pid');
      const metadata = JSON.parse(readFileSync(file, 'utf8'));
      metadata.startedAt = '2026-10-01T01:00:00Z';
      writeFileSync(file, JSON.stringify(metadata));
    }
    send(FetchAgentResponseMessageSchema.parse({ type: 'fetch_agent_response', payload: { requestId: m.requestId, agent: a, project: null, error: null } }));
  });
});
createInterface({ input: process.stdin }).on('line', (command) => console.log(JSON.stringify(command === 'injection' ? injection : counts)));
process.on('SIGTERM', () => { for (const ws of server.clients) ws.terminate(); server.close(() => process.exit(0)); });
