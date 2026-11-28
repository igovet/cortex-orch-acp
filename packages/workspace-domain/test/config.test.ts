import { describe, expect, it } from "vitest";
import { addCoordinatorModel, consultationListed, kindLabel, ROLE_CATALOG, WORK_KINDS, setPreferred, capture, configIssues, coordinatorAllowed, coordinatorChoices, coordinatorSeed, removeCoordinatorModel, setCoordinatorTier, startCoordinatorList, coordinatorEfforts, coordinatorResolution, emptyWorkspace, machineDefaults, NEUTRAL_POLICY, orderedEfforts, permitted, policyOf, previewRoute, profileChanges, resolveEffort, routingProblems, sessionsOf, setTier, setTierRange, tierEffort, tierFor, tierRange } from "../src";

describe("automatic role tiers", () => {
  it("scales planning, general work and exploration by complexity", () => {
    const complexities = ["low", "normal", "high"] as const;
    for (const role of ["worker-general", "worker-explorer", "worker-planner", "worker-technical-writer"]) {
      expect(complexities.map(complexity => tierFor(role, false, complexity))).toEqual(["light", "standard", "deep"]);
    }
    expect(complexities.map(complexity => tierFor("worker-explorer", true, complexity))).toEqual(["standard", "deep", "flagship"]);
  });
});

describe("task default routes", () => {
  it("starts with the configured effort, or medium for absent and legacy defaults", () => {
    const machine = emptyWorkspace().machines[0];
    const defaults = machine.config.surfaces.user_defaults;
    defaults.coordinator = null;
    defaults.routeDefault = null;
    expect(machineDefaults(machine).settings.effort).toBe("medium");
    const choice = defaults.coordinator = {engine:"claude-agent-acp",model:"opus",effort:"high",parameters:{}};
    machine.config.catalogue["claude-agent-acp"].models = [{id:"opus",label:"Opus",efforts:["default","high","medium","low"],observed:true}];
    expect(machineDefaults(machine).settings.effort).toBe("high");
    choice.effort = "default";
    expect(machineDefaults(machine).settings.effort).toBe("medium");
    expect(resolveEffort("default", "high", ["low", "high"])).toBe("high");
    expect(resolveEffort("", "", ["high", "low"])).toBe("low");
    expect(resolveEffort("", "", ["default"])).toBe("");
    expect(orderedEfforts(["default", "max", "medium", "high", "xhigh", "low", "engine_default"])).toEqual(["low", "medium", "high", "xhigh", "max"]);
  });
  it("keeps the exact configured connection route instead of picking the first model", () => {
    const machine = emptyWorkspace().machines[0];
    machine.config.surfaces.user_defaults.coordinator = {engine:"claude-agent-acp",model:"opus",effort:"high",parameters:{}};
    machine.config.catalogue["claude-agent-acp"].models = [{id:"opus",routeId:"route:owner",label:"Opus",efforts:["high"],observed:true}];
    expect(machineDefaults(machine).settings.model).toBe("route:owner");
  });
  it("keeps an exact default route identity, including when its model has withdrawn", () => {
    const machine = emptyWorkspace().machines[0];
    machine.config.surfaces.user_defaults.coordinator = null;
    machine.config.surfaces.user_defaults.routeDefault = {routeId:"route:withdrawn",connectionId:"connection",model:"opus",parameters:{thought_level:"high"}};
    expect(machineDefaults(machine).settings.model).toBe("route:withdrawn");
  });
});

