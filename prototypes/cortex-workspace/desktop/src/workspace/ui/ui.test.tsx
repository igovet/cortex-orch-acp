import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { loadFixtures } from "../../fixtures";
import { createMockWorkspace } from "../adapter/mock";
import { WORKSPACE_CASES, WORKSPACE_FIXTURE_NOW } from "../adapter/fixtures";
import type { WorkspacePort } from "../domain";
import Workspace from "./index";
import { hrefOf, parseRoute, type Route } from "@cortex/workspace-ui/router";
import { buildIndex, columnsOf, engineLimits, homeView, inboxItems, inScope } from "@cortex/workspace-ui/selectors";

const LOCAL = "mock-machine-00000", REMOTE = "mock-machine-00001";
const MOSAIC = "mock-project-00000", CEDAR = "mock-project-00001", HARBOR = "mock-project-00007";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const tick = () => new Promise(r => setTimeout(r, 0));
async function mount(path: string, port?: () => WorkspacePort) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port} autoplay={false} />, root);
  await tick();
  return root;
}
const remount = async (path = location.pathname + location.search) => { dispose?.(); document.body.innerHTML = ""; return mount(path); };
const app = () => document.querySelector<HTMLElement>(".ws-app")!;
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const named = (selector: string, name: string) => all(selector).find(el => (el.getAttribute("aria-label") ?? el.textContent ?? "").includes(name))!;
const click = async (el: Element) => { (el as HTMLElement).click(); await tick(); };
const press = async (key: string, opts: KeyboardEventInit = {}) => { document.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...opts })); await tick(); };
const heading = (id: string) => document.getElementById(id)!.textContent!;
const fixtureIndex = () => buildIndex(loadFixtures().workspace, WORKSPACE_FIXTURE_NOW);

describe("routes", () => {
  it("round-trips every route through its URL", () => {
    const routes: Route[] = [
      { name: "home", machine: LOCAL }, { name: "inbox", machine: LOCAL }, { name: "archive", machine: LOCAL },
      { name: "new", machine: LOCAL, kind: "sprint", project: CEDAR }, { name: "task", machine: LOCAL, task: "mock-task-00002" },
      { name: "project", machine: LOCAL, project: CEDAR, view: "list", scope: { kind: "standalone" } },
      { name: "project", machine: LOCAL, project: CEDAR, view: "board", scope: { kind: "sprint", id: "s1" } },
      { name: "settings", section: "appearance", machine: null }, { name: "settings", section: "storage", machine: LOCAL },
      { name: "settings", section: "orchestration", machine: LOCAL, page: "routing" }, { name: "sprint", machine: LOCAL, project: CEDAR, sprint: "s1", tab: "summary" },
    ];
    for (const route of routes) {
      const [path, search] = hrefOf(route).split("?");
      expect(parseRoute(path, search ? `?${search}` : "")).toEqual(route);
    }
    expect(parseRoute("/m/x/p/y/nope").name).toBe("notfound");
    expect(parseRoute("/settings/app/storage").name).toBe("notfound");
  });

  it("routes no earlier exploration", () => {
    for (const path of ["/a", "/b", "/archive/a", "/needs-you", "/p/cedar-notes", "/settings", "/settings/about"]) expect(parseRoute(path).name).toBe("notfound");
  });
});

