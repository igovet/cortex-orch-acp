import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { pinLogoSource } from "../src/startup";
import { flush, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });

const held = () => { let open!: (value: unknown) => void, fail!: (error: Error) => void; const promise = new Promise((a, b) => { open = a; fail = b; }); return { promise, open, fail }; };
const chrome = () => ({ maximized: () => false, minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn(), startDragging: vi.fn(), guardClose: () => {}, closeNow: () => {} });
const logo = () => { const img = document.createElement("img"); img.alt = ""; return img; };
const FRAME = ".titlebar, .sidebar, .ws-tabstrip, .main";
const slot = () => document.querySelector(".l-startup-slot")?.textContent?.trim() ?? "";
const press = (name: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => (b.getAttribute("aria-label") ?? b.textContent)?.trim() === name);

type Query = ReturnType<typeof scriptedPort>["port"]["query"];
const open = { promise: Promise.resolve(), open: () => {}, fail: () => {} } as unknown as ReturnType<typeof held>;
function mount(extra: Record<string, unknown> = {}, gate = held(), wrap?: (query: Query) => Query) {
  const s = scriptedPort(), state = withTask(emptyWorkspace(), "A task");
  const reads = { count: 0 };
  s.port.query = (async (op: string) => {
    reads.count++;
    await gate.promise;
    return op === "capabilities.get" ? { everything: true, operations: [] } : op === "project.snapshot" ? [{ kind: "projection", scope: "workspace", version: "1", state }] : [];
  }) as typeof s.port.query;
  if (wrap) s.port.query = wrap(s.port.query);
  const root = document.createElement("div"); document.body.append(root);
  history.replaceState(null, "", "/");
  dispose = render(() => <Workspace port={s.port} {...extra} />, root);
  return { s, root, gate, reads };
}

it("shows only the logo and the window controls until the first coherent read, then the whole interface at once", async () => {
  const c = chrome(), image = logo();
  const { root, gate } = mount({ chrome: c, logo: image });
  await flush();
  expect(root.querySelector(".l-startup-logo img")).toBe(image);
  expect(root.querySelector(FRAME)).toBeNull();
  expect(root.textContent).toBe("");
  press("Minimize")!.click(); press("Maximize")!.click(); press("Close window")!.click();
  expect([c.minimize, c.toggleMaximize, c.close].map(f => f.mock.calls.length)).toEqual([1, 1, 1]);
  gate.open(null);
  await flush(); await flush();
  expect(root.querySelector(".l-startup")).toBeNull();
  expect(root.querySelector(".titlebar")).toBeTruthy();
  expect(root.querySelector(".sidebar")).toBeTruthy();
});

it("never shows a logo that failed to arrive as a broken picture, and asks for it again until it does", async () => {
  vi.useFakeTimers();
  let width = 0;
  const image = logo(); image.setAttribute("src", "/cortex-256.png");
  Object.defineProperty(image, "complete", { get: () => true });
  Object.defineProperty(image, "naturalWidth", { get: () => width });
  const asked = vi.spyOn(image, "src", "set");
  mount({ logo: image });
  await vi.advanceTimersByTimeAsync(0);
  expect(image.style.visibility).toBe("hidden");
  await vi.advanceTimersByTimeAsync(300);
  expect(asked).toHaveBeenCalledWith("/cortex-256.png");
  width = 256; image.dispatchEvent(new Event("load"));
  expect(image.style.visibility).toBe("");
});

it("keeps asking for the logo at the page's first address after the router moves the page", () => {
  // The built page writes the logo as ./cortex-256.png; under /m/local/p/x that would be /m/local/p/cortex-256.png, a missing image.
  history.replaceState(null, "", "/");
  const image = logo(); image.setAttribute("src", "./cortex-256.png");
  pinLogoSource(image);
  const at = new URL("/cortex-256.png", location.href).href;
  expect(image.getAttribute("src")).toBe(at);
  history.replaceState(null, "", "/m/local/p/x/code");
  expect(image.src).toBe(at);
  pinLogoSource(image);
  expect(image.getAttribute("src")).toBe(at);
  history.replaceState(null, "", "/");
});

