/**
 * Paseo 0.9.1 cannot reduce history until session/load returns. Stage privately,
 * then replay in acknowledged windows. The provider withholds session.ready
 * until its ordered notification lane consumes the checked final barrier.
 */
import { spawn } from "node:child_process";
import { randomUUID, createHash } from "node:crypto";
import { ReplaySpool, readFrames } from "./replay-spool.mjs";

const acknowledged = process.env.CORTEX_REPLAY_ACK === "1";
const target = JSON.parse(process.env.CORTEX_ACP_TARGET ?? "null");
if (!target || !Array.isArray(target.argv) || !target.argv.length ||
    target.argv.some(x => typeof x !== "string" || !x) ||
    (target.env !== undefined && (!target.env || typeof target.env !== "object" ||
      Array.isArray(target.env) || Object.entries(target.env).some(([key, value]) =>
        !/^(HOME|XDG_CONFIG_HOME|XDG_STATE_HOME)$/.test(key) ||
        typeof value !== "string" || !value.startsWith("/")))))
  throw new Error("CORTEX_ACP_TARGET is invalid");
const child = spawn(target.argv[0], target.argv.slice(1), {
  env: { ...process.env, ...(target.env ?? {}) }, stdio: ["pipe", "pipe", "inherit"],
});
let load, spool, postReplay, replayTask, waiter, failed = false, childExited = false;
const write = (stream, frame) => new Promise((resolve, reject) =>
  stream.write(JSON.stringify(frame) + "\n", error => error ? reject(error) : resolve()));
const emit = frame => write(process.stdout, frame);
function fail(message) {
  if (failed) return;
  failed = true;
  process.exitCode = 1;
  process.stderr.write("Cortex ACP Paseo shim: " + message + "\n");
  waiter?.reject(new Error(message));
  waiter = undefined;
  spool?.close();
  postReplay?.close();
  if (load && !load.responded) void emit({jsonrpc:"2.0", id:load.id,
    error:{code:-32603,message:"Incomplete history: replay failed before completion"}}).catch(() => {});
  child.kill("SIGTERM");
}
function waitForAck(count) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Incomplete history: replay acknowledgement timed out")), 30000);
    waiter = { count, resolve: () => { clearTimeout(timer); waiter = undefined; resolve(); },
      reject: error => { clearTimeout(timer); reject(error); } };
    if (childExited) waiter.reject(new Error("Incomplete history: agent exited during replay"));
  });
}
function adapt(frame, index) {
  const update = frame.params?.update;
  if (update?.sessionUpdate !== "user_message_chunk") return frame;
  const identity = update.messageId ?? `replay:${load.sessionId}:${index}`;
  return { ...frame, params: { ...frame.params, update: { ...update,
    sessionUpdate: "agent_message_chunk",
    messageId: "__cortex_history_user_v2__" + Buffer.from(identity).toString("base64url") } } };
}
async function replay(response) {
  const batch = spool;
  spool = undefined;
  const manifest = batch.manifest();
  load.responded = true;
  try {
    if (acknowledged) {
      const ready = waitForAck(0);
      await emit({ ...response, result: { ...response.result, _meta: {
        ...response.result?._meta, cortexReplay: { token: load.token, ...manifest } } } });
      await ready;
    } else {
      await emit(response);
      await new Promise(resolve => setImmediate(resolve));
    }
    let count = 0, bytes = 0, windowBytes = 0;
    const hash = createHash("sha256");
    const barrier = async (final) => {
      const consumed = waitForAck(count);
      await emit({jsonrpc:"2.0",method:"session/update",params:{sessionId:load.sessionId,
        update:{sessionUpdate:"agent_message_chunk",
          messageId:`__cortex_replay_barrier__${load.token}:${count}:${final ? "final" : "batch"}`,
          content:{type:"text",text:JSON.stringify({count,bytes,sha256:hash.copy().digest("hex"),final})}}}});
      await consumed;
    };
    for (const frame of batch.frames()) {
      if (failed || (acknowledged && childExited)) throw new Error("Incomplete history: agent exited during replay");
      const data = JSON.stringify(frame);
      count++; bytes += Buffer.byteLength(data); windowBytes += Buffer.byteLength(data); hash.update(data);
      await emit(frame);
      if (acknowledged && (count % 64 === 0 || windowBytes >= 1024 * 1024)) {
        await barrier(false); windowBytes = 0;
      }
    }
    if (acknowledged) await barrier(true);
  } finally { batch.close(); }
  load = undefined;
}
child.stdin.on("error", () => fail("Incomplete history: agent input closed"));
process.stdout.on("error", () => fail("Incomplete history: client output closed"));
child.on("error", () => fail("Incomplete history: agent process failed"));
child.on("exit", () => {
  childExited = true;
  // During a barrier wait there is no child stdout read to observe EOF.
  waiter?.reject(new Error("Incomplete history: agent exited during replay"));
});
void (async () => {
  try {
    for await (const frame of readFrames(process.stdin)) {
      if (frame.method === "_cortex/replayAck" && acknowledged) {
        if (!load || frame.params?.token !== load.token || frame.params?.count !== waiter?.count)
          throw new Error("Incomplete history: mismatched replay acknowledgement");
        waiter.resolve();
        continue;
      }
      if (frame.method === "session/load" && frame.id != null) {
        if (load) throw new Error("Overlapping session/load requests");
        load = {id:frame.id,sessionId:frame.params?.sessionId,token:randomUUID(),responded:false};
        spool = new ReplaySpool();
      }
      await write(child.stdin, frame);
    }
    child.stdin.end();
  } catch { fail("Incomplete history: client stream or replay staging failed"); }
})();
void (async () => {
  try {
    for await (const frame of readFrames(child.stdout)) {
      if (failed) break;
      if (load && frame.id === load.id && ("result" in frame || "error" in frame)) {
        if ("error" in frame) {
          spool.close(); spool = undefined; load = undefined;
          await emit(frame);
        } else {
          // Keep reading configuration responses while open() applies initial
          // settings before emitting session.opened. Notifications remain ordered.
          replayTask = replay(frame).then(async () => {
            while (postReplay) {
              const pending = postReplay; postReplay = undefined;
              try { for (const item of pending.frames()) await emit(item); }
              finally { pending.close(); }
            }
          }).catch(() => fail("Incomplete history: replay acknowledgement or drain failed"));
        }
      } else if (load?.responded && !("id" in frame)) {
        postReplay ??= new ReplaySpool();
        postReplay.append(frame);
      } else if (load && frame.method === "session/update" && frame.params?.sessionId === load.sessionId) {
        spool.append(adapt(frame, spool.count));
      } else { await emit(frame); }
    }
    await replayTask;
    if (load) fail("Incomplete history: agent exited before replay completion");
  } catch { fail("Incomplete history: agent stream or replay staging failed"); }
  finally {
    await replayTask;
    spool?.close(); postReplay?.close();
    // The client keeps stdin open after child failure. Do not leave a pending load hanging.
    process.stdin.destroy();
    child.kill("SIGTERM");
    process.stdout.write("", () => process.exit(process.exitCode ?? (child.exitCode ? 1 : 0)));
  }
})();
process.on("SIGTERM", () => { fail("Replay cancelled"); process.stdin.destroy(); });
process.on("SIGINT", () => { fail("Replay cancelled"); process.stdin.destroy(); });