describe("read models", () => {
  it("counts Needs you the same on Home, in the projects table and on every board", () => {
    const index = fixtureIndex(), home = homeView(index, LOCAL);
    expect(home.needs.length).toBeGreaterThan(0);
    expect(home.projects.reduce((sum, p) => sum + p.counts.needs, 0)).toBe(home.needs.length);
    expect(home.projects.reduce((sum, p) => sum + columnsOf(inScope(p, { kind: "all" })).needs.length, 0)).toBe(home.needs.length);
  });

  it("never reports a task on an offline machine as a confirmed state", () => {
    const index = fixtureIndex(), offline = index.byId.get(WORKSPACE_CASES.offline)!;
    expect(offline.unknown).toBe(true);
    expect(offline.fact.text).toBe("Last known: Working");
    expect(offline.column).toBe("work");
    expect(homeView(index, REMOTE).projects[0].signal?.text).toBe("Offline");
    expect(inboxItems(index).some(d => d.view?.id === WORKSPACE_CASES.offline)).toBe(false);
  });

  it("shows a stop the engine has not confirmed as unconfirmed, not as stopped", () => {
    expect(fixtureIndex().byId.get(WORKSPACE_CASES.stopping)!.fact.text).toBe("Stop not confirmed");
  });

  it("keeps an unknown engine limit unknown and offers no window but the week", () => {
    const state = loadFixtures().workspace, limits = engineLimits(state, state.machines[0]);
    expect(limits.rows.map(r => r.label)).toEqual(["Claude", "Codex"]);
    expect(limits.rows[0].usedPercent).toBeNull();
    expect(limits.rows.every(r => r.window === "weekly")).toBe(true);
    expect(engineLimits(state, state.machines[1]).rows).toEqual([]);
  });

  it("carries sprint planning and unmerged-work questions in the Inbox only", () => {
    const kinds = inboxItems(fixtureIndex()).map(d => d.kind);
    expect(kinds).toContain("Planning");
    expect(kinds).toContain("Unmerged work");
  });
});

describe("navigation", () => {
  it("opens on Home of the local machine, with counters that match their rows", async () => {
    await mount("/");
    expect(location.pathname).toBe(`/m/${LOCAL}`);
    expect(document.querySelector(".crumbs .cur")!.textContent).toBe("Home");
    expect(heading("h-need")).toContain(String(all(".home .need").length));
    expect(heading("h-review")).toContain(String(all('[aria-labelledby="h-review"] .item').length));
    expect(heading("h-merge")).toContain(String(all('[aria-labelledby="h-merge"] .item').length));
    expect(all(".home .tr:not(.th)").length).toBe(6);
    expect(document.querySelector(".nav[aria-current=page]")!.textContent).toContain("Home");
  });

  it("moves between Home, a project board, its list and a task, and Back and Forward follow", async () => {
    await mount(`/m/${LOCAL}`);
    const back = () => document.querySelector('[aria-label="Back"]')!, forward = () => document.querySelector('[aria-label="Forward"]')!;
    expect(back().getAttribute("aria-disabled")).toBe("true");
    await click(named(".proj", "Cedar Notes"));
    expect(location.pathname).toBe(`/m/${LOCAL}/p/${CEDAR}/board`);
    expect(document.querySelector(".crumbs .cur")!.textContent).toBe("Cedar Notes");
    expect(document.querySelector(".proj[aria-current=page]")!.textContent).toContain("Cedar Notes");
    expect(all(".board-wrap .col-h").length).toBeGreaterThan(0);
    await click(named('[role="tab"]', "List"));
    expect(location.pathname).toBe(`/m/${LOCAL}/p/${CEDAR}/list`);
    expect(all(".lrow").length).toBeGreaterThan(0);
    expect(document.querySelector(".board-wrap")).toBeNull();
    await click(all(".lrow")[0]);
    expect(location.pathname).toContain("/t/");
    expect(document.querySelector(".crumbs .sep")).toBeTruthy();
    expect(back().hasAttribute("aria-disabled")).toBe(false);
    expect(forward().getAttribute("aria-disabled")).toBe("true");
    history.back();
    await expect.poll(() => location.pathname).toBe(`/m/${LOCAL}/p/${CEDAR}/list`);
    await expect.poll(() => forward().getAttribute("aria-disabled")).toBeNull();
  });

  it("scopes the board to a sprint and back, and a sprint offers its queue", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    const sprint = all('#m-scope .mi').find(el => el.textContent!.includes("Synthetic linked sprint"))!;
    await click(sprint);
    expect(location.pathname).toMatch(/\/s\/[^/]+\/board$/);
    expect(document.querySelector(".scope-btn")!.textContent).toContain("Synthetic linked sprint");
    expect(document.querySelector(".sprint-bar")).toBeTruthy();
    expect(document.querySelector('.seg [aria-current="true"]')!.textContent).toBe("Automatic");
    expect(all(".card").length).toBe(3);
    await click(named("#m-scope .mi", "All work"));
    expect(document.querySelector(".sprint-bar")).toBeNull();
  });

  it("switches machine only on request, and an offline machine says so", async () => {
    await mount(`/m/${LOCAL}`);
    expect(all(".proj").length).toBe(6);
    await click(all("#m-machine .mi").find(el => el.textContent!.includes("Synthetic remote"))!);
    expect(location.pathname).toBe(`/m/${REMOTE}`);
    expect(all(".proj").map(el => el.textContent)).toEqual([expect.stringContaining("Mosaic Studio")]);
    expect(document.querySelector(".machine")!.textContent).toContain("Offline");
    expect(document.querySelector(".tb-main")!.textContent).toContain("last confirmed");
    await click(named(".proj", "Mosaic Studio"));
    expect(document.querySelector(".strip.info")!.textContent).toContain("Last known state");
    expect(all(".card-a .btn").length).toBe(0);
  });

  it("explains unknown machines, projects and tasks instead of failing", async () => {
    await mount("/m/nobody");
    expect(document.querySelector(".note-panel")!.textContent).toContain("not connected");
    await remount(`/m/${LOCAL}/p/${MOSAIC}/board`);
    expect(document.querySelector(".note-panel")!.textContent).toContain("not on this machine");
    await remount(`/m/${LOCAL}/t/nothing`);
    expect(document.querySelector(".note-panel")!.textContent).toContain("not on this machine");
  });
});

