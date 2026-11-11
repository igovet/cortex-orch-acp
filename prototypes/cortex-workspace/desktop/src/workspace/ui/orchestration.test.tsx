import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import Workspace, { MOCK_CONNECT_MS } from "./index";
import { hrefOf, parseRoute } from "@cortex/workspace-ui/router";

const LOCAL = "mock-machine-00000", REMOTE = "mock-machine-00001", GPU = "mock-machine-00002";
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const tick = () => new Promise(r => setTimeout(r, 0));
const wait = (ms: number) => new Promise(r => setTimeout(r, ms));
async function mount(path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace autoplay={false} />, root);
  await tick();
  return root;
}
const orch = (page = "", machine = LOCAL) => mount(`/settings/machine/${machine}/orchestration${page ? `/${page}` : ""}`);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const named = (selector: string, name: string) => all(selector).find(el => (el.getAttribute("aria-label") ?? el.textContent ?? "").includes(name))!;
const label = <T extends HTMLElement = HTMLInputElement>(name: string) => $<T>(`[aria-label="${name}"]`);
const click = async (el: Element) => { (el as HTMLElement).click(); await tick(); };
const type = async (el: HTMLInputElement | HTMLTextAreaElement, value: string) => { el.value = value; el.dispatchEvent(new Event("input", { bubbles: true })); el.dispatchEvent(new Event("change", { bubbles: true })); await tick(); };
const mock = (name: string) => click(named("#m-mock .mi", name));
const text = () => document.body.textContent ?? "";
const pageText = () => $(".set-main").textContent ?? "";
const tab = (name: string) => click(named('[role="tab"].tab', name));
const save = () => click($(".savebar .btn-primary"));

describe("routes", () => {
  it("round-trips every page, a server and a tab", () => {
    for (const path of ["defaults", "models", "routing", "effective", "policy", "tools", "profiles", "files"]) expect(hrefOf(parseRoute(`/settings/machine/m/orchestration/${path}`))).toBe(`/settings/machine/m/orchestration/${path}`);
    expect(parseRoute("/settings/machine/m/orchestration/tools/playwright", "?tab=sandbox")).toMatchObject({ page: "tools", server: "playwright", tab: "sandbox" });
    expect(hrefOf(parseRoute("/settings/machine/m/orchestration/tools/new", "?tab=connect"))).toBe("/settings/machine/m/orchestration/tools/new?tab=connect");
    expect(parseRoute("/settings/machine/m/orchestration/models/extra").name).toBe("notfound");
  });
});

