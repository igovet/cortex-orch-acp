import { presentToolSemantics } from "../shared/tool-presentation.ts";
import { gatewayPresentation } from "../shared/gateway-presentation.ts";
import type { TranscriptEntry, WorkerEvent } from "./worker-panel-events.ts";

export type TranscriptItem =
  | { kind: "entry"; entry: TranscriptEntry }
  | { kind: "group"; id: number; entries: TranscriptEntry[]; summary: string; loading: boolean };

export function isToolEvent(event: WorkerEvent): boolean {
  return event.tool_kind != null || event.activity_ref?.startsWith("tool:") === true
    || event.activity_ref?.startsWith("tool-error:") === true || event.type.toLowerCase().includes("tool")
    || /^(?:Read|Edit|Shell|Terminal|Skill)(?:\s|$)|^mcp(?:__|\.)/i.test(event.title ?? "");
}

/** An MCP call from a server other than Cortex, e.g. `mcp.codebase_memory.search_code`. */
function externalMcp(event: WorkerEvent): boolean {
  return /^mcp(?:__|\.)/.test(event.title ?? "") && !/^(?:mcp\.cortex\.|mcp__cortex__)/.test(event.title ?? "");
}

export function toolSemantics(event: WorkerEvent) {
  return presentToolSemantics({
    // The gateway reports external MCP calls as `execute`; the chat names them
    // by their tool instead of calling them Shell, and so does the panel.
    name: event.title ?? event.tool_kind, title: event.title, toolKind: externalMcp(event) ? undefined : event.tool_kind,
    input: { ...(event.input && typeof event.input === "object" ? event.input as Record<string, unknown> : {}), content: event.content, locations: event.locations },
    rawInput: event.raw_input,
    rawOutput: event.raw_output ?? event.output,
  });
}

/** Classify once for the detail renderer, row and group counts. Only
 * file-oriented gateway operations get native Read/Edit treatment. */
export function nativeKind(event: WorkerEvent): "read" | "edit" | "shell" | "search" | "other" {
  const title = event.title ?? "";
  const input = event.raw_input ?? event.input;
  const envelope = input && typeof input === "object" ? input as Record<string, unknown> : {};
  const gateway = gatewayPresentation(title, input, event.raw_output ?? event.output);
  if (gateway) return gateway.kind;
  if (envelope.server === "cortex" || /^(?:mcp\.cortex\.|mcp__cortex__)/i.test(title)) return "other";
  if (externalMcp(event) || typeof envelope.server === "string") return "other";
  const kind = toolSemantics(event).kind;
  if (kind === "read" || kind === "edit") return kind;
  if (/search|grep|glob|web_search/i.test(`${title} ${event.tool_kind ?? ""}`)) return "search";
  return kind === "shell" ? kind : "other";
}

/** Paseo's resolveToolCallIconName, keyed by the same classification the
 * main chat uses, so a worker row carries the chat's Lucide icon. */
const CHAT_ICONS = { read: "Eye", edit: "Pencil", shell: "SquareTerminal", search: "Search", other: "Wrench" } as const;
export function chatToolIcon(event: WorkerEvent): string {
  return CHAT_ICONS[nativeKind(event)];
}

/** Paseo's overview summary (buildOverviewGroup + its label), word for word. */
export function summarizeToolRun(events: WorkerEvent[]): string {
  const edited = new Set<string>();
  const read = new Set<string>();
  let commands = 0, searches = 0, other = 0;
  for (const event of events) {
    const kind = nativeKind(event);
    const target = toolSemantics(event).target ?? `#${event.id}`;
    if (kind === "edit") edited.add(target);
    else if (kind === "shell") commands += 1;
    else if (kind === "read") read.add(target);
    else if (kind === "search") searches += 1;
    else other += 1;
  }
  const parts = ([
    [edited.size, "edited", "file", "files"],
    [commands, "ran", "command", "commands"],
    [read.size, "read", "file", "files"],
    [searches, "searched", "time", "times"],
    [other, "used", "other tool", "other tools"],
  ] as const).filter(([count]) => count > 0).map(([count, verb, one, many]) => `${verb} ${count} ${count === 1 ? one : many}`);
  const text = parts.length <= 1 ? parts[0] ?? ""
    : parts.length === 2 ? `${parts[0]} and ${parts[1]}`
    : `${parts.slice(0, -1).join(", ")}, and ${parts[parts.length - 1]}`;
  return text ? text[0].toLocaleUpperCase() + text.slice(1) : text;
}

/** Every run of consecutive tool rows becomes one expandable group, as in
 * Paseo's main chat; prose rows stay between groups. */
export function groupToolRuns(entries: TranscriptEntry[], active = true): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  let run: TranscriptEntry[] = [];
  const flush = () => {
    if (run.length === 0) return;
    const events = run.map((entry) => entry.event);
    items.push({
      kind: "group", id: events[0].id, entries: run, summary: summarizeToolRun(events),
      loading: active && events.some((event) => event.status === "in_progress" || event.status === "pending" || event.status === "running"),
    });
    run = [];
  };
  for (const entry of entries) {
    if (isToolEvent(entry.event)) run.push(entry);
    else { flush(); items.push({ kind: "entry", entry }); }
  }
  flush();
  return items;
}

function stringField(value: unknown, keys: readonly string[]): string | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  for (const key of keys) {
    const candidate = (value as Record<string, unknown>)[key];
    if (typeof candidate === "string" && candidate) return candidate;
  }
  return undefined;
}

/** Paseo's native row text (buildToolCallDisplayModel): Read/Edit + path,
 * Shell + command, otherwise the tool's own readable name. */
export function nativeRow(event: WorkerEvent): { label: string; summary?: string } {
  const kind = nativeKind(event);
  const semantics = toolSemantics(event);
  const gateway = gatewayPresentation(event.title ?? "", event.raw_input ?? event.input, event.raw_output ?? event.output);
  const input = gateway?.input ?? event.raw_input ?? event.input;
  if (kind === "read" || kind === "edit") {
    const path = stringField(input, ["filePath", "file_path", "path", "absolutePath"]) ?? semantics.target ?? undefined;
    return { label: kind === "read" ? "Read" : "Edit", ...(path ? { summary: path } : {}) };
  }
  if (kind === "shell") {
    const command = stringField(input, ["command", "cmd"]) ?? semantics.target ?? undefined;
    return { label: "Shell", ...(command ? { summary: command } : {}) };
  }
  return { label: semantics.label || semantics.action || event.title || "Tool call" };
}
