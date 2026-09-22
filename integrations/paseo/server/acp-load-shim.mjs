/**
 * ACP stdio compatibility shim for Paseo 0.8.
 *
 * Paseo's bundled ACP bridge filters session/update notifications against its
 * native session id before assigning that id after session/load returns. The
 * shim keeps the ACP agent wire protocol unchanged and delays only the
 * matching session/update notifications until the load response has crossed
 * the bridge. It is intentionally a dumb JSON-RPC relay: it never logs or
 * rewrites frame contents; only the client-facing order is changed for this
 * Paseo 0.8 compatibility path. During replay, Paseo 0.8 also discards
 * `user_message_chunk` updates. Those replay-only chunks are tagged and
 * carried as agent chunks so the provider wrapper can restore them as user
 * timeline items after the bridge has reduced them.
 */
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";

const MAX_ITEMS = 4096;
const MAX_BYTES = 16 * 1024 * 1024;
const REPLAY_USER_MESSAGE_PREFIX = `__cortex_history_user__${randomUUID()}:`;

function fail(message) {
  process.stderr.write(`Cortex ACP Paseo shim: ${message}\n`);
  process.exitCode = 1;
  try { child?.kill("SIGTERM"); } catch {}
}

let child;
try {
  const raw = process.env.CORTEX_ACP_TARGET;
  if (!raw) throw new Error("CORTEX_ACP_TARGET is missing");
  const target = JSON.parse(raw);
  if (!target || !Array.isArray(target.argv) || target.argv.length === 0 ||
      target.argv.some((value) => typeof value !== "string" || value.length === 0) ||
      (target.env !== undefined && (!target.env || typeof target.env !== "object" ||
        Array.isArray(target.env) || Object.entries(target.env).some(([key, value]) =>
          !/^(HOME|XDG_CONFIG_HOME|XDG_STATE_HOME)$/.test(key) ||
          typeof value !== "string" || !value.startsWith("/"))))) {
    throw new Error("CORTEX_ACP_TARGET is invalid");
  }
  child = spawn(target.argv[0], target.argv.slice(1), {
    env: { ...process.env, ...(target.env ?? {}) },
    stdio: ["pipe", "pipe", "inherit"],
  });
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
  throw error;
}

let pendingLoadId;
let pendingLoadSessionId;
let buffered = [];
let bufferedBytes = 0;
let closed = false;
let replayUserMessageIndex = 0;
let replayFlushPending = false;
let postReplayQueue = [];
let postReplayQueueBytes = 0;

function stop(code = 1) {
  if (closed) return;
  closed = true;
  if (!child.killed) child.kill("SIGTERM");
  process.exitCode = code;
}

function encode(frame) {
  return `${JSON.stringify(frame)}\n`;
}

function send(frame) {
  if (closed || !child.stdin.writable) return;
  child.stdin.write(encode(frame));
}

function emit(frame, callback) {
  if (closed) return;
  process.stdout.write(encode(frame), callback);
}

function flushReplayBatch(batch) {
  setImmediate(() => {
    try {
      if (!closed) {
        for (const frame of batch) emit(frame);
        const queued = postReplayQueue;
        postReplayQueue = [];
        postReplayQueueBytes = 0;
        for (const frame of queued) emit(frame);
      }
    } finally {
      replayFlushPending = false;
    }
  });
}

function adaptReplayUserMessage(frame) {
  const update = frame?.params?.update;
  if (!update || update.sessionUpdate !== "user_message_chunk") return frame;
  replayUserMessageIndex += 1;
  return {
    ...frame,
    params: {
      ...frame.params,
      update: {
        ...update,
        sessionUpdate: "agent_message_chunk",
        messageId: `${REPLAY_USER_MESSAGE_PREFIX}${replayUserMessageIndex}`,
      },
    },
  };
}

