import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Project, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { draftTask, flush, projection, scriptedPort } from "./support";

const HOME = `/m/${LOCAL_MACHINE_ID}`, NEW = `${HOME}/new`, TASK = `${HOME}/t/t`, INBOX = `${HOME}/inbox`;
const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
const state = (patch: Partial<Task> = {}): WorkspaceState => {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  s.projects.push(project);
  s.tasks.push({ ...draftTask(), ...patch });
  return s;
};

let dispose: (() => void) | undefined;
const unmount = () => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; };
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(unmount);

async function mount(path = HOME, port = scriptedPort().port) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port} initial={{ state: state(), version: "1", capabilities: { everything: true, operations: [] } }} />, root);
  await flush(); await flush();
}
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const tabs = () => all('[role="tab"].ws-tab-name');
const titles = () => tabs().map(t => t.textContent);
const shown = () => $('.ws-pane:not([data-parked])')!;
const where = () => location.pathname + location.search;
const press = (init: KeyboardEventInit) => document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ctrlKey: true, ...init }));
const newTab = async () => { press({ key: "t", code: "KeyT" }); await flush(); await flush(); };
/** The way any address arrives from outside: it opens in the active tab. */
const open = async (path: string) => { history.pushState(null, "", path); window.dispatchEvent(new PopStateEvent("popstate")); await flush(); await flush(); };
const describeField = () => $<HTMLTextAreaElement>('textarea[aria-label="Describe the task"]')!;
const typeInto = (field: HTMLTextAreaElement, value: string) => { field.value = value; field.dispatchEvent(new InputEvent("input", { bubbles: true })); };

