import type { z } from "zod";
import type {
  ConfigError,
  ConfigUpdateInput,
  McpOperation,
  connectionsResult,
  mcpServersResult,
  projectCeilingResult,
  routingResult,
  userDefaultsResult,
} from "../shared/config.ts";

/**
 * Pure, dependency-free form model for the Cortex Settings screen
 * (`settings-panel.tsx`). Every editable setting is a typed form field here
 * (text, number, enum, toggle, list or row list): the screen never asks the
 * owner to type JSON. Each surface has a `...ToForm` (API view -> form) and
 * a `build...` (form -> exact `cortex.config.update` input, or field-level
 * errors keyed by the API's own field paths) so the round trip is testable
 * without rendering React Native.
 *
 * A stored value this model cannot represent without loss (an MCP server
 * with keys the API does not manage, a routing key with an unexpected shape)
 * is reported as read-only rather than silently rewritten.
 */

export const SETTINGS_SECTIONS = [
  { id: "user_defaults", title: "User defaults" },
  { id: "project_ceiling", title: "Project ceiling" },
  { id: "routing", title: "Routing" },
  { id: "mcp_servers", title: "MCP servers" },
  { id: "connections", title: "Connections" },
] as const;
export type SettingsSectionId = typeof SETTINGS_SECTIONS[number]["id"];

// --- API shapes ---------------------------------------------------------------

export type UserDefaultsView = z.infer<typeof userDefaultsResult>;
export type ProjectCeilingView = z.infer<typeof projectCeilingResult>;
export type RoutingView = z.infer<typeof routingResult>;
export type RoutingSourceView = RoutingView["sources"][number];
export type McpServersView = z.infer<typeof mcpServersResult>;
export type McpServerView = McpServersView["servers"][number];
export type ConnectionsView = z.infer<typeof connectionsResult>;
export type ConnectionView = ConnectionsView["connections"][number];
export type CeilingView = UserDefaultsView["ceiling"];

type UpdateFor<S extends ConfigUpdateInput["surface"]> = Extract<ConfigUpdateInput, { surface: S }>;
export type UserDefaultsInput = UpdateFor<"user_defaults">;
export type ProjectCeilingInput = UpdateFor<"project_ceiling">;
export type RoutingInput = UpdateFor<"routing">;
export type McpServersInput = UpdateFor<"mcp_servers">;
export type ConnectionsInput = UpdateFor<"connections">;
export type CeilingInput = NonNullable<ProjectCeilingInput["ceiling"]>;
export type RoutingRules = RoutingInput["rules"];
export type McpServerInput = Extract<McpOperation, { op: "upsert" }>["server"];
export type OnboardInput = { expected_revision: string; explicit_opt_in: true; name?: string };

// --- errors -------------------------------------------------------------------

/** Field path (the API's own, e.g. `ceiling.limits.tokens`) -> message. */
export type FieldErrors = Record<string, string>;
export type Built<T> = { ok: true; value: T } | { ok: false; errors: FieldErrors };

/** True when a write failed only because `expected_revision` is stale: the
 * caller should offer a reload, not treat this as a validation mistake. */
export function isRevisionConflict(error: Pick<ConfigError, "code">): boolean {
  return error.code === "revision_conflict";
}

/** One readable line for a `ConfigError`, folding in every field-level
 * problem so a refused write never hides which setting was rejected. */
export function describeConfigError(error: ConfigError): string {
  const parts = [error.message];
  if (error.field) parts.push(`(field: ${error.field})`);
  if (error.errors && error.errors.length > 0) {
    parts.push(error.errors.map((item) => `${item.field}: ${item.message}`).join("; "));
  }
  return parts.join(" — ");
}

function addError(errors: FieldErrors, key: string, message: string): void {
  errors[key] = errors[key] ? `${errors[key]}; ${message}` : message;
}

/** The refused write's problems keyed by field, so each control can show
 * its own. `stripPrefixes` maps an operation-scoped path
 * (`operations[0].server.env.TOKEN`) onto the form's own (`env.TOKEN`). */
export function fieldErrorsFromConfigError(error: ConfigError, stripPrefixes: readonly string[] = []): FieldErrors {
  const errors: FieldErrors = {};
  const strip = (field: string) => {
    for (const prefix of stripPrefixes) if (field.startsWith(prefix)) return field.slice(prefix.length);
    return field;
  };
  if (error.errors && error.errors.length > 0) {
    for (const item of error.errors) if (item.field) addError(errors, strip(item.field), item.message);
  } else if (error.field) {
    addError(errors, strip(error.field), error.message);
  }
  return errors;
}

/** The message for the control at `key`: its own problem plus any nested
 * one (`key.x`, `key[0]`). */
export function errorFor(errors: FieldErrors | null | undefined, key: string): string | undefined {
  if (!errors) return undefined;
  const found: string[] = [];
  for (const [field, message] of Object.entries(errors)) {
    if (field === key || field.startsWith(`${key}.`) || field.startsWith(`${key}[`)) found.push(message);
  }
  return found.length > 0 ? found.join("; ") : undefined;
}

