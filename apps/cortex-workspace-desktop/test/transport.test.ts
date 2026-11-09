import { describe, it, expect, vi } from "vitest";
import { createDesktopTransport } from "../src/transport";
import { build } from "vite";
import { fileURLToPath } from "node:url";
import { bundleViolations } from "../../../packages/workspace-ui/test/boundary";
import { readFileSync } from "node:fs";

describe("native port", () => {
  it("fences new writes and waits for outstanding acknowledgements before closing", async () => {
    let answer!: (value: unknown) => void;
    const invoke = vi.fn(() => new Promise(resolve => { answer = resolve; }));
    const native = createDesktopTransport({ invoke, listen: async () => () => {} });
    const write = native.transport.request("machine.call", { op: "repo.file.save" });
    const finish = vi.fn(async () => {});
    const closing = native.close(finish);
    await expect(native.transport.request("machine.call", {})).rejects.toThrow("closing");
    expect(finish).not.toHaveBeenCalled();
    answer({ outcome: "saved" });
    await write; await closing;
    expect(finish).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledOnce();
  });
  it("does not terminate on an unacknowledged request; retry never resends it", async () => {
    let fail!: (error: Error) => void;
    const invoke = vi.fn().mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; })).mockResolvedValue({});
    const native = createDesktopTransport({ invoke, listen: async () => () => {} });
    const write = native.transport.request("machine.call", {}).catch(() => {});
    const finish = vi.fn(async () => {});
    const closing = native.close(finish);
    fail(new Error("lost reply"));
    await expect(closing).rejects.toThrow("lost reply");
    await write;
    expect(finish).not.toHaveBeenCalled();
    await native.transport.request("service.status", {});
    expect(invoke).toHaveBeenCalledTimes(2);
  });
  it("forwards registry reads unchanged; the shared port owns project discovery", async () => {
    const invoke = vi.fn(async (body: {operation: string; message: unknown}) => body.operation === "projects.list"
      ? [{projectId:"p",displayName:"Project",canonicalRoot:"/project",activation:{actorId:"native"}}]
      : {operations:[]});
    const native = createDesktopTransport({invoke, listen: async () => () => {}});
    const projects = await native.transport.request("projects.list", {});
    expect(projects).toEqual([{projectId:"p",displayName:"Project",canonicalRoot:"/project",activation:{actorId:"native"}}]);
    expect(invoke.mock.calls[0][0].operation).toBe("projects.list");
    native.dispose();
  });
  it("a refused or failed action stays that action's answer, and is never retried", async () => {
    const invoke = vi.fn().mockRejectedValue("service_request_failed");
    const native = createDesktopTransport({invoke, listen: async () => () => {}});
    const updates: unknown[] = [];
    native.transport.subscribe(event => updates.push(event));
    for (const operation of ["projects.register", "projects.activate", "machine.call", "tasks.start", "capabilities.get", "terminal.read"])
      await expect(native.transport.request(operation, {requestId:"one"})).rejects.toThrow("service_request_failed");
    expect(invoke).toHaveBeenCalledTimes(6);
    expect(updates).toEqual([]);
    native.dispose();
  });
  it("only the status check can take the machine offline, and its next answer brings it back", async () => {
    const invoke = vi.fn().mockRejectedValueOnce("service_disconnected").mockResolvedValue({channel:"dev"});
    const native = createDesktopTransport({invoke, listen: async () => () => {}});
    const updates: unknown[] = [];
    native.transport.subscribe(event => updates.push(event));
    await expect(native.transport.request("service.status", {})).rejects.toThrow("service_disconnected");
    expect(updates).toEqual([{type:"connection",state:"disconnected",reason:"Machine service disconnected"}]);
    await native.transport.request("service.status", {});
    await native.transport.request("service.status", {});
    expect(updates).toEqual([{type:"connection",state:"disconnected",reason:"Machine service disconnected"},{type:"connection",state:"connected"}]);
    native.dispose();
  });
  it("passes the service's project change through as a refresh", async () => {
    let event: (e: unknown) => void = () => {};
    const native = createDesktopTransport({invoke: vi.fn(), listen: async f => { event=f; return () => {}; }});
    const updates: unknown[] = [];
    native.transport.subscribe(e => updates.push(e));
    await Promise.resolve();
    event({type:"refresh"});
    expect(updates).toEqual([{type:"refresh"}]);
    native.dispose();
  });
  it("validates native event shape and releases an asynchronously installed listener", async () => {
    let event: (e: unknown) => void = () => {};
    let resolve!: (f: () => void) => void;
    const unlisten = vi.fn();
    const native = createDesktopTransport({invoke: vi.fn(), listen: f => { event=f; return new Promise(r => {resolve=r;}); }});
    const updates: unknown[] = [];
    native.transport.subscribe(e => updates.push(e));
    event({type:"invalidated",projectId:"p"});
    event({type:"invalidated",projectId:42});
    expect(updates.filter((e: any) => e.type === "invalidated")).toHaveLength(1);
    native.dispose();
    resolve(unlisten);
    await Promise.resolve();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});

it("bundles the real shared presentation with no demo backend", async () => {
  const result: any = await build({root:fileURLToPath(new URL("..", import.meta.url)),build:{write:false},logLevel:"silent"});
  const output = result.output.filter((o: any) => o.type === "chunk");
  expect(bundleViolations(output.map((o: any) => ({modules:Object.keys(o.modules),code:o.code})))).toEqual([]);
  expect(output.flatMap((o: any) => Object.keys(o.modules)).some((p: string) => p.endsWith("/workspace-ui/src/port/real.ts"))).toBe(true);
});
it("ships no remote capabilities, generic plugin or network escape", () => {
  const root = new URL("../src-tauri/", import.meta.url);
  const cap = JSON.parse(readFileSync(new URL("capabilities/main.json", root), "utf8"));
  expect(cap.windows).toEqual(["main"]);
  expect(cap.remote).toBeUndefined();
  expect(cap.permissions).toEqual(["allow-workspace-request","allow-workspace-notify","allow-workspace-close","core:event:allow-listen","core:event:allow-unlisten",
    "core:window:allow-minimize","core:window:allow-toggle-maximize","core:window:allow-is-maximized","core:window:allow-close","core:window:allow-start-dragging"]);
  const cfg = JSON.parse(readFileSync(new URL("tauri.conf.json", root), "utf8"));
  expect(cfg.build.devUrl).toBeUndefined();
  expect(cfg.app.security.csp).not.toMatch(/unsafe-eval|https:|\*/);
  expect(cfg.app.security.csp).toContain("frame-src 'none'");
  expect(cfg.app.security.csp).toContain("font-src 'self';");
});
it("bundles the design faces as files the closed CSP can load", async () => {
  const result: any = await build({root:fileURLToPath(new URL("..", import.meta.url)),build:{write:false},logLevel:"silent"});
  const names = result.output.filter((o: any) => o.type === "asset").map((o: any) => o.fileName as string);
  for (const face of ["roboto-latin-wght","roboto-cyrillic-wght","cousine-latin-400","cousine-cyrillic-400"])
    expect(names.some((n: string) => n.includes(face) && n.endsWith(".woff2")), face).toBe(true);
  const css = result.output.find((o: any) => String(o.fileName).endsWith(".css")).source as string;
  expect(css).not.toMatch(/url\(["']?data:font|data:application\/font/);
  expect(readFileSync(new URL("../public/licenses/fonts.html", import.meta.url), "utf8")).toContain("SIL OPEN FONT LICENSE");
});
