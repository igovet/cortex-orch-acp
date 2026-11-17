// Disposable real service/project for the feed-scroll gate. The service is the shipped native
// binary launched in its native form: `workspace-service <allowed-root> <runtime-root>`. No
// native engine can be constructed by this launcher (disposable HOME, no connections).
// Prints one JSON line when seeded, waits for one line on stdin, then stops the service.
import {spawn} from "node:child_process";
import {createConnection} from "node:net";
import {existsSync, mkdtempSync, mkdirSync, rmSync, statSync} from "node:fs";
import {tmpdir} from "node:os";
import {join, resolve} from "node:path";
import {createInterface} from "node:readline";
import {fileURLToPath} from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const binary = process.env.WORKSPACE_TEST_SERVICE_BINARY ?? resolve(root, "target/debug/workspace-service");
if (!existsSync(binary) || !statSync(binary).isFile()) throw Error("run `cargo build --locked -p workspace-service` first");
if (!existsSync(join(root, ".git"))) throw Error("the native service accepts only a source checkout (with .git) or an installed bundle as its runtime root");

class Client {
  constructor(path) {
    this.number = 0;
    this.buffer = Buffer.alloc(0);
    this.waiting = null;
    this.ready = new Promise((resolveReady, reject) => {
      this.socket = createConnection(path, resolveReady);
      this.socket.once("error", reject);
    });
    this.socket.on("data", chunk => { this.buffer = Buffer.concat([this.buffer, chunk]); this.pump(); });
    this.socket.on("close", () => this.waiting?.reject(Error("service closed the connection")));
  }
  pump() {
    while (this.buffer.length >= 4) {
      const size = this.buffer.readUInt32BE(0);
      if (size === 0 || size > 1048576) return this.waiting?.reject(Error("invalid frame size"));
      if (this.buffer.length < 4 + size) return;
      const message = JSON.parse(this.buffer.subarray(4, 4 + size).toString());
      this.buffer = this.buffer.subarray(4 + size);
      if (message.method) continue; // a notification, not our reply
      const waiting = this.waiting;
      this.waiting = null;
      if (message.id !== this.number) return waiting?.reject(Error("reply for another request"));
      if (message.error) return waiting?.reject(Error("service error: " + JSON.stringify(message.error)));
      waiting?.resolve(message.result);
    }
  }
  async call(method, params) {
    await this.ready;
    const id = ++this.number;
    const body = Buffer.from(JSON.stringify({jsonrpc: "2.0", id, method, params}));
    const header = Buffer.alloc(4);
    header.writeUInt32BE(body.length);
    const reply = new Promise((resolveReply, reject) => {
      const timer = setTimeout(() => reject(Error("RPC timeout: " + method)), 15000);
      this.waiting = {resolve: value => { clearTimeout(timer); resolveReply(value); }, reject: error => { clearTimeout(timer); reject(error); }};
    });
    this.socket.write(Buffer.concat([header, body]));
    return reply;
  }
  close() { this.socket.destroy(); }
}

const base = mkdtempSync(join(tmpdir(), "cw-feed-startup-"));
const project = join(base, "projects", "fixture");
mkdirSync(project, {recursive: true});
const home = join(base, "home");
mkdirSync(home);
for (const name of ["config", "state", "cache", "runtime"]) mkdirSync(join(home, name), {mode: 0o700});
const env = {};
for (const key of ["PATH", "LANG", "LC_ALL"]) if (process.env[key] !== undefined) env[key] = process.env[key];
Object.assign(env, {HOME: home, XDG_CONFIG_HOME: join(home, "config"), XDG_STATE_HOME: join(home, "state"),
  XDG_CACHE_HOME: join(home, "cache"), XDG_RUNTIME_DIR: join(home, "runtime")});
const child = spawn(binary, [join(base, "projects"), root], {env, stdio: "ignore"});
const exited = new Promise(resolveExit => child.on("exit", (code, signal) => resolveExit(code ?? (signal ? 1 : 0))));
let client;
let failure;
try {
  // The service socket lives under the channel's runtime namespace; the debug build is the dev channel.
  const socket = join(home, "runtime/cortex-workspace-dev/service.sock");
  const deadline = Date.now() + 15000;
  while (!existsSync(socket) && Date.now() < deadline) {
    if (child.exitCode !== null) throw Error("service exited");
    await new Promise(r => setTimeout(r, 20));
  }
  if (!existsSync(socket)) throw Error("service socket did not appear");
  client = new Client(socket);
  const projectId = (await client.call("projects.register", {root: project, displayName: "Disposable sprint project"})).projectId;
  await client.call("projects.activate", {projectId});
  const machine = async (op, params) => {
    const response = await client.call("machine.call", {op, projectId, params});
    if (!response.ok) throw Error("machine call refused: " + (response.code ?? op));
    return response.result;
  };
  const sprint = (await machine("product.sprint", {action: "create", description: "Disposable mixed-state sprint", runMode: "manual"})).sprintId;
  const ids = {};
  const labels = ["Backlog fixture", "Done fixture", "Planning fixture", "Sprint fixture", "Archived fixture"];
  for (const [index, label] of labels.entries()) {
    const reply = await client.call("workspace", {protocol: {major: 1, minor: 0}, requestId: "seed-" + index, projectId, actorId: "fixture",
      operation: "tasks.create", target: projectId, expectedVersion: "0", ownerEpoch: null, payload: {description: label, checkout: {kind: "main"}}});
    ids[label] = reply.target;
  }
  for (const label of ["Planning fixture", "Sprint fixture"])
    await machine("product.task", {action: "sprint", taskId: ids[label], sprintId: sprint, planning: label.startsWith("Planning")});
  for (const label of ["Done fixture", "Archived fixture"])
    await machine("product.task", {action: "close", taskId: ids[label], choice: "as_is", confirmed: true});
  await machine("product.task", {action: "merged", taskId: ids["Archived fixture"]});
  await machine("product.task", {action: "archive", taskId: ids["Archived fixture"]});
  await machine("product.sprint", {action: "patch", sprintId: sprint, patch: {taskIds: [ids["Sprint fixture"]], planningTaskId: ids["Planning fixture"]}});
  // The actual startup projection and product read intentionally disagree until combined.
  const product = await machine("product.read", {});
  if (!product.tasks[ids["Archived fixture"]]?.archive) throw Error("fixture archive was not recorded");
  const overlays = Object.fromEntries(Object.entries(product.tasks).map(([key, value]) =>
    [key, {planning: value.planning ?? null, status: value.status ?? null, archive: Boolean(value.archive)}]));
  console.log(JSON.stringify({socket, projectId, tasks: ids, sprintId: sprint, overlays}));
  await new Promise(resolveLine => createInterface({input: process.stdin}).once("line", resolveLine).once("close", resolveLine));
  await client.call("service.stop", {});
  if (await Promise.race([exited, new Promise(r => setTimeout(() => r("timeout"), 15000))]) !== 0) throw Error("service did not stop cleanly");
} catch (error) {
  failure = error;
} finally {
  client?.close();
  if (child.exitCode === null) {
    child.kill("SIGTERM");
    if (await Promise.race([exited.then(() => true), new Promise(r => setTimeout(() => r(false), 10000))]) !== true) child.kill("SIGKILL");
  }
  rmSync(base, {recursive: true, force: true});
}
if (failure) { console.error(String(failure.stack ?? failure)); process.exit(1); }
