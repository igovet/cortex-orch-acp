// Shared by the rendered drivers (`*.driver.mjs`, `*.measure.mjs`, `*.render.mjs`), which run inside the
// cortex-workspace-rendered image: a private Xvfb, one WebKitGTK window (webkit-host.c, compiled here on first use), page
// scripts evaluated through it, window captures, and the real X input of xdotool. Nothing here talks to the owner's display.
import {execFileSync, spawn} from "node:child_process";
import {createInterface} from "node:readline";
import {mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync, existsSync} from "node:fs";
import {tmpdir} from "node:os";
import {join, dirname} from "node:path";
import {fileURLToPath} from "node:url";

export const sleep = seconds => new Promise(resolve => setTimeout(resolve, seconds * 1000));
export const here = dirname(fileURLToPath(import.meta.url));
export const testDir = join(here, "..");
// A script body saved beside the driver (the page-side probes keep their own files so they read as JavaScript).
export const probe = name => readFileSync(join(here, "probes", name), "utf8");

export const screen = {xvfbPid: null};
const cleanups = [];
export function onCleanup(fn) { cleanups.push(fn); }
async function cleanup() { for (const fn of cleanups.splice(0).reverse()) { try { await fn(); } catch { /* best effort */ } } }

/** A private X server. `-displayfd` reports the display it picked on its first output line. */
export async function startXvfb(geometry = "1920x1200x24") {
  const xvfb = spawn("Xvfb", ["-displayfd", "1", "-screen", "0", geometry, "-nolisten", "tcp"], {stdio: ["ignore", "pipe", "ignore"]});
  screen.xvfbPid = xvfb.pid;
  const display = await new Promise((resolve, reject) => {
    createInterface({input: xvfb.stdout}).once("line", line => resolve(":" + line.trim()));
    xvfb.once("error", reject);
    xvfb.once("exit", code => reject(Error("Xvfb exited " + code)));
  });
  process.env.DISPLAY = display; // xdotool and every child it starts act on this private screen only
  onCleanup(async () => {
    xvfb.kill("SIGTERM");
    await new Promise(resolve => { const timer = setTimeout(() => { xvfb.kill("SIGKILL"); resolve(); }, 5000); xvfb.once("exit", () => { clearTimeout(timer); resolve(); }); });
  });
  return display;
}

export const engineEnv = (display, {compositing = false} = {}) => ({
  ...process.env, DISPLAY: display, GDK_BACKEND: "x11", NO_AT_BRIDGE: "1", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS: "1",
  ...(compositing ? {} : {WEBKIT_DISABLE_COMPOSITING_MODE: "1"}),
});

let compiled;
export function compileHost() {
  if (compiled) return compiled;
  // The image compiles the window host when it is built; an older image that still carries a compiler gets it compiled here.
  if (existsSync("/opt/cortex-render/webkit-host")) return compiled = "/opt/cortex-render/webkit-host";
  const dir = mkdtempSync(join(tmpdir(), "cw-webkit-host-"));
  const out = join(dir, "webkit-host");
  const flags = execFileSync("pkg-config", ["--cflags", "--libs", "gtk+-3.0", "webkit2gtk-4.1"], {encoding: "utf8"}).trim().split(/\s+/);
  execFileSync("cc", ["-O1", "-o", out, join(here, "webkit-host.c"), ...flags], {stdio: ["ignore", "inherit", "inherit"]});
  onCleanup(() => rmSync(dir, {recursive: true, force: true}));
  return compiled = out;
}

