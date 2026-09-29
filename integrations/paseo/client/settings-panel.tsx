import { usePaseo, useRpc, type PluginSurfaceProps } from "@getpaseo/plugin/client";
import { Icon, ScrollView } from "@getpaseo/plugin/client/react-native";
import { useCallback, useEffect, useState } from "react";
import { Pressable, Switch, Text, TextInput, View } from "react-native";
import {
  discoverOpenCode,
  onboardOpenCode,
  readConfig,
  updateConfig,
  type ConfigBinding,
  type ConfigError,
  type ConfigUpdateInput,
  type McpOperation,
} from "../shared/config.js";
import {
  FALLBACK_DENYLIST,
  FALLBACK_ELIGIBLE_CLASSES,
  LIMIT_KEYS,
  MCP_ENGINES,
  MCP_ERROR_PREFIXES,
  MCP_ROLES,
  PERMISSION_LEVELS,
  SETTINGS_SECTIONS,
  buildConnectionMaxSessions,
  buildConnectionRename,
  buildMcpUpsert,
  buildOnboard,
  buildProjectCeilingUpdate,
  buildRoutingUpdate,
  buildUserDefaultsUpdate,
  composePin,
  connectionAction,
  describeConfigError,
  errorAt,
  errorFor,
  fieldErrorsFromConfigError,
  isRevisionConflict,
  mcpServerToForm,
  mcpUpdate,
  mergeErrors,
  newMcpServerForm,
  preservedDefaultKeys,
  projectCeilingRemoval,
  projectCeilingToForm,
  routingTarget,
  routingVocabulary,
  userDefaultsToForm,
  type AffinityRow,
  type CeilingForm,
  type CeilingView,
  type ConnectionView,
  type ConnectionsView,
  type EnvRow,
  type FieldErrors,
  type KeyValueRow,
  type McpServerForm,
  type McpServersView,
  type PinRow,
  type ProjectCeilingView,
  type RoutingForm,
  type RoutingView,
  type RoutingVocabulary,
  type SettingsSectionId,
  type TierRow,
  type UserDefaultsForm,
  type UserDefaultsView,
} from "./settings-model.ts";

/**
 * The Cortex Settings screen. Every editable setting is a typed control
 * (text, number, selector, toggle, add/remove list or row list) backed by
 * `settings-model.ts`; nothing here accepts raw JSON. Settings the API does
 * not manage, generated metadata and entries that cannot be shown without
 * loss are displayed read-only and never rewritten.
 */

type Theme = PluginSurfaceProps["theme"];

// --- visual primitives, matching the existing panels' inline style -------------

function replaceAt<T>(items: readonly T[], index: number, item: T): T[] {
  return items.map((current, i) => (i === index ? item : current));
}
function removeAt<T>(items: readonly T[], index: number): T[] {
  return items.filter((_item, i) => i !== index);
}
function move<T>(items: readonly T[], index: number, delta: number): T[] {
  const target = index + delta;
  if (target < 0 || target >= items.length) return [...items];
  const next = [...items];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}

function Label({ text, theme }: { text: string; theme: Theme }) {
  return <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, fontWeight: "600", marginBottom: 4 }}>{text}</Text>;
}

function Hint({ text, theme }: { text?: string | null; theme: Theme }) {
  if (!text) return null;
  return <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, marginBottom: 4 }}>{text}</Text>;
}

function ErrorText({ message, theme }: { message?: string; theme: Theme }) {
  if (!message) return null;
  return <Text accessibilityRole="alert" accessibilityLiveRegion="polite" style={{ color: theme.colors.statusDanger, fontSize: 11, marginTop: 3 }}>
    {message}
  </Text>;
}

function Card({ title, children, theme }: { title?: string; children: React.ReactNode; theme: Theme }) {
  return <View style={{ borderWidth: 1, borderColor: theme.colors.border, borderRadius: 9, padding: 12, gap: 10, backgroundColor: theme.colors.surface1 }}>
    {title ? <Text accessibilityRole="header" style={{ color: theme.colors.foreground, fontSize: 14, fontWeight: "700" }}>{title}</Text> : null}
    {children}
  </View>;
}

function Group({ title, children, theme, hint }: { title: string; children: React.ReactNode; theme: Theme; hint?: string }) {
  return <View style={{ gap: 8, borderTopWidth: 1, borderColor: theme.colors.border, paddingTop: 10 }}>
    <Text accessibilityRole="header" style={{ color: theme.colors.foreground, fontSize: 13, fontWeight: "600" }}>{title}</Text>
    <Hint text={hint} theme={theme} />
    {children}
  </View>;
}

function TextField({ label, value, onChangeText, theme, placeholder, hint, error, secure, numeric, editable = true, compact }: {
  label: string; value: string; onChangeText: (value: string) => void; theme: Theme; placeholder?: string; hint?: string;
  error?: string; secure?: boolean; numeric?: boolean; editable?: boolean; compact?: boolean;
}) {
  return <View style={compact ? { flex: 1, minWidth: 120 } : undefined}>
    {compact ? null : <Label text={label} theme={theme} />}
    <Hint text={hint} theme={theme} />
    <TextInput
      value={value}
      onChangeText={onChangeText}
      editable={editable}
      placeholder={placeholder}
      placeholderTextColor={theme.colors.foregroundMuted}
      accessibilityLabel={label}
      accessibilityHint={error}
      accessibilityState={{ disabled: !editable }}
      secureTextEntry={secure}
      keyboardType={numeric ? "number-pad" : "default"}
      inputMode={numeric ? "numeric" : "text"}
      autoCapitalize="none"
      autoCorrect={false}
      style={{ borderWidth: 1, borderColor: error ? theme.colors.statusDanger : theme.colors.border, borderRadius: 6, paddingHorizontal: 8,
        paddingVertical: 6, color: theme.colors.foreground, fontSize: 13, backgroundColor: editable ? theme.colors.surface0 : theme.colors.surface1 }}
    />
    <ErrorText message={error} theme={theme} />
  </View>;
}

function ToggleField({ label, value, onValueChange, theme, hint, disabled }: {
  label: string; value: boolean; onValueChange: (value: boolean) => void; theme: Theme; hint?: string; disabled?: boolean;
}) {
  return <View style={{ gap: 2 }}>
    <View style={{ flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
      <Text style={{ color: theme.colors.foreground, fontSize: 13, flex: 1 }}>{label}</Text>
      <Switch value={value} onValueChange={onValueChange} disabled={disabled} accessibilityLabel={label} accessibilityRole="switch"
        accessibilityState={{ checked: value, disabled: Boolean(disabled) }} />
    </View>
    <Hint text={hint} theme={theme} />
  </View>;
}

function Button({ title, onPress, theme, disabled, tone = "accent", icon, accessibilityLabel }: {
  title: string; onPress: () => void; theme: Theme; disabled?: boolean; tone?: "accent" | "danger" | "muted"; icon?: string; accessibilityLabel?: string;
}) {
  const color = tone === "danger" ? theme.colors.statusDanger : tone === "muted" ? theme.colors.foregroundMuted : theme.colors.accent;
  return <Pressable
    accessibilityRole="button"
    accessibilityLabel={accessibilityLabel ?? title}
    accessibilityState={{ disabled: Boolean(disabled) }}
    disabled={disabled}
    onPress={onPress}
    style={{ paddingVertical: 6, paddingHorizontal: 10, borderRadius: 6, borderWidth: 1, borderColor: color,
      alignSelf: "flex-start", opacity: disabled ? 0.5 : 1, flexDirection: "row", alignItems: "center", gap: 4 }}
  >
    {icon ? <Icon name={icon} size={12} color={color} /> : null}
    <Text style={{ color, fontSize: 12, fontWeight: "600" }}>{title}</Text>
  </Pressable>;
}

type Option<T extends string> = { value: T; label: string; disabled?: boolean };

/** A single choice among fixed options (an enum), as radio buttons. */
function ChoiceField<T extends string>({ label, value, options, onChange, theme, hint, error, compact, disabled }: {
  label: string; value: T; options: ReadonlyArray<Option<T>>; onChange: (value: T) => void; theme: Theme; hint?: string; error?: string;
  compact?: boolean; disabled?: boolean;
}) {
  return <View>
    {compact ? null : <Label text={label} theme={theme} />}
    <Hint text={hint} theme={theme} />
    <View accessibilityRole="radiogroup" accessibilityLabel={label} style={{ flexDirection: "row", flexWrap: "wrap", gap: 6 }}>
      {options.map((option) => {
        const selected = option.value === value;
        const off = Boolean(disabled || option.disabled);
        const color = selected ? theme.colors.accent : theme.colors.foregroundMuted;
        return <Pressable key={option.value} accessibilityRole="radio" accessibilityLabel={`${label}: ${option.label}`}
          accessibilityState={{ checked: selected, disabled: off }} disabled={off} onPress={() => onChange(option.value)}
          style={{ paddingVertical: 4, paddingHorizontal: 8, borderRadius: 6, borderWidth: 1, borderColor: color, opacity: off ? 0.5 : 1 }}>
          <Text style={{ color, fontSize: 12, fontWeight: selected ? "700" : "500" }}>{selected ? `● ${option.label}` : option.label}</Text>
        </Pressable>;
      })}
    </View>
    <ErrorText message={error} theme={theme} />
  </View>;
}

/** Several choices among fixed options, as checkboxes. */
function MultiChoiceField({ label, values, options, onChange, theme, error }: {
  label: string; values: readonly string[]; options: ReadonlyArray<Option<string>>; onChange: (values: string[]) => void; theme: Theme; error?: string;
}) {
  return <View>
    <Label text={label} theme={theme} />
    <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6 }}>
      {options.map((option) => {
        const checked = values.includes(option.value);
        const color = checked ? theme.colors.accent : theme.colors.foregroundMuted;
        return <Pressable key={option.value} accessibilityRole="checkbox" accessibilityLabel={`${label}: ${option.label}`}
          accessibilityState={{ checked }} onPress={() => onChange(checked ? values.filter((item) => item !== option.value) : [...values, option.value])}
          style={{ paddingVertical: 4, paddingHorizontal: 8, borderRadius: 6, borderWidth: 1, borderColor: color }}>
          <Text style={{ color, fontSize: 12 }}>{checked ? `☑ ${option.label}` : `☐ ${option.label}`}</Text>
        </Pressable>;
      })}
    </View>
    <ErrorText message={error} theme={theme} />
  </View>;
}