describe("profile changes in words", () => {
  const changed = () => {
    const machine = emptyWorkspace().machines[0], c = machine.config;
    c.catalogue["claude-agent-acp"].models = [{id:"opus",label:"Opus 5.5",efforts:["high"],observed:true}];
    c.surfaces.mcp_servers = [{name:"memory",command:"m",args:[],env:[],enabled:true,roles:null,engines:null,allowedTools:[],readOnlyTools:[],coordinatorTools:null,alwaysLoadTools:null,readPaths:[],writePaths:[],network:false,runtimeDirEnv:[],handTools:[],unsupportedKeys:[],problem:null}];
    machine.connections = [{id:"cn_ab12cd34ef56",engine:"claude",label:"Claude",note:"",sessions:2,state:"connected",checkedAt:null,detail:{connectionId:"cn_ab12cd34ef56",provider:"",authMode:"",keyKind:null,endpoint:null,launchCommand:"",credential:"",qualified:true,createdAt:"",updatedAt:""}}];
    c.profiles = [{id:"p",name:"p",description:"",updatedAt:null,sections:capture(c.surfaces,sessionsOf(machine))}];
    c.active = "p";
    return machine;
  };
  it("has nothing to say when nothing changed", () => expect(profileChanges(changed())).toEqual([]));
  it("names models, settings, servers and connections, and no key or id", () => {
    const machine = changed(), c = machine.config;
    c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
    c.surfaces.routing.engines["claude-agent-acp"].policy = {opus:{effortFloor:null,maxEffort:"high",effortLadder:null,alsoStandardFor:null,preferredFor:null,preferredWhen:null,requestNames:null,tierEfforts:null}};
    c.surfaces.user_defaults.language = "ru";
    c.surfaces.user_defaults.ceiling = {...c.surfaces.user_defaults.ceiling,attempts:3};
    c.surfaces.mcp_servers[0].enabled = false;
    machine.connections[0].sessions = 4;
    const lines = profileChanges(machine).flatMap(x => x.lines);
    expect(lines).toEqual(["Opus 5.5 turned on", "Opus 5.5 max effort: High", "Language: Russian", "Limits: fallback attempts changed", "memory turned off", "Claude: 4 sessions at most, was 2"]);
  });
  it("still says something for a setting with no sentence of its own", () => {
    const machine = changed();
    machine.config.surfaces.routing.legacyTiers = ["x"];
    expect(profileChanges(machine)).toEqual([{section:"routing",lines:["Other settings changed"]}]);
  });
  it("reads an exact route by its model", () => {
    const machine = changed(), c = machine.config;
    c.surfaces.user_defaults.routeDefault = {routeId:"xr_6b08617e228a",connectionId:"cn_ab12cd34ef56",model:"opus",parameters:{}};
    expect(coordinatorResolution(machine)).toEqual({ok:true,route:"Claude · Opus 5.5, exact route"});
  });
});

describe("a saved config that still names the retired default alias", () => {
  it("is reported as no longer offered, never mapped to another model", () => {
    const machine = emptyWorkspace().machines[0], routing = machine.config.surfaces.routing;
    machine.config.catalogue["claude-agent-acp"].models = [{id:"opus",label:"Opus 5.5",efforts:["high"],observed:true}];
    routing.engines["claude-agent-acp"].enabled = ["default", "opus"];
    const issue = configIssues(machine).find(i => i.id === "offered:claude-agent-acp");
    expect(issue?.text).toContain("no longer offers the engine alias \"default\"");
    expect(issue?.text).not.toContain("opus");
  });
});

