import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { capture, emptyWorkspace, LOCAL_MACHINE_ID, routeIdOf, type EngineConnection, type MachineConfig, type Worker, type Message } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { SIDEBAR_DEFAULT, SIDEBAR_MIN } from "../src/prefs";
import { capabilitiesWire, flush, refOf, scriptedTransport, snapshot, stoppedMerge, wireDecision, wireTask } from "./support";

/** The screens on the real port, with a machine that answers as the helper does, so what they show is what real data gives. */
const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"];
const repo = { projectId: "p", remote: "", tree: ["a.txt"], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "dev", defaultBranch: "main",
  checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "dev", path: "/cedar", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }] };
const worker = (patch: Partial<Worker>): Worker => ({ id: "w1", taskId: "task-1", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Connect the board", now: null, waitsFor: null, scope: [], route: { model: "opus", effort: "high" }, attempt: { n: 1, of: 1 }, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [{ sequence: 1, text: "Reading the board", kind: "note", at: null }], result: null, ...patch } as Worker);

function machine(tasks = [wireTask({ executionId: "e", phase: "execution", runtimeState: "EXECUTING", executionState: "running" })], workers: Worker[] = [], more: { version?: string; connections?: EngineConnection[]; config?: (config: MachineConfig) => void } = {}) {
  const config = structuredClone(emptyWorkspace().machines[0].config);
  config.surfaces.user_defaults = { ...config.surfaces.user_defaults, coordinator: { engine: "claude-agent-acp", model: "opus", effort: "high", parameters: {} }, mode: "auto", language: "ru" };
  config.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus", efforts: ["medium", "high"], observed: true }];
  // A route is offered only from a connected engine with the model enabled, as on a real machine.
  config.setup["claude-agent-acp"].status = "connected";
  config.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
  more.config?.(config);
  const product = {tasks:{},sprints:[],messages:[] as Message[]};
  const history: Record<string,unknown>[] = [];
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call", "executions.control"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(OFFERED),
    "project.snapshot": () => snapshot(String(4 + history.length), tasks),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
    "executions.prompt": message => {
      history.push({historyId:"h"+history.length,sequence:String(history.length+1),revision:"0",kind:"user",text:(message as {text:string}).text,toolStatus:null,availability:"available"});
      return {outcome:"recorded"};
    },
    "executions.control": () => ({outcome:"recorded"}),
    "history.page": () => ({items:history,nextCursor:null,highWatermark:String(history.length)}),
    "executions.get": () => ({ executionId: "e", taskId: "task-1", version: "2", runtimeTaskId: "r", acpSessionId: "s", runtimePlanRevision: "0", ownerEpoch: "3", state: "result", warnings: [], resultId: "res" }),
    "results.get": () => ({ kind: "answer", resultId: "res", taskId: "task-1", executionId: "e", version: "1", text: "Done", evidence: [], checks: [{ name: "pytest", outcome: "passed", evidenceRef: null }], observedWorkspaceDigest: null }),
    "tasks.create": message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "task-2", entityVersion: "1", recordedAt: "2026-11-05T00:00:00Z" }),
    "tasks.start": message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "task-2", entityVersion: "2", recordedAt: "2026-11-05T00:00:00Z" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params?: {action:string;message:Message;messageId:string} };
      if (op === "product.message" && params) {
        if (params.action === "add") product.messages.push(params.message);
        if (params.action === "remove") product.messages = product.messages.filter(m => m.id !== params.messageId);
        return {ok:true,result:{}};
      }
      if (op === "repo.snapshot") return { ok: true, result: repo };
      if (op === "product.read") return { ok: true, result: product };
      if (op === "workers") return { ok: true, result: workers };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: more.version ?? "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: more.connections ?? [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      return { ok: true, result: {} };
    },
  });
  return s;
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

async function open(s: ReturnType<typeof machine>, path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  await settled();
  return root;
}
const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
// A text button by its words, or an icon button, such as the paper plane, by its accessible name.
const button = (text: string, within = "body") => all(`${within} button`).find(b => b.textContent?.trim().startsWith(text) || b.getAttribute("aria-label") === text);
/** Chooses Queue as the delivery, then presses the plane, which now reads Queue message. */
const queue = () => { button("Queue", ".mc-delivery")!.click(); button("Queue message")!.click(); };
/** Opens the folded team line of the conversation. */
const openTeam = async () => { all(".timeline button.fold").find(b => /working|finished/.test(b.textContent ?? "") && b.getAttribute("aria-expanded") === "false")?.click(); await settled(); };
const type = (label: string, value: string) => { const el = $<HTMLTextAreaElement>(`[aria-label="${label}"]`)!; el.value = value; el.dispatchEvent(new Event("input", { bubbles: true })); };
const prompts = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "executions.prompt").map(c => (c.message as { text: string }).text);
const TASK = `/m/${LOCAL_MACHINE_ID}/t/task-1`;

it("opts in only identified controls on the desktop frame and task composer", async () => {
  await open(machine(), TASK);
  for (const label of ["Sidebar", "Terminal", "Attach files", "Queue"]) {
    expect(button(label), label).toBeTruthy();
    expect(button(label)!.hasAttribute("data-tooltip"), label).toBe(true);
    expect(button(label)!.hasAttribute("title"), label).toBe(false);
  }
  expect(button("Add project")?.hasAttribute("data-tooltip")).toBe(false);
  expect($(".ws-tooltip-host")).not.toBeNull();
});

describe("machine overview, read when a screen shows it", () => {
  const ops = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "machine.call").map(c => (c.message as { op: string }).op);
  const go = async (to: string) => { history.pushState(null, "", to); window.dispatchEvent(new PopStateEvent("popstate")); await settled(); };
  /** The overview answers with values unlike every default, and only when the test lets it. */
  const overviewHeld = (s: ReturnType<typeof machine>, fail = 0) => {
    const answer = s.answers["machine.call"]!;
    let open!: () => void, failures = fail;
    const gate = new Promise<void>(resolve => { open = resolve; });
    s.answers["machine.call"] = async message => {
      if ((message as { op: string }).op !== "overview") return answer(message);
      await gate;
      if (failures-- > 0) return { ok: false, code: "machine_busy", message: "busy" };
      return { ok: true, result: { name: "Desk", retentionDays: 12, autoArchiveDays: 4, worktreeRoot: "~/t", version: "1", diskMb: 3, storage: [{ label: "Workspace state", mb: 3 }], cleanup: null, diagnostics: null, ghAvailable: true, reclamationFailures: 0 } };
    };
    return { open: () => open() };
  };
  const stepper = (label: string) => $(`.stepper[aria-label="${label}"] .v`)?.textContent;

  it("opens without asking for the overview, then reads it once when Settings, Storage opens", async () => {
    const s = machine(), held = overviewHeld(s);
    const root = await open(s, `/m/${LOCAL_MACHINE_ID}/p/p`);
    // The first screen is up on the settings read alone; storage accounting was never asked for.
    expect(root.querySelector(".l-startup")).toBeNull();
    expect(root.querySelector(".titlebar")).toBeTruthy();
    expect(ops(s)).toContain("config.read");
    expect(ops(s)).not.toContain("overview");
    await go(`/settings/machine/${LOCAL_MACHINE_ID}/storage`);
    expect(ops(s).filter(op => op === "overview")).toHaveLength(1);
    // While it is on its way the page says so, never the stand-in 30 and 7 days or an empty size.
    expect(stepper("Retention")).toBeUndefined();
    expect(root.textContent).toContain("Reading this machine's details…");
    expect(root.textContent).not.toContain("No cleanup has run");
    held.open(); await settled();
    expect(stepper("Retention")).toBe("12 days");
    expect(stepper("Auto archive")).toBe("4 days");
    expect(root.textContent).toContain("Workspace state");
    expect(root.textContent).toContain("3 MB in total");
    expect(root.textContent).not.toContain("Reading this machine's details…");
    // Other screens that show it share the read already made.
    await go(`/settings/machine/${LOCAL_MACHINE_ID}/overview`);
    await go(`/m/${LOCAL_MACHINE_ID}/archive`);
    expect(root.textContent).toContain("Kept for 12 days after archiving");
    expect(ops(s).filter(op => op === "overview")).toHaveLength(1);
  });

  it("saves the background purge and attachment age together with the archive rules", async () => {
    const s = machine();
    const original = s.answers["machine.call"]!;
    let rules = {retentionDays: 30, autoArchiveDays: 7, automaticArchivePurge: true, unreferencedAttachmentRetentionDays: 1};
    s.answers["machine.call"] = async message => {
      const {op, params} = message as {op: string; params: {patch: typeof rules}};
      const reply = await original(message);
      if (op === "machine.update") rules = { ...rules, ...params.patch };
      if (op === "overview") return {ok: true, result: { ...(reply as {result: object}).result, ...rules }};
      return reply;
    };
    await open(s, `/settings/machine/${LOCAL_MACHINE_ID}/storage`);
    const purge = $<HTMLInputElement>('input[aria-label="Delete expired archives automatically"]')!;
    expect(purge.checked).toBe(true);
    expect(stepper("Unreferenced attachments")).toBe("1 day");
    purge.click();
    button("More: Unreferenced attachments")!.click();
    button("Save")!.click();
    await settled();
    expect(s.calls.find(c => c.operation === "machine.call" && (c.message as {op: string}).op === "machine.update")?.message)
      .toMatchObject({params: {patch: {
        retentionDays: 30, autoArchiveDays: 7, automaticArchivePurge: false, unreferencedAttachmentRetentionDays: 2,
      }}});
    expect(purge.checked).toBe(false);
    expect(stepper("Unreferenced attachments")).toBe("2 days");
  });

  it("reads the overview for a shown inspector Machine panel, and not for a hidden one", async () => {
    localStorage.setItem("cw.ws.inspectorHidden", "true");
    const hidden = machine();
    await open(hidden, `/m/${LOCAL_MACHINE_ID}`);
    expect($(".titlebar")).toBeTruthy();
    expect(ops(hidden)).not.toContain("overview");
    dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear();
    const s = machine(), held = overviewHeld(s);
    await open(s, `/m/${LOCAL_MACHINE_ID}`);
    expect(ops(s).filter(op => op === "overview")).toHaveLength(1);
    const kept = () => [...document.querySelectorAll(".inspector dt")].find(dt => dt.textContent === "Archive kept")?.nextElementSibling?.textContent;
    expect(kept()).toBe("Reading this machine's details…");
    held.open(); await settled();
    expect(kept()).toBe("12 days");
  });

  it("never holds the interface back on a failed overview read, says so where it is shown, and retries", async () => {
    vi.useFakeTimers();
    try {
      const s = machine(), held = overviewHeld(s, 1);
      held.open();
      history.replaceState(null, "", `/settings/machine/${LOCAL_MACHINE_ID}/storage`);
      const root = document.createElement("div"); document.body.append(root);
      dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
      await vi.advanceTimersByTimeAsync(0); await vi.advanceTimersByTimeAsync(0);
      // The interface is open and usable; only the overview's own fields wait, saying why.
      expect(root.querySelector(".titlebar")).toBeTruthy();
      expect(ops(s).filter(op => op === "overview")).toHaveLength(1);
      expect(root.textContent).toContain("This machine's details could not be read yet. Retrying automatically.");
      expect(stepper("Retention")).toBeUndefined();
      expect([...root.querySelectorAll("button")].some(b => b.textContent === "Run cleanup now")).toBe(true);
      await vi.advanceTimersByTimeAsync(3100);
      expect(ops(s).filter(op => op === "overview")).toHaveLength(2);
      expect(stepper("Retention")).toBe("12 days");
      expect(root.textContent).not.toContain("could not be read yet");
    } finally { vi.useRealTimers(); }
  });
});

