import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type MachineConfig, type Project, type RoutingRules, type UserDefaults, type WorkspaceCommand, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort } from "./support";

/**
 * The orchestration settings are organised as the Cortex settings panel: nine sections in its order, one frame on each
 * (context bar, profile bar, strips, save bar), one draft per file. These tests walk the sections on a port that offers
 * everything, with the machine's configuration sent as the service sends it.
 */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string) => all("button, a").find(b => b.textContent?.trim() === text || b.getAttribute("aria-label") === text);
const title = () => $("h1.set-title")?.textContent;
const bar = () => $(".savebar")?.textContent ?? "";
const M = `/settings/machine/${LOCAL_MACHINE_ID}`;

const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
function world(patch?: (c: MachineConfig) => void): WorkspaceState {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(project);
  const c = state.machines[0].config;
  c.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus", efforts: ["medium", "high"], observed: true }];
  c.setup["claude-agent-acp"].status = "connected";
  c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
  c.surfaces.routing.providers = ["claude-agent-acp"];
  c.surfaces.user_defaults = { ...c.surfaces.user_defaults, coordinator: { engine: "claude-agent-acp", model: "opus", effort: "high", parameters: {} } };
  patch?.(c);
  return state;
}
async function open(path: string, state = world()) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settled();
  s.emit(projection("1", state));
  await settled();
  return s;
}
const last = (s: ReturnType<typeof scriptedPort>) => s.commands.at(-1)!.payload as Extract<WorkspaceCommand, { type: "save_surface" }>;
const go = async (text: string) => { (all('nav[aria-label="Settings"] a.snav').find(t => t.textContent?.trim() === text) as HTMLElement).click(); await settled(); };
/** A settings table takes its roles from its header row a frame after it is drawn. */
const frames = () => new Promise(resolve => setTimeout(resolve, 60));
const current = () => all('nav[aria-label="Settings"] .snav.sub[aria-current="page"]').map(a => a.textContent);

const SECTIONS = ["Defaults", "Coordinator models", "Models", "Engines and models", "Project ceiling", "Routing", "Profiles", "MCP servers", "Connections"];

