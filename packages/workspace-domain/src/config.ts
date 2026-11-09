import type { Ceiling, EngineKey, Machine, MachineConfig, McpPolicy, McpServer, Problem, Profile, ProfileSection, ProfileSections, RoutingRules, SurfaceKey, TaskSettings, UserDefaults } from "./types";
import { engineLabel, ENGINE_KEYS } from "./catalog";
import { pathMissing } from "./mcp";
import { modelLabel, notOffered, type PreviewContext } from "./routing-rules";

export const SURFACE_LABELS: Record<SurfaceKey | "connections", string> = { routing: "routing rules", user_defaults: "coordinator and limits", project_ceiling: "project limits", mcp_servers: "MCP servers", connections: "connections" };
export const PROFILE_SECTION_LABELS: Record<ProfileSection, string> = { routing: "routing", defaults: "defaults and ceiling", mcp: "MCP usage policy", sessions: "session limits" };
export const PROFILE_SECTIONS = Object.keys(PROFILE_SECTION_LABELS) as ProfileSection[];
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const isObject = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const named = (v: unknown): v is { name: string }[] => Array.isArray(v) && v.length > 0 && v.every(x => isObject(x) && typeof x.name === "string");

/** Dotted names of what differs, never values: lists are one key, except lists of named things, which go by name. */
export function diffPaths(a: unknown, b: unknown, at = ""): string[] {
  if (same(a, b)) return [];
  const join = (key: string) => at ? `${at}.${key}` : key;
  if (named(a) && named(b)) {
    const of = (list: { name: string }[], name: string) => list.find(x => x.name === name);
    return [...new Set([...a, ...b].map(x => x.name))].flatMap(name => diffPaths(of(a, name), of(b, name), join(name)));
  }
  if (isObject(a) && isObject(b)) return [...new Set([...Object.keys(a), ...Object.keys(b)])].flatMap(key => diffPaths(a[key], b[key], join(key)));
  return [at || "value"];
}

const mcpPolicy = (s: McpServer): McpPolicy => { const { command: _c, args: _a, env: _e, handTools: _h, unsupportedKeys: _u, problem: _p, ...rest } = s; return structuredClone(rest); };
export function capture(surfaces: MachineConfig["surfaces"], sessions: Record<string, number>): ProfileSections {
  return { routing: structuredClone(surfaces.routing), defaults: structuredClone(surfaces.user_defaults), mcp: surfaces.mcp_servers.map(mcpPolicy), sessions: { ...sessions } };
}
export const sessionsOf = (machine: Machine) => Object.fromEntries(machine.connections.map(c => [c.id, c.sessions]));
export const captureProfile = (machine: Machine) => capture(machine.config.surfaces, sessionsOf(machine));

/** What a new task on this machine starts with. A task keeps its own copy, so saving here never touches running work. */
export function machineDefaults(machine: Machine): { settings: TaskSettings; checkout: "worktree" | "main" } {
  const d = machine.config.surfaces.user_defaults;
  const choice = d.coordinator;
  const model = choice ? machine.config.catalogue[choice.engine].models.find(m => m.id === choice.model)?.routeId ?? choice.model : d.routeDefault?.routeId ?? "";
  const effort = choice?.effort ?? d.routeDefault?.parameters.thought_level ?? "";
  return { settings: { model, effort, mode: d.mode, language: d.language }, checkout: d.checkout };
}
/** The levels this model reports. Max stays for light work. An unknown model gets a list that says it was not observed. */
export function coordinatorEfforts(catalogue: MachineConfig["catalogue"], engine: EngineKey, model: string) {
  const found = catalogue[engine].models.find(m => m.id === model);
  return found ? { levels: found.efforts.filter(e => e !== "max"), observed: true } : { levels: ["low", "medium", "high"], observed: false };
}
export type Resolution = { ok: true; route: string } | { ok: false; code: string; text: string };
export function coordinatorResolution(machine: Machine): Resolution {
  const { surfaces: { routing, user_defaults: d }, catalogue } = machine.config;
  if (d.routeDefault) { const model = d.routeDefault.model, engine = ENGINE_KEYS.find(e => catalogue[e].models.some(m => m.id === model)); return { ok: true, route: engine ? `${engineLabel(engine)} · ${modelLabel(catalogue, engine, model)}, exact route` : `${model}, exact route` }; }
  if (!d.coordinator) return { ok: false, code: "coordinator_unset", text: "No coordinator is chosen." };
  const { engine, model } = d.coordinator;
  if (routing.deny.includes(`${engine}/*`)) return { ok: false, code: "coordinator_engine_off", text: `${engineLabel(engine)} is turned off in Models.` };
  if (routing.deny.includes(model) || routing.deny.includes(`${engine}/${model}`)) return { ok: false, code: "coordinator_route_denied", text: "A deny rule refuses this model." };
  if (!routing.engines[engine].enabled.includes(model)) return { ok: false, code: "coordinator_model_disabled", text: "This model is off in Models." };
  if (!catalogue[engine].models.some(m => m.id === model)) return { ok: false, code: "coordinator_model_unavailable", text: "The engine does not offer this model now." };
  return { ok: true, route: `${engineLabel(engine)} · ${modelLabel(catalogue, engine, model)}` };
}