describe("panels", () => {
  const toggle = (name: string) => document.querySelector(`[aria-label="${name}"]`)!;

  it("restores the sidebar from its fixed toggle, the keyboard and the palette, and keeps the state across reloads", async () => {
    await mount(`/m/${LOCAL}`);
    await click(toggle("Sidebar"));
    expect(app().dataset.sidebar).toBe("hidden");
    expect(toggle("Sidebar").getAttribute("aria-pressed")).toBe("false");
    await click(toggle("Sidebar"));
    expect(app().dataset.sidebar).toBe("shown");
    await press("b", { ctrlKey: true });
    expect(app().dataset.sidebar).toBe("hidden");
    await remount();
    expect(app().dataset.sidebar).toBe("hidden");
    await press("b", { ctrlKey: true });
    expect(app().dataset.sidebar).toBe("shown");
    await press("b", { ctrlKey: true });
    await press("k", { ctrlKey: true });
    await click(all(".ws-pal-o").find(el => el.textContent!.includes("Show sidebar"))!);
    expect(app().dataset.sidebar).toBe("shown");
  });

  it("hides and restores the inspector", async () => {
    await mount(`/m/${LOCAL}`);
    expect(toggle("Inspector").getAttribute("aria-pressed")).toBe("true");
    await click(toggle("Inspector"));
    expect(app().dataset.inspector).toBe("hidden");
    await press("b", { ctrlKey: true, altKey: true, code: "KeyB" });
    expect(app().dataset.inspector).toBe("shown");
    expect(app().dataset.sidebar).toBe("shown");
  });

  it("offers the terminal where a checkout is in view and keeps it open across views", async () => {
    await mount(`/m/${LOCAL}`);
    expect(toggle("Terminal").getAttribute("aria-disabled")).toBe("true");
    await press("j", { ctrlKey: true });
    expect(app().dataset.terminal).toBe("closed");
    expect(document.querySelector(".terminal")).toBeNull();
    await click(named(".proj", "Cedar Notes"));
    expect(toggle("Terminal").hasAttribute("aria-disabled")).toBe(false);
    await press("j", { ctrlKey: true });
    expect(app().dataset.terminal).toBe("open");
    expect(document.querySelector(".terminal")!.textContent).toContain("No shell runs");
    await click(named(".nav", "Home"));
    expect(document.querySelector(".terminal")).toBeNull();
    await click(named(".proj", "Cedar Notes"));
    expect(app().dataset.terminal).toBe("open");
    await click(toggle("Terminal"));
    expect(app().dataset.terminal).toBe("closed");
  });

  it("keeps the sidebar width inside its limits and across reloads", async () => {
    await mount(`/m/${LOCAL}`);
    const handle = () => document.querySelector<HTMLElement>(".ws-resize")!;
    handle().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await tick();
    expect(app().style.getPropertyValue("--sidebar-w")).toBe("276px");
    await remount();
    expect(app().style.getPropertyValue("--sidebar-w")).toBe("276px");
    for (let i = 0; i < 20; i++) handle().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await tick();
    expect(handle().getAttribute("aria-valuenow")).toBe("360");
  });

  it("nests live tasks under a project, collapses on request and remembers it", async () => {
    await mount(`/m/${LOCAL}`);
    expect(all(".task-row").length).toBeGreaterThan(0);
    expect(all(".task-row").length).toBeLessThanOrEqual(6);
    const open = named(".proj-x", "Hide tasks");
    await click(open);
    expect(all(".task-row").length).toBe(0);
    await remount();
    expect(all(".task-row").length).toBe(0);
  });

  it("does not change what is expanded when a project is opened, so no row moves", async () => {
    await mount(`/m/${LOCAL}`);
    const expanded = () => all(".proj-x").map(el => el.getAttribute("aria-expanded"));
    const before = expanded();
    await click(all(".proj").find(el => el.textContent!.includes("Indigo Maps"))!);
    expect(document.querySelector(".proj[aria-current=page]")!.textContent).toContain("Indigo Maps");
    expect(expanded()).toEqual(before);
  });
});

