import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import type { AcpStream, AcpStreamMessage } from "@getpaseo/plugin/server/acp";
import type { ProviderEvent, ProviderRegistration } from "@getpaseo/plugin/server/provider";
import { readFrames } from "./replay-spool.mjs";

type Connection = Awaited<ReturnType<ProviderRegistration["connect"]>>;
type Manifest = { count: number; bytes: number; sha256: string };
type Load = {
  sessionId: string; requestId: string; nativeId: string; timer: ReturnType<typeof setTimeout>;
  token?: string; expected?: Manifest; count: number; bytes: number;
  hash: ReturnType<typeof createHash>; ready?: ProviderEvent; complete: boolean;
  items: Array<{type: string; id?: string; text?: string}>;
  barrier?: Manifest & { final: boolean }; ack?: (count: number) => void; stop?: () => void;
  opened?: boolean;
};
const BARRIER = "__cortex_replay_barrier__";

/** One controller per provider connection, using the installed SDK connector API. */
export class ReplayController {
  private loads = new Map<string, Load>();
  private children = new Set<ReturnType<typeof spawn>>();
  private emit: (event: ProviderEvent) => void = () => {};
  private command: readonly [string, ...string[]];
  constructor(command: readonly [string, ...string[]]) { this.command = command; }

  private fail(load: Load, message = "Incomplete history: replay did not complete") {
    if (!this.loads.delete(load.sessionId)) return;
    clearTimeout(load.timer);
    load.stop?.();
    this.emit({ type: "request.failed", requestId: load.requestId, error: { message } });
  }

  connect = (): AcpStream => {
    const child = spawn(this.command[0], this.command.slice(1), {
      env: { ...process.env, CORTEX_REPLAY_ACK: "1" }, stdio: ["pipe", "pipe", "pipe"],
    });
    this.children.add(child);
    // Never expose host stderr/transcript data. Named failures are reported by the gate.
    child.stderr!.resume();
    child.stdin!.on("error", () => { if (load && !load.complete) this.fail(load); });
    child.on("error", () => { if (load) this.fail(load, "Incomplete history: replay process failed"); });
    child.on("close", () => {
      this.children.delete(child);
      if (load && !load.complete) this.fail(load, "Incomplete history: replay process closed");
    });
    const write = (frame: unknown) => new Promise<void>((resolve, reject) =>
      child.stdin!.write(JSON.stringify(frame) + "\n", error => error ? reject(error) : resolve()));
    const stop = () => { child.kill("SIGTERM"); };
    const frames = readFrames(child.stdout!);
    let load: Load | undefined;
    let loadId: string | number | null | undefined;
    let replaying = false;
    return {
      writable: new WritableStream<AcpStreamMessage>({
        write: async frame => {
          if ("method" in frame && frame.method === "session/load" && "id" in frame) {
            const nativeId = (frame.params as { sessionId: string }).sessionId;
            load = [...this.loads.values()].find(item => item.nativeId === nativeId);
            if (!load) throw new Error("Incomplete history: no matching replay request");
            loadId = frame.id;
            load.stop = stop;
            load.ack = count => { void write({jsonrpc:"2.0",method:"_cortex/replayAck",
              params:{token:load!.token,count}}).catch(() => this.fail(load!)); };
          }
          await write(frame);
        },
        close: stop, abort: stop,
      }),
      readable: new ReadableStream<AcpStreamMessage>({
        pull: async controller => {
          try {
            const next = await frames.next();
            if (next.done) {
              if (load && !load.complete) this.fail(load);
              controller.close(); return;
            }
            const frame = next.value;
            if (load && frame.id === loadId && "result" in frame) {
              const manifest = frame.result?._meta?.cortexReplay;
              if (!manifest || typeof manifest.token !== "string" ||
                  !Number.isSafeInteger(manifest.count) || manifest.count < 0 ||
                  !Number.isSafeInteger(manifest.bytes) || manifest.bytes < 0 ||
                  !/^[a-f0-9]{64}$/.test(manifest.sha256))
                throw new Error("Incomplete history: missing replay manifest");
              load.token = manifest.token; load.expected = manifest; replaying = true;
              // Paseo can announce the provider session before its internal
              // session/load response reaches this connector. The handshake is
              // still valid; acknowledge once both sides are present.
              if (load.opened && load.ack) load.ack(0);
            } else if (load && replaying && frame.method === "session/update" &&
                       frame.params?.sessionId === load.nativeId) {
              const update = frame.params.update;
              if (update?.messageId?.startsWith(BARRIER + load.token + ":")) {
                const barrier = JSON.parse(update.content.text);
                if (barrier.count !== load.count || barrier.bytes !== load.bytes ||
                    barrier.sha256 !== load.hash.copy().digest("hex") ||
                    typeof barrier.final !== "boolean" || load.barrier)
                  throw new Error("Incomplete history: replay integrity mismatch");
                if (barrier.final && (barrier.count !== load.expected!.count ||
                    barrier.bytes !== load.expected!.bytes || barrier.sha256 !== load.expected!.sha256))
                  throw new Error("Incomplete history: final replay manifest mismatch");
                load.barrier = barrier;
                if (barrier.final) replaying = false;
              } else {
                const kind = update?.sessionUpdate;
                if (kind === "agent_message_chunk" || kind === "agent_thought_chunk") {
                  if (update.content?.type !== "text" || typeof update.content.text !== "string")
                    throw new Error("Incomplete history: unsupported replay text");
                  load.items.push({ type: kind === "agent_message_chunk" ? "assistant_message" : "reasoning",
                    id: update.messageId, text: update.content.text });
                } else if (kind === "tool_call" || kind === "tool_call_update") {
                  if (typeof update.toolCallId !== "string" || !update.toolCallId ||
                      (update.status !== undefined && !["pending", "in_progress", "completed", "failed"].includes(update.status)))
                    throw new Error("Incomplete history: invalid saved card");
                  load.items.push({ type: "tool_call", id: update.toolCallId });
                } else if (kind === "plan") load.items.push({ type: "todo", id: "acp-plan" });
                else if (kind === "compaction_update") load.items.push({ type: "compaction", id: update.compactionId });
                else if (!["available_commands_update", "current_mode_update", "config_option_update",
                           "usage_update", "session_info_update"].includes(kind))
                  throw new Error("Incomplete history: unsupported replay update");
                const data = JSON.stringify(frame);
                load.count++; load.bytes += Buffer.byteLength(data); load.hash.update(data);
                if (load.count > load.expected!.count || load.bytes > load.expected!.bytes)
                  throw new Error("Incomplete history: replay exceeds manifest");
              }
            }
            controller.enqueue(frame);
          } catch {
            if (load) this.fail(load, "Incomplete history: replay stream failed validation");
            stop(); controller.error(new Error("Incomplete history: replay stream failed validation"));
          }
        },
        cancel: stop,
      }),
    };
  };

