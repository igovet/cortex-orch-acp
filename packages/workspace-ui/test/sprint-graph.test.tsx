import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type PlanItem, type Project, type Sprint, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { arrange, convRoom, inlineScale, mapWanted, noRoomWhy, toolbarTier } from "../src/sprint-space";
import { draftTask, flush, projection, scriptedPort } from "./support";

let dispose: (() => void) | undefined;
const width0 = window.innerWidth, height0 = window.innerHeight;
const resize = (w: number, h = 900) => {
  Object.defineProperty(window, "innerWidth", { value: w, configurable: true, writable: true });
  Object.defineProperty(window, "innerHeight", { value: h, configurable: true, writable: true });
  window.dispatchEvent(new Event("resize"));
};
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; document.documentElement.removeAttribute("data-keyboard"); resize(width0, height0); vi.restoreAllMocks(); });

const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string, root: ParentNode = document) => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === text);
const node = (key: string) => $<SVGGElement>(`.gsv .n[data-id="t:${key}"]`)!;
const cls = (key: string) => node(key).getAttribute("class")!.split(" ");
const edge = (from: string, to: string) => $<SVGPathElement>(`.gsv path[data-e="t:${from}>t:${to}"]`)!;
const key = (target: Element, k: string, init: KeyboardEventInit = {}) => target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
const press = (el: Element) => { el.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 })); el.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, button: 0 })); };

const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
/** A binary tree of tasks: task i waits for task (i - 1) / 2, so 15 tasks make 4 layers. Task 5 is the only one about exports. */
const plan = (n: number): PlanItem[] => Array.from({ length: n }, (_, i) => ({ key: `k${i + 1}`, title: i === 4 ? "Write the export" : `Task title ${i + 1}`, description: `Do ${i + 1}`, dependsOn: i ? [`k${Math.floor((i - 1) / 2) + 1}`] : [] }));
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Audit", title: "Audit", runMode: "manual", targetBranch: null, planning: "ready", conversation: [], plan: { version: 1, items: plan(15) }, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null, ...patch });
/** The same plan with its tasks created: task 1 needs the owner, the rest wait in the queue. */
function created(patch: Partial<Sprint> = {}): { sprint: Sprint; tasks: Task[] } {
  const items = plan(15);
  const tasks = items.map((item, i): Task => ({ ...draftTask(), id: `t-${item.key}`, title: item.title!, sprintId: "sp", status: i === 0 ? "needs_you" : "draft", dependencies: item.dependsOn.map(d => `t-${d}`) }));
  return { tasks, sprint: sprint({ planning: "created", materializedVersion: 1, taskIds: tasks.map(t => t.id), plan: { version: 1, items }, ...patch }) };
}
function world(tasks: Task[], sprints: Sprint[]): WorkspaceState {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(project);
  state.tasks.push(...tasks);
  state.sprints.push(...sprints);
  return state;
}
const SPRINT = `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/plan`;
async function open(s: Sprint, tasks: Task[] = [], view: "list" | "graph" = "graph") {
  if (view === "graph") localStorage.setItem("cw.plan.view", JSON.stringify("graph"));
  const port = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", SPRINT);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port.port} />, root);
  await settled();
  port.emit(projection("1", world(tasks, [s])));
  await settled();
  return port;
}