export class Host {
  /** `env` is the whole environment for the engine; `script` is injected into every top-level page before its own scripts. */
  static async start({width = 1280, height = 800, decorated = true, script = "", env, timeout = 20}) {
    const host = new Host(timeout);
    host.child = spawn(compileHost(), [], {env, stdio: ["pipe", "pipe", "inherit"]});
    createInterface({input: host.child.stdout}).on("line", line => host.answer(line));
    host.child.on("exit", () => host.fail(Error("the WebKit host exited")));
    onCleanup(() => host.stop());
    await host.send("new", `${width} ${height} ${decorated ? 1 : 0}`, script);
    host.version = (await host.send("version")).split(".").map(Number);
    return host;
  }
  constructor(timeout) { this.timeout = timeout; this.pending = new Map(); this.number = 0; }
  answer(line) {
    const [id, status, body = ""] = line.split(" ");
    const waiter = this.pending.get(id);
    if (!waiter) return;
    this.pending.delete(id);
    const text = Buffer.from(body, "base64").toString("utf8");
    status === "ok" ? waiter.resolve(text) : waiter.reject(Error(text));
  }
  fail(error) { for (const waiter of this.pending.values()) waiter.reject(error); this.pending.clear(); }
  send(name, ...args) {
    const id = String(++this.number);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(Object.assign(Error("WebKit callback timeout"), {timeout: true})); }, this.timeout * 1000);
      this.pending.set(id, {resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); }});
      this.child.stdin.write([id, name, ...args.map(a => Buffer.from(String(a)).toString("base64"))].join(" ") + "\n");
    });
  }
  async stop() {
    if (!this.child || this.child.exitCode !== null) return;
    try { await this.send("quit"); } catch { /* it is going away */ }
    this.child.kill("SIGTERM");
  }
  /** The body of a function in the page, run as `JSON.stringify((()=>{BODY})()??null)`; its value comes back parsed. */
  async js(code) {
    return JSON.parse(await this.send("eval", "JSON.stringify((()=>{" + code + "})()??null)"));
  }
  /** Runs a real function in the page (its source text, arguments as JSON) and returns its value; the function may not close over this file. */
  call(f, ...args) { return this.js(`return (${f.toString()})(${args.map(a => JSON.stringify(a ?? null)).join(",")});`); }
  async wait(code, tries = 100, delay = 0.1) {
    for (let i = 0; i < tries; i++) {
      if (await this.js("return !!(" + code + ")")) return;
      await sleep(delay);
    }
    throw new TimeoutError(code);
  }
  load(uri) { return this.send("load", uri); }
  reload() { return this.send("reload"); }
  resize(width, height) { return this.send("resize", `${width} ${height}`); }
  move(x, y) { return this.send("move", `${x} ${y}`); }
  hide() { return this.send("hide"); }
  show() { return this.send("show"); }
  /** The CLIPBOARD selection as text, or null when it is empty. */
  async clipboard() { const text = await this.send("clipboard"); return text === "\x01none" ? null : text; }
  async size() { return (await this.send("size")).split(" ").map(Number); }
  async origin() { return (await this.send("origin")).split(" ").map(Number); }
  /** Saves the window (or the whole screen) as PNG; with `raw` the pixels also go to that file: [width, height, rowstride, channels]. */
  async capture(path, {root = false, raw = ""} = {}) {
    const answer = (await this.send("capture", path, root ? "root" : "window", raw)).split(" ").map(Number);
    return answer;
  }
}
export class TimeoutError extends Error { constructor(message) { super(message); this.name = "TimeoutError"; } }

/** Real X input, as the former drivers ran it (XTEST through xdotool). */
export function xdotool(...args) { execFileSync("xdotool", args.map(String), {stdio: ["ignore", "inherit", "inherit"]}); }
export const have = name => { try { execFileSync("which", [name], {stdio: "ignore"}); return true; } catch { return false; } };

/** Runs a driver: one failure path, the cleanups always, and the process exits with the driver's code. */
export async function main(driver) {
  let code = 1;
  try { code = (await driver()) ?? 0; }
  catch (error) { console.error(error?.name + ": " + (error?.message ?? error)); code = 1; }
  await cleanup();
  process.exit(code);
}

/** Python's `sorted(...)[int(len*p)]` and friends, for the same percentile arithmetic as the originals. */
export const sorted = xs => [...xs].sort((a, b) => a - b);
export const writeJson = (path, value, indent) => writeFileSync(path, JSON.stringify(value, null, indent) + (indent ? "\n" : ""));
export {mkdirSync, existsSync, writeFileSync, readFileSync, join};
/** Python's urllib.parse.quote(text, safe=""): everything but the unreserved characters is percent-encoded. */
export const quote = text => [...Buffer.from(text)].map(b => /[A-Za-z0-9_.\-~]/.test(String.fromCharCode(b)) ? String.fromCharCode(b) : "%" + b.toString(16).toUpperCase().padStart(2, "0")).join("");
/** Python's str.replace: every occurrence, and the replacement is literal text (no `$` patterns). */
export const sub = (text, from, to) => text.split(from).join(to);

/** A static site on a loopback port that answers every unknown path with /index.html, as the single-page prototype needs. */
export async function serveSite(directory) {
  const {createServer} = await import("node:http");
  const {readFile} = await import("node:fs/promises");
  const types = {".js": "text/javascript", ".css": "text/css", ".html": "text/html", ".png": "image/png", ".svg": "image/svg+xml", ".json": "application/json", ".woff2": "font/woff2", ".woff": "font/woff", ".ttf": "font/ttf"};
  const server = createServer(async (req, res) => {
    let path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
    if (path.includes("..")) { res.writeHead(400); res.end(); return; }
    let file = join(directory, path.replace(/^\/+/, ""));
    let body;
    try { body = await readFile(file); } catch { file = join(directory, "index.html"); body = await readFile(file).catch(() => null); }
    if (!body) { res.writeHead(404); res.end(); return; }
    res.setHeader("Content-Type", types[file.slice(file.lastIndexOf("."))] ?? "application/octet-stream");
    res.end(body);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  onCleanup(() => new Promise(resolve => server.close(resolve)));
  return `http://127.0.0.1:${server.address().port}`;
}
/** A window manager on the private screen (the window gets real focus, as it would on the owner's desktop). */
export function startOpenbox() {
  const child = spawn("openbox", [], {stdio: "ignore"});
  onCleanup(() => child.kill("SIGTERM"));
  return child;
}
