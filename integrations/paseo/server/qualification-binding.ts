/** Internal read-only helper for cortex_paseo_dev.py binding, not a plugin RPC. */
import { createHash } from "node:crypto";
import { once } from "node:events";
import { execFile, type ChildProcess } from "node:child_process";
import { realpathSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, normalize, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";
import { createPaseoApi } from "@getpaseo/client";
import { DaemonClient } from "@getpaseo/client/internal/daemon-client";
import { parsePaseoRuntimeSessionId } from "./worker-history.ts";

export type BindingInput = { agent: string; workspace: string; project: string };
type AgentAPI = { agents: { ref(id: string): { refresh(): Promise<unknown>; current(): unknown } } };
const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const digest = (value: unknown) => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const bindingDigest = (value: Record<string, unknown>) =>
  digest(Object.fromEntries(Object.keys(value).sort().map((key) => [key, value[key]])));
function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function observation(value: unknown, input: BindingInput) {
  if (!record(value) || Buffer.byteLength(JSON.stringify(value)) > 65536) throw new Error("binding_host_refused");
  const session = parsePaseoRuntimeSessionId(value.runtimeInfo);
  if (value.id !== input.agent || value.provider !== "cortex" || value.workspaceId !== input.workspace
      || value.cwd !== input.project || !record(value.runtimeInfo) || value.runtimeInfo.provider !== "cortex"
      || !session) throw new Error("binding_host_refused");
  return { agent_id: input.agent, workspace_id: input.workspace, project_root: input.project,
    session_id: session, wrapper_sha256: digest(value.runtimeInfo) };
}

/** Fetch only one exact agent; storage callback is the guarded immutable reader. */
export async function qualifyBinding(api: AgentAPI, input: BindingInput, read: (session: string) => Promise<unknown>) {
  if (!uuid.test(input.agent) || !/^[A-Za-z0-9_-]{1,256}$/.test(input.workspace)
      || !isAbsolute(input.project) || normalize(input.project) !== input.project || input.project.includes("\0")) throw new Error("binding_invalid_input");
  const handle = api.agents.ref(input.agent);
  await handle.refresh();
  const first = observation(handle.current(), input);
  const storage = await read(first.session_id);
  if (!record(storage) || Object.keys(storage).sort().join(",") !== "binding,binding_sha256,provenance"
      || storage.provenance !== "guarded-immutable-session-task-current-owner" || !record(storage.binding)
      || Object.keys(storage.binding).sort().join(",") !== "owner_epoch,project_root,session_id,task_id" || storage.binding.project_root !== input.project
      || storage.binding.session_id !== first.session_id || typeof storage.binding.task_id !== "string"
      || !/^t_[0-9a-f]{12}$/.test(storage.binding.task_id) || !Number.isSafeInteger(storage.binding.owner_epoch)
      || Number(storage.binding.owner_epoch) < 1 || typeof storage.binding_sha256 !== "string"
      || !/^[0-9a-f]{64}$/.test(storage.binding_sha256)
      || bindingDigest(storage.binding)
        !== storage.binding_sha256) throw new Error("binding_storage_refused");
  await handle.refresh();
  if (JSON.stringify(first) !== JSON.stringify(observation(handle.current(), input))) throw new Error("binding_host_drift");
  // Ownership can change while the host re-fetch is in flight. Revalidate it;
  // this still is an observation, not a lease or a resource-release receipt.
  if (JSON.stringify(storage) !== JSON.stringify(await read(first.session_id))) throw new Error("binding_storage_drift");
  return { available: true, host: first, binding: storage.binding, binding_sha256: storage.binding_sha256,
    provenance: "single-agent-api/refetch + guarded-immutable-session-task-current-owner" };
}

const inspectionReasons = new Set([
  "inspection_invalid_binding", "inspection_binding_mismatch", "unsafe_storage",
  "inspection_uncheckpointed_wal", "legacy_storage_migration_required", "unsupported_storage",
  "inspection_unsafe_parameter", "inspection_evidence_overflow", "inspection_snapshot_changed",
  "inspection_invalid_cursor", "inspection_busy", "inspection_refused",
]);
const bindingErrors = new Set([
  "binding_invalid_input", "binding_host_refused", "binding_host_drift",
  "binding_storage_refused", "binding_storage_drift", "binding_endpoint_refused",
  "binding_endpoint_drift", "binding_timeout", "binding_anonymous_connection_refused",
]);

export function bindingErrorCode(error: unknown): string {
  return error instanceof Error && bindingErrors.has(error.message)
    ? error.message : "binding_anonymous_host_or_storage_refused";
}

/** Only a complete exit-1 inspector refusal can supply a diagnostic, never a binding. */
export function inspectionRefusalReason(exitCode: unknown, stdout: string, terminated = false): string | null {
  if (exitCode !== 1 || terminated) return null;
  try {
    const value: unknown = JSON.parse(stdout);
    if (!record(value) || value.available !== false || typeof value.error !== "string"
        || !inspectionReasons.has(value.error)
        || stdout !== '{"available": false, "error": "' + value.error + '"}\n') return null;
    return value.error;
  } catch { return null; }
}

class ChildRefusal extends Error {
  diagnostic: { stage: "endpoint-provenance" | "task-discovery"; reason: string };
  constructor(stage: "endpoint-provenance" | "task-discovery", reason: string) {
    super("binding_storage_refused");
    this.diagnostic = { stage, reason };
  }
}

/** Classify process metadata, never its message, stderr or arbitrary output. */
export function childFailureReason(code: unknown, signal: unknown, killed: boolean): string {
  if (code === "ERR_CHILD_PROCESS_STDIO_MAXBUFFER") return "output_overflow";
  if (killed) return "timeout";
  if (signal) return "subprocess_terminated";
  if (code === "ENOENT" || code === "EACCES") return "spawn_failed";
  return "subprocess_failed";
}

const children = new Set<ChildProcess>();
function python(executable: string, argv: string[], timeout: number,
                stage: "endpoint-provenance" | "task-discovery"): Promise<unknown> {
  return new Promise((accept, reject) => {
    const child = execFile(executable, ["-B", ...argv], { timeout, maxBuffer: 8192, env: process.env }, (error, stdout) => {
      children.delete(child);
      if (error) {
        const reason = stage === "task-discovery"
          ? inspectionRefusalReason(error.code, stdout, Boolean(error.killed || error.signal)) : null;
        return reject(new ChildRefusal(stage, reason ?? childFailureReason(error.code, error.signal, Boolean(error.killed))));
      }
      try { accept(JSON.parse(stdout)); } catch { reject(new ChildRefusal(stage, "invalid_output")); }
    });
    children.add(child);
  });
}

async function main() {
  const [url, agent, workspace, project, seconds, executable, root] = process.argv.slice(2);
  const timeout = Number(seconds) * 1000;
  if (!Number.isInteger(timeout) || timeout < 1000 || timeout > 30000) throw new Error("binding_invalid_input");
  // Recheck the wrapper's no-override endpoint gate even on a direct helper invocation.
  const readEndpoint = () => python(executable, ["-c",
    "import json,sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]+'/scripts'); from cortex_paseo_dev import _binding_endpoint,_private_json; h=Path.home()/'.paseo-dev'; before=_binding_endpoint(h); sid=_private_json(h/'paseo.pid')['serverId']; assert before==_binding_endpoint(h); print(json.dumps({'url':before[0],'sha256':before[1],'serverId':sid}))", root], timeout, "endpoint-provenance");
  const endpoint = await readEndpoint();
  if (!record(endpoint) || endpoint.url !== url || !/^ws:\/\/127\.0\.0\.1:[0-9]{1,5}\/ws$/.test(url)) throw new Error("binding_endpoint_refused");
  if (realpathSync(project) !== project || realpathSync(root) !== root) throw new Error("binding_invalid_input");
  for (const path of [root, resolve(homedir(), ".paseo"), resolve(homedir(), ".paseo-dev")]) {
    if (project === path || project.startsWith(path + sep) || path.startsWith(project + sep)) throw new Error("binding_invalid_input");
  }
  // No credential files, automatic startup, registration or engine methods.
  // Auth-required hosts refuse; this helper never opens a login/account route.
  const client = new DaemonClient({ url, clientId: "cortex-qualification-binding", clientType: "cli",
    reconnect: { enabled: false }, connectTimeoutMs: timeout,
    logger: { debug() {}, info() {}, warn() {}, error() {} } });
  const api = createPaseoApi(client);
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    for (const child of children) child.kill("SIGKILL");
    void client.close();
  }, timeout);
  try {
    try { await client.connect(); } catch {
      throw new Error(timedOut ? "binding_timeout" : "binding_anonymous_connection_refused");
    }
    if (client.getLastServerInfoMessage()?.serverId !== endpoint.serverId) throw new Error("binding_endpoint_refused");
    const result = await qualifyBinding(api, { agent, workspace, project },
      (session) => python(executable, [resolve(root, "scripts/cortex_inspect_task.py"), "--discover",
        "--project", project, "--session", session], timeout, "task-discovery"));
    if (timedOut) throw new Error("binding_timeout");
    if (client.getLastServerInfoMessage()?.serverId !== endpoint.serverId
        || JSON.stringify(endpoint) !== JSON.stringify(await readEndpoint())) throw new Error("binding_endpoint_drift");
    if (timedOut) throw new Error("binding_timeout");
    process.stdout.write(JSON.stringify(result) + "\n");
  } finally {
    clearTimeout(timer);
    const closing = [...children].map((child) => {
      const closed = once(child, "close");
      child.kill("SIGKILL");
      return closed;
    });
    await Promise.all(closing);
    try { await api.dispose(); } finally { await client.close(); }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    const code = bindingErrorCode(error);
    process.stdout.write(JSON.stringify({ available: false, error: code,
      ...(error instanceof ChildRefusal ? { diagnostic: error.diagnostic } : {}) }) + "\n");
    process.exitCode = 1;
  });
}
