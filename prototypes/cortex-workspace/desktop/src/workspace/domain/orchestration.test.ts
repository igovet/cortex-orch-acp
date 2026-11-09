import { describe, expect, it } from "vitest";
import { createMockWorkspace } from "../adapter/mock";
import type { WorkspaceCommand, WorkspacePort } from "./types";
import {
  accessFor, autoOn, ceilingProblems, ceilingState, configIssues, coordinatorEfforts, coordinatorResolution, createInitialWorkspace, diffPaths, effectiveCeiling, machineDefaults, mcpProblems, mcpTools, mockConnect, newMcpServer,
  parseCommandLine, previewContext, previewGrid, previewRoute, profileDifferences, profileState, ROLE_CATALOG, routingProblems, routingSource, serverStatus, setAffinity, setCoordinatorMode, setEngineUse, setGrant, setLevel, setModelAuto, setModelEnabled, setPin,
  setPolicy, setProvider, setTier, suggestLevels, tierFor, toolLevel, toolWarnings, usageFor, WORK_KINDS,
} from "./index";

const LOCAL = "mock-machine-00000";
const fs = "node:fs";
const { readFileSync } = await import(/* @vite-ignore */ fs);
const preset = JSON.parse(readFileSync(new URL("../../../../../../policy/model-settings-default.json", import.meta.url).pathname.replace("/@fs", ""), "utf8"));
const setup = () => {
  const p = createMockWorkspace(createInitialWorkspace());
  p.dispatch({ type: "add_project", id: "project", machineId: LOCAL, name: "Project", repository: "/repo", openBranch: "main", defaultBranch: "main" });
  return p;
};
const send = (p: WorkspacePort, c: WorkspaceCommand) => { const r = p.dispatch(c); expect(r.ok, r.message).toBe(true); return r; };
const refuse = (p: WorkspacePort, c: WorkspaceCommand) => { const r = p.dispatch(c); expect(r.ok).toBe(false); return r.message; };
const machine = (p: WorkspacePort) => p.snapshot().machines[0];
const cfg = (p: WorkspacePort) => machine(p).config;
const save = (p: WorkspacePort, surface: "routing" | "user_defaults" | "mcp_servers", value: unknown, extra = {}) =>
  p.dispatch({ type: "save_surface", machineId: LOCAL, surface, value, revision: cfg(p).revisions[surface], ...extra } as WorkspaceCommand);
const rules = () => structuredClone(cfg(setup()).surfaces.routing);
const server = (name: string) => structuredClone(cfg(setup()).surfaces.mcp_servers.find(s => s.name === name)!);
const ctx = (p = setup(), usage = {}) => ({ rules: cfg(p).surfaces.routing, ceiling: cfg(p).surfaces.user_defaults.ceiling, catalogue: cfg(p).catalogue, usage });

describe("the bundled preset", () => {
  it("mirrors policy/model-settings-default.json exactly", () => {
    const r = rules(), src = preset.routing;
    expect(r.version).toBe(2);
    expect(r.deny).toEqual(src.deny);
    expect(r.providers).toEqual(src.providers);
    for (const e of ["claude-agent-acp", "codex-acp", "opencode"] as const) {
      expect(r.engines[e].enabled).toEqual(src.enabled[e]);
      expect(r.engines[e].auto).toEqual(src.models[e].auto);
      expect(r.engines[e].tiers).toEqual(src.models[e].tiers);
      for (const [model, want] of Object.entries<Record<string, unknown>>(src.models[e].policy)) {
        const got = r.engines[e].policy[model];
        expect(got.effortFloor).toEqual(want.effort_floor ?? null);
        expect(got.maxEffort).toEqual(want.max_effort ?? null);
        expect(got.effortLadder).toEqual(want.effort_ladder ?? null);
        expect(got.alsoStandardFor).toEqual(want.also_standard_for ?? null);
        expect(got.preferredFor).toEqual(want.preferred_for ?? null);
        expect(got.preferredWhen).toEqual(want.preferred_when ?? null);
        expect(got.requestNames).toEqual(want.request_names ?? null);
      }
    }
    expect(r.engines["claude-agent-acp"].quota).toEqual(src.quota_windows["claude-agent-acp"]);
    const d = cfg(setup()).surfaces.user_defaults;
    expect(d.coordinator).toMatchObject({ engine: "claude-agent-acp", model: "opus[1m]", effort: "medium" });
    expect(d.language).toBe("ru");
    for (const [name, want] of Object.entries<Record<string, unknown>>(preset.mcp_servers)) {
      const s = server(name);
      expect(s.allowedTools).toEqual(want.allowed_tools);
      expect(s.readOnlyTools).toEqual(want.read_only_tools ?? []);
      expect(s.coordinatorTools).toEqual(want.coordinator_tools ?? null);
      expect(s.alwaysLoadTools).toEqual(want.always_load_tools ?? null);
      expect(s.roles).toEqual(want.roles ?? null);
      expect(s.engines).toEqual(want.engines ?? null);
      expect(s.network).toBe(want.network);
      expect(s.enabled).toBe(false);
    }
    expect(cfg(setup()).profiles.map(x => x.name)).toEqual(["default"]);
  });
  it("has the 23 profiles in their access classes, and the 12 work kinds", () => {
    expect(ROLE_CATALOG).toHaveLength(23);
    const count = (a: string) => ROLE_CATALOG.filter(r => r.access === a).length;
    expect([count("writing"), count("read_only"), count("consultant")]).toEqual([12, 10, 1]);
    expect(WORK_KINDS.map(k => k.id)).toEqual(["implementation_backend", "implementation_frontend", "implementation_frontend_design", "operations", "debugging", "testing", "investigation", "review", "design", "writing", "planning", "analysis"]);
  });
  it("computes role tiers the way the runtime does", () => {
    const row = (role: string, escalate: boolean) => (["low", "normal", "high"] as const).map(c => tierFor(role, escalate, c));
    expect(row("worker-architect", false)).toEqual(["standard", "deep", "deep"]);
    expect(row("worker-architect", true)).toEqual(["deep", "flagship", "flagship"]);
    expect(row("worker-planner", false)).toEqual(["light", "light", "standard"]);
    expect(row("worker-planner", true)).toEqual(["light", "standard", "deep"]);
    expect(row("worker-explorer", true)).toEqual(["light", "light", "light"]);
    expect(row("worker-backend-dev", false)).toEqual(["light", "standard", "deep"]);
    expect(row("worker-backend-dev", true)).toEqual(["standard", "deep", "flagship"]);
  });
});