/** First existing file wins, and files never merge: project, then the environment, then the machine. */
export function routingSource(machine: Machine, projectId?: string | null): { kind: "project" | "environment" | "machine"; path: string } {
  if (projectId && machine.config.projects[projectId]?.routing) return { kind: "project", path: "<project>/.cortex/routing.json" };
  if (machine.config.files.envRouting) return { kind: "environment", path: machine.config.files.envRouting };
  return { kind: "machine", path: "~/.config/cortex/routing.json" };
}
export const routingFor = (machine: Machine, projectId?: string | null): RoutingRules => (projectId ? machine.config.projects[projectId]?.routing : undefined) ?? machine.config.surfaces.routing;
export const projectsWithRouting = (machine: Machine) => Object.keys(machine.config.projects).filter(id => machine.config.projects[id].routing);
/** One set of rules decides both the routes and their quota windows, so a draft or a project's file is previewed whole. */
export function previewContext(machine: Machine, projectId?: string | null, rules?: RoutingRules): PreviewContext {
  const effective = rules ?? routingFor(machine, projectId);
  return {
    rules: effective, ceiling: effectiveCeiling(machine, projectId), catalogue: machine.config.catalogue, usage: (engine, model) => usageFor(machine, engine, model, effective),
    invalid: machine.config.files.invalidRouting, migration: machine.config.files.migration,
  };
}
/** A window listing this model, else one shared by the engine, else an unmapped report that counts for every model. Unknown stays null. */
export function usageFor(machine: Machine, engine: EngineKey, model: string, rules = machine.config.surfaces.routing): number | null {
  const seen = machine.config.quotaObserved[engine], windows = rules.engines[engine].quota, percent = (w: string) => seen.find(s => s.window === w)?.percent ?? null;
  const exact = windows.find(w => w.models?.includes(model)) ?? windows.find(w => w.models === null);
  if (exact) return percent(exact.window);
  return seen.find(s => !windows.some(w => w.window === s.window))?.percent ?? null;
}
export const unmappedWindows = (machine: Machine, engine: EngineKey, rules: RoutingRules) => machine.config.quotaObserved[engine].filter(s => !rules.engines[engine].quota.some(w => w.window === s.window));

