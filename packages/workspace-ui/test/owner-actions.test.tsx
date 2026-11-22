// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountPhone, Workspace, type PhoneShell, type PhoneState } from "../src";
import { createRealPort } from "../src/port/real";
import { queuedTerminalCommand, queueTerminalCommand, takeTerminalCommand } from "../src/terminal-command";
import type { TerminalPort } from "../src/terminal-client";
import { emptyWorkspace, LOCAL_MACHINE_ID, readOwnerActionReply, readOwnerActions } from "@cortex/workspace-domain";
import { capabilitiesWire, flush, projection, scriptedPort, scriptedTransport } from "./support";

const xterm = vi.hoisted(() => ({ data: (_data: string) => {} }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  options: any = {}; cols = 80; rows = 24;
  open() {} reset() {} loadAddon() {} focus() {}
  write(_bytes: Uint8Array, done: () => void) { done(); }
  onData(fn: (data: string) => void) { xterm.data = fn; }
  dispose() {}
} }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));

/** Access a task lacks (a sign-in, sudo), on the desktop and on the phone, against the real port. */
let disposers: (() => void)[] = [];
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/");
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  while (takeTerminalCommand("p1") !== undefined || takeTerminalCommand("p") !== undefined) { /* drain */ }
});
afterEach(() => { for (const d of disposers.splice(0)) d(); document.body.innerHTML = ""; delete document.documentElement.dataset.shell; vi.unstubAllGlobals(); });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const card = (title: string) => document.querySelector<HTMLElement>(`[role="dialog"][aria-label="${title}"]`);
const button = (title: string, text: string) => [...(card(title)?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(b => b.textContent?.trim() === text);
const TOKEN = "ghp_fixtureTokenNotReal0000";

const base = { projectId: "p1", projectName: "Cedar", taskId: "t1", task: "Publish the release", kind: "access", recheck: true, openedAt: Date.now(), checkedAt: null, lastStatus: "not_logged_in" };
const GITHUB = { ...base, actionId: "oa_gh1", service: "github", title: "Sign in to GitHub", detail: "The task needs the GitHub CLI signed in.",
  terminal: { command: "gh auth login", argv: ["gh", "auth", "login"] }, token: { label: "GitHub token", hint: "A personal access token (classic)." } };
const CODEX = { ...base, actionId: "oa_cx1", service: "codex", title: "Sign in to Codex", detail: "Codex is not signed in.",
  terminal: { command: "codex login", argv: ["codex", "login"] }, token: null };
const CLAUDE = { ...base, actionId: "oa_cl1", service: "claude", title: "Sign in to Claude", detail: "Claude is not signed in.",
  terminal: { command: "claude auth login", argv: ["claude", "auth", "login"] }, token: null };
const SUDO = { ...base, actionId: "oa_su1", service: "sudo", title: "sudo needs your password", detail: "A command needs root.", terminal: null, token: null };

function machine(actions: unknown[], phoneTokens = true, acts: Record<string, (m: any) => unknown> = {}) {
  let open = [...actions] as { actionId: string }[];
  return scriptedTransport({
    "service.capabilities": () => ({ methods: ["projects.list", "owner_actions.list", "owner_actions.act"] }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire([]),
    "owner_actions.list": () => ({ actions: open, phoneTokens }),
    "owner_actions.act": (m: any) => {
      const custom = acts[m.action];
      if (custom) { const reply = custom(m) as { outcome?: string }; if (reply?.outcome === "resolved") open = open.filter(x => x.actionId !== m.actionId); return reply; }
      if (m.action === "terminal") return { outcome: "terminal", command: "gh auth login", argv: ["gh", "auth", "login"] };
      open = open.filter(a => a.actionId !== m.actionId);
      return { outcome: "resolved", how: m.action === "token" ? "token" : m.action === "dismiss" ? "dismissed" : "recheck" };
    },
  });
}
const acts = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "owner_actions.act").map(c => c.message);

const stubTerminal = (): TerminalPort => ({
  open: vi.fn(async () => ({ sessionId: "s", data: [], next: 0, truncated: false, exited: false })),
  read: vi.fn(async () => ({ sessionId: "s", data: [], next: 0, truncated: false, exited: false })),
  write: vi.fn(async (_p: string, _s: string, data: string) => ({ written: new TextEncoder().encode(data).length })),
  resize: vi.fn(async () => ({})), close: vi.fn(async () => ({})),
});
async function desktop(s: ReturnType<typeof machine>, extra: { notify?: any; terminal?: TerminalPort | null } = {}) {
  const root = document.createElement("div"); document.body.append(root);
  disposers.push(render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} notify={extra.notify} terminal={extra.terminal === null ? undefined : extra.terminal ?? stubTerminal()} />, root));
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

describe("owner action cards", () => {
  it("on the desktop: offers per service only what the machine supports", async () => {
    await desktop(machine([GITHUB, CODEX, CLAUDE, SUDO]));
    const gh = card("Sign in to GitHub")!;
    expect(gh.textContent).toContain("Cedar · Publish the release");
    expect(gh.textContent).toContain("The task needs the GitHub CLI signed in.");
    expect(button("Sign in to GitHub", "Open sign-in in terminal")).toBeTruthy();
    expect(button("Sign in to GitHub", "I've signed in, check again")).toBeTruthy();
    expect(button("Sign in to GitHub", "Sign in with token")).toBeTruthy();
    expect(button("Sign in to GitHub", "Dismiss")).toBeTruthy();
    const field = gh.querySelector<HTMLInputElement>('input[aria-label="GitHub token"]')!;
    expect(field.type).toBe("password");
    expect(field.autocomplete).toBe("off");
    for (const title of ["Sign in to Codex", "Sign in to Claude"]) {
      expect(button(title, "Open sign-in in terminal")).toBeTruthy();
      expect(button(title, "I've signed in, check again")).toBeTruthy();
      expect(button(title, "Sign in with token")).toBeUndefined();
      expect(card(title)!.querySelector("input")).toBeNull();
    }
    const sudo = card("sudo needs your password")!;
    expect(sudo.textContent).toContain("Password for sudo prompt opens here");
    expect(sudo.querySelector("input")).toBeNull();
    expect(button("sudo needs your password", "Open sign-in in terminal")).toBeUndefined();
    expect(button("sudo needs your password", "Check again")).toBeTruthy();
  });

  it("on the desktop: the terminal sign-in marks the action, opens the task's terminal and queues the command", async () => {
    const s = machine([GITHUB]);
    await desktop(s);
    button("Sign in to GitHub", "Open sign-in in terminal")!.click(); await settle();
    expect(acts(s)).toEqual([{ projectId: "p1", actionId: "oa_gh1", action: "terminal" }]);
    expect(location.pathname).toContain("/t/t1");
    expect(localStorage.getItem("cw.ws.terminalOpen")).toBe("true");
    expect(queuedTerminalCommand("p1")).toBe("gh auth login");
    expect(card("Sign in to GitHub")!.textContent).toContain("Finish it there, then check again");
  });

  it("on the desktop without a terminal: shows the command instead of the terminal button", async () => {
    await desktop(machine([CLAUDE]), { terminal: null });
    expect(button("Sign in to Claude", "Open sign-in in terminal")).toBeUndefined();
    expect(card("Sign in to Claude")!.querySelector(".owner-command")!.textContent).toBe("claude auth login");
  });

  it("on the desktop: a pasted token is sent once, cleared at once and kept nowhere", async () => {
    const s = machine([GITHUB]);
    await desktop(s);
    const field = card("Sign in to GitHub")!.querySelector<HTMLInputElement>("input")!;
    field.value = TOKEN;
    button("Sign in to GitHub", "Sign in with token")!.click();
    expect(field.value).toBe("");
    await settle();
    expect(acts(s)).toEqual([{ projectId: "p1", actionId: "oa_gh1", action: "token", token: TOKEN }]);
    expect(card("Sign in to GitHub")).toBeNull();
    expect(document.body.innerHTML).not.toContain(TOKEN);
    expect(JSON.stringify({ ...localStorage })).not.toContain(TOKEN);
    expect(JSON.stringify({ ...sessionStorage })).not.toContain(TOKEN);
  });

  it("on the desktop: check again closes a resolved card and says so when access is still missing", async () => {
    let missing = true;
    const s = machine([CODEX], true, { recheck: () => missing ? { outcome: "still_needed", status: "not_logged_in" } : { outcome: "resolved", how: "recheck" } });
    await desktop(s);
    button("Sign in to Codex", "I've signed in, check again")!.click(); await settle();
    expect(card("Sign in to Codex")!.textContent).toContain("Still not signed in");
    missing = false;
    button("Sign in to Codex", "I've signed in, check again")!.click(); await settle();
    expect(acts(s).map((m: any) => m.action)).toEqual(["recheck", "recheck"]);
    expect(card("Sign in to Codex")).toBeNull();
  });

  it("on the desktop: a refusal comes back in words", async () => {
    const s = machine([GITHUB], true, { token: () => { throw new Error("owner_token_refused"); } });
    await desktop(s);
    card("Sign in to GitHub")!.querySelector<HTMLInputElement>("input")!.value = TOKEN;
    button("Sign in to GitHub", "Sign in with token")!.click(); await settle();
    expect(card("Sign in to GitHub")!.querySelector('[role="alert"]')!.textContent).toContain("did not accept this token");
    expect(document.body.innerHTML).not.toContain(TOKEN);
  });

  it("announces a new action once through the window's notifications", async () => {
    const notify = vi.fn(async () => {});
    await desktop(machine([GITHUB]), { notify });
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledWith({ label: "Sign in to GitHub: Publish the release", target: { kind: "task", project: "p1", task: "t1" } });
    for (const d of disposers.splice(0)) d();
    document.body.innerHTML = "";
    await desktop(machine([GITHUB]), { notify });
    expect(notify).toHaveBeenCalledTimes(1);
  });

  it("on the phone: shows the command to run on the computer, no terminal button, and a token field", async () => {
    const s = machine([GITHUB, CODEX]);
    await phone(s);
    expect(document.querySelector(".owner-stack")?.getAttribute("data-shell")).toBe("phone");
    expect(card("Sign in to GitHub")!.querySelector(".owner-command")!.textContent).toBe("gh auth login");
    expect(card("Sign in to Codex")!.querySelector(".owner-command")!.textContent).toBe("codex login");
    expect(card("Sign in to GitHub")!.textContent).toContain("Run this in a terminal on the computer");
    expect(button("Sign in to GitHub", "Open sign-in in terminal")).toBeUndefined();
    const field = card("Sign in to GitHub")!.querySelector<HTMLInputElement>("input")!;
    expect(field.type).toBe("password");
    field.value = TOKEN;
    button("Sign in to GitHub", "Sign in with token")!.click(); await settle();
    expect(acts(s)).toEqual([{ projectId: "p1", actionId: "oa_gh1", action: "token", token: TOKEN }]);
    expect(document.body.innerHTML).not.toContain(TOKEN);
  });

  it("on the phone: with phone tokens off there is no token field, only the note", async () => {
    await phone(machine([GITHUB], false));
    expect(card("Sign in to GitHub")!.textContent).toContain("Passwords and tokens from phones are turned off on the computer");
    expect(card("Sign in to GitHub")!.querySelector("input")).toBeNull();
    expect(button("Sign in to GitHub", "Sign in with token")).toBeUndefined();
    expect(button("Sign in to GitHub", "I've signed in, check again")).toBeTruthy();
  });

  it("shows nothing for a service without owner actions", async () => {
    const s = scriptedTransport({
      "service.capabilities": () => ({ methods: ["projects.list"] }),
      "projects.list": () => [],
      "capabilities.get": () => capabilitiesWire([]),
    });
    const root = document.createElement("div"); document.body.append(root);
    disposers.push(render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} />, root));
    await settle();
    expect(document.querySelector(".owner-stack")).toBeNull();
    expect(s.calls.some(c => c.operation === "owner_actions.list")).toBe(false);
  });
});