describe("routing rules", () => {
  const C = "claude-agent-acp", X = "codex-acp";
  it("the engine switch writes a deny token and the last automatic engine cannot go", () => {
    const off = setEngineUse(rules(), C, false);
    expect(off.deny).toContain(`${C}/*`);
    expect(setEngineUse(off, C, true).deny).not.toContain(`${C}/*`);
    const one = setProvider(rules(), C, false);
    expect(one.providers).toEqual([X]);
    expect(setProvider(one, X, false).providers).toEqual([X]);
  });
  it("turning a model off clears Auto but keeps its stored policy; Auto turns it on", () => {
    const off = setModelEnabled(rules(), C, "haiku", false);
    expect(off.engines[C].enabled).not.toContain("haiku");
    expect(autoOn(off, C, "haiku")).toBe(false);
    expect(routingProblems(off, cfg(setup()).catalogue)).toEqual([]);
    expect(off.engines[C].policy.haiku.effortLadder).toEqual(["high", "xhigh", "max"]);
    expect(autoOn(setModelAuto(off, C, "haiku", true), C, "haiku")).toBe(true);
  });
  it("reports each policy mistake at its own field", () => {
    const cat = cfg(setup()).catalogue;
    expect(routingProblems(rules(), cat)).toEqual([]);
    const at = (r: ReturnType<typeof rules>) => Object.fromEntries(routingProblems(r, cat).map(p => [p.path, p.message]));
    expect(at(setTier(rules(), C, "haiku", "light", false))[`models.${C}.tiers.haiku`]).toBe("Choose a tier.");
    expect(at(setPolicy(rules(), C, "opus[1m]", { effortFloor: "xhigh", maxEffort: "low" }))[`models.${C}.policy.opus[1m].max_effort`]).toBe("Floor must not be above max.");
    expect(at(setPolicy(rules(), C, "haiku", { effortLadder: ["xhigh", "high"] }))[`models.${C}.policy.haiku.effort_ladder`]).toBe("Up to 3 levels, each higher.");
    expect(at(setPolicy(rules(), C, "haiku", { effortLadder: ["low", "medium", "high", "xhigh"] }))[`models.${C}.policy.haiku.effort_ladder`]).toBe("Up to 3 levels, each higher.");
    expect(at(setModelAuto(rules(), C, "opus[1m]", false))[`models.${C}.policy.opus[1m].preferred_for`]).toBe("Turn on Auto first.");
    expect(at(setPolicy(rules(), C, "opus[1m]", { preferredWhen: "x".repeat(501) }))[`models.${C}.policy.opus[1m].preferred_when`]).toContain("500");
    expect(at(setPolicy(rules(), C, "haiku", { requestNames: ["auto"] }))[`models.${C}.policy.haiku.request_names`]).toContain("auto");
    expect(at(setPolicy(rules(), X, "gpt-6-luna", { requestNames: ["opus"] }))[`models.${X}.policy.gpt-6-luna.request_names`]).toContain("Opus");
    expect(at(setPin(rules(), "worker-debugger", { engine: X, model: "gpt-6-sol", effort: null }))["pin.worker-debugger"]).toBe("This model is off.");
    const dup = rules(); dup.engines[C].quota.push({ window: "seven_day_opus", models: null });
    expect(at(dup)[`quota_windows.${C}[4].window`]).toBe("Already listed.");
  });
  it("a save with a mistake is refused and changes nothing", () => {
    const p = setup(), before = cfg(p).revisions.routing;
    expect(save(p, "routing", setTier(rules(), C, "haiku", "light", false)).ok).toBe(false);
    expect(cfg(p).revisions.routing).toBe(before);
  });
  it("affinity falls back to the built-in table, and null means no lean", () => {
    const r = setAffinity(rules(), "debugging", X);
    expect(r.affinity.debugging).toBe(X);
    expect(setAffinity(r, "debugging", "builtin").affinity.debugging).toBeUndefined();
    expect(setAffinity(r, "review", null).affinity.review).toBeNull();
  });
  it("each surface has its own revision, so a routing save never conflicts with defaults", () => {
    const p = setup(), { revisions } = cfg(p);
    send(p, { type: "save_surface", machineId: LOCAL, surface: "user_defaults", value: { ...cfg(p).surfaces.user_defaults, language: "en" }, revision: revisions.user_defaults });
    expect(cfg(p).revisions.routing).toBe(revisions.routing);
    expect(save(p, "routing", setPin(rules(), "*", null)).ok).toBe(true);
  });
  it("a stale save keeps the machine as it was and changed keys can be listed", () => {
    const p = setup(), base = structuredClone(cfg(p).surfaces.routing), { revisions } = cfg(p);
    send(p, { type: "config_changed", machineId: LOCAL, surface: "routing" });
    const mine = setProvider(base, C, false);
    expect(refuse(p, { type: "save_surface", machineId: LOCAL, surface: "routing", value: mine, revision: revisions.routing })).toContain("Your draft is kept");
    expect(diffPaths(base, cfg(p).surfaces.routing)).toEqual(["deny"]);
    expect(diffPaths(base, mine)).toEqual(["providers"]);
  });
  it("a project routing file replaces machine rules whole, and $CORTEX_ROUTING is read-only", () => {
    const p = setup();
    expect(routingSource(machine(p), "project")).toMatchObject({ kind: "machine" });
    send(p, { type: "save_surface", machineId: LOCAL, projectId: "project", surface: "routing", value: setProvider(rules(), C, false), revision: cfg(p).projects.project?.revisions?.routing ?? 0 });
    expect(routingSource(machine(p), "project")).toMatchObject({ kind: "project" });
    expect(cfg(p).surfaces.routing.providers).toHaveLength(2);
    send(p, { type: "set_config_fault", machineId: LOCAL, fault: "env_routing", on: true });
    expect(routingSource(machine(p), null)).toMatchObject({ kind: "environment" });
    expect(save(p, "routing", rules()).message).toContain("CORTEX_ROUTING");
  });
});

