// The shipped native service, started in its native form (`workspace-service <allowed-root> <runtime-root>`) with a
// disposable HOME and XDG tree, and a client for its length-prefixed JSON-RPC socket. No engine or account exists in it.
import { spawn, type ChildProcess } from "node:child_process";
import { createConnection, type Socket } from "node:net";
import { closeSync, existsSync, mkdirSync, mkdtempSync, openSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "../../..");
const BINARY = process.env.WORKSPACE_TEST_SERVICE_BINARY ?? resolve(ROOT, "target/debug/workspace-service");

const SOCKET = "cortex-workspace-dev/service.sock";

export function nativeServiceHome(): string {
  return mkdtempSync(resolve(tmpdir(), "cw-"));
}

export type Reply = { ok?: boolean; result?: unknown; code?: string; message?: string; [key: string]: unknown };

/** Frames are a 4-byte big-endian length and a JSON body, as the desktop and the Paseo bridge speak them. */
class Client {
  private number = 0;
  private buffer = Buffer.alloc(0);
  private waiting = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>();
  private socket: Socket;
  ready: Promise<void>;
  constructor(path: string) {
    this.socket = createConnection(path);
    this.ready = new Promise((resolveReady, reject) => { this.socket.once("connect", () => resolveReady()); this.socket.once("error", reject); });
    this.socket.on("data", chunk => { this.buffer = Buffer.concat([this.buffer, chunk]); this.pump(); });
    this.socket.on("close", () => this.fail(new Error("service closed the connection")));
  }
  private fail(error: Error) { for (const waiter of this.waiting.values()) waiter.reject(error); this.waiting.clear(); }
  private pump() {
    while (this.buffer.length >= 4) {
      const size = this.buffer.readUInt32BE(0);
      if (size === 0 || size > 16 * 1048576) return this.fail(new Error("invalid frame size"));
      if (this.buffer.length < 4 + size) return;
      const message = JSON.parse(this.buffer.subarray(4, 4 + size).toString());
      this.buffer = this.buffer.subarray(4 + size);
      if (message.method) continue; // a notification, not our reply
      const waiting = this.waiting.get(message.id); this.waiting.delete(message.id);
      if (!waiting) continue; // a reply nobody waits for any more (it timed out)
      if (message.error) waiting.reject(new Error("service error: " + JSON.stringify(message.error)));
      else waiting.resolve(message.result);
    }
  }
  async call(method: string, params: unknown, timeout = 30000): Promise<unknown> {
    await this.ready;
    const id = ++this.number;
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    const header = Buffer.alloc(4); header.writeUInt32BE(body.length);
    const reply = new Promise<unknown>((resolveReply, reject) => {
      const timer = setTimeout(() => { this.waiting.delete(id); reject(new Error("RPC timeout: " + method)); }, timeout);
      this.waiting.set(id, { resolve: value => { clearTimeout(timer); resolveReply(value); }, reject: error => { clearTimeout(timer); reject(error); } });
    });
    this.socket.write(Buffer.concat([header, body]));
    return reply;
  }
  close() { this.socket.destroy(); }
}

export class NativeService {
  private constructor(readonly child: ChildProcess, private client: Client, readonly exited: Promise<number>) {}

  /** `home` becomes HOME and every XDG directory; `projects` is the one allowed project root. */
  static async start(home: string, projects: string, env: Record<string, string> = {}): Promise<NativeService> {
    if (!existsSync(BINARY) || !statSync(BINARY).isFile()) throw new Error("run `cargo build --locked -p workspace-service` first");
    if (!existsSync(resolve(ROOT, ".git"))) throw new Error("the native service accepts only a source checkout (with .git) or an installed bundle as its runtime root");
    mkdirSync(projects, { recursive: true });
    const dirs = { config: resolve(home, "config"), state: resolve(home, "xdg-state"), cache: resolve(home, "cache"), runtime: resolve(home, "runtime") };
    for (const dir of Object.values(dirs)) mkdirSync(dir, { recursive: true, mode: 0o700 });
    const clean: Record<string, string> = {};
    for (const key of ["PATH", "LANG", "LC_ALL"]) if (process.env[key] !== undefined) clean[key] = process.env[key]!;
    Object.assign(clean, { HOME: home, TMPDIR: home, XDG_CONFIG_HOME: dirs.config, XDG_STATE_HOME: dirs.state, XDG_CACHE_HOME: dirs.cache, XDG_RUNTIME_DIR: dirs.runtime }, env);
    const child = spawn(BINARY, [projects, ROOT], { env: clean, stdio: "ignore" });
    const exited = new Promise<number>(resolveExit => child.on("exit", (code, signal) => resolveExit(code ?? (signal ? 1 : 0))));
    // The service socket lives under the channel's runtime namespace; the debug build is the dev channel.
    const socket = resolve(dirs.runtime, SOCKET);
    const deadline = Date.now() + 15000;
    while (!existsSync(socket) && Date.now() < deadline) {
      if (child.exitCode !== null) throw new Error("service exited");
      await new Promise(r => setTimeout(r, 20));
    }
    if (!existsSync(socket)) { child.kill("SIGKILL"); throw new Error("service socket did not appear"); }
    // The service binds through a directory FD on Linux. Use the same method
    // to connect when TMPDIR makes the full pathname too long for sun_path.
    if (Buffer.byteLength(socket) < 108) return new NativeService(child, new Client(socket), exited);
    const dir = openSync(dirname(socket), "r");
    try {
      const client = new Client(`/proc/self/fd/${dir}/${basename(socket)}`);
      await client.ready;
      return new NativeService(child, client, exited);
    } catch (error) {
      child.kill("SIGKILL");
      throw error;
    } finally {
      closeSync(dir);
    }
  }

  call(method: string, params: unknown) { return this.client.call(method, params); }

  /** One helper operation of the machine endpoint, answered as the service answers it: `{ ok, result }` or a refusal. */
  machine(op: string, projectId: string | undefined, params: unknown = {}): Promise<Reply> {
    return this.client.call("machine.call", { op, ...(projectId ? { projectId } : {}), params }) as Promise<Reply>;
  }

  async stop() {
    try { await this.client.call("service.stop", {}, 10000); } catch { /* it is going away */ }
    this.client.close();
    if (await Promise.race([this.exited.then(() => true), new Promise<boolean>(r => setTimeout(() => r(false), 10000))]) !== true) this.child.kill("SIGKILL");
  }
}