describe("the space rule", () => {
  it("lets the conversation give way first, then the sidebar, and keeps Details docked", () => {
    const at = (width: number, more: Partial<Parameters<typeof arrange>[0]> = {}) => arrange({ width, view: "graph", full: false, sidebar: true, sidebarW: 260, conv: true, details: false, opened: "none", ...more });
    expect(at(1920)).toMatchObject({ sidebar: true, conv: true, convW: 520, main: 1140 });
    expect(at(1440)).toMatchObject({ sidebar: true, conv: true, convW: 400, main: 780 });
    expect(at(1280)).toMatchObject({ conv: true, convW: 340, main: 680 });
    expect(at(1024)).toMatchObject({ sidebar: true, conv: false, main: 764 });
    expect(at(1440, { details: true })).toMatchObject({ sidebar: true, conv: false, details: true, replaced: false, main: 880 });
    expect(at(1024, { details: true })).toMatchObject({ sidebar: false, conv: false, details: true, main: 724 });
    expect(at(900, { details: true })).toMatchObject({ details: true, replaced: true });
    // The panel opened last wins: the conversation closes Details, and the selection is the caller's to keep.
    expect(at(1440, { details: true, opened: "conv" })).toMatchObject({ conv: true, details: false });
    expect(at(1024, { details: true, opened: "sb" })).toMatchObject({ sidebar: true, details: false });
    // Full view keeps only the plan and Details, and List needs less room than Graph.
    expect(at(1440, { full: true, details: true })).toMatchObject({ sidebar: false, conv: false, details: true, main: 1140 });
    expect(arrange({ width: 1024, view: "list", full: false, sidebar: true, sidebarW: 260, conv: true, details: false, opened: "none" })).toMatchObject({ conv: true, convW: 340 });
    // Down to the window minimum nothing overlaps: the columns always add up to the window.
    for (const width of [960, 1024, 1100, 1280, 1440, 1920]) for (const details of [false, true]) for (const view of ["list", "graph"] as const) {
      const s = arrange({ width, view, full: false, sidebar: true, sidebarW: 360, conv: true, details, opened: "none" });
      expect(s.replaced).toBe(false);
      expect((s.sidebar ? 360 : 0) + (s.conv ? s.convW : 0) + (s.details ? 300 : 0) + s.main).toBe(width);
      expect(s.main).toBeGreaterThanOrEqual(view === "graph" ? 640 : 420);
    }
  });
  it("names the room the conversation needs, and sizes the rest by the canvas", () => {
    expect(convRoom(979, "graph")).toBe(false);
    expect(convRoom(980, "graph")).toBe(true);
    expect(noRoomWhy("graph")).toBe("Widen the window to 980 px to show the conversation");
    expect(inlineScale({ width: 600, height: 200 }, 0)).toBe(1);
    expect(inlineScale({ width: 1552, height: 656 }, 700)).toBeLessThan(0.92);
    expect([toolbarTier(1140), toolbarTier(700), toolbarTier(600)]).toEqual([0, 2, 3]);
    expect([mapWanted(null, 680), mapWanted(null, 1140), mapWanted(true, 680), mapWanted(false, 1140)]).toEqual([false, true, true, false]);
  });
});