/** The message for a list or map control as a whole: its own problem and
 * problems the API reports by name under it (`rules.pin.worker`), but not
 * per-entry ones (`key[0]`), which each entry shows itself. */
export function errorAt(errors: FieldErrors | null | undefined, key: string): string | undefined {
  if (!errors) return undefined;
  const found: string[] = [];
  for (const [field, message] of Object.entries(errors)) {
    if (field === key || field.startsWith(`${key}.`)) found.push(message);
  }
  return found.length > 0 ? found.join("; ") : undefined;
}

export function mergeErrors(...sets: Array<FieldErrors | null | undefined>): FieldErrors {
  const merged: FieldErrors = {};
  for (const set of sets) if (set) for (const [key, message] of Object.entries(set)) addError(merged, key, message);
  return merged;
}

// --- shared validation --------------------------------------------------------

const CONTROL = /[\x00-\x1f\x7f]/;
const SPACE_OR_CONTROL = /[\s\x00-\x1f\x7f]/;
export const ROUTE_ID = /^xr_[0-9a-f]{12,64}$/;
export const CONNECTION_ID = /^cn_[0-9a-f]{12}$/;
export const PARAMETER_NAME = /^[a-z][a-z0-9_]{0,63}$/;
export const ROLE_KEY = /^(?:\*|[A-Za-z0-9][A-Za-z0-9._-]{0,127})$/;
export const SERVER_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
export const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

export type NumberResult = { ok: true; value: number | null } | { ok: false; error: string };

/** A whole-number text field: blank is `null` (unset), anything else must be
 * digits within the bounds. */
export function parseWholeNumber(text: string, bounds: { min?: number; max?: number } = {}): NumberResult {
  const trimmed = text.trim();
  if (!trimmed) return { ok: true, value: null };
  if (!/^\d{1,15}$/.test(trimmed)) return { ok: false, error: "Enter a whole number (digits only)." };
  const value = Number(trimmed);
  const min = bounds.min ?? 0;
  if (value < min) return { ok: false, error: `Must be at least ${min}.` };
  if (bounds.max !== undefined && value > bounds.max) return { ok: false, error: `Must be at most ${bounds.max}.` };
  return { ok: true, value };
}

function wholeNumber(text: string, key: string, errors: FieldErrors, bounds: { min?: number; max?: number } = {}): number | null {
  const parsed = parseWholeNumber(text, bounds);
  if (!parsed.ok) { addError(errors, key, parsed.error); return null; }
  return parsed.value;
}

type ListRule = {
  trim?: boolean;
  noSpaces?: boolean;
  maxLength?: number;
  maxItems?: number;
  unique?: boolean;
  oneOf?: readonly string[];
  check?: (value: string) => string | null;
};

/** Validates a list control's entries (`key[i]` per entry); `null` (the
 * setting is unset) passes through unchanged. */
function checkList(values: readonly string[] | null, key: string, errors: FieldErrors, rule: ListRule = {}): string[] | null {
  if (values === null) return null;
  const out: string[] = [];
  const seen = new Set<string>();
  values.forEach((raw, index) => {
    const value = rule.trim === false ? raw : raw.trim();
    const itemKey = `${key}[${index}]`;
    if (rule.trim !== false && !value) { addError(errors, itemKey, "Enter a value or remove this entry."); return; }
    if (rule.noSpaces && SPACE_OR_CONTROL.test(value)) addError(errors, itemKey, "Must not contain spaces.");
    else if (value.includes("\u0000")) addError(errors, itemKey, "Must not contain a NUL character.");
    if (value.length > (rule.maxLength ?? 256)) addError(errors, itemKey, `At most ${rule.maxLength ?? 256} characters.`);
    if (rule.oneOf && !rule.oneOf.includes(value)) addError(errors, itemKey, `Must be one of ${rule.oneOf.join(", ")}.`);
    const problem = rule.check?.(value);
    if (problem) addError(errors, itemKey, problem);
    if (rule.unique !== false && seen.has(value)) addError(errors, itemKey, "Duplicate entry.");
    seen.add(value);
    out.push(value);
  });
  if (rule.maxItems !== undefined && values.length > rule.maxItems) addError(errors, key, `At most ${rule.maxItems} entries.`);
  return out;
}

function cleanText(value: string, key: string, errors: FieldErrors, label: string, limit = 256): string {
  const trimmed = value.trim();
  if (!trimmed) addError(errors, key, `Enter ${label}.`);
  else if (CONTROL.test(trimmed)) addError(errors, key, "Must not contain control characters.");
  else if (trimmed.length > limit) addError(errors, key, `At most ${limit} characters.`);
  return trimmed;
}

function sortedJson(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) => {
    if (item && typeof item === "object" && !Array.isArray(item)) {
      return Object.fromEntries(Object.entries(item as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
    }
    return item;
  });
}