describe("every field of the inventory is reachable", () => {
  it("Coordinator", async () => {
    await orch("defaults");
    for (const t of ["Model", "Effort", "Access mode", "Language", "Checkout for new tasks", "Status"]) expect(named(".srow .t", t)).toBeTruthy();
    expect(named(".srow", "Status").textContent).toContain("Bound to Claude · Opus");
    await click(named(".ws-adv-h", "Advanced"));
    expect(pageText()).toContain("Other coordinator parameters");
    await click(named(".btn", "Add parameter"));
    await type(label("Parameter value: name_1"), "plan");
    expect($(".savebar").textContent).toContain("1 unsaved");
    await click(named(".btn", "Use an exact route"));
    expect($('[aria-label^="Route model"]')).toBeTruthy();
    expect($('[aria-label^="Route connection"]')).toBeTruthy();
    expect(pageText()).not.toMatch(/Route id|xr_/);
    await click(named(".btn", "Clear"));
    expect(named(".srow .t", "Effort")).toBeTruthy();
  });
  it("Models: switches, tiers and the whole per-model policy", async () => {
    await orch("models");
    expect(all(".omrow:not(.th)").length).toBe(14);
    for (const name of ["Use Claude", "Claude in automatic routing", "Use Codex", "Codex in automatic routing", "Use OpenCode", "Haiku enabled", "Haiku in automatic routing"]) expect(label(name)).toBeTruthy();
    expect(named(".tier button", "Opus: Flagship").getAttribute("aria-pressed")).toBe("true");
    await click(named(".omrow", "Haiku"));
    expect(named(".omrow", "Haiku").getAttribute("aria-selected")).toBe("true");
    for (const name of ["Effort floor", "Max effort"]) expect($(`.ws-detail [aria-label^="${name}"]`)).toBeTruthy();
    expect(label("Haiku preferred for Writing").checked).toBe(true);
    expect($(".ws-detail").textContent).toContain("Prefer when");
    expect($(".ws-detail").textContent).toContain("Effort ladder");
    expect(label("Haiku also standard for Testing").checked).toBe(true);
    expect(label("Request name")).toBeTruthy();
    expect($(".ws-detail").textContent).not.toMatch(/Route id|xr_/);
    expect($(".ws-detail").textContent).toContain("Eligible");
    expect(pageText()).toContain("Observed 02:25");
    expect(named(".omrow", "GPT-5.6 Terra").textContent).toContain("Not confirmed by the engine yet");
    expect(pageText()).toContain("Not offered now");
    await click(label("Remove claude-opus-4-1"));
    expect(pageText()).not.toContain("claude-opus-4-1");
  });
  it("a stored value the model no longer offers stays visible and marked", async () => {
    await orch("models");
    await click(named(".omrow", "Sonnet"));
    await click(named("#m-effortFloor-claude-sonnet-5 .mi", "Low"));
    expect($(".savebar").textContent).toContain("unsaved");
    await click(named(".omrow", "Default"));
    expect($(".ws-detail .sub").textContent).toBe("default");
  });
  it("Routing: all 23 roles and the star, with the built-in facts locked", async () => {
    await orch("routing");
    const rows = all(".orrow:not(.th)");
    expect(rows.length).toBe(24);
    expect(rows[0].textContent).toContain("All roles");
    expect(named(".orrow", "Architect").textContent).toContain("deep / escalates to flagship");
    expect(named(".orrow", "Explorer").textContent).toContain("Investigation");
    expect(named(".orrow", "Build Verification").textContent).toContain("Built in");
    expect(named(".orrow", "Build Verification").querySelector("button")).toBeNull();
    expect(named(".orrow", "Security Auditor").textContent).toContain("Claude · Opus · High");
    await click(label("Edit pin: Debugger"));
    expect(pageText()).toContain("Strict. If this route is unavailable the work is refused, not moved.");
    await click(named("#m-pin-e-worker-debugger .mi", "Codex"));
    await click(named("#m-pin-m-worker-debugger .mi", "GPT-6.1 Terra"));
    await click(named("#m-pin-f-worker-debugger .mi", "High"));
    expect(named(".orrow", "Debugger").textContent).toContain("Codex · GPT-6.1 Terra · High");
    expect($(".savebar").textContent).toContain("unsaved");
  });
  it("Work kinds: lean, preferred models and stretch", async () => {
    await mount(`/settings/machine/${LOCAL}/orchestration/routing?tab=kinds`);
    expect(all(".okrow:not(.th)").length).toBe(13);
    expect(named(".okrow", "Frontend design implementation").textContent).toContain("Requires its engine");
    expect($('[aria-label^="debugging engine lean"]').textContent).toContain("Built in (no lean)");
    await click(named("#m-lean-debugging .mi", "Codex"));
    expect($('[aria-label^="debugging engine lean"]').textContent).toContain("Codex");
    await click(label("Remove GPT-6.1 Terra from Testing"));
    expect(label("Remove GPT-6.1 Terra from Testing")).toBeNull();
    const stretch = label<HTMLButtonElement>("Haiku stretches to Debugging");
    expect(stretch.getAttribute("aria-pressed")).toBe("false");
    await click(stretch);
    expect(label<HTMLButtonElement>("Haiku stretches to Debugging").getAttribute("aria-pressed")).toBe("true");
    const add = all("#m-pref-review .mi");
    expect(add.find(x => x.textContent?.includes("GPT-6 Luna"))?.getAttribute("aria-disabled")).toBeNull();
  });
  it("Quota: the fixed band, real windows, no 5-hour row, an unmapped report that can be mapped", async () => {
    await mount(`/settings/machine/${LOCAL}/orchestration/routing?tab=quota`);
    expect(pageText()).toContain("Hot at 95%: ranked behind cooler routes. 100%: excluded. There is no setting for this.");
    expect(label("Claude window 1").value).toBe("seven_day_opus");
    expect(pageText()).not.toContain("OpenCode");
    expect(pageText()).not.toMatch(/five_hour|5h|5-hour/);
    expect(pageText()).toContain("Counts for every model of this engine. 42% used.");
    await click(named(".srow .btn", "Map…"));
    expect(label("Codex window 1").value).toBe("seven_day");
    expect(named(".oqrow", "Remove window seven_day") ?? pageText()).toBeTruthy();
    await click(named('[aria-label="seven_day applies to"] button', "No models"));
    expect($(".savebar").textContent).toContain("unsaved");
    await click(label("Claude window 1"));
    await type(label("Claude window 2"), "seven_day_opus");
    expect(pageText()).toContain("Already listed.");
    expect($(".savebar").textContent).toContain("Fix 1 field");
  });
  it("Rules: deny tokens, orphans and the inert auto-allow", async () => {
    await mount(`/settings/machine/${LOCAL}/orchestration/routing?tab=rules`);
    expect(pageText()).toContain("gpt-5.5");
    expect(pageText()).toContain("Other entries");
    expect(pageText()).toContain("Stored, has no effect under current routing. Use Auto in Models.");
    const input = label("Deny token");
    await type(input, "*");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await tick();
    expect(pageText()).toContain("A wildcard needs an engine");
    await click(label("Remove gpt-5.5"));
    expect($(".savebar").textContent).toContain("unsaved");
  });
  it("Limits: every ceiling field, the stored ones read-only", async () => {
    await orch("policy");
    for (const t of ["Attempts per task", "Connections", "Data recipients", "Failure classes", "Budget", "Allow work to move to another provider"]) expect(named(".srow .t", t)).toBeTruthy();
    expect(label("Attempts per task").value).toBe("8");
    await click(label("Restrict Connections"));
    expect(pageText()).toContain("Nothing would be allowed.");
    await click(label("Connections: codex-acp"));
    expect(pageText()).not.toContain("Nothing would be allowed.");
    await click(named(".ws-adv-h", "Advanced"));
    for (const t of ["Allowed routes", "Fallback target routes"]) expect(named(".srow .t", t)).toBeTruthy();
    expect(pageText()).toContain("Stored, not applied");
    expect(named(".srow", "Tokens").textContent).toContain("2000000");
    expect(pageText()).toContain("Never a reason to fall back: cancellation, refusal, authorization_loss, provider_restriction.");
  });
  it("Connections gain status, rename and read-only detail", async () => {
    await mount(`/settings/machine/${LOCAL}/connections`);
    expect(pageText()).toContain("Engines: download and connect");
    expect(named(".srow", "OpenCode").textContent).toContain("OpenCode is never set up automatically");
    await click(named(".srow .btn", "Enable OpenCode"));
    await click(named("[data-bar=dialog] .btn-primary", "Enable"));
    expect(named(".srow", "OpenCode").textContent).toContain("Connected");
    expect(all("#m-conn-codex-acp .mi").map(m => m.textContent)).not.toContain("Rename…");
    await click(all(".crow .ws-edit").find(b => /codex/i.test(b.textContent ?? ""))!);
    const name = $<HTMLInputElement>(".crow .ws-edit-in");
    await type(name, "Work Codex");
    name.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await tick();
    expect(named(".crow", "Work Codex")).toBeTruthy();
    await click(named(".ws-adv-h", "Details of claude-agent-acp"));
    for (const t of ["Engine", "Provider", "Launch command", "Qualified"]) expect(pageText()).toContain(t);
    expect(pageText()).not.toContain("Connection id");
    expect(named(".btn", "Add connection")).toBeTruthy();
  });
  it("Profiles and Files", async () => {
    await orch("profiles");
    expect(named(".ofrow", "default").textContent).toContain("Active");
    expect(named(".ofrow", "night-runs").textContent).toContain("Stores only routing. Save to store every section.");
  });
  it("Files lists sources, preserved keys, presets and migration", async () => {
    await orch("files");
    expect(named(".oyrow", "Machine routing").textContent).toContain("In use");
    for (const t of ["<project>/.cortex/routing.json", "$CORTEX_ROUTING", "mcp-servers.json", "ssh-askpass.json", "owner_note", "CORTEX_ENGINE_UPDATE", "Default preset", "Model policy migration"]) expect(pageText()).toContain(t);
    expect(named(".srow .btn", "Apply preset").getAttribute("aria-disabled")).toBe("true");
    await click(named(".srow .btn", "Reset to default"));
    expect($("dialog[open]").textContent).toContain("A private backup of each is kept");
    await click(named("[data-bar=dialog] .btn-danger", "Reset"));
    expect(pageText()).toContain("2 private backups kept.");
  });
  it("Help is in English only", async () => {
    await orch("routing");
    const pop = () => all(".ws-hint-pop").find(p => p.getAttribute("aria-label") === "About role pins")!;
    expect(pop().textContent).not.toMatch(/[а-я]/);
    expect(pop().textContent).toMatch(/[a-z]/);
  });
});