describe("route preview", () => {
  const input = { role: "worker-backend-dev", kind: "implementation_backend", complexity: "normal", attempt: "first" } as const;
  it("walks tier, preference and lean, and says why the rest are out", () => {
    const r = previewRoute(input, ctx());
    expect(r.tier).toBe("standard");
    expect(r.rows.map(x => x.model)).toEqual(["gpt-6.1-sol", "claude-sonnet-5", "gpt-6.1-terra"]);
    expect(r.rows[0]).toMatchObject({ tier: "standard", effort: "medium", decidedBy: "Preferred" });
    expect(r.rows[2].decidedBy).toBe("Lean to Codex");
    expect(r.excluded.find(x => x.model === "opus[1m]")?.reason).toBe("Not in this tier");
    expect(r.excluded.find(x => x.model === "gpt-6-luna")?.reason).toBe("Testing only for standard work");
    expect(r.excluded.find(x => x.model === "claude-opus-4-8")?.reason).toContain("Denied");
  });
  it("a stretch model joins standard work for its own kind", () => {
    expect(previewRoute({ ...input, kind: "testing" }, ctx()).rows.map(x => x.model)).toContain("gpt-6-luna");
  });
  it("ranks a hot plan behind cooler routes, and leaves out a full one", () => {
    const hot = previewRoute(input, ctx(setup(), { "claude-agent-acp": 96 }));
    expect(hot.rows[0].model).toBe("gpt-6.1-sol");
    const full = previewRoute(input, ctx(setup(), { "claude-agent-acp": 100 }));
    expect(full.rows.map(r => r.engine)).not.toContain("claude-agent-acp");
    expect(full.excluded.find(x => x.model === "claude-sonnet-5")?.reason).toBe("Plan limit reached");
  });
  it("reads usage through the rules it previews: saved, draft or project", () => {
    const m = structuredClone(machine(setup())), X = "codex-acp", sol = "gpt-6.1-sol";
    m.config.quotaObserved[X] = [{ window: "shared", percent: 100 }, { window: "sol", percent: 10 }];
    m.config.surfaces.routing.engines[X].quota = [{ window: "shared", models: null }];
    const mapped = structuredClone(m.config.surfaces.routing);
    mapped.engines[X].quota = [{ window: "sol", models: [sol] }, { window: "shared", models: null }];
    const row = (c: ReturnType<typeof previewContext>) => previewRoute(input, c).rows.find(r => r.model === sol);
    const reason = (c: ReturnType<typeof previewContext>) => previewRoute(input, c).excluded.find(x => x.model === sol)?.reason;
    expect(row(previewContext(m))).toBeUndefined();
    expect(reason(previewContext(m))).toBe("Plan limit reached");
    expect(usageFor(m, X, sol, mapped)).toBe(10);
    expect(row(previewContext(m, null, mapped))?.usage).toBe(10);
    m.config.projects.project = { routing: mapped };
    expect(row(previewContext(m, "project"))?.usage).toBe(10);
    expect(reason(previewContext(m, "project", m.config.surfaces.routing))).toBe("Plan limit reached");
  });
  it("flagship is only reached by escalation", () => {
    expect(previewRoute({ ...input, complexity: "high" }, ctx()).tier).toBe("deep");
    expect(previewRoute({ ...input, complexity: "high", attempt: "escalated" }, ctx()).tier).toBe("flagship");
  });
  it("keeps design work on its engine while a route there is eligible", () => {
    const r = previewRoute({ role: "worker-frontend-dev", kind: "implementation_frontend_design", complexity: "high", attempt: "first" }, ctx());
    expect(new Set(r.rows.map(x => x.engine))).toEqual(new Set(["claude-agent-acp"]));
  });
  it("a pin is one strict row; a pin that cannot run is refused, not moved", () => {
    const base = ctx();
    const pinned = { ...base, rules: setPin(base.rules, "worker-backend-dev", { engine: "codex-acp", model: "gpt-6-luna", effort: "high" }) };
    const r = previewRoute(input, pinned);
    expect(r.rows).toHaveLength(1);
    expect(r.rows[0]).toMatchObject({ model: "gpt-6-luna", decidedBy: "Pinned, strict", effort: "high" });
    const dead = { ...base, rules: setPin(setModelEnabled(base.rules, "codex-acp", "gpt-6.1-terra", false), "*", { engine: "codex-acp", model: "gpt-6.1-terra", effort: null }) };
    expect(previewRoute(input, dead).blocked?.code).toBe("pinned_route_unavailable");
    expect(previewRoute(input, { ...base, rules: setEngineUse(pinned.rules, "codex-acp", false) }).blocked?.code).toBe("route_denied");
  });
  it("says why nothing can run", () => {
    const base = ctx();
    const none = structuredClone(base.rules);
    for (const e of Object.values(none.engines)) e.auto = [];
    expect(previewRoute(input, { ...base, rules: none }).blocked?.code).toBe("no_available_route");
    expect(previewRoute(input, { ...base, rules: { ...base.rules, version: 1 } }).blocked?.code).toBe("model_policy_migration_required");
  });
  it("the limits narrow what can run", () => {
    const base = ctx();
    const ceiling = { ...base.ceiling, allowedConnections: ["codex-acp"] };
    expect(previewRoute(input, { ...base, ceiling }).rows.every(r => r.engine === "codex-acp")).toBe(true);
  });
  it("gives one cell per role and complexity", () => {
    const grid = previewGrid("first", ctx());
    expect(grid).toHaveLength(23);
    expect(grid[0].cells).toHaveLength(3);
  });
});