/** A setting that may be absent: a switch sets/unsets it and, while set,
 * shows its own control. `unsetText` says what absence means. */
function Optional<T>({ label, value, emptyValue, onChange, theme, unsetText, children, error }: {
  label: string; value: T | null; emptyValue: () => T; onChange: (value: T | null) => void; theme: Theme; unsetText: string;
  children: (value: T) => React.ReactNode; error?: string;
}) {
  return <View style={{ gap: 6 }}>
    <ToggleField label={label} value={value !== null} onValueChange={(on) => onChange(on ? emptyValue() : null)} theme={theme}
      hint={value === null ? unsetText : undefined} />
    {value !== null ? <View style={{ paddingLeft: 8, gap: 6 }}>{children(value)}</View> : null}
    <ErrorText message={error} theme={theme} />
  </View>;
}

/** An add/remove list of free-text entries (optionally reorderable). */
function StringListField({ label, values, onChange, theme, errors, errorKey, placeholder, ordered, secure }: {
  label: string; values: readonly string[]; onChange: (values: string[]) => void; theme: Theme; errors?: FieldErrors; errorKey: string;
  placeholder?: string; ordered?: boolean; secure?: boolean;
}) {
  return <View style={{ gap: 6 }}>
    {values.length === 0 ? <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>No entries.</Text> : null}
    {values.map((value, index) => <View key={index} style={{ flexDirection: "row", gap: 6, alignItems: "flex-start", flexWrap: "wrap" }}>
      <TextField compact label={`${label} ${index + 1}`} value={value} placeholder={placeholder} theme={theme} secure={secure}
        onChangeText={(text) => onChange(replaceAt(values, index, text))} error={errorFor(errors, `${errorKey}[${index}]`)} />
      {ordered ? <>
        <Button title="Up" icon="ChevronUp" accessibilityLabel={`Move ${label} ${index + 1} up`} theme={theme} tone="muted"
          disabled={index === 0} onPress={() => onChange(move(values, index, -1))} />
        <Button title="Down" icon="ChevronDown" accessibilityLabel={`Move ${label} ${index + 1} down`} theme={theme} tone="muted"
          disabled={index === values.length - 1} onPress={() => onChange(move(values, index, 1))} />
      </> : null}
      <Button title="Remove" icon="Trash2" accessibilityLabel={`Remove ${label} ${index + 1}`} theme={theme} tone="danger"
        onPress={() => onChange(removeAt(values, index))} />
    </View>)}
    <Button title={`Add ${label.toLowerCase()}`} icon="Plus" theme={theme} onPress={() => onChange([...values, ""])} />
    <ErrorText message={errorAt(errors, errorKey)} theme={theme} />
  </View>;
}

/** An ordered list whose entries come from fixed options (e.g. engines). */
function OrderedChoiceList({ label, values, options, onChange, theme, errors, errorKey }: {
  label: string; values: readonly string[]; options: readonly string[]; onChange: (values: string[]) => void; theme: Theme;
  errors?: FieldErrors; errorKey: string;
}) {
  const remaining = options.filter((option) => !values.includes(option));
  return <View style={{ gap: 6 }}>
    {values.length === 0 ? <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>No entries.</Text> : null}
    {values.map((value, index) => <View key={`${value}-${index}`} style={{ flexDirection: "row", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
      <Text style={{ color: theme.colors.foreground, fontSize: 13, minWidth: 140 }}>{index + 1}. {value}</Text>
      <Button title="Up" icon="ChevronUp" accessibilityLabel={`Move ${value} up`} theme={theme} tone="muted" disabled={index === 0}
        onPress={() => onChange(move(values, index, -1))} />
      <Button title="Down" icon="ChevronDown" accessibilityLabel={`Move ${value} down`} theme={theme} tone="muted" disabled={index === values.length - 1}
        onPress={() => onChange(move(values, index, 1))} />
      <Button title="Remove" icon="Trash2" accessibilityLabel={`Remove ${value} from ${label}`} theme={theme} tone="danger"
        onPress={() => onChange(removeAt(values, index))} />
      <ErrorText message={errorFor(errors, `${errorKey}[${index}]`)} theme={theme} />
    </View>)}
    {remaining.length > 0 ? <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6 }}>
      {remaining.map((option) => <Button key={option} title={`Add ${option}`} icon="Plus" accessibilityLabel={`Add ${option} to ${label}`}
        theme={theme} onPress={() => onChange([...values, option])} />)}
    </View> : null}
    <ErrorText message={errorAt(errors, errorKey)} theme={theme} />
  </View>;
}

/** An add/remove list of structured rows (map entries). */
function RowList<Row>({ label, rows, onChange, newRow, renderRow, theme, errors, errorKey, addLabel }: {
  label: string; rows: readonly Row[]; onChange: (rows: Row[]) => void; newRow: () => Row;
  renderRow: (row: Row, update: (row: Row) => void, index: number) => React.ReactNode; theme: Theme; errors?: FieldErrors; errorKey: string;
  addLabel: string;
}) {
  return <View style={{ gap: 6 }}>
    {rows.length === 0 ? <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>No entries.</Text> : null}
    {rows.map((row, index) => <View key={index} style={{ borderWidth: 1, borderColor: theme.colors.border, borderRadius: 6, padding: 8, gap: 6 }}>
      {renderRow(row, (next) => onChange(replaceAt(rows, index, next)), index)}
      <ErrorText message={errorFor(errors, `${errorKey}[${index}]`)} theme={theme} />
      <Button title="Remove" icon="Trash2" accessibilityLabel={`Remove ${label} ${index + 1}`} theme={theme} tone="danger"
        onPress={() => onChange(removeAt(rows, index))} />
    </View>)}
    <Button title={addLabel} icon="Plus" theme={theme} onPress={() => onChange([...rows, newRow()])} />
    <ErrorText message={errorAt(errors, errorKey)} theme={theme} />
  </View>;
}

function Banner({ text, theme, tone = "warning" }: { text: string; theme: Theme; tone?: "warning" | "danger" | "muted" | "success" }) {
  const color = tone === "danger" ? theme.colors.statusDanger : tone === "warning" ? theme.colors.statusWarning
    : tone === "success" ? theme.colors.accent : theme.colors.foregroundMuted;
  return <View accessibilityRole={tone === "danger" ? "alert" : undefined}
    style={{ borderWidth: 1, borderColor: color, borderRadius: 6, padding: 8, flexDirection: "row", alignItems: "center", gap: 6 }}>
    <Icon name={tone === "success" ? "Check" : "CircleAlert"} size={13} color={color} />
    <Text style={{ color, fontSize: 12, flex: 1 }}>{text}</Text>
  </View>;
}

/** A value shown for information only: generated, managed elsewhere, or not
 * representable as a typed field. */
function ReadOnlyRow({ label, value, theme }: { label: string; value: string; theme: Theme }) {
  return <View accessibilityLabel={`${label} (read-only): ${value}`} style={{ flexDirection: "row", gap: 6, flexWrap: "wrap" }}>
    <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, fontWeight: "600" }}>{label}:</Text>
    <Text selectable style={{ color: theme.colors.foregroundMuted, fontSize: 11, flexShrink: 1 }}>{value}</Text>
  </View>;
}

function ReadOnlyNotice({ text, theme }: { text: string; theme: Theme }) {
  return <View style={{ flexDirection: "row", gap: 6, alignItems: "center" }}>
    <Icon name="Lock" size={12} color={theme.colors.foregroundMuted} />
    <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, flex: 1 }}>{text}</Text>
  </View>;
}

const listText = (values: readonly string[] | null, unset = "any"): string => (values === null ? unset : values.length === 0 ? "none" : values.join(", "));

