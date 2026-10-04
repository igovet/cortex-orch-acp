import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { qualifyBinding, inspectionRefusalReason, childFailureReason, bindingErrorCode } from "./qualification-binding.ts";
const agent = "11111111-1111-1111-1111-111111111111";
const session = "acps_aaaaaaaaaaaa";
const input = { agent, workspace: "owned", project: "/tmp/owned" };
const host = () => ({ id: agent, workspaceId: input.workspace, cwd: input.project, provider: "cortex",
  runtimeInfo: { provider: "cortex", sessionId: "plugin:" + JSON.stringify({ version: 1, data: { sessionId: session } }) } });
const storage = () => {
  const binding = { owner_epoch: 1, project_root: input.project, session_id: session, task_id: "t_aaaaaaaaaaaa" };
  return { binding, binding_sha256: createHash("sha256").update(JSON.stringify(binding)).digest("hex"),
    provenance: "guarded-immutable-session-task-current-owner" };
};
let cases = 0;
async function run(change: (h: any, s: any) => void, error?: string, second = false) {
  const h = host(), s = storage();
  if (!second) change(h, s);
  let fetches = 0, reads = 0, refs = 0;
  const api = { agents: { ref(id: string) {
    refs++; assert.equal(id, agent);
    return { async refresh() { fetches++; if (second && fetches === 2) change(h, s); }, current() { return h; } };
  } } };
  // No list, workspace, engine, send, startup or mutation methods exist.
  const action = () => qualifyBinding(api, input, async (sid) => {
    assert.equal(sid, session); reads++; return structuredClone(s);
  });
  if (error) await assert.rejects(action, new RegExp(error)); else {
    const result = await action();
    assert.deepEqual(result.binding, storage().binding);
    assert.equal(fetches, 2); assert.equal(reads, 2);
    assert.equal(result.available, true);
  }
  assert.equal(refs, 1);
  cases++;
}
await run(() => {});
for (const key of ["id", "provider", "workspaceId", "cwd"]) {
  await run((h) => { h[key] = "wrong"; }, "binding_host_refused");
  await run((h) => { h[key] = "wrong"; }, "binding_host_refused", true);
}
for (const wrapped of [null, "acps_aaaaaaaaaaaa", "plugin:invalid",
  'plugin:{"version":2,"data":{"sessionId":"acps_aaaaaaaaaaaa"}}',
  'plugin:{"version":1,"version":1,"data":{"sessionId":"acps_aaaaaaaaaaaa"}}',
  'plugin:{"version":1,"data":{"sessionId":"acps_aaaaaaaaaaaa","sessionId":"acps_bbbbbbbbbbbb"}}',
  'plugin:{"version":1,"data":{"sessionId":"acps_aaaaaaaaaaaa"},"extra":1}',
  "plugin:" + JSON.stringify({version:1,data:{sessionId:"acps_prefix"}}),
  'plugin:{"version":1,"data":{"\\u0073essionId":"acps_aaaaaaaaaaaa"}}']) {
  await run((h) => { h.runtimeInfo.sessionId = wrapped; }, "binding_host_refused");
}
await run((h) => { h.runtimeInfo.provider = "codex"; }, "binding_host_refused");
await run((h) => { h.runtimeInfo = null; }, "binding_host_refused");
await run((h) => { h.title = "x".repeat(65536); }, "binding_host_refused");
for (const key of ["project_root", "session_id", "task_id", "owner_epoch"]) {
  await run((_h, s) => { delete s.binding[key]; }, "binding_storage_refused");
}
for (const epoch of [0, -1, true, "1", 1.2, Number.MAX_SAFE_INTEGER + 1]) {
  await run((_h, s) => { s.binding.owner_epoch = epoch; }, "binding_storage_refused");
}
await run((_h, s) => { s.binding_sha256 = "bad"; }, "binding_storage_refused");
await run((_h, s) => { s.binding_sha256 = "a".repeat(64); }, "binding_storage_refused");
await run((_h, s) => { s.private = "sentinel"; }, "binding_storage_refused");
await run((_h, s) => { s.binding.private = "sentinel"; }, "binding_storage_refused");
await run((_h, s) => { s.provenance = "invented"; }, "binding_storage_refused");
await run((_h, s) => { s.binding.owner_epoch++; }, "binding_storage_drift", true);
for (const bad of [{...input, agent: "1111"}, {...input, workspace: ""},
  {...input, project: "relative"}, {...input, project: "/tmp/../owned"}]) {
  await assert.rejects(() => qualifyBinding({agents:{ref() { throw Error("unexpected ref"); }}}, bad, async () => {
    throw Error("unexpected read");
  }), /binding_invalid_input/); cases++;
}
console.log(`qualification-binding: ${cases} cases passed; zero engines/enumeration`);

for (const reason of ["inspection_busy", "inspection_uncheckpointed_wal", "unsafe_storage",
  "inspection_binding_mismatch", "legacy_storage_migration_required", "unsupported_storage",
  "inspection_invalid_binding", "inspection_unsafe_parameter", "inspection_evidence_overflow",
  "inspection_snapshot_changed", "inspection_invalid_cursor", "inspection_refused"]) {
  const stdout = '{"available": false, "error": "' + reason + '"}\n';
  assert.equal(inspectionRefusalReason(1, stdout), reason);
  for (const code of [0, 2, null, "1", "ETIMEDOUT", "ERR_CHILD_PROCESS_STDIO_MAXBUFFER"]) {
    assert.equal(inspectionRefusalReason(code, stdout), null);
  }
  assert.equal(inspectionRefusalReason(1, stdout, true), null);
}
for (const stdout of [
  "", "not JSON", '{"available": false, "error": "private sentinel"}\n',
  '{"available": true, "error": "inspection_busy"}\n',
  '{"available": false, "error": "inspection_busy", "private": "sentinel"}\n',
  '{"available": false, "error": "inspection_busy", "error": "inspection_busy"}\n',
  '{"available": false, "\\u0065rror": "inspection_busy"}\n',
  '{"available": false, "error": "inspection_busy"}',
  '{"available": false, "error": "inspection_busy"}\nextra',
  '{"available": false, "error": "inspection_busy"}\n{"available": false}',
  '{"available": false, "error": ',
]) assert.equal(inspectionRefusalReason(1, stdout), null);
console.log("inspection refusal diagnostics: complete allowlisted exit-1 only; malformed/ambiguous/terminated output withheld");
for (const [code, signal, killed, reason] of [
  ["ENOENT", null, false, "spawn_failed"], ["EACCES", null, false, "spawn_failed"],
  [7, null, false, "subprocess_failed"], ["private secret sentinel", null, false, "subprocess_failed"],
  [null, "SIGTERM", false, "subprocess_terminated"], [null, "SIGTERM", true, "timeout"],
  ["ERR_CHILD_PROCESS_STDIO_MAXBUFFER", "SIGTERM", true, "output_overflow"],
] as const) assert.equal(childFailureReason(code, signal, killed), reason);
console.log("child diagnostics: bounded process-metadata classification only");
assert.equal(bindingErrorCode(new Error("binding_endpoint_refused")), "binding_endpoint_refused");
for (const value of [new Error("binding_private_secret_sentinel"), new Error("private secret sentinel"),
  { message: "binding_endpoint_refused" }, null]) {
  assert.equal(bindingErrorCode(value), "binding_anonymous_host_or_storage_refused");
}