describe("the plan's List and Graph views", () => {
  it("opens on List, and remembers Graph once the owner picks it", async () => {
    await open(sprint(), [], "list");
    expect($(".sp-plan")?.dataset.view).toBe("list");
    expect($(".gsv")).toBeNull();
    button("Graph")!.click(); await settled();
    expect($(".sp-plan")?.dataset.view).toBe("graph");
    expect(JSON.parse(localStorage.getItem("cw.plan.view")!)).toBe("graph");
    expect(all(".gsv .n")).toHaveLength(15);
    expect($(".gsv")?.getAttribute("aria-label")).toBe("Dependency graph, 15 tasks");
    dispose?.(); document.body.innerHTML = "";
    await open(sprint(), [], "list");
    expect($(".sp-plan")?.dataset.view).toBe("graph");
  });

  it("shows the small graph under the list, and a row that opens the Graph view when it cannot be read", async () => {
    await open(sprint({ plan: { version: 1, items: plan(3) } }), [], "list");
    expect(all(".depg .ws-node")).toHaveLength(3);
    expect($(".gv-open-row")).toBeNull();
    // An explicit pick only: nothing is lit until the owner picks a task.
    expect(all(".depg .e.on")).toHaveLength(0);
  });

  it("enters Full view and leaves it with every panel as it was, writing no preference", async () => {
    resize(1440);
    await open(sprint());
    const stored = { ...localStorage };
    expect($(".ws-app")?.dataset.sidebar).toBe("shown");
    // The chat is its own tab: the Plan tab has the whole width.
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(true);
    button("Full view")!.click(); await settled();
    expect($(".sp-body")?.hasAttribute("data-full")).toBe(true);
    expect($(".ws-app")?.dataset.sidebar).toBe("hidden");
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(true);
    expect($(".sp-h")).toBeNull();
    expect($(".sp-plan-h")).toBeNull();
    expect($(".gv-bar")?.textContent).toContain("Exit full view");
    expect($(".gv-bar .gv-title")?.textContent).toBe("Plan v1 · 15 tasks");
    // The conversation stays mounted, so an unsent message survives.
    expect($("#ws-plan-input")).not.toBeNull();
    button("Exit full view")!.click(); await settled();
    expect($(".ws-app")?.dataset.sidebar).toBe("shown");
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(true);
    expect($(".sp-h")).not.toBeNull();
    expect({ ...localStorage }).toEqual(stored);
  });

  it("gives way by the one rule as the window narrows, and never writes it down", async () => {
    resize(1024, 640);
    await open(sprint({ planning: "planning" }));
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(true);
    expect($(".ws-app")?.dataset.sidebar).toBe("shown");
    expect(button("Working on the plan")).toBeTruthy();
    // Details opened: the sidebar gives way too, and Details is docked beside the canvas.
    node("k2").focus(); key(node("k2"), "Enter"); await settled();
    expect($(".ws-read")).not.toBeNull();
    expect($(".ws-app")?.dataset.sidebar).toBe("hidden");
    expect($(".sp-body")?.getAttribute("style")).toContain("minmax(0, 1fr) var(--inspector-w)");
    expect(localStorage.getItem("cw.ws.sidebarHidden")).toBeNull();
    // Asking for the sidebar back wins: Details closes and the selection stays.
    key(document.body, "b", { ctrlKey: true }); await settled();
    expect($(".ws-app")?.dataset.sidebar).toBe("shown");
    expect($(".ws-read")).toBeNull();
    expect(localStorage.getItem("cw.ws.sidebarHidden")).toBeNull();
    // The chip opens the Chat tab, which has the whole width, and Ctrl+\ goes back to the plan.
    button("Working on the plan")!.click(); await settled();
    expect(location.pathname).toMatch(/\/s\/sp\/planning$/);
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(false);
    expect($<HTMLElement>(".sp-plan")!.hidden).toBe(true);
    expect($(".sp-body")?.getAttribute("style")).toContain("grid-template-columns: minmax(0, 1fr)");
    key(document.body, "\\", { ctrlKey: true }); await settled();
    expect(location.pathname).toMatch(/\/s\/sp\/plan$/);
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(true);
  });

  it("keeps stopped planning's reason and its Retry or Resume in a short Graph and in Full view", async () => {
    const lane = () => $(".l-lane")!;
    for (const [failure, action] of [["The planning model stopped before it answered.", "Retry planning"], [undefined, "Resume planning"]] as const) {
      resize(1024, 640);
      const port = await open({ ...sprint({ planning: "cancelled" }), ...(failure ? { planningFailure: failure } : {}) });
      expect($(".sp-plan")?.dataset.view).toBe("graph");
      expect(lane().textContent).toContain(failure ?? "Planning is cancelled. Your draft, answers and plan are kept.");
      if (failure) expect(lane().querySelector('[role="alert"]')?.textContent).toContain(failure);
      expect(button(action, lane())).toBeTruthy();
      button("Full view")!.click(); await settled();
      expect($(".sp-body")?.hasAttribute("data-full")).toBe(true);
      expect($(".sp-h")).toBeNull();
      expect(lane().textContent).toContain(failure ?? "Planning is cancelled.");
      button(action, lane())!.click(); await settled();
      expect(port.commands.some(c => c.operation === "resume_planning")).toBe(true);
      dispose?.(); dispose = undefined; document.body.innerHTML = "";
    }
    // While planning runs, the information strip still gives way in a short Graph.
    resize(1024, 640);
    await open(sprint({ planning: "ready" }));
    expect(lane().textContent).not.toContain("Sprint planning.");
  });
});

