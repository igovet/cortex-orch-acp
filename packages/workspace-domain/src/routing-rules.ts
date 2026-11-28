import type { Ceiling, CoordinatorModel, EffortRange, EngineCatalogue, EngineKey, EngineRules, ModelPolicy, Problem, RoutePin, RoutingRules, SprintRoute, Tier } from "./types";
import { EFFORT_BY_TIER, EFFORT_LEVELS, effortLabel, engineLabel, ENGINE_KEYS, ENGINES, FALLBACK_ORDER, HOT_PERCENT, NEUTRAL_POLICY, RESERVED_WORDS, ROLE_CATALOG, TIER_LABELS, TIERS, WORK_KINDS } from "./catalog";

type Catalogue = Record<EngineKey, EngineCatalogue>;
const copy = (rules: RoutingRules): RoutingRules => structuredClone(rules);
const without = <T,>(list: T[], item: T) => list.filter(x => x !== item);

const SHIFT = { low: 1, normal: 0, high: -1 } as const;
export type Complexity = keyof typeof SHIFT;
/** New execution roles start at the tier for their complexity; retained roles keep their original base. */
export function tierFor(role: string, escalate: boolean, complexity: Complexity = "normal"): Tier {
  const base = ROLE_CATALOG.find(r => r.id === role)?.base ?? "standard";
  let at = TIERS.indexOf(base) + SHIFT[complexity];
  at = escalate ? at - 1 : Math.max(TIERS.indexOf("deep"), at);
  return TIERS[Math.min(TIERS.length - 1, Math.max(0, at))];
}