describe("tier effort ranges and the coordinator profile", () => {
  const ALL = ["low", "medium", "high", "xhigh", "max"];
  const world = () => {
    const machine = emptyWorkspace().machines[0], c = machine.config, r = c.surfaces.routing;
    c.catalogue["claude-agent-acp"].models = [
      {id:"opus",label:"Opus",efforts:ALL,observed:true}, {id:"sonnet",label:"Sonnet",efforts:ALL,observed:true}, {id:"haiku",label:"Haiku",efforts:["low","medium","high"],observed:true},
    ];
    r.engines["claude-agent-acp"].enabled = ["opus", "sonnet", "haiku"];
    return { machine, c, r };
  };
  const claude = "claude-agent-acp" as const;
  it("validates ranges by tier, placement, order and level", () => {
    const { c, r } = world();
    r.providers = [claude];
    r.engines[claude].tiers = { sonnet: ["deep"] };
    r.engines[claude].policy = { sonnet: { ...NEUTRAL_POLICY, tierEfforts: { deep: { min: "xhigh", max: "high" }, light: { min: "low", max: "low" }, ...({ huge: { min: "low", max: "low" } } as object) } } };
    expect(routingProblems(r, c.catalogue).filter(p => p.path.includes("tier_efforts"))).toEqual([
      { path: "models.claude-agent-acp.policy.sonnet.tier_efforts.deep", message: "Lowest must not be above highest." },
      { path: "models.claude-agent-acp.policy.sonnet.tier_efforts.light", message: "Place the model in this tier first, or remove its range." },
      { path: "models.claude-agent-acp.policy.sonnet.tier_efforts.huge", message: "\"huge\" is not a tier." },
    ]);
    r.engines[claude].policy.sonnet.tierEfforts = { deep: { min: "low", max: "ultra" } };
    expect(routingProblems(r, c.catalogue).map(p => p.message)).toEqual(["Choose a lowest and a highest effort."]);
    r.engines[claude].policy.sonnet.tierEfforts = { deep: { min: "xhigh", max: "xhigh" } };
    expect(routingProblems(r, c.catalogue)).toEqual([]);
  });
  it("permits only the range on a tier that has one, max included, and keeps floor and max elsewhere", () => {
    const ranged = { ...NEUTRAL_POLICY, tierEfforts: { deep: { min: "xhigh", max: "max" } } };
    expect(permitted(ranged, ALL, "deep")).toEqual(["xhigh", "max"]);
    expect(permitted(ranged, ALL, "flagship")).toEqual(["low", "medium", "high"]);
    expect(permitted({ ...NEUTRAL_POLICY, effortFloor: "medium", maxEffort: "xhigh" }, ALL, "standard")).toEqual(["medium", "high", "xhigh"]);
    expect(permitted(NEUTRAL_POLICY, ALL, "light")).toEqual(ALL);
    expect(permitted(NEUTRAL_POLICY, ALL, "standard")).toEqual(["low", "medium", "high", "xhigh"]);
    expect(tierEffort({ ...NEUTRAL_POLICY, tierEfforts: { standard: { min: "high", max: "xhigh" } } }, ALL, "standard")).toBe("high");
  });
  it("sets and clears a range, and dropping a tier drops its range", () => {
    const { r } = world();
    let next = setTier(r, claude, "sonnet", "deep", true);
    next = setTierRange(next, claude, "sonnet", "deep", { min: "xhigh", max: "xhigh" });
    expect(tierRange(next, claude, "sonnet", "deep")).toEqual({ min: "xhigh", max: "xhigh" });
    expect(policyOf(setTierRange(next, claude, "sonnet", "deep", null), claude, "sonnet").tierEfforts).toBeNull();
    expect(policyOf(setTier(next, claude, "sonnet", "deep", false), claude, "sonnet").tierEfforts).toBeNull();
  });
  it("routes a ranged tier inside its range", () => {
    const { c, r } = world();
    r.engines[claude].auto = ["sonnet"]; r.engines[claude].tiers = { sonnet: ["deep"] };
    r.engines[claude].policy = { sonnet: { ...NEUTRAL_POLICY, tierEfforts: { deep: { min: "xhigh", max: "xhigh" } } } };
    r.providers = [claude];
    const preview = previewRoute({ role: "worker-architect", kind: "architecture", complexity: "normal", attempt: "first" }, { rules: r, ceiling: c.surfaces.user_defaults.ceiling, catalogue: c.catalogue });
    expect(preview.rows[0]).toMatchObject({ model: "sonnet", tier: "deep", effort: "xhigh" });
  });
  it("offers the coordinator only models with a tier and the union of their tiers' efforts", () => {
    const { machine, c, r } = world();
    expect(coordinatorChoices(r, c.catalogue)).toBeNull();
    expect(coordinatorEfforts(c.catalogue, claude, "haiku", r)).toEqual({ levels: ["low", "medium", "high"], observed: true });
    r.engines[claude].tiers = { opus: ["flagship"], sonnet: ["deep", "standard"] };
    r.engines[claude].policy = { opus: { ...NEUTRAL_POLICY, tierEfforts: { flagship: { min: "low", max: "high" } } }, sonnet: { ...NEUTRAL_POLICY, tierEfforts: { deep: { min: "xhigh", max: "xhigh" }, standard: { min: "high", max: "xhigh" } } } };
    expect(coordinatorChoices(r, c.catalogue)).toEqual([
      { engine: claude, model: "opus", label: "Opus", efforts: ["low", "medium", "high"] },
      { engine: claude, model: "sonnet", label: "Sonnet", efforts: ["high", "xhigh"] },
    ]);
    expect(coordinatorAllowed(r, claude, "haiku", ["low"])).toEqual([]);
    expect(coordinatorEfforts(c.catalogue, claude, "haiku", r)).toEqual({ levels: ["low", "medium", "high"], observed: true, outside: true });
    r.deny = ["opus"];
    expect(coordinatorChoices(r, c.catalogue)?.map(x => x.model)).toEqual(["sonnet"]);
    // Out of the profile, the coordinator is reported, not silently moved.
    c.surfaces.user_defaults.coordinator = { engine: claude, model: "haiku", effort: "medium", parameters: {} };
    expect(coordinatorResolution(machine)).toMatchObject({ ok: false, code: "coordinator_outside_profile", text: "This model has no tier in Models, so it cannot coordinate." });
  });
  it("clamps the inherited medium default into the only range a deep model has", () => {
    const { machine, c, r } = world();
    r.engines[claude].tiers = { sonnet: ["deep"] };
    r.engines[claude].policy = { sonnet: { ...NEUTRAL_POLICY, tierEfforts: { deep: { min: "xhigh", max: "xhigh" } } } };
    c.surfaces.user_defaults.coordinator = { engine: claude, model: "sonnet", effort: "medium", parameters: {} };
    expect(machineDefaults(machine).settings.effort).toBe("xhigh");
    expect(coordinatorResolution(machine).ok).toBe(true);
    // Without any tier anywhere nothing narrows it.
    r.engines[claude].tiers = {};
    expect(machineDefaults(machine).settings.effort).toBe("medium");
  });
});