describe("finding and selecting in the graph", () => {
  it("lights only the picked task's links, needs solid and unblocks dashed, and dims the rest", async () => {
    await open(sprint());
    expect(all(".gsv .n.dim")).toHaveLength(0);
    press(node("k2")); await settled();
    expect(cls("k2")).toContain("picked");
    expect(node("k2").getAttribute("aria-pressed")).toBe("true");
    expect(cls("k1")).toContain("lk"); expect(cls("k4")).toContain("lk"); expect(cls("k5")).toContain("lk");
    expect(cls("k3")).toContain("dim"); expect(cls("k8")).toContain("dim");
    expect(edge("k1", "k2").getAttribute("class")).toContain("need");
    expect(edge("k2", "k4").getAttribute("class")).toContain("block");
    expect(edge("k1", "k3").getAttribute("class")).toContain("dim");
    expect($(".gv-band")?.textContent).toContain("Task 2 · Needs 1 · Unblocks 4, 5");
    button("Whole chain")!.click(); await settled();
    expect(cls("k8")).toContain("lk");
    expect(edge("k4", "k8").getAttribute("class")).toContain("block");
    expect(JSON.parse(localStorage.getItem("cw.graph.links")!)).toBe("chain");
    // A press on the empty canvas clears the pick.
    press($(".gv-canvas")!); await settled();
    expect(all(".gsv .n.dim")).toHaveLength(0);
  });

  it("dims what search and the status filter leave out, in place, with a count and a way back", async () => {
    const c = created();
    await open(c.sprint, c.tasks);
    const field = $<HTMLInputElement>('[aria-label="Search tasks in the graph"]')!;
    field.value = "export"; field.dispatchEvent(new Event("input", { bubbles: true }));
    await wait(120); await settled();
    expect($(".gv-count")?.textContent).toBe("1 of 1");
    expect(cls("k5")).toEqual(expect.arrayContaining(["mt", "cur"]));
    expect(cls("k1")).toContain("dim");
    expect(all(".gsv .n")).toHaveLength(15);
    expect($(".ws-live")?.textContent).toBe("1 match. 1 of 1, task 5, Write the export.");
    field.value = "zzz"; field.dispatchEvent(new Event("input", { bubbles: true }));
    await wait(120); await settled();
    expect($(".gv-count")?.textContent).toBe("No matches");
    expect($(".gv-band")?.textContent).toContain('No task matches "zzz".');
    button("Clear search")!.click(); await settled();
    expect(all(".gsv .n.dim")).toHaveLength(0);
    // Status rows count by state word; task 1 needs the owner.
    const rows = all<HTMLLabelElement>(".gv-pop label.mi").filter(l => l.closest("[aria-label='Show tasks by status']"));
    const needs = rows.find(r => r.textContent?.includes("Needs you"))!;
    expect(needs.textContent).toContain("1");
    const box = needs.querySelector("input")!;
    box.checked = true; box.dispatchEvent(new Event("change", { bubbles: true })); await settled();
    expect($(".gv-bar")?.textContent).toContain("Status · 1");
    expect($(".gv-count")?.textContent).toBe("1 of 15");
    expect(cls("k1")).not.toContain("dim");
    expect(cls("k2")).toContain("dim");
    expect(node("k1").dataset.f).toBe("needs");
  });

  it("moves by keyboard along dependencies and layers, and Esc steps back one thing at a time", async () => {
    resize(1440);
    await open(sprint());
    document.documentElement.setAttribute("data-keyboard", "");
    expect(node("k1").getAttribute("tabindex")).toBe("0");
    expect(node("k2").getAttribute("tabindex")).toBe("-1");
    node("k1").focus();
    key(node("k1"), "ArrowRight"); await settled();
    expect(["t:k2", "t:k3"]).toContain((document.activeElement as SVGGElement).dataset.id);
    const first = (document.activeElement as SVGGElement).dataset.id!;
    key(document.activeElement!, "ArrowDown"); key(document.activeElement!, "ArrowUp"); await settled();
    expect((document.activeElement as SVGGElement).dataset.id).toBe(first);
    key(document.activeElement!, "ArrowLeft"); await settled();
    expect((document.activeElement as SVGGElement).dataset.id).toBe("t:k1");
    key(document.activeElement!, "End"); await settled();
    expect((document.activeElement as SVGGElement).dataset.id).toBe("t:k15");
    key(document.activeElement!, "Home"); await settled();
    key(document.activeElement!, " "); await settled();
    expect(cls("k1")).toContain("picked");
    // Collapse the first task's layer: focus moves to its band, and Enter opens it again.
    key(document.activeElement!, "c"); await settled();
    expect((document.activeElement as SVGGElement).dataset.id).toBe("b:0");
    expect($(".gsv .band")?.getAttribute("aria-label")).toContain("Layer 1, collapsed, 1 tasks");
    key(document.activeElement!, "Enter"); await settled();
    expect($(".gsv .band")).toBeNull();
    // Esc: search text, then Details, then selection, then Full view.
    key(node("k1"), "e"); await settled();
    expect($(".sp-body")?.hasAttribute("data-full")).toBe(true);
    const field = $<HTMLInputElement>('[aria-label="Search tasks in the graph"]')!;
    key(document.body, "/"); expect(document.activeElement).toBe(field);
    field.value = "Task"; field.dispatchEvent(new Event("input", { bubbles: true })); await wait(120); await settled();
    node("k1").focus(); key(node("k1"), "Enter"); await settled();
    expect($(".ws-read")).not.toBeNull();
    key(document.body, "Escape"); await settled();
    expect(field.value).toBe("");
    expect($(".ws-read")).not.toBeNull();
    key(document.body, "Escape"); await settled();
    expect($(".ws-read")).toBeNull();
    expect(cls("k1")).toContain("picked");
    key(document.body, "Escape"); await settled();
    expect(cls("k1")).not.toContain("picked");
    expect($(".sp-body")?.hasAttribute("data-full")).toBe(true);
    key(document.body, "Escape"); await settled();
    expect($(".sp-body")?.hasAttribute("data-full")).toBe(false);
  });

  it("turns the minimap on with the first press where the canvas starts without it", async () => {
    await open(sprint());
    const toggle = $<HTMLButtonElement>('[aria-label="Minimap"]')!;
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    node("k1").focus(); key(node("k1"), "m"); await settled();
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    expect(JSON.parse(localStorage.getItem("cw.graph.minimap")!)).toBe(true);
    key(node("k1"), "m"); await settled();
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
  });

  it("pans with the minimap's arrows and keeps the focus on it, never moving to a task", async () => {
    resize(1440);
    localStorage.setItem("cw.graph.minimap", JSON.stringify(true));
    await open(sprint());
    document.documentElement.setAttribute("data-keyboard", "");
    // A task had the focus first, so the canvas remembers one to go back to.
    node("k1").focus(); await settled();
    const map = $(".gv-map")!, layer = $(".gv-layer")!;
    map.focus();
    expect(document.activeElement).toBe(map);
    const moves: string[] = [layer.style.transform];
    for (const [k, init] of [["ArrowRight", {}], ["ArrowRight", {}], ["ArrowDown", {}], ["ArrowLeft", { ctrlKey: true }]] as const) {
      expect(key(map, k, init)).toBe(false);
      await settled();
      expect(document.activeElement).toBe(map);
      moves.push(layer.style.transform);
    }
    expect(all(".gsv .n").some(n => n === document.activeElement)).toBe(false);
    // Each press moves the view by one step, and a held Ctrl does not move it twice.
    const at = (t: string) => /translate\((-?[\d.]+)px, (-?[\d.]+)px\)/.exec(t)!.slice(1).map(Number);
    const [x0, y0] = at(moves[0]), [x1] = at(moves[1]), [x2] = at(moves[2]), [, y3] = at(moves[3]), [x4] = at(moves[4]);
    expect([x1 - x0, x2 - x1, y3 - y0, x4 - x2]).toEqual([-80, -80, -80, 80]);
  });
});