export const engineInUse = (rules: RoutingRules, engine: EngineKey) => !rules.deny.includes(`${engine}/*`);
export function setEngineUse(rules: RoutingRules, engine: EngineKey, on: boolean) {
  const next = copy(rules), token = `${engine}/*`;
  next.deny = on ? without(next.deny, token) : next.deny.includes(token) ? next.deny : [...next.deny, token];
  return next;
}
/** Every token that refuses this model: an exact id, engine/model, or engine/*. */
export const denyTokens = (rules: RoutingRules, engine: EngineKey, model: string) => rules.deny.filter(t => t === model || t === `${engine}/${model}` || t === `${engine}/*`);
export const setDeny = (rules: RoutingRules, deny: string[]) => ({ ...copy(rules), deny });
/** Tokens that name nothing the machine knows about. They stay until the owner removes them. */
export function orphanTokens(rules: RoutingRules, catalogue: Catalogue) {
  return rules.deny.filter(t => {
    if (t.endsWith("/*")) return false;
    return !ENGINE_KEYS.some(e => catalogue[e].models.some(m => m.id === t || `${e}/${m.id}` === t));
  });
}
/** Automatic routing needs an engine, so the last one stays. */
export function setProvider(rules: RoutingRules, engine: EngineKey, on: boolean) {
  const next = copy(rules);
  if (on && !next.providers.includes(engine)) next.providers = ENGINE_KEYS.filter(e => e === engine || next.providers.includes(e));
  else if (!on && next.providers.length > 1) next.providers = without(next.providers, engine);
  return next;
}
const edit = (rules: RoutingRules, engine: EngineKey, fn: (e: EngineRules) => void) => { const next = copy(rules); fn(next.engines[engine]); return next; };
/** Off reads as out of automatic routing too. The stored `auto` entry stays, because a preferred model must stay listed there and comes back with it. */
export const setModelEnabled = (rules: RoutingRules, engine: EngineKey, model: string, on: boolean) => edit(rules, engine, e => {
  e.enabled = on ? (e.enabled.includes(model) ? e.enabled : [...e.enabled, model]) : without(e.enabled, model);
});
export const autoOn = (rules: RoutingRules, engine: EngineKey, model: string) => rules.engines[engine].enabled.includes(model) && rules.engines[engine].auto.includes(model);
export const setModelAuto = (rules: RoutingRules, engine: EngineKey, model: string, on: boolean) => edit(rules, engine, e => {
  e.auto = on ? (e.auto.includes(model) ? e.auto : [...e.auto, model]) : without(e.auto, model);
  if (on && !e.enabled.includes(model)) e.enabled = [...e.enabled, model];
});
export const setTier = (rules: RoutingRules, engine: EngineKey, model: string, tier: Tier, on: boolean) => edit(rules, engine, e => {
  const now = e.tiers[model] ?? [], next = on ? (now.includes(tier) ? now : TIERS.filter(t => t === tier || now.includes(t))) : without(now, tier);
  if (next.length) e.tiers[model] = next; else delete e.tiers[model];
  // A range belongs to a tier the model is placed in, so taking the tier away takes its range too.
  const ranges = e.policy[model]?.tierEfforts;
  if (!on && ranges?.[tier]) { const { [tier]: _gone, ...rest } = ranges; e.policy[model] = { ...e.policy[model], tierEfforts: Object.keys(rest).length ? rest : null }; }
});
export const policyOf = (rules: RoutingRules, engine: EngineKey, model: string): ModelPolicy => rules.engines[engine].policy[model] ?? NEUTRAL_POLICY;
/** True when any model lists the senior-consultant kind; otherwise the consultant uses the Design list. */
export const consultationListed = (rules: RoutingRules): boolean => ENGINE_KEYS.some(e => Object.values(rules.engines[e].policy).some(p => p.preferredFor?.includes("consultation")));
export const setPolicy = (rules: RoutingRules, engine: EngineKey, model: string, patch: Partial<ModelPolicy>) => edit(rules, engine, e => { e.policy[model] = { ...NEUTRAL_POLICY, ...e.policy[model], ...patch }; });
/** The effort range this model has on this tier, or null when the tier keeps the floor and max rules. */
export const tierRange = (rules: RoutingRules, engine: EngineKey, model: string, tier: Tier): EffortRange | null => policyOf(rules, engine, model).tierEfforts?.[tier] ?? null;
export function setTierRange(rules: RoutingRules, engine: EngineKey, model: string, tier: Tier, range: EffortRange | null) {
  const { [tier]: _was, ...rest } = policyOf(rules, engine, model).tierEfforts ?? {};
  const next = range ? { ...rest, [tier]: { min: range.min, max: range.max } } : rest;
  return setPolicy(rules, engine, model, { tierEfforts: Object.keys(next).length ? next : null });
}
const flip = (list: string[] | null, item: string, on: boolean) => { const next = on ? [...(list ?? []).filter(x => x !== item), item] : (list ?? []).filter(x => x !== item); return next.length ? next : null; };
export const setPreferred = (rules: RoutingRules, engine: EngineKey, model: string, kind: string, on: boolean) => setPolicy(rules, engine, model, { preferredFor: flip(policyOf(rules, engine, model).preferredFor, kind, on) });
export const setStretch = (rules: RoutingRules, engine: EngineKey, model: string, kind: string, on: boolean) => setPolicy(rules, engine, model, { alsoStandardFor: flip(policyOf(rules, engine, model).alsoStandardFor, kind, on) });
export function setPin(rules: RoutingRules, role: string, pin: RoutePin | null) {
  const next = copy(rules);
  if (pin) next.pins[role] = pin; else delete next.pins[role];
  return next;
}
export function setAffinity(rules: RoutingRules, kind: string, engine: EngineKey | null | "builtin") {
  const next = copy(rules);
  if (engine === "builtin") delete next.affinity[kind]; else next.affinity[kind] = engine;
  return next;
}
export const leanOf = (rules: RoutingRules, kind: string): EngineKey | null => kind in rules.affinity ? rules.affinity[kind] : WORK_KINDS.find(k => k.id === kind)?.lean ?? null;
/** An id the owner reads when nothing names it better. The engine alias `default` (no longer offered) names no concrete model. */
export const plainModel = (id: string) => id.toLowerCase() === "default" ? "Default (engine alias)" : id;
export const modelLabel = (catalogue: Catalogue, engine: EngineKey, id: string) => catalogue[engine].models.find(m => m.id === id)?.label ?? plainModel(id);
/** The route a sprint plan assigned, as a card shows it: model · tier · effort. */
export const sprintRouteLabel = (catalogue: Catalogue, route: SprintRoute) => `${catalogue[route.engine] ? modelLabel(catalogue, route.engine, route.model) : plainModel(route.model)} · ${TIER_LABELS[route.tier] ?? route.tier} · ${effortLabel(route.effort)}`;
/** Enabled ids the engine no longer lists. Nothing is inferred: the owner removes them or waits. */
export const notOffered = (rules: RoutingRules, catalogue: Catalogue, engine: EngineKey) => rules.engines[engine].enabled.filter(id => !catalogue[engine].models.some(m => m.id === id));