function CeilingSummary({ ceiling, theme }: { ceiling: CeilingView; theme: Theme }) {
  const limits = Object.entries(ceiling.limits).map(([key, value]) => `${key} ${value}`).join(", ") || "none";
  return <View style={{ gap: 2 }}>
    <ReadOnlyRow label="Allowed routes" value={listText(ceiling.allowed_routes)} theme={theme} />
    <ReadOnlyRow label="Allowed connections" value={listText(ceiling.allowed_connections)} theme={theme} />
    <ReadOnlyRow label="Allowed data recipients" value={listText(ceiling.allowed_data_recipients)} theme={theme} />
    <ReadOnlyRow label="Permission level" value={ceiling.permission_level ?? "unrestricted"} theme={theme} />
    <ReadOnlyRow label="Limits" value={limits} theme={theme} />
    <ReadOnlyRow label="Spend limit" value={ceiling.spend_limit === null ? "none" : `${ceiling.spend_limit}${ceiling.spend_limit_enforceable ? " (enforceable)" : ""}`} theme={theme} />
    <ReadOnlyRow label="Fallback target routes" value={listText(ceiling.fallback_policy.allowed_target_routes)} theme={theme} />
    <ReadOnlyRow label="Fallback failure classes" value={listText(ceiling.fallback_policy.allowed_failure_classes)} theme={theme} />
    <ReadOnlyRow label="Fallback budget" value={ceiling.fallback_policy.budget === null ? "none" : String(ceiling.fallback_policy.budget)} theme={theme} />
    <ReadOnlyRow label="Fallback data-recipient consent" value={ceiling.fallback_policy.data_recipient_consent ? "yes" : "no"} theme={theme} />
  </View>;
}

// --- generic surface read/write plumbing --------------------------------------

type ReadState<Result> =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; result: Result; binding: ConfigBinding };

type SaveError = { message: string; conflict: boolean; fields: FieldErrors };

/** One surface's read/reload/save cycle. Every write carries the revision
 * the form was built from; a stale one comes back as a conflict the owner
 * resolves by reloading, never by overwriting. */
function useConfigSurface<Result>(surface: string, workspaceId: string | null, requireWorkspace: boolean) {
  const read = useRpc(readConfig);
  const write = useRpc(updateConfig);
  const [state, setState] = useState<ReadState<Result>>({ status: "loading" });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<SaveError | null>(null);

  const reload = useCallback(async () => {
    setSaveError(null);
    if (requireWorkspace && !workspaceId) {
      setState({ status: "error", message: "Select a project workspace above to read this surface." });
      return;
    }
    setState({ status: "loading" });
    try {
      const out = await read({ surface: surface as never, ...(workspaceId ? { workspaceId } : {}) });
      // The read output's shape varies by surface; each caller asked for one.
      if (out.ok) setState({ status: "ready", result: out.result as unknown as Result, binding: out.binding });
      else setState({ status: "error", message: describeConfigError(out.error) });
    } catch (error) {
      setState({ status: "error", message: error instanceof Error ? error.message : "Read failed" });
    }
  }, [read, surface, workspaceId, requireWorkspace]);

  useEffect(() => { reload(); }, [reload]);

  const save = useCallback(async (input: ConfigUpdateInput, prefixes: readonly string[] = []): Promise<boolean> => {
    setSaving(true);
    setSaveError(null);
    try {
      const out = await write(input);
      if (out.ok) {
        setState({ status: "ready", result: out.result as unknown as Result, binding: out.binding });
        return true;
      }
      const error = out.error as ConfigError;
      setSaveError({ message: describeConfigError(error), conflict: isRevisionConflict(error), fields: fieldErrorsFromConfigError(error, prefixes) });
      return false;
    } catch (error) {
      setSaveError({ message: error instanceof Error ? error.message : "Save failed", conflict: false, fields: {} });
      return false;
    } finally {
      setSaving(false);
    }
  }, [write]);

  return { state, reload, save, saving, saveError, setSaveError };
}

function SurfaceFrame<Result>({ state, reload, theme, children }: {
  state: ReadState<Result>; reload: () => void; theme: Theme; children: (result: Result) => React.ReactNode;
}) {
  if (state.status === "loading") return <Text style={{ color: theme.colors.foregroundMuted, fontSize: 13 }}>Loading…</Text>;
  if (state.status === "error") return <View style={{ gap: 8 }}>
    <Banner text={state.message} theme={theme} tone="danger" />
    <Button title="Retry" onPress={reload} theme={theme} />
  </View>;
  return <>{children(state.result)}</>;
}

function SaveStatus({ saveError, notice, reload, theme }: { saveError: SaveError | null; notice: string | null; reload: () => void; theme: Theme }) {
  if (saveError) {
    return <View style={{ gap: 6 }}>
      <Banner text={saveError.conflict ? `Changed elsewhere since this was loaded; nothing was written. ${saveError.message}` : `Not saved: ${saveError.message}`}
        theme={theme} tone="danger" />
      {saveError.conflict && <Button title="Reload latest (discards edits here)" onPress={reload} theme={theme} />}
    </View>;
  }
  return notice ? <Banner text={notice} theme={theme} tone="muted" /> : null;
}

// --- ceiling editor (owner and project) ----------------------------------------

function CeilingEditor({ form, onChange, theme, errors, owner }: {
  form: CeilingForm; onChange: (form: CeilingForm) => void; theme: Theme; errors: FieldErrors; owner: string;
}) {
  const set = (patch: Partial<CeilingForm>) => onChange({ ...form, ...patch });
  const restricted = (key: "allowed_routes" | "allowed_connections" | "allowed_data_recipients" | "fallback_allowed_target_routes"
    | "fallback_allowed_failure_classes", label: string, errorKey: string, placeholder: string, hint?: string) =>
    <Optional label={`Restrict ${label}`} value={form[key]} emptyValue={() => [] as string[]} onChange={(value) => set({ [key]: value } as Partial<CeilingForm>)}
      theme={theme} unsetText={`No restriction: any ${label} allowed at this layer.`}>
      {(values) => <>
        <Hint text={hint} theme={theme} />
        <StringListField label={label.charAt(0).toUpperCase() + label.slice(1)} values={values} onChange={(next) => set({ [key]: next } as Partial<CeilingForm>)}
          theme={theme} errors={errors} errorKey={errorKey} placeholder={placeholder} />
      </>}
    </Optional>;
  return <View style={{ gap: 10 }}>
    {restricted("allowed_routes", "allowed routes", "ceiling.allowed_routes", "exact route id")}
    {restricted("allowed_connections", "allowed connections", "ceiling.allowed_connections", "cn_…")}
    {restricted("allowed_data_recipients", "allowed data recipients", "ceiling.allowed_data_recipients", "recipient")}
    <ChoiceField label={`${owner} permission level`} value={form.permission_level ?? ""} theme={theme}
      options={[{ value: "", label: "Unrestricted" }, ...PERMISSION_LEVELS.map((level) => ({ value: level, label: level }))]}
      onChange={(value) => set({ permission_level: value || null })} error={errorFor(errors, "ceiling.permission_level")} />
    <View style={{ gap: 6 }}>
      <Label text="Limits (blank = no limit)" theme={theme} />
      {[...LIMIT_KEYS, ...Object.keys(form.limits).filter((key) => !(LIMIT_KEYS as readonly string[]).includes(key))].map((key) =>
        <TextField key={key} label={`Limit: ${key}`} value={form.limits[key] ?? ""} numeric theme={theme} placeholder="no limit"
          onChangeText={(text) => set({ limits: { ...form.limits, [key]: text } })} error={errorFor(errors, `ceiling.limits.${key}`)} />)}
      <ErrorText message={errorAt(errors, "ceiling.limits")} theme={theme} />
    </View>
    <TextField label="Spend limit (blank = none)" value={form.spend_limit} numeric theme={theme} placeholder="none"
      onChangeText={(text) => set({ spend_limit: text })} error={errorFor(errors, "ceiling.spend_limit")} />
    <ToggleField label="Spend limit enforceable" value={form.spend_limit_enforceable} theme={theme}
      onValueChange={(value) => set({ spend_limit_enforceable: value })} hint="Required for a spend limit: unknown cost is never treated as zero." />
    <Group title="Fallback policy" theme={theme}>
      {restricted("fallback_allowed_target_routes", "fallback target routes", "ceiling.fallback_policy.allowed_target_routes", "exact route id")}
      {restricted("fallback_allowed_failure_classes", "fallback failure classes", "ceiling.fallback_policy.allowed_failure_classes", "failure class",
        `Acted on: ${FALLBACK_ELIGIBLE_CLASSES.join(", ")}. Never allowed: ${FALLBACK_DENYLIST.join(", ")}.`)}
      <TextField label="Fallback budget (blank = none)" value={form.fallback_budget} numeric theme={theme} placeholder="none"
        onChangeText={(text) => set({ fallback_budget: text })} error={errorFor(errors, "ceiling.fallback_policy.budget")} />
      <ToggleField label="Fallback data-recipient consent" value={form.fallback_data_recipient_consent} theme={theme}
        onValueChange={(value) => set({ fallback_data_recipient_consent: value })} />
    </Group>
    <ErrorText message={errors.ceiling} theme={theme} />
  </View>;
}

// --- User defaults ---------------------------------------------------------------