describe("dimmed graph text", () => {
  const source = (name: string) => readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src", name), "utf8");
  const rules = (css: string) => [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(m => ({ selector: m[1].trim(), body: m[2] }));
  const tokens = rules(source("mockup-styles.css")), own = rules(source("workspace.css"));
  const token = (theme: "dark" | "light", name: string) => {
    const block = tokens.find(r => r.selector.endsWith(`[data-theme="${theme}"]`) && r.body.includes("--bg-canvas"))!.body;
    return new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`).exec(block)![1];
  };
  const luminance = (hex: string) => {
    const [r, g, b] = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(c => c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4);
    return .2126 * r + .7152 * g + .0722 * b;
  };
  const contrast = (a: string, b: string) => { const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p); return (x + .05) / (y + .05); };

  it("reads at 4.5:1 on the canvas in both themes, for every task line and collapsed layer", () => {
    const dim = own.filter(r => /\.dim\b/.test(r.selector) && / text\b|\.w\b|\.no\b/.test(r.selector));
    const sel = dim.map(r => r.selector).join(", ");
    for (const part of [".gsv .n.dim text", ".gsv .n.dim .w", ".gsv .n.dim .no", ".gsv .band.dim text"]) expect(sel).toContain(part);
    // One solid colour: no transparency on the text, so the ratio is the colour's own.
    for (const r of dim) expect(r.body).not.toMatch(/opacity/);
    const colours = [...new Set(dim.map(r => /fill:\s*var\(--([\w-]+)\)/.exec(r.body)![1]))];
    expect(colours).toEqual(["fg-3"]);
    // A dimmed task has no fill of its own and a dimmed layer is filled with the canvas, so the canvas is behind the text.
    expect(own.find(r => r.selector === ".gsv .n.dim .bx")!.body).toContain("fill: none");
    expect(own.find(r => r.selector === ".gsv .band.dim .bx")!.body).toContain("fill: var(--bg-canvas)");
    expect(own.find(r => r.selector === ".gv-canvas")!.body).toContain("background: var(--bg-canvas)");
    for (const theme of ["dark", "light"] as const) for (const surface of ["bg-canvas", "bg-raised"]) {
      expect(contrast(token(theme, colours[0]), token(theme, surface))).toBeGreaterThanOrEqual(4.5);
    }
  });
});

describe("the sprint's tabs", () => {
  it("offers Chat, Plan, Board and Summary once each, and gives the chat and the plan the whole width", async () => {
    resize(1280);
    await open(sprint(), [], "list");
    expect(all(".tabs [role=tab]").map(t => t.textContent?.trim())).toEqual(["Chat", "Plan", "Board", "Summary"]);
    expect($(".tabs [aria-selected=true]")?.textContent).toBe("Plan");
    expect($(".sp-plan-h [aria-label='Planning conversation']")).toBeNull();
    all<HTMLAnchorElement>(".tabs [role=tab]")[0].click(); await settled();
    expect($(".tabs [aria-selected=true]")?.textContent).toBe("Chat");
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(false);
    expect($<HTMLElement>(".sp-plan")!.hidden).toBe(true);
    expect($(".sp-body")?.getAttribute("style")).toContain("grid-template-columns: minmax(0, 1fr)");
    // The proposed plan's line in the chat opens the Plan tab.
    button("Open plan")!.click(); await settled();
    expect($<HTMLElement>(".sp-plan")!.hidden).toBe(false);
  });

  it("says once why a created sprint's chat cannot be written to", async () => {
    const c = created();
    await open(c.sprint, c.tasks, "list");
    all<HTMLAnchorElement>(".tabs [role=tab]")[0].click(); await settled();
    const field = $<HTMLTextAreaElement>("#ws-plan-input")!;
    expect(field.disabled).toBe(true);
    expect(field.placeholder).toBe("Tasks are created. Edit them on the Board.");
    const bar = field.closest(".composer")!.querySelector(".c-bar")!;
    // The note is said in the writing area only, never again under the tools.
    expect(bar.textContent).not.toContain("Edit them on the Board");
    for (const label of ["Attach files", "Voice input", "Send to the planner"]) {
      const control = bar.querySelector(`[aria-label="${label}"]`)!;
      expect(control.getAttribute("aria-disabled")).toBe("true");
      expect(control.getAttribute("aria-describedby")).toBe("ws-plan-input");
    }
  });
});

describe("Request changes and the plan's other versions", () => {
  it("leaves Full view, opens the Chat tab and puts the cursor in it, keeping the pick", async () => {
    resize(1440);
    await open(sprint());
    button("Full view")!.click(); await settled();
    press(node("k3")); await settled();
    button("Request changes", $(".gv-band")!)!.click(); await settled();
    expect($(".sp-body")?.hasAttribute("data-full")).toBe(false);
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(false);
    expect(document.activeElement?.id).toBe("ws-plan-input");
    // Back on the Plan tab the graph keeps the pick.
    button("Open plan")!.click(); await settled();
    expect(cls("k3")).toContain("picked");
  });

  it("works at the window minimum too: the chat has its own tab, so nothing waits for room", async () => {
    resize(960, 720);
    await open(sprint());
    button("Full view")!.click(); await settled();
    const request = button("Request changes", $(".gv-band")!)!;
    expect(request.getAttribute("aria-disabled")).toBeNull();
    request.click(); await settled();
    expect($(".sp-body")?.hasAttribute("data-full")).toBe(false);
    expect($<HTMLElement>(".sp-conv")!.hidden).toBe(false);
  });

  it("shows no states and no Status filter for a past version or a draft", async () => {
    const c = created({ planning: "created", materializedVersion: 2, history: [{ version: 1, items: plan(15) }], plan: { version: 2, items: plan(15) } });
    await open(c.sprint, c.tasks);
    expect($(".gv-bar")?.textContent).toContain("Status");
    expect(node("k1").dataset.f).toBe("needs");
    button("Previous plan")!.click();
    [...document.querySelectorAll<HTMLButtonElement>(".mi")].find(b => b.textContent?.includes("Plan v1"))!.click(); await settled();
    expect($(".sp-plan-h")?.textContent).toContain("Past version · read only");
    expect($(".gv-bar")?.textContent).not.toContain("Status");
    expect(all<SVGGElement>(".gsv .n").every(n => n.dataset.f === "")).toBe(true);
    expect(button("Request changes")).toBeUndefined();
    dispose?.(); document.body.innerHTML = "";
    await open(sprint());
    button("Edit")!.click(); await settled();
    expect($(".sp-plan")?.dataset.view).toBe("list");
    button("Graph")!.click(); await settled();
    expect($(".sp-plan-h")?.textContent).toContain("Draft · not saved");
    expect($(".gv-band")?.textContent).toContain("Change titles and dependencies in List.");
    expect($(".gv-bar")?.textContent).not.toContain("Status");
  });
});