const inside = (list: string[] | null, outer: string[] | null) => !list || !outer || list.every(x => outer.includes(x));
const within = (n: number | null, outer: number | null) => n === null || outer === null || n <= outer;
const whole = (n: number | null) => n === null || (Number.isInteger(n) && n >= 0);
const WIDER = "Must stay within the machine limit.";
export function ceilingProblems(c: Ceiling, machine: Ceiling | null): Problem[] {
  const out: Problem[] = [], put = (path: string, message: string) => out.push({ path, message });
  if (!whole(c.attempts)) put("attempts", "Use a whole number from 0, or leave it empty.");
  if (!whole(c.budget)) put("fallback.budget", "Use a whole number from 0, or leave it empty.");
  if (machine) {
    if (!within(c.attempts, machine.attempts)) put("attempts", WIDER);
    if (!within(c.budget, machine.budget)) put("fallback.budget", WIDER);
    if (!inside(c.allowedConnections, machine.allowedConnections)) put("allowed_connections", WIDER);
    if (!inside(c.allowedRecipients, machine.allowedRecipients)) put("allowed_data_recipients", WIDER);
    if (!inside(c.allowedRoutes, machine.allowedRoutes)) put("allowed_routes", WIDER);
    if (!inside(c.targetRoutes, machine.targetRoutes)) put("fallback.allowed_target_routes", WIDER);
    if (!inside(c.failureClasses, machine.failureClasses)) put("fallback.allowed_failure_classes", WIDER);
    if (c.consent && !machine.consent) put("fallback.consent", WIDER);
  }
  return out;
}
export type CeilingFieldState = "not_set" | "narrows" | "no_longer_narrows" | "violation";
const fieldState = (project: unknown, machine: unknown, ok: boolean): CeilingFieldState => project === null ? "not_set" : !ok ? "violation" : same(project, machine) ? "no_longer_narrows" : "narrows";
export function ceilingState(machine: Ceiling, project: Ceiling): Record<"attempts" | "budget" | "connections" | "recipients" | "routes" | "targets" | "classes", CeilingFieldState> {
  return {
    attempts: fieldState(project.attempts, machine.attempts, within(project.attempts, machine.attempts)), budget: fieldState(project.budget, machine.budget, within(project.budget, machine.budget)),
    connections: fieldState(project.allowedConnections, machine.allowedConnections, inside(project.allowedConnections, machine.allowedConnections)),
    recipients: fieldState(project.allowedRecipients, machine.allowedRecipients, inside(project.allowedRecipients, machine.allowedRecipients)),
    routes: fieldState(project.allowedRoutes, machine.allowedRoutes, inside(project.allowedRoutes, machine.allowedRoutes)),
    targets: fieldState(project.targetRoutes, machine.targetRoutes, inside(project.targetRoutes, machine.targetRoutes)),
    classes: fieldState(project.failureClasses, machine.failureClasses, inside(project.failureClasses, machine.failureClasses)),
  };
}
const meet = (a: string[] | null, b: string[] | null) => a && b ? a.filter(x => b.includes(x)) : a ?? b;
const least = (a: number | null, b: number | null) => a === null ? b : b === null ? a : Math.min(a, b);
export function effectiveCeiling(machine: Machine, projectId?: string | null): Ceiling {
  const m = machine.config.surfaces.user_defaults.ceiling, p = projectId ? machine.config.projects[projectId]?.ceiling : undefined;
  if (!p) return m;
  return {
    ...m, attempts: least(m.attempts, p.attempts), budget: least(m.budget, p.budget), consent: m.consent && p.consent, allowedRoutes: meet(m.allowedRoutes, p.allowedRoutes), allowedConnections: meet(m.allowedConnections, p.allowedConnections),
    allowedRecipients: meet(m.allowedRecipients, p.allowedRecipients), targetRoutes: meet(m.targetRoutes, p.targetRoutes), failureClasses: meet(m.failureClasses, p.failureClasses),
  };
}
export function defaultsProblems(d: UserDefaults): Problem[] {
  const out: Problem[] = [...ceilingProblems(d.ceiling, null)];
  if (d.coordinator && d.routeDefault) out.push({ path: "route_default", message: "Clear the exact route first" });
  if (d.routeDefault && !/^xr_[0-9a-f]{12,64}$/.test(d.routeDefault.routeId)) out.push({ path: "route_default.route_id", message: "This route is not valid. Pick a model from the list." });
  if (d.routeDefault && d.routeDefault.connectionId && !/^cn_[0-9a-f]{12}$/.test(d.routeDefault.connectionId)) out.push({ path: "route_default.connection_id", message: "A connection id is cn_ and 12 hex characters." });
  for (const [path, params] of [["coordinator.parameters", d.coordinator?.parameters], ["route_default.parameters", d.routeDefault?.parameters]] as const) for (const [name, value] of Object.entries(params ?? {})) {
    if (!/^[a-z][a-z0-9_]{0,63}$/.test(name)) out.push({ path: `${path}.${name}`, message: "Use lowercase letters, digits and _, starting with a letter." });
    if (value.length > 128) out.push({ path: `${path}.${name}`, message: "Keep the value within 128 characters." });
  }
  return out;
}