describe("limits", () => {
  it("a project may narrow the machine ceiling, never widen it, and shows what has stopped narrowing", () => {
    const p = setup(), base = cfg(p).surfaces.user_defaults;
    const machineCeiling = { ...base.ceiling, attempts: 4, allowedConnections: ["claude-agent-acp", "codex-acp"], consent: false };
    send(p, { type: "save_surface", machineId: LOCAL, surface: "user_defaults", value: { ...base, ceiling: machineCeiling }, revision: cfg(p).revisions.user_defaults });
    const project = { ...machineCeiling, attempts: 6, consent: true, allowedConnections: ["opencode"] };
    expect(ceilingProblems(project, machineCeiling).map(x => x.path).sort()).toEqual(["allowed_connections", "attempts", "fallback.consent"]);
    expect(refuse(p, { type: "save_surface", machineId: LOCAL, projectId: "project", surface: "project_ceiling", value: project, revision: cfg(p).projects.project?.revisions?.project_ceiling ?? 0 })).toContain("Must stay within the machine limit");
    const narrow = { ...machineCeiling, attempts: 2, allowedConnections: ["codex-acp"] };
    send(p, { type: "save_surface", machineId: LOCAL, projectId: "project", surface: "project_ceiling", value: narrow, revision: cfg(p).projects.project?.revisions?.project_ceiling ?? 0 });
    expect(effectiveCeiling(machine(p), "project")).toMatchObject({ attempts: 2, allowedConnections: ["codex-acp"] });
    expect(ceilingState({ ...machineCeiling, attempts: 2 }, { ...machineCeiling, attempts: 2 }).attempts).toBe("no_longer_narrows");
    expect(ceilingState(machineCeiling, narrow).attempts).toBe("narrows");
    send(p, { type: "save_surface", machineId: LOCAL, projectId: "project", surface: "project_ceiling", value: null, revision: cfg(p).projects.project?.revisions?.project_ceiling ?? 0 });
    expect(cfg(p).projects.project?.ceiling).toBeUndefined();
  });
  it("attempts accepts any whole number from zero, or none at all", () => {
    const c = cfg(setup()).surfaces.user_defaults.ceiling;
    expect(ceilingProblems({ ...c, attempts: 40 }, null)).toEqual([]);
    expect(ceilingProblems({ ...c, attempts: null }, null)).toEqual([]);
    expect(ceilingProblems({ ...c, attempts: -1 }, null).map(x => x.path)).toEqual(["attempts"]);
  });
});

describe("coordinator", () => {
  it("feeds new tasks and leaves running work alone", () => {
    const p = setup(), d = cfg(p).surfaces.user_defaults;
    send(p, { type: "save_surface", machineId: LOCAL, surface: "user_defaults", value: { ...d, coordinator: { ...d.coordinator!, model: "claude-sonnet-5", effort: "high" }, checkout: "main" }, revision: cfg(p).revisions.user_defaults });
    expect(machineDefaults(machine(p))).toEqual({ settings: { model: "claude-sonnet-5", effort: "high", mode: "development", language: "ru" }, checkout: "main" });
  });
  it("offers the model's observed levels, or a marked fallback", () => {
    const c = cfg(setup()).catalogue;
    expect(coordinatorEfforts(c, "claude-agent-acp", "opus[1m]")).toEqual({ levels: ["low", "medium", "high", "xhigh"], observed: true });
    expect(coordinatorEfforts(c, "codex-acp", "nothing")).toEqual({ levels: ["low", "medium", "high"], observed: false });
  });
  it("resolves to a route, or says why not", () => {
    const m = machine(setup());
    expect(coordinatorResolution(m)).toMatchObject({ ok: true });
    const off = structuredClone(m);
    off.config.surfaces.routing = setModelEnabled(off.config.surfaces.routing, "claude-agent-acp", "opus[1m]", false);
    expect(coordinatorResolution(off)).toMatchObject({ ok: false, code: "coordinator_model_disabled" });
  });
  it("refuses both a portable coordinator and an exact route", () => {
    const p = setup(), d = cfg(p).surfaces.user_defaults;
    const value = { ...d, routeDefault: { routeId: "xr_0123456789abcdef", connectionId: "cn_0123456789ab", model: "opus[1m]", parameters: {} } };
    expect(refuse(p, { type: "save_surface", machineId: LOCAL, surface: "user_defaults", value, revision: cfg(p).revisions.user_defaults })).toContain("Clear the exact route first");
    send(p, { type: "save_surface", machineId: LOCAL, surface: "user_defaults", value: { ...value, coordinator: null }, revision: cfg(p).revisions.user_defaults });
    expect(machineDefaults(machine(p)).settings.model).toBe("xr_0123456789abcdef");
  });
});

