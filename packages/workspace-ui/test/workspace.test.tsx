import { render } from "solid-js/web";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Project, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort, unavailableTransport } from "../src/port/real";
import { capabilitiesWire, draftTask, flush, projection, scriptedPort, scriptedTransport, snapshot, wireTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); document.documentElement.removeAttribute("data-theme"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

async function mount(port: Parameters<typeof Workspace>[0]["port"], path = "/", extra: Partial<Parameters<typeof Workspace>[0]> = {}) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port} {...extra} />, root);
  await flush(); await flush();
  return root;
}
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string) => all("button").find(b => (b.textContent ?? "").includes(text));

const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
const withDraft = (patch: Partial<Task> = {}): WorkspaceState => {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(project);
  state.tasks.push({ ...draftTask(), ...patch });
  return state;
};

describe("native window frame", () => {
  const chrome = (maximized = false) => ({ maximized: () => maximized, minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn(), startDragging: vi.fn(), guardClose: vi.fn(), closeNow: vi.fn() });
  const fire = (el: EventTarget, type: string, init: MouseEventInit = {}) => el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, ...init }));

  it("has one real control group, each button doing what it says", async () => {
    const c = chrome();
    await mount(scriptedPort().port, "/", { chrome: c });
    expect(all(".win")).toHaveLength(1);
    expect($(".win")?.getAttribute("aria-hidden")).toBeNull();
    for (const [label, call] of [["Minimize", c.minimize], ["Maximize", c.toggleMaximize], ["Close window", c.close]] as const) {
      $(`.win button[aria-label="${label}"]`)!.click();
      expect(call).toHaveBeenCalledOnce();
    }
  });
  it("names Restore while maximized", async () => {
    await mount(scriptedPort().port, "/", { chrome: chrome(true) });
    expect($('.win button[aria-label="Restore"]')).toBeTruthy();
  });
  it("drags only from free band space, and only once the pointer really moved", async () => {
    const c = chrome();
    await mount(scriptedPort().port, "/", { chrome: c });
    const band = $(".tb-main")!;
    fire(band, "mousedown", { clientX: 500, clientY: 10 });
    fire(document, "mousemove", { clientX: 501, clientY: 10, buttons: 1 });
    expect(c.startDragging).not.toHaveBeenCalled();
    fire(document, "mousemove", { clientX: 510, clientY: 12, buttons: 1 });
    fire(document, "mousemove", { clientX: 520, clientY: 14, buttons: 1 });
    expect(c.startDragging).toHaveBeenCalledOnce();
    fire($(".titlebar button")!, "mousedown", { clientX: 10, clientY: 10 });
    fire(document, "mousemove", { clientX: 40, clientY: 30, buttons: 1 });
    expect(c.startDragging).toHaveBeenCalledOnce();
  });
  it("maximizes on a double click of free band space, never of a control", async () => {
    const c = chrome();
    await mount(scriptedPort().port, "/", { chrome: c });
    fire($(".titlebar button")!, "dblclick");
    expect(c.toggleMaximize).not.toHaveBeenCalled();
    fire($(".tb-main")!, "dblclick");
    expect(c.toggleMaximize).toHaveBeenCalledOnce();
  });
  it("shows no window controls and no drag in a browser, and only the demo keeps its inert group", async () => {
    await mount(scriptedPort().port);
    expect($(".win")).toBeNull();
    dispose?.(); document.body.innerHTML = "";
    await mount(scriptedPort({ mode: "demo" }).port);
    expect($(".win")?.getAttribute("aria-hidden")).toBe("true");
  });
  it("puts a host notice inside the main area, so it cannot take a frame row", async () => {
    await mount(scriptedPort().port, "/", { notice: <div class="strip info" id="host-notice">Setting up</div> });
    expect($("#host-notice")?.closest("main")).toBeTruthy();
    expect($(".ws-shell")?.firstElementChild?.classList.contains("titlebar")).toBe(true);
  });
});