/** Structural equality of two JSON values, ignoring object key order. */
export function sameJson(a: unknown, b: unknown): boolean {
  return sortedJson(a) === sortedJson(b);
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function isStringRecord(value: unknown, allowNull = false): value is Record<string, string | null> {
  return isPlainObject(value) && Object.values(value).every((item) => typeof item === "string" || (allowNull && item === null));
}

/** A key/value row a map editor (defaults `parameters`) renders. */
export type KeyValueRow = { key: string; value: string };

// --- ceilings -----------------------------------------------------------------

/** `PermissionLevel` members, broadest last (`configuration/policy.py`). */
export const PERMISSION_LEVELS = ["READ_ONLY", "STANDARD", "ELEVATED"] as const;
/** The only limit names a ceiling accepts (`management.LIMIT_KEYS`). */
export const LIMIT_KEYS = ["attempts", "calls", "tokens", "concurrency"] as const;
/** Never eligible for fallback, whatever a policy says (`FALLBACK_HARD_DENYLIST`). */
export const FALLBACK_DENYLIST = ["authorization_loss", "cancellation", "provider_restriction", "refusal"] as const;
/** The failure classes fallback authorization actually acts on. */
export const FALLBACK_ELIGIBLE_CLASSES = ["engine_failure", "capacity_unavailable"] as const;

/** A ceiling as form fields. A list is `null` when that dimension is
 * unrestricted; numbers are text (blank = unset) so partial input can be
 * shown with its own error. */
export type CeilingForm = {
  allowed_routes: string[] | null;
  allowed_connections: string[] | null;
  allowed_data_recipients: string[] | null;
  permission_level: string | null;
  limits: Record<string, string>;
  spend_limit: string;
  spend_limit_enforceable: boolean;
  fallback_allowed_target_routes: string[] | null;
  fallback_allowed_failure_classes: string[] | null;
  fallback_budget: string;
  fallback_data_recipient_consent: boolean;
};

const copyList = (values: readonly string[] | null): string[] | null => (values ? [...values] : null);
const numberText = (value: number | null): string => (value === null ? "" : String(value));

export function ceilingToForm(view: CeilingView): CeilingForm {
  const limits: Record<string, string> = {};
  for (const key of LIMIT_KEYS) limits[key] = "";
  for (const [key, value] of Object.entries(view.limits)) limits[key] = String(value);
  return {
    allowed_routes: copyList(view.allowed_routes),
    allowed_connections: copyList(view.allowed_connections),
    allowed_data_recipients: copyList(view.allowed_data_recipients),
    permission_level: view.permission_level,
    limits,
    spend_limit: numberText(view.spend_limit),
    spend_limit_enforceable: view.spend_limit_enforceable,
    fallback_allowed_target_routes: copyList(view.fallback_policy.allowed_target_routes),
    fallback_allowed_failure_classes: copyList(view.fallback_policy.allowed_failure_classes),
    fallback_budget: numberText(view.fallback_policy.budget),
    fallback_data_recipient_consent: view.fallback_policy.data_recipient_consent,
  };
}

/** The ceiling request object, or every problem keyed `<prefix>.<field>`. */
export function buildCeiling(form: CeilingForm, prefix = "ceiling"): Built<CeilingInput> {
  const errors: FieldErrors = {};
  const key = (name: string) => `${prefix}.${name}`;
  const set = (values: string[] | null, name: string, rule: ListRule = {}) => checkList(values, key(name), errors, { maxItems: 1000, ...rule });
  const allowedRoutes = set(form.allowed_routes, "allowed_routes");
  const allowedConnections = set(form.allowed_connections, "allowed_connections");
  const recipients = set(form.allowed_data_recipients, "allowed_data_recipients");
  if (form.permission_level !== null && !(PERMISSION_LEVELS as readonly string[]).includes(form.permission_level)) {
    addError(errors, key("permission_level"), `Must be unrestricted or one of ${PERMISSION_LEVELS.join(", ")}.`);
  }
  const limits: Record<string, number> = {};
  for (const [name, text] of Object.entries(form.limits)) {
    const value = wholeNumber(text, key(`limits.${name}`), errors);
    if (value !== null) limits[name] = value;
  }
  const spendLimit = wholeNumber(form.spend_limit, key("spend_limit"), errors);
  if (spendLimit !== null && !form.spend_limit_enforceable) {
    addError(errors, key("spend_limit"), "A spend limit needs \"Spend limit enforceable\" switched on (unknown cost is never treated as zero).");
  }
  const targets = set(form.fallback_allowed_target_routes, "fallback_policy.allowed_target_routes");
  const classes = set(form.fallback_allowed_failure_classes, "fallback_policy.allowed_failure_classes", {
    check: (value) => ((FALLBACK_DENYLIST as readonly string[]).includes(value) ? `${value} can never be a fallback class.` : null),
  });
  const budget = wholeNumber(form.fallback_budget, key("fallback_policy.budget"), errors);
  if (Object.keys(errors).length > 0) return { ok: false, errors };
  return {
    ok: true,
    value: {
      allowed_routes: allowedRoutes,
      allowed_connections: allowedConnections,
      allowed_data_recipients: recipients,
      permission_level: form.permission_level,
      limits,
      spend_limit: spendLimit,
      spend_limit_enforceable: form.spend_limit_enforceable,
      fallback_policy: {
        allowed_target_routes: targets,
        allowed_failure_classes: classes,
        budget,
        data_recipient_consent: form.fallback_data_recipient_consent,
      },
    },
  };
}

// --- user defaults ------------------------------------------------------------

export type UserDefaultsForm = {
  route_id: string;
  route_connection_id: string;
  route_model: string;
  /** `""` = automatic (the key is removed). */
  language: string;
  parameters: KeyValueRow[];
  ceiling_set: boolean;
  ceiling: CeilingForm;
};

const stringValue = (value: unknown): string => (typeof value === "string" ? value : "");

export function userDefaultsToForm(view: UserDefaultsView): UserDefaultsForm {
  const parameters = isPlainObject(view.defaults.parameters)
    ? Object.entries(view.defaults.parameters).map(([key, value]) => ({ key, value: typeof value === "string" ? value : String(value) }))
    : [];
  return {
    route_id: stringValue(view.defaults.route_id),
    route_connection_id: stringValue(view.defaults.route_connection_id),
    route_model: stringValue(view.defaults.route_model),
    language: stringValue(view.defaults.language),
    parameters,
    ceiling_set: view.ceiling_set,
    ceiling: ceilingToForm(view.ceiling),
  };
}

/** The keys of `defaults` this screen never edits (the API preserves them). */
export function preservedDefaultKeys(view: UserDefaultsView): string[] {
  const editable = new Set(view.editable_keys);
  return [...new Set([...view.preserved_keys, ...Object.keys(view.defaults).filter((key) => !editable.has(key))])].sort();
}

/**
 * The `user_defaults` update for what changed since `view` was read, or
 * `null` when nothing did. Only changed settings are sent (the API patches),
 * so a stored value this form shows approximately is never rewritten unless
 * the owner edits it.
 */
export function buildUserDefaultsUpdate(view: UserDefaultsView, form: UserDefaultsForm): Built<UserDefaultsInput | null> {
  const initial = userDefaultsToForm(view);
  const errors: FieldErrors = {};
  const defaults: NonNullable<UserDefaultsInput["defaults"]> = {};

  const route = { route_id: form.route_id.trim(), route_connection_id: form.route_connection_id.trim(), route_model: form.route_model.trim() };
  const routeChanged = route.route_id !== initial.route_id.trim() || route.route_connection_id !== initial.route_connection_id.trim()
    || route.route_model !== initial.route_model.trim();
  if (routeChanged) {
    const filled = Object.values(route).filter(Boolean).length;
    if (filled === 0) {
      defaults.route_id = null;
      defaults.route_connection_id = null;
      defaults.route_model = null;
    } else if (filled < 3) {
      for (const [name, value] of Object.entries(route)) {
        if (!value) addError(errors, `defaults.${name}`, "Set the route id, connection and model together, or clear all three.");
      }
    } else {
      if (!ROUTE_ID.test(route.route_id)) addError(errors, "defaults.route_id", "Must be an observed route id (xr_ followed by 12-64 hex digits).");
      if (!CONNECTION_ID.test(route.route_connection_id)) addError(errors, "defaults.route_connection_id", "Must be a connection id (cn_ followed by 12 hex digits).");
      cleanText(route.route_model, "defaults.route_model", errors, "the route's exact model id");
      Object.assign(defaults, route);
    }
  }

  if (!sameJson(form.parameters, initial.parameters)) {
    const parameters: Record<string, string> = {};
    form.parameters.forEach((row, index) => {
      const name = row.key.trim();
      const rowKey = `defaults.parameters[${index}]`;
      if (!PARAMETER_NAME.test(name)) addError(errors, rowKey, "Names are lowercase identifiers (a-z, 0-9, _), starting with a letter.");
      else if (name in parameters) addError(errors, rowKey, "Duplicate parameter name.");
      if (!row.value.trim()) addError(errors, rowKey, "Enter a value.");
      else if (CONTROL.test(row.value) || row.value.length > 128) addError(errors, rowKey, "Values are at most 128 printable characters.");
      parameters[name] = row.value;
    });
    defaults.parameters = form.parameters.length === 0 ? null : parameters;
  }

  if (form.language !== initial.language) {
    const languages = view.languages.length > 0 ? view.languages : ["en", "ru"];
    if (form.language && !languages.includes(form.language)) addError(errors, "defaults.language", `Must be automatic or one of ${languages.join(", ")}.`);
    defaults.language = (form.language || null) as "en" | "ru" | null;
  }

  let ceiling: CeilingInput | null | undefined;
  if (!form.ceiling_set) {
    if (view.ceiling_set) ceiling = null;
  } else {
    const built = buildCeiling(form.ceiling);
    if (!built.ok) Object.assign(errors, built.errors);
    else {
      const stored = buildCeiling(ceilingToForm(view.ceiling));
      if (!view.ceiling_set || !stored.ok || !sameJson(built.value, stored.value)) ceiling = built.value;
    }
  }

  if (Object.keys(errors).length > 0) return { ok: false, errors };
  const hasDefaults = Object.keys(defaults).length > 0;
  if (!hasDefaults && ceiling === undefined) return { ok: true, value: null };
  return {
    ok: true,
    value: {
      surface: "user_defaults",
      expected_revision: view.revision,
      ...(hasDefaults ? { defaults } : {}),
      ...(ceiling !== undefined ? { ceiling } : {}),
    },
  };
}

// --- project ceiling ----------------------------------------------------------

/** The stored project ceiling, or (when none is set yet) the owner ceiling
 * as a starting point that already narrows it. */
export function projectCeilingToForm(view: ProjectCeilingView): CeilingForm {
  return ceilingToForm(view.ceiling ?? view.user_ceiling);
}

export function buildProjectCeilingUpdate(view: ProjectCeilingView, workspaceId: string, form: CeilingForm): Built<ProjectCeilingInput> {
  const built = buildCeiling(form);
  if (!built.ok) return built;
  return { ok: true, value: { surface: "project_ceiling", workspaceId, expected_revision: view.revision, ceiling: built.value } };
}

export function projectCeilingRemoval(view: ProjectCeilingView, workspaceId: string): ProjectCeilingInput {
  return { surface: "project_ceiling", workspaceId, expected_revision: view.revision, ceiling: null };
}

// --- routing ------------------------------------------------------------------

export const ROUTING_KEYS = ["pin", "deny", "auto_allow", "tiers", "providers", "engine_affinity"] as const;
export type RoutingKey = typeof ROUTING_KEYS[number];

/** One `pin` entry: a role (or `*`) pinned to `engine/model[:effort]`. The
 * model is the full id after the engine (for OpenCode `provider/model`). */
export type PinRow = { role: string; engine: string; model: string; effort: string };
export type TierRow = { model: string; tier: string };
/** `engine: ""` means `null` (no affinity for that work kind). */
export type AffinityRow = { kind: string; engine: string };

/** Each key is `null` while it is absent from the file (the built-in policy
 * applies); a present key keeps its exact order. */
export type RoutingForm = {
  pin: PinRow[] | null;
  deny: string[] | null;
  auto_allow: string[] | null;
  tiers: TierRow[] | null;
  providers: string[] | null;
  engine_affinity: AffinityRow[] | null;
};

export function emptyRoutingForm(): RoutingForm {
  return { pin: null, deny: null, auto_allow: null, tiers: null, providers: null, engine_affinity: null };
}

export function parsePin(role: string, value: string): PinRow {
  const colon = value.indexOf(":");
  const route = colon === -1 ? value : value.slice(0, colon);
  const effort = colon === -1 ? "" : value.slice(colon + 1);
  const slash = route.indexOf("/");
  return slash === -1 ? { role, engine: "", model: route, effort } : { role, engine: route.slice(0, slash), model: route.slice(slash + 1), effort };
}

export function composePin(row: Pick<PinRow, "engine" | "model" | "effort">): string {
  const route = row.engine ? `${row.engine}/${row.model}` : row.model;
  return row.effort ? `${route}:${row.effort}` : route;
}

export type RoutingVocabulary = { engines: string[]; optionalEngines: string[]; efforts: string[]; tiers: string[]; workKinds: string[] };

export function routingVocabulary(view: RoutingView): RoutingVocabulary {
  return {
    engines: [...new Set([...view.builtin.supported_engines, ...view.builtin.optional_engines])],
    optionalEngines: [...view.builtin.optional_engines],
    efforts: [...view.builtin.efforts],
    tiers: [...view.builtin.tiers],
    workKinds: [...view.builtin.work_kinds, "*"],
  };
}

export type RoutingFormResult = { ok: true; form: RoutingForm } | { ok: false; reason: string };

/** The six supported keys as form rows, or why the stored rules cannot be
 * shown as typed fields without losing something. */
export function routingRulesToForm(rules: Record<string, unknown> | undefined): RoutingFormResult {
  const form = emptyRoutingForm();
  if (!rules) return { ok: true, form };
  const unrepresentable = (key: string): RoutingFormResult => ({
    ok: false,
    reason: `The stored "${key}" setting has a shape these fields cannot show without changing it; edit the file by hand, then reload.`,
  });
  if (rules.pin !== undefined) {
    if (!isStringRecord(rules.pin)) return unrepresentable("pin");
    form.pin = Object.entries(rules.pin).map(([role, value]) => parsePin(role, value as string));
  }
  for (const key of ["deny", "auto_allow", "providers"] as const) {
    if (rules[key] === undefined) continue;
    if (!isStringArray(rules[key])) return unrepresentable(key);
    form[key] = [...(rules[key] as string[])];
  }
  if (rules.tiers !== undefined) {
    if (!isStringRecord(rules.tiers)) return unrepresentable("tiers");
    form.tiers = Object.entries(rules.tiers).map(([model, tier]) => ({ model, tier: tier as string }));
  }
  if (rules.engine_affinity !== undefined) {
    if (!isStringRecord(rules.engine_affinity, true)) return unrepresentable("engine_affinity");
    form.engine_affinity = Object.entries(rules.engine_affinity).map(([kind, engine]) => ({ kind, engine: engine ?? "" }));
  }
  return { ok: true, form };
}

export type RoutingTargetState = {
  target: "user" | "project";
  source: RoutingSourceView | undefined;
  /** The revision a write to this file is checked against. */
  revision: string;
  editable: boolean;
  /** Why the file is read-only here, when it is. */
  reason: string | null;
  form: RoutingForm | null;
  /** Keys in the file this screen does not manage; a write keeps them. */
  ignoredKeys: string[];
};

export function routingTarget(view: RoutingView, target: "user" | "project"): RoutingTargetState {
  const source = view.sources.find((entry) => entry.source === target);
  const base = { target, source, revision: source?.revision ?? "absent", ignoredKeys: [...(source?.ignored_keys ?? [])] };
  if (!source) {
    const reason = target === "project" ? "Select a project workspace to edit its routing file." : "The user routing file is not available.";
    return { ...base, editable: false, reason, form: null };
  }
  if (!source.writable) return { ...base, editable: false, reason: "Cortex never writes this routing source; edit it where it is kept.", form: null };
  if (source.exists && (source.rules === undefined || source.revision === null)) {
    const detail = source.error ? `: ${source.error.message}` : "";
    return { ...base, editable: false, reason: `This file could not be read as routing rules${detail}. Fix it by hand, then reload.`, form: null };
  }
  const parsed = routingRulesToForm(source.rules);
  if (!parsed.ok) return { ...base, editable: false, reason: parsed.reason, form: null };
  return { ...base, editable: true, reason: null, form: parsed.form };
}

function isOpenCodeExact(model: string): boolean {
  const slash = model.indexOf("/");
  return slash > 0 && slash < model.length - 1;
}

function checkRouteToken(token: string, key: "deny" | "auto_allow", engines: readonly string[]): string | null {
  const slash = token.indexOf("/");
  if (slash === -1) return null;
  const prefix = token.slice(0, slash);
  const rest = token.slice(slash + 1);
  if (!engines.includes(prefix) || !rest) return `The part before "/" must be an engine (${engines.join(", ")}); write an OpenCode model with its engine, e.g. opencode/${token}.`;
  if (prefix === "opencode" && key === "auto_allow" && !isOpenCodeExact(rest)) {
    return "Auto-allow lifts the OpenCode exclusion only for an exact opencode/<provider>/<model> route.";
  }
  return null;
}

export function buildRoutingRules(form: RoutingForm, vocabulary: RoutingVocabulary): Built<RoutingRules> {
  const errors: FieldErrors = {};
  const rules: RoutingRules = {};
  const { engines } = vocabulary;
  if (form.pin !== null) {
    const pin: Record<string, string> = {};
    form.pin.forEach((row, index) => {
      const key = `rules.pin[${index}]`;
      const role = row.role.trim();
      const model = row.model.trim();
      if (!ROLE_KEY.test(role)) addError(errors, key, "Role must be a worker role name or *.");
      else if (role in pin) addError(errors, key, "Each role is pinned once.");
      if (!engines.includes(row.engine)) addError(errors, key, `Choose an engine (${engines.join(", ")}).`);
      if (!model) addError(errors, key, "Enter the full model id.");
      else if (SPACE_OR_CONTROL.test(model) || model.includes(":")) addError(errors, key, "The model id has no spaces or \":\" (choose the effort separately).");
      else if (row.engine === "opencode" && !isOpenCodeExact(model)) {
        addError(errors, key, "An OpenCode pin names the exact provider/model id, e.g. anthropic/claude-sonnet-4-5.");
      }
      if (row.effort && !vocabulary.efforts.includes(row.effort)) addError(errors, key, `Effort must be one of ${vocabulary.efforts.join(", ")}.`);
      const value = composePin({ engine: row.engine, model, effort: row.effort });
      if (value.length > 256) addError(errors, key, "The pinned route is at most 256 characters.");
      pin[role] = value;
    });
    rules.pin = pin;
  }
  for (const key of ["deny", "auto_allow"] as const) {
    const values = checkList(form[key], `rules.${key}`, errors, { noSpaces: true, maxItems: 1000, check: (token) => checkRouteToken(token, key, engines) });
    if (values !== null) rules[key] = values;
  }
  if (form.tiers !== null) {
    const tiers: Record<string, string> = {};
    form.tiers.forEach((row, index) => {
      const key = `rules.tiers[${index}]`;
      const model = row.model.trim();
      if (!model || SPACE_OR_CONTROL.test(model)) addError(errors, key, "Enter a model id without spaces.");
      else if (model in tiers) addError(errors, key, "Each model has one tier.");
      if (!vocabulary.tiers.includes(row.tier)) addError(errors, key, `Tier must be one of ${vocabulary.tiers.join(", ")}.`);
      tiers[model] = row.tier;
    });
    rules.tiers = tiers;
  }
  if (form.providers !== null) {
    const providers = checkList(form.providers, "rules.providers", errors, { oneOf: engines, maxItems: 16 });
    if (form.providers.length === 0) addError(errors, "rules.providers", "Add at least one engine, or unset this setting.");
    rules.providers = providers ?? [];
  }
  if (form.engine_affinity !== null) {
    const affinity: Record<string, string | null> = {};
    form.engine_affinity.forEach((row, index) => {
      const key = `rules.engine_affinity[${index}]`;
      if (!vocabulary.workKinds.includes(row.kind)) addError(errors, key, `Work kind must be one of ${vocabulary.workKinds.join(", ")}.`);
      else if (row.kind in affinity) addError(errors, key, "Each work kind appears once.");
      if (row.engine && !engines.includes(row.engine)) addError(errors, key, `Engine must be none or one of ${engines.join(", ")}.`);
      affinity[row.kind] = row.engine || null;
    });
    rules.engine_affinity = affinity;
  }
  return Object.keys(errors).length > 0 ? { ok: false, errors } : { ok: true, value: rules };
}

export function buildRoutingUpdate(
  view: RoutingView, target: "user" | "project", workspaceId: string | null, form: RoutingForm,
): Built<RoutingInput> {
  const state = routingTarget(view, target);
  if (!state.editable) return { ok: false, errors: { target: state.reason ?? "This routing file is read-only." } };
  const rules = buildRoutingRules(form, routingVocabulary(view));
  if (!rules.ok) return rules;
  return {
    ok: true,
    value: { surface: "routing", target, ...(workspaceId ? { workspaceId } : {}), expected_revision: state.revision, rules: rules.value },
  };
}

// --- MCP servers --------------------------------------------------------------

export const MCP_ROLES = ["coordinator", "worker"] as const;
export const MCP_ENGINES = ["codex-acp", "claude-agent-acp", "opencode"] as const;
export const MCP_LIST_KEYS = [
  "args", "allowed_tools", "read_only_tools", "coordinator_tools", "always_load_tools", "read_paths", "write_paths", "runtime_dir_env",
] as const;
export type McpListKey = typeof MCP_LIST_KEYS[number];
const MCP_MANAGED_KEYS = new Set<string>(["command", "env", "enabled", "network", "roles", "engines", ...MCP_LIST_KEYS]);

/** One environment variable. A stored value is never shown: it is kept
 * (`{"keep": true}`) until the owner explicitly replaces or removes it. */
export type EnvRow = { name: string; stored: boolean; valueSet: boolean; mode: "keep" | "replace"; value: string };

/** Each optional key is `null` while it is absent from the entry. */
export type McpServerForm = {
  command: string;
  env: EnvRow[] | null;
  enabled: boolean | null;
  network: boolean | null;
  roles: string[] | null;
  engines: string[] | null;
} & Record<McpListKey, string[] | null>;

export function newMcpServerForm(): McpServerForm {
  return {
    command: "", env: null, enabled: null, network: null, roles: null, engines: null,
    args: null, allowed_tools: null, read_only_tools: null, coordinator_tools: null, always_load_tools: null,
    read_paths: null, write_paths: null, runtime_dir_env: null,
  };
}

export type McpFormResult = { ok: true; form: McpServerForm } | { ok: false; reason: string };

/** The stored entry as form fields, or why it is read-only here: a key the
 * API does not manage would be dropped by an upsert (the whole entry is
 * replaced), and a value of an unexpected shape cannot be shown exactly. */
export function mcpServerToForm(server: McpServerView): McpFormResult {
  if (server.unsupported_keys.length > 0) {
    return {
      ok: false,
      reason: `Also has settings this screen does not manage (${server.unsupported_keys.join(", ")}); saving here would drop them, so edit this entry by hand. Enable/disable and remove still keep them.`,
    };
  }
  const config = server.config as Record<string, unknown> | null;
  if (!config) return { ok: false, reason: "This entry is not an object; edit it by hand." };
  const unexpected = (key: string): McpFormResult => ({ ok: false, reason: `The stored "${key}" value has an unexpected shape; edit this entry by hand.` });
  const form = newMcpServerForm();
  if (typeof config.command !== "string") return unexpected("command");
  form.command = config.command;
  for (const key of Object.keys(config)) if (!MCP_MANAGED_KEYS.has(key)) return unexpected(key);
  for (const key of MCP_LIST_KEYS) {
    if (config[key] === undefined) continue;
    if (!isStringArray(config[key])) return unexpected(key);
    form[key] = [...(config[key] as string[])];
  }
  for (const key of ["enabled", "network"] as const) {
    if (config[key] === undefined) continue;
    if (typeof config[key] !== "boolean") return unexpected(key);
    form[key] = config[key] as boolean;
  }
  for (const [key, allowed] of [["roles", MCP_ROLES], ["engines", MCP_ENGINES]] as const) {
    if (config[key] === undefined) continue;
    const values = config[key];
    if (!isStringArray(values) || !values.every((item) => (allowed as readonly string[]).includes(item))) return unexpected(key);
    form[key] = [...values];
  }
  if (config.env !== undefined && config.env !== null) {
    if (!Array.isArray(config.env)) return unexpected("env");
    const rows: EnvRow[] = [];
    for (const item of config.env as Array<{ name: string | null; value_set: boolean }>) {
      if (typeof item?.name !== "string") return unexpected("env");
      rows.push({ name: item.name, stored: true, valueSet: item.value_set === true, mode: "keep", value: "" });
    }
    form.env = rows;
  }
  return { ok: true, form };
}

/** The `upsert` operation for `form`, or every problem keyed by the form's
 * own field names (`command`, `env.NAME`, `allowed_tools[2]`, ...). */
export function buildMcpUpsert(
  name: string, form: McpServerForm, options: { isNew: boolean; existingNames: readonly string[]; engines?: readonly string[] },
): Built<McpOperation> {
  const errors: FieldErrors = {};
  const trimmedName = name.trim();
  if (!SERVER_NAME.test(trimmedName) || trimmedName === "cortex") {
    addError(errors, "name", "Use letters, digits, '.', '_' or '-' (up to 64, not \"cortex\").");
  } else if (options.isNew && options.existingNames.includes(trimmedName)) {
    addError(errors, "name", "A server with this name already exists; edit it instead.");
  }
  const command = form.command.trim();
  if (!command) addError(errors, "command", "Enter the command to run.");
  else if (command.includes("\u0000") || command.length > 4096) addError(errors, "command", "At most 4096 characters, no NUL.");
  const server: Record<string, unknown> = { command };
  const lists: Record<McpListKey, ListRule> = {
    args: { trim: false, unique: false, maxLength: 16384, maxItems: 256 },
    allowed_tools: { maxItems: 512 },
    read_only_tools: { maxItems: 512 },
    coordinator_tools: { maxItems: 512 },
    always_load_tools: { maxItems: 512 },
    read_paths: { maxLength: 4096, maxItems: 256 },
    write_paths: { maxLength: 4096, maxItems: 256 },
    runtime_dir_env: { maxLength: 128, maxItems: 64 },
  };
  for (const key of MCP_LIST_KEYS) {
    const values = checkList(form[key], key, errors, lists[key]);
    if (values !== null) server[key] = values;
  }
  if (form.enabled !== null) server.enabled = form.enabled;
  if (form.network !== null) server.network = form.network;
  const roles = checkList(form.roles, "roles", errors, { oneOf: MCP_ROLES });
  if (roles !== null) server.roles = roles;
  const engines = checkList(form.engines, "engines", errors, { oneOf: options.engines ?? MCP_ENGINES });
  if (engines !== null) server.engines = engines;
  if (form.env !== null) {
    const env: Record<string, string | { keep: true }> = {};
    form.env.forEach((row, index) => {
      const envName = row.name.trim();
      const key = `env[${index}]`;
      if (!ENV_NAME.test(envName)) addError(errors, key, "Variable names are identifiers (A-Z, a-z, 0-9, _), not starting with a digit.");
      else if (envName in env) addError(errors, key, "Duplicate variable name.");
      if (row.mode === "keep") {
        if (!row.stored) addError(errors, key, "Enter a value: there is no stored value to keep.");
        env[envName] = { keep: true };
      } else {
        if (row.value.includes("\u0000") || row.value.length > 16384) addError(errors, key, "At most 16384 characters, no NUL.");
        env[envName] = row.value;
      }
    });
    server.env = env;
  }
  if (Object.keys(errors).length > 0) return { ok: false, errors };
  return { ok: true, value: { op: "upsert", name: trimmedName, server: server as McpServerInput } };
}

/** Maps an MCP write's field paths (`operations[0].server.env.X`) onto the
 * editor's own keys. */
export const MCP_ERROR_PREFIXES = ["operations[0].server.", "operations[0]."] as const;

export function mcpUpdate(view: McpServersView, operation: McpOperation): McpServersInput {
  return { surface: "mcp_servers", expected_revision: view.revision, operations: [operation] };
}

// --- connections --------------------------------------------------------------

export function buildConnectionRename(view: ConnectionsView, connectionId: string, name: string): Built<ConnectionsInput> {
  const errors: FieldErrors = {};
  const cleaned = cleanText(name, "name", errors, "a name", 120);
  if (Object.keys(errors).length > 0) return { ok: false, errors };
  return { ok: true, value: { surface: "connections", expected_revision: view.revision, connection_id: connectionId, operation: "rename", name: cleaned } };
}

export function buildConnectionMaxSessions(view: ConnectionsView, connectionId: string, text: string): Built<ConnectionsInput> {
  const parsed = parseWholeNumber(text, { min: 1, max: view.max_concurrent_sessions_limit });
  if (!parsed.ok) return { ok: false, errors: { max_concurrent_sessions: parsed.error } };
  if (parsed.value === null) return { ok: false, errors: { max_concurrent_sessions: "Enter a number of sessions." } };
  return {
    ok: true,
    value: {
      surface: "connections", expected_revision: view.revision, connection_id: connectionId,
      operation: "set_max_concurrent_sessions", max_concurrent_sessions: parsed.value,
    },
  };
}

export function connectionAction(view: ConnectionsView, connectionId: string, operation: "validate" | "remove"): ConnectionsInput {
  return { surface: "connections", expected_revision: view.revision, connection_id: connectionId, operation };
}

/** OpenCode v2 registration: only an explicit opt-in is ever sent. */
export function buildOnboard(view: ConnectionsView, optIn: boolean, name: string): Built<OnboardInput> {
  const errors: FieldErrors = {};
  if (!optIn) addError(errors, "explicit_opt_in", "Switch on the opt-in first.");
  const trimmed = name.trim();
  if (trimmed) cleanText(trimmed, "name", errors, "a name", 120);
  if (Object.keys(errors).length > 0) return { ok: false, errors };
  return { ok: true, value: { expected_revision: view.revision, explicit_opt_in: true, ...(trimmed ? { name: trimmed } : {}) } };
}
