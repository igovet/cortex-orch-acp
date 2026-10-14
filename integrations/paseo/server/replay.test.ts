import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync, existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { runAcpProvider } from "@getpaseo/plugin/server/acp";
import { ReplayController } from "./replay-connector.ts";
import { ReplaySpool, readFrames } from "./replay-spool.mjs";
import { wrapCortexConnection } from "./provider.ts";

async function collect<T>(values: AsyncIterable<T>): Promise<T[]> {
  const result: T[] = []; for await (const value of values) result.push(value); return result;
}

// True UTF-8 byte and count boundaries, including the old 4096-item cliff.
for (const count of [4095, 4096, 4097]) {
  const spool = new ReplaySpool();
  try {
    for (let i = 0; i < count; i++) spool.append({ id: i, text: "🙂я" });
    assert.deepEqual([...spool.frames()], Array.from({ length: count }, (_, id) => ({ id, text: "🙂я" })));
  } finally { spool.close(); }
}
const frame = { text: "🙂я" };
const size = Buffer.byteLength(JSON.stringify(frame));
for (const offset of [-1, 0, 1]) {
  const spool = new ReplaySpool({ items: 2, bytes: size + offset, frameBytes: size + offset });
  try {
    if (offset < 0) assert.throws(() => spool.append(frame), /limit/);
    else { spool.append(frame); assert.deepEqual([...spool.frames()], [frame]); }
  } finally { spool.close(); }
}
for (const count of [1, 2, 3]) {
  const spool = new ReplaySpool({ items: 2, bytes: 1000, frameBytes: 1000 });
  try {
    for (let i = 0; i < Math.min(count, 2); i++) spool.append(frame);
    if (count === 3) assert.throws(() => spool.append(frame), /limit/);
    else assert.equal([...spool.frames()].length, count);
  } finally { spool.close(); }
}
const bytes = Buffer.from(JSON.stringify(frame) + "\n");
assert.deepEqual(await collect(readFrames(Readable.from([...bytes].map(byte => Buffer.from([byte]))))), [frame]);
for (const newline of ["", "\n"]) {
  await assert.rejects(async () => collect(readFrames(
    Readable.from([Buffer.from(JSON.stringify(frame) + newline)]), size - 1)), /oversized/);
}
await assert.rejects(async () => collect(readFrames(Readable.from([Buffer.from('{"text":')]))), /truncated/);