describe("a queued terminal command", () => {
  it("is typed into that project's shell once it runs, then forgotten", async () => {
    const terminal = stubTerminal();
    history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/board`);
    const backend = scriptedPort();
    const root = document.createElement("div"); document.body.append(root);
    disposers.push(render(() => <Workspace port={backend.port} terminal={terminal} />, root));
    const state = emptyWorkspace();
    state.projects.push({ id: "p", machineId: LOCAL_MACHINE_ID, name: "Project", repository: "/project", openBranch: "main", defaultBranch: "main" });
    await flush(); await flush();
    backend.emit(projection("1", state));
    await flush(); await flush();
    queueTerminalCommand("p", "gh auth login");
    queueTerminalCommand("p", "two\nlines");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "j", ctrlKey: true, bubbles: true, cancelable: true }));
    await settle();
    expect(terminal.open).toHaveBeenCalledWith("p", 80, 24);
    expect(vi.mocked(terminal.write).mock.calls.map(c => c[2])).toEqual(["gh auth login\r"]);
    expect(queuedTerminalCommand("p")).toBeUndefined();
    xterm.data("x"); await settle();
    expect(vi.mocked(terminal.write).mock.calls.map(c => c[2])).toEqual(["gh auth login\r", "x"]);
  });
});

describe("owner action wire readers", () => {
  it("reads exact shapes and refuses malformed ones", () => {
    const read = readOwnerActions({ actions: [GITHUB, SUDO], phoneTokens: false });
    expect(read.phoneTokens).toBe(false);
    expect(read.actions[0]).toMatchObject({ actionId: "oa_gh1", service: "github", terminal: { command: "gh auth login" }, token: { label: "GitHub token" } });
    expect(read.actions[1]).toMatchObject({ terminal: null, token: null, checkedAt: null });
    for (const bad of [null, {}, { actions: [], phoneTokens: "yes" }, { actions: [{ ...GITHUB, actionId: "has space" }], phoneTokens: true },
      { actions: [{ ...GITHUB, kind: "other" }], phoneTokens: true }, { actions: [{ ...GITHUB, terminal: { command: "a\nb", argv: ["a"] } }], phoneTokens: true },
      { actions: [{ ...GITHUB, actionId: "x".repeat(97) }], phoneTokens: true }])
      expect(() => readOwnerActions(bad)).toThrow();
    expect(readOwnerActionReply({ outcome: "resolved", how: "token" })).toEqual({ outcome: "resolved", how: "token" });
    expect(readOwnerActionReply({ outcome: "still_needed", status: "not_logged_in" })).toEqual({ outcome: "still_needed", status: "not_logged_in" });
    expect(() => readOwnerActionReply({ outcome: "resolved", how: "magic" })).toThrow();
    expect(() => readOwnerActionReply({ outcome: "terminal", command: "rm\n", argv: [] })).toThrow();
  });
});
