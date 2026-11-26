// @vitest-environment jsdom
import { createRoot } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountPhone, PAIR_WORDS, Workspace, type PhoneMachine, type PhoneShell, type PhoneState } from "../src";
import type { PhoneNotificationWords, PhoneTap } from "../src/phone-shell";
import { workspaceCopy } from "../src/copy";
import { notificationWords } from "../src/phone-shell";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedPort, scriptedTransport } from "./support";

/** The phone app against a native side driven by hand, and each machine's service scripted through the real port. */
let disposers: (() => void)[] = [];
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { for (const d of disposers.splice(0)) d(); document.body.innerHTML = ""; delete document.documentElement.dataset.shell; });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim() === text || b.getAttribute("aria-label") === text);
const text = () => document.body.textContent ?? "";
const type = (label: string, value: string) => { const el = document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!; el.value = value; el.dispatchEvent(new Event("input", { bubbles: true })); };
const deferred = <T,>() => { let resolve!: (v: T) => void, reject!: (e: Error) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };

function service(methods: string[] = ["projects.list"]) {
  return scriptedTransport({
    "service.capabilities": () => ({ methods }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.answer"]),
  });
}
const machine = (id: string, name: string, patch: Partial<PhoneMachine> = {}): PhoneMachine => ({ id, name, state: "connected", path: "direct", lastConnectedAt: null, ...patch });

/** A native side: it keeps the paired machines and the active one, and lets each test settle the camera and pairing by hand. */
function fakeShell(start: PhoneState = { active: null, machines: [] }) {
  let state = start;
  const listeners = new Set<(s: PhoneState) => void>();
  const transports = new Map<string, ReturnType<typeof service>>();
  const publish = (next: PhoneState) => { state = next; for (const l of listeners) l(state); };
  const scans: ReturnType<typeof deferred<string | null>>[] = [];
  const begins: { link: string; d: ReturnType<typeof deferred<{ machineName: string; fingerprint: string }>> }[] = [];
  const finishes: { name: string; d: ReturnType<typeof deferred<{ machineId: string }>> }[] = [];
  const shell: PhoneShell = {
    state: () => Promise.resolve(state),
    subscribe: listener => { listeners.add(listener); return () => listeners.delete(listener); },
    scan: () => { const d = deferred<string | null>(); scans.push(d); return d.promise; },
    pairBegin: link => { const d = deferred<{ machineName: string; fingerprint: string }>(); begins.push({ link, d }); return d.promise; },
    pairFinish: name => { const d = deferred<{ machineId: string }>(); finishes.push({ name, d }); return d.promise; },
    pairCancel: vi.fn(() => Promise.resolve()),
    select: vi.fn(async (id: string) => publish({ ...state, active: id })),
    rename: vi.fn(async (id: string, name: string) => publish({ ...state, machines: state.machines.map(m => m.id === id ? { ...m, name } : m) })),
    forget: vi.fn(async (id: string) => { const machines = state.machines.filter(m => m.id !== id); publish({ machines, active: state.active === id ? machines[0]?.id ?? null : state.active }); }),
    transport: vi.fn((id: string) => { const s = transports.get(id) ?? service(); transports.set(id, s); return s.transport; }),
    retry: vi.fn(() => Promise.resolve()),
    relay: { read: vi.fn(async () => ({ url: "https://relay.example.org", isDefault: true })), write: vi.fn(async () => undefined) },
    version: "1.0.6", deviceName: "Pixel 9",
  };
  return { shell, scans, begins, finishes, transports, publish, get state() { return state; },
    add(m: PhoneMachine) { publish({ ...state, machines: [...state.machines, m] }); } };
}
async function mount(shell: PhoneShell) {
  const root = document.createElement("div"); document.body.append(root);
  const app = mountPhone(root, shell);
  disposers.push(app.dispose);
  await settle();
  expect(root.querySelector(".ws-tooltip-host")).toBeNull();
  return app;
}

it("opens the machine Board from the phone drawer and keeps the drawer available", async () => {
  const f = fakeShell({ active: "m1", machines: [machine("m1", "Desk")] });
  await mount(f.shell);
  button("Menu")!.click();
  const board = all(".ws-drawer a.nav").find(a => a.textContent?.trim() === "Board")!;
  expect(board.previousElementSibling?.textContent?.trim()).toBe("Home");
  board.click();
  await settle();
  expect(document.querySelector(".ws-phonebar-t")?.textContent).toBe("Board");
  expect(text()).toContain("No projects on this machine yet");
  expect(document.querySelector(".ws-drawer")).toBeNull();
  button("Menu")!.click();
  expect(all(".ws-drawer a.nav").find(a => a.textContent?.trim() === "Board")?.getAttribute("aria-current")).toBe("page");
});

describe("first run on a phone", () => {
  it("pastes a link, shows the fingerprint, waits for Allow and opens Home on the machine's own transport", async () => {
    const f = fakeShell();
    await mount(f.shell);
    expect(text()).toContain("Connect to your machine");
    expect(text()).not.toMatch(/pairing code/i);
    button("Paste pairing link")!.click(); await settle();
    type("Pairing link", "cortex-pair:abc"); button("Continue")!.click(); await settle();
    expect(f.begins.map(b => b.link)).toEqual(["cortex-pair:abc"]);
    f.begins[0].d.resolve({ machineName: "igor-workstation", fingerprint: "4F2A 91C7 D03E" }); await settle();
    expect(text()).toContain("4F2A 91C7 D03E");
    expect(text()).toContain("Waiting for you to press Allow on the computer. No project data has been sent.");
    expect(f.finishes.map(x => x.name)).toEqual(["igor-workstation"]);
    type("Name for this machine", "Workstation");
    f.add(machine("m1", "igor-workstation"));
    f.finishes[0].d.resolve({ machineId: "m1" }); await settle();
    expect(f.shell.rename).toHaveBeenCalledWith("m1", "Workstation");
    expect(f.shell.select).toHaveBeenCalledWith("m1");
    expect(f.shell.transport).toHaveBeenCalledWith("m1");
    expect(f.transports.get("m1")!.calls.some(c => c.operation === "capabilities.get")).toBe(true);
    expect(document.querySelector(".ws-phonebar")?.textContent).toContain("Home");
    expect(document.documentElement.dataset.shell).toBe("phone");
    expect(all(".ph-screen")).toHaveLength(0);
  });

  for (const [code, where] of [["denied", "finish"], ["expired", "finish"], ["unreachable", "begin"], ["refused", "begin"]] as const) {
    it(`says plainly when pairing ends as ${code}, with a way to try again`, async () => {
      const f = fakeShell();
      await mount(f.shell);
      button("Paste pairing link")!.click(); await settle();
      type("Pairing link", "cortex-pair:abc"); button("Continue")!.click(); await settle();
      if (where === "begin") f.begins[0].d.reject(new Error(code));
      else { f.begins[0].d.resolve({ machineName: "box", fingerprint: "AAAA" }); await settle(); f.finishes[0].d.reject(new Error(code)); }
      await settle();
      expect(document.querySelector('[role="alert"]')?.textContent).toBe(PAIR_WORDS[code]);
      expect(button("Scan QR code")).toBeTruthy();
      expect(button("Paste pairing link")).toBeTruthy();
      expect(f.shell.select).not.toHaveBeenCalled();
    });
  }

  it.each([
    ["unsupported_pairing_link_version", "This pairing link was made by an incompatible version."],
    ["remote_incompatible_version", "The computer and this app are different versions."],
  ])("explains a pairing version refusal (%s) instead of treating it as invalid or offline", async (code, message) => {
    const f = fakeShell();
    await mount(f.shell);
    button("Paste pairing link")!.click(); await settle();
    type("Pairing link", "cortex-pair:abc"); button("Continue")!.click(); await settle();
    f.begins[0].d.reject(new Error(code)); await settle();
    expect(document.querySelector('[role="alert"]')?.textContent).toBe(`${message} Update the app or the computer so both match.`);
    expect(button("Paste pairing link")).toBeTruthy();
    expect(f.finishes).toHaveLength(0);
  });

  it("refuses text that is not a pairing link before anything is sent", async () => {
    const f = fakeShell();
    await mount(f.shell);
    button("Paste pairing link")!.click(); await settle();
    type("Pairing link", "7KQ2-M9XD"); button("Continue")!.click(); await settle();
    expect(f.begins).toHaveLength(0);
    expect(document.querySelector('[role="alert"]')?.textContent).toBe(PAIR_WORDS.invalid_link);
  });

  it("goes back when the scan is cancelled, and offers the link when the camera is denied", async () => {
    const f = fakeShell();
    await mount(f.shell);
    button("Scan QR code")!.click(); await settle();
    expect(document.querySelector('[aria-label="Scan QR code"]')).toBeTruthy();
    f.scans[0].resolve(null); await settle();
    expect(text()).toContain("Connect to your machine");
    expect(document.querySelector('section[aria-label="Scan QR code"]')).toBeNull();
    button("Scan QR code")!.click(); await settle();
    f.scans[1].reject(new Error("camera_denied")); await settle();
    expect(document.querySelector('[role="alert"]')?.textContent).toBe(PAIR_WORDS.camera_denied);
    button("Paste pairing link")!.click(); await settle();
    expect(document.querySelector('[aria-label="Pairing link"]')).toBeTruthy();
  });

  it("scans a code straight into pairing, and Cancel while waiting tells the native side", async () => {
    const f = fakeShell();
    const app = await mount(f.shell);
    button("Scan QR code")!.click(); await settle();
    f.scans[0].resolve("cortex-pair:xyz"); await settle();
    expect(f.begins[0].link).toBe("cortex-pair:xyz");
    f.begins[0].d.resolve({ machineName: "box", fingerprint: "BEEF" }); await settle();
    expect(app.back()).toBe(true); await settle();
    expect(f.shell.pairCancel).toHaveBeenCalledOnce();
    expect(text()).toContain("Connect to your machine");
    expect(app.back()).toBe(false);
  });
});

describe("machines on a phone", () => {
  it("switching tears the workspace down and starts the next machine from its Home", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation"), machine("b", "GPU box", { path: "relay" })] });
    const app = await mount(f.shell);
    const first = f.transports.get("a")!;
    expect(first.calls.length).toBeGreaterThan(0);
    history.replaceState(null, "", "/m/local/archive");
    button("Menu")!.click(); await settle();
    expect(document.querySelector(".ws-drawer")).toBeTruthy();
    expect(document.querySelector(".ws-machine-m")?.textContent).toContain("Connected · direct");
    (document.querySelector(".ws-machine-m") as HTMLElement).click(); await settle();
    expect(document.querySelector(".ws-drawer")).toBeNull();
    const sheet = document.querySelector('.ph-sheet[aria-label="Machines"]')!;
    expect(sheet.textContent).toContain("Connected · through relay");
    const before = first.calls.length;
    [...sheet.querySelectorAll<HTMLElement>(".ph-row")].find(r => r.textContent?.includes("GPU box"))!.click(); await settle();
    expect(f.shell.select).toHaveBeenCalledWith("b");
    expect(f.transports.get("b")!.calls.length).toBeGreaterThan(0);
    // Nothing more is asked of the machine left behind, and the new one starts at Home.
    await new Promise(r => setTimeout(r, 50)); await settle();
    expect(first.calls.length).toBe(before);
    expect(location.pathname).not.toContain("archive");
    expect(document.querySelector(".ws-phonebar")?.textContent).toContain("Home");
    expect(all(".ws-app")).toHaveLength(1);
    expect(app.back()).toBe(false);
  });

  it("forgetting the last machine returns to the connect screen", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation")] });
    await mount(f.shell);
    button("Menu")!.click(); await settle();
    button("Settings")!.click(); await settle();
    expect(text()).toContain("This phone");
    [...all(".ph-row")].find(r => r.textContent?.includes("Workstation"))!.click(); await settle();
    button("Forget this machine…")!.click(); await settle();
    expect(document.querySelector('[role="alertdialog"]')?.textContent).toContain("Nothing on the computer is deleted.");
    button("Forget")!.click(); await settle();
    expect(f.shell.forget).toHaveBeenCalledWith("a");
    expect(all(".ws-app")).toHaveLength(0);
    expect(text()).toContain("Connect to your machine");
  });

  it("shows revocation and update-needed as distinct states in the machine list", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation"), machine("b", "Laptop")] });
    await mount(f.shell);
    f.publish({ active: "a", machines: [
      machine("a", "Workstation", { state: "rejected", path: null }),
      machine("b", "Laptop", { state: "update_needed", path: null }),
    ] });
    await settle();
    const rejected = "This phone is no longer paired with this machine. Pair it again on the machine.";
    expect(document.querySelector(".ws-phone-conn")?.textContent).toContain(rejected);
    button("Menu")!.click(); await settle();
    button("Settings")!.click(); await settle();
    const rows = all(".ph-row").map(row => row.textContent ?? "");
    expect(rows.find(row => row.includes("Workstation"))).toContain(`Not paired${rejected}`);
    expect(rows.find(row => row.includes("Laptop"))).toContain("Update neededThe computer and this app are different versions. Update the app or the computer so both match.");
    expect(rows.some(row => row.includes("Offline"))).toBe(false);
  });

  it("shows the phone's own settings, with app lock only where the phone says it is available", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation", { state: "offline", path: null, lastConnectedAt: Date.now() - 86_400_000 })] });
    const app = await mount(f.shell);
    button("Menu")!.click(); await settle();
    button("Settings")!.click(); await settle();
    expect(text()).toMatch(/Offline · last connected yesterday \d\d:\d\d/);
    expect(text()).toContain("Relay server");
    expect(text()).not.toContain("App lock");
    expect(app.back()).toBe(true); await settle();
    f.shell.appLock = { read: async () => ({ enabled: false, available: true }), write: vi.fn(async () => undefined) };
    button("Menu")!.click(); await settle();
    button("Settings")!.click(); await settle();
    expect(text()).toContain("App lock");
    [...all(".ph-row")].find(r => r.textContent?.includes("Relay server"))!.click(); await settle();
    type("Relay server address", "http://plain.example"); button("Save")!.click(); await settle();
    expect(document.querySelector(".ph-err")?.textContent).toContain("https://");
    expect(f.shell.relay!.write).not.toHaveBeenCalled();
    type("Relay server address", "https://relay.mine.example"); button("Save")!.click(); await settle();
    expect(f.shell.relay!.write).toHaveBeenCalledWith("https://relay.mine.example");
  });
  it("turns its own notifications on once a machine is paired, off from Settings, and a tap opens its machine", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation"), machine("b", "Laptop")] });
    let tap: ((tap: PhoneTap) => void) | null = null;
    const set = vi.fn(async (enabled: boolean, _words: PhoneNotificationWords) => ({ enabled, permission: "granted" as const, batteryExempt: false }));
    f.shell.notifications = { set, battery: vi.fn(async () => undefined), taps: listener => { tap = listener; return () => { tap = null; }; } };
    await mount(f.shell);
    expect(set.mock.calls.map(c => c[0])).toEqual([true]);
    // Android shows the words in the interface's language; this test runs in English.
    expect(set.mock.calls[0][1]).toMatchObject({ ongoingTitle: "Connected to your computers", kinds: { failed: "Task failed" }, channels: { answer: { name: "Needs your answer" } } });
    expect(notificationWords(key => workspaceCopy(key, "ru-RU"))).toMatchObject({ ongoingTitle: "На связи с компьютерами",
      kinds: { question: "Нужен ваш ответ", result: "Задача завершена", failed: "Сбой задачи" } });
    // A connection change is not a new wish: nothing is applied again.
    f.publish({ ...f.state, machines: [machine("a", "Workstation", { state: "offline", path: null }), machine("b", "Laptop")] }); await settle();
    expect(set).toHaveBeenCalledTimes(1);
    button("Menu")!.click(); await settle();
    button("Settings")!.click(); await settle();
    expect(text()).toContain("Phone notifications");
    expect(text()).toContain("without battery optimisation");
    button("Battery settings")!.click(); await settle();
    expect(f.shell.notifications.battery).toHaveBeenCalled();
    const toggle = document.querySelector<HTMLInputElement>('[aria-label="Phone notifications"]')!;
    expect(toggle.checked).toBe(true);
    toggle.checked = false; toggle.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(set.mock.calls.at(-1)?.[0]).toBe(false);
    expect(localStorage.getItem("cw.ws.notifications")).toBe("false");
    // A tap on another machine's notification closes Settings and opens that machine.
    tap!({ machineId: "b", target: { kind: "task", project: "p", task: "t" } }); await settle();
    expect(f.shell.select).toHaveBeenCalledWith("b");
    expect(all(".ph-screen")).toHaveLength(0);
  });
});