describe("invented fields are gone", () => {
  it("no trace of the old settings on any page", async () => {
    for (const page of ["", "defaults", "models", "routing", "effective", "policy", "tools", "profiles", "files"]) {
      dispose?.(); document.body.innerHTML = "";
      await orch(page);
      expect(pageText()).not.toMatch(/Raise effort on retry|Floor and ceiling|Same engine first|Any allowed engine|Ask before work moves|Security Reviewer|Docs Writer|Frontend, QA|Designers|Fast and cheap|Deep review|project, read|Save mine over it|Project override/);
    }
  });
  it("shows no dash in place of a missing value", async () => {
    for (const page of ["models", "routing", "tools", "profiles", "files"]) {
      dispose?.(); document.body.innerHTML = "";
      await orch(page);
      for (const cell of all(".gr > span, .gr > span > span")) expect(["—", "-", "–", "n/a"]).not.toContain(cell.textContent?.trim());
    }
  });
});

describe("one draft per file", () => {
  it("Models and Routing share one draft, one save and no prompt between them", async () => {
    await orch("models");
    await click(label("Default in automatic routing"));
    expect($(".savebar").textContent).toContain("1 unsaved change in routing rules");
    await click(named(".snav.sub", "Routing"));
    expect(location.pathname).toContain("orchestration/routing");
    expect($("dialog[open]")).toBeNull();
    await click(label("Edit pin: Debugger"));
    await click(named(".ws-pin .btn", "Done"));
    const n = Number(/(\d+) unsaved/.exec($(".savebar").textContent!)![1]);
    expect(n).toBeGreaterThan(1);
    await click(named(".snav.sub", "Models"));
    expect(label("Default in automatic routing").checked).toBe(false);
    await save();
    expect(label("Default in automatic routing").checked).toBe(false);
    expect($(".savebar").textContent).toContain("Saved, but the project file still wins");
  });
  it("a save that differs from the profile says so and offers to store it", async () => {
    await orch("defaults");
    await click(named('[aria-label="Effort"] button', "High"));
    await save();
    expect($(".savebar").textContent).toContain("Saved on This machine. Not yet in profile default.");
    await click(named(".savebar .btn", "Save to profile"));
    expect($(".set-ctx").textContent).toContain("Applied");
  });
  it("Coordinator and Limits share one draft too", async () => {
    await orch("defaults");
    await click(named('[aria-label="Effort"] button', "High"));
    await click(named(".snav.sub", "Limits and fallback"));
    expect($("dialog[open]")).toBeNull();
    await type(label("Attempts per task"), "12");
    expect($(".savebar").textContent).toContain("2 unsaved changes in coordinator and limits");
  });
  it("a field error turns Save off and names the count", async () => {
    await orch("models");
    await click(named(".tier button", "Haiku: Light"));
    expect(named(".omrow", "Haiku").textContent).toContain("Choose a tier.");
    expect($(".savebar").textContent).toContain("Fix 1 field");
    expect($(".savebar .btn-primary").getAttribute("aria-disabled")).toBe("true");
    await click($(".savebar .btn-primary"));
    expect($(".savebar").textContent).toContain("Fix 1 field");
    await click(named(".tier button", "Haiku: Light"));
    expect($(".savebar")).toBeNull();
  });
  it("a conflict is reviewed before anything is written over it", async () => {
    await orch("defaults");
    await mock("Next save hits a conflict");
    await click(named('[aria-label="Language"] button', "English"));
    await save();
    expect($(".savebar").textContent).toContain("Changed elsewhere since you opened it. Nothing was written.");
    expect(all(".savebar .btn").map(b => b.textContent)).toEqual(["Discard mine", "Review changes"]);
    await click(named(".savebar .btn", "Review changes"));
    expect($(".ws-review").textContent).toContain("language");
    expect($(".ws-review").textContent).toContain("collision");
    await click(named(".savebar .btn", "Reapply mine"));
    expect($(".savebar").textContent).toContain("Saved on This machine");
  });
  it("an unknown result claims nothing until the machine is read again", async () => {
    await orch("defaults");
    await mock("Next save times out");
    await click(named('[aria-label="Effort"] button', "Low"));
    await save();
    expect($(".savebar").textContent).toContain("No answer. It may have been saved.");
    expect(all(".savebar .btn-primary").map(b => b.textContent)).toEqual(["Read again"]);
    await click(named(".savebar .btn-primary", "Read again"));
    expect($(".savebar")).toBeNull();
  });
});