describe("palette and modals", () => {
  it("makes the frame inert while open, searches, runs actions and returns", async () => {
    await mount(`/m/${LOCAL}`);
    const shell = document.querySelector(".ws-shell")!;
    expect(shell.hasAttribute("inert")).toBe(false);
    await press("k", { ctrlKey: true });
    expect(document.querySelector("dialog[open]")).toBeTruthy();
    expect(shell.hasAttribute("inert")).toBe(true);
    const input = document.querySelector<HTMLInputElement>(".ws-pal-in")!;
    input.value = "Cedar";
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await tick();
    expect(all(".ws-pal-o")[0].textContent).toContain("Cedar Notes");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await tick();
    expect(location.pathname).toBe(`/m/${LOCAL}/p/${CEDAR}/board`);
    expect(shell.hasAttribute("inert")).toBe(false);
    expect(document.querySelector("dialog[open]")).toBeNull();
    await press("k", { ctrlKey: true });
    const again = document.querySelector<HTMLInputElement>(".ws-pal-in")!;
    again.value = ">archive";
    again.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await tick();
    expect(all(".ws-pal-o").map(el => el.textContent)).toEqual([expect.stringContaining("Go to Archive")]);
    again.value = "zzzz";
    again.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await tick();
    expect(document.querySelector(".ws-pal-none")!.textContent).toBe("No matches");
  });

  it("does not run an action that is unavailable here", async () => {
    await mount(`/m/${LOCAL}`);
    await press("k", { ctrlKey: true });
    await click(all(".ws-pal-o").find(el => el.textContent!.includes("Show terminal"))!);
    expect(document.querySelector("dialog[open]")).toBeTruthy();
    expect(app().dataset.terminal).toBe("closed");
  });

  it("ignores shortcuts behind a modal", async () => {
    await mount(`/m/${LOCAL}`);
    await press("?");
    expect(document.querySelector("dialog[open]")!.textContent).toContain("Help and shortcuts");
    await press("b", { ctrlKey: true });
    expect(app().dataset.sidebar).toBe("shown");
  });
});