/** One word for what automatic routing would do with this model, before anything live is known. */
export function autoVerdict(rules: RoutingRules, catalogue: Catalogue, engine: EngineKey, model: string) {
  const e = rules.engines[engine];
  if (denyTokens(rules, engine, model).length) return "Refused by a deny rule";
  if (!e.enabled.includes(model)) return "Off";
  if (!catalogue[engine].models.some(m => m.id === model)) return "Not offered now";
  if (!rules.providers.includes(engine)) return "Engine is not in automatic routing";
  if (!e.auto.includes(model)) return "Manual selection only";
  return e.tiers[model]?.length ? "Eligible" : "Needs a tier";
}

const rank = (id: string) => (EFFORT_LEVELS as readonly string[]).indexOf(id);
/** The field-level rules of routing.json. The save refuses what this reports, so the UI and the command never disagree. */
export function routingProblems(rules: RoutingRules, catalogue: Catalogue): Problem[] {
  const out: Problem[] = [], add = (path: string, message: string) => out.push({ path, message });
  if (rules.providers.length === 0) add("providers", "Keep at least one engine in automatic routing.");
  const names = new Map<string, string>();
  for (const engine of ENGINE_KEYS) {
    const e = rules.engines[engine], where = `models.${engine}`;
    for (const model of e.auto) if (!(e.tiers[model]?.length)) add(`${where}.tiers.${model}`, "Choose a tier.");
    for (const [model, p] of Object.entries(e.policy)) {
      const at = `${where}.policy.${model}`, label = modelLabel(catalogue, engine, model);
      if (p.effortFloor && p.maxEffort && rank(p.effortFloor) > rank(p.maxEffort)) add(`${at}.max_effort`, "Floor must not be above max.");
      if (p.effortLadder && (p.effortLadder.length < 1 || p.effortLadder.length > 3 || p.effortLadder.some((x, i) => i > 0 && rank(x) <= rank(p.effortLadder![i - 1])))) add(`${at}.effort_ladder`, "Up to 3 levels, each higher.");
      if ((p.preferredFor?.length || p.preferredWhen) && !e.auto.includes(model)) add(`${at}.preferred_for`, "Turn on Auto first.");
      if (p.preferredWhen !== null && !p.preferredWhen.trim()) add(`${at}.preferred_when`, "Write a condition, or leave it empty.");
      if ((p.preferredWhen?.length ?? 0) > 500) add(`${at}.preferred_when`, "Keep it within 500 characters.");
      const seen = new Set<string>();
      for (const name of p.requestNames ?? []) {
        if (name !== name.toLowerCase() || name.length < 2 || name.length > 32) add(`${at}.request_names`, `"${name}" must be 2 to 32 lowercase characters.`);
        else if (RESERVED_WORDS.includes(name)) add(`${at}.request_names`, `"${name}" is reserved.`);
        else if (seen.has(name)) add(`${at}.request_names`, `"${name}" is listed twice.`);
        else if (names.has(name)) add(`${at}.request_names`, `"${name}" is already used by ${names.get(name)}.`);
        seen.add(name); if (!names.has(name)) names.set(name, label);
      }
      if ((p.requestNames?.length ?? 0) > 16) add(`${at}.request_names`, "Up to 16 names.");
      for (const [tier, range] of Object.entries(p.tierEfforts ?? {})) {
        const path = `${at}.tier_efforts.${tier}`;
        if (!(TIERS as readonly string[]).includes(tier)) add(path, `"${tier}" is not a tier.`);
        else if (!(e.tiers[model] ?? []).includes(tier as Tier)) add(path, "Place the model in this tier first, or remove its range.");
        else if (!range || rank(range.min) < 0 || rank(range.max) < 0) add(path, "Choose a lowest and a highest effort.");
        else if (rank(range.min) > rank(range.max)) add(path, "Lowest must not be above highest.");
      }
    }
    const windows = new Set<string>();
    e.quota.forEach((q, i) => {
      const at = `quota_windows.${engine}[${i}].window`;
      if (!q.window.trim()) add(at, "Enter the exact window label.");
      else if (windows.has(q.window)) add(at, "Already listed.");
      windows.add(q.window);
    });
  }
  for (const [role, pin] of Object.entries(rules.pins)) if (!rules.engines[pin.engine].enabled.includes(pin.model)) add(`pin.${role}`, "This model is off.");
  if (rules.coordinatorModels) {
    // The save refuses an empty or malformed list; a model that is off or not offered stays listed, marked where it is shown.
    if (!rules.coordinatorModels.length) add("coordinator_models", "Keep at least one model, so a task can start.");
    if (rules.coordinatorModels.length > 64) add("coordinator_models", "Up to 64 models.");
    const seen = new Set<string>();
    for (const c of rules.coordinatorModels) {
      const at = `coordinator_models.${c.engine}.${c.model}`;
      if (seen.has(at)) add(at, "Listed twice.");
      seen.add(at);
      if (!Object.keys(c.tiers).length) add(at, "Choose at least one category.");
      for (const [tier, range] of Object.entries(c.tiers)) {
        const path = `${at}.${tier}`;
        if (!(TIERS as readonly string[]).includes(tier)) add(path, `"${tier}" is not a category.`);
        else if (!range || rank(range.min) < 0 || rank(range.max) < 0) add(path, "Choose a lowest and a highest effort.");
        else if (rank(range.min) > rank(range.max)) add(path, "Lowest must not be above highest.");
      }
    }
  }
  return out;
}