describe("project scope", () => {
  const pickProject = async (name: string) => { await click(named('[aria-label="Scope"] button', "Project")); await click(named("#m-scope-project .mi", name)); };
  it("appears only where it applies", async () => {
    for (const [page, has] of [["models", true], ["routing", true], ["effective", true], ["policy", true], ["defaults", false], ["tools", false], ["profiles", false], ["files", false]] as const) {
      dispose?.(); document.body.innerHTML = "";
      await orch(page);
      expect(!!$('[aria-label="Scope"]'), page).toBe(has);
    }
  });
  it("saving creates the project's routing file; machine rules stop applying there", async () => {
    await orch("routing");
    await pickProject("Cedar Notes");
    expect(pageText()).toContain("Saving creates this project's own routing file. It replaces machine rules for this project, so it starts from them.");
    await click(label("Edit pin: Debugger"));
    await click(named(".ws-pin .btn", "Done"));
    await click(named(".savebar .btn-primary", "on Cedar Notes"));
    expect($(".savebar").textContent).toContain("Saved on Cedar Notes.");
    expect(pageText()).toContain("Editing the project's own routing file.");
    await click(named('[aria-label="Scope"] button', "Machine"));
    expect(pageText()).toContain("Project routing wins in");
    expect(pageText()).toContain("Cedar Notes");
  });
  it("a machine save a project hides says so, with the way to the source", async () => {
    await orch("models");
    await click(label("Default in automatic routing"));
    await save();
    expect($(".savebar").textContent).toContain("Saved, but the project file still wins in");
    await click(named(".savebar .btn", "Open source"));
    expect(location.pathname).toContain("/files");
  });
  it("the project ceiling shows machine, project and effective, and a wider value is refused at its field", async () => {
    await orch("policy");
    await pickProject("Cedar Notes");
    const row = () => named(".olrow", "Attempts per task");
    expect(row().textContent).toContain("Starts from the machine ceiling");
    await type(label("Project Attempts per task"), "20");
    expect(row().textContent).toContain("Must stay within the machine limit.");
    expect($(".savebar").textContent).toContain("Fix 1 field");
    await type(label("Project Attempts per task"), "3");
    expect(row().textContent).not.toContain("Must stay");
    await click(named(".savebar .btn-primary", "on Cedar Notes"));
    expect(named(".btn", "Remove project ceiling")).toBeTruthy();
    expect(all(".olrow")[1].textContent).toContain("3");
    await click(named(".btn", "Remove project ceiling"));
    await click(named("[data-bar=dialog] .btn-danger", "Remove"));
    expect(pageText()).toContain("This project has no ceiling of its own.");
  });
  it("$CORTEX_ROUTING turns every routing control off with the reason", async () => {
    await orch("models");
    await mock("$CORTEX_ROUTING is set");
    expect(pageText()).toContain("Routing comes from $CORTEX_ROUTING");
    expect(label("Haiku enabled").disabled).toBe(true);
    expect(named(".tier button", "Haiku: Light").getAttribute("aria-disabled")).toBe("true");
  });
});

describe("route preview", () => {
  it("answers for one role, says why others are out, and fills from the grid", async () => {
    await orch("effective");
    expect(all(".oprow:not(.th)").map(r => r.children[1].textContent)).toEqual(["Codex · GPT-6.1 Sol", "Claude · Sonnet", "Codex · GPT-6.1 Terra"]);
    expect(named(".srow", "Would run").textContent).toContain("Medium");
    expect(pageText()).toContain("Live availability, retries and plan hints are only known at dispatch.");
    expect(named(".oprow", "Sol").textContent).toContain("42%");
    expect(all(".oxrow").find(r => r.children[0].textContent === "Claude · Opus")?.textContent).toContain("Not in this tier");
    await click(named('[aria-label="View"] button', "All roles"));
    expect(all(".ogrow:not(.th)").length).toBe(23);
    await click(named(".ogrow .ws-cellbtn", "Fill the inputs: Security Auditor"));
    expect(named('[aria-label="View"] button', "This work").getAttribute("aria-current")).toBe("true");
    expect($('[aria-label^="Role:"]').textContent).toContain("Security Auditor");
    expect(all(".oprow:not(.th)")).toHaveLength(1);
    expect($(".oprow:not(.th)").textContent).toContain("Pinned, strict");
  });
  it("previews the draft before it is saved", async () => {
    await orch("models");
    await click(label("Use Codex"));
    await click(named(".snav.sub", "Route preview"));
    expect($("dialog[open]")).toBeNull();
    expect(named('[aria-label="Using"] button', "Saved").getAttribute("aria-current")).toBe("true");
    expect(all(".oprow:not(.th)").length).toBe(3);
    await click(named('[aria-label="Using"] button', "Draft"));
    expect(all(".oprow:not(.th)").map(r => r.children[1].textContent)).toEqual(["Claude · Sonnet"]);
  });
  it("keeps the fixed rules a click away, read-only", async () => {
    await orch("effective");
    await click(named(".ws-adv-h", "How the router decides"));
    for (const t of ["Tier fallback order", "Effort by tier", "Rank order"]) expect(named(".srow", t).textContent).toContain("Built in");
  });
  it("says when nothing can run", async () => {
    await orch("effective");
    await mock("Model policy needs migration");
    expect($(".note-box.bad").textContent).toContain("model_policy_migration_required");
  });
});