describe("board", () => {
  it("renders only a window of a long column while its counter stays exact", async () => {
    await mount(`/m/${REMOTE}/p/${MOSAIC}/board`);
    const column = document.querySelector('[aria-label="Not started"]')!;
    expect(column.querySelector(".col-h .n")!.textContent).toBe("217");
    expect(column.querySelectorAll(".card").length).toBeLessThan(20);
    expect(column.querySelector<HTMLElement>(".v-inner")!.style.height).not.toBe("0px");
  });

  it("virtualises the list view too", async () => {
    await mount(`/m/${REMOTE}/p/${MOSAIC}/list`);
    expect(all(".lrow").length).toBeLessThan(40);
    expect(document.querySelector(".ws-list .sec-h")!.textContent).toContain("Not started");
  });

  it("collapses an empty column to a strip whose counter agrees", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    for (const strip of all(".col.strip")) expect(strip.querySelector(".n")).toBeNull();
    for (const col of all(".col:not(.strip):not(.archive)")) {
      const n = col.querySelector(".col-h .n")?.textContent;
      if (n === undefined) expect(col.querySelectorAll(".card").length).toBe(0);
      else expect(Number(n)).toBeGreaterThanOrEqual(col.querySelectorAll(".card").length);
    }
  });

  it("keeps card actions in their own band, outside the card's content", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    expect(all(".card").length).toBeGreaterThan(0);
    for (const card of all(".card")) {
      expect(card.querySelector(".card-link .btn, .card-link button")).toBeNull();
      expect(card.querySelector(".card-f .card-a")).toBeTruthy();
    }
  });

  it("starts a draft, and the counters follow the same list", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    const count = (label: string) => document.querySelector(`[aria-label="${label}"] .col-h .n`)?.textContent ?? "0";
    const before = Number(count("Working"));
    const start = all(".card .btn").find(el => el.textContent === "Start")!;
    await click(start);
    expect(Number(count("Working"))).toBe(before + 1);
  });

  it("explains a rejected action and leaves the task where it was", async () => {
    // The domain refuses a second writer on a shared checkout; this fixture already has one in progress there.
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    const count = () => document.querySelector('[aria-label="Working"] .col-h .n')!.textContent;
    const before = count();
    await click(all(".card").find(c => c.textContent!.includes("Main checkout") && c.textContent!.includes("Start"))!.querySelector(".btn")!);
    expect(document.querySelector(".ws-receipt")!.textContent).toContain("Mock action blocked");
    expect(count()).toBe(before);
  });

  it("previews a card with Space without leaving the board, and Escape clears it", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    const link = document.querySelector<HTMLElement>(".card-link")!;
    link.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true }));
    await tick();
    expect(location.pathname).toBe(`/m/${LOCAL}/p/${CEDAR}/board`);
    expect(document.querySelector('.card[aria-current="true"]')).toBeTruthy();
    expect(document.querySelector(".inspector")!.textContent).toContain("Selected task");
    await press("Escape");
    expect(document.querySelector('.card[aria-current="true"]')).toBeNull();
  });

  it("filters to an honest empty state", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    await press("/");
    const field = document.querySelector<HTMLInputElement>("#ws-filter")!;
    field.value = "no such task anywhere";
    field.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await tick();
    expect(document.querySelector(".note-panel")!.textContent).toContain("No tasks match");
    expect(document.querySelector(".board-wrap")).toBeNull();
  });
});

describe("empty states", () => {
  it("offers a first step on an empty machine, and shows no stale counters", async () => {
    await mount("/", () => createMockWorkspace());
    expect(document.querySelector(".note-panel")!.textContent).toContain("No projects on this machine yet");
    expect(document.querySelector(".nav .count")).toBeNull();
    expect(document.querySelector(".sb-usage")).toBeNull();
  });

  it("agrees with the counters in a project without tasks", async () => {
    await mount(`/m/${LOCAL}/p/${HARBOR}/board`);
    expect(document.querySelector(".note-panel")!.textContent).toContain("No tasks in this project yet");
    await click(named(".nav", "Home"));
    const row = all(".home .tr").find(r => r.textContent!.includes("Harbor Sandbox"))!;
    expect(row.querySelectorAll(".none").length).toBeGreaterThanOrEqual(3);
    expect(row.textContent).not.toMatch(/\b0\b/);
  });

  it("labels an unknown limit as unavailable, never as zero", async () => {
    await mount(`/m/${LOCAL}`);
    expect(document.querySelector(".u-grid")!.textContent).toContain("Unavailable");
    expect(document.querySelector(".u-grid")!.textContent).toContain("42% used");
    expect(document.querySelector(".sb-usage")!.textContent).not.toContain("Claude");
    expect(document.querySelector(".sb-usage")!.textContent).toContain("Codex");
    expect(document.querySelector(".ws-app")!.textContent).not.toContain("—");
    expect(document.body.textContent).not.toMatch(/\b5h\b/);
  });
});