describe("orchestration settings, organised as the Cortex settings panel", () => {
  it("lists the nine sections in the panel's order and opens Defaults on the bare route", async () => {
    await open(`${M}/orchestration`);
    expect(all('nav[aria-label="Settings"] .snav.sub').map(a => a.textContent?.trim())).toEqual(SECTIONS);
    expect(title()).toBe("Defaults");
    expect(current()).toEqual(["Defaults"]);
    // Page links, not tabs: nothing promises arrow keys, and the open one is the current page.
    expect(all('nav[aria-label="Settings"] [role="tab"], nav[aria-label="Settings"] [aria-selected]')).toEqual([]);
    // Engine connections is no longer an item of its own beside Orchestration.
    expect(all('nav[aria-label="Settings"] .snav:not(.sub)').map(a => a.textContent?.trim())).not.toContain("Engine connections");
  });

  it("lands each existing link on the section or page it names", async () => {
    const cases: [string, string, string][] = [
      ["orchestration/defaults", "Defaults", "Defaults"], ["orchestration/coordinator", "Coordinator models", "Coordinator models"], ["orchestration/models", "Models", "Models"], ["orchestration/engines", "Engines and models", "Engines and models"],
      ["orchestration/ceiling", "Project ceiling", "Project ceiling"], ["orchestration/routing?tab=quota", "Routing", "Routing"], ["orchestration/effective", "Route preview", "Routing"],
      ["orchestration/policy", "Defaults", "Defaults"], ["orchestration/tools", "MCP servers", "MCP servers"], ["orchestration/profiles", "Profiles", "Profiles"],
      ["orchestration/files", "Files and sources", "Routing"], ["connections", "Connections", "Connections"],
    ];
    for (const [path, heading, selected] of cases) {
      await open(`${M}/${path}`);
      expect(title(), path).toBe(heading);
      expect(document.body.textContent, path).not.toContain("Tools and MCP");
      expect(current(), path).toEqual([selected]);
      if (path.endsWith("policy")) {
        expect($("#set-machine-ceiling"), path).toBeTruthy();
        // The page scrolls to Machine ceiling, and the keyboard goes there with it.
        expect(document.activeElement?.tagName, path).toBe("H2");
        expect(document.activeElement?.textContent, path).toBe("Machine ceiling");
      }
      if (path === "orchestration/effective" || path === "orchestration/files") expect(all("a.ws-back").map(a => a.textContent), path).toEqual(["Back to Routing"]);
      dispose?.(); dispose = undefined; document.body.innerHTML = "";
    }
  });

  it("puts the profile bar on every section and the project picker only on the sections that can act on a project", async () => {
    const pages: [string, boolean][] = [["orchestration/defaults", false], ["orchestration/coordinator", true], ["orchestration/models", true], ["orchestration/engines", true], ["orchestration/ceiling", true], ["orchestration/routing", true], ["orchestration/profiles", false], ["orchestration/tools", true], ["connections", false]];
    for (const [path, scoped] of pages) {
      await open(`${M}/${path}`);
      expect($(".set-prof"), path).toBeTruthy();
      expect(button("Save as new…"), path).toBeTruthy();
      expect(!!$('[popovertarget="m-scope-project"]'), path).toBe(scoped);
      dispose?.(); dispose = undefined; document.body.innerHTML = "";
    }
  });

  it("keeps the connection state in the context bar at every width, dropping only caption words and the machine name", async () => {
    // The drawn word may go on a phone; what is read out is its unseen copy, the same at every width.
    const state = () => { const st = $('[aria-label="Machine and project"] .ws-ctx-state'); return { text: st?.textContent?.trim(), hidden: !!st?.matches(".l-compact-hide, .l-tight-hide"), drawnHidden: st?.querySelector(".ws-ctx-word")?.getAttribute("aria-hidden"), read: st?.nextElementSibling?.matches(".ws-live") ? st.nextElementSibling.textContent : null }; };
    const s = await open(`${M}/orchestration/models`);
    expect(state()).toEqual({ text: "Connected", hidden: false, drawnHidden: "true", read: "Connected" });
    // The words that go first on a narrow bar are the captions and the machine's name, which the sidebar also shows.
    expect(all('[aria-label="Machine and project"] .l-compact-hide, [aria-label="Machine and project"] .l-tight-hide').map(e => e.textContent?.trim())).toEqual(["This machine", "Project"]);
    // This machine is connected exactly when its service is: the port says it went away.
    s.emit({ kind: "connection", connection: { state: "disconnected", reason: null, at: null } });
    await settled();
    expect(state()).toEqual({ text: "Offline", hidden: false, drawnHidden: "true", read: "Offline" });
  });

  it("Defaults: shows each recommendation and saves the defaults draft with its own button", async () => {
    const s = await open(`${M}/orchestration/defaults`);
    expect(all(".srow .d.rec").map(d => d.textContent)).toContain("Recommended: A strong general model; it plans and checks the work.");
    expect($(".set-rec")?.textContent).toMatch(/^Recommended: /);
    all(".seg button").find(b => b.textContent === "English")!.click();
    await settled();
    expect(bar()).toContain("1 unsaved change in defaults");
    button("Save defaults on This machine")!.click();
    await settled();
    expect(last(s)).toMatchObject({ type: "save_surface", surface: "user_defaults" });
    expect((last(s).value as UserDefaults).language).toBe("en");
    s.receipt(true);
    await settled();
    expect(bar()).toContain("Saved on This machine.");
  });

  it("Defaults: clearing an exact route restores its Codex coordinator", async () => {
    const s = await open(`${M}/orchestration/defaults`, world(c => {
      c.catalogue["codex-acp"].models = [{ id: "gpt-6.2-sol", label: "Sol", efforts: ["medium"], observed: true }];
      c.setup["codex-acp"].status = "connected";
      c.surfaces.routing.engines["codex-acp"].enabled = ["gpt-6.2-sol"];
      c.surfaces.routing.providers.push("codex-acp");
      c.surfaces.user_defaults.coordinator = { engine: "codex-acp", model: "gpt-6.2-sol", effort: "medium", parameters: {} };
    }));
    const status = () => $('[aria-labelledby="set-coord-h"] .st')?.textContent;
    const bound = status();
    expect(bound).toMatch(/^Bound to .*Sol$/);

    button("Exact default route")!.click();
    button("Use an exact route")!.click();
    await settled();
    expect(status()).toBe(`${bound}, exact route`);

    button("Clear")!.click();
    await settled();
    expect(status()).toBe(bound);

    // The route round trip leaves no change; make a separate edit to inspect the save payload.
    button("English")!.click();
    button("Save defaults on This machine")!.click();
    await settled();
    expect(last(s).value).toMatchObject({
      routeDefault: null,
      coordinator: { engine: "codex-acp", model: "gpt-6.2-sol" },
    });
  });

  it("Models: switches a model off and saves the routing rules", async () => {
    const s = await open(`${M}/orchestration/models`);
    expect(all(".set-grp .set-h").map(h => h.textContent)).toContain("Claude1 of 1 on");
    $<HTMLInputElement>('input[aria-label="Opus enabled"]')!.click();
    await settled();
    button("Save routing rules on This machine")!.click();
    await settled();
    expect(last(s)).toMatchObject({ type: "save_surface", surface: "routing" });
    expect((last(s).value as RoutingRules).engines["claude-agent-acp"].enabled).toEqual([]);
  });

  it("Engines and models: automatic needs a tier in words before the save is offered", async () => {
    const s = await open(`${M}/orchestration/engines`);
    $<HTMLInputElement>('input[aria-label="Pick Opus automatically"]')!.click();
    await settled();
    expect(document.body.textContent).toContain("Choose a tier.");
    const save = button("Save routing rules on This machine")!;
    expect(save.getAttribute("aria-disabled")).toBe("true");
    expect(save.dataset.why).toBe("Fix 1 field");
    button("Opus: Standard")!.click();
    await settled();
    expect(button("Opus: Standard")!.getAttribute("aria-pressed")).toBe("true");
    button("Save routing rules on This machine")!.click();
    await settled();
    const value = last(s).value as RoutingRules;
    expect(value.engines["claude-agent-acp"].auto).toEqual(["opus"]);
    expect(value.engines["claude-agent-acp"].tiers.opus).toEqual(["standard"]);
  });

  it("keeps one routing draft across Models, Engines and models and Routing, with no prompt between them", async () => {
    await open(`${M}/orchestration/models`);
    $<HTMLInputElement>('input[aria-label="Opus enabled"]')!.click();
    await settled();
    await go("Engines and models");
    expect(title()).toBe("Engines and models");
    expect(all<HTMLDialogElement>("dialog").filter(d => d.open || d.hasAttribute("open")).map(d => d.textContent)).toEqual([]);
    expect(bar()).toContain("1 unsaved change in routing rules");
    await go("Routing");
    expect(title()).toBe("Routing");
    expect(bar()).toContain("1 unsaved change in routing rules");
  });

  it("writes nothing over a change made elsewhere until the owner has reviewed it", async () => {
    const s = await open(`${M}/orchestration/defaults`);
    all(".seg button").find(b => b.textContent === "Russian")!.click();
    await settled();
    button("Save defaults on This machine")!.click();
    await settled();
    s.receipt(false, undefined, "revision_conflict");
    await settled();
    expect(bar()).toContain("Changed elsewhere since you opened it. Nothing was written.");
    expect(button("Reapply mine")).toBeUndefined();
    button("Review changes")!.click();
    await settled();
    expect($(".ws-review")?.textContent).toContain("Changed by you");
    expect(button("Reapply mine")).toBeTruthy();
    expect(s.commands.filter(c => c.operation === "save_surface")).toHaveLength(1);
  });

  it("is read-only offline: every write and profile action is off with its reason, and nothing is sent", async () => {
    const s = await open(`${M}/orchestration/models`);
    // This machine is connected exactly when its service is: the port says it went away.
    s.emit({ kind: "connection", connection: { state: "disconnected", reason: null, at: null } });
    await settled();
    expect(document.body.textContent).toContain("This machine is offline, so settings are read-only");
    expect(document.body.textContent).toContain("Nothing is queued.");
    expect($<HTMLInputElement>('input[aria-label="Opus enabled"]')!.disabled).toBe(true);
    for (const label of ["Save to profile", "Save as new…"]) {
      expect(button(label)!.getAttribute("aria-disabled"), label).toBe("true");
      expect(button(label)!.dataset.why, label).toContain("offline");
    }
    button("Save as new…")!.click();
    await settled();
    expect(s.commands).toHaveLength(0);
  });

  it("Project ceiling: says what to do with no project chosen, then edits that project's ceiling", async () => {
    await open(`${M}/orchestration/ceiling`);
    expect(document.body.textContent).toContain("Choose a project in the bar above to read or set its ceiling.");
    expect($(".savebar")).toBeNull();
    all("#m-scope-project .mi").find(b => b.textContent?.includes("Cedar"))!.click();
    await settled();
    expect(document.body.textContent).toContain("This project has no ceiling of its own. Saving creates one.");
    const field = $<HTMLInputElement>('input[aria-label="Project Attempts per task"]')!;
    field.value = "4";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    await settled();
    expect(button("Save project ceiling on Cedar")).toBeTruthy();
  });

  it("Routing: shows its groups in the panel's order, and the writer's ladder follows complexity", async () => {
    await open(`${M}/orchestration/routing`);
    expect(all(".set-in > .set-grp").map(g => g.id)).toEqual(["set-files", "set-policy", "set-roles", "set-kinds", "set-quota", "set-deny", "set-router"]);
    const writer = all(".gr.orrow").find(r => r.textContent?.includes("Technical Writer"))!;
    expect(writer.textContent).toContain("Low: Light, normal: Standard, high: Deep");
    expect(button("Pin: Technical Writer")?.textContent).toBe("Pin…");
    expect(document.body.textContent).toContain("Project, then environment, then user. Files never merge.");
  });

  it("Connections: sits in the orchestration frame and points to Models", async () => {
    await open(`${M}/connections`);
    expect(title()).toBe("Connections");
    expect($(".set-prof")).toBeTruthy();
    expect(all("a").find(a => a.textContent === "Models")?.getAttribute("href")).toBe(`${M}/orchestration/models`);
  });
});