export type Usage = Partial<Record<EngineKey, number | null>> | ((engine: EngineKey, model: string) => number | null);
export type PreviewContext = { rules: RoutingRules; ceiling: Ceiling; catalogue: Catalogue; usage?: Usage; invalid?: boolean; migration?: boolean };
export type PreviewInput = { role: string; kind: string; complexity: Complexity; attempt: "first" | "escalated" };
export type PreviewRow = { engine: EngineKey; model: string; label: string; tier: Tier; effort: string; usage: number | null; decidedBy: string };
export type PreviewExcluded = { engine: EngineKey; model: string; label: string; reason: string };
export type RoutePreview = { blocked: { code: string; text: string } | null; pinned: boolean; baseTier: Tier; tier: Tier; served: Tier | null; effort: string | null; rows: PreviewRow[]; excluded: PreviewExcluded[] };

/** A route id is derived from what it names, so nothing needs a table of them. */
export function routeIdOf(engine: EngineKey, model: string) {
  let a = 2166136261, b = 374761393;
  for (const c of `${engine}/${model}`) { a = Math.imul(a ^ c.charCodeAt(0), 16777619); b = Math.imul(b + c.charCodeAt(0), 2246822519); }
  const hex = (n: number) => (n >>> 0).toString(16).padStart(8, "0");
  return `xr_${hex(a)}${hex(b)}`;
}
export function allRoutes(rules: RoutingRules, catalogue: Catalogue) {
  return ENGINE_KEYS.flatMap(engine => {
    const ids = [...new Set([...catalogue[engine].models.map(m => m.id), ...rules.engines[engine].enabled])];
    return ids.filter(id => rules.engines[engine].enabled.includes(id)).map(model => ({ engine, model, id: routeIdOf(engine, model), label: `${engineLabel(engine)} · ${modelLabel(catalogue, engine, model)}` }));
  });
}
const usageOf = (ctx: PreviewContext, engine: EngineKey, model: string) => (typeof ctx.usage === "function" ? ctx.usage(engine, model) : ctx.usage?.[engine]) ?? null;
function refusal(ctx: PreviewContext, engine: EngineKey, model: string): string | null {
  const c = ctx.ceiling, r = ctx.rules;
  const tokens = denyTokens(r, engine, model);
  if (tokens.length) return `Denied by ${tokens[0]}`;
  if (c.allowedConnections && !c.allowedConnections.includes(engine)) return "Connection is outside the limits";
  if (c.allowedRecipients && !c.allowedRecipients.includes(ENGINES.find(e => e.id === engine)!.recipient)) return "Data recipient is outside the limits";
  if (c.allowedRoutes && !c.allowedRoutes.includes(routeIdOf(engine, model))) return "Route is outside the limits";
  return null;
}
/**
 * The offered efforts this model may run at on this tier: inside the tier's range when it has one (max too, when the range
 * reaches it), else above the floor and below max, where a flagship stops at high and max is for light work only.
 */