function UserDefaultsSection({ theme }: { theme: Theme }) {
  const surface = useConfigSurface<UserDefaultsView>("user_defaults", null, false);
  const connections = useConfigSurface<ConnectionsView>("connections", null, false);
  const [form, setForm] = useState<UserDefaultsForm | null>(null);
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [notice, setNotice] = useState<string | null>(null);
  const { state } = surface;

  useEffect(() => {
    if (state.status !== "ready") return;
    setForm(userDefaultsToForm(state.result));
    setClientErrors({});
  }, [state]);

  const errors = mergeErrors(clientErrors, surface.saveError?.fields);
  const set = (patch: Partial<UserDefaultsForm>) => setForm((prev) => (prev ? { ...prev, ...patch } : prev));

  const onSave = async () => {
    if (state.status !== "ready" || !form) return;
    const built = buildUserDefaultsUpdate(state.result, form);
    setNotice(null);
    if (!built.ok) { setClientErrors(built.errors); surface.setSaveError(null); return; }
    setClientErrors({});
    if (built.value === null) { setNotice("Nothing changed."); return; }
    if (await surface.save(built.value)) setNotice("Saved.");
  };

  const connectionOptions = (current: string) => {
    const listed = connections.state.status === "ready" ? connections.state.result.connections : [];
    const options = [{ value: "", label: "None" }, ...listed.map((item) => ({ value: item.id, label: `${item.name} (${item.engine} / ${item.provider})` }))];
    if (current && !listed.some((item) => item.id === current)) options.push({ value: current, label: `${current} (not registered)` });
    return options;
  };

  return <Card title="User defaults" theme={theme}>
    <SurfaceFrame state={state} reload={surface.reload} theme={theme}>
      {(result) => form ? <View style={{ gap: 10 }}>
        <ReadOnlyRow label="File" value={result.path} theme={theme} />
        <Group title="Default route" theme={theme} hint="Set the route id, connection and model together, or clear all three.">
          <TextField label="Route id" value={form.route_id} onChangeText={(value) => set({ route_id: value })} theme={theme}
            placeholder="xr_…" hint="An observed route id (xr_ followed by hex digits)." error={errorFor(errors, "defaults.route_id")} />
          {connections.state.status === "ready"
            ? <ChoiceField label="Connection" value={form.route_connection_id} options={connectionOptions(form.route_connection_id)} theme={theme}
              onChange={(value) => set({ route_connection_id: value })} error={errorFor(errors, "defaults.route_connection_id")} />
            : <TextField label="Connection id" value={form.route_connection_id} onChangeText={(value) => set({ route_connection_id: value })} theme={theme}
              placeholder="cn_…" hint="The connection list could not be read; enter the id." error={errorFor(errors, "defaults.route_connection_id")} />}
          <TextField label="Model (exact id)" value={form.route_model} onChangeText={(value) => set({ route_model: value })} theme={theme}
            hint="The route's full model id; OpenCode ids include the provider, e.g. anthropic/claude-sonnet-4-5."
            error={errorFor(errors, "defaults.route_model")} />
          <Button title="Clear default route" tone="muted" theme={theme} onPress={() => set({ route_id: "", route_connection_id: "", route_model: "" })} />
          <ErrorText message={errors.defaults} theme={theme} />
        </Group>
        <ChoiceField label="Language" value={form.language} theme={theme} error={errorFor(errors, "defaults.language")}
          options={[{ value: "", label: "Automatic" }, ...(result.languages.length > 0 ? result.languages : ["en", "ru"]).map((code) => ({ value: code, label: code }))]}
          onChange={(value) => set({ language: value })} />
        <Group title="Parameters" theme={theme} hint="Name (lowercase identifier) → value. Remove every row to delete the setting.">
          <RowList<KeyValueRow> label="Parameter" rows={form.parameters} onChange={(rows) => set({ parameters: rows })} theme={theme}
            errors={errors} errorKey="defaults.parameters" addLabel="Add parameter" newRow={() => ({ key: "", value: "" })}
            renderRow={(row, update, index) => <View style={{ flexDirection: "row", gap: 6, flexWrap: "wrap" }}>
              <TextField compact label={`Parameter ${index + 1} name`} value={row.key} placeholder="name" theme={theme}
                onChangeText={(key) => update({ ...row, key })} />
              <TextField compact label={`Parameter ${index + 1} value`} value={row.value} placeholder="value" theme={theme}
                onChangeText={(value) => update({ ...row, value })} />
            </View>} />
        </Group>
        {preservedDefaultKeys(result).length > 0 && <ReadOnlyNotice theme={theme}
          text={`Kept unchanged (not editable here): ${preservedDefaultKeys(result).join(", ")}`} />}
        <Group title="Owner ceiling" theme={theme} hint="The widest limits any project or task may use.">
          <ToggleField label="Set an owner ceiling" value={form.ceiling_set} onValueChange={(value) => set({ ceiling_set: value })} theme={theme}
            hint={form.ceiling_set ? undefined : result.ceiling_set ? "Saving now removes the owner ceiling." : "No owner ceiling: every dimension is unrestricted."} />
          {form.ceiling_set && <CeilingEditor form={form.ceiling} onChange={(ceiling) => set({ ceiling })} theme={theme} errors={errors} owner="Owner" />}
        </Group>
        <SaveStatus saveError={surface.saveError} notice={notice} reload={surface.reload} theme={theme} />
        {Object.keys(clientErrors).length > 0 && <Banner text="Fix the highlighted fields before saving." theme={theme} tone="danger" />}
        <Button title={surface.saving ? "Saving…" : "Save"} icon="Check" onPress={onSave} theme={theme} disabled={surface.saving} />
      </View> : null}
    </SurfaceFrame>
  </Card>;
}

// --- Project ceiling --------------------------------------------------------------

function ProjectCeilingSection({ theme, workspaceId }: { theme: Theme; workspaceId: string | null }) {
  const surface = useConfigSurface<ProjectCeilingView>("project_ceiling", workspaceId, true);
  const [form, setForm] = useState<CeilingForm | null>(null);
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const { state } = surface;

  useEffect(() => {
    if (state.status !== "ready") return;
    setForm(projectCeilingToForm(state.result));
    setClientErrors({});
    setConfirmRemove(false);
  }, [state]);

  if (!workspaceId) return <Card title="Project ceiling" theme={theme}>
    <Banner text="Select a project workspace above to read or set its ceiling." theme={theme} tone="muted" />
  </Card>;

  const errors = mergeErrors(clientErrors, surface.saveError?.fields);
  const onSave = async () => {
    if (state.status !== "ready" || !form) return;
    setNotice(null);
    const built = buildProjectCeilingUpdate(state.result, workspaceId, form);
    if (!built.ok) { setClientErrors(built.errors); surface.setSaveError(null); return; }
    setClientErrors({});
    if (await surface.save(built.value)) setNotice("Saved.");
  };
  const onRemove = async () => {
    if (state.status !== "ready") return;
    setNotice(null);
    if (await surface.save(projectCeilingRemoval(state.result, workspaceId))) setNotice("Project ceiling removed.");
  };

  return <Card title="Project ceiling" theme={theme}>
    <SurfaceFrame state={state} reload={surface.reload} theme={theme}>
      {(result) => form ? <View style={{ gap: 10 }}>
        <ReadOnlyRow label="Project" value={result.project_root} theme={theme} />
        {result.violation && <Banner theme={theme} tone="danger" text={`Violates the owner ceiling: ${result.violation.message}`} />}
        {!result.narrows_user_ceiling && !result.violation && result.exists &&
          <Banner theme={theme} tone="warning" text="This project ceiling no longer narrows the current owner ceiling." />}
        {!result.exists && <Banner theme={theme} tone="muted" text="No project ceiling is set; the fields start from the owner ceiling." />}
        <Group title="Owner ceiling (read-only; the project ceiling must narrow it)" theme={theme}>
          <CeilingSummary ceiling={result.user_ceiling} theme={theme} />
        </Group>
        <Group title="Effective ceiling (read-only)" theme={theme}>
          <CeilingSummary ceiling={result.effective_ceiling} theme={theme} />
        </Group>
        {Object.keys(result.project_defaults).length > 0 && <ReadOnlyNotice theme={theme}
          text={`Project defaults kept unchanged: ${Object.keys(result.project_defaults).sort().join(", ")}`} />}
        {result.updated_at && <ReadOnlyRow label="Updated" value={result.updated_at} theme={theme} />}
        <Group title="Project ceiling" theme={theme}>
          <CeilingEditor form={form} onChange={setForm} theme={theme} errors={errors} owner="Project" />
        </Group>
        <SaveStatus saveError={surface.saveError} notice={notice} reload={surface.reload} theme={theme} />
        {Object.keys(clientErrors).length > 0 && <Banner text="Fix the highlighted fields before saving." theme={theme} tone="danger" />}
        <View style={{ flexDirection: "row", gap: 8, flexWrap: "wrap" }}>
          <Button title={surface.saving ? "Saving…" : "Save"} icon="Check" onPress={onSave} theme={theme} disabled={surface.saving} />
          {result.exists && (confirmRemove
            ? <>
              <Button title="Confirm remove" tone="danger" onPress={onRemove} theme={theme} disabled={surface.saving} />
              <Button title="Cancel" tone="muted" onPress={() => setConfirmRemove(false)} theme={theme} />
            </>
            : <Button title="Remove project ceiling" icon="Trash2" onPress={() => setConfirmRemove(true)} theme={theme} disabled={surface.saving} tone="danger" />)}
        </View>
      </View> : null}
    </SurfaceFrame>
  </Card>;
}