describe("shared styles", () => {
  const folder = fileURLToPath(import.meta.url).replace(/test\/[^/]*$/, "src/");
  const source = (name: string) => readFileSync(folder + name, "utf8");
  const selectors = (css: string) => new Set([...css.replace(/\{[^{}]*\}/g, "{}").matchAll(/\.([a-zA-Z][\w-]*)/g)].map(m => m[1]));
  it("adds no class that collides with an approved mockup class", () => {
    // A second .split once turned the title band's split button into a 360px grid.
    const approved = selectors(source("mockup-styles.css"));
    const own = source("workspace.css").split("\n").flatMap(line => { const m = /^(\.[a-z][\w-]*) \{ display: grid/.exec(line); return m ? [m[1].slice(1)] : []; });
    expect(own.filter(name => approved.has(name))).toEqual([]);
  });

  it("has a mask for every icon a screen names", () => {
    // Without an .i-<name> rule, --m is empty and the icon paints as a solid block of its text colour.
    const masks = selectors(source("mockup-styles.css") + source("workspace.css"));
    const files = readdirSync(folder, { recursive: true }).map(String).filter(name => /\.tsx?$/.test(name));
    const named = new Set(files.flatMap(name => [...source(name).matchAll(/(?:\bn=|\bicon[:=] ?)"([a-z][a-z0-9-]*)"/g)].map(m => m[1])));
    expect(named.size).toBeGreaterThan(40);
    expect([...named].filter(name => !masks.has(`i-${name}`))).toEqual([]);
  });
});

describe("production workspace", () => {
  it("starts empty on the local machine, with no demo anywhere", async () => {
    const root = await mount(scriptedPort().port);
    expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}`);
    expect($(".crumbs .cur")?.textContent).toBe("Home");
    expect($(".mock-chip")).toBeNull();
    expect($(".ws-demo-banner")).toBeNull();
    expect(root.textContent).not.toMatch(/mock|fixture|simulat|prototype|Demo/i);
  });

  it("names a service it cannot reach, offers a retry, and never falls back to demo data", async () => {
    const root = await mount(createRealPort({ transport: unavailableTransport(), actorId: "owner" }));
    // Offline, not a failed start: the workspace frame with its Offline notice, and nothing about projects guessed.
    expect($(".l-startup")).toBeNull();
    expect($(".ws-conn")).toBeTruthy();
    expect($(".main [role=status]")?.textContent).toMatch(/could not be loaded/i);
    expect(root.textContent).not.toMatch(/No projects on this machine/);
    expect(button("Try again")).toBeTruthy();
    expect(root.textContent).not.toMatch(/mock-|fixture|Cedar Notes|Mosaic/i);
    expect(all(".proj").length).toBe(0);
  });

  it("recovers on Retry once the service answers", async () => {
    const s = scriptedTransport({});
    const root = await mount(createRealPort({ transport: s.transport, actorId: "owner", projects: [{ projectId: "p", name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" }] }));
    expect($(".ws-conn")).toBeTruthy();
    s.answers["capabilities.get"] = () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create"]);
    s.answers["project.snapshot"] = () => snapshot("1", [wireTask()]);
    button("Try again")!.click();
    await flush(); await flush();
    expect($(".l-startup")).toBeNull();
    expect($(".ws-conn")).toBeNull();
    expect(root.textContent).not.toMatch(/could not be loaded/);
    expect(root.textContent).toContain("Cedar");
  });

  it("disables what the machine does not offer, says why, and sends nothing", async () => {
    const s = scriptedPort({ capabilities: { everything: false, operations: ["tasks.create"] } });
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/t`);
    s.emit(projection("1", withDraft()));
    await flush();
    const start = all(".tb-actions button").find(b => b.textContent?.includes("Start"))!;
    expect(start.getAttribute("aria-disabled")).toBe("true");
    expect(start.dataset.why).toMatch(/not available/i);
    start.click();
    await flush();
    expect(s.commands).toEqual([]);
  });

  it("enables the same control when the machine offers it, and a receipt alone changes nothing", async () => {
    const s = scriptedPort();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/t`);
    s.emit(projection("1", withDraft()));
    await flush();
    const start = all(".tb-actions button").find(b => b.textContent?.includes("Start"))!;
    expect(start.getAttribute("aria-disabled")).toBeNull();
    start.click();
    await flush();
    expect(s.commands).toHaveLength(1);
    expect(s.commands[0].operation).toBe("start_task");
    s.receipt(true);
    await flush();
    expect(all(".tb-actions button").some(b => b.textContent?.includes("Start"))).toBe(true);
    s.emit(projection("2", withDraft({ status: "working", process: "live" })));
    await flush();
    expect(all(".tb-actions button").some(b => b.textContent?.includes("Start"))).toBe(false);
  });

  it("leaves model, effort, access and language unavailable until a session offers them", async () => {
    await mount(scriptedPort().port, `/m/${LOCAL_MACHINE_ID}/new`);
    // Model and effort, then one Settings control for access and language.
    const selects = all(".c-route :is(.sel, .c-set)");
    expect(selects).toHaveLength(3);
    for (const select of selects) expect(select.getAttribute("aria-disabled")).toBe("true");
  });

  it("keeps Voice in its place but unusable, with its reason, where it cannot be delivered", async () => {
    await mount(scriptedPort().port, `/m/${LOCAL_MACHINE_ID}/new`);
    const voice = all("button").find(b => b.getAttribute("aria-label") === "Voice input")!;
    expect(voice.getAttribute("aria-disabled")).toBe("true");
    expect(voice.dataset.why).toBeTruthy();
    voice.click(); await flush();
    expect($(".micbar")).toBeNull();
  });

  it("opens every screen, and a machine that offers nothing leaves its controls off", async () => {
    const root = await mount(scriptedPort().port, "/settings/machine/"+LOCAL_MACHINE_ID+"/orchestration/tools");
    expect(root.textContent).toContain("Tools and MCP");
    expect(root.textContent).not.toContain("Not connected in this version");
    expect(root.querySelector('input[aria-label="Server name"]')).toBeNull();
    for (const path of [`/m/${LOCAL_MACHINE_ID}/archive`, "/settings/app/shortcuts", `/settings/machine/${LOCAL_MACHINE_ID}/storage`, `/settings/machine/${LOCAL_MACHINE_ID}/diagnostics`]) {
      dispose?.(); document.body.innerHTML = "";
      const page = await mount(scriptedPort().port, path);
      expect(page.textContent).not.toContain("Not connected in this version");
      expect(page.querySelector(".crumbs")).toBeTruthy();
    }
  });

  it("keeps the theme in Settings only, with System as the default", async () => {
    await mount(scriptedPort().port);
    expect(all(".ws-shell button").some(b => /theme/i.test(`${b.title} ${b.getAttribute("aria-label") ?? ""}`))).toBe(false);
    history.pushState(null, "", "/settings/app/appearance");
    window.dispatchEvent(new PopStateEvent("popstate"));
    await flush();
    const system = all('[aria-current="true"], [aria-checked="true"], input:checked').some(el => /System/.test(el.closest("label, button, .seg")?.textContent ?? ""));
    expect(system).toBe(true);
    expect(localStorage.getItem("cw.theme")).toBeNull();
  });

  it("describes a failed run as failed, and a cancelled one as cancelled, never as done", async () => {
    const s = scriptedPort();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/p/p/board`);
    s.emit(projection("1", (() => { const st = withDraft({ status: "failed" }); st.tasks.push({ ...draftTask(), id: "c", title: "Cancelled one", status: "cancelled" }); return st; })()));
    await flush();
    const done = $('[data-col="done"]') ?? all(".col").find(c => c.textContent?.startsWith("Done"));
    expect(done?.textContent ?? "").not.toContain("Fix the race");
    expect(done?.textContent ?? "").not.toContain("Cancelled one");
    const needs = all(".col").find(c => c.textContent?.includes("Needs you"));
    expect(needs?.textContent).toContain("Run failed");
    expect(needs?.textContent).toContain("Run cancelled");
  });
});