export type ProfileState = "none" | "in_sync" | "modified" | "interrupted" | "unavailable";
export const activeProfile = (c: MachineConfig) => c.profiles.find(p => p.id === c.active) ?? null;
const sessionsIn = (c: MachineConfig) => activeProfile(c)?.sections.sessions;
export function profileDifferences(c: MachineConfig, sessions?: Record<string, number>): { section: ProfileSection; keys: string[] }[] {
  const p = activeProfile(c);
  if (!p) return [];
  const now = capture(c.surfaces, sessions ?? sessionsIn(c) ?? {});
  return PROFILE_SECTIONS.flatMap(section => {
    if (p.sections[section] === undefined) return [];
    const keys = diffPaths(p.sections[section], now[section]);
    return keys.length ? [{ section, keys }] : [];
  });
}
export function profileState(c: MachineConfig, sessions?: Record<string, number>): ProfileState {
  if (c.unavailable) return "unavailable";
  if (c.interrupted) return "interrupted";
  if (!activeProfile(c)) return "none";
  return profileDifferences(c, sessions).length ? "modified" : "in_sync";
}
export function matchesProfile(c: MachineConfig, p: Profile, sessions: Record<string, number>) {
  const now = capture(c.surfaces, sessions);
  return PROFILE_SECTIONS.every(s => p.sections[s] === undefined || same(p.sections[s], now[s]));
}
export const missingSections = (p: Profile) => PROFILE_SECTIONS.filter(s => p.sections[s] === undefined);

export type ConfigIssue = { id: string; text: string; page: "defaults" | "models" | "tools" | "profiles" | "files"; server?: string };
/** Everything that stops or quietly weakens work on this machine, each with where to fix it. */
export function configIssues(machine: Machine): ConfigIssue[] {
  const c = machine.config, out: ConfigIssue[] = [], bound = coordinatorResolution(machine);
  if (c.files.invalidRouting) out.push({ id: "invalid", text: "The routing file is invalid. Every dispatch is refused until this is repaired.", page: "files" });
  if (c.files.migration || c.surfaces.routing.version !== 2) out.push({ id: "migration", text: "The model policy needs migration.", page: "files" });
  if (!bound.ok) out.push({ id: "coordinator", text: `Coordinator not bound: ${bound.text}`, page: "defaults" });
  if (ENGINE_KEYS.some(e => c.catalogue[e].failed)) out.push({ id: "catalogue", text: `A model list could not be read: ${ENGINE_KEYS.filter(e => c.catalogue[e].failed).map(engineLabel).join(", ")}. The previous list is kept.`, page: "models" });
  for (const e of ENGINE_KEYS) if (notOffered(c.surfaces.routing, c.catalogue, e).length) out.push({ id: `offered:${e}`, text: `${engineLabel(e)} no longer offers ${notOffered(c.surfaces.routing, c.catalogue, e).join(", ")}.`, page: "models" });
  for (const s of c.surfaces.mcp_servers) {
    if (s.problem) out.push({ id: `server:${s.name}`, text: `${s.name}: ${s.problem}`, page: "tools", server: s.name });
    if (s.enabled && [...s.readPaths, ...s.writePaths].some(pathMissing)) out.push({ id: `path:${s.name}`, text: `${s.name} will not start: a path does not exist.`, page: "tools", server: s.name });
  }
  if (c.interrupted) out.push({ id: "profile", text: "A profile switch was interrupted.", page: "profiles" });
  if (c.unavailable) out.push({ id: "profile-file", text: c.unavailable, page: "profiles" });
  return out;
}