describe("MCP tool access", () => {
  const columns = ["coordinator", "read_only", "writing", "always"] as const;
  const subset = (a: string[], b: string[]) => a.every(x => b.includes(x));
  const legal = (s: ReturnType<typeof server>) => {
    expect(subset(s.readOnlyTools, s.allowedTools)).toBe(true);
    if (s.coordinatorTools) expect(subset(s.coordinatorTools, s.readOnlyTools)).toBe(true);
    if (s.alwaysLoadTools) expect(subset(s.alwaysLoadTools, s.allowedTools)).toBe(true);
  };
  it("cascades down the ladder and back", () => {
    const s0 = setCoordinatorMode(server("vivaldi"), "choose");
    const up = setGrant(s0, "chrome_navigate", "coordinator", true);
    expect(toolLevel(up.server, "chrome_navigate")).toBe("coordinator");
    expect(up.note).toBe("Also given to read-only and writing workers.");
    const mid = setGrant(up.server, "chrome_navigate", "read_only", false);
    expect(toolLevel(mid.server, "chrome_navigate")).toBe("writing");
    expect(mid.note).toBe("Also removed from the coordinator.");
    const down = setGrant(up.server, "chrome_navigate", "writing", false);
    expect(toolLevel(down.server, "chrome_navigate")).toBe("off");
    expect(down.note).toBe("Also removed from read-only workers and the coordinator.");
  });
  it("no sequence of ticks can ever make the lists illegal", () => {
    let s = setCoordinatorMode(server("codebase-memory-mcp"), "choose"), seed = 13014;
    const next = (n: number) => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) % n;
    const tools = s.allowedTools;
    for (let i = 0; i < 2000; i++) {
      s = setGrant(s, tools[next(tools.length)], columns[next(4)], next(2) === 0).server;
      legal(s);
    }
  });
  it("Same as read-only leaves the coordinator list unset and follows read-only", () => {
    const s = server("playwright");
    expect(s.coordinatorTools).toBeNull();
    const up = setGrant(s, "browser_snapshot", "read_only", true).server;
    expect(toolLevel(up, "browser_snapshot")).toBe("coordinator");
    expect(setGrant(up, "browser_snapshot", "coordinator", false).server).toEqual(up);
    expect(setCoordinatorMode(up, "choose").coordinatorTools).toEqual(["browser_snapshot"]);
    expect(setCoordinatorMode(setCoordinatorMode(up, "choose"), "same").coordinatorTools).toBeNull();
  });
  it("Always load needs the tool granted", () => {
    const s = server("vivaldi");
    const on = setGrant(s, "chrome_navigate", "always", true).server;
    expect(on.alwaysLoadTools).toEqual(["chrome_navigate"]);
    expect(setGrant(on, "chrome_navigate", "writing", false).server.alwaysLoadTools).toEqual([]);
    expect(setGrant(s, "nothing", "always", true).server.alwaysLoadTools ?? []).toEqual([]);
  });
  it("sets many tools at one level", () => {
    const s = setCoordinatorMode(server("vivaldi"), "choose");
    const r = setLevel(s, ["chrome_navigate", "chrome_keyboard"], "read_only");
    expect(r.readOnlyTools).toEqual(expect.arrayContaining(["chrome_navigate", "chrome_keyboard"]));
    const off = setLevel(r, ["chrome_navigate"], "off");
    expect(off.allowedTools).not.toContain("chrome_navigate");
    legal(off);
  });
  it("a role gets exactly what its class allows, on the engines the server allows", () => {
    const on = (name: string) => ({ ...server(name), enabled: true }), got = (s: ReturnType<typeof server>, role: string, engine: "claude-agent-acp" | "codex-acp" = "claude-agent-acp") => accessFor(s, role, engine);
    const s = on("codebase-memory-mcp");
    expect(got(s, "worker-backend-dev").tools).toHaveLength(15);
    expect(got(s, "worker-security-auditor").tools).toHaveLength(11);
    expect(got(s, "coordinator").tools).toHaveLength(8);
    expect(got(s, "coordinator").reason).toBeNull();
    expect(got(s, "worker-senior-consultant")).toEqual({ tools: [], granted: [], reason: "The consultant gets no external servers." });
    const pw = on("playwright");
    expect(got(pw, "worker-frontend-dev", "codex-acp")).toMatchObject({ tools: [], reason: "Not offered on Codex." });
    expect(got(pw, "worker-frontend-dev", "codex-acp").granted).toHaveLength(25);
    expect(got(pw, "worker-frontend-dev").tools).toHaveLength(25);
    expect(got(pw, "coordinator")).toMatchObject({ tools: [], reason: "For workers only." });
    expect(got({ ...pw, roles: ["coordinator"] }, "worker-frontend-dev")).toMatchObject({ tools: [], reason: "For the coordinator only." });
    expect(got(pw, "worker-code-reviewer")).toMatchObject({ tools: [], reason: null });
  });
  it("a server that never loads gives nothing, and its grants stay as configured", () => {
    const off = server("codebase-memory-mcp");
    expect(off.enabled).toBe(false);
    for (const [role, n] of [["coordinator", 8], ["worker-security-auditor", 11], ["worker-backend-dev", 15]] as const) {
      const r = accessFor(off, role, "claude-agent-acp");
      expect(r.tools).toEqual([]);
      expect(r.granted).toHaveLength(n);
      expect(r.reason).toBe("Server is off.");
    }
    const on = { ...off, enabled: true };
    expect(accessFor({ ...on, readPaths: ["/opt/missing/thing"] }, "coordinator", "claude-agent-acp")).toMatchObject({ tools: [], reason: "A path does not exist." });
    expect(accessFor({ ...on, problem: "Bad entry" }, "coordinator", "claude-agent-acp")).toMatchObject({ tools: [], reason: "Bad entry" });
  });
  it("new tools arrive with no access, and a tool the server dropped is kept and marked", () => {
    const s = server("playwright");
    const seen = mockConnect(s, null, 1000);
    expect(seen.ok).toBe(true);
    const rows = mcpTools(s, seen.ok ? seen.observation : null);
    const fresh = rows.find(t => t.name === "browser_pdf_save")!;
    expect(fresh).toMatchObject({ state: "new", level: "off" });
    expect(s.allowedTools).not.toContain("browser_pdf_save");
    const stale = { ...s, allowedTools: [...s.allowedTools, "browser_gone"] };
    expect(mcpTools(stale, seen.ok ? seen.observation : null).find(t => t.name === "browser_gone")).toMatchObject({ state: "not_offered", level: "writing" });
    expect(mcpTools(s, null).every(t => t.state === "granted" && !t.checked)).toBe(true);
  });
  it("suggests from the server's hints in the draft, and hints stay claims", () => {
    const s = server("codebase-memory-mcp"), seen = mockConnect(s, null, 1000);
    const obs = seen.ok ? seen.observation : null;
    const out = suggestLevels(s, mcpTools(s, obs));
    expect(toolLevel(out, "search_graph")).not.toBe("writing");
    expect(toolLevel(out, "delete_project")).toBe("off");
    expect(toolLevel(out, "index_repository")).toBe("writing");
  });
  it("warns about what the runtime would not tell you", () => {
    const seen = mockConnect(server("codebase-memory-mcp"), null, 1000), obs = seen.ok ? seen.observation : null;
    const texts = (s: ReturnType<typeof server>) => toolWarnings(s, mcpTools(s, obs)).map(w => w.text);
    const s = server("codebase-memory-mcp");
    expect(texts(setLevel(s, ["delete_project"], "read_only"))).toContain("The server says this tool changes things. Read-only roles would get it.");
    expect(texts({ ...s, readOnlyTools: [], coordinatorTools: null })).toContain("Reviewers, auditors and other read-only roles get nothing from this server.");
    expect(texts(setLevel(s, ["search_graph"], "off"))).toContain("All four are needed to use this as the code index.");
    expect(texts(setLevel(setCoordinatorMode(s, "choose"), ["index_repository"], "coordinator"))).toContain("Keep indexing for workers.");
    expect(texts({ ...s, alwaysLoadTools: s.allowedTools })).toContain("Each one costs prompt space in every Claude session.");
    expect(texts({ ...s, roles: ["worker"] })).toContain("These coordinator tools have no effect.");
  });
});