// --- Routing ----------------------------------------------------------------------

function RoutingEditor({ form, onChange, vocabulary, theme, errors }: {
  form: RoutingForm; onChange: (form: RoutingForm) => void; vocabulary: RoutingVocabulary; theme: Theme; errors: FieldErrors;
}) {
  const set = (patch: Partial<RoutingForm>) => onChange({ ...form, ...patch });
  const engineOptions = vocabulary.engines.map((engine) => ({ value: engine, label: vocabulary.optionalEngines.includes(engine) ? `${engine} (optional)` : engine }));
  return <View style={{ gap: 12 }}>
    <Optional label="Pin roles to routes" value={form.pin} emptyValue={() => [] as PinRow[]} onChange={(pin) => set({ pin })} theme={theme}
      unsetText="Not set: roles are routed automatically." error={errorAt(errors, "rules.pin")}>
      {(rows) => <RowList<PinRow> label="Pin" rows={rows} onChange={(pin) => set({ pin })} theme={theme} errors={errors} errorKey="rules.pin"
        addLabel="Add pin" newRow={() => ({ role: "*", engine: vocabulary.engines[0] ?? "", model: "", effort: "" })}
        renderRow={(row, update, index) => <View style={{ gap: 6 }}>
          <TextField label={`Pin ${index + 1} role`} value={row.role} placeholder="* or a worker role" theme={theme}
            onChangeText={(role) => update({ ...row, role })} />
          <ChoiceField label={`Pin ${index + 1} engine`} value={row.engine} options={row.engine && !vocabulary.engines.includes(row.engine)
            ? [...engineOptions, { value: row.engine, label: `${row.engine} (unknown)` }] : engineOptions} theme={theme}
            onChange={(engine) => update({ ...row, engine })} />
          <TextField label={`Pin ${index + 1} model (full id)`} value={row.model} theme={theme}
            placeholder={row.engine === "opencode" ? "provider/model, e.g. anthropic/claude-sonnet-4-5" : "model id"}
            onChangeText={(model) => update({ ...row, model })} />
          <ChoiceField label={`Pin ${index + 1} effort`} value={row.effort} theme={theme}
            options={[{ value: "", label: "Engine default" }, ...vocabulary.efforts.map((effort) => ({ value: effort, label: effort })),
              ...(row.effort && !vocabulary.efforts.includes(row.effort) ? [{ value: row.effort, label: `${row.effort} (unknown)` }] : [])]}
            onChange={(effort) => update({ ...row, effort })} />
          <ReadOnlyRow label="Pinned route" value={composePin(row) || "—"} theme={theme} />
        </View>} />}
    </Optional>
    <Optional label="Deny list" value={form.deny} emptyValue={() => [] as string[]} onChange={(deny) => set({ deny })} theme={theme}
      unsetText="Not set: the built-in deny list applies.">
      {(values) => <StringListField label="Denied route or engine" values={values} onChange={(deny) => set({ deny })} theme={theme}
        errors={errors} errorKey="rules.deny" placeholder="engine, engine/model or model" />}
    </Optional>
    <Optional label="Auto-allow list" value={form.auto_allow} emptyValue={() => [] as string[]} onChange={(auto_allow) => set({ auto_allow })} theme={theme}
      unsetText="Not set: nothing is lifted from the automatic-routing exclusions.">
      {(values) => <>
        <Hint theme={theme} text="OpenCode routes are lifted only by an exact opencode/<provider>/<model> entry." />
        <StringListField label="Auto-allowed route" values={values} onChange={(auto_allow) => set({ auto_allow })} theme={theme}
          errors={errors} errorKey="rules.auto_allow" placeholder="engine/model" />
      </>}
    </Optional>
    <Optional label="Model tiers" value={form.tiers} emptyValue={() => [] as TierRow[]} onChange={(tiers) => set({ tiers })} theme={theme}
      unsetText="Not set: built-in tiers apply." error={errorAt(errors, "rules.tiers")}>
      {(rows) => <RowList<TierRow> label="Tier" rows={rows} onChange={(tiers) => set({ tiers })} theme={theme} errors={errors} errorKey="rules.tiers"
        addLabel="Add model tier" newRow={() => ({ model: "", tier: vocabulary.tiers[0] ?? "" })}
        renderRow={(row, update, index) => <View style={{ gap: 6 }}>
          <TextField label={`Tier ${index + 1} model id`} value={row.model} theme={theme} onChangeText={(model) => update({ ...row, model })} />
          <ChoiceField label={`Tier ${index + 1} tier`} value={row.tier} theme={theme} onChange={(tier) => update({ ...row, tier })}
            options={vocabulary.tiers.map((tier) => ({ value: tier, label: tier }))} />
        </View>} />}
    </Optional>
    <Optional label="Engine preference order" value={form.providers} emptyValue={() => [] as string[]} onChange={(providers) => set({ providers })}
      theme={theme} unsetText="Not set: the built-in engine order applies.">
      {(values) => <OrderedChoiceList label="Engine preference" values={values} options={vocabulary.engines} theme={theme} errors={errors}
        errorKey="rules.providers" onChange={(providers) => set({ providers })} />}
    </Optional>
    <Optional label="Engine affinity per work kind" value={form.engine_affinity} emptyValue={() => [] as AffinityRow[]}
      onChange={(engine_affinity) => set({ engine_affinity })} theme={theme} unsetText="Not set: built-in affinities apply."
      error={errorAt(errors, "rules.engine_affinity")}>
      {(rows) => <RowList<AffinityRow> label="Affinity" rows={rows} onChange={(engine_affinity) => set({ engine_affinity })} theme={theme}
        errors={errors} errorKey="rules.engine_affinity" addLabel="Add affinity"
        newRow={() => ({ kind: vocabulary.workKinds.find((kind) => !rows.some((row) => row.kind === kind)) ?? "*", engine: "" })}
        renderRow={(row, update, index) => <View style={{ gap: 6 }}>
          <ChoiceField label={`Affinity ${index + 1} work kind`} value={row.kind} theme={theme} onChange={(kind) => update({ ...row, kind })}
            options={[...vocabulary.workKinds.map((kind) => ({ value: kind, label: kind })),
              ...(vocabulary.workKinds.includes(row.kind) ? [] : [{ value: row.kind, label: `${row.kind} (unknown)` }])]} />
          <ChoiceField label={`Affinity ${index + 1} engine`} value={row.engine} theme={theme} onChange={(engine) => update({ ...row, engine })}
            options={[{ value: "", label: "None" }, ...engineOptions,
              ...(row.engine && !vocabulary.engines.includes(row.engine) ? [{ value: row.engine, label: `${row.engine} (unknown)` }] : [])]} />
        </View>} />}
    </Optional>
  </View>;
}

function RoutingSection({ theme, workspaceId }: { theme: Theme; workspaceId: string | null }) {
  const surface = useConfigSurface<RoutingView>("routing", workspaceId, false);
  const [target, setTarget] = useState<"user" | "project">("user");
  const [form, setForm] = useState<RoutingForm | null>(null);
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [notice, setNotice] = useState<string | null>(null);
  const { state } = surface;

  useEffect(() => {
    if (!workspaceId && target === "project") setTarget("user");
  }, [workspaceId, target]);
  useEffect(() => {
    if (state.status !== "ready") return;
    setForm(routingTarget(state.result, target).form);
    setClientErrors({});
  }, [state, target]);

  const errors = mergeErrors(clientErrors, surface.saveError?.fields);
  const onSave = async () => {
    if (state.status !== "ready" || !form) return;
    setNotice(null);
    const built = buildRoutingUpdate(state.result, target, workspaceId, form);
    if (!built.ok) { setClientErrors(built.errors); surface.setSaveError(null); return; }
    setClientErrors({});
    if (await surface.save(built.value)) setNotice("Saved.");
  };

  return <Card title="Routing" theme={theme}>
    <SurfaceFrame state={state} reload={surface.reload} theme={theme}>
      {(result) => {
        const vocabulary = routingVocabulary(result);
        const current = routingTarget(result, target);
        return <View style={{ gap: 10 }}>
          <View style={{ gap: 2 }}>
            <ReadOnlyRow label="Effective source" value={`${result.effective_source}${result.effective_valid ? "" : " (invalid)"}`} theme={theme} />
            <ReadOnlyRow label="Precedence" value={result.precedence.join(" → ")} theme={theme} />
            {result.sources.map((source) => <ReadOnlyRow key={source.source} label={`${source.source} file`} theme={theme}
              value={`${source.path} — ${source.exists ? (source.writable ? "editable" : "read-only") : "absent"}${source.valid === false ? " — invalid" : ""}${source.error ? ` — ${source.error.message}` : ""}`} />)}
            <ReadOnlyRow label="Engines" value={`${result.builtin.supported_engines.join(", ")} · optional: ${result.builtin.optional_engines.join(", ") || "none"}`} theme={theme} />
            <ReadOnlyRow label="Excluded from automatic routing by default" value={[...result.builtin.default_auto_deny_engines, ...result.builtin.default_auto_deny_families].join(", ") || "none"} theme={theme} />
            <ReadOnlyRow label="Efforts" value={result.builtin.efforts.join(", ")} theme={theme} />
          </View>
          <ChoiceField label="File to edit" value={target} theme={theme} onChange={(value) => setTarget(value)}
            options={[{ value: "user", label: "User routing file" }, { value: "project", label: "Project routing file", disabled: !workspaceId }]}
            hint={workspaceId ? undefined : "Select a project workspace above to edit its routing file."} />
          {current.ignoredKeys.length > 0 && <ReadOnlyNotice theme={theme} text={`Kept unchanged (not managed here): ${current.ignoredKeys.join(", ")}`} />}
          {current.source?.valid === false && current.editable && current.source.error &&
            <Banner theme={theme} tone="warning" text={`This file is currently invalid: ${current.source.error.message}`} />}
          {!current.editable || !form
            ? <ReadOnlyNotice theme={theme} text={current.reason ?? "This routing file is read-only."} />
            : <>
              <RoutingEditor form={form} onChange={setForm} vocabulary={vocabulary} theme={theme} errors={errors} />
              {result.shadowed_by && <Banner theme={theme} tone="warning" text={`Saved, but the ${result.shadowed_by} source still takes precedence over this file.`} />}
              <SaveStatus saveError={surface.saveError} notice={notice} reload={surface.reload} theme={theme} />
              {Object.keys(clientErrors).length > 0 && <Banner text="Fix the highlighted fields before saving." theme={theme} tone="danger" />}
              <Button title={surface.saving ? "Saving…" : `Save ${target} routing`} icon="Check" onPress={onSave} theme={theme} disabled={surface.saving} />
            </>}
        </View>;
      }}
    </SurfaceFrame>
  </Card>;
}