describe("the coordinator list, apart from the worker tiers", () => {
  const ALL = ["low", "medium", "high", "xhigh", "max"];
  const claude = "claude-agent-acp" as const, codex = "codex-acp" as const;
  const world = () => {
    const machine = emptyWorkspace().machines[0], c = machine.config, r = c.surfaces.routing;
    c.catalogue[claude].models = [{ id: "opus", label: "Opus", efforts: ALL, observed: true }, { id: "sonnet", label: "Sonnet", efforts: ALL, observed: true }, { id: "haiku", label: "Haiku", efforts: ["low", "medium", "high"], observed: true }];
    c.catalogue[codex].models = [{ id: "gpt-6.2-sol", label: "Sol", efforts: ["low", "medium", "high", "xhigh"], observed: true }];
    r.engines[claude].enabled = ["opus", "sonnet", "haiku"];
    r.engines[codex].enabled = ["gpt-6.2-sol"];
    // Worker tiers that a coordinator list must not follow.
    r.engines[claude].tiers = { haiku: ["light"], sonnet: ["standard"] };
    return { machine, c, r };
  };
  it("offers only the listed models, at the union of their ranges the model offers", () => {
    const { machine, c, r } = world();
    r.coordinatorModels = [
      { engine: claude, model: "opus", tiers: { flagship: { min: "low", max: "high" } } },
      { engine: codex, model: "gpt-6.2-sol", tiers: { deep: { min: "xhigh", max: "xhigh" }, standard: { min: "high", max: "xhigh" }, light: { min: "medium", max: "max" } } },
    ];
    expect(coordinatorChoices(r, c.catalogue)).toEqual([
      { engine: claude, model: "opus", label: "Opus", efforts: ["low", "medium", "high"] },
      // Light's range ends at max, which Sol does not offer: that range adds nothing.
      { engine: codex, model: "gpt-6.2-sol", label: "Sol", efforts: ["high", "xhigh"] },
    ]);
    // Sonnet keeps its worker tier but is not listed.
    expect(coordinatorAllowed(r, claude, "sonnet", ALL)).toEqual([]);
    expect(coordinatorEfforts(c.catalogue, claude, "sonnet", r)).toMatchObject({ outside: true });
    c.surfaces.user_defaults.coordinator = { engine: claude, model: "sonnet", effort: "medium", parameters: {} };
    expect(coordinatorResolution(machine)).toMatchObject({ ok: false, code: "coordinator_outside_profile", text: "This model is not in Coordinator models, so it cannot coordinate." });
    // A listed model moves the saved effort to the nearest allowed level.
    c.surfaces.user_defaults.coordinator = { engine: codex, model: "gpt-6.2-sol", effort: "medium", parameters: {} };
    expect(coordinatorResolution(machine).ok).toBe(true);
    expect(machineDefaults(machine).settings.effort).toBe("high");
    // Switched off or refused, a listed model is not offered.
    r.deny = ["opus"];
    expect(coordinatorChoices(r, c.catalogue)?.map(x => x.model)).toEqual(["gpt-6.2-sol"]);
  });
  it("falls back to the worker tiers for a profile without the list", () => {
    const { c, r } = world();
    expect(r.coordinatorModels).toBeUndefined();
    expect(coordinatorChoices(r, c.catalogue)?.map(x => x.model)).toEqual(["sonnet", "haiku"]);
  });
  it("edits the list, seeds it from the worker tiers and reports what the save refuses", () => {
    const { c, r } = world();
    r.providers = [claude];
    r.engines[claude].policy = { sonnet: { ...NEUTRAL_POLICY, tierEfforts: { standard: { min: "high", max: "xhigh" } } } };
    expect(coordinatorSeed(r, c.catalogue)).toEqual([
      { engine: claude, model: "sonnet", tiers: { standard: { min: "high", max: "xhigh" } } },
      { engine: claude, model: "haiku", tiers: { light: { min: "low", max: "high" } } },
    ]);
    let next = startCoordinatorList(r, c.catalogue);
    expect(next.coordinatorModels?.map(x => x.model)).toEqual(["sonnet", "haiku"]);
    next = addCoordinatorModel(next, c.catalogue, codex, "gpt-6.2-sol");
    expect(next.coordinatorModels?.at(-1)).toEqual({ engine: codex, model: "gpt-6.2-sol", tiers: { standard: { min: "medium", max: "high" } } });
    next = setCoordinatorTier(next, codex, "gpt-6.2-sol", "flagship", { min: "low", max: "high" });
    expect(Object.keys(next.coordinatorModels!.at(-1)!.tiers)).toEqual(["flagship", "standard"]);
    next = setCoordinatorTier(next, codex, "gpt-6.2-sol", "standard", null);
    next = setCoordinatorTier(next, codex, "gpt-6.2-sol", "flagship", null);
    expect(routingProblems(next, c.catalogue)).toEqual([{ path: "coordinator_models.codex-acp.gpt-6.2-sol", message: "Choose at least one category." }]);
    next = setCoordinatorTier(next, codex, "gpt-6.2-sol", "deep", { min: "xhigh", max: "high" });
    expect(routingProblems(next, c.catalogue)).toEqual([{ path: "coordinator_models.codex-acp.gpt-6.2-sol.deep", message: "Lowest must not be above highest." }]);
    for (const x of next.coordinatorModels!) next = removeCoordinatorModel(next, x.engine, x.model);
    expect(next.coordinatorModels).toEqual([]);
    expect(routingProblems(next, c.catalogue)).toEqual([{ path: "coordinator_models", message: "Keep at least one model, so a task can start." }]);
  });
});

describe("senior consultation work kind", () => {
  it("is its own kind used only by the senior consultant", () => {
    expect(kindLabel("consultation")).toBe("Senior consultation");
    expect(WORK_KINDS.some(k => k.id === "consultation")).toBe(true);
    expect(ROLE_CATALOG.filter(r => r.kind === "consultation").map(r => r.id)).toEqual(["worker-senior-consultant"]);
    expect(ROLE_CATALOG.filter(r => r.kind === "design").map(r => r.id)).not.toContain("worker-senior-consultant");
  });
  it("reports whether any model lists it, so the Design list applies until then", () => {
    const rules = emptyWorkspace().machines[0].config.surfaces.routing;
    expect(consultationListed(rules)).toBe(false);
    const next = setPreferred(rules, "claude-agent-acp", "opus", "consultation", true);
    expect(consultationListed(next)).toBe(true);
  });
});