describe("browser tabs", () => {
  it("names each tab from its screen and adds one with +", async () => {
    await mount(NEW);
    expect(titles()).toEqual(["New task"]);
    $('button[aria-label="New tab"]')!.click(); await flush(); await flush();
    expect(titles()).toEqual(["New task", "Home"]);
    expect(where()).toBe(HOME);
  });

  it("a sidebar link changes only the active tab", async () => {
    await mount(HOME);
    await newTab();
    $<HTMLAnchorElement>(`nav.sidebar a.nav[href="${INBOX}"]`)!.click(); await flush(); await flush();
    expect(titles()).toEqual(["Home", "Inbox"]);
    expect(where()).toBe(INBOX);
    tabs()[0].click(); await flush();
    expect(where()).toBe(HOME);
    expect(titles()).toEqual(["Home", "Inbox"]);
  });

  it("keeps a new-task draft, its field and its focus across a tab switch", async () => {
    await mount(NEW);
    const field = describeField();
    field.focus();
    typeInto(field, "half written task");
    await newTab();
    await open(TASK);
    expect(shown().querySelector('textarea[aria-label="Describe the task"]')).toBeNull();
    expect(field.closest(".ws-pane")!.hasAttribute("data-parked")).toBe(true);
    expect(field.isConnected).toBe(true);
    tabs()[0].click(); await flush();
    expect(describeField()).toBe(field);
    expect(field.value).toBe("half written task");
    expect(document.activeElement).toBe(field);
    expect(where()).toBe(NEW);
  });

  it("Ctrl+Tab cycles, Ctrl+Shift+Tab goes back, Ctrl+1 and Ctrl+9 jump", async () => {
    await mount(HOME);
    await newTab(); await newTab();
    const active = () => tabs().findIndex(t => t.getAttribute("aria-selected") === "true");
    expect(active()).toBe(2);
    press({ key: "Tab" }); await flush();
    expect(active()).toBe(0);
    press({ key: "Tab", shiftKey: true }); await flush();
    expect(active()).toBe(2);
    press({ key: "1" }); await flush();
    expect(active()).toBe(0);
    press({ key: "9" }); await flush();
    expect(active()).toBe(2);
  });

  it("leaves Ctrl+T and Ctrl+W to a focused terminal, and keeps Ctrl+Tab", async () => {
    await mount(HOME);
    await newTab();
    const shell = document.createElement("div");
    shell.className = "terminal";
    const input = shell.appendChild(document.createElement("textarea"));
    $("main")!.append(shell);
    input.focus();
    const t = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ctrlKey: true, key: "t", code: "KeyT" });
    input.dispatchEvent(t); await flush();
    expect(tabs()).toHaveLength(2);
    expect(t.defaultPrevented).toBe(false);
    input.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ctrlKey: true, key: "w", code: "KeyW" })); await flush();
    expect(tabs()).toHaveLength(2);
    input.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ctrlKey: true, key: "Tab" })); await flush();
    expect(tabs()[0].getAttribute("aria-selected")).toBe("true");
  });

  it("closes with the x, Ctrl+W and a middle click, and the last close leaves a fresh tab", async () => {
    await mount(HOME);
    await newTab(); await newTab();
    $<HTMLButtonElement>(".ws-tab-x")!.click(); await flush();
    expect(tabs()).toHaveLength(2);
    press({ key: "w", code: "KeyW" }); await flush();
    expect(tabs()).toHaveLength(1);
    $(".ws-tab")!.dispatchEvent(new MouseEvent("auxclick", { bubbles: true, button: 1 })); await flush(); await flush();
    expect(titles()).toEqual(["Home"]);
    expect(all(".ws-pane")).toHaveLength(1);
    expect(where()).toBe(HOME);
  });

  it("drag reorders the strip without moving the panes", async () => {
    await mount(NEW);
    await newTab();
    await open(INBOX);
    const panes = all(".ws-pane");
    const [first, , ] = all(".ws-tab");
    first.dispatchEvent(new Event("dragstart", { bubbles: true }));
    all(".ws-tab")[1].dispatchEvent(new Event("drop", { bubbles: true, cancelable: true })); await flush();
    expect(titles()).toEqual(["Inbox", "New task"]);
    expect(all(".ws-pane")).toEqual(panes);
  });

  it("asks before closing a tab with an unsent draft, and only then closes", async () => {
    await mount(NEW);
    typeInto(describeField(), "do not lose me");
    await newTab();
    $<HTMLButtonElement>('button[aria-label="Close New task"]')!.click(); await flush();
    const dialog = $("dialog[open]")!;
    expect(dialog.textContent).toContain("Close New task?");
    [...dialog.querySelectorAll("button")].find(b => b.textContent === "Cancel")!.click(); await flush();
    expect(titles()).toEqual(["New task", "Home"]);
    expect(describeField().value).toBe("do not lose me");
    $<HTMLButtonElement>('button[aria-label="Close New task"]')!.click(); await flush();
    [...$("dialog[open]")!.querySelectorAll("button")].find(b => b.textContent === "Close tab")!.click(); await flush(); await flush();
    expect(titles()).toEqual(["Home"]);
  });

  it("gives each tab its own Back and Forward", async () => {
    await mount(HOME);
    await open(INBOX);
    await newTab();
    expect($<HTMLButtonElement>('button[aria-label="Back"]')!.classList.contains("ws-hide")).toBe(true);
    await open(NEW);
    $<HTMLButtonElement>('button[aria-label="Back"]')!.click(); await flush(); await flush();
    expect(where()).toBe(HOME);
    tabs()[0].click(); await flush();
    expect(where()).toBe(INBOX);
    $<HTMLButtonElement>('button[aria-label="Back"]')!.click(); await flush(); await flush();
    expect(where()).toBe(HOME);
    $<HTMLButtonElement>('button[aria-label="Forward"]')!.click(); await flush(); await flush();
    expect(where()).toBe(INBOX);
  });

  it("restores the tabs after a restart, opening only the active one", async () => {
    await mount(NEW);
    await newTab();
    await open(INBOX);
    tabs()[0].click(); await flush();
    tabs()[1].click(); await flush();
    unmount();
    await mount("/");
    expect(titles()).toEqual(["New task", "Inbox"]);
    expect(where()).toBe(INBOX);
    expect($('.ws-pane[data-parked]')!.children).toHaveLength(0);
  });

  it("opens a deep link in the active tab", async () => {
    await mount(NEW);
    await newTab();
    unmount();
    await mount(INBOX);
    expect(titles()).toEqual(["New task", "Inbox"]);
  });

  it("a parked tab keeps its ids out of the document, so menus resolve to the tab on view", async () => {
    await mount(NEW);
    await newTab();
    await open(NEW);
    const ids = all("[id]").map(e => e.id).filter(id => id.startsWith("m-"));
    expect(ids.length).toBeGreaterThan(0);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) expect(document.getElementById(id)?.closest(".ws-pane[data-parked]") ?? null).toBeNull();
    tabs()[0].click(); await flush();
    expect(document.getElementById(ids[0])?.closest(".ws-pane")).toBe(shown());
  });

  it("ten tabs add no timers, and a parked tab reads the machine only when it is back on view", async () => {
    const script = scriptedPort();
    const live = new Set<unknown>();
    const { setInterval: set, clearInterval: clear } = globalThis;
    globalThis.setInterval = ((f: () => void, ms?: number) => { const id = set(f, ms); if ((ms ?? 0) >= 100) live.add(id); return id; }) as typeof setInterval; // jsdom's rAF is a 16 ms interval.
    globalThis.clearInterval = ((id: ReturnType<typeof setInterval>) => { live.delete(id); clear(id); }) as typeof clearInterval;
    try {
      await mount(TASK, script.port);
      const baseline = live.size;
      expect(baseline).toBeGreaterThan(0);
      for (let i = 0; i < 9; i++) { await newTab(); await open(TASK); }
      expect(tabs()).toHaveLength(10);
      tabs()[0].click(); await flush();
      expect(live.size).toBe(baseline);
      expect(tabs()[0].textContent).toBe("Fix the race");
      const parked = all('.ws-pane[data-parked]')[0];
      const before = parked.textContent;
      script.emit(projection("2", state({ description: "Words that arrived later", title: "Renamed" })));
      await flush();
      expect(parked.textContent).toBe(before);
      expect(titles().every(t => t === "Renamed")).toBe(true);
      tabs()[1].click(); await flush(); await flush();
      expect(shown()).toBe(parked);
      expect(parked.textContent).toContain("Words that arrived later");
    } finally { globalThis.setInterval = set; globalThis.clearInterval = clear; }
  });
});