describe("orchestration settings, for assistive technology", () => {
  const headers = (table: Element) => [...table.querySelectorAll('[role="columnheader"]')].map(h => h.textContent?.trim());
  const lines = (table: Element) => [...table.children].map(c => c.getAttribute("role"));

  it("Project ceiling: the limits read as a table, each field in its row and column", async () => {
    await open(`${M}/orchestration/ceiling`);
    all("#m-scope-project .mi").find(b => b.textContent?.includes("Cedar"))!.click();
    await settled(); await frames();
    const table = $('.set-in [role="table"]')!;
    expect(table).toBeTruthy();
    expect(headers(table)).toEqual(["Limit", "Machine", "Project", "Effective"]);
    expect(new Set(lines(table))).toEqual(new Set(["row"]));
    const field = $<HTMLInputElement>('input[aria-label="Project Attempts per task"]')!;
    const row = field.closest('[role="row"]')!;
    expect(field.closest('[role="cell"]')?.getAttribute("data-label")).toBe("Project");
    expect(row.querySelector('[role="cell"]')?.textContent).toBe("Attempts per task");
  });

  it("Engines and models: the senior consultation kind is its own chip and says it uses the Design list until set", async () => {
    await open(`${M}/orchestration/engines`);
    button("Details of Opus")!.click();
    await settled(); await frames();
    const chip = $<HTMLInputElement>('input[aria-label="Opus preferred for Senior consultation"]')!;
    expect(chip).toBeTruthy();
    expect(chip.closest("label")!.textContent).toContain("Senior Consultant");
    expect(chip.closest("label")!.textContent).toContain("uses Design list until set");
  });

  it("Engines and models: the model table stays a table with a model's details open", async () => {
    await open(`${M}/orchestration/engines`);
    button("Details of Opus")!.click();
    await settled(); await frames();
    const table = $('.set-in [role="table"]')!;
    expect(table).toBeTruthy();
    expect(headers(table)).toEqual(["Model", "Automatic", "Tiers", "Preferred work", ""]);
    expect(new Set(lines(table))).toEqual(new Set(["row"]));
    const detail = $(".ws-mdetail")!;
    expect(detail.getAttribute("role")).toBe("cell");
    expect(detail.parentElement?.parentElement).toBe(table);
    expect($<HTMLInputElement>('input[aria-label="Pick Opus automatically"]')!.closest('[role="cell"]')?.getAttribute("data-label")).toBe("Automatic");
  });

  it("Engines and models: edits a tier's effort range and saves it as tier efforts", async () => {
    const s = await open(`${M}/orchestration/engines`, world(c => {
      c.surfaces.routing.engines["claude-agent-acp"].auto = ["opus"];
      c.surfaces.routing.engines["claude-agent-acp"].tiers = { opus: ["deep"] };
    }));
    button("Details of Opus")!.click();
    await settled();
    const item = (pop: string, text: string) => all<HTMLButtonElement>(`[id^="${pop}"] [role="menuitem"]`).find(b => b.textContent?.startsWith(text))!;
    expect(all(`[id^="m-range-min-deep-"] [role="menuitem"]`).map(b => b.textContent)).toEqual(["No range", "Medium", "High"]);
    item("m-range-min-deep-", "Medium").click();
    await settled();
    item("m-range-max-deep-", "High").click();
    await settled();
    button("Save routing rules on This machine")!.click();
    await settled();
    expect((last(s).value as RoutingRules).engines["claude-agent-acp"].policy.opus.tierEfforts).toEqual({ deep: { min: "medium", max: "high" } });
  });

  it("Defaults: the coordinator is offered only models with a tier, at the efforts their tiers allow", async () => {
    await open(`${M}/orchestration/defaults`, world(c => {
      c.catalogue["claude-agent-acp"].models.push({ id: "haiku", label: "Haiku", efforts: ["low", "medium", "high"], observed: true }, { id: "sonnet", label: "Sonnet", efforts: ["low", "medium", "high", "xhigh"], observed: true });
      const r = c.surfaces.routing.engines["claude-agent-acp"];
      r.enabled = ["opus", "haiku", "sonnet"];
      r.tiers = { opus: ["flagship"], sonnet: ["deep"] };
      r.policy = { sonnet: { effortFloor: null, maxEffort: null, effortLadder: null, alsoStandardFor: null, preferredFor: null, preferredWhen: null, requestNames: null, tierEfforts: { deep: { min: "xhigh", max: "xhigh" } } } };
      c.surfaces.user_defaults.coordinator = { engine: "claude-agent-acp", model: "haiku", effort: "medium", parameters: {} };
    }));
    const options = () => all('#m-def-model [role="menuitem"]').map(b => [b.querySelector(".grow")?.firstChild?.textContent, b.getAttribute("aria-disabled")]);
    // The saved coordinator outside the profile stays shown, but cannot be picked again.
    expect(options()).toEqual([["Haiku", "true"], ["Opus", null], ["Sonnet", null]]);
    expect($('[aria-labelledby="set-coord-h"] .st')?.textContent).toContain("This model has no tier in Models, so it cannot coordinate.");
    all<HTMLButtonElement>('#m-def-model [role="menuitem"]').find(b => b.textContent?.startsWith("Sonnet"))!.click();
    await settled();
    expect(all('[aria-labelledby="set-coord-h"] .seg button').map(b => b.textContent)).toEqual(["Extra high"]);
    expect(options().map(o => o[0])).toEqual(["Opus", "Sonnet"]);
  });

  it("Coordinator models: adds a model, ticks its categories, sets each range and saves it as coordinator models", async () => {
    const s = await open(`${M}/orchestration/coordinator`, world(c => {
      c.catalogue["claude-agent-acp"].models.push({ id: "sonnet", label: "Sonnet", efforts: ["low", "medium", "high", "xhigh"], observed: true });
      c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus", "sonnet"];
      c.surfaces.routing.coordinatorModels = [{ engine: "claude-agent-acp", model: "opus", tiers: { flagship: { min: "medium", max: "high" } } }];
    }));
    expect(title()).toBe("Coordinator models");
    expect($("#set-cm-h")?.textContent).toBe("Models the coordinator may use");
    const item = (pop: string, text: string) => all<HTMLButtonElement>(`[id^="${pop}"] [role="menuitem"]`).find(b => b.textContent?.startsWith(text))!;
    // Only switched-on models that are not listed yet can be added.
    expect(all('#m-cm-add [role="menuitem"]').map(b => b.querySelector(".grow")?.firstChild?.textContent)).toEqual(["Sonnet"]);
    item("m-cm-add", "Sonnet").click();
    await settled();
    const tick = (label: string) => all<HTMLInputElement>("input.chk").find(i => i.getAttribute("aria-label") === label)!;
    expect(tick("Sonnet coordinates Standard").checked).toBe(true);
    tick("Sonnet coordinates Deep").click();
    await settled();
    expect(all(`[id^="m-cm-min-deep-claude-agent-acp-sonnet"] [role="menuitem"]`).map(b => b.textContent)).toEqual(["Low", "Medium", "High", "Extra high"]);
    item("m-cm-min-deep-claude-agent-acp-sonnet", "Extra high").click();
    await settled();
    item("m-cm-min-standard-claude-agent-acp-sonnet", "High").click();
    await settled();
    item("m-cm-max-standard-claude-agent-acp-sonnet", "Extra high").click();
    await settled();
    tick("Opus coordinates Frontier").click();
    await settled();
    // Opus has no category left: the save waits for one.
    expect(all(".ws-err").map(e => e.textContent)).toContain("Choose at least one category.");
    tick("Opus coordinates Frontier").click();
    await settled();
    button("Save routing rules on This machine")!.click();
    await settled();
    expect(last(s)).toMatchObject({ type: "save_surface", surface: "routing" });
    expect((last(s).value as RoutingRules).coordinatorModels).toEqual([
      { engine: "claude-agent-acp", model: "opus", tiers: { flagship: { min: "high", max: "high" } } },
      { engine: "claude-agent-acp", model: "sonnet", tiers: { deep: { min: "xhigh", max: "xhigh" }, standard: { min: "high", max: "xhigh" } } },
    ]);
  });

  it("Coordinator models: a profile without the list says so and starts it from the worker tiers", async () => {
    const s = await open(`${M}/orchestration/coordinator`, world(c => { c.surfaces.routing.engines["claude-agent-acp"].tiers = { opus: ["flagship"] }; }));
    expect($("#set-cm-legacy-h")?.textContent).toBe("No coordinator list yet");
    expect(document.body.textContent).toContain("the coordinator may use any model with a tier under Engines and models");
    button("Start the list")!.click();
    await settled();
    button("Remove Opus from coordinator models")!.click();
    await settled();
    expect(all(".ws-err").map(e => e.textContent)).toContain("Keep at least one model, so a task can start.");
    expect(s.commands.filter(c => c.payload.type === "save_surface")).toEqual([]);
  });

  it("Defaults: the coordinator is offered only listed coordinator models, at their ranges", async () => {
    await open(`${M}/orchestration/defaults`, world(c => {
      c.catalogue["claude-agent-acp"].models.push({ id: "sonnet", label: "Sonnet", efforts: ["low", "medium", "high", "xhigh"], observed: true });
      const r = c.surfaces.routing.engines["claude-agent-acp"];
      r.enabled = ["opus", "sonnet"];
      // Worker tiers name Opus; the coordinator list names only Sonnet.
      r.tiers = { opus: ["flagship"] };
      c.surfaces.routing.coordinatorModels = [{ engine: "claude-agent-acp", model: "sonnet", tiers: { standard: { min: "high", max: "xhigh" } } }];
    }));
    const options = () => all('#m-def-model [role="menuitem"]').map(b => [b.querySelector(".grow")?.firstChild?.textContent, b.getAttribute("aria-disabled")]);
    expect(options()).toEqual([["Opus", "true"], ["Sonnet", null]]);
    expect($('[aria-labelledby="set-coord-h"] .st')?.textContent).toContain("This model is not in Coordinator models, so it cannot coordinate.");
    expect(document.body.textContent).toContain("Sonnet (Standard)");
    all<HTMLButtonElement>('#m-def-model [role="menuitem"]').find(b => b.textContent?.startsWith("Sonnet"))!.click();
    await settled();
    expect(all('[aria-labelledby="set-coord-h"] .seg button').map(b => b.textContent)).toEqual(["High", "Extra high"]);
  });

  it("puts each group heading right under the page title, at level 2", async () => {
    await open(`${M}/orchestration/defaults`);
    expect(all(".set-in h1").map(h => h.textContent)).toEqual(["Defaults"]);
    const groups = all(".set-in .set-h .l-sect-t");
    expect(groups.length).toBeGreaterThan(3);
    expect(new Set(groups.map(h => h.tagName))).toEqual(new Set(["H2"]));
    expect(all(".set-in h3")).toEqual([]);
  });
});
