import type { Ceiling, EngineKey, Machine, McpPolicy, ModelPolicy, ProfileSection, RoutingRules, UserDefaults } from "./types";
import { ACCESS_MODES, effortLabel, engineLabel, ENGINE_KEYS, kindLabel, LANGUAGES, ROLE_CATALOG, TIER_LABELS } from "./catalog";
import { activeProfile, capture, sessionsOf } from "./config";
import { modelLabel } from "./routing-rules";

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const both = <T,>(a: Record<string, T>, b: Record<string, T>) => [...new Set([...Object.keys(a), ...Object.keys(b)])];
const added = <T,>(from: readonly T[], to: readonly T[]) => to.filter(x => !from.includes(x));

const POLICY: Record<keyof ModelPolicy, string> = { effortFloor: "effort floor", maxEffort: "max effort", effortLadder: "effort ladder", alsoStandardFor: "stretch work", preferredFor: "preferred work", preferredWhen: "prefer when", requestNames: "request names" };
const LIMITS: Record<keyof Ceiling, string> = { allowedRoutes: "allowed routes", allowedConnections: "allowed connections", allowedRecipients: "data recipients", attempts: "fallback attempts", failureClasses: "fallback failures", targetRoutes: "fallback routes", budget: "model run budget", consent: "fallback consent", stored: "stored limits" };
const roleName = (id: string) => id === "*" ? "all roles" : ROLE_CATALOG.find(r => r.id === id)?.label ?? id;

function routing(was: RoutingRules, now: RoutingRules, model: (engine: EngineKey, id: string) => string): string[] {
  const out: string[] = [];
  for (const e of ENGINE_KEYS) {
    const a = was.engines[e], b = now.engines[e], m = (id: string) => model(e, id);
    if (!a || !b) continue;
    out.push(...added(a.enabled, b.enabled).map(id => `${m(id)} turned on`), ...added(b.enabled, a.enabled).map(id => `${m(id)} turned off`));
    out.push(...added(a.auto, b.auto).map(id => `${m(id)} may be picked automatically`), ...added(b.auto, a.auto).map(id => `${m(id)} is no longer picked automatically`));
    for (const id of both(a.tiers, b.tiers)) if (!same(a.tiers[id] ?? [], b.tiers[id] ?? [])) out.push(`${m(id)} tiers: ${(b.tiers[id] ?? []).map(t => TIER_LABELS[t]).join(", ") || "none"}`);
    for (const id of both(a.policy, b.policy)) for (const key of Object.keys(POLICY) as (keyof ModelPolicy)[]) {
      const value = b.policy[id]?.[key] ?? null;
      if (same(a.policy[id]?.[key] ?? null, value)) continue;
      out.push(`${m(id)} ${POLICY[key]}: ${typeof value === "string" && key !== "preferredWhen" ? effortLabel(value) : value === null ? "cleared" : "changed"}`);
    }
    if (!same(a.quota, b.quota)) out.push(`${engineLabel(e)} quota windows changed`);
  }
  out.push(...added(was.providers, now.providers).map(e => `${engineLabel(e)} joins automatic routing`), ...added(now.providers, was.providers).map(e => `${engineLabel(e)} leaves automatic routing`));
  out.push(...added(was.deny, now.deny).map(x => `Deny rule added: ${x}`), ...added(now.deny, was.deny).map(x => `Deny rule removed: ${x}`));
  for (const role of both(was.pins, now.pins)) if (!same(was.pins[role], now.pins[role])) { const pin = now.pins[role]; out.push(pin ? `Pin for ${roleName(role)}: ${model(pin.engine, pin.model)}` : `Pin for ${roleName(role)} removed`); }
  for (const kind of both(was.affinity, now.affinity)) if (!same(was.affinity[kind], now.affinity[kind])) out.push(`Engine lean for ${kind === "*" ? "all kinds" : kindLabel(kind)} changed`);
  return out;
}

function defaults(was: UserDefaults, now: UserDefaults, model: (engine: EngineKey, id: string) => string): string[] {
  const out: string[] = [], a = was.coordinator, b = now.coordinator;
  if (a?.engine !== b?.engine || a?.model !== b?.model) out.push(b ? `Coordinator model: ${model(b.engine, b.model)}` : "Coordinator model cleared");
  if (a?.effort !== b?.effort && b) out.push(`Coordinator effort: ${effortLabel(b.effort)}`);
  if (!same(a?.parameters, b?.parameters)) out.push("Coordinator parameters changed");
  if (!same(was.routeDefault, now.routeDefault)) out.push(now.routeDefault ? "Exact default route changed" : "Exact default route cleared");
  if (was.mode !== now.mode) out.push(`Access mode: ${ACCESS_MODES.find(x => x.id === now.mode)?.label ?? now.mode}`);
  if (was.language !== now.language) out.push(`Language: ${LANGUAGES.find(x => x.id === now.language)?.label ?? now.language}`);
  if (was.checkout !== now.checkout) out.push(`Checkout for new tasks: ${now.checkout === "main" ? "Main checkout" : "Worktree"}`);
  for (const key of Object.keys(LIMITS) as (keyof Ceiling)[]) if (!same(was.ceiling[key], now.ceiling[key])) out.push(`Limits: ${LIMITS[key]} changed`);
  return out;
}

function servers(was: McpPolicy[], now: McpPolicy[]): string[] {
  const of = (list: McpPolicy[], name: string) => list.find(s => s.name === name);
  return [...new Set([...was, ...now].map(s => s.name))].flatMap(name => {
    const a = of(was, name), b = of(now, name);
    if (!a) return [`${name} added`];
    if (!b) return [`${name} removed`];
    if (same(a, b)) return [];
    return a.enabled !== b.enabled ? [`${name} turned ${b.enabled ? "on" : "off"}`] : [`${name}: who may use it changed`];
  });
}

function sessions(was: Record<string, number>, now: Record<string, number>, name: (id: string) => string): string[] {
  return both(was, now).flatMap(id => was[id] === now[id] ? [] : [now[id] === undefined ? `${name(id)}: session limit removed` : was[id] === undefined ? `${name(id)}: ${now[id]} sessions at most` : `${name(id)}: ${now[id]} sessions at most, was ${was[id]}`]);
}

/** What differs from the active profile, in the words the settings pages use. A setting with no sentence of its own still never shows as a key. */
export function profileChanges(machine: Machine): { section: ProfileSection; lines: string[] }[] {
  const c = machine.config, p = activeProfile(c);
  if (!p) return [];
  const now = capture(c.surfaces, sessionsOf(machine)), model = (e: EngineKey, id: string) => modelLabel(c.catalogue, e, id);
  const connection = (id: string) => machine.connections.find(x => x.id === id)?.label ?? "A removed connection";
  const say = (section: ProfileSection): string[] => {
    const was = p.sections[section];
    if (was === undefined || same(was, now[section])) return [];
    const lines = section === "routing" ? routing(was as RoutingRules, now.routing, model) : section === "defaults" ? defaults(was as UserDefaults, now.defaults, model)
      : section === "mcp" ? servers(was as McpPolicy[], now.mcp) : sessions(was as Record<string, number>, now.sessions, connection);
    return lines.length ? lines : ["Other settings changed"];
  };
  return (["routing", "defaults", "mcp", "sessions"] as const).flatMap(section => { const lines = say(section); return lines.length ? [{ section, lines }] : []; });
}