describe("MCP list", () => {
  it("shows the real servers with their state and access counts", async () => {
    await orch("tools");
    expect(all(".osrow:not(.th)").length).toBe(6);
    expect(named(".osrow", "legacy-bridge").textContent).toContain("Has settings this app does not manage.");
    expect(named(".osrow", "codebase-memory-mcp").textContent).toContain("15 writing · 11 read-only · 8 coordinator");
    expect(named(".osrow", "playwright").textContent).toContain("Claude");
    expect(named(".osrow", "codebase-memory-mcp").textContent).toContain("All");
    expect(named(".osrow", "broken-exporter").textContent).toContain("Will not start: a path does not exist");
  });
  it("turns a server on and off, and duplicates and removes through the one draft", async () => {
    await orch("tools");
    await click(label("vivaldi enabled"));
    expect(label("vivaldi enabled").checked).toBe(true);
    await click(named("#m-srv-vivaldi .mi", "Duplicate"));
    await type($<HTMLInputElement>("dialog[open] input"), "vivaldi-codex");
    await click(named("[data-bar=dialog] .btn-primary", "Duplicate"));
    expect(all(".osrow:not(.th)").length).toBe(7);
    await click(named("#m-srv-broken-exporter .mi", "Remove"));
    await click(named("[data-bar=dialog] .btn-danger", "Remove"));
    expect(all(".osrow:not(.th)").length).toBe(6);
    expect($(".savebar").textContent).toContain("unsaved changes in MCP servers");
    await save();
    expect($(".savebar").textContent).toContain("Saved on This machine");
    expect(all(".osrow:not(.th)").map(r => r.querySelector(".w500")?.textContent)).toContain("vivaldi-codex");
  });
  it("an existing server opens on its tools, and a missing one says so", async () => {
    await orch("tools");
    await click(named(".osrow", "vivaldi"));
    expect(location.pathname).toContain("/tools/vivaldi");
    expect(named('[role="tab"].tab', "Tools").getAttribute("aria-selected")).toBe("true");
    dispose?.(); document.body.innerHTML = "";
    await orch("tools/nothing");
    expect(pageText()).toContain("nothing is not on this machine.");
  });
});

describe("MCP tool access matrix", () => {
  const open = (name: string, tab = "tools") => mount(`/settings/machine/${LOCAL}/orchestration/tools/${name}?tab=${tab}`);
  const box = (tool: string, col: string) => $<HTMLInputElement>(`[aria-label^="${tool}, ${col},"]`);
  const row = (tool: string) => named(".omxrow", tool);
  it("shows what the configuration says, per role class", async () => {
    await open("codebase-memory-mcp");
    expect(box("search_graph", "coordinator").checked).toBe(true);
    expect(box("query_graph", "coordinator").checked).toBe(false);
    expect(box("query_graph", "read-only workers").checked).toBe(true);
    expect(box("index_repository", "read-only workers").checked).toBe(false);
    expect(box("index_repository", "writing workers").checked).toBe(true);
    expect(box("search_graph", "always load").checked).toBe(true);
    expect(row("delete_project").textContent).toContain("Remove a project's index");
  });
  it("cascades with a note, and every cascade counts in the save bar", async () => {
    await open("codebase-memory-mcp");
    await click(box("index_status", "writing workers"));
    expect(box("index_status", "read-only workers").checked).toBe(false);
    expect(box("index_status", "coordinator").checked).toBe(false);
    expect(box("index_status", "always load").checked).toBe(false);
    expect($(".ws-note").textContent).toBe("Also removed from read-only workers and the coordinator.");
    expect($(".savebar").textContent).toContain("unsaved");
    await click(box("index_repository", "coordinator"));
    expect(box("index_repository", "read-only workers").checked).toBe(true);
    expect($(".ws-note").textContent).toBe("Also given to read-only and writing workers.");
    expect(pageText()).toContain("Keep indexing for workers.");
  });
  it("the consultant is fixed at Never, in every row", async () => {
    await open("codebase-memory-mcp");
    const rows = all(".omxrow:not(.th)");
    expect(rows.length).toBe(15);
    for (const r of rows) { const cell = r.children[5]; expect(cell.textContent).toBe("Never"); expect(cell.querySelector("input")).toBeNull(); expect(cell.getAttribute("title")).toBe("The consultant gets no external servers"); }
  });
  it("Same as read-only is not editable until Choose, and Workers only turns the column off with the reason", async () => {
    await open("playwright");
    expect(box("browser_snapshot", "coordinator").disabled).toBe(true);
    expect(box("browser_snapshot", "coordinator").title).toContain("This server is for workers only");
    await click(named('[aria-label="Who gets this server"] button', "Coordinator and workers"));
    expect(box("browser_snapshot", "coordinator").title).toContain("Follows read-only");
    await click(named('[aria-label="Coordinator tools"] button', "Choose"));
    expect(box("browser_snapshot", "coordinator").disabled).toBe(false);
  });
  it("Check as gives the exact count for a role on an engine", async () => {
    await open("codebase-memory-mcp");
    const out = () => $(".ws-gets").textContent, why = () => named(".srow", "Check as").querySelector(".d")!.textContent;
    expect(label("codebase-memory-mcp enabled").checked).toBe(true);
    expect(out()).toBe("11 of 15 tools");
    expect(why()).toBe("The exact result for one role on one engine.");
    await click(named("#m-as-role-codebase-memory-mcp .mi", "Senior Consultant"));
    expect(out()).toBe("0 of 15 tools");
    expect(why()).toBe("The consultant gets no external servers.");
    await click(named("#m-as-role-codebase-memory-mcp .mi", "Coordinator"));
    expect(out()).toBe("8 of 15 tools");
    await click(named("#m-as-role-codebase-memory-mcp .mi", "Backend Dev"));
    expect(out()).toBe("15 of 15 tools");
    await click(label("Engine Codex"));
    await click(named("#m-as-engine-codebase-memory-mcp .mi", "Codex"));
    expect(out()).toBe("0 of 15 tools");
    expect(why()).toBe("Not offered on Codex. 15 granted below.");
  });
  it("Check as counts nothing while the server is off, and the grants stay editable", async () => {
    await open("codebase-memory-mcp");
    const out = () => $(".ws-gets").textContent, why = () => named(".srow", "Check as").querySelector(".d")!.textContent;
    await click(label("codebase-memory-mcp enabled"));
    expect(label("codebase-memory-mcp enabled").checked).toBe(false);
    expect(out()).toBe("0 of 15 tools");
    expect(why()).toBe("Server is off. 11 granted below.");
    await click(named("#m-as-role-codebase-memory-mcp .mi", "Coordinator"));
    expect(out()).toBe("0 of 15 tools");
    expect(why()).toBe("Server is off. 8 granted below.");
    expect(box("search_graph", "read-only workers").checked).toBe(true);
    expect(box("search_graph", "read-only workers").disabled).toBe(false);
    await click(box("delete_project", "read-only workers"));
    expect(box("delete_project", "read-only workers").checked).toBe(true);
    await click(named("#m-as-role-codebase-memory-mcp .mi", "Security Auditor"));
    expect(why()).toBe("Server is off. 12 granted below.");
    await click(label("codebase-memory-mcp enabled"));
    expect(out()).toBe("12 of 15 tools");
    await click(label("codebase-memory-mcp enabled"));
    expect(out()).toBe("0 of 15 tools");
  });
  it("engines are per server: the last one stays, and a new grant needs nothing else", async () => {
    await open("playwright");
    expect(label("Engine Claude").checked).toBe(true);
    expect(label("Engine Codex").checked).toBe(false);
    expect(label("Engine Claude").disabled).toBe(true);
  });
  it("New and Not offered read from the last check; a new tool has no access until granted", async () => {
    await open("docs-search");
    expect(row("get_index").textContent).toContain("New");
    expect(box("get_index", "writing workers").checked).toBe(false);
    expect(row("lookup_legacy").textContent).toContain("Not offered");
    expect(box("lookup_legacy", "writing workers").checked).toBe(true);
    await click(named('[aria-label="Show tools"] button', "Not offered"));
    expect(all(".omxrow:not(.th)").length).toBe(1);
    await click(label("Remove lookup_legacy"));
    expect(all(".omxrow:not(.th)").length).toBe(0);
    await click(named('[aria-label="Show tools"] button', "All"));
    await click(box("get_index", "writing workers"));
    await save();
    expect(box("get_index", "writing workers").checked).toBe(true);
  });
  it("sets many at once, filters, and suggests from hints only in the draft", async () => {
    await open("codebase-memory-mcp");
    await type(label("Search tools"), "index_");
    expect(all(".omxrow:not(.th)").length).toBe(3);
    await click(label("Select every shown tool"));
    await click(named("#m-bulk-codebase-memory-mcp .mi", "Writing only"));
    expect(box("index_status", "read-only workers").checked).toBe(false);
    await type(label("Search tools"), "");
    await click(named(".ws-mtool .btn", "Suggest from server"));
    expect($(".ws-note").textContent).toContain("Server says");
    expect(box("delete_project", "writing workers").checked).toBe(false);
    expect($(".savebar").textContent).toContain("unsaved");
  });
  it("Check tools reports the difference and changes nothing else; a failure keeps the grants", async () => {
    await open("playwright");
    await click(named(".ws-mtool .btn", "Check tools"));
    await wait(MOCK_CONNECT_MS + 20);
    expect(pageText()).toContain("2 new, 0 no longer offered. Nothing else changed.");
    expect($(".savebar")).toBeNull();
    expect(row("browser_pdf_save").textContent).toContain("New");
    await click(named("#m-edit-outcome .mi", "Timed out"));
    await click(named(".ws-mtool .btn", "Check tools"));
    await wait(MOCK_CONNECT_MS + 20);
    expect(pageText()).toContain("No answer in 20 seconds. Stored grants stay, and the matrix stays editable.");
    expect(box("browser_click", "writing workers").disabled).toBe(false);
  });
  it("is a grid for the keyboard, with a label on every cell", async () => {
    await open("vivaldi");
    expect($('[role="grid"]')).toBeTruthy();
    const first = box("get_windows_and_tabs", "read-only workers");
    first.focus();
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(document.activeElement).toBe(box("get_windows_and_tabs", "writing workers"));
    (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement).toBe(box("chrome_navigate", "writing workers"));
  });
});