export function permitted(p: ModelPolicy, efforts: readonly string[], served: Tier) {
  const range = p.tierEfforts?.[served];
  if (range) return efforts.filter(e => rank(e) >= 0 && rank(e) >= rank(range.min) && rank(e) <= rank(range.max));
  const top = p.maxEffort ?? (served === "flagship" ? "high" : null);
  return efforts.filter(e => (e !== "max" || served === "light") && (!p.effortFloor || rank(e) >= rank(p.effortFloor)) && (!top || rank(e) <= rank(top)));
}
/** The level of `list` closest to `target`; a tie goes to the lower one. */
export function nearest(target: string, list: readonly string[]) {
  return list.length ? list.reduce((best, e) => Math.abs(rank(e) - rank(target)) < Math.abs(rank(best) - rank(target)) ? e : best, list[0]) : target;
}
/** What automatic routing starts this model at on this tier: the tier's usual effort, moved into what is permitted. */
export const tierEffort = (p: ModelPolicy, efforts: readonly string[], tier: Tier) => nearest(EFFORT_BY_TIER[tier], permitted(p, orderedLevels(efforts), tier));
const orderedLevels = (efforts: readonly string[]) => [...efforts].sort((a, b) => rank(a) - rank(b));

/** The coordinator list of these rules, or null for a profile written before it (the coordinator then follows the worker tiers). */
export const coordinatorModelsOf = (rules: RoutingRules): CoordinatorModel[] | null => rules.coordinatorModels ?? null;
export const coordinatorEntry = (rules: RoutingRules, engine: EngineKey, model: string) => rules.coordinatorModels?.find(c => c.engine === engine && c.model === model) ?? null;
/** The profile constrains the coordinator: with its coordinator list, or (a profile without one) once any engine places a model in a tier. */
export const profileConstrained = (rules: RoutingRules) => !!rules.coordinatorModels || ENGINE_KEYS.some(e => Object.values(rules.engines[e]?.tiers ?? {}).some(t => t.length > 0));
/** The offered levels inside one range; nothing when the model does not offer both of its ends, as the runtime refuses that range. */
export function rangeLevels(range: EffortRange | undefined, levels: readonly string[]) {
  if (!range || !levels.includes(range.min) || !levels.includes(range.max)) return [];
  return levels.filter(x => rank(x) >= rank(range.min) && rank(x) <= rank(range.max));
}
/**
 * The efforts the coordinator may use on this model, among the levels it offers: the union of its ranges in the coordinator list,
 * or (a profile without one) every level the model may run at on any of its worker tiers. Null when the profile names no
 * coordinator model at all, so nothing narrows the choice; empty when the model is outside it (off, refused or not listed).
 */
