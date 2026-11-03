import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync, existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { runAcpProvider } from "@getpaseo/plugin/server/acp";
import { ReplayController } from "./replay-connector.ts";
import { cortexTransformer, wrapCortexConnection } from "./provider.ts";
import { WorkerCardFence } from "./worker-card-fence.ts";

// A Paseo composer keeps its last thinking choice across model switches. A
// model without efforts (Haiku) has no thought_level option, and Paseo's
// bridge then refused to create the agent: "ACP session does not expose
// thinking configuration". A stale effort the model does not offer is
// dropped, so Cortex keeps the model's own effort; supported ones still apply.
const dir = mkdtempSync(join(tmpdir(), "cortex-thinking-test-"));
const fixture = join(dir, "agent.py");
const calls = join(dir, "config-calls.jsonl");
writeFileSync(fixture, `
import json, os, sys
EFFORTS = {"opus": ["low", "medium", "high"], "haiku": []}
if os.environ.get("CORTEX_MODEL_EFFORTS") == "1":
    print(json.dumps(EFFORTS)); sys.exit(0)
state = {"model": "opus", "thought_level": "medium"}
def options():
    result = [{"id": "model", "name": "Model", "category": "model", "type": "select",
               "currentValue": state["model"],
               "options": [{"value": model, "name": model} for model in EFFORTS]}]
    if EFFORTS[state["model"]]:
        result.append({"id": "thought_level", "name": "Thought level", "category": "thought_level",
                       "type": "select", "currentValue": state["thought_level"],
                       "options": [{"value": value, "name": value} for value in EFFORTS[state["model"]]]})
    return result
def send(frame): print(json.dumps(frame), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    method = req.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"sessionId": "native", "configOptions": options()}})
    elif method == "session/set_config_option":
        with open(${JSON.stringify(calls)}, "a") as log: log.write(json.dumps(req["params"]) + "\\n")
        config_id, value = req["params"]["configId"], req["params"]["value"]
        allowed = {item["value"] for option in options() if option["id"] == config_id for item in option["options"]}
        if value not in allowed:
            send({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32602,
                  "message": f"{value!r} is not a valid value for configId {config_id!r}."}})
            continue
        state[config_id] = value
        if config_id == "model":
            state["thought_level"] = (EFFORTS[value] or [None])[0]
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"configOptions": options()}})
    elif "id" in req:
        send({"jsonrpc": "2.0", "id": req["id"], "result": {}})
`);

function configCalls(): Array<{ configId: string; value: string }> {
  if (!existsSync(calls)) return [];
  const result = readFileSync(calls, "utf8").trim().split("\n").map((line) => JSON.parse(line));
  rmSync(calls);
  return result.map(({ configId, value }) => ({ configId, value }));
}

async function until(connection: any, matches: (event: any) => boolean, label: string): Promise<any> {
  let timer: ReturnType<typeof setTimeout>;
  let off = () => {};
  return Promise.race([
    new Promise((resolve) => { off = connection.onEvent((event: any) => { if (matches(event)) resolve(event); }); }),
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} timed out`)), 15000); }),
  ]).finally(() => { clearTimeout(timer!); off(); });
}

async function open(config: { model?: string; thinkingOption?: string }, configure?: Record<string, string>) {
  const command = ["env", "CORTEX_ACP_TARGET=" + JSON.stringify({ argv: ["python3", "-B", fixture] }),
    process.execPath, fileURLToPath(new URL("./acp-load-shim.mjs", import.meta.url))] as [string, ...string[]];
  const replay = new ReplayController(command);
  const fence = new WorkerCardFence();
  const base = runAcpProvider({
    id: "cortex", label: "Cortex", connector: replay.connect,
    transformers: [cortexTransformer(command, fence)],
  });
  const connection = wrapCortexConnection(replay.wrap(await base.connect({
    versions: [1], capabilities: ["prompt.message", "session.configure"],
  })), [], fence);
  try {
    const opened = until(connection, (event) => event.type === "session.ready" || event.type === "request.failed", "open");
    await connection.send({ type: "session.open", requestId: "open", sessionId: "boundary", history: "replay",
      config: { cwd: dir, env: {}, mcpServers: {}, settings: {}, persist: false, ...config } } as any);
    const ready = await opened;
    let configured: any = null;
    if (configure && ready.type === "session.ready") {
      const done = until(connection, (event) => event.requestId === "configure" &&
        (event.type === "request.completed" || event.type === "request.failed"), "configure");
      await connection.send({ type: "session.configure", requestId: "configure", sessionId: "boundary",
        changes: configure } as any);
      configured = await done;
    }
    return { ready, configured, calls: configCalls() };
  } finally { await connection.close(); }
}

try {
  // The live defect: Haiku chosen with a High thinking preference left over.
  const haiku = await open({ model: "haiku", thinkingOption: "high" });
  assert.equal(haiku.ready.type, "session.ready", JSON.stringify(haiku.ready));
  assert.deepEqual(haiku.calls, [{ configId: "model", value: "haiku" }]);

  // A supported effort is still applied.
  const opus = await open({ model: "opus", thinkingOption: "high" });
  assert.equal(opus.ready.type, "session.ready", JSON.stringify(opus.ready));
  assert.deepEqual(opus.calls, [{ configId: "model", value: "opus" }, { configId: "thought_level", value: "high" }]);

  // An effort the model does not offer leaves Cortex's own choice in place.
  const stale = await open({ model: "opus", thinkingOption: "xhigh" });
  assert.equal(stale.ready.type, "session.ready", JSON.stringify(stale.ready));
  assert.deepEqual(stale.calls, [{ configId: "model", value: "opus" }]);

  // The same holds when the composer switches an open chat to Haiku.
  const switched = await open({}, { model: "haiku", thinkingOption: "high" });
  assert.equal(switched.ready.type, "session.ready", JSON.stringify(switched.ready));
  assert.equal(switched.configured?.type, "request.completed", JSON.stringify(switched.configured));
  assert.deepEqual(switched.calls, [{ configId: "model", value: "haiku" }]);
} finally { rmSync(dir, { recursive: true, force: true }); }
console.log("thinking: unsupported thinking preference dropped, supported effort applied");