function parseStream(stream, onFrame, name) {
  let pending = "";
  stream.on("data", (chunk) => {
    pending += chunk.toString("utf8");
    let newline;
    while ((newline = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, newline).replace(/\r$/, "");
      pending = pending.slice(newline + 1);
      if (!line.trim()) continue;
      let frame;
      try { frame = JSON.parse(line); } catch {
        stop(1); fail(`${name} emitted malformed JSON`); return;
      }
      onFrame(frame);
      if (closed) return;
    }
    if (pending.length > MAX_BYTES) {
      stop(1); fail(`${name} emitted an oversized unterminated frame`);
    }
  });
  stream.on("end", () => {
    if (pending.trim()) {
      stop(1); fail(`${name} ended with a truncated JSON frame`);
      return;
    }
    if (name === "client" && !child.stdin.destroyed) child.stdin.end();
  });
  stream.on("error", (error) => {
    stop(1); fail(`${name} failed: ${error instanceof Error ? error.message : String(error)}`);
  });
}

parseStream(process.stdin, (frame) => {
  if (!frame || typeof frame !== "object" || Array.isArray(frame)) {
    stop(1); fail("client emitted a non-object ACP frame"); return;
  }
  if (frame.method === "session/load" && frame.id !== undefined && frame.id !== null) {
    if (pendingLoadId !== undefined || replayFlushPending) {
      stop(1); fail("overlapping session/load requests are not supported"); return;
    }
    pendingLoadId = frame.id;
    pendingLoadSessionId = frame.params?.sessionId;
    buffered = [];
    bufferedBytes = 0;
  }
  send(frame);
}, "client");

parseStream(child.stdout, (frame) => {
  if (!frame || typeof frame !== "object" || Array.isArray(frame)) {
    stop(1); fail("agent emitted a non-object ACP frame"); return;
  }
  const isResponse = frame.id !== undefined && frame.id !== null &&
    (Object.prototype.hasOwnProperty.call(frame, "result") ||
      Object.prototype.hasOwnProperty.call(frame, "error"));
  if (pendingLoadId !== undefined && isResponse && frame.id === pendingLoadId) {
    // Deliver the response before replay notifications. Paseo assigns its
    // native session id in the response continuation; setImmediate prevents
    // the next notification from being consumed in the same parser turn.
    if (Object.prototype.hasOwnProperty.call(frame, "error")) {
      buffered = [];
      bufferedBytes = 0;
      pendingLoadId = undefined;
      pendingLoadSessionId = undefined;
      emit(frame);
      return;
    }
    // Close the capture window before writing the response. Notifications
    // arriving after this point are live traffic; they are queued only for
    // this one event-loop turn so replay order remains deterministic.
    const replayBatch = buffered;
    buffered = [];
    bufferedBytes = 0;
    pendingLoadId = undefined;
    pendingLoadSessionId = undefined;
    replayFlushPending = true;
    emit(frame, () => flushReplayBatch(replayBatch));
    return;
  }
  if (pendingLoadId !== undefined && frame.method === "session/update" &&
      frame.params?.sessionId === pendingLoadSessionId) {
    const size = Buffer.byteLength(JSON.stringify(frame), "utf8");
    if (buffered.length >= MAX_ITEMS || bufferedBytes + size > MAX_BYTES) {
      stop(1); fail("session/load history exceeded shim limits"); return;
    }
    buffered.push(adaptReplayUserMessage(frame));
    bufferedBytes += size;
    return;
  }
  if (replayFlushPending) {
    const size = Buffer.byteLength(JSON.stringify(frame), "utf8");
    if (postReplayQueue.length >= MAX_ITEMS || postReplayQueueBytes + size > MAX_BYTES) {
      stop(1); fail("post-load notification queue exceeded limits"); return;
    }
    postReplayQueue.push(frame);
    postReplayQueueBytes += size;
  } else {
    emit(frame);
  }
}, "agent");

child.on("error", (error) => { stop(1); fail(`agent process failed: ${error.message}`); });
child.on("close", (code) => {
  if (!closed) stop(code === 0 ? 0 : 1);
  // Paseo still holds our stdin open, so without an explicit exit a dead
  // agent left every pending request (a session/load above all) unanswered
  // forever instead of failing as a closed provider.
  process.stdout.write("", () => process.exit());
});
process.on("SIGTERM", () => { if (!child.killed) child.kill("SIGTERM"); });
process.on("SIGINT", () => { if (!child.killed) child.kill("SIGINT"); });
