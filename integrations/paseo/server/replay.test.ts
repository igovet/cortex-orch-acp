import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
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
def send(frame): print(json.dumps(frame, ensure_ascii=False), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    if req.get("method") == "initialize":
        send({"jsonrpc":"2.0","id":req["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":True}}})
    elif req.get("method") == "session/set_config_option":
        send({"jsonrpc":"2.0","id":req["id"],"result":{"configOptions":[]}})
    elif req.get("method") == "session/load":
        sid = req["params"]["sessionId"]
        if sid == "crash-before": os._exit(7)
        for i in range(4097):
            update = ({"sessionUpdate":"tool_call","toolCallId":f"card:{i}","title":f"Card {i}","status":"completed","kind":"other"} if i % 3 == 0 else
                      {"sessionUpdate":"user_message_chunk" if i % 3 == 1 else "agent_message_chunk",
                       "messageId":f"message:{i}","content":{"type":"text","text":f"🙂я {i}"}})
            if sid == "invalid-card" and i == 99: update["status"] = "impossible"
            send({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})
            if sid == "crash-mid" and i == 100: os._exit(8)
        send({"jsonrpc":"2.0","id":req["id"],"result":{}})
        if sid == "crash-after": os._exit(9)
`);
async function load(nativeId: string) {
  const command = ["env", "CORTEX_ACP_TARGET=" + JSON.stringify({ argv: ["python3", "-B", fixture] }),
    process.execPath, resolve("server/acp-load-shim.mjs")] as [string, ...string[]];
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
      history:"replay",persistence:{version:1,data:{sessionId:nativeId}},
      config:{cwd:dir,env:{},mcpServers:{},settings:{language:"english"},persist:false}});
    let timer: ReturnType<typeof setTimeout>;
    await Promise.race([done, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("test load timed out")), 15000);
    })]).finally(() => clearTimeout(timer!));
    return events;
  } finally { await connection.close(); }
}
try {
  const first = await load("history");
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
  for (const id of ["crash-before", "crash-mid", "crash-after", "invalid-card"]) {
    const events = await load(id);
    assert.ok(events.some(e => e.type === "request.failed"), id);
    assert.ok(!events.some(e => e.type === "session.ready"), id);
  }
} finally { rmSync(dir, { recursive: true, force: true }); }
console.log("replay: spool boundaries, UTF-8, 4097-item bridge, stable reload and crash checks passed");