// --- MCP servers -------------------------------------------------------------------

function EnvEditor({ rows, onChange, theme, errors }: { rows: EnvRow[]; onChange: (rows: EnvRow[]) => void; theme: Theme; errors: FieldErrors }) {
  return <RowList<EnvRow> label="Environment variable" rows={rows} onChange={onChange} theme={theme} errors={errors} errorKey="env"
    addLabel="Add environment variable" newRow={() => ({ name: "", stored: false, valueSet: false, mode: "replace", value: "" })}
    renderRow={(row, update, index) => row.stored
      ? <View style={{ gap: 6 }}>
        <ReadOnlyRow label="Variable" value={`${row.name} — ${row.valueSet ? "value stored (never shown)" : "stored value is empty"}`} theme={theme} />
        <ChoiceField label={`${row.name} value`} value={row.mode} theme={theme}
          options={[{ value: "keep", label: "Keep stored value" }, { value: "replace", label: "Replace value" }]}
          onChange={(mode) => update({ ...row, mode, value: mode === "keep" ? "" : row.value })} />
        {row.mode === "replace" && <TextField label={`New value for ${row.name}`} value={row.value} secure theme={theme}
          onChangeText={(value) => update({ ...row, value })} />}
      </View>
      : <View style={{ flexDirection: "row", gap: 6, flexWrap: "wrap" }}>
        <TextField compact label={`Environment variable ${index + 1} name`} value={row.name} placeholder="NAME" theme={theme}
          onChangeText={(name) => update({ ...row, name })} />
        <TextField compact label={`Environment variable ${index + 1} value`} value={row.value} placeholder="value" secure theme={theme}
          onChangeText={(value) => update({ ...row, value })} />
      </View>} />;
}

function TriStateField({ label, value, onChange, theme, unsetLabel }: {
  label: string; value: boolean | null; onChange: (value: boolean | null) => void; theme: Theme; unsetLabel: string;
}) {
  const current = value === null ? "unset" : value ? "on" : "off";
  return <ChoiceField label={label} value={current} theme={theme}
    options={[{ value: "unset", label: unsetLabel }, { value: "on", label: "On" }, { value: "off", label: "Off" }]}
    onChange={(next) => onChange(next === "unset" ? null : next === "on")} />;
}

const MCP_LIST_LABELS: Array<[Exclude<keyof McpServerForm, "command" | "env" | "enabled" | "network" | "roles" | "engines">, string, string]> = [
  ["allowed_tools", "Allowed tools", "Not set: every tool the server offers."],
  ["read_only_tools", "Read-only tools", "Not set."],
  ["coordinator_tools", "Coordinator tools", "Not set."],
  ["always_load_tools", "Always-loaded tools", "Not set."],
  ["read_paths", "Read paths", "Not set."],
  ["write_paths", "Write paths", "Not set."],
  ["runtime_dir_env", "Runtime-directory variables", "Not set."],
];

function McpServerEditor({ name, isNew, initial, existingNames, engines, onCancel, onSave, saving, serverErrors, theme }: {
  name: string; isNew: boolean; initial: McpServerForm; existingNames: readonly string[]; engines: readonly string[]; onCancel: () => void;
  onSave: (operation: McpOperation) => Promise<boolean>; saving: boolean; serverErrors: FieldErrors | undefined; theme: Theme;
}) {
  const [serverName, setServerName] = useState(name);
  const [form, setForm] = useState<McpServerForm>(initial);
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const errors = mergeErrors(clientErrors, serverErrors);
  const set = (patch: Partial<McpServerForm>) => setForm((prev) => ({ ...prev, ...patch }));
  const submit = async () => {
    const built = buildMcpUpsert(serverName, form, { isNew, existingNames, engines });
    if (!built.ok) { setClientErrors(built.errors); return; }
    setClientErrors({});
    await onSave(built.value);
  };
  const hasRedacted = form.command.includes("[redacted]") || (form.args ?? []).some((part) => part.includes("[redacted]"));
  return <View style={{ gap: 10, borderWidth: 1, borderColor: theme.colors.accent, borderRadius: 6, padding: 10 }}>
    <Text accessibilityRole="header" style={{ color: theme.colors.foreground, fontSize: 13, fontWeight: "700" }}>
      {isNew ? "New MCP server" : `Edit ${name}`}
    </Text>
    {isNew
      ? <TextField label="Server name" value={serverName} onChangeText={setServerName} theme={theme} placeholder="my-server" error={errorFor(errors, "name")} />
      : <ReadOnlyRow label="Server name" value={name} theme={theme} />}
    <TextField label="Command" value={form.command} onChangeText={(command) => set({ command })} theme={theme} error={errorFor(errors, "command")}
      placeholder="executable" />
    <Optional label="Arguments" value={form.args} emptyValue={() => [] as string[]} onChange={(args) => set({ args })} theme={theme} unsetText="No arguments.">
      {(values) => <StringListField label="Argument" values={values} onChange={(args) => set({ args })} theme={theme} errors={errors} errorKey="args" ordered />}
    </Optional>
    {hasRedacted && <Hint theme={theme} text="Parts shown as [redacted] keep their stored value when saved; replace the whole part to change it." />}
    <Optional label="Environment variables" value={form.env} emptyValue={() => [] as EnvRow[]} onChange={(env) => set({ env })} theme={theme}
      unsetText="No environment variables." error={errorAt(errors, "env")}>
      {(rows) => <>
        <Hint theme={theme} text="Stored values are never shown. Keep them, replace them, or remove the variable." />
        <EnvEditor rows={rows} onChange={(env) => set({ env })} theme={theme} errors={errors} />
      </>}
    </Optional>
    <TriStateField label="Enabled" value={form.enabled} onChange={(enabled) => set({ enabled })} theme={theme} unsetLabel="Default (on)" />
    <TriStateField label="Network access" value={form.network} onChange={(network) => set({ network })} theme={theme} unsetLabel="Default" />
    <Optional label="Limit to roles" value={form.roles} emptyValue={() => [] as string[]} onChange={(roles) => set({ roles })} theme={theme}
      unsetText="Not set: every role." error={errorAt(errors, "roles")}>
      {(values) => <MultiChoiceField label="Roles" values={values} theme={theme} onChange={(roles) => set({ roles })}
        options={MCP_ROLES.map((role) => ({ value: role, label: role }))} />}
    </Optional>
    <Optional label="Limit to engines" value={form.engines} emptyValue={() => [] as string[]} onChange={(next) => set({ engines: next })} theme={theme}
      unsetText="Not set: every engine." error={errorAt(errors, "engines")}>
      {(values) => <MultiChoiceField label="Engines" values={values} theme={theme} onChange={(next) => set({ engines: next })}
        options={engines.map((engine) => ({ value: engine, label: engine }))} />}
    </Optional>
    {MCP_LIST_LABELS.map(([key, label, unsetText]) => <Optional key={key} label={label} value={form[key]} emptyValue={() => [] as string[]}
      onChange={(values) => set({ [key]: values } as Partial<McpServerForm>)} theme={theme} unsetText={unsetText}>
      {(values) => <StringListField label={label.replace(/s$/, "")} values={values} onChange={(next) => set({ [key]: next } as Partial<McpServerForm>)} theme={theme}
        errors={errors} errorKey={key} />}
    </Optional>)}
    {Object.keys(clientErrors).length > 0 && <Banner text="Fix the highlighted fields before saving." theme={theme} tone="danger" />}
    <View style={{ flexDirection: "row", gap: 8 }}>
      <Button title={saving ? "Saving…" : "Save server"} icon="Check" onPress={submit} theme={theme} disabled={saving} />
      <Button title="Cancel" tone="muted" onPress={onCancel} theme={theme} />
    </View>
  </View>;
}