export function coordinatorAllowed(rules: RoutingRules, engine: EngineKey, model: string, offered: readonly string[]): string[] | null {
  if (!profileConstrained(rules)) return null;
  const e = rules.engines[engine];
  if (!e || !e.enabled.includes(model) || denyTokens(rules, engine, model).length) return [];
  const levels = orderedLevels(offered);
  if (rules.coordinatorModels) {
    const entry = coordinatorEntry(rules, engine, model);
    if (!entry) return [];
    return levels.filter(x => TIERS.some(t => rangeLevels(entry.tiers[t], levels).includes(x)));
  }
  const tiers = e.tiers[model] ?? [];
  if (!tiers.length) return [];
  const p = policyOf(rules, engine, model);
  return levels.filter(x => tiers.some(t => permitted(p, levels, t).includes(x)));
}
export type CoordinatorOption = { engine: EngineKey; model: string; label: string; efforts: string[] };
/** Every offered model the coordinator may be, with its allowed efforts. Null when the profile does not constrain it. */
export function coordinatorChoices(rules: RoutingRules, catalogue: Catalogue): CoordinatorOption[] | null {
  if (!profileConstrained(rules)) return null;
  return ENGINE_KEYS.flatMap(engine => (catalogue[engine]?.models ?? []).flatMap(m => {
    const efforts = coordinatorAllowed(rules, engine, m.id, m.efforts) ?? [];
    return efforts.length ? [{ engine, model: m.id, label: m.label, efforts }] : [];
  }));
}
/** What sprint planning assigns for a complexity: its categories in the order they are tried (high: Frontier, then Deep). */
export const SPRINT_TIERS: Record<Complexity, Tier[]> = { low: ["light", "standard"], normal: ["standard", "deep", "light"], high: ["flagship", "deep"] };
/**
 * The coordinator list this profile would start from: every enabled model with a worker tier, at those tiers, each with its tier
 * range or else the levels its floor and max allow there. A model offering no level gets no category.
 */
export function coordinatorSeed(rules: RoutingRules, catalogue: Catalogue): CoordinatorModel[] {
  return ENGINE_KEYS.flatMap(engine => rules.engines[engine].enabled.filter(id => !denyTokens(rules, engine, id).length && (rules.engines[engine].tiers[id]?.length ?? 0) > 0).map(model => {
    const levels = orderedLevels(catalogue[engine].models.find(m => m.id === model)?.efforts ?? []), p = policyOf(rules, engine, model);
    const tiers: Partial<Record<Tier, EffortRange>> = {};
    for (const t of TIERS.filter(t => rules.engines[engine].tiers[model]!.includes(t))) {
      const own = p.tierEfforts?.[t], allowed = own ? rangeLevels(own, levels) : permitted(p, levels, t).filter(x => rank(x) >= 0);
      if (allowed.length) tiers[t] = { min: allowed[0], max: allowed[allowed.length - 1] };
    }
    return { engine, model, tiers };
  }));
}
const coordinatorEdit = (rules: RoutingRules, fn: (list: CoordinatorModel[]) => CoordinatorModel[]) => { const next = copy(rules); next.coordinatorModels = fn(next.coordinatorModels ?? []); return next; };
/** Adds a model with one Standard category over the levels it offers nearest medium to high, or none when it offers no level. */
export function addCoordinatorModel(rules: RoutingRules, catalogue: Catalogue, engine: EngineKey, model: string) {
  if (coordinatorEntry(rules, engine, model)) return rules;
  const levels = orderedLevels(catalogue[engine].models.find(m => m.id === model)?.efforts ?? []).filter(x => rank(x) >= 0);
  const range = levels.length ? { min: nearest("medium", levels), max: nearest("high", levels) } : null;
  return coordinatorEdit(rules, list => [...list, { engine, model, tiers: range ? { standard: range } : {} }]);
}
export const removeCoordinatorModel = (rules: RoutingRules, engine: EngineKey, model: string) => coordinatorEdit(rules, list => list.filter(c => !(c.engine === engine && c.model === model)));
/** Ticks or clears one category of a listed model; a range replaces that category's range. */
export function setCoordinatorTier(rules: RoutingRules, engine: EngineKey, model: string, tier: Tier, range: EffortRange | null) {
  return coordinatorEdit(rules, list => list.map(c => {
    if (c.engine !== engine || c.model !== model) return c;
    const { [tier]: _was, ...rest } = c.tiers;
    const next: Partial<Record<Tier, EffortRange>> = range ? { ...rest, [tier]: { min: range.min, max: range.max } } : rest;
    return { ...c, tiers: Object.fromEntries(TIERS.filter(t => next[t]).map(t => [t, next[t]!])) };
  }));
}
/** Starts the coordinator list of a profile written before it, from what the worker tiers allow now. */
export const startCoordinatorList = (rules: RoutingRules, catalogue: Catalogue) => coordinatorEdit(rules, () => coordinatorSeed(rules, catalogue));
/**
 * What the router would do with this work, read from the rules alone. Live availability, retries and plan hints are only
 * known at dispatch, so the preview says what it can know and nothing else.
 */