describe("the phone shell of the shared interface", () => {
  it("has no tab strip, window controls, terminal, editor save or shortcut hints", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation")] });
    await mount(f.shell);
    expect(document.querySelector(".ws-tabstrip")).toBeNull();
    expect(document.querySelector(".win")).toBeNull();
    expect(document.querySelector('[aria-label="Terminal"]')).toBeNull();
    button("Menu")!.click(); await settle();
    expect(all(".kbd").map(k => k.textContent)).toEqual([]);
    expect(button("Help and shortcuts")).toBeUndefined();
    expect(document.querySelector(".ws-resize")).toBeNull();
  });

  it("keeps Code a viewer: no editor or save, no terminal, folder chooser or saving to disk, whatever the machine offers", async () => {
    const { PHONE_WITHHELD } = await import("../src/workspace");
    const { createWorkspaceStore } = await import("../src/store");
    const picked = vi.fn(async () => "/somewhere");
    const make = (withhold?: readonly string[]) => createRoot(dispose => { disposers.push(dispose); const p = scriptedPort({ capabilities: { everything: true, operations: [] } }); return createWorkspaceStore({ ...p.port, codeLease: { acquire: vi.fn(), renew: vi.fn(), release: vi.fn(), save: vi.fn() }, codeLanguage: { open: vi.fn(), change: vi.fn(), status: vi.fn(), definition: vi.fn(), close: vi.fn(), subscribe: vi.fn() }, pickFolder: picked }, { withhold }); });
    const desktop = make(), phone = make(PHONE_WITHHELD);
    await settle();
    expect(desktop.canEdit()).toBe(true);
    expect(desktop.can("workspace.terminal")).toBe(true);
    expect(phone.canEdit()).toBe(false);
    expect(desktop.codeLanguage()).toBeDefined();
    expect(phone.codeLanguage()).toBeUndefined();
    expect(phone.can("workspace.terminal")).toBe(false);
    expect(phone.canSaveAttachment()).toBe(false);
    expect(await phone.pickFolder()).toBeNull();
    expect(picked).not.toHaveBeenCalled();
  });

  it("shows the reconnecting band with the time of the last read, turns writes off, and clears on reconnect", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation")] });
    await mount(f.shell);
    const s = f.transports.get("a")!;
    s.send({ type: "connection", state: "disconnected" }); await settle();
    const band = document.querySelector(".ws-phone-conn")!;
    expect(band.textContent).toMatch(/^Reconnecting to Workstation…( Shown as of \d\d:\d\d\.)?Retry$/);
    const add = button("Add project") ?? (button("Menu")!.click(), await settle(), button("Add project"));
    expect(add!.getAttribute("aria-disabled")).toBe("true");
    expect(add!.dataset.why).toMatch(/Not connected/);
    button("Retry")!.click(); await settle();
    expect(f.shell.retry).toHaveBeenCalledWith("a");
    s.send({ type: "connection", state: "connected" }); await settle();
    expect(document.querySelector(".ws-phone-conn")).toBeNull();
  });

  it("Back closes the drawer first, then goes back, then says there is nothing left", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation")] });
    const app = await mount(f.shell);
    expect(location.pathname).toMatch(/\/m\/[^/]+$/);
    button("Menu")!.click(); await settle();
    [...all(".ws-drawer a.nav")].find(a => a.textContent?.includes("Archive"))!.click(); await settle();
    expect(document.querySelector(".ws-drawer")).toBeNull();
    expect(location.pathname).toMatch(/archive$/);
    // The machine sheet over the drawer goes first, then the drawer, then the page.
    button("Menu")!.click(); await settle();
    (document.querySelector(".ws-machine-m") as HTMLElement).click(); await settle();
    button("Menu")!.click(); await settle();
    expect(document.querySelector(".ph-sheet")).toBeTruthy();
    expect(app.back()).toBe(true); await settle();
    expect(document.querySelector(".ph-sheet")).toBeNull();
    expect(document.querySelector(".ws-drawer")).toBeTruthy();
    expect(app.back()).toBe(true); await settle();
    expect(document.querySelector(".ws-drawer")).toBeNull();
    expect(location.pathname).toMatch(/archive$/);
    expect(app.back()).toBe(true); await settle();
    expect(location.pathname).toMatch(/\/m\/[^/]+$/);
    expect(app.back()).toBe(false);
  });

  it("Back closes an open dialog before it leaves the page", async () => {
    const f = fakeShell({ active: "a", machines: [machine("a", "Workstation")] });
    const app = await mount(f.shell);
    button("Menu")!.click(); await settle();
    [...all(".ws-drawer a.nav")].find(a => a.textContent?.includes("Archive"))!.click(); await settle();
    const dialog = document.createElement("dialog");
    const cancelled = vi.fn((e: Event) => { e.preventDefault(); dialog.removeAttribute("open"); });
    dialog.addEventListener("cancel", cancelled); dialog.setAttribute("open", ""); document.body.append(dialog);
    expect(app.back()).toBe(true);
    expect(cancelled).toHaveBeenCalledOnce();
    expect(location.pathname).toMatch(/archive$/);
    expect(app.back()).toBe(true); await settle();
    expect(location.pathname).not.toMatch(/archive$/);
  });
});

describe("the desktop shell is unchanged by default", () => {
  it("renders the tab strip and never marks the root as a phone", async () => {
    const s = service();
    const root = document.createElement("div"); document.body.append(root);
    disposers.push(render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} />, root));
    await settle();
    expect(document.querySelector(".ws-tabstrip")).toBeTruthy();
    expect(document.documentElement.dataset.shell).toBeUndefined();
    expect(document.querySelector("[data-shell]")).toBeNull();
    expect(document.querySelector(".ws-phonebar")).toBeNull();
    expect(all(".kbd").map(k => k.textContent)).toEqual(expect.arrayContaining(["Ctrl+N", "Ctrl+K"]));
  });
});