function McpServersSection({ theme }: { theme: Theme }) {
  const surface = useConfigSurface<McpServersView>("mcp_servers", null, false);
  const [editing, setEditing] = useState<{ name: string; isNew: boolean; form: McpServerForm } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { state } = surface;

  const run = async (operation: McpOperation, done: string): Promise<boolean> => {
    if (state.status !== "ready") return false;
    setNotice(null);
    const ok = await surface.save(mcpUpdate(state.result, operation), MCP_ERROR_PREFIXES);
    if (ok) { setNotice(done); setEditing(null); setConfirmRemove(null); }
    return ok;
  };

  return <Card title="Owner MCP servers" theme={theme}>
    <SurfaceFrame state={state} reload={surface.reload} theme={theme}>
      {(result) => {
        const names = result.servers.map((server) => server.name);
        const engines = result.engines.length > 0 ? result.engines : [...MCP_ENGINES];
        return <View style={{ gap: 10 }}>
          <ReadOnlyRow label="File" value={result.path} theme={theme} />
          {result.servers.length === 0 && <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>No MCP servers configured.</Text>}
          {result.servers.map((server) => {
            const parsed = mcpServerToForm(server);
            const config = (server.config ?? {}) as Record<string, unknown>;
            const env = Array.isArray(config.env) ? (config.env as Array<{ name: string | null; value_set: boolean }>) : [];
            return <View key={server.name} style={{ borderWidth: 1, borderColor: theme.colors.border, borderRadius: 6, padding: 8, gap: 6 }}>
              <Text accessibilityRole="header" style={{ color: theme.colors.foreground, fontSize: 13, fontWeight: "600" }}>{server.name}</Text>
              {server.enabled !== null && <ToggleField label={`${server.name} enabled`} value={server.enabled} theme={theme} disabled={surface.saving}
                onValueChange={(enabled) => run({ op: "set_enabled", name: server.name, enabled }, `${server.name} ${enabled ? "enabled" : "disabled"}.`)} />}
              {!server.valid && server.problem && <Banner text={server.problem.message} theme={theme} tone="danger" />}
              {typeof config.command === "string" && <ReadOnlyRow label="Command" value={[config.command, ...(Array.isArray(config.args) ? config.args.map(String) : [])].join(" ")} theme={theme} />}
              {env.length > 0 && <ReadOnlyRow label="Environment" value={env.map((item) => `${item.name ?? "?"}${item.value_set ? " (set)" : " (empty)"}`).join(", ")} theme={theme} />}
              {server.redacted && <Hint theme={theme} text="Credential-looking parts are shown as [redacted]." />}
              {!parsed.ok && <ReadOnlyNotice theme={theme} text={parsed.reason} />}
              {editing && !editing.isNew && editing.name === server.name
                ? <McpServerEditor name={server.name} isNew={false} initial={editing.form} existingNames={names} engines={engines} saving={surface.saving}
                  serverErrors={surface.saveError?.fields} theme={theme} onCancel={() => { setEditing(null); surface.setSaveError(null); }}
                  onSave={(operation) => run(operation, `${server.name} saved.`)} />
                : <View style={{ flexDirection: "row", gap: 8, flexWrap: "wrap" }}>
                  <Button title="Edit" theme={theme} disabled={!parsed.ok || surface.saving} accessibilityLabel={`Edit ${server.name}`}
                    onPress={() => { if (parsed.ok) { surface.setSaveError(null); setEditing({ name: server.name, isNew: false, form: parsed.form }); } }} />
                  {confirmRemove === server.name
                    ? <>
                      <Button title={`Confirm remove ${server.name}`} tone="danger" theme={theme} disabled={surface.saving}
                        onPress={() => run({ op: "remove", name: server.name }, `${server.name} removed.`)} />
                      <Button title="Cancel" tone="muted" theme={theme} onPress={() => setConfirmRemove(null)} />
                    </>
                    : <Button title="Remove" icon="Trash2" tone="danger" theme={theme} disabled={surface.saving} accessibilityLabel={`Remove ${server.name}`}
                      onPress={() => setConfirmRemove(server.name)} />}
                </View>}
            </View>;
          })}
          {editing?.isNew
            ? <McpServerEditor name="" isNew initial={editing.form} existingNames={names} engines={engines} saving={surface.saving}
              serverErrors={surface.saveError?.fields} theme={theme} onCancel={() => { setEditing(null); surface.setSaveError(null); }}
              onSave={(operation) => run(operation, "Server added.")} />
            : <Button title="Add MCP server" icon="Plus" theme={theme} disabled={surface.saving}
              onPress={() => { surface.setSaveError(null); setEditing({ name: "", isNew: true, form: newMcpServerForm() }); }} />}
          <SaveStatus saveError={surface.saveError} notice={notice} reload={surface.reload} theme={theme} />
        </View>;
      }}
    </SurfaceFrame>
  </Card>;
}

// --- Connections + OpenCode v2 ------------------------------------------------------

function ConnectionCard({ connection, view, busy, save, errors, theme }: {
  connection: ConnectionView; view: ConnectionsView; busy: boolean; save: (input: ConfigUpdateInput) => Promise<boolean>;
  errors: FieldErrors | undefined; theme: Theme;
}) {
  const [name, setName] = useState(connection.name);
  const [sessions, setSessions] = useState(String(connection.max_concurrent_sessions));
  const [clientErrors, setClientErrors] = useState<FieldErrors>({});
  const [confirmRemove, setConfirmRemove] = useState(false);
  useEffect(() => { setName(connection.name); setSessions(String(connection.max_concurrent_sessions)); }, [connection]);
  const allowed = (operation: string) => view.operations.length === 0 || view.operations.includes(operation);
  const shown = mergeErrors(clientErrors, errors);
  const apply = async (built: ReturnType<typeof buildConnectionRename>) => {
    if (!built.ok) { setClientErrors(built.errors); return; }
    setClientErrors({});
    await save(built.value);
  };
  return <View style={{ borderWidth: 1, borderColor: theme.colors.border, borderRadius: 6, padding: 8, gap: 6 }}>
    <Text accessibilityRole="header" style={{ color: theme.colors.foreground, fontSize: 13, fontWeight: "600" }}>
      {connection.name} — {connection.engine} / {connection.provider}
    </Text>
    <View style={{ gap: 2 }}>
      <ReadOnlyRow label="Id" value={connection.id} theme={theme} />
      <ReadOnlyRow label="State" value={`${connection.state}${connection.qualified ? "" : " (unqualified)"}${connection.optional_engine ? " · optional engine" : ""}`} theme={theme} />
      <ReadOnlyRow label="Auth" value={`${connection.auth_mode}${connection.key_kind ? ` · ${connection.key_kind}` : ""}`} theme={theme} />
      <ReadOnlyRow label="Credential" value={`${connection.credential.kind}${connection.credential.owned === undefined ? "" : connection.credential.owned ? " (owned)" : " (referenced)"}`} theme={theme} />
      {connection.endpoint && <ReadOnlyRow label="Endpoint" value={connection.endpoint} theme={theme} />}
      {connection.launch_command && <ReadOnlyRow label="Launch command" value={connection.launch_command.join(" ")} theme={theme} />}
      <ReadOnlyRow label="Created / updated" value={`${connection.created_at} / ${connection.updated_at}`} theme={theme} />
    </View>
    <View style={{ flexDirection: "row", gap: 8, alignItems: "flex-start", flexWrap: "wrap" }}>
      <TextField compact label={`Name of ${connection.id}`} value={name} onChangeText={setName} theme={theme} error={errorFor(shown, "name")} />
      <Button title="Rename" theme={theme} disabled={busy || !allowed("rename") || name.trim() === connection.name}
        accessibilityLabel={`Rename ${connection.name}`} onPress={() => apply(buildConnectionRename(view, connection.id, name))} />
    </View>
    <View style={{ flexDirection: "row", gap: 8, alignItems: "flex-start", flexWrap: "wrap" }}>
      <TextField compact numeric label={`Max concurrent sessions for ${connection.name} (1-${view.max_concurrent_sessions_limit})`} value={sessions}
        onChangeText={setSessions} theme={theme} error={errorFor(shown, "max_concurrent_sessions")} />
      <Button title="Apply" theme={theme} accessibilityLabel={`Apply max concurrent sessions for ${connection.name}`}
        disabled={busy || !allowed("set_max_concurrent_sessions") || sessions.trim() === String(connection.max_concurrent_sessions)}
        onPress={() => apply(buildConnectionMaxSessions(view, connection.id, sessions))} />
    </View>
    <View style={{ flexDirection: "row", gap: 8, flexWrap: "wrap" }}>
      <Button title="Validate" theme={theme} disabled={busy || !allowed("validate")} accessibilityLabel={`Validate ${connection.name}`}
        onPress={() => save(connectionAction(view, connection.id, "validate"))} />
      {confirmRemove
        ? <>
          <Button title={`Confirm remove ${connection.name}`} tone="danger" theme={theme} disabled={busy}
            onPress={() => save(connectionAction(view, connection.id, "remove"))} />
          <Button title="Cancel" tone="muted" theme={theme} onPress={() => setConfirmRemove(false)} />
        </>
        : <Button title="Remove" icon="Trash2" tone="danger" theme={theme} disabled={busy || !allowed("remove")} accessibilityLabel={`Remove ${connection.name}`}
          onPress={() => setConfirmRemove(true)} />}
    </View>
  </View>;
}