  wrap(connection: Connection): Connection {
    const listeners = new Set<(event: ProviderEvent) => void>();
    this.emit = event => listeners.forEach(listener => listener(event));
    const unsubscribe = connection.onEvent(event => {
      const load = "sessionId" in event ? this.loads.get(event.sessionId) : undefined;
      if (load && event.type === "timeline.item" && event.item.type === "assistant_message" &&
          event.item.messageId?.startsWith(BARRIER + load.token + ":")) {
        if (load.items.length || !load.barrier || event.item.text !== JSON.stringify(load.barrier)) {
          this.fail(load, "Incomplete history: replay barrier was not preserved"); return;
        }
        const barrier = load.barrier; load.barrier = undefined;
        load.ack!(barrier.count);
        if (barrier.final) {
          load.complete = true;
          clearTimeout(load.timer);
          if (load.ready) { this.loads.delete(load.sessionId); this.emit(load.ready); }
        }
        return;
      }
      if (load && event.type === "timeline.item" && !load.complete) {
        const expected = load.items.shift();
        if (!expected || event.item.type !== expected.type ||
            (expected.id !== undefined && event.item.id !== expected.id) ||
            (expected.text !== undefined && (!("text" in event.item) || !event.item.text.endsWith(expected.text)))) {
          this.fail(load, "Incomplete history: replay item was lost or changed"); return;
        }
      }
      if (load && event.type === "session.ready") {
        if (!load.complete) { load.ready = event; return; }
        this.loads.delete(load.sessionId);
      }
      if (event.type === "request.failed") {
        const pending = [...this.loads.values()].find(item => item.requestId === event.requestId);
        if (pending) { this.loads.delete(pending.sessionId); clearTimeout(pending.timer); pending.stop?.(); }
      }
      if (load && (event.type === "session.runtime_failed" || event.type === "session.closed"))
        this.fail(load);
      this.emit(event);
      // All subscribers have seen session.opened before any replay is released.
      if (load && event.type === "session.opened") {
        load.opened = true;
        if (load.token && load.ack) load.ack(0);
      }
    });
    return {
      ...connection,
      send: async input => {
        if (input.type === "session.open" && input.persistence) {
          const data = input.persistence.data as { sessionId?: string };
          if (typeof data?.sessionId === "string") {
            if ([...this.loads.values()].some(item => item.nativeId === data.sessionId))
              throw new Error("History is already loading");
            const load: Load = { sessionId: input.sessionId, requestId: input.requestId,
              nativeId: data.sessionId, items: [], count: 0, bytes: 0, hash: createHash("sha256"), complete: false,
              timer: setTimeout(() => this.fail(load, "Incomplete history: replay timed out"), 120000) };
            this.loads.set(input.sessionId, load);
            // session/load restores the task's own configuration. The SDK
            // otherwise reapplies Paseo's cached composer settings before
            // session.opened, turning display-only history into mutations
            // (or a load failure when an old model/effort is no longer valid).
            input = { ...input, config: { ...input.config,
              model: undefined, mode: undefined, thinkingOption: undefined, settings: {},
            } };
          }
        } else if ("sessionId" in input && this.loads.has(input.sessionId)) {
          if (input.type === "session.close") this.fail(this.loads.get(input.sessionId)!);
          else throw new Error("History is still loading; retry after restoration completes");
        }
        try { await connection.send(input); }
        catch (error) {
          if ("sessionId" in input) {
            const pending = this.loads.get(input.sessionId);
            if (pending) this.fail(pending);
          }
          throw error;
        }
      },
      onEvent: listener => { listeners.add(listener); return () => { listeners.delete(listener); }; },
      close: async () => {
        for (const load of this.loads.values()) this.fail(load, "Incomplete history: replay cancelled");
        for (const child of this.children) child.kill("SIGTERM");
        unsubscribe(); listeners.clear();
        await connection.close();
      },
    };
  }
}