describe("creating work", () => {
  const describeTask = async (text: string) => {
    const area = document.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    area.value = text;
    area.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await tick();
  };

  it("requires a description, writes the title and lands the draft on the board", async () => {
    await mount(`/m/${LOCAL}/new?project=${CEDAR}`);
    await click(named(".create-foot .btn", "Save as draft"));
    expect(document.querySelector(".err")!.textContent).toContain("Describe what needs doing");
    expect(location.pathname).toBe(`/m/${LOCAL}/new`);
    await describeTask("Limit the export endpoint to sixty requests a minute.");
    await expect.poll(() => document.querySelector(".title-prev .tt")?.textContent).toBe("Limit the export endpoint to sixty requests a minute");
    await click(named(".create-foot .btn", "Save as draft"));
    expect(location.pathname).toBe(`/m/${LOCAL}/p/${CEDAR}/board`);
    expect(document.querySelector('[aria-label="Not started"] .col-h .n')!.textContent).not.toBe("0");
  });

  it("creates a sprint and opens it", async () => {
    await mount(`/m/${LOCAL}/new?kind=sprint&project=${CEDAR}`);
    await describeTask("Add an audit log export.");
    await click(named(".create-foot .btn", "Start planning"));
    expect(location.pathname).toMatch(/\/s\/[^/]+\/planning$/);
    expect(document.querySelector(".sp-conv .brief")!.textContent).toContain("Add an audit log export.");
    expect(document.querySelector(".sp-plan .note-panel")!.textContent).toContain("No plan yet");
  });
});

describe("theme", () => {
  it("changes only in Settings and defaults to System", async () => {
    await mount(`/m/${LOCAL}`);
    expect(document.querySelector(".sidebar")!.textContent).not.toMatch(/theme/i);
    expect(document.querySelector(".titlebar")!.textContent).not.toMatch(/theme/i);
    await click(document.querySelector('[aria-label="Settings"]')!);
    expect(location.pathname).toBe("/settings/app/appearance");
    expect(document.querySelector('[aria-label="Theme"] [aria-current="true"]')!.textContent).toBe("System");
    await click(named('[aria-label="Theme"] button', "Dark"));
    expect(localStorage.getItem("cw.theme")).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    await click(named('[aria-label="Theme"] button', "Light"));
    expect(document.documentElement.dataset.theme).toBe("light");
  });
});

describe("hard interface rules", () => {
  // Vitest blanks CSS imports, even with ?raw, so the audit reads the real files from disk.
  const source = async (name: string) => {
    const { readFileSync } = await import(/* @vite-ignore */ `node:${"fs"}`) as { readFileSync(path: URL, encoding: "utf8"): string };
    return readFileSync(new URL(name, import.meta.url), "utf8");
  };
  const allCss = async () => (await source("../../../../../../packages/workspace-ui/src/mockup-styles.css")) + (await source("../../../../../../packages/workspace-ui/src/workspace.css"));
  /** Selector and body of every rule, with at-rule wrappers flattened. */
  const rules = (css: string) => [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(m => ({ selector: m[1].trim(), body: m[2] }));

  it("never changes weight, size or box for a selected, hovered or focused state", async () => {
    const stateful = /\[aria-(current|selected|pressed)|:hover|:focus|:active|:checked/;
    const shifting = /(^|[;\s])(font-weight|font-size|letter-spacing|padding[a-z-]*|margin[a-z-]*|border-width|width|height|line-height)\s*:/;
    const audited = rules(await allCss()).filter(r => stateful.test(r.selector) && !/^\.(sw|rc|ck)\b/.test(r.selector) && !r.selector.startsWith("#pt-"));
    expect(audited.length).toBeGreaterThan(50);
    const breaking = audited.filter(r => shifting.test(r.body)).map(r => `${r.selector} { ${r.body.trim()} }`);
    expect(breaking).toEqual([]);
  });

  it("draws focus only for the keyboard", async () => {
    const css = await allCss(), focusRules = rules(css).filter(r => /:focus(?![-\w])/.test(r.selector));
    expect(focusRules.length).toBeGreaterThan(0);
    for (const rule of focusRules) expect(`${rule.selector} { ${rule.body} }`).not.toMatch(/outline:\s*(?!none)\S|box-shadow|background/);
    expect(await source("../../../../../../packages/workspace-ui/src/workspace.css")).toContain(":not(:has(:focus-visible))");
  });

  it("keeps the shared stylesheet identical to the approved mockups", async () => {
    expect(await source("../../../../../../packages/workspace-ui/src/mockup-styles.css")).toBe(await source("../../../../mockups/styles.css"));
  });
});