describe("MCP edit", () => {
  const open = (name: string, tab: string) => mount(`/settings/machine/${LOCAL}/orchestration/tools/${name}?tab=${tab}`);
  it("edits the connection: name fixed, env secrets never shown, redacted arguments explained", async () => {
    await open("docs-search", "connection");
    expect($('[aria-label="Server name"]')).toBeNull();
    expect(pageText()).toContain("Value stored, never shown");
    expect(pageText()).toContain("Keeps its stored value. Replace the whole part to change it.");
    expect(pageText()).toContain("Command, arguments and environment are not part of profiles.");
    expect(named(".srow", "Transport").textContent).toContain("Local process (stdio)");
    await click(named('[aria-label="Value of DOCS_API_KEY"] button', "Replace"));
    await type(label("New value of DOCS_API_KEY"), "secret-value");
    expect(label("New value of DOCS_API_KEY").type).toBe("password");
    await save();
    expect(text()).not.toContain("secret-value");
    expect(pageText()).toContain("Value stored, never shown");
  });
  it("edits the command and arguments, and splits a pasted line", async () => {
    await open("vivaldi", "connection");
    await type(label("Paste a command line"), `node /opt/x/server.js --port "9 9"`);
    await click(named(".ws-line-r .btn", "Split"));
    expect(label("Command").value).toBe("node");
    expect(label("Argument 3").value).toBe("9 9");
    await type(label("Argument 1"), "/opt/y/server.js");
    await save();
    expect(label("Argument 1").value).toBe("/opt/y/server.js");
  });
  it("keeps a quoted option value in one argument", async () => {
    await open("vivaldi", "connection");
    await type(label("Paste a command line"), `server --directory="/tmp/a b"`);
    await click(named(".ws-line-r .btn", "Split"));
    expect(label("Command").value).toBe("server");
    expect(label("Argument 1").value).toBe("--directory=/tmp/a b");
    expect($('[aria-label="Argument 2"]')).toBeNull();
    expect(label("Paste a command line").value).toBe("");
    await type(label("Paste a command line"), "npx pkg \\\n  --port 9");
    await click(named(".ws-line-r .btn", "Split"));
    expect(label("Argument 1").value).toBe("pkg");
    expect(label("Argument 3").value).toBe("9");
  });
  it("refuses a line it cannot split, keeps every field, and clears the refusal on edit", async () => {
    await open("vivaldi", "connection");
    const command = label("Command").value, first = label("Argument 1").value, box = () => label("Paste a command line"), refusal = () => $('.ws-err[role="alert"]');
    await type(box(), `node --dir="/opt/a b`);
    await click(named(".ws-line-r .btn", "Split"));
    expect(refusal().textContent).toBe("A quote is not closed.");
    expect(box().getAttribute("aria-invalid")).toBe("true");
    expect(box().value).toBe(`node --dir="/opt/a b`);
    expect(label("Command").value).toBe(command);
    expect(label("Argument 1").value).toBe(first);
    expect($(".savebar")).toBeNull();
    await type(box(), `node --dir="/opt/a b"`);
    expect(refusal()).toBeNull();
    await click(named(".ws-line-r .btn", "Split"));
    expect(label("Argument 1").value).toBe("--dir=/opt/a b");
  });
  it("edits the sandbox, with each path checked", async () => {
    await open("vivaldi", "sandbox");
    await click(label("Network"));
    expect(label("Network").checked).toBe(false);
    await click(named(".btn", "Add write path"));
    await type(label("Write path 1"), "relative");
    expect(pageText()).toContain("Use a full path, starting with /.");
    expect($(".savebar").textContent).toContain("Fix 1 field");
    await type(label("Write path 1"), "/opt/missing/cache");
    expect(pageText()).toContain("The server is dropped from sessions until this exists.");
    const input = label("Runtime directory variable");
    await type(input, "home");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await tick();
    expect(pageText()).toContain("Use A-Z, 0-9, _.");
  });
  it("a server with settings this app does not manage can only be turned on, off or removed", async () => {
    await open("legacy-bridge", "tools");
    expect(pageText()).toContain("Unsupported: timeout_ms.");
    expect($<HTMLInputElement>('[aria-label^="ping, writing workers,"]').disabled).toBe(true);
    expect(label("legacy-bridge enabled").disabled).toBe(false);
    await click(named('[role="tab"].tab', "Connection"));
    expect(label("Command").disabled).toBe(true);
  });
  it("an offline machine shows everything and changes nothing", async () => {
    await mount(`/settings/machine/${REMOTE}/orchestration/tools/playwright?tab=tools`);
    expect($(":is(.strip.info, .l-strip[data-tone=info])").textContent).toContain("Nothing is queued");
    expect($<HTMLInputElement>('[aria-label^="browser_click, writing workers,"]').disabled).toBe(true);
    await click(named(".snav.sub", "Models"));
    expect(label("Opus enabled").disabled).toBe(true);
    expect(named(".btn", "Refresh model lists").getAttribute("aria-disabled")).toBe("true");
  });
});