it("says it is loading only after two seconds, with a loader but no progress figure, and more after fifteen", async () => {
  vi.useFakeTimers();
  const notice = <div class="strip info">Codex: Installing the qualified adapter…</div>;
  mount({ notice });
  await vi.advanceTimersByTimeAsync(1900);
  expect(slot()).toBe("");
  expect(document.querySelector(".l-startup .spin")).toBeNull();
  await vi.advanceTimersByTimeAsync(200);
  expect(slot()).toBe("Loading workspace…");
  expect(document.querySelector(".l-startup .spin")).not.toBeNull();
  await vi.advanceTimersByTimeAsync(13000);
  expect(slot()).toContain("Still waiting for the service. The first start can take longer.");
  expect(slot()).toContain("Installing the qualified adapter");
  expect(document.querySelector(".l-startup .spin")).not.toBeNull();
  expect(document.querySelector(".l-startup [role=progressbar]")).toBeNull();
});

it("keeps a failure on screen with its reason and a working Try again, and a later success reveals the interface", async () => {
  // The service answers, but the workspace it holds cannot be read: there is nothing true to show.
  const { root, reads } = mount({}, open, answer => (async (op: string, params: never) => {
    if (op === "project.facts") throw new Error("Product facts were refused");
    return answer(op as never, params);
  }) as Query);
  await flush(); await flush();
  expect(slot()).toContain("The workspace could not be loaded.");
  expect(slot()).toContain("Product facts were refused");
  expect(root.querySelector(FRAME)).toBeNull();
  const before = reads.count;
  press("Try again")!.click();
  await flush();
  expect(reads.count).toBeGreaterThan(before);
  // Still refused: the failure stays, it never falls back to an endless logo.
  await flush();
  expect(slot()).toContain("could not be loaded");
});

it("opens an offline relaunch into the offline workspace, never the failure page, and fills it once the service is back", async () => {
  // The service is stopped: every read fails before it answers, and the machine's own connection line says so.
  let away = true;
  const { s, root } = mount({ chrome: chrome(), logo: logo() }, open, answer => (async (op: string, params: never) => {
    if (away) throw new Error("Machine service disconnected");
    return answer(op as never, params);
  }) as Query);
  s.emit({ kind: "connection", connection: { state: "disconnected", reason: "Machine service disconnected", at: Date.now() } });
  await flush(); await flush();
  expect(root.querySelector(".l-startup")).toBeNull();
  for (const part of [".titlebar", ".sidebar", ".main", ".ws-conn"]) expect(root.querySelector(part), part).toBeTruthy();
  expect(press("Add project")).toBeTruthy();
  expect(root.querySelector(".main [role=status]")?.textContent).toContain("The workspace could not be loaded.");
  // Nothing about projects is guessed while the machine is away.
  expect(root.textContent).not.toMatch(/No projects on this machine|This task is not on this machine/);
  // Its own reads and the machine's connection line take turns naming it; the frame never flickers back to the logo.
  s.emit({ kind: "connection", connection: { state: "unavailable", reason: "Machine service disconnected", at: Date.now() } });
  await flush();
  expect(root.querySelector(".l-startup")).toBeNull();
  // Retry while still away keeps the frame too.
  press("Retry")!.click();
  expect(root.querySelector(".l-startup")).toBeNull();
  await flush(); await flush();
  expect(root.querySelector(".ws-conn")).toBeTruthy();
  away = false;
  s.emit({ kind: "connection", connection: { state: "connected", reason: null, at: Date.now() } });
  await flush(); await flush();
  expect(root.querySelector(".ws-conn")).toBeNull();
  expect(root.textContent).not.toMatch(/could not be loaded/);
  expect(root.querySelector(".l-startup")).toBeNull();
});

it("ignores the app's global shortcuts while it starts", async () => {
  const { root } = mount();
  await flush();
  for (const init of [{ key: "k", ctrlKey: true }, { key: "t", code: "KeyT", ctrlKey: true }, { key: "?" }]) window.dispatchEvent(new KeyboardEvent("keydown", { ...init, bubbles: true }));
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
  await flush();
  expect(root.querySelector("dialog[open]")).toBeNull();
  expect(root.querySelector(".ws-pal")).toBeNull();
});

it("shows a close failure as soon as it happens, even in the first seconds", async () => {
  const [closeError, setCloseError] = createSignal<string | null>(null);
  mount({ chrome: { ...chrome(), closeError } });
  await flush();
  expect(slot()).toBe("");
  setCloseError("The window could not close safely.");
  expect(slot()).toBe("The window could not close safely.");
});