type Discovery = { status: "idle" | "loading" | "error"; message?: string; found?: boolean; version?: string; qualified?: boolean; qualifiedVersions?: string };

function ConnectionsSection({ theme }: { theme: Theme }) {
  const surface = useConfigSurface<ConnectionsView>("connections", null, false);
  const discover = useRpc(discoverOpenCode);
  const onboard = useRpc(onboardOpenCode);
  const [discovery, setDiscovery] = useState<Discovery>({ status: "idle" });
  const [optIn, setOptIn] = useState(false);
  const [openCodeName, setOpenCodeName] = useState("");
  const [onboarding, setOnboarding] = useState<{ busy: boolean; error: string | null; conflict: boolean; fields: FieldErrors }>({ busy: false, error: null, conflict: false, fields: {} });
  const [activeConnection, setActiveConnection] = useState<string | null>(null);
  const { state } = surface;

  const runDiscover = async () => {
    setDiscovery({ status: "loading" });
    try {
      const out = await discover({});
      if (!out.ok) { setDiscovery({ status: "error", message: describeConfigError(out.error) }); return; }
      setDiscovery({
        status: "idle", found: out.result.found, version: out.result.version, qualified: out.result.qualified,
        qualifiedVersions: out.result.qualified_versions === "any" ? "any" : out.result.qualified_versions.join(", "),
      });
    } catch (error) {
      setDiscovery({ status: "error", message: error instanceof Error ? error.message : "Discovery failed" });
    }
  };

  const runOnboard = async () => {
    if (state.status !== "ready") return;
    const built = buildOnboard(state.result, optIn, openCodeName);
    if (!built.ok) { setOnboarding({ busy: false, error: null, conflict: false, fields: built.errors }); return; }
    setOnboarding({ busy: true, error: null, conflict: false, fields: {} });
    try {
      const out = await onboard(built.value);
      if (out.ok) {
        setOnboarding({ busy: false, error: null, conflict: false, fields: {} });
        setOptIn(false);
        await surface.reload();
        return;
      }
      setOnboarding({ busy: false, error: describeConfigError(out.error), conflict: isRevisionConflict(out.error), fields: fieldErrorsFromConfigError(out.error) });
    } catch (error) {
      setOnboarding({ busy: false, error: error instanceof Error ? error.message : "OpenCode registration failed", conflict: false, fields: {} });
    }
  };

  return <Card title="Connections" theme={theme}>
    <SurfaceFrame state={state} reload={surface.reload} theme={theme}>
      {(result) => <View style={{ gap: 10 }}>
        {result.connections.length === 0 && <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>No connections registered.</Text>}
        {result.connections.map((connection) => <ConnectionCard key={connection.id} connection={connection} view={result} busy={surface.saving}
          theme={theme} errors={activeConnection === connection.id ? surface.saveError?.fields : undefined}
          save={(input) => { setActiveConnection(connection.id); return surface.save(input); }} />)}
        <SaveStatus saveError={surface.saveError} notice={null} reload={surface.reload} theme={theme} />
        <Group title="Engines (read-only)" theme={theme}>
          {result.engines.map((engine) => <ReadOnlyRow key={engine.engine} label={`${engine.engine}${engine.optional ? " (optional)" : ""}`} theme={theme}
            value={`onboarding: ${engine.onboarding} · qualified: ${engine.qualified_versions === "any" ? "any" : engine.qualified_versions.join(", ") || "none"} · connections: ${engine.connection_ids.length}`} />)}
        </Group>
        <Group title="OpenCode v2 (optional engine)" theme={theme}
          hint="Opt-in only. Registering never changes routing: OpenCode routes stay excluded from automatic routing until you pin or auto-allow an exact opencode/<provider>/<model> route under Routing.">
          <Button title={discovery.status === "loading" ? "Checking…" : "Check for OpenCode"} theme={theme} onPress={runDiscover} disabled={discovery.status === "loading"} />
          {discovery.status === "error" && <Banner text={discovery.message ?? "Discovery failed"} theme={theme} tone="danger" />}
          {discovery.status === "idle" && discovery.found !== undefined && <View style={{ gap: 6 }}>
            <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>
              {discovery.found
                ? `Found version ${discovery.version ?? "unknown"} — ${discovery.qualified ? "qualified" : "not qualified"} (qualified versions: ${discovery.qualifiedVersions})`
                : "No OpenCode binary found in the standard locations."}
            </Text>
            {discovery.found && discovery.qualified && <>
              <ToggleField label="I want to enable OpenCode v2 for this owner" value={optIn} onValueChange={setOptIn} theme={theme} />
              <TextField label="Connection name (optional)" value={openCodeName} onChangeText={setOpenCodeName} theme={theme} placeholder="OpenCode"
                error={errorFor(onboarding.fields, "name")} />
              <ErrorText message={errorFor(onboarding.fields, "explicit_opt_in")} theme={theme} />
              <Button title={onboarding.busy ? "Enabling…" : "Enable OpenCode v2"} theme={theme} disabled={!optIn || onboarding.busy} onPress={runOnboard} />
            </>}
          </View>}
          {onboarding.error && <View style={{ gap: 6 }}>
            <Banner text={onboarding.conflict ? `Connections changed elsewhere; nothing was registered. ${onboarding.error}` : `Not registered: ${onboarding.error}`}
              theme={theme} tone="danger" />
            {onboarding.conflict && <Button title="Reload latest" onPress={() => { setOnboarding({ busy: false, error: null, conflict: false, fields: {} }); surface.reload(); }} theme={theme} />}
          </View>}
          {result.opencode && <Banner theme={theme} tone="success"
            text={`Onboarded OpenCode ${result.opencode.version} — next: ${result.opencode.next_action ?? "none"} — automatic routing: ${result.opencode.automatic_routing}`} />}
        </Group>
      </View>}
    </SurfaceFrame>
  </Card>;
}

// --- Top-level screen ------------------------------------------------------------

function WorkspacePicker({ theme, workspaceId, onChange }: { theme: Theme; workspaceId: string | null; onChange: (id: string | null) => void }) {
  const paseo = usePaseo();
  const [workspaces, setWorkspaces] = useState<Array<{ id: string; label: string }> | null>(null);
  useEffect(() => {
    let cancelled = false;
    paseo.workspaces.list().then((listed) => {
      if (cancelled) return;
      setWorkspaces(listed.entries.map((entry) => {
        // The daemon's workspace descriptor carries a display name under one
        // of a few keys depending on host version; fall back to the id.
        const named = entry as unknown as { title?: string | null; name?: string | null; workspaceDirectory?: string };
        return { id: entry.id, label: named.title ?? named.name ?? named.workspaceDirectory ?? entry.id };
      }));
    }).catch(() => { if (!cancelled) setWorkspaces([]); });
    return () => { cancelled = true; };
  }, [paseo]);

  return <ChoiceField label="Project (for project ceiling and project routing)" value={workspaceId ?? ""} theme={theme}
    options={[{ value: "", label: "Owner only" }, ...(workspaces ?? []).map((workspace) => ({ value: workspace.id, label: workspace.label }))]}
    onChange={(value) => onChange(value || null)} />;
}

/** The dedicated Cortex Settings screen (`client.addSettingsScreen`). Every
 * section reads and writes the owner configuration API directly
 * (`cortex.config.*` plugin RPCs); Paseo's own settings document is never
 * used, so this UI and hand-edited Cortex files stay one source of truth. */
export function SettingsScreen({ theme }: PluginSurfaceProps) {
  const [section, setSection] = useState<SettingsSectionId>("user_defaults");
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);

  return <ScrollView style={{ flex: 1, backgroundColor: theme.colors.surface0 }} contentContainerStyle={{ padding: 14, gap: 14 }}>
    <Text accessibilityRole="header" style={{ color: theme.colors.foreground, fontSize: 18, fontWeight: "700" }}>Cortex settings</Text>
    <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>
      Every setting is a form field. Save writes it to the Cortex configuration files (the project ceiling to the project store); a change made
      elsewhere since loading is refused, never overwritten. Settings Cortex manages elsewhere are shown read-only.
    </Text>
    <WorkspacePicker theme={theme} workspaceId={workspaceId} onChange={setWorkspaceId} />
    <ChoiceField label="Section" value={section} theme={theme} onChange={setSection}
      options={SETTINGS_SECTIONS.map((entry) => ({ value: entry.id, label: entry.title }))} />
    {section === "user_defaults" && <UserDefaultsSection theme={theme} />}
    {section === "project_ceiling" && <ProjectCeilingSection theme={theme} workspaceId={workspaceId} />}
    {section === "routing" && <RoutingSection theme={theme} workspaceId={workspaceId} />}
    {section === "mcp_servers" && <McpServersSection theme={theme} />}
    {section === "connections" && <ConnectionsSection theme={theme} />}
  </ScrollView>;
}