describe("MCP add", () => {
  const start = () => mount(`/settings/machine/${LOCAL}/orchestration/tools/new`);
  const connect = async () => { await click(named(".btn", "Connect and list tools")); expect(pageText()).toContain("Starting the server…"); await wait(MOCK_CONNECT_MS + 20); };
  it("keeps later steps closed until the first passes, and names what is wrong", async () => {
    await start();
    expect(named('[role="tab"].tab', "2 Connect").getAttribute("aria-disabled")).toBe("true");
    await type(label("Server name"), "cortex");
    expect(pageText()).toContain("Reserved");
    await type(label("Server name"), "playwright");
    expect(pageText()).toContain("Already exists");
    await type(label("Server name"), "search-tools");
    await type(label("Command"), "/opt/tools/search");
    expect(named('[role="tab"].tab', "2 Connect").getAttribute("aria-disabled")).toBeNull();
  });
  it("runs define, connect, tool access and saves off, with no tool granted by default", async () => {
    await start();
    expect(named("#m-add-template .mi", "playwright")).toBeTruthy();
    await type(label("Server name"), "search-tools");
    await type(label("Command"), "/opt/tools/search");
    await click(named(".btn", "Next: Connect"));
    expect(pageText()).toContain("Starts the server in its sandbox and lists its tools. It calls none of them.");
    expect(pageText()).toContain("Simulated: no server is started.");
    await connect();
    expect(pageText()).toContain("Connected");
    expect(pageText()).toContain("4 tools");
    await click(named(".btn", "Go to Tool access"));
    const boxes = () => all<HTMLInputElement>('[aria-label*=", writing workers,"]');
    expect(boxes().length).toBe(4);
    expect(boxes().every(b => !b.checked)).toBe(true);
    expect(named(".btn", "Save, keep off").getAttribute("aria-disabled")).toBe("true");
    expect(pageText()).toContain("Grant at least one tool.");
    await click(named(".ws-mtool .btn", "Suggest from server"));
    expect(boxes().filter(b => b.checked).length).toBe(3);
    await click(named(".btn", "Save, keep off"));
    expect(location.pathname).toContain("/tools/search-tools");
    expect(named('[role="tab"].tab', "Tools").getAttribute("aria-selected")).toBe("true");
    expect(pageText()).toContain("Last check");
    expect(label("search-tools enabled").checked).toBe(false);
    expect($<HTMLInputElement>('[aria-label^="write_item, writing workers,"]').checked).toBe(true);
    await click(named(".lnk", "All servers"));
    expect(all(".osrow:not(.th)").length).toBe(7);
  });
  it("starts from a known server and can save it on", async () => {
    await start();
    await click(named("#m-add-template .mi", "vivaldi"));
    expect(label("Command").value).toBe("/usr/bin/node");
    await type(label("Server name"), "vivaldi-2");
    await click(named(".btn", "Next: Connect"));
    await connect();
    expect(pageText()).toContain("11 tools");
    await click(named(".btn", "Go to Tool access"));
    await click($('[aria-label^="chrome_navigate, writing workers,"]'));
    await click(named(".btn", "Save and turn on"));
    expect(label("vivaldi-2 enabled").checked).toBe(true);
  });
  it("each failure has its own fixed text and a way on, including tool names by hand", async () => {
    await start();
    await type(label("Server name"), "search-tools");
    await type(label("Command"), "/opt/tools/search");
    await click(named(".btn", "Next: Connect"));
    for (const [outcome, message] of [["Command not found", "Command not found."], ["Exited at start", "The server closed at once."], ["Timed out", "No answer in 20 seconds."], ["Path refused", "A path was refused"], ["Needs a variable", "missing environment variable"]] as const) {
      await click(named("#m-add-outcome .mi", outcome));
      await click(named(".btn", all(".ws-connect .btn").some(b => b.textContent === "Retry") ? "Retry" : "Connect and list tools"));
      await wait(MOCK_CONNECT_MS + 20);
      expect($('.ws-connect [role="alert"]').textContent).toContain(message);
    }
    await click(named("#m-add-outcome .mi", "Not supported"));
    await click(named(".btn", "Retry"));
    await wait(MOCK_CONNECT_MS + 20);
    expect(pageText()).toContain("This machine's runtime cannot list tools.");
    const input = label("Tool name");
    await type(input, "find_things");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await tick();
    await click(all(".btn").filter(b => b.textContent === "Go to Tool access").at(-1)!);
    expect(named(".omxrow", "find_things").textContent).toContain("Not checked");
    expect($<HTMLInputElement>('[aria-label^="find_things, writing workers,"]').checked).toBe(true);
    await click(named(".btn", "Save, keep off"));
    expect(location.pathname).toContain("/tools/search-tools");
  });
  it("an offline machine can only be told so", async () => {
    await mount(`/settings/machine/${REMOTE}/orchestration/tools`);
    expect(named(".btn", "Add server").getAttribute("aria-disabled")).toBe("true");
  });
});