describe("MCP servers", () => {
  it("names, commands and tools follow the loader's rules", () => {
    const all = cfg(setup()).surfaces.mcp_servers;
    const at = (over: object, isNew = true) => Object.fromEntries(mcpProblems({ ...newMcpServer(), command: "/opt/x", allowedTools: ["a"], ...over }, all, isNew).map(p => [p.path, p.message]));
    expect(at({ name: "fresh" })).toEqual({});
    expect(at({ name: "cortex" }).name).toBe("Reserved");
    expect(at({ name: "playwright" }).name).toBe("Already exists");
    expect(at({ name: "bad name" }).name).toContain("letters");
    expect(at({ name: "fresh", command: "" }).command).toBeTruthy();
    expect(at({ name: "fresh", allowedTools: [] }).allowed_tools).toBe("Grant at least one tool.");
    expect(at({ name: "fresh", runtimeDirEnv: ["lower"] }).runtime_dir_env).toBe("Use A-Z, 0-9, _. PATH, HOME, TMPDIR and XDG_RUNTIME_DIR are reserved.");
    expect(at({ name: "fresh", runtimeDirEnv: ["HOME"] }).runtime_dir_env).toContain("reserved");
    expect(at({ name: "fresh", readPaths: ["relative/path"] })["read_paths[0]"]).toBeTruthy();
  });
  const split = (line: string) => { const r = parseCommandLine(line); return r.ok ? [r.command, ...r.args] : r.message; };
  it("splits a pasted command line, quotes included", () => {
    expect(parseCommandLine(`node /opt/x/cli.js --flag "two words" 'a b'`)).toEqual({ ok: true, command: "node", args: ["/opt/x/cli.js", "--flag", "two words", "a b"] });
    expect(split("  server   a\t b \n")).toEqual(["server", "a", "b"]);
    expect(split(`server "" ''`)).toEqual(["server", "", ""]);
  });
  it("joins quoted and unquoted parts of one word", () => {
    expect(split(`server --directory="/tmp/a b"`)).toEqual(["server", "--directory=/tmp/a b"]);
    expect(split(`server --directory='/tmp/a b' x`)).toEqual(["server", "--directory=/tmp/a b", "x"]);
    expect(split(`a"b c"'d e'f`)).toEqual(["ab cd ef"]);
  });
  it("reads escapes the way a shell does", () => {
    expect(split(String.raw`server /tmp/a\ b`)).toEqual(["server", "/tmp/a b"]);
    expect(split(String.raw`server "say \"hi\"" 'it\s' "a\b" "c\\d"`)).toEqual(["server", `say "hi"`, String.raw`it\s`, String.raw`a\b`, String.raw`c\d`]);
    expect(split(String.raw`server \$HOME 'a$b' "c\$d" 'x|y' "*"`)).toEqual(["server", "$HOME", "a$b", "c$d", "x|y", "*"]);
    expect(split("npx -y pkg \\\n  --port 9")).toEqual(["npx", "-y", "pkg", "--port", "9"]);
    expect(split("npx pkg \\\r\n  --port 9")).toEqual(["npx", "pkg", "--port", "9"]);
  });
  it("refuses what it cannot split faithfully, and says why", () => {
    for (const line of [`server "a`, `server 'a`, String.raw`server "a\"`]) expect(split(line)).toBe("A quote is not closed.");
    expect(split("server a\\")).toBe("The line ends in a backslash.");
    for (const line of ["a | b", "a && b", "a; b", "a > out", "a < in", "echo $HOME", `echo "$HOME"`, "echo `id`", 'echo "`id`"', "echo $(id)", "(a)", "ls *.txt", "ls a?", "a {b,c}", "a # note"]) expect(split(line)).toContain("needs a shell");
    expect(split("first\nsecond")).toBe("Paste one command at a time.");
    expect(split("~/bin/server --x")).toBe("Use a full path, starting with /.");
    expect(split("TOKEN=abc123 server")).toBe("Add variables under Environment, then paste the command alone.");
    for (const line of ["", "   ", `"" a`]) expect(split(line)).toBe("Paste the command that starts the server.");
  });
  it("keeps text that only looks like shell syntax", () => {
    expect(split("server a=b --x=y#z a~b")).toEqual(["server", "a=b", "--x=y#z", "a~b"]);
    expect(split(`"TOKEN=abc" server '~/x' '#c'`)).toEqual(["TOKEN=abc", "server", "~/x", "#c"]);
  });
  it("simulated connect covers every failure with fixed text", () => {
    const s = server("playwright");
    for (const [forced, code] of [["command_not_found", "command_not_found"], ["exited", "exited"], ["timeout", "timeout"], ["path_refused", "path_refused"], ["needs_variable", "needs_variable"], ["not_supported", "not_supported"]] as const) {
      const r = mockConnect(s, forced, 1);
      expect(r.ok).toBe(false);
      if (!r.ok) { expect(r.code).toBe(code); expect(r.message.length).toBeGreaterThan(0); }
    }
    const r = mockConnect({ ...newMcpServer(), name: "x", command: "" }, null, 1);
    expect(r).toMatchObject({ ok: false, code: "command_not_found", field: "command" });
    const missing = mockConnect({ ...s, readPaths: ["/opt/missing/thing"] }, null, 1);
    expect(missing).toMatchObject({ ok: false, code: "path_refused" });
  });
  it("saving never stores an env value in the clear, and keep, replace and remove work", () => {
    const p = setup(), list = structuredClone(cfg(p).surfaces.mcp_servers);
    list[0].env = [{ name: "KEEP", stored: true, action: "keep", value: "" }, { name: "SWAP", stored: true, action: "replace", value: "new" }, { name: "GONE", stored: true, action: "remove", value: "" }, { name: "FRESH", stored: false, action: "replace", value: "v" }];
    expect(save(p, "mcp_servers", list).ok).toBe(true);
    expect(cfg(p).surfaces.mcp_servers[0].env).toEqual([{ name: "KEEP", stored: true, action: "keep", value: "" }, { name: "SWAP", stored: true, action: "keep", value: "" }, { name: "FRESH", stored: true, action: "keep", value: "" }]);
  });
  it("a save with no tools granted is refused, and a saved check keeps what it saw", () => {
    const p = setup(), list = structuredClone(cfg(p).surfaces.mcp_servers);
    list[0].allowedTools = []; list[0].readOnlyTools = []; list[0].coordinatorTools = null; list[0].alwaysLoadTools = null;
    expect(save(p, "mcp_servers", list).ok).toBe(false);
    const seen = mockConnect(server("vivaldi"), null, 5);
    send(p, { type: "record_mcp_tools", machineId: LOCAL, server: "vivaldi", observation: seen.ok ? seen.observation : null });
    expect(cfg(p).mcpObserved.vivaldi?.at).toBe(5);
  });
  it("reads a server's state, worst first", () => {
    const s = server("playwright");
    expect(serverStatus({ ...s, enabled: true }).text).toBe("On");
    expect(serverStatus(s).text).toBe("Off");
    expect(serverStatus({ ...s, enabled: true, readPaths: ["/opt/missing/x"] }).text).toBe("Will not start: a path does not exist");
    expect(serverStatus({ ...s, unsupportedKeys: ["timeout"] }).text).toBe("Has settings this app does not manage. Turn on, off or remove only.");
    expect(serverStatus({ ...s, problem: "Invalid" }).text).toBe("Invalid");
  });
});