export function previewRoute(input: PreviewInput, ctx: PreviewContext): RoutePreview {
  const { rules, catalogue } = ctx, escalate = input.attempt === "escalated";
  const baseTier = ROLE_CATALOG.find(r => r.id === input.role)?.base ?? "standard", tier = tierFor(input.role, escalate, input.complexity);
  const empty: RoutePreview = { blocked: null, pinned: false, baseTier, tier, served: null, effort: null, rows: [], excluded: [] };
  const stop = (code: string, text: string): RoutePreview => ({ ...empty, blocked: { code, text } });
  if (ctx.invalid) return stop("invalid_routing_overrides", "The routing file is invalid, so every dispatch is refused.");
  if (rules.version !== 2 || ctx.migration) return stop("model_policy_migration_required", "The model policy needs migration before anything can run.");
  const pin = rules.pins[input.role] ?? rules.pins["*"];
  if (pin) {
    const label = modelLabel(catalogue, pin.engine, pin.model), why = refusal(ctx, pin.engine, pin.model);
    if (why) return stop(denyTokens(rules, pin.engine, pin.model).length ? "route_denied" : "pinned_route_unavailable", `Pinned to ${label}. ${why}. A pin is strict, so the work is refused, not moved.`);
    if (!rules.engines[pin.engine].enabled.includes(pin.model)) return stop("pinned_route_unavailable", `Pinned to ${label}, which is off. A pin is strict, so the work is refused, not moved.`);
    const effort = pin.effort ?? EFFORT_BY_TIER[tier];
    return { ...empty, pinned: true, served: tier, effort, rows: [{ engine: pin.engine, model: pin.model, label: `${engineLabel(pin.engine)} · ${label}`, tier, effort, usage: usageOf(ctx, pin.engine, pin.model), decidedBy: "Pinned, strict" }] };
  }
  type Candidate = { engine: EngineKey; model: string; label: string; tiers: Tier[]; policy: ModelPolicy; efforts: string[]; usage: number | null };
  const usable: Candidate[] = [], excluded: PreviewExcluded[] = [];
  for (const engine of ENGINE_KEYS) {
    const e = rules.engines[engine], known = catalogue[engine].models;
    for (const model of [...new Set([...known.map(m => m.id), ...e.enabled])]) {
      const entry = known.find(m => m.id === model), label = `${engineLabel(engine)} · ${entry?.label ?? model}`, usage = usageOf(ctx, engine, model);
      const reason = refusal(ctx, engine, model)
        ?? (!e.enabled.includes(model) ? "Not enabled" : !entry ? "Not offered now" : !rules.providers.includes(engine) ? "Engine is not in automatic routing" : !e.auto.includes(model) ? "Not in automatic routing" : !(e.tiers[model]?.length) ? "No tier" : usage !== null && usage >= 100 ? "Plan limit reached" : null);
      if (reason) excluded.push({ engine, model, label, reason });
      else usable.push({ engine, model, label, tiers: e.tiers[model], policy: policyOf(rules, engine, model), efforts: entry!.efforts, usage });
    }
  }
  const lean = leanOf(rules, input.kind), bound = !!WORK_KINDS.find(k => k.id === input.kind)?.bound;
  const pool = bound && lean && usable.some(c => c.engine === lean) ? usable.filter(c => c.engine === lean) : usable;
  const stretch = (c: Candidate) => c.tiers.includes("light") && !!c.policy.alsoStandardFor?.includes(input.kind);
  const fits = (c: Candidate, t: Tier) => c.tiers.includes(t) || (t === "standard" && stretch(c));
  const served = FALLBACK_ORDER[tier].find(t => pool.some(c => fits(c, t))) ?? null;
  if (!served) return { ...stop("no_available_route", "No route is eligible for this work."), excluded };
  const inTier = pool.filter(c => fits(c, served));
  const preferred = (c: Candidate) => !!c.policy.preferredFor?.includes(input.kind) && (!c.policy.preferredWhen || bound);
  const hot = (c: Candidate) => c.usage !== null && c.usage >= HOT_PERCENT;
  const allKnown = inTier.every(c => c.usage !== null);
  const order = (c: Candidate) => [preferred(c) ? 0 : 1, hot(c) ? 1 : 0, c.engine === lean ? 0 : 1, allKnown ? c.usage! : 0, ENGINE_KEYS.indexOf(c.engine)];
  const ranked = inTier.map((c, i) => ({ c, key: [...order(c), i] })).sort((a, b) => { for (let i = 0; i < a.key.length; i++) if (a.key[i] !== b.key[i]) return a.key[i] - b.key[i]; return 0; }).map(x => x.c);
  const effortOf = (c: Candidate) => {
    const list = permitted(c.policy, orderedLevels(c.efforts), served);
    if (stretch(c) && c.policy.effortLadder && served === "standard") return nearest(c.policy.effortLadder[escalate ? Math.min(1, c.policy.effortLadder.length - 1) : 0], list);
    let target = EFFORT_BY_TIER[served];
    if (escalate && tierFor(input.role, false, input.complexity) === tier) target = (EFFORT_LEVELS as readonly string[])[Math.min(rank(target) + 1, rank("high"))];
    return nearest(target, list);
  };
  const decided = (c: Candidate) => hot(c) ? "Hot plan" : preferred(c) ? "Preferred" : lean && c.engine === lean ? `Lean to ${engineLabel(lean)}` : allKnown && inTier.length > 1 ? "Lower usage" : "Engine order";
  const rows = ranked.map((c): PreviewRow => ({ engine: c.engine, model: c.model, label: c.label, tier: served, effort: effortOf(c), usage: c.usage, decidedBy: decided(c) }));
  for (const c of usable) {
    if (rows.some(r => r.engine === c.engine && r.model === c.model)) continue;
    const kinds = c.policy.alsoStandardFor;
    excluded.push({ engine: c.engine, model: c.model, label: c.label, reason: !pool.includes(c) ? "This work stays on its leaned engine" : served === "standard" && kinds?.length ? `${kinds.map(k => WORK_KINDS.find(x => x.id === k)?.label ?? k).join(", ")} only for standard work` : "Not in this tier" });
  }
  return { ...empty, served, effort: rows[0].effort, rows, excluded };
}
export type GridRow = { role: string; label: string; cells: { complexity: Complexity; text: string; blocked: boolean }[] };
/** Every role at every complexity, each with its own work kind. */
export function previewGrid(attempt: "first" | "escalated", ctx: PreviewContext): GridRow[] {
  return ROLE_CATALOG.map(r => ({
    role: r.id, label: r.label,
    cells: (["low", "normal", "high"] as const).map(complexity => {
      const p = previewRoute({ role: r.id, kind: r.kind, complexity, attempt }, ctx);
      return { complexity, text: p.blocked ? p.blocked.code : `${p.rows[0].label}, ${p.rows[0].effort}`, blocked: !!p.blocked };
    }),
  }));
}