const dir = mkdtempSync(join(tmpdir(), "cortex-replay-test-"));
const fixture = join(dir, "agent.py");
writeFileSync(fixture, `
import json, sys, os
config_calls = os.path.join(os.path.dirname(__file__), "config-calls.jsonl")
methods = os.path.join(os.path.dirname(__file__), "methods.log")
def send(frame): print(json.dumps(frame, ensure_ascii=False), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    with open(methods, "a") as log: log.write(str(req.get("method")) + "\\n")
    if req.get("method") == "initialize":
        send({"jsonrpc":"2.0","id":req["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":True}}})
    elif req.get("method") in ("session/set_config_option", "session/set_mode"):
        with open(config_calls, "a") as log: log.write(json.dumps(req) + "\\n")
        send({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32602,
              "message":"'gpt-6-sol' is not a valid value for configId 'model'."}})
    elif req.get("method") == "session/new":
        send({"jsonrpc":"2.0","id":req["id"],"result":{"sessionId":"new-session","configOptions":[
            {"id":"model","name":"Model","category":"model","type":"select","currentValue":"available-model",
             "options":[{"value":"available-model","name":"Available"}]}]}})
    elif req.get("method") == "session/load":
        sid = req["params"]["sessionId"]
        if sid == "crash-before": os._exit(7)

        if sid == "code-mode":
            for cid, tool, args, inner_status, result in [
                ("raw-read", "cortex.read_report", {"report_id": "r_abc"}, "error", None),
                ("raw-shell", "cortex.command_run", {"argv": ["pwd"]}, "completed", None),
                ("raw-result", "cortex.command_run", {"argv": ["pwd"]}, "completed", "/project"),
            ]:
                updates = [
                    {"sessionUpdate":"tool_call","toolCallId":cid,"title":"execute","kind":"other","status":"pending","rawInput":{}},
                    {"sessionUpdate":"tool_call_update","toolCallId":cid,"status":"in_progress",
                     "rawInput":{"code":"return await tools.cortex.command_run({})"}},
                    {"sessionUpdate":"tool_call_update","toolCallId":cid,"status":"completed",
                     "rawOutput":{"metadata":{"toolCalls":[{"tool":tool,"input":args,"status":inner_status}]}},
                     "content":[{"type":"content","content":{"type":"text","text":"content discarded by installed ACP bridge"}}]},
                ]
                if result is not None:
                    updates[-1]["rawOutput"]["result"] = {"content":[{"type":"text","text":result}]}
                for update in updates:
                    send({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})
            send({"jsonrpc":"2.0","id":req["id"],"result":{}})
            continue
        for i in range(4097):
            update = ({"sessionUpdate":"tool_call","toolCallId":f"card:{i}","title":f"Card {i}","status":"completed","kind":"other"} if i % 3 == 0 else
                      {"sessionUpdate":"user_message_chunk" if i % 3 == 1 else "agent_message_chunk",
                       "messageId":f"message:{i}","content":{"type":"text","text":f"🙂я {i}"}})
            if sid == "invalid-card" and i == 99: update["status"] = "impossible"
            send({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})
            if sid == "crash-mid" and i == 100: os._exit(8)
        send({"jsonrpc":"2.0","id":req["id"],"result":{"configOptions":[
            {"id":"model","name":"Model","category":"model","type":"select","currentValue":"available-model",
             "options":[{"value":"available-model","name":"Available"}]}]}})
        if sid == "crash-after": os._exit(9)
`);
async function load(nativeId: string | null, staleConfig = false, configureAfter = false, closeAfter = false) {
  const command = ["env", "CORTEX_ACP_TARGET=" + JSON.stringify({ argv: ["python3", "-B", fixture] }),
    process.execPath, fileURLToPath(new URL("./acp-load-shim.mjs", import.meta.url))] as [string, ...string[]];
  const replay = new ReplayController(command);
  const base = runAcpProvider({ id: "cortex", label: "Cortex", connector: replay.connect });
  const connection = wrapCortexConnection(replay.wrap(await base.connect({
    versions: [1], capabilities: ["session.persistence", "prompt.message", "session.configure"],
  })), []);
  const events: any[] = [];
  let settle!: () => void;
  const done = new Promise<void>(resolve => { settle = resolve; });
  connection.onEvent(event => {
    events.push(event);
    if (event.type === "session.ready" || event.type === "request.failed") settle();
  });
  try {
    await connection.send({type:"session.open", requestId:"open",sessionId:"boundary",
      history:"replay",...(nativeId ? {persistence:{version:1,data:{sessionId:nativeId}}} : {}),
      config:{cwd:dir,env:{},mcpServers:{},settings:staleConfig ? {retired_setting:"old"} : {},persist:false,
        ...(staleConfig ? {model:"gpt-6-sol",mode:"retired-mode",thinkingOption:"retired-effort"} : {})}});
    let timer: ReturnType<typeof setTimeout>;
    await Promise.race([done, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("test load timed out")), 15000);
    })]).finally(() => clearTimeout(timer!));
    if (configureAfter) {
      const configured = new Promise<void>(resolve => connection.onEvent(event => {
        if ((event.type === "request.failed" || event.type === "request.completed") && event.requestId === "configure") resolve();
      }));
      await connection.send({type:"session.configure",requestId:"configure",sessionId:"boundary",
        changes:{model:"gpt-6-sol"}});
      let configTimer: ReturnType<typeof setTimeout>;
      await Promise.race([configured, new Promise((_, reject) => {
        configTimer = setTimeout(() => reject(new Error("test configure timed out")), 5000);
      })]).finally(() => clearTimeout(configTimer!));
    }
    if (closeAfter) {
      const closed = new Promise<void>(resolve => connection.onEvent(event => {
        if ((event.type === "request.failed" || event.type === "request.completed") && event.requestId === "close") resolve();
      }));
      await connection.send({type:"session.close",requestId:"close",sessionId:"boundary"});
      let closeTimer: ReturnType<typeof setTimeout>;
      await Promise.race([closed, new Promise((_, reject) => {
        closeTimer = setTimeout(() => reject(new Error("test close timed out")), 5000);
      })]).finally(() => clearTimeout(closeTimer!));
    }
    return events;
  } finally { await connection.close(); }
}
try {

  const codeMode = await load("code-mode");
  assert.equal(codeMode.filter(e => e.type === "request.failed").length, 0);
  assert.equal(codeMode.at(-1).type, "session.ready");
  const cards = codeMode.filter(e => e.type === "timeline.item" && e.item.type === "tool_call");
  const final = (id: string) => cards.filter(e => e.item.callId === id).at(-1).item;
  assert.equal(cards[0].item.name, "Shell", "title-only pending snapshot is ambiguous after the bridge");
  assert.equal(final("raw-read").name, "Read report: r_abc");
  assert.equal(final("raw-read").status, "failed");
  assert.equal(final("raw-read").detail.output, null, "ACP content is not available after the installed bridge");
  assert.deepEqual({ ...final("raw-shell").detail }, { type: "shell", command: "pwd" });
  assert.deepEqual({ ...final("raw-result").detail }, { type: "shell", command: "pwd", output: "/project" });
  const first = await load("history");
  assert.equal(first.filter(e => e.type === "request.failed").length, 0);
  assert.equal(first.at(-1).type, "session.ready");
  const restored = await load("history", true);
  assert.equal(restored.filter(e => e.type === "request.failed").length, 0, JSON.stringify(restored.slice(-2)));
  assert.equal(restored.at(-1).type, "session.ready");
  assert.equal(restored.filter(e => e.type === "timeline.item").length, 4097);
  assert.equal(existsSync(join(dir, "config-calls.jsonl")), false, "restoring history must not configure the task");
  const second = await load("history");
  for (const events of [first, second]) {
    assert.equal(events.filter(e => e.type === "request.failed").length, 0, JSON.stringify(events.slice(-2)));
    assert.equal(events.at(-1).type, "session.ready");
    const items = events.filter(e => e.type === "timeline.item");
    assert.equal(items.length, 4097);
    assert.ok(events.findIndex(e => e.type === "session.opened") < events.findIndex(e => e.type === "timeline.item"));
    assert.equal(items[1].item.id, "message:1");
    assert.equal(items[1].item.type, "user_message");
    assert.equal(items[1].item.text, "🙂я 1");
  }
  assert.deepEqual(first.filter(e => e.type === "timeline.item"), second.filter(e => e.type === "timeline.item"));
  const fresh = await load(null, true);
  assert.ok(fresh.some(e => e.type === "request.failed" && e.error.message.includes("not a valid value")));
  assert.ok(!fresh.some(e => e.type === "session.ready"));
  let calls = readFileSync(join(dir, "config-calls.jsonl"), "utf8").trim().split("\n").map(line => JSON.parse(line));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].params.value, "gpt-6-sol");
  rmSync(join(dir, "config-calls.jsonl"));
  // The bridge's session/close on teardown is answered by the connector:
  // Cortex does not implement it, and closing the connector ends the process.
  rmSync(join(dir, "methods.log"), { force: true });
  const plain = await load(null, false, false, true);
  assert.ok(plain.some(e => e.type === "session.ready"), JSON.stringify(plain.slice(-2)));
  assert.ok(plain.some(e => e.type === "session.closed"), JSON.stringify(plain.slice(-2)));
  assert.ok(!plain.some(e => e.type === "request.failed"), JSON.stringify(plain.slice(-2)));
  const seen = readFileSync(join(dir, "methods.log"), "utf8").trim().split("\n");
  assert.ok(seen.includes("session/new"), seen.join(","));
  assert.ok(!seen.includes("session/close"), "session/close must not reach Cortex: " + seen.join(","));
  const configured = await load("history", false, true);
  assert.ok(configured.some(e => e.type === "session.ready"));
  assert.ok(configured.some(e => e.type === "request.failed" && e.requestId === "configure"
    && e.error.message.includes("not a valid value")));
  calls = readFileSync(join(dir, "config-calls.jsonl"), "utf8").trim().split("\n").map(line => JSON.parse(line));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].params.value, "gpt-6-sol");
  for (const id of ["crash-before", "crash-mid", "crash-after", "invalid-card"]) {
    const events = await load(id);
    assert.ok(events.some(e => e.type === "request.failed"), id);
    assert.ok(!events.some(e => e.type === "session.ready"), id);
  }
} finally { rmSync(dir, { recursive: true, force: true }); }
console.log("replay: raw Code Mode cards, spool boundaries, UTF-8, 4097-item bridge, stable reload and crash checks passed");