describe("profiles", () => {
  const edit = (p: WorkspacePort, language = "en") => { const d = cfg(p).surfaces.user_defaults; send(p, { type: "save_surface", machineId: LOCAL, surface: "user_defaults", value: { ...d, language }, revision: cfg(p).revisions.user_defaults }); };
  it("states follow the machine, and differences name keys only", () => {
    const p = setup();
    expect(profileState(cfg(p))).toBe("in_sync");
    edit(p);
    expect(profileState(cfg(p))).toBe("modified");
    expect(profileDifferences(cfg(p))).toEqual([{ section: "defaults", keys: ["language"] }]);
    send(p, { type: "save_profile", machineId: LOCAL });
    expect(profileState(cfg(p))).toBe("in_sync");
    send(p, { type: "deactivate_profile", machineId: LOCAL });
    expect(profileState(cfg(p))).toBe("none");
  });
  it("switching with changes either saves them or drops them", () => {
    const p = setup();
    send(p, { type: "create_profile", machineId: LOCAL, name: "night-runs", description: "For long runs" });
    edit(p);
    send(p, { type: "activate_profile", machineId: LOCAL, profileId: "default", keep: "save" });
    expect(cfg(p).surfaces.user_defaults.language).toBe("ru");
    send(p, { type: "activate_profile", machineId: LOCAL, profileId: cfg(p).profiles[1].id, keep: "save" });
    expect(cfg(p).surfaces.user_defaults.language).toBe("en");
    edit(p, "auto");
    send(p, { type: "activate_profile", machineId: LOCAL, profileId: "default", keep: "discard" });
    expect(cfg(p).profiles[1].sections.defaults?.language).toBe("en");
    expect(cfg(p).surfaces.user_defaults.language).toBe("ru");
  });
  it("never captures commands, arguments, environment or project settings", () => {
    const p = setup(), list = structuredClone(cfg(p).surfaces.mcp_servers);
    list[0].command = "/opt/moved/server"; list[0].enabled = true;
    expect(save(p, "mcp_servers", list).ok).toBe(true);
    expect(profileDifferences(cfg(p))).toEqual([{ section: "mcp", keys: ["codebase-memory-mcp.enabled"] }]);
    send(p, { type: "revert_profile", machineId: LOCAL });
    expect(cfg(p).surfaces.mcp_servers[0]).toMatchObject({ command: "/opt/moved/server", enabled: false });
    expect(JSON.stringify(cfg(p).profiles[0].sections.mcp)).not.toContain("/opt/moved/server");
  });
  it("names follow the real rule, and the active one cannot be deleted", () => {
    const p = setup();
    for (const name of ["Mine", "under_score", "-x"]) refuse(p, { type: "create_profile", machineId: LOCAL, name, description: "" });
    send(p, { type: "create_profile", machineId: LOCAL, name: "mine-2", description: "" });
    refuse(p, { type: "delete_profile", machineId: LOCAL, profileId: cfg(p).active! });
    send(p, { type: "activate_profile", machineId: LOCAL, profileId: "default", keep: "discard" });
    send(p, { type: "delete_profile", machineId: LOCAL, profileId: cfg(p).profiles[1].id });
    expect(cfg(p).profiles).toHaveLength(1);
  });
  it("an interrupted activation is reported and recovered", () => {
    const p = setup();
    send(p, { type: "set_config_fault", machineId: LOCAL, fault: "interrupted", on: true });
    expect(profileState(cfg(p))).toBe("interrupted");
    expect(configIssues(machine(p)).map(i => i.id)).toContain("profile");
    send(p, { type: "activate_profile", machineId: LOCAL, profileId: "default", keep: "discard" });
    expect(profileState(cfg(p))).toBe("in_sync");
  });
});