describe("profiles", () => {
  it("the context bar reads the same everywhere and says in words what differs", async () => {
    await orch("profiles");
    expect($(".set-ctx").textContent).toContain("Modified (1)");
    expect($("#m-ctx-diff").textContent).toContain("Pin for Security Auditor: Opus");
    expect(named(".srow", "Modified from default")).toBeTruthy();
    await click(named("#m-ctx-diff .mi", "Revert to saved"));
    expect($(".set-ctx").textContent).toContain("Applied");
  });
  it("switching with changes offers save, discard and cancel, and says drafts go", async () => {
    await orch("defaults");
    await click(named('[aria-label="Effort"] button', "High"));
    await click(named("#m-ctx-profile .mi", "night-runs"));
    const dialog = $("dialog[open]");
    expect(dialog.textContent).toContain("routing");
    expect(dialog.textContent).toContain("1 unsaved draft is dropped either way.");
    await click(named("[data-bar=dialog] .btn", "Cancel"));
    expect($(".savebar")).toBeTruthy();
    await click(named("#m-ctx-profile .mi", "night-runs"));
    await click(named("[data-bar=dialog] .btn-danger", "Discard and switch"));
    expect($(".savebar")).toBeNull();
    expect($('[aria-label^="Profile:"]').textContent).toContain("night-runs");
  });
  it("names follow the real rule, and the active profile cannot be deleted", async () => {
    await orch("profiles");
    await click(named(".btn", "Save as new"));
    await type($<HTMLInputElement>("dialog[open] input"), "Bad Name");
    await click(named("[data-bar=dialog] .btn-primary", "Save as new"));
    expect($("dialog[open]")).toBeTruthy();
    await type($<HTMLInputElement>("dialog[open] input"), "weekend-1");
    await click(named("[data-bar=dialog] .btn-primary", "Save as new"));
    expect(named(".ofrow", "weekend-1").textContent).toContain("Active");
    expect(named("#m-prof-profile-weekend-1 .mi", "Delete").getAttribute("aria-disabled")).toBe("true");
    await click(named("#m-prof-default .mi", "Delete"));
    await click(named("[data-bar=dialog] .btn-danger", "Delete"));
    expect(all(".ofrow:not(.th)").length).toBe(2);
  });
  it("an interrupted switch offers the three ways out", async () => {
    await orch("profiles");
    await mock("Profile switch interrupted");
    expect($(".set-ctx").textContent).toContain("Interrupted");
    for (const t of ["Activate again", "Save current settings to it", "Detach"]) expect(named(".note-box .btn", t)).toBeTruthy();
    await click(named(".note-box .btn", "Detach"));
    expect($(".set-ctx").textContent).toContain("No profile");
  });
});

describe("overview", () => {
  it("summarises from real values and says where the rules come from", async () => {
    await orch();
    expect(named(".srow", "Models").textContent).toContain("Claude: 6 of 8 models on, 5 automatic");
    expect(named(".srow", "Limits and fallback").textContent).toContain("Attempts 8, fallback off");
    expect(named(".srow", "Tools and MCP").textContent).toContain("3 of 6 servers on, 43 tools granted");
    expect(named(".srow", "Routing file in use").textContent).toContain("~/.config/cortex/routing.json");
  });
  it("puts what needs attention first, each linked to its fix", async () => {
    await orch("", GPU);
    expect(named(".set-h", "Needs attention")).toBeTruthy();
    await click(named(".srow", "The model policy needs migration."));
    expect(location.pathname).toContain("/files");
    await click(named(".srow .btn", "Preview"));
    expect(pageText()).toContain("Legacy tiers go: haiku: light.");
    expect(named(".srow .btn-primary", "Apply").getAttribute("aria-disabled")).toBe("true");
    await click(label("I reviewed the migration"));
    await click(named(".srow .btn-primary", "Apply"));
    expect(pageText()).toContain("This machine is on model policy 2.");
  });
  it("an invalid routing file is said on every routing page", async () => {
    await orch("routing");
    await mock("Routing file is invalid");
    expect($(".note-box.bad").textContent).toContain("Every dispatch is refused until this is repaired.");
  });
});
