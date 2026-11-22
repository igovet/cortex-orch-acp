// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountPhone, Workspace, type PhoneShell, type PhoneState } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport } from "./support";

/** A sudo password prompt of a task's host command, on the desktop and on the phone, against the real port. */
let disposers: (() => void)[] = [];
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { for (const d of disposers.splice(0)) d(); document.body.innerHTML = ""; delete document.documentElement.dataset.shell; });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const card = () => document.querySelector<HTMLElement>('[role="dialog"][aria-label="Password for sudo"]');
const field = () => document.querySelector<HTMLInputElement>('input[aria-label="Sudo password"]');
const button = (text: string) => [...(card()?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(b => b.textContent?.trim() === text);
const SECRET = "fixture-typed-secret";

const PROMPT = {
  promptId: "a1b2c3", title: "Password for sudo", prompt: "[sudo] password for owner:", command: "sudo -A apt-get install make",
  cwd: "/home/owner/project", taskId: "t1", task: "Install the build tools", worker: "worker-backend-dev",
  projectId: "p1", projectName: "Cedar", openedAt: Date.now(), expiresAt: Date.now() + 600_000,
};
function machine(phoneAnswers = true) {
  let open = [PROMPT];
  return scriptedTransport({
    "service.capabilities": () => ({ methods: ["projects.list", "askpass.list", "askpass.answer"] }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire([]),
    "askpass.list": () => ({ prompts: open, phoneAnswers }),
    "askpass.answer": () => { open = []; return { outcome: "answered" }; },
  });
}
const answers = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "askpass.answer").map(c => c.message);

async function desktop(s: ReturnType<typeof machine>) {
  const root = document.createElement("div"); document.body.append(root);
  disposers.push(render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} />, root));
  await settle();
}
async function phone(s: ReturnType<typeof machine>) {
  const state: PhoneState = { active: "m1", machines: [{ id: "m1", name: "Workstation", state: "connected", path: "direct", lastConnectedAt: null }] };
  const shell: PhoneShell = {
    state: () => Promise.resolve(state), subscribe: () => () => {},
    scan: () => Promise.resolve(null), pairBegin: () => new Promise(() => {}), pairFinish: () => new Promise(() => {}), pairCancel: vi.fn(() => Promise.resolve()),
    select: vi.fn(async () => {}), rename: vi.fn(async () => {}), forget: vi.fn(async () => {}),
    transport: vi.fn(() => s.transport), retry: vi.fn(() => Promise.resolve()),
    relay: { read: vi.fn(async () => ({ url: null, isDefault: true })), write: vi.fn(async () => undefined) },
    version: "1.0.6", deviceName: "Pixel 9",
  };
  const root = document.createElement("div"); document.body.append(root);
  disposers.push(mountPhone(root, shell).dispose);
  await settle();
}

describe("sudo password prompt", () => {
  it("on the desktop: names the command, masks the field, sends the password once and keeps nothing", async () => {
    const s = machine();
    await desktop(s);
    expect(card()).toBeTruthy();
    expect(card()!.textContent).toContain("sudo -A apt-get install make");
    expect(card()!.textContent).toContain("Cedar · Install the build tools · worker-backend-dev");
    expect(card()!.textContent).toContain("not saved or shown to the model");
    expect(field()!.type).toBe("password");
    expect(field()!.autocomplete).toBe("off");
    field()!.value = SECRET;
    button("Submit")!.click(); await settle();
    expect(answers(s)).toEqual([{ promptId: "a1b2c3", password: SECRET }]);
    expect(card()).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(JSON.stringify({ ...localStorage })).not.toContain(SECRET);
    expect(JSON.stringify({ ...sessionStorage })).not.toContain(SECRET);
  });

  it("on the desktop: Cancel sends a cancel and no password", async () => {
    const s = machine();
    await desktop(s);
    field()!.value = SECRET;
    button("Cancel")!.click(); await settle();
    expect(answers(s)).toEqual([{ promptId: "a1b2c3", cancel: true }]);
    expect(card()).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
  });

  it("on the phone: the same card answers over the machine's transport", async () => {
    const s = machine();
    await phone(s);
    expect(document.documentElement.dataset.shell).toBe("phone");
    expect(document.querySelector(".sudo-stack")?.getAttribute("data-shell")).toBe("phone");
    field()!.value = SECRET;
    button("Submit")!.click(); await settle();
    expect(answers(s)).toEqual([{ promptId: "a1b2c3", password: SECRET }]);
  });

  it("on the phone: when the owner turned phone passwords off, only Cancel is offered", async () => {
    const s = machine(false);
    await phone(s);
    expect(card()!.textContent).toContain("Passwords and tokens from phones are turned off on the computer");
    expect(field()).toBeNull();
    expect(button("Submit")).toBeUndefined();
    button("Cancel")!.click(); await settle();
    expect(answers(s)).toEqual([{ promptId: "a1b2c3", cancel: true }]);
  });

  it("shows nothing for a service without sudo prompts", async () => {
    const s = scriptedTransport({
      "service.capabilities": () => ({ methods: ["projects.list"] }),
      "projects.list": () => [],
      "capabilities.get": () => capabilitiesWire([]),
    });
    const root = document.createElement("div"); document.body.append(root);
    disposers.push(render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} />, root));
    await settle();
    expect(card()).toBeNull();
    expect(s.calls.some(c => c.operation === "askpass.list")).toBe(false);
  });
});