describe("files, presets and faults", () => {
  it("a reset needs confirmation and replaces routing and defaults only", () => {
    const p = setup(), list = structuredClone(cfg(p).surfaces.mcp_servers);
    list[0].enabled = true;
    expect(save(p, "mcp_servers", list).ok).toBe(true);
    expect(save(p, "routing", setProvider(rules(), "codex-acp", false)).ok).toBe(true);
    refuse(p, { type: "reset_preset", machineId: LOCAL, confirmed: false });
    send(p, { type: "reset_preset", machineId: LOCAL, confirmed: true });
    expect(cfg(p).surfaces.routing.providers).toHaveLength(2);
    expect(cfg(p).surfaces.mcp_servers[0].enabled).toBe(true);
    expect(cfg(p).files.backups).toBe(2);
  });
  it("apply seeds absent files only, and a migration needs consent", () => {
    const p = setup();
    refuse(p, { type: "apply_preset", machineId: LOCAL });
    send(p, { type: "set_config_fault", machineId: LOCAL, fault: "migration", on: true });
    expect(configIssues(machine(p)).map(i => i.id)).toContain("migration");
    refuse(p, { type: "migrate_policy", machineId: LOCAL, confirmed: false });
    send(p, { type: "migrate_policy", machineId: LOCAL, confirmed: true });
    expect(configIssues(machine(p)).map(i => i.id)).not.toContain("migration");
  });
  it("issues name a server with a missing path and an unreadable model list", () => {
    const p = setup(), list = structuredClone(cfg(p).surfaces.mcp_servers);
    list[2].enabled = true; list[2].readPaths = ["/opt/missing/x"];
    expect(save(p, "mcp_servers", list).ok).toBe(true);
    send(p, { type: "set_config_fault", machineId: LOCAL, fault: "catalogue", on: true });
    const issues = configIssues(machine(p));
    expect(issues.find(i => i.id === "path:vivaldi")).toMatchObject({ page: "tools", server: "vivaldi" });
    expect(issues.find(i => i.id === "catalogue")).toBeTruthy();
    send(p, { type: "refresh_models", machineId: LOCAL, now: 9 });
    expect(cfg(p).catalogue["codex-acp"]).toMatchObject({ observedAt: 9 });
  });
  it("offline machines write nothing", () => {
    const p = setup();
    send(p, { type: "add_remote", id: "remote", name: "Remote" });
    const remote = () => p.snapshot().machines[1];
    expect(refuse(p, { type: "save_surface", machineId: "remote", surface: "user_defaults", value: remote().config.surfaces.user_defaults, revision: remote().config.revisions.user_defaults })).toContain("read-only");
    for (const c of [{ type: "activate_profile", machineId: "remote", profileId: "default", keep: "discard" }, { type: "save_profile", machineId: "remote" }, { type: "refresh_models", machineId: "remote", now: 1 }, { type: "reset_preset", machineId: "remote", confirmed: true }] as WorkspaceCommand[]) expect(refuse(p, c)).toContain("read-only");
  });
  it("connections gain the real operations", () => {
    const p = setup();
    send(p, { type: "rename_connection", machineId: LOCAL, connectionId: "codex-acp", name: "Codex at work" });
    expect(machine(p).connections.find(c => c.id === "codex-acp")?.label).toBe("Codex at work");
    refuse(p, { type: "rename_connection", machineId: LOCAL, connectionId: "codex-acp", name: "x".repeat(121) });
    send(p, { type: "set_config_fault", machineId: LOCAL, fault: "engine_missing", on: true });
    expect(cfg(p).setup["codex-acp"].status).toBe("not_downloaded");
    send(p, { type: "download_engine", machineId: LOCAL, engine: "codex-acp" });
    expect(cfg(p).setup["codex-acp"].status).toBe("downloaded_no_login");
    refuse(p, { type: "download_engine", machineId: LOCAL, engine: "opencode" });
    send(p, { type: "enable_opencode", machineId: LOCAL, name: "OpenCode" });
    expect(cfg(p).setup.opencode.status).toBe("connected");
  });
});