describe("engine availability", () => {
  it("keeps a connected engine visible when the service has no usage row", async () => {
    const s = machine();
    const answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      const reply = answer(message) as {ok:boolean;result:Record<string,unknown>};
      if ((message as {op:string}).op === "config.read") reply.result.connections = [
        {id:"c",engine:"claude",label:"Claude",note:"",sessions:1,state:"connected",checkedAt:null,detail:{}},
        {id:"o",engine:"opencode",label:"OpenCode",note:"",sessions:1,state:"connected",checkedAt:null,detail:{}},
      ];
      return reply;
    };
    await open(s, `/settings/machine/${LOCAL_MACHINE_ID}/connections`);
    expect(all(".urow").some(r => r.textContent?.includes("Claude") && r.textContent.includes("Not reported"))).toBe(true);
    // OpenCode has no weekly usage source: no limits row, never a misleading usage row.
    expect(all(".urow").some(r => r.textContent?.includes("OpenCode"))).toBe(false);
    expect(document.querySelector(".sb-usage")?.textContent).toContain("Not reported");
    expect(document.querySelector(".sb-usage")?.textContent).not.toContain("OpenCode");
  });

  it("shows Claude unavailable rather than hiding it in Limits", async () => {
    const s = machine();
    const answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      const reply = answer(message) as {ok:boolean;result:Record<string,unknown>};
      if ((message as {op:string}).op === "config.read") reply.result.usage = [
        {engine:"codex",connected:true,usedPercent:24,resetAt:null,observedAt:null,window:"weekly"},
        {engine:"claude",connected:true,usedPercent:null,resetAt:null,observedAt:null,window:"weekly"},
      ];
      return reply;
    };
    await open(s, `/settings/machine/${LOCAL_MACHINE_ID}/connections`);
    const rows = all(".urow").filter(r => !r.classList.contains("th"));
    expect(rows.some(r => /Claude/.test(r.textContent ?? "") && /Not reported/.test(r.textContent ?? ""))).toBe(true);
    expect(rows.some(r => /Codex/.test(r.textContent ?? "") && /24% used/.test(r.textContent ?? ""))).toBe(true);
  });

  const ops = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "machine.call").map(c => (c.message as { op: string }).op);
  /** A machine whose `config.read` answers whatever `usage` holds at the time of the read. */
  const reporting = (usage: { value: unknown[] }) => {
    const s = machine();
    const answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      const reply = answer(message) as {ok:boolean;result:Record<string,unknown>};
      if ((message as {op:string}).op === "config.read") reply.result.usage = structuredClone(usage.value);
      return reply;
    };
    return s;
  };
  const RESET = "2026-11-17T02:59:00Z";
  const claude = (observedAt: string, windows: {name: string; usedPercent: number}[]) => ({
    engine: "claude", connected: true, usedPercent: Math.max(...windows.map(w => w.usedPercent)), resetAt: RESET, observedAt, window: "weekly",
    windows: windows.map(w => ({ ...w, resetAt: RESET, observedAt })),
  });
  const codex = (usedPercent: number, observedAt: string) => ({ engine: "codex", connected: true, usedPercent, resetAt: RESET, observedAt, window: "weekly" });
  const bars = () => all(".sb-usage .um").map(b => b.textContent?.replace(/\s+/g, " ").trim());
  const limitRows = () => all(".urow").filter(r => !r.classList.contains("th")).map(r => r.textContent ?? "");

  it("shows Claude's general weekly window and its Fable window separately everywhere", async () => {
    const at = new Date().toISOString();
    const s = reporting({ value: [claude(at, [{ name: "seven_day", usedPercent: 67 }, { name: "seven_day_fable", usedPercent: 2 }]), codex(33, at)] });
    await open(s, `/settings/machine/${LOCAL_MACHINE_ID}/connections`);
    expect(bars()).toEqual(["Claude67%", "Fable2%", "Codex33%"]);
    expect($(".sb-usage")?.getAttribute("aria-label")).toBe("Engine limits: Claude all models 67%, Claude Fable 2%, Codex 33% used this week");
    const rows = limitRows();
    expect(rows.some(r => r.includes("This week, all models") && r.includes("67% used"))).toBe(true);
    expect(rows.some(r => r.includes("This week, Fable") && r.includes("2% used"))).toBe(true);
    expect(rows.some(r => r.includes("Codex") && r.includes("33% used"))).toBe(true);
    expect(rows.every(r => !r.includes("Stale"))).toBe(true);
    const card = [...document.querySelectorAll("#m-usage .u-eng")].find(e => e.textContent?.includes("Claude"))!;
    expect([...card.querySelectorAll(".u-win")].map(w => w.textContent)).toEqual([expect.stringContaining("All models67% used"), expect.stringContaining("Fable2% used")]);
    expect(card.querySelector(".u-src")?.textContent).toMatch(/^As of \d\d:\d\d, reported by the provider$/);
  });

  it("gives each sidebar limit its own line, so three windows fit the narrowest and the default sidebar without overlap", async () => {
    const at = new Date().toISOString();
    const s = reporting({ value: [claude(at, [{ name: "seven_day", usedPercent: 100 }, { name: "seven_day_fable", usedPercent: 2 }]), codex(33, at)] });
    await open(s, `/m/${LOCAL_MACHINE_ID}`);
    expect(bars()).toEqual(["Claude100%", "Fable2%", "Codex33%"]);
    // jsdom lays nothing out, so the fit is read from the rules the engine lays the bar out by.
    const css = readFileSync(fileURLToPath(import.meta.url).replace(/test\/[^/]*$/, "src/mockup-styles.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    const rule = (selector: string) => [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].find(m => m[1].trim() === selector)?.[2] ?? "";
    const box = rule(".sb-usage"), line = rule(".um"), name = rule(".um > :first-child"), missing = rule(".um > .na"), value = rule(".um .tnum");
    // Every window is one line of the button's grid, its name, bar and value in three columns shared by all lines,
    // never one more cell squeezed into a single row (where "70%" was drawn over "Fable" and "33%" was cut at the edge).
    expect(box).not.toMatch(/grid-auto-flow:\s*column/);
    expect(line).toMatch(/display:\s*contents/);
    const columns = /grid-template-columns:\s*minmax\(0, max-content\) minmax\((\d+)px, 1fr\) max-content/.exec(box);
    expect(columns).not.toBeNull();
    expect(box).not.toMatch(/(^|;)\s*height:/);
    // A long name is cut with an ellipsis inside its own column; a value and "Not reported" are never cut or wrapped.
    expect(name).toMatch(/min-width:\s*0/); expect(name).toMatch(/overflow:\s*hidden/); expect(name).toMatch(/text-overflow:\s*ellipsis/); expect(name).toMatch(/white-space:\s*nowrap/);
    expect(value).toMatch(/white-space:\s*nowrap/);
    expect(missing).toMatch(/grid-column:\s*2 \/ -1/);
    // The widest line, with a generous 0.62 em per character at 12 px, fits the narrowest sidebar (and so the default one).
    const pad = 2 * Number(/padding:\s*\d+px (\d+)px/.exec(box)![1]), gap = Number(/column-gap:\s*(\d+)px/.exec(box)![1]), em = 12 * .62;
    const text = (el: Element | null | undefined) => (el?.textContent ?? "").length * em;
    const widest = Math.max(...all(".sb-usage .um").map(u => {
      const [label, ...rest] = [...u.children];
      return pad + text(label) + gap + (u.querySelector(".na") ? text(u.querySelector(".na")) : Number(columns![1]) + gap + text(rest.at(-1)));
    }));
    for (const width of [SIDEBAR_MIN, SIDEBAR_DEFAULT]) expect(widest).toBeLessThanOrEqual(width);
    // Unreported, the line's words take the bar and value columns together.
    const t = new Date().toISOString();
    const unreported = reporting({ value: [claude(t, [{ name: "seven_day", usedPercent: 40 }]), codex(33, t)] });
    dispose?.(); document.body.innerHTML = "";
    await open(unreported, `/m/${LOCAL_MACHINE_ID}`);
    const fable = all(".sb-usage .um").find(u => u.textContent?.startsWith("Fable"))!;
    expect(fable.querySelector(".na")?.textContent).toBe("Not reported");
    expect(pad + text(fable.firstElementChild) + gap + text(fable.querySelector(".na"))).toBeLessThanOrEqual(SIDEBAR_MIN);
  });

  it("says Not reported for a Fable window the provider did not report, never zero", async () => {
    const s = reporting({ value: [claude(new Date().toISOString(), [{ name: "seven_day", usedPercent: 40 }])] });
    await open(s, `/settings/machine/${LOCAL_MACHINE_ID}/connections`);
    expect(bars()).toEqual(["Claude40%", "FableNot reported"]);
    expect(limitRows().some(r => r.includes("This week, Fable") && r.includes("Not reported"))).toBe(true);
    expect(limitRows().some(r => r.includes("This week, Fable") && /\d+%/.test(r))).toBe(false);
  });

  it("marks a limit snapshot older than fifteen minutes as stale and says when it was taken", async () => {
    const old = new Date(Date.now() - 2 * 3600_000).toISOString();
    const s = reporting({ value: [claude(old, [{ name: "seven_day", usedPercent: 54 }, { name: "seven_day_fable", usedPercent: 0 }])] });
    await open(s, `/settings/machine/${LOCAL_MACHINE_ID}/connections`);
    expect(limitRows().filter(r => /Stale, as of \d\d:\d\d/.test(r))).toHaveLength(2);
    expect(all(".sb-usage .um.stale")).toHaveLength(2);
    expect($(".sb-usage")?.getAttribute("aria-label")).toContain("some values are stale");
  });

  it("replaces an older limit value with a newer one while the app stays open", async () => {
    vi.useFakeTimers();
    try {
      const usage = { value: [claude(new Date().toISOString(), [{ name: "seven_day", usedPercent: 54 }, { name: "seven_day_fable", usedPercent: 0 }]), codex(30, new Date().toISOString())] };
      const s = reporting(usage);
      history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}`);
      const root = document.createElement("div"); document.body.append(root);
      dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
      for (let i = 0; i < 4; i++) await vi.advanceTimersByTimeAsync(0);
      expect(bars()).toEqual(["Claude54%", "Fable0%", "Codex30%"]);
      const reads = ops(s).filter(op => op === "config.read").length;
      // The service refreshed its limits in the background; the next machine read carries them.
      usage.value = [claude(new Date(Date.now() + 300_000).toISOString(), [{ name: "seven_day", usedPercent: 67 }, { name: "seven_day_fable", usedPercent: 2 }]), codex(33, new Date(Date.now() + 300_000).toISOString())];
      await vi.advanceTimersByTimeAsync(300_000 + 120_000);
      for (let i = 0; i < 4; i++) await vi.advanceTimersByTimeAsync(0);
      expect(ops(s).filter(op => op === "config.read").length).toBeGreaterThan(reads);
      expect(bars()).toEqual(["Claude67%", "Fable2%", "Codex33%"]);
    } finally { vi.useRealTimers(); }
  });

  it("refuses a switched-off model for new sprint planning and omits it from composer options", async () => {
    const s = machine([], [], {config(c) {
      c.setup["codex-acp"].status = "connected";
      c.catalogue["codex-acp"].models = [{id:"sol",label:"Disabled Sol",efforts:["low"],observed:true}];
      c.surfaces.routing.engines["codex-acp"].enabled = ["sol"];
      c.surfaces.routing.deny = ["codex-acp/*"];
    }});
    const p = createRealPort({transport:s.transport,actorId:"owner",registry:true});
    await p.query("capabilities.get", {});
    const facts = await p.query("machine.facts", {});
    expect(facts[0].options?.models.some(m => m.id === "sol")).toBe(false);
    const payload = {type:"create_sprint" as const,projectId:"p",description:"Work",runMode:"manual" as const,
      checkout:{kind:"main" as const},settings:{model:"sol",effort:"low",mode:"auto" as const,language:"auto" as const}};
    const result = await p.command({requestId:"disabled-sprint",operation:"create_sprint",expectedVersion:null,payload});
    expect(result.ok).toBe(false);
    expect(s.calls.some(c => c.operation === "machine.call" && (c.message as {op:string}).op === "product.sprint")).toBe(false);
  });

  it("does not offer a switched-off engine in coordinator defaults", async () => {
    const s = machine([], [], {config(c) {
      c.setup["codex-acp"].status = "connected";
      c.catalogue["codex-acp"].models = [{id:"sol",label:"Disabled Sol",efforts:["low"],observed:true}];
      c.surfaces.routing.engines["codex-acp"].enabled = ["sol"];
      c.surfaces.routing.deny = ["codex-acp/*"];
    }});
    await open(s, `/settings/machine/${LOCAL_MACHINE_ID}/orchestration/defaults`);
    expect(document.querySelector("#m-def-model")).toBeTruthy();
    expect(document.querySelector("#m-def-model")?.textContent).not.toContain("Disabled Sol");
  });
});

describe("Accept on real native result facts", () => {
  for (const hasWireResult of [false, true]) it(`accepts the displayed native result with ${hasWireResult ? "a different" : "no"} wire result`, async () => {
    const s = machine([wireTask({executionId:"e",phase:"result",runtimeState:"COMPLETED",executionState:"result"})]);
    if (!hasWireResult) s.answers["capabilities.get"] = () => capabilitiesWire(OFFERED.filter(op => op !== "results.get"));
    const answer = s.answers["machine.call"]!;
    const result = {kind:"work",version:2,sourceVersion:"47",digest:"native-review-digest",summary:"Native result",checks:[{name:"Runtime check",outcome:"passed",detail:""}],acceptanceAvailable:true};
    let accepted = false;
    s.answers["machine.call"] = message => {
      const call = message as {op:string;params?:{action:string}};
      if (call.op === "product.read") return {ok:true,result:{tasks:{"task-1":{
        result,workspaceDigest:result.digest,reviewActions:{accept:true},
        ...(accepted ? {status:"ready_to_merge",acceptance:{version:"47",digest:result.digest}} : {}),
      }},sprints:[],messages:[]}};
      if (call.op === "product.task" && call.params?.action === "accept") accepted = true;
      return answer(message);
    };
    await open(s, TASK + "/changes");
    expect(button("Accept v2")?.getAttribute("aria-disabled")).not.toBe("true");
    expect(button("Accept v2")).toBeTruthy();
    button("Accept v2")!.click();
    await settled();
    const calls = s.calls.filter(c => c.operation === "machine.call" && (c.message as {params?:{action:string}}).params?.action === "accept");
    expect(calls).toHaveLength(1);
    expect(calls[0].message).toMatchObject({op:"product.task",projectId:"p",params:{action:"accept",taskId:"task-1",digest:result.digest,version:"47",acknowledgeOpenChecks:false}});
    expect(document.body.textContent).not.toContain("Acceptance was not confirmed");
  });
});

describe("task merge recovery on real-port data", () => {
  it("clears only the saved legacy record after explicit confirmation and permits a retry after Git is settled", async () => {
    const s = machine([wireTask({executionId:"e",phase:"result",runtimeState:"COMPLETED",executionState:"result"})]);
    const { conflictId, gitState, ...legacy } = stoppedMerge();
    let cleared = false, gitSettled = false;
    const asked: Record<string, unknown>[] = [];
    const answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      const call = message as {op:string;params?:Record<string, unknown>};
      if (call.op === "product.read") return {ok:true,result:{tasks:{"task-1":cleared
        ? {conflict:"none",merge:null,requiresFreshResult:true}
        : {conflict:"failed",merge:legacy}},sprints:[]}};
      if (call.op === "product.task" && call.params?.action === "abort_merge") {
        asked.push(call.params);
        if (!gitSettled) return {ok:false,code:"legacy_merge_still_open",message:"Finish or abort the original merge in Git first."};
        cleared = true;
        return {ok:true,result:{}};
      }
      return answer(message);
    };
    await open(s, TASK + "/changes/conflict");
    expect(document.body.textContent).toContain("Saved merge from an earlier version");
    for (const label of ["Abort merge", "Finish merge", "Inspect merge state", "Stop resolution", "Hand to the app"]) {
      expect(button(label)).toBeUndefined();
    }
    button("Clear saved merge…")!.click();
    expect(document.activeElement?.textContent).toBe("Cancel");
    expect($('dialog[open]')?.textContent).toContain("Clearing the saved record leaves Git unchanged");
    expect(asked).toEqual([]);
    button("Cancel", "dialog[open]")!.click();
    expect(asked).toEqual([]);
    button("Clear saved merge…")!.click();
    button("Clear saved merge record", "dialog[open]")!.click();
    await settled();
    expect($('dialog[open] [role="alert"]')?.textContent).toContain("Finish or abort the original merge in Git first");
    expect(asked).toEqual([{action:"abort_merge",taskId:"task-1",discardLegacy:true,requestId:expect.any(String)}]);
    gitSettled = true;
    button("Clear saved merge record", "dialog[open]")!.click();
    await settled();
    expect(asked).toHaveLength(2);
    expect(asked[1]).toEqual({...asked[0],requestId:expect.any(String)});
    expect(asked[1].requestId).not.toBe(asked[0].requestId);
    expect($('dialog[open]')).toBeFalsy();
    expect(document.body.textContent).toContain("No merge in progress");
    expect(prompts(s)).toEqual([]);
    expect(s.calls.some(c => c.operation === "executions.control")).toBe(false);
  });

  it("fences Abort while resolving, inspects uncertainty, and offers Finish with no displayed paths", async () => {
    const s = machine([wireTask({executionId:"e",phase:"result",runtimeState:"COMPLETED",executionState:"result"})]);
    const before = stoppedMerge();
    const handoff = { state:"unknown", requestId:"h1",executionId:"e",sourceRevision:"s",targetRevision:"t",
      manifest:{sourceRevision:"s",targetRevision:"t",paths:["a.txt"],complete:true,truncated:false} };
    let merge: Record<string, unknown> = {...before, handoff:{...handoff,state:"active"}};
    let conflict = "resolving", resolvedBy: string | null = null;
    s.answers["executions.cancel"] = () => {
      conflict = "failed"; merge = {...before, gitState:"unknown", handoff};
      return {outcome:"cancellation_requested"};
    };
    const answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      const call = message as {op:string;params?:{action?:string}};
      if (call.op === "product.read") return {ok:true,result:{tasks:{"task-1":{merge,conflict,resolvedBy}},sprints:[]}};
      if (call.op === "product.task" && call.params?.action === "finish_merge") {
        conflict = "none"; resolvedBy = "you";
        merge = {...merge, finished:true, gitState:"completed", freshChecks:{state:"not_requested",revision:"resolved"}};
      }
      return answer(message);
    };
    s.answers["executions.control"] = message => {
      const { action } = message as {action:string};
      if (action === "inspect_merge") {
        conflict = "conflict"; merge = {...before, handoff:{...handoff,state:"stopped"}};
      }
      if (action === "request_merge_checks") merge = {...merge,freshChecks:{state:"requested",revision:"resolved"}};
      return {done:true};
    };
    await open(s, TASK + "/changes/conflict");
    expect(button("Abort merge")).toBeUndefined();
    expect(button("Finish merge")).toBeUndefined();
    expect(button("Hand to the app")).toBeUndefined();
    expect(document.body.textContent).toContain("App handoff recorded");
    button("Stop resolution")!.click();
    await settled();
    expect(button("Abort merge")).toBeUndefined();
    expect(document.body.textContent).toContain("Merge outcome unconfirmed");
    expect(document.body.textContent).not.toContain("Resolve it yourself, resolve it automatically or abort the merge.");
    button("Inspect merge state")!.click();
    await settled();
    expect(button("Abort merge")).toBeTruthy();
    expect(button("Finish merge")).toBeTruthy();
    button("Finish merge")!.click();
    await settled();
    expect(document.body.textContent).toContain("Fresh review and verification not requested");
    expect(button("Abort merge")).toBeUndefined();
    button("Request fresh checks")!.click();
    await settled();
    expect(document.body.textContent).toContain("Fresh review and verification requested");
    expect(button("Request fresh checks")).toBeUndefined();
    expect(prompts(s)).toEqual([]);
  });
});

describe("Git on real data", () => {
  it("commits unstaged files in one request and opens cached Git while the next snapshot is pending", async () => {
    const s = machine([]);
    const answer = s.answers["machine.call"]!;
    const changed = {...repo, checkouts:[{...repo.checkouts[0], draft:"Save changes", files:[
      {path:"a.txt",status:"M",add:1,del:0,hunks:[],stage:"unstaged"},
      {path:"b.txt",status:"A",add:1,del:0,hunks:[],stage:"untracked"},
    ]}]};
    let pending = false, finish: ((value: unknown) => void) | undefined;
    s.answers["machine.call"] = message => {
      const {op} = message as {op:string};
      if (op === "repo.snapshot") return pending ? new Promise(resolve => {finish = resolve;}) : {ok:true,result:changed};
      if (op === "product.read") return {ok:true,result:{hasGit:true,tasks:{},sprints:[]}};
      if (op === "git") { pending = true; return {ok:true,result:{}}; }
      return answer(message);
    };
    await open(s, `/m/${LOCAL_MACHINE_ID}/p/p/git`);
    expect(button("Commit", ".commit-box")).toBeDefined();
    button("Commit", ".commit-box")!.click();
    await settled();
    const actions = s.calls.filter(c => c.operation === "machine.call" && (c.message as {op:string}).op === "git");
    expect(actions.map(c => (c.message as {params:{action:string}}).params.action)).toEqual(["commit"]);
    expect(finish).toBeDefined();
    // Reopen the tab before Git has answered. The last known checkout is immediately usable.
    all<HTMLAnchorElement>("a").find(a => a.textContent?.trim() === "Board")!.click();
    await settled();
    all<HTMLAnchorElement>("a").find(a => a.textContent?.trim() === "Git")!.click();
    expect($('[aria-label="Status"]')?.textContent).toContain("a.txt");
    expect(button("Create empty repository")).toBeUndefined();
    finish!({ok:true,result:{...changed,checkouts:[{...changed.checkouts[0],files:[],draft:""}]}});
    await settled();
    expect($('[aria-label="Status"]')?.textContent).toContain("No uncommitted changes.");
  });

  it("offers Git initialization only after the helper confirms no repository", async () => {
    const s = machine([]);
    const answer = s.answers["machine.call"]!;
    let finish!: (value: unknown) => void;
    s.answers["machine.call"] = message => {
      const {op} = message as {op:string};
      if (op === "repo.snapshot") return new Promise(resolve => {finish = resolve;});
      if (op === "product.read") return {ok:true,result:{tasks:{},sprints:[]}};
      return answer(message);
    };
    await open(s, `/m/${LOCAL_MACHINE_ID}/p/p/git`);
    await new Promise(resolve => setTimeout(resolve, 60));
    expect(document.querySelector("main")?.textContent).toContain("No Git record for this project");
    expect(button("Create empty repository")).toBeUndefined();
    finish({ok:true,result:{...repo,hasGit:false}});
    await settled();
    expect(document.querySelector("main")?.textContent).toContain("This project has no Git repository");
    expect(button("Create empty repository")).toBeDefined();
  });
});

describe("task session on real data", () => {
  it("keeps planning visible in All work before it has created tasks", async () => {
    const s = machine(), answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => (message as {op:string}).op === "product.read"
      ? {ok:true,result:{tasks:{"task-1":{planning:true}},sprints:[{id:"sp",projectId:"p",planningTaskId:"task-1",description:"Plan the next release",title:"Next release",planning:"planning",runMode:"manual",targetBranch:null,conversation:[],plan:null,materializedVersion:null,taskIds:[],queueState:"idle",pauseReason:null}],messages:[]}}
      : answer(message);
    await open(s, `/m/${LOCAL_MACHINE_ID}/p/p`);
    expect(document.querySelector("main")?.textContent).toContain("Sprint: Next release");
    expect(document.querySelector("main")?.textContent).toContain("No tasks created yet");
    const link=[...document.querySelectorAll<HTMLAnchorElement>("main a")].find(a=>a.textContent === "Open planning");
    expect(link?.getAttribute("href")).toContain("/s/sp/planning");
    link!.click(); await settled();
    expect(document.querySelector("main")?.textContent).toContain("Plan the next release");
  });

  it("shows an execution worker question in the task, Inbox and sidebar without a global WAITING_USER", async () => {
    const s = machine();
    s.answers["decisions.list"] = () => ({items:[wireDecision()],nextCursor:null,highWatermark:"4"});
    await open(s, TASK);
    expect(document.querySelector(".timeline")?.textContent).toContain("Which?");
    const inbox = [...document.querySelectorAll<HTMLAnchorElement>("a")].find(a => a.getAttribute("href") === `/m/${LOCAL_MACHINE_ID}/inbox`);
    expect(inbox?.textContent).toContain("1");
    inbox?.click(); await settled();
    expect(document.querySelector(".main")?.textContent ?? document.body.textContent).toContain("Which?");
  });

  it("names the state and the runtime phase, and marks only that phase as now", async () => {
    await open(machine(), TASK);
    // The state reads in the strip and the inspector; the title band carries no state chip.
    expect($(".tb-actions .chip, .tb-actions .tb-state")).toBeNull();
    expect($(".inspector")?.textContent).toContain("Working");
    // Progress reads as the current runtime phase in words; no permanent phase strip and no invented percentage.
    expect($(".taskbar .td-progress")?.textContent).toBe("Executing");
    expect($(".phases")).toBeNull();
  });

  it("claims no progress for a wait, which says nothing about how far the task got", async () => {
    await open(machine([wireTask({ executionId: "e", phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" })]), TASK);
    expect($(".phases")).toBeNull();
  });

  it("opens the inspector, with what the machine sets for every task and no empty row", async () => {
    await open(machine(), TASK);
    const inspector = $(".inspector")!;
    expect(inspector.textContent).toContain("Branchdev");
    expect(inspector.textContent).toContain("Opus, High");
    expect(inspector.textContent).toContain("Auto");
    expect(inspector.textContent).toContain("Russian");
    expect(inspector.textContent).toContain("What this machine sets for every task.");
    expect(all(".inspector dd").filter(dd => !dd.textContent?.trim())).toEqual([]);
    expect($(".ws-app")?.getAttribute("data-inspector")).toBe("shown");
  });

  it("loads older guidance with its delivery and attachments through the combined cursor", async () => {
    const s = machine(undefined, [worker({activityCursor:"combined",activityPartial:true})]);
    const base = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      const p = message as {op:string;params?:{cursor?:string}};
      if (p.op === "workers" && p.params?.cursor) return {ok:true,result:[worker({
        activity:[{sequence:0,kind:"message",text:"Older guidance",messageId:"old-guidance",at:null}],
        messages:[{id:"old-guidance",taskId:"task-1",workerId:"w1",text:"Older guidance",mode:"send_now",
          delivery:"delivered",at:null,deliveredAt:null,failure:null,attachments:["old-note.txt"]}],
        activityCursor:null,activityPartial:false
      })]};
      return base(message);
    };
    await open(s, `${TASK}/workers/w1/live`);
    await settled();
    expect(document.body.textContent).toContain("Older guidance");
    expect(document.body.textContent).toContain("old-note.txt");
    expect(document.body.textContent).toContain("Delivered does not mean read");
    expect(button("Load older activity")).toBeUndefined();
  });

  it("shows the current project branch, not the Git default", async () => {
    await open(machine(), `/m/${LOCAL_MACHINE_ID}/p/p/board`);
    expect(document.querySelector('[aria-label="Current branch: dev"]')?.textContent).toBe("dev");
  });

  it("shows the workers in the conversation, in the pane and on their tab", async () => {
    const workers = [worker({}), worker({ id: "w2", role: "code-reviewer", status: "completed", lane: "review", result: { summary: "No findings", checks: [] } })];
    await open(machine(undefined, workers), TASK);
    await openTeam();
    expect(all(".wrow").map(r => r.textContent)).toEqual([expect.stringContaining("code-reviewer"), expect.stringContaining("frontend-dev")]);
    expect($(".inspector")?.textContent).toContain("1 active, 1 finished");
    dispose?.(); document.body.innerHTML = "";
    await open(machine(undefined, workers), `${TASK}/workers/w2`);
    expect($(".livepane .d-title")?.textContent).toContain("code-reviewer");
    expect($(".livepane")?.textContent).toContain("No findings");
  });

  it("keeps following off until the owner returns and folds a failing step", async () => {
    const activity: Worker["activity"] = [
      {sequence:1,kind:"note",text:"Worker prose",at:null},
      {sequence:2,kind:"tool",text:"Read failed",error:true,at:null},
      {sequence:3,kind:"edit",text:"Changed file",at:null},
    ];
    const workers = [worker({activity})], s = machine(undefined, workers);
    await open(s, `${TASK}/workers/w1/live`);
    expect($('[aria-label="Filter activity"]')).toBeNull();
    expect($(".stream")?.textContent).toContain("Worker prose");
    expect($(".stream")?.textContent).not.toContain("Read failed");
    expect($(".stream button.fold")?.textContent).toMatch(/1 tool call needs attention/);
    $(".stream button.fold")!.click(); await settled();
    expect($(".stream")?.textContent).toContain("Read failed");
    const stream = $(".stream")!;
    stream.scrollTop = -100; stream.dispatchEvent(new Event("scroll")); await settled();
    // Opening the block stopped following: Latest is the feed's one follow control.
    expect($(".dock-w .new-pill")).not.toBeNull();
    expect($('[aria-label="Follow new activity"]')).toBeNull();
    workers[0] = worker({activity:[activity[1],activity[2],{sequence:4,kind:"note",text:"New arrival",at:null}]});
    s.send({type:"refresh"}); await settled();
    expect($(".dock-w .new-pill")).not.toBeNull();
    expect(stream.scrollTop).toBe(-100);
    expect($(".new-pill")?.textContent).toContain("1 new");
    s.send({type:"refresh"}); await settled();
    expect($(".new-pill")?.textContent).toContain("1 new");
    $(".new-pill")!.click(); await settled();
    expect($(".dock-w .new-pill")).toBeNull();
    expect(stream.scrollTop).toBe(0);
  });

  // Three chunks, with a code block cut in two: only joining before parsing renders it as one block.
  const reportChunks = ["# Outcome\n\nOpening line\n```\nfirst half ", "second half\n```\nMiddle line\n", "Closing line"];
  const reportPage = (at: number) => ({reportId:"r_public",title:"Published report",sha256:"a".repeat(64),markdown:reportChunks[at],totalCharacters:reportChunks.join("").length,nextCursor:at<reportChunks.length-1?`cursor-${at+1}`:null,partial:at<reportChunks.length-1});
  const chunkOf = (request: unknown) => Number((request as {cursor?:string}).cursor?.slice("cursor-".length) ?? 0);
  const finishedReportWorker = async (answer: (request: unknown) => unknown) => {
    const workers = [worker({})];
    const s = machine(undefined, workers);
    s.answers["capabilities.get"] = () => capabilitiesWire([...OFFERED,"reports.read"]);
    s.answers["reports.read"] = answer;
    await open(s, `${TASK}/workers/w1`);
    workers[0] = worker({status:"completed",result:{summary:"Done",checks:[],reportId:"r_public"}});
    s.send({type:"refresh"}); await settled();
    return s;
  };
  const report = () => $('dialog[data-dialog="Worker report"]')!;

  it("opens only the referenced public report and shows all of it at once", async () => {
    const s = await finishedReportWorker(request => reportPage(chunkOf(request)));
    button("Open report")!.click(); await settled();
    s.send({type:"refresh"}); await settled();
    expect(report().textContent).toContain("Opening line");
    expect(report().textContent).toContain("Middle line");
    expect(report().textContent).toContain("Closing line");
    expect(report().querySelectorAll("pre")).toHaveLength(1);
    expect(report().querySelector("pre")?.textContent).toBe("first half second half");
    expect(button("Next page")).toBeUndefined();
    expect(report().textContent).not.toContain("Partial report page");
    expect(s.calls.filter(c=>c.operation==="reports.read").map(c=>c.message)).toEqual([
      expect.objectContaining({target:"r_public",limit:4000}),
      expect.objectContaining({target:"r_public",cursor:"cursor-1",limit:4000}),
      expect.objectContaining({target:"r_public",cursor:"cursor-2",limit:4000}),
    ]);
  });

  it("never shows part of a report as the report when a later chunk fails, and Retry reads it whole", async () => {
    let broken = true;
    const s = await finishedReportWorker(request => {
      const at = chunkOf(request);
      if (at === 1 && broken) throw new Error("socket closed");
      return reportPage(at);
    });
    button("Open report")!.click(); await settled();
    expect(report().querySelector('[role="alert"]')).not.toBeNull();
    expect(report().textContent).not.toContain("Opening line");
    broken = false;
    button("Retry")!.click(); await settled();
    expect(report().querySelector('[role="alert"]')).toBeNull();
    expect(report().textContent).toContain("Closing line");
    expect(s.calls.filter(c=>c.operation==="reports.read")).toHaveLength(5);
  });

  it("refuses to stitch a report that changed between chunks", async () => {
    await finishedReportWorker(request => {
      const at = chunkOf(request);
      return at ? {...reportPage(at),sha256:"b".repeat(64)} : reportPage(at);
    });
    button("Open report")!.click(); await settled();
    expect(report().querySelector('[role="alert"]')?.textContent).toContain("The report changed");
    expect(report().textContent).not.toContain("Opening line");
  });

  it("shows a readable message, never the raw service refusal, when a report read is refused", async () => {
    await finishedReportWorker(() => { throw new Error("refused: This control is not available for native execution yet."); });
    button("Open report")!.click(); await settled();
    const text = report().querySelector('[role="alert"]')?.textContent ?? "";
    expect(text).toContain("cannot be opened right now");
    expect(text).not.toContain("native execution");
  });

  it("stops reading a report once it is closed, and reopening starts clean", async () => {
    let release = () => {}, reads = 0;
    const held = new Promise<void>(resolve => { release = resolve; });
    await finishedReportWorker(async request => {
      if (++reads === 2) await held;
      return reportPage(chunkOf(request));
    });
    button("Open report")!.click(); await settled();
    $('[aria-label="Close report"]')!.click(); await settled();
    release(); await settled();
    expect(reads).toBe(2);
    expect(report().hasAttribute("open")).toBe(false);
    button("Open report")!.click(); await settled();
    expect(report().querySelector("pre")?.textContent).toBe("first half second half");
    expect(reads).toBe(5);
  });

  it("gives a task with no workers the whole width", async () => {
    await open(machine(), TASK);
    expect($(".detail")).toBeNull();
    expect($(".detail-toggle")).toBeNull();
  });

  it("reports the runtime's verification, never a fixed Not run", async () => {
    await open(machine([wireTask({ executionId: "e", phase: "result", runtimeState: "COMPLETED", executionState: "result" })]), `${TASK}/workers/verification`);
    expect($(".wlist")?.textContent).toContain("1 passed");
    expect($(".livepane")?.textContent).toContain("pytest");
    expect($(".livepane")?.textContent).not.toContain("Not run");
  });

  it("describes Stop as a resumable pause", async () => {
    await open(machine(), TASK);
    // Stop is the composer's action button while the field is empty, and nowhere in the title band.
    expect(all(".tb-actions button").some(b => /Stop|Resume|Start/.test(b.textContent ?? ""))).toBe(false);
    expect(all('[aria-label="Task composer"] button').at(-1)?.getAttribute("aria-label")).toBe("Stop");
  });

  it("offers no terminal it does not have", async () => {
    await open(machine(), TASK);
    expect(button("Open terminal here")).toBeUndefined();
  });
});

describe("questions on real data", () => {
  const waiting = wireTask({ executionId: "e", phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" });
  const batch = [wireDecision({ decisionId: "d1", questionId: "q1", batchId: "b", prompt: "Which file?", options: [{ optionId: "o1", label: "First" }] }), wireDecision({ decisionId: "d2", questionId: "q2", batchId: "b", prompt: "Which branch?", options: [{ optionId: "o2", label: "Second" }] })];
  const opt = (text: string) => all(".dc .opt").find(o => o.textContent?.includes(text))!;

  it("shows the exact pending question while paused without submitting an answer", async () => {
    const s = machine([wireTask({executionId:"e",phase:"paused",runtimeState:"PAUSED",executionState:"paused"})]);
    s.answers["decisions.list"] = () => ({items:[{...batch[0],allowedOperations:[]}],nextCursor:null,highWatermark:"4"});
    await open(s,TASK);
    expect(document.body.textContent).toContain("Which file?");
    expect(document.body.textContent).toContain("Resume the task to answer");
    expect(s.calls.filter(c=>c.operation==="decisions.answer")).toEqual([]);
  });

  it("sends a batch's answers together with the form's one button, then reads it as answered", async () => {
    const s = machine([waiting]);
    s.answers["decisions.list"] = () => ({ items: batch, nextCursor: null, highWatermark: "4" });
    s.answers["decisions.answer"] = message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "b", entityVersion: "3", recordedAt: "2026-11-05T00:00:00Z" });
    await open(s, TASK);
    // No question has its own plane, and the answer form has no model or effort.
    expect(all('.dc [aria-label="Send answer"]')).toEqual([]);
    expect($(".dc .c-route")).toBeNull();
    opt("First").click();
    await settled();
    // Choosing sends nothing.
    expect(s.calls.filter(c => c.operation === "decisions.answer")).toEqual([]);
    expect(button("Send answers")!.getAttribute("aria-disabled")).toBe("true");
    opt("Second").click(); await settled();
    button("Send answers")!.click(); await settled();
    const [sent, ...more] = s.calls.filter(c => c.operation === "decisions.answer").map(c => c.message as { target: string; payload: { batchId: string; answers: { text: string }[] } });
    expect(more).toEqual([]);
    expect(sent.target).toBe("b");
    expect(sent.payload.answers.map(a => a.text)).toEqual(["First", "Second"]);
    expect(all(".dc .qa-a").map(a => a.textContent)).toEqual(["First", "Second"]);
    expect(button("Send answers")).toBeUndefined();
  });

  it("keeps a batch the machine refused open with its answers, so nothing reads as answered that was not", async () => {
    const s = machine([waiting]);
    s.answers["decisions.list"] = () => ({ items: batch, nextCursor: null, highWatermark: "4" });
    s.answers["decisions.answer"] = message => ({ requestId: (message as { requestId: string }).requestId, outcome: "busy", target: "b", entityVersion: "2", recordedAt: "2026-11-05T00:00:00Z" });
    await open(s, TASK);
    opt("First").click(); opt("Second").click(); await settled();
    button("Send answers")!.click(); await settled();
    expect($(".ws-receipt")?.textContent).toMatch(/still running/i);
    expect($('.dc [role="alert"]')?.textContent).toMatch(/still running/i);
    expect(all(".dc .qa")).toEqual([]);
    expect(all(".dc .opt").filter(o => o.getAttribute("aria-checked") === "true")).toHaveLength(2);
    expect(button("Send answers")!.getAttribute("aria-disabled")).toBeNull();
  });
});

describe("messages on real data", () => {
  it("uses native empty-clipboard/drop fallbacks in the answer composer and preserves multiline answer bytes", async () => {
    const s = machine();
    const attachments = [{name:"clip.png",mimeType:"image/png",data:btoa("fixture png")}];
    s.answers["service.capabilities"] = () => ({methods:["machine.call","native.clipboard_image","native.take_dropped_files"],machine:true});
    s.answers["capabilities.get"] = () => ({...capabilitiesWire(OFFERED),attachments:true});
    s.answers["native.clipboard_image"] = () => attachments;
    s.answers["native.take_dropped_files"] = () => [{name:"drop.txt",mimeType:"text/plain",data:btoa("drop")}];
    s.answers["decisions.list"] = () => ({items:[wireDecision()],nextCursor:null,highWatermark:"4"});
    s.answers["decisions.answer"] = message => ({requestId:(message as any).requestId,target:"d",outcome:"recorded",entityVersion:"3",recordedAt:"2026-11-06T00:00:00Z"});
    await open(s,TASK);
    type("Your answer","first line\nsecond line");
    const textarea = document.querySelector('[aria-label="Your answer"]')!;
    const paste = new Event("paste",{bubbles:true,cancelable:true});
    Object.defineProperty(paste,"clipboardData",{value:{files:[],types:[]}});
    textarea.dispatchEvent(paste);
    const drop = new Event("drop",{bubbles:true,cancelable:true});
    Object.defineProperty(drop,"dataTransfer",{value:{files:[],types:["text/uri-list"]}});
    textarea.dispatchEvent(drop);
    await settled();
    expect(document.querySelector(".dc-answer")?.textContent).toMatch(/clip.png.*drop.txt/s);
    button("Send answer")!.click();
    await settled();
    const sent = s.calls.find(c => c.operation === "decisions.answer")!.message as any;
    expect(sent.payload.answers[0]).toMatchObject({text:"first line\nsecond line",attachments:[refOf(attachments[0]),refOf({name:"drop.txt",mimeType:"text/plain",data:btoa("drop")})]});
    expect(s.calls.filter(c => c.operation === "native.clipboard_image")).toHaveLength(1);
  });

  it("reads picker, drop and paste bytes; queue Edit restores them and a failed send keeps them", async () => {
    const s = machine();
    s.answers["capabilities.get"] = () => ({...capabilitiesWire(OFFERED),attachments:true});
    await open(s, TASK);
    const file = new File(["drop fixture"],"drop.txt",{type:"text/plain"});
    const picker = document.querySelector<HTMLInputElement>('.dock input[type="file"]')!;
    Object.defineProperty(picker, "files", {configurable:true,value:[new File(["picked"],"pick.txt",{type:"text/plain"})]});
    picker.dispatchEvent(new Event("change",{bubbles:true}));
    const drop = new Event("drop",{bubbles:true,cancelable:true});
    Object.defineProperty(drop,"dataTransfer",{value:{files:[file],types:["Files"]}});
    document.querySelector('[aria-label="Message"]')!.dispatchEvent(drop);
    const paste = new Event("paste",{bubbles:true,cancelable:true});
    Object.defineProperty(paste,"clipboardData",{value:{files:[new File(["png fixture"],"paste.png",{type:"image/png"})]}});
    document.querySelector('[aria-label="Message"]')!.dispatchEvent(paste);
    await settled();
    expect(document.querySelector(".composer")?.textContent).toMatch(/pick.txt.*drop.txt.*paste.png/s);
    type("Message","With files");
    queue();
    await settled();
    // The queued row shows only its text; its files come back in Edit.
    expect($(".queue")?.textContent).toContain("With files");
    expect($(".queue")?.textContent).not.toMatch(/pick.txt|drop.txt|paste.png/);
    button("Edit")!.click();
    await settled();
    expect($(".queue")).toBeNull();
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("With files");
    expect(document.querySelector(".dock .composer")?.textContent).toMatch(/pick.txt.*drop.txt.*paste.png/s);
    s.answers["executions.prompt"] = () => { throw new Error("fixture lost acknowledgement"); };
    button("Queue", ".mc-delivery")!.click();
    button("Send message now")!.click();
    await settled();
    // The message that did not arrive stays on its line in the conversation, files and all, and Edit takes it back.
    expect($('.timeline [data-sent="failed"]')?.textContent).toMatch(/With files.*paste.png/s);
    expect($('.timeline .ws-sent-meta [role="alert"]')?.textContent).toMatch(/^Not sent: /);
    button("Edit", ".timeline .ws-sent-meta")!.click();
    await settled();
    expect($('.timeline [data-sent]')).toBeNull();
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("With files");
    expect(document.querySelector(".dock .composer")?.textContent).toContain("paste.png");
    // The prompt names the uploaded files; their bytes are the ones picked, dropped and pasted.
    const sent = s.calls.find(c => c.operation === "executions.prompt")!.message as {attachments:{name:string;sha256:string}[]};
    expect(sent.attachments.map(a => [a.name,atob(s.bodies.get(a.sha256)!)])).toEqual([["pick.txt","picked"],["drop.txt","drop fixture"],["paste.png","png fixture"]]);
  });

  it("holds a queued message unsent, and Edit brings it back to the field", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Use the other file");
    queue();
    await settled();
    expect($(".queue")?.textContent).toContain("Use the other file");
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("");
    expect(prompts(s)).toEqual([]);
    button("Edit")!.click();
    await settled();
    expect($(".queue")).toBeNull();
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("Use the other file");
    expect(prompts(s)).toEqual([]);
  });

  it("Send now delivers a queued message once, and it then reads as said", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Second thought");
    queue();
    await settled();
    button("Send now")!.click();
    await settled();
    expect(prompts(s)).toEqual(["Second thought"]);
    expect($(".queue")).toBeNull();
    expect($(".timeline")?.textContent).toContain("Second thought");
  });

  it("keeps a queued message when its send fails, and sends nothing twice", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Keep me");
    queue();
    await settled();
    s.answers["executions.prompt"] = () => { throw new Error("socket closed"); };
    button("Send now")!.click();
    await settled();
    expect($(".queue")?.textContent).toContain("Keep me");
    expect($(".ws-receipt")?.textContent).toMatch(/not known/);
  });

  it("a steer reaches the machine and stays in the conversation", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Mind the race");
    button("Send message now")!.click();
    // It reads in the conversation from the press, before the machine has answered.
    expect($('.timeline [data-sent="sending"]')?.textContent).toContain("Mind the race");
    await settled();
    expect(prompts(s)).toEqual(["Mind the race"]);
    expect($(".timeline")?.textContent).toContain("Mind the race");
    // The conversation reread carries the runtime's own record of it, which reads once, in its place.
    await vi.waitFor(() => expect($(".timeline [data-sent]")).toBeNull(), { timeout: 3000 });
    expect(all(".timeline .mine").map(m => m.textContent?.trim())).toEqual(["Mind the race"]);
  });

  it("targets a worker safe point, never generic coordinator steering", async () => {
    const s = machine(undefined, [worker({})]);
    await open(s, `${TASK}/workers/w1`);
    const field = $<HTMLTextAreaElement>('[aria-label="Message frontend-dev"]')!;
    expect(field.placeholder).toContain("directly");
    expect($(".livepane .c-bar")?.textContent).toContain("Next step");
    type("Message frontend-dev", "Keep the old colors");
    $<HTMLElement>('[aria-label="Send to frontend-dev"]')!.click();
    await settled();
    expect(prompts(s)).toEqual([]);
    expect(s.calls.find(c => c.operation === "executions.control")?.message).toMatchObject({action:"worker_message",workerId:"w1",text:"Keep the old colors"});
  });

  it("confirms a stop for one worker without stopping the whole task", async () => {
    const s = machine(undefined, [worker({})]);
    await open(s, `${TASK}/workers/w1`);
    // The worker is stopped from its composer's action button, as a task is from its own; the header has no Stop of its own.
    expect(all(".livepane .d-h button").some(b => b.textContent?.startsWith("Stop"))).toBe(false);
    const stop = $<HTMLElement>('.livepane [aria-label="Worker composer"] .c-send[aria-label="Stop worker…"]')!;
    expect(stop.getAttribute("aria-disabled")).not.toBe("true");
    stop.click();
    await settled();
    expect($("dialog[open]")).not.toBeNull();
    // Its accessible name is its visible title, which names the worker; it once said "Stop this worker?" instead.
    const named = $("dialog[open]")!.getAttribute("aria-labelledby");
    expect($("dialog[open] h2")?.textContent).toBe("Stop frontend-dev?");
    expect(named && document.getElementById(named)?.textContent).toBe("Stop frontend-dev?");
    expect($("dialog[open]")!.hasAttribute("aria-label")).toBe(false);
    // No words sit beside a composer's paper plane: the text-only Attach keeps its reason in its own box.
    const attach = $<HTMLElement>('dialog[open] [aria-label="Attach files"]')!;
    expect(attach.getAttribute("data-why")).toBe("A stop reason is text only. Files cannot be sent with it.");
    expect($("dialog[open] .c-bar .l-reason")).toBeNull();
    type("Reason, optional", "Wrong approach");
    all("dialog[open] button").find(b => b.textContent?.trim() === "Stop worker")!.click();
    await settled();
    expect(s.calls.find(c => c.operation === "executions.control")?.message).toMatchObject({action:"worker_stop",workerId:"w1",reason:"Wrong approach"});
  });
});

describe("new task on real data", () => {
  const NEW = `/m/${LOCAL_MACHINE_ID}/new`;
  const radio = (text: string) => all<HTMLLabelElement>("label.rc").find(l => l.textContent?.includes(text))!;

  it("offers task worktrees with the real branch base", async () => {
    const s = machine([]);
    s.answers["service.capabilities"] = () => ({ methods: ["machine.call", "executions.control"], machine: true, taskWorktrees: true });
    await open(s, NEW);
    expect(radio("Separate worktree").querySelector("input")!.disabled).toBe(false);
    expect(radio("Separate worktree").textContent).toMatch(/currently open branch/);
    expect(radio("Separate worktree").querySelector("input")!.checked).toBe(true);
    expect(radio("Project folder").textContent).toContain("dev");
  });

  it("offers per-task routing and a target branch choice", async () => {
    await open(machine([]), NEW);
    // Model and effort, then access and language under one Settings control.
    const route = all(".c-route .sel");
    expect(route.map(b => b.textContent?.trim())).toEqual(["Opus", "High"]);
    const settings = $(".c-route .c-set")!;
    expect(settings.getAttribute("aria-label")).toBe("Settings. Access mode: Auto, Message language: Russian");
    for (const select of [...route, settings]) expect(select.getAttribute("aria-disabled")).not.toBe("true");
    expect($('[aria-label="Merge into main"]')).toBeTruthy();
  });

  it.each(["routing", "ceiling"] as const)("uses the project %s to choose and save the permitted coordinator", async restrict => {
    const codexRoute = "xr_bbbbbbbbbbbbbbbbbbbbbbbb";
    const s = machine([], [], { config: c => {
      c.surfaces.routing.engines["codex-acp"].enabled = ["gpt-6-luna"];
      c.setup["codex-acp"].status = "connected";
      c.catalogue["codex-acp"].models = [{ id: "gpt-6-luna", routeId: codexRoute, label: "Luna", efforts: ["low"], observed: true }];
    } });
    const answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      if ((message as { op: string }).op !== "project.config.read") return answer(message);
      const c = emptyWorkspace().machines[0].config;
      const routing = c.surfaces.routing;
      routing.engines["codex-acp"].enabled = ["gpt-6-luna"];
      const ceiling = { ...c.surfaces.user_defaults.ceiling, allowedRoutes: [routeIdOf("codex-acp", "gpt-6-luna")] };
      return { ok: true, result: { value: restrict === "routing" ? { routing } : { ceiling }, tokens: {} } };
    };
    await open(s, NEW);
    expect(all(".c-route .sel").map(b => b.textContent?.trim())).toEqual(["Luna", "Low"]);
    expect($("#m-model-new")?.textContent).not.toContain("Opus");
    type("Describe the task", "Save the chosen route");
    button("Save draft")!.click();
    await settled();
    expect(s.calls.find(c => c.operation === "tasks.create")?.message).toMatchObject({ payload: { settings: { model: codexRoute, effort: "low" } } });
  });

  it("Start now creates the task and starts it", async () => {
    const s = machine([]);
    await open(s, NEW);
    type("Describe the task", "Fix the race");
    button("Start now")!.click();
    await settled();
    expect(s.calls.map(c => c.operation)).toEqual(expect.arrayContaining(["tasks.create", "tasks.start"]));
  });
});

describe("what no screen says", () => {
  const ID = /\b(?:xr|cn)_[0-9a-f]{6,}|sha256|[0-9a-f]{16,}/;
  const claude: EngineConnection = { id: "cn_ab12cd34ef56", engine: "claude", label: "Claude", note: "Native", sessions: 4, state: "connected", checkedAt: null,
    detail: { connectionId: "cn_ab12cd34ef56", provider: "anthropic", authMode: "native", keyKind: null, endpoint: null, launchCommand: "adapter", credential: "native", qualified: true, createdAt: "2026-11-05", updatedAt: "2026-11-05" } };
  const owned = () => machine(undefined, undefined, { version: "1.0.6+sha256.94ba5360d3d1f14d", connections: [claude], config: c => { c.catalogue["claude-agent-acp"].models = [{ id: "opus", routeId: "xr_6b08617e228a19a17e2474f3", label: "Opus 5.5", efforts: ["medium", "high"], observed: true, contextWindow: 1000000 }]; c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"]; } });
  const said = () => [document.body.textContent ?? "", ...[...document.querySelectorAll("[title], [aria-label], [placeholder]")].flatMap(el => ["title", "aria-label", "placeholder"].map(a => el.getAttribute(a) ?? ""))].join("\n");
  const settings = (page: string) => `/settings/machine/${LOCAL_MACHINE_ID}/${page}`;

  it("keeps route, connection and build ids off every page but Diagnostics", async () => {
    for (const path of [settings("overview"), settings("connections"), settings("orchestration/models"), settings("orchestration/defaults"), settings("orchestration/routing"), settings("orchestration/policy"), "/settings/app/machines", `/m/${LOCAL_MACHINE_ID}/new`]) {
      await open(owned(), path);
      expect(said(), path).not.toMatch(ID);
      dispose?.(); dispose = undefined; document.body.innerHTML = "";
    }
    await open(owned(), settings("diagnostics"));
    expect(document.body.textContent).toContain("1.0.6+sha256.94ba5360d3d1f14d");
    expect(document.body.textContent).toContain("cn_ab12cd34ef56");
  });
  it("reads a version as the version people talk about", async () => {
    await open(owned(), settings("overview"));
    expect(document.body.textContent).toContain("1.0.6");
  });
  it("never shows a route id as a model's name, even once the engines stop offering it", async () => {
    const s = machine([wireTask({ executionId: "e", phase: "execution", runtimeState: "EXECUTING", executionState: "running" })], [], { config: c => { c.surfaces.user_defaults.coordinator = null; c.surfaces.user_defaults.routeDefault = { routeId: "xr_6b08617e228a19a17e2474f3", connectionId: "cn_ab12cd34ef56", model: "gone", parameters: {} }; } });
    await open(s, `/m/${LOCAL_MACHINE_ID}/new`);
    expect(said()).not.toMatch(ID);
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    await open(s, settings("orchestration/defaults"));
    expect(document.body.textContent).toContain("No longer registered");
    expect(said()).not.toMatch(ID);
  });
  it("says in words what differs from the profile, and tiers read from light to flagship", async () => {
    const s = machine(undefined, undefined, { connections: [claude], config: c => {
      c.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus 5.5", efforts: ["high"], observed: true }];
      c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
      c.profiles = [{ id: "default", name: "default", description: "", updatedAt: null, sections: capture({ ...c.surfaces, routing: { ...c.surfaces.routing, engines: { ...c.surfaces.routing.engines, "claude-agent-acp": { ...c.surfaces.routing.engines["claude-agent-acp"], enabled: [] } } } }, { cn_ab12cd34ef56: 2 }) }];
      c.active = "default";
    } });
    await open(s, settings("orchestration/engines"));
    const diff = [...document.querySelectorAll(".ws-diff")].map(d => d.textContent).join(" ");
    expect(diff).toContain("Opus 5.5 turned on");
    expect(diff).toContain("Claude: 4 sessions at most, was 2");
    expect(diff).not.toMatch(ID);
    expect(document.querySelector(".chip.st-warn")?.textContent).toBe("Modified (2)");
    // Tiers read in full words, from light to flagship; no single-letter control remains.
    expect([...document.querySelector(".tier")!.querySelectorAll("button")].map(b => b.textContent)).toEqual(["Light", "Standard", "Deep", "Flagship"]);
    expect(document.querySelector('input[aria-label="Use Claude"]')).toBeTruthy();
  });
  it("edits a title by clicking it, with no second control beside it", async () => {
    await open(machine(), `/m/${LOCAL_MACHINE_ID}/t/task-1`);
    expect(document.querySelector(".brief")?.textContent).not.toMatch(/Title (generated|set)|Edit|Rename/);
    expect(document.querySelector(".ws-title")).toBeTruthy();
  });
  it("has nothing to count on an empty project, and says no zero", async () => {
    await open(machine([]), `/m/${LOCAL_MACHINE_ID}`);
    expect([...document.querySelectorAll(".n, .count, .task-row.all")].map(n => n.textContent?.trim())).not.toContain("0");
    expect(document.body.textContent).not.toMatch(/All 0/);
  });
});
