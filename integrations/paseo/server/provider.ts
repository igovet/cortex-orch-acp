import { ReplayController } from "./replay-connector.ts";
import { ReplaySpool } from "./replay-spool.mjs";
import { isHiddenOperation } from "../shared/timeline-visibility.ts";
import { presentToolSemantics } from "../shared/tool-presentation.ts";
import { gatewayPresentation } from "../shared/gateway-presentation.ts";
import { shellCommand } from "../shared/shell-command.ts";
import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { homedir } from "node:os";
import { runAcpProvider } from "@getpaseo/plugin/server/acp";
import type {
  ProviderEvent,
  ProviderRegistration,
  ProviderToolCallDetail,
} from "@getpaseo/plugin/server/provider";
import {
  CORTEX_QUESTION_INPUT_MARKER,
  isCortexQuestionRequest,
} from "../shared/question-marker.ts";
import { parseWorkerResult } from "../shared/worker-timeline-result.ts";

type CommandArgv = [string, ...string[]];

type LaunchConfig = {
  argv: string[];
  env?: Record<string, string>;
  shim?: string;
};

const REPLAY_USER_MESSAGE_ID = /^__cortex_history_user__[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}:[1-9][0-9]*$/;

function repairFailedTimelineError(event: ProviderEvent): ProviderEvent {
  if (
    event.type === "timeline.item" &&
    event.item.type === "tool_call" &&
    event.item.status === "failed" &&
    event.item.error === null
  ) {
    return {
      ...event,
      item: {
        ...event.item,
        error: `Tool call failed: ${event.item.name}`,
      },
    };
  }
  return event;
}

function restoreReplayUserMessage(event: ProviderEvent): ProviderEvent {
  if (
    event.type === "timeline.item" &&
    event.item.type === "assistant_message" &&
    typeof event.item.messageId === "string" &&
    (REPLAY_USER_MESSAGE_ID.test(event.item.messageId) || event.item.messageId.startsWith("__cortex_history_user_v2__"))
  ) {
    const { messageId: _messageId, ...item } = event.item;
    const id = _messageId.startsWith("__cortex_history_user_v2__")
      ? Buffer.from(_messageId.slice("__cortex_history_user_v2__".length), "base64url").toString("utf8")
      : item.id;
    return { ...event, item: { ...item, id, type: "user_message" } };
  }
  return event;
}

type ToolCallTimelineItem = Extract<ProviderEvent, { type: "timeline.item" }>['item'] & {
  type: "tool_call";
};

function firstString(value: unknown, keys: readonly string[]): string | undefined {
  if (typeof value === "string") return value;
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  for (const key of keys) {
    const candidate = (value as Record<string, unknown>)[key];
    if (typeof candidate === "string") return candidate;
  }
  return undefined;
}

function firstNumber(value: unknown, keys: readonly string[]): number | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  for (const key of keys) {
    const candidate = (value as Record<string, unknown>)[key];
    if (typeof candidate === "number" && Number.isFinite(candidate)) return candidate;
  }
  return undefined;
}

/**
 * Turn the exact Claude file/shell payloads into Paseo's supported detail
 * union.  The stock ACP bridge otherwise emits `unknown`, which makes the
 * main chat show Input/Output JSON even when rawInput/rawOutput identify a
 * native Read or Shell call.  Keep the source values in metadata so this
 * presentation projection never becomes the record of the call.
 */
export function nativeToolDetail(event: ProviderEvent): ProviderEvent {
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") return event;
  const item = event.item as ToolCallTimelineItem;
  if (item.detail.type !== "unknown") return event;
  const rawInput = item.detail.input;
  const rawOutput = item.detail.output;
  const gateway = gatewayPresentation(item.name, rawInput, rawOutput);
  const input = gateway?.input ?? rawInput;
  const output = gateway?.output ?? rawOutput;
  const name = gateway?.kind === "edit" ? "write" : gateway?.kind === "read" ? "read" : gateway?.kind === "shell" ? "shell" : item.name.trim().toLowerCase();
  const filePath = firstString(input, ["filePath", "file_path", "path", "absolutePath"]);
  const command = shellCommand(input);
  const oldString = firstString(input, ["oldString", "old_string", "oldText", "old_text"]);
  const newString = firstString(input, ["newString", "new_string", "newText", "new_text", "content"]);
  const unifiedDiff = firstString(input, ["unifiedDiff", "unified_diff", "patch", "diff"]);
  const readContent = firstString(output, ["content", "text", "output", "stdout"])
    ?? (typeof output === "string" ? output : undefined);
  const shellOutput = firstString(output, ["stdout", "output", "content", "text"])
    ?? (typeof output === "string" ? output : undefined);

  let detail: ProviderToolCallDetail | undefined;
  if (filePath && (/^(?:edit|patch|write|replace|modify)\b/.test(name)
    || oldString !== undefined || newString !== undefined || unifiedDiff !== undefined)) {
    detail = {
      type: "edit",
      filePath,
      ...(oldString === undefined ? {} : { oldString }),
      ...(newString === undefined ? {} : { newString }),
      ...(unifiedDiff === undefined ? {} : { unifiedDiff }),
    };
  } else if (filePath && /^(?:read|view|cat)\b/.test(name)) {
    detail = {
      type: "read",
      filePath,
      ...(readContent === undefined ? {} : { content: readContent }),
      ...(firstNumber(input, ["offset"]) === undefined ? {} : { offset: firstNumber(input, ["offset"]) }),
      ...(firstNumber(input, ["limit"]) === undefined ? {} : { limit: firstNumber(input, ["limit"]) }),
    };
  } else if (command && /^(?:shell|bash|terminal|execute|run(?:\s+command)?)\b/.test(name)) {
    const exitCode = firstNumber(output, ["exitCode", "exit_code", "code"]);
    detail = {
      type: "shell",
      command,
      ...(firstString(input, ["cwd", "directory", "workingDirectory"]) === undefined
        ? {}
        : { cwd: firstString(input, ["cwd", "directory", "workingDirectory"]) }),
      ...(shellOutput === undefined ? {} : { output: shellOutput }),
      ...(exitCode === undefined ? {} : { exitCode }),
    };
  }
  if (!detail) return event;
  return {
    ...event,
    item: {
      ...item,
      detail,
      metadata: {
        ...(item.metadata ?? {}),
        cortexRawInput: rawInput,
        cortexRawOutput: rawOutput,
      },
    },
  };
}


function visibleTimeline(event: ProviderEvent): boolean {
  return !(event.type === "timeline.item" && (event.item.type === "reasoning"
    || event.item.type === "tool_call" && isHiddenOperation(event.item.name)));
}

const TIMELINE_DETAIL_MAX_BYTES = 16 * 1024;
const TIMELINE_RESULT_MAX_BYTES = 16 * 1024;
const TIMELINE_ERROR_MAX_BYTES = 1024;
const TIMELINE_ID_MAX_BYTES = 512;
const TIMELINE_LABEL_MAX_BYTES = 1024;
const TIMELINE_SESSION_MAX_BYTES = 256;
const TIMELINE_TIMESTAMP_MAX_BYTES = 128;
const MAX_BOUNDED_COLLECTION_ITEMS = 256;
const MAX_BOUNDED_DEPTH = 8;

type BoundedJson = null | boolean | number | string | BoundedJson[] | { [key: string]: BoundedJson };
type BoundedJsonResult = { value: BoundedJson; bytes: number };
type TimelineItemEvent = Extract<ProviderEvent, { type: "timeline.item" }>;

function jsonStringContentByteLength(character: string): number {
  const codePoint = character.codePointAt(0) ?? 0;
  if (codePoint === 0x22 || codePoint === 0x5c
    || codePoint === 0x08 || codePoint === 0x09 || codePoint === 0x0a
    || codePoint === 0x0c || codePoint === 0x0d) return 2;
  if (codePoint < 0x20 || (codePoint >= 0xd800 && codePoint <= 0xdfff)) return 6;
  return codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
}

function hasUnpairedSurrogate(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const codeUnit = value.charCodeAt(index);
    if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (next < 0xdc00 || next > 0xdfff) return true;
      index += 1;
    } else if (codeUnit >= 0xdc00 && codeUnit <= 0xdfff) {
      return true;
    }
  }
  return false;
}

function jsonStringByteLength(value: string): number {
  if (!/["\\\u0000-\u001f]/.test(value) && !hasUnpairedSurrogate(value)) {
    return Buffer.byteLength(value, "utf8") + 2;
  }
  let bytes = 2;
  for (const character of value) bytes += jsonStringContentByteLength(character);
  return bytes;
}

/** Clip a string so its JSON representation fits a byte budget without splitting UTF-8. */
function clipJsonString(value: string, maxBytes: number): string {
  const limit = Math.max(0, Math.floor(maxBytes));
  if (jsonStringByteLength(value) <= limit) return value;
  if (limit < 2) return "";
  const totalUtf8Bytes = Buffer.byteLength(value, "utf8");
  const fullMarker = "… [truncated " + totalUtf8Bytes + " bytes]";
  const markerContentBytes = jsonStringByteLength(fullMarker) - 2;
  if (markerContentBytes + 2 > limit) {
    const prefix: string[] = [];
    let contentBytes = 0;
    for (const character of value) {
      const nextBytes = jsonStringContentByteLength(character);
      if (contentBytes + nextBytes + 2 > limit) break;
      prefix.push(character);
      contentBytes += nextBytes;
    }
    return prefix.join("");
  }
  const contentBudget = limit - 2 - markerContentBytes;
  const prefix: string[] = [];
  let contentBytes = 0;
  let prefixUtf8Bytes = 0;
  for (const character of value) {
    const nextBytes = jsonStringContentByteLength(character);
    if (contentBytes + nextBytes > contentBudget) break;
    prefix.push(character);
    contentBytes += nextBytes;
    prefixUtf8Bytes += Buffer.byteLength(character, "utf8");
  }
  const omittedUtf8Bytes = Math.max(0, totalUtf8Bytes - prefixUtf8Bytes);
  return prefix.join("") + "… [truncated " + omittedUtf8Bytes + " bytes]";
}

/** Preserve JSON shape while bounding the complete serialized value and its nesting. */
function boundJsonValue(value: unknown, maxBytes: number): BoundedJson | undefined {
  const visit = (input: unknown, budget: number, depth: number, ancestors: Set<object>): BoundedJsonResult | undefined => {
    if (budget < 1 || depth > MAX_BOUNDED_DEPTH) return undefined;
    if (input === null) return budget >= 4 ? { value: null, bytes: 4 } : undefined;
    if (typeof input === "boolean") {
      const bytes = input ? 4 : 5;
      return budget >= bytes ? { value: input, bytes } : undefined;
    }
    if (typeof input === "number") {
      const encoded = Number.isFinite(input) ? JSON.stringify(input) : "null";
      if (encoded === undefined) return undefined;
      const bytes = Buffer.byteLength(encoded, "utf8");
      return budget >= bytes ? { value: Number.isFinite(input) ? input : null, bytes } : undefined;
    }
    if (typeof input === "string") {
      const clipped = clipJsonString(input, budget);
      const bytes = jsonStringByteLength(clipped);
      return bytes <= budget ? { value: clipped, bytes } : undefined;
    }
    if (typeof input !== "object" || input === undefined || ancestors.has(input)) return undefined;
    if (Array.isArray(input)) {
      if (budget < 2) return undefined;
      ancestors.add(input);
      const output: BoundedJson[] = [];
      let bytes = 2;
      const count = Math.min(input.length, MAX_BOUNDED_COLLECTION_ITEMS);
      for (let index = 0; index < count; index += 1) {
        const separatorBytes = output.length === 0 ? 0 : 1;
        const child = visit(input[index], budget - bytes - separatorBytes, depth + 1, ancestors);
        if (!child) break;
        output.push(child.value);
        bytes += separatorBytes + child.bytes;
      }
      ancestors.delete(input);
      return { value: output, bytes };
    }

    if (budget < 2) return undefined;
    ancestors.add(input);
    const output: Record<string, BoundedJson> = Object.create(null) as Record<string, BoundedJson>;
    let bytes = 2;
    let stored = 0;
    let visited = 0;
    for (const key in input as Record<string, unknown>) {
      if (!Object.prototype.hasOwnProperty.call(input, key)) continue;
      if (visited >= MAX_BOUNDED_COLLECTION_ITEMS) break;
      visited += 1;
      const separatorBytes = stored === 0 ? 0 : 1;
      const keyBudget = Math.min(256, budget - bytes - separatorBytes - 3);
      if (keyBudget < 2) break;
      const boundedKey = clipJsonString(key, keyBudget);
      const keyBytes = jsonStringByteLength(boundedKey);
      const fixedBytes = separatorBytes + keyBytes + 1;
      const child = visit((input as Record<string, unknown>)[key], budget - bytes - fixedBytes, depth + 1, ancestors);
      if (!child || Object.prototype.hasOwnProperty.call(output, boundedKey)) continue;
      output[boundedKey] = child.value;
      bytes += fixedBytes + child.bytes;
      stored += 1;
    }
    ancestors.delete(input);
    return { value: output, bytes };
  };
  return visit(value, Math.max(0, Math.floor(maxBytes)), 0, new Set())?.value;
}

// Required keys of Paseo's ToolCallDetail union. The daemon closes the whole
// provider connection on the first schema-invalid timeline item, so budget
// trimming must never drop one of these.
const TOOL_DETAIL_REQUIRED_KEYS: Record<string, readonly string[]> = {
  shell: ["command"],
  read: ["filePath"],
  edit: ["filePath"],
  write: ["filePath"],
  search: ["query"],
  fetch: ["url"],
  worktree_setup: ["worktreePath", "branchName", "log", "commands"],
  sub_agent: ["log"],
  plain_text: [],
  plan: ["text"],
};

const OMITTED_TOOL_DETAIL = {
  type: "plain_text",
  label: "Tool details",
  text: "Tool details omitted to stay within the timeline budget.",
} as const;

function boundToolDetail(detail: unknown): BoundedJson {
  if (!detail || typeof detail !== "object" || Array.isArray(detail)) return OMITTED_TOOL_DETAIL;
  const source = detail as Record<string, unknown>;
  if (source.type === "unknown") {
    // Split the budget so an oversized input cannot starve the required output key.
    const half = Math.floor((TIMELINE_DETAIL_MAX_BYTES - 64) / 2);
    return {
      type: "unknown",
      input: boundJsonValue(source.input, half) ?? null,
      output: boundJsonValue(source.output, half) ?? null,
    };
  }
  const required = typeof source.type === "string" ? TOOL_DETAIL_REQUIRED_KEYS[source.type] : undefined;
  const bounded = boundJsonValue(detail, TIMELINE_DETAIL_MAX_BYTES);
  if (!required || !bounded || typeof bounded !== "object" || Array.isArray(bounded)
    || required.some((key) => !Object.prototype.hasOwnProperty.call(bounded, key))) {
    return OMITTED_TOOL_DETAIL;
  }
  return bounded;
}

function boundedWorkerResult(value: unknown): ReturnType<typeof parseWorkerResult> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const source = value as Record<string, unknown>;
  if (source.summary !== null && typeof source.summary !== "string") return undefined;
  if (!Array.isArray(source.checks)) return undefined;

  // Keep room for check names and dispositions even when a legacy result has
  // an unbounded summary. Drop checks from the tail when the fixed projection
  // budget is reached, matching the runtime's result contract.
  const summary = source.summary === null ? null : clipJsonString(source.summary, 4 * 1024);
  const checks: Array<Record<string, string>> = [];
  let encodedBytes = Buffer.byteLength(JSON.stringify({ summary, checks }), "utf8");
  const optionalLimits = {
    blocker: 1024,
    closedBy: 512,
    claimedOutcome: 256,
    evidenceGap: 1024,
  } as const;
  const count = Math.min(source.checks.length, MAX_BOUNDED_COLLECTION_ITEMS);
  for (let index = 0; index < count; index += 1) {
    const candidate = source.checks[index];
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) return undefined;
    const checkSource = candidate as Record<string, unknown>;
    if (typeof checkSource.name !== "string" || typeof checkSource.outcome !== "string") return undefined;
    const check: Record<string, string> = {
      name: clipJsonString(checkSource.name, 512),
      outcome: clipJsonString(checkSource.outcome, 256),
    };
    for (const [key, limit] of Object.entries(optionalLimits)) {
      const field = checkSource[key];
      if (field === undefined) continue;
      if (typeof field !== "string") return undefined;
      check[key] = clipJsonString(field, limit);
    }
    const checkBytes = Buffer.byteLength(JSON.stringify(check), "utf8");
    const addedBytes = checkBytes + (checks.length === 0 ? 0 : 1);
    if (encodedBytes + addedBytes > TIMELINE_RESULT_MAX_BYTES) break;
    checks.push(check);
    encodedBytes += addedBytes;
  }
  return parseWorkerResult({ summary, checks });
}

function boundedTimelineEnvelope(event: TimelineItemEvent): Pick<TimelineItemEvent, "type" | "sessionId" | "timestamp"> {
  return {
    type: "timeline.item",
    sessionId: clipJsonString(event.sessionId, TIMELINE_SESSION_MAX_BYTES),
    ...(event.timestamp === undefined
      ? {}
      : { timestamp: clipJsonString(event.timestamp, TIMELINE_TIMESTAMP_MAX_BYTES) }),
  };
}

function boundedToolError(item: ToolCallTimelineItem): BoundedJson | null {
  if (item.status !== "failed") return null;
  return boundJsonValue(item.error, TIMELINE_ERROR_MAX_BYTES) ?? "Tool call failed";
}

export function presentTimeline(event: ProviderEvent): ProviderEvent {
  const repaired = restoreReplayUserMessage(repairFailedTimelineError(event));
  if (repaired.type !== "timeline.item" || repaired.item.type !== "tool_call") return repaired;

  const sourceItem = repaired.item;
  if (sourceItem.callId.startsWith("worker:")) {
    // Worker transcripts and raw runtime mirrors are unnecessary for both
    // the inline row and the Workers panel (the panel reads durable history).
    // Bound before validation so legacy runtimes and replay rows are safe too.
    const resultSource = sourceItem.metadata?.cortexWorkerResult
      ?? (sourceItem.detail.type === "unknown" ? sourceItem.detail.output : undefined);
    const result = boundedWorkerResult(resultSource);
    const name = clipJsonString(sourceItem.name, TIMELINE_LABEL_MAX_BYTES);
    const workerItem = {
      type: "tool_call" as const,
      id: clipJsonString(sourceItem.id, TIMELINE_ID_MAX_BYTES),
      callId: clipJsonString(sourceItem.callId, TIMELINE_ID_MAX_BYTES),
      name,
      status: sourceItem.status,
      error: boundedToolError(sourceItem),
      detail: { type: "plan" as const, text: name },
      ...(result === undefined ? {} : { metadata: { cortexWorkerResult: result } }),
    } as ToolCallTimelineItem;
    return { ...boundedTimelineEnvelope(repaired), item: workerItem };
  }

  // Bound the incoming detail and label before formatting so legacy or
  // replayed multi-megabyte tool payloads do not create a second large copy.
  const rawInputSource = sourceItem.metadata?.cortexRawInput
    ?? (sourceItem.detail.type === "unknown" ? sourceItem.detail.input : sourceItem.detail);
  const boundedRawInput = boundJsonValue(rawInputSource, TIMELINE_DETAIL_MAX_BYTES);
  const boundedSourceDetail = boundToolDetail(sourceItem.detail);
  const sourceRevertToken = sourceItem.revertToken === undefined
    ? undefined
    : boundJsonValue(sourceItem.revertToken, 1024);
  const boundedSourceItem = {
    ...sourceItem,
    id: clipJsonString(sourceItem.id, TIMELINE_ID_MAX_BYTES),
    ...(sourceRevertToken === undefined ? {} : { revertToken: sourceRevertToken }),
    callId: clipJsonString(sourceItem.callId, TIMELINE_ID_MAX_BYTES),
    name: clipJsonString(sourceItem.name, TIMELINE_LABEL_MAX_BYTES),
    error: boundedToolError(sourceItem),
    detail: boundedSourceDetail as unknown as ProviderToolCallDetail,
    ...(boundedRawInput === undefined ? { metadata: undefined } : { metadata: { cortexRawInput: boundedRawInput } }),
  } as ToolCallTimelineItem;
  const projected = nativeToolDetail({ ...boundedTimelineEnvelope(repaired), item: boundedSourceItem });
  if (projected.type !== "timeline.item" || projected.item.type !== "tool_call") return projected;
  const item = projected.item;
  const rawInput = item.metadata?.cortexRawInput
    ?? (item.detail.type === "unknown" ? item.detail.input : item.detail);
  const semantics = presentToolSemantics({ name: item.name, rawInput });
  const boundedDetail = boundToolDetail(item.detail);
  const revertToken = item.revertToken === undefined
    ? undefined
    : boundJsonValue(item.revertToken, 1024);
  const toolItem = {
    type: "tool_call" as const,
    id: clipJsonString(item.id, TIMELINE_ID_MAX_BYTES),
    ...(revertToken === undefined ? {} : { revertToken }),
    callId: clipJsonString(item.callId, TIMELINE_ID_MAX_BYTES),
    name: clipJsonString(semantics.label, TIMELINE_LABEL_MAX_BYTES),
    status: item.status,
    error: boundedToolError(item),
    detail: boundedDetail as unknown as ProviderToolCallDetail,
  } as ToolCallTimelineItem;
  return { ...boundedTimelineEnvelope(projected), item: toolItem };
}

function nonEmptyArgv(parts: string[]): CommandArgv {
  if (parts.length === 0 || parts.some((part) => part.length === 0)) {
    throw new Error("Cortex ACP launch command must be a non-empty argv array");
  }
  return parts as CommandArgv;
}

function shimmedCommand(shim: string, targetArgv: string[], targetEnv?: Record<string, string>): CommandArgv {
  if (!shim.startsWith("/") || !existsSync(shim)) {
    throw new Error(`Cortex ACP Paseo load shim is missing: ${shim}`);
  }
  return nonEmptyArgv([
    "env",
    "CORTEX_PASEO_QUESTION_UI=1",
    `CORTEX_ACP_TARGET=${JSON.stringify({ argv: targetArgv, env: targetEnv })}`,
    process.execPath,
    shim,
  ]);
}

function bundledShimPath(): string {
  const explicit = process.env.CORTEX_PASEO_SHIM?.trim();
  if (explicit) return explicit;
  const configHomes = [process.env.XDG_CONFIG_HOME].filter(
    (value): value is string => Boolean(value),
  );
  for (const configHome of configHomes) {
    const launchConfig = join(configHome, "cortex", "paseo-plugin-launch.json");
    if (!existsSync(launchConfig)) continue;
    try {
      const configured = JSON.parse(readFileSync(launchConfig, "utf8")) as LaunchConfig;
      if (typeof configured.shim === "string") return configured.shim;
    } catch {
      throw new Error(`Invalid Cortex Paseo launch configuration: ${launchConfig}`);
    }
  }
  const source = sourceTreeLaunch();
  if (source) return source.shim;
  const homeLaunch = join(homedir(), ".config", "cortex", "paseo-plugin-launch.json");
  if (existsSync(homeLaunch)) {
    try {
      const configured = JSON.parse(readFileSync(homeLaunch, "utf8")) as LaunchConfig;
      if (typeof configured.shim === "string") return configured.shim;
    } catch {
      throw new Error(`Invalid Cortex Paseo launch configuration: ${homeLaunch}`);
    }
  }
  throw new Error("Cortex ACP Paseo load shim is unavailable; configure a launch record or CORTEX_PASEO_SOURCE_ROOT");
}

function sourceTreeLaunch(): { entry: string; shim: string } | undefined {
  const root = process.env.CORTEX_PASEO_SOURCE_ROOT?.trim();
  if (!root) return undefined;
  if (!isAbsolute(root)) {
    throw new Error("CORTEX_PASEO_SOURCE_ROOT must be an absolute path");
  }
  const entry = resolve(root, "src", "cortex_acp.py");
  const shim = resolve(root, "integrations", "paseo", "server", "acp-load-shim.mjs");
  if (!existsSync(entry) || !existsSync(shim)) {
    throw new Error(`CORTEX_PASEO_SOURCE_ROOT has no Cortex entry point or Paseo shim: ${root}`);
  }
  return { entry, shim };
}

function cortexCommand(): CommandArgv {
  const command = process.env.CORTEX_ACP_COMMAND?.trim();
  // An explicit command is useful for packaged deployments. The installer
  // does not need to inject it: the source tree contains a deterministic
  // Python entry point next to this plugin.
  const parseArgv = (raw: string): string[] | undefined => {
    const parsed: unknown = JSON.parse(raw);
    if (
      Array.isArray(parsed) &&
      parsed.length > 0 &&
      parsed.every((part): part is string => typeof part === "string" && part.length > 0)
    ) {
      return parsed;
    }
    return undefined;
  };

  // The guarded dev wrapper supplies this only for a directory plugin in an
  // isolated dev daemon. It is an explicit request to run this checkout, so
  // it must win over any inherited launch record or command from another
  // Cortex installation. Production launches leave it unset.
  const source = sourceTreeLaunch();
  if (source) return shimmedCommand(source.shim, ["python3", "-B", source.entry]);

  if (command?.startsWith("[")) {
    const parsed = parseArgv(command);
    if (
      parsed !== undefined
    ) {
      return shimmedCommand(bundledShimPath(), parsed);
    }
    throw new Error("CORTEX_ACP_COMMAND JSON must be a non-empty string argv array");
  }
  if (command) {
    return shimmedCommand(bundledShimPath(), [command]);
  }

  // Prefer a launch record in this XDG profile. Production profiles can still
  // use the normal HOME/.config record when no XDG location is supplied.
  const isolatedConfigHome = process.env.XDG_CONFIG_HOME;
  const regularConfigHome = join(homedir(), ".config");
  const configHomes = [
    isolatedConfigHome,
    regularConfigHome,
  ].filter((value, index, values): value is string =>
    Boolean(value) && values.indexOf(value) === index,
  );
  for (const configHome of configHomes) {
    const launchConfig = join(configHome, "cortex", "paseo-plugin-launch.json");
    if (!existsSync(launchConfig)) continue;
    const configured: unknown = JSON.parse(readFileSync(launchConfig, "utf8"));
    if (
      configured &&
      typeof configured === "object" &&
      "argv" in configured &&
      Array.isArray(configured.argv) &&
      configured.argv.length > 0 &&
      configured.argv.every(
        (part): part is string => typeof part === "string" && part.length > 0,
      ) &&
      (!("shim" in configured) ||
        (typeof configured.shim === "string" && configured.shim.startsWith("/") && existsSync(configured.shim))) &&
      (!("env" in configured) || (
        typeof configured.env === "object" &&
        configured.env !== null &&
        !Array.isArray(configured.env) &&
        Object.entries(configured.env).every(
          ([key, value]) =>
            /^(HOME|XDG_CONFIG_HOME|XDG_STATE_HOME)$/.test(key) &&
            typeof value === "string" &&
            value.length > 0 &&
            value.startsWith("/"),
        )
      ))
    ) {
      const launch = configured as LaunchConfig;
      const targetEnv = launch.env
        ? launch.env
        : undefined;
      if (!launch.shim) {
        throw new Error(`Cortex ACP launch configuration requires the Paseo load shim: ${launchConfig}`);
      }
      return shimmedCommand(launch.shim, launch.argv, targetEnv);
    }
    throw new Error(`Invalid Cortex Paseo launch configuration: ${launchConfig}`);
  }

  throw new Error("Cortex ACP launch unavailable; configure a Paseo launch record or CORTEX_PASEO_SOURCE_ROOT");
}

/**
 * Paseo's stock ACP bridge labels every `session/request_permission` as a
 * tool permission. Cortex marks only its ask_user requests with the stable
 * name/input marker below; this provider changes that one presentation field
 * to `question`. `send()` remains delegated to the stock ACP bridge, so the
 * selectedActionId is returned to the exact Cortex ACP session unchanged.
 */
/** Config id Cortex reads a card's typed "Other..." answer from. */
export const QUESTION_ANSWER_CONFIG_ID = "cortex_question_answer";
const ANSWER_REQUEST_PREFIX = "cortex-question-answer:";

/** The typed text of a card answer that picked none of its options. */
export function typedAnswer(
  answers: unknown,
  actions: ReadonlyArray<{ label: string }>,
): string | undefined {
  if (!answers || typeof answers !== "object" || Array.isArray(answers)) return undefined;
  const answer = Object.values(answers).find((value): value is string => typeof value === "string");
  if (answer === undefined || !answer.trim()) return undefined;
  return actions.some((action) => action.label === answer) ? undefined : answer;
}

/** Paseo offers prompt.steer and the stock ACP bridge lacks it. */
export function emulatesSteer(offered: readonly string[], supported: readonly string[]): boolean {
  return offered.includes("prompt.steer") && !supported.includes("prompt.steer");
}

/** Config id the ACP agent uses to durably queue a Paseo message sent
 * while a turn is already active. */
export const QUEUED_MESSAGE_CONFIG_ID = "cortex_queued_message";
const QUEUE_REQUEST_PREFIX = "cortex-queued-message:";

/** One ACP-shaped content block Cortex can durably queue and later resolve
 * (see ``acp.protocol.content_blocks_to_text`` /
 * ``acp.replay_store.resolve_prompt_blocks``, the same blocks accepted for
 * an ordinary new-turn ``session/prompt``). */
export type QueuedContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string }
  | { type: "resource_link"; name: string; uri: string; mimeType?: string };

/**
 * Content Cortex can durably queue while a turn is active: text, inline
 * images, and uploaded files (carried as a ``resource_link``). Any other
 * attachment kind (a forge/review/GitHub attachment, for instance) is
 * refused rather than silently dropped -- this bridge has no durable-queue
 * path for it yet.
 */
export function queuedContent(
  input: Parameters<ProviderConnection["send"]>[0],
): QueuedContentBlock[] | null {
  if (input.type !== "session.prompt" || input.prompt.delivery !== "steer") return null;
  if (input.prompt.input.type !== "message") return null;
  const content = input.prompt.input.content;
  if (!content.length) return null;
  const blocks: QueuedContentBlock[] = [];
  for (const part of content) {
    if (part.type === "text") {
      if (part.text.trim()) blocks.push({ type: "text", text: part.text });
    } else if (part.type === "image") {
      blocks.push({ type: "image", data: part.data, mimeType: part.mimeType });
    } else if (part.type === "uploaded_file") {
      blocks.push({
        type: "resource_link", name: part.fileName,
        uri: pathToFileURL(part.path).href, mimeType: part.mimeType,
      });
    } else {
      return null;
    }
  }
  return blocks.length ? blocks : null;
}

/** A short, human-facing echo of queued content for the synthetic timeline
 * item -- the provider's ``user_message`` shape carries only text. */
function queuedContentEcho(blocks: readonly QueuedContentBlock[]): string {
  return blocks.map((block) => {
    if (block.type === "text") return block.text;
    if (block.type === "image") return "[image attached]";
    return `[file attached: ${block.name}]`;
  }).join("\n");
}


/** The Cortex process a shimmed launch command starts (see shimmedCommand). */
function launchTarget(command: readonly string[]): LaunchConfig | undefined {
  const prefix = "CORTEX_ACP_TARGET=";
  const raw = command.find((part) => part.startsWith(prefix));
  return raw === undefined ? undefined : JSON.parse(raw.slice(prefix.length)) as LaunchConfig;
}

/** Each model's own efforts, from Cortex (CORTEX_MODEL_EFFORTS=1). */
function modelEfforts(command: readonly string[]): Promise<Record<string, string[]>> {
  const target = launchTarget(command);
  if (target === undefined) return Promise.resolve({});
  return new Promise((resolve, reject) => {
    execFile(target.argv[0], target.argv.slice(1), {
      env: { ...process.env, ...(target.env ?? {}), CORTEX_MODEL_EFFORTS: "1" },
      timeout: 15000,
    }, (error, stdout) => {
      if (error) reject(error);
      else resolve(JSON.parse(stdout) as Record<string, string[]>);
    });
  });
}

/**
 * Paseo's ACP bridge gives every model the efforts of whichever one was
 * selected when it built its catalog, so switching models showed the wrong
 * efforts, or none. Each model gets its own.
 */
export function withModelEfforts<T extends {
  models: ReadonlyArray<{ id: string; defaultThinkingOptionId?: string }>;
  defaultThinkingOption?: string;
}>(
  catalog: T, efforts: Record<string, string[]>,
): T {
  return {
    ...catalog,
    models: catalog.models.map((model) => ({
      ...model,
      thinkingOptions: (efforts[model.id] ?? []).map((value) => ({ id: value, label: value })),
      // The ACP bridge gives us the effective session choice separately as
      // `defaultThinkingOption`.  Once each model receives its own option
      // list, carry that choice onto the matching model or Paseo falls back
      // to the first option (usually "low") in its composer.
      ...(catalog.defaultThinkingOption && (efforts[model.id] ?? []).includes(catalog.defaultThinkingOption)
        ? { defaultThinkingOptionId: catalog.defaultThinkingOption }
        : {}),
    })),
  };
}

/**
 * Cortex's access modes (see configuration/access_mode.py): Paseo draws a
 * mode's own icon and colour in its composer mode control, so Guarded reads
 * as a safe shield and Auto as an open, dangerous one. Neither is marked
 * unattended: Paseo must never pick Auto on its own.
 */
const ACCESS_MODE_VISUALS: Record<string, { icon: string; colorTier: string }> = {
  guarded: { icon: "ShieldCheck", colorTier: "safe" },
  auto: { icon: "ShieldOff", colorTier: "dangerous" },
};

export function withAccessModeVisuals<T extends { modes: ReadonlyArray<{ id: string }> }>(state: T): T {
  return {
    ...state,
    modes: state.modes.map((mode) => ({ ...mode, ...(ACCESS_MODE_VISUALS[mode.id] ?? {}) })),
  };
}

export function createCortexProvider(): ProviderRegistration {
  const command = cortexCommand();
  const makeBase = (replay: ReplayController) => runAcpProvider({
    // Register the native provider under `cortex` for new chats. The setup
    // command removes only the exact owned generic registration; old generic
    // chats may therefore become inaccessible. The plugin manifest remains
    // `cortex-acp`.
    id: "cortex",
    label: "Cortex",
    description: "Cortex ACP orchestration",
    connector: replay.connect,
    // Cortex advertises its slash commands immediately after session/new.
    // Paseo otherwise emits session.ready and asks the composer for its
    // command list before that asynchronous notification arrives, caching an
    // empty list for the chat lifetime.  Wait for the initial update while
    // keeping startup bounded if an older/failed agent does not advertise it.
    acpOptions: {
      waitForInitialCommands: true,
      initialCommandsTimeoutMs: 2000,
    },
    // Paseo 0.8 requires a non-null timeline error for every failed tool
    // item, while ACP permits rawOutput to be omitted. Preserve the observed
    // failure status and title when an agent supplies no output; do not claim
    // an underlying cause that the ACP event did not report.
    transformers: [{
      async discover(catalog) {
        try {
          return withAccessModeVisuals(withModelEfforts(catalog, await modelEfforts(command)));
        } catch {
          // Cortex unavailable: the bridge's own catalog, only the mode visuals added
          return withAccessModeVisuals(catalog);
        }
      },
      toolCall(snapshot) {
        if (snapshot.status !== "failed" || snapshot.output !== null) {
          return snapshot;
        }
        return {
          ...snapshot,
          output: `Tool call failed: ${snapshot.title}`,
        };
      },
    }],
  });

  const base = makeBase(new ReplayController(command));
  return {
    ...base,
    async connect(request) {
      const replay = new ReplayController(command);
      return wrapCortexConnection(replay.wrap(await makeBase(replay).connect(request)), request.capabilities);
    },
  };
}

type ProviderConnection = Awaited<ReturnType<ProviderRegistration["connect"]>>;

/** Cortex presentation and question handling around the stock ACP bridge. */
export function wrapCortexConnection(
  connection: ProviderConnection,
  offered: readonly string[],
): ProviderConnection {
  // The installed ACP bridge lacks prompt.steer. Carry the message (text,
  // and any image/uploaded-file content) through its concurrent config
  // channel; Cortex durably queues it without cancelling the running turn
  // or any worker, and delivers it in order once that turn completes.
  const steer = emulatesSteer(offered, connection.capabilities);
  const capabilities = steer ? [...connection.capabilities, "prompt.steer"] : connection.capabilities;
  const questionActions = new Map<
    string,
    Array<{ id: string; label: string }>
  >();
  const questionIds = new Map<string, string>();
  // Our own configure requests: their completion is ours to await,
  // never Paseo's to see.
  const answerRequests = new Map<string, { resolve: () => void; reject: (error: Error) => void }>();
  const queueRequests = new Map<string, { resolve: () => void; reject: (error: Error) => void }>();
  const activeTurns = new Map<string, string>();
  const lastTurns = new Map<string, string>();
  connection.onEvent((event) => {
    if (event.type === "session.turn") {
      if (event.state === "started") {
        activeTurns.set(event.sessionId, event.turnId);
        lastTurns.set(event.sessionId, event.turnId);
      }
      else if (activeTurns.get(event.sessionId) === event.turnId) activeTurns.delete(event.sessionId);
    }
    if (event.type !== "request.completed" && event.type !== "request.failed") return;
    const pending = answerRequests.get(event.requestId) ?? queueRequests.get(event.requestId);
    if (!pending) return;
    answerRequests.delete(event.requestId);
    queueRequests.delete(event.requestId);
    if (event.type === "request.completed") pending.resolve();
    else pending.reject(new Error(event.error.message));
  });
  /**
   * Paseo's bridge answers a card only with one of its options. A typed
   * "Other..." answer therefore goes to Cortex as a config value (the
   * bridge forwards any config id), and only once Cortex has it is the
   * card retired as declined -- Cortex ignores a decline that arrives
   * after the answer.
   */
  async function sendTypedAnswer(
    input: Extract<Parameters<typeof connection.send>[0], { type: "session.permission" }>,
    questionId: string,
    answer: string,
  ) {
    const requestId = `${ANSWER_REQUEST_PREFIX}${randomUUID()}`;
    const delivered = new Promise<void>((resolve, reject) => answerRequests.set(requestId, { resolve, reject }));
    await connection.send({
      type: "session.configure",
      requestId,
      sessionId: input.sessionId,
      changes: { settings: { [QUESTION_ANSWER_CONFIG_ID]: JSON.stringify({ questionId, answer }) } },
    });
    await delivered;
    listeners.forEach((listener) => listener({
      type: "timeline.item",
      sessionId: input.sessionId,
      item: { type: "user_message", id: `cortex-question-answer:${questionId}`, text: answer },
    } as ProviderEvent));
    await connection.send({ ...input, response: { behavior: "deny" } });
  }
  async function sendQueuedMessage(
    input: Extract<Parameters<typeof connection.send>[0], { type: "session.prompt" }>,
  ) {
    const content = queuedContent(input);
    // Paseo accepts only a matching `steer` result without replacing the
    // running turn. A refused capture must fail the send rather than claim
    // delivery. Paseo displays that failure itself, so do not also emit a
    // notice: it would render the same error twice.
    const refuse = (description: string): never => {
      throw new Error(`Message was not queued. ${description}`);
    };
    if (content === null) {
      refuse("This content cannot be queued during an active turn. Nothing was sent; current work is continuing.");
    }
    const requestId = `${QUEUE_REQUEST_PREFIX}${randomUUID()}`;
    const delivered = new Promise<void>((resolve, reject) => {
      queueRequests.set(requestId, { resolve, reject });
    });
    try {
      await connection.send({
        type: "session.configure", requestId, sessionId: input.sessionId,
        changes: { settings: { [QUEUED_MESSAGE_CONFIG_ID]: JSON.stringify({
          clientMessageId: input.prompt.clientMessageId, content,
        }) } },
      });
      await delivered;
    } catch (error) {
      queueRequests.delete(requestId);
      const reason = error instanceof Error ? error.message.trim().slice(0, 300) : "";
      refuse(`Cortex did not confirm durable capture${reason ? ` (${reason})` : ""}. `
        + "Nothing was sent; current work is continuing. Send the message again.");
    }
    listeners.forEach((listener) => listener({
      type: "timeline.item", sessionId: input.sessionId,
      item: { type: "user_message", id: input.prompt.clientMessageId,
        clientMessageId: input.prompt.clientMessageId, text: queuedContentEcho(content!) },
    }));
    // A turn that ended while Cortex captured the steer is still the one
    // Paseo expects; Cortex processes the captured text in a background
    // continuation, so the steer is accepted for that turn.
    const turnId = activeTurns.get(input.sessionId) ?? lastTurns.get(input.sessionId);
    listeners.forEach((listener) => listener({
      type: "session.prompt_result", sessionId: input.sessionId,
      clientMessageId: input.prompt.clientMessageId,
      result: turnId ? { type: "steer", turnId } : { type: "completed" },
    }));
  }
  const openedSessions = new Set<string>();
  const bufferedTimeline = new Map<string, ReplaySpool>();
  const listeners = new Set<(event: ProviderEvent) => void>();
  let unsubscribe: (() => void) | undefined;
  return {
    ...connection,
    capabilities,
    send(input: Parameters<typeof connection.send>[0]) {
      if (steer && input.type === "session.prompt" && input.prompt.delivery === "steer") {
        return sendQueuedMessage(input) as ReturnType<typeof connection.send>;
      }
      if (
        input.type === "session.permission" &&
        input.response.behavior === "allow" &&
        input.response.selectedActionId === undefined &&
        input.response.updatedInput !== undefined
      ) {
        const answers = input.response.updatedInput.answers;
        const answer =
          answers &&
          typeof answers === "object" &&
          !Array.isArray(answers)
            ? Object.values(answers).find(
                (value): value is string => typeof value === "string",
              )
            : undefined;
        const actions = questionActions.get(input.permissionId);
        const selected = actions?.find((action) => action.label === answer);
        const typed = actions ? typedAnswer(answers, actions) : undefined;
        const questionId = questionIds.get(input.permissionId);
        if (typed !== undefined && questionId !== undefined) {
          const permissionId = input.permissionId;
          // A refused answer keeps the card and its options for a retry.
          return sendTypedAnswer(input, questionId, typed).then(() => {
            questionActions.delete(permissionId);
            questionIds.delete(permissionId);
          }) as ReturnType<typeof connection.send>;
        }
        if (actions && !selected) {
          throw new Error(
            `Cortex question response did not match a displayed option for ${input.permissionId}`,
          );
        }
        if (selected) {
          input = {
            ...input,
            response: {
              behavior: "allow",
              selectedActionId: selected.id,
            },
          };
        }
      }
      const result = connection.send(input);
      if (input.type === "session.permission") {
        void result.finally(() => {
          questionActions.delete(input.permissionId);
          questionIds.delete(input.permissionId);
        });
      }
      return result;
    },
    close() {
      questionActions.clear();
      questionIds.clear();
      answerRequests.clear();
      queueRequests.clear();
      activeTurns.clear();
      lastTurns.clear();
      openedSessions.clear();
      for (const spool of bufferedTimeline.values()) spool.close();
      bufferedTimeline.clear();
      listeners.clear();
      unsubscribe?.();
      return connection.close();
    },
    onEvent(subscriber) {
      listeners.add(subscriber);
      const listener = (event: ProviderEvent) => listeners.forEach(callback => callback(event));
      unsubscribe ??= connection.onEvent((event) => {
        if (!visibleTimeline(event)) return;
        if ((event.type === "request.completed" || event.type === "request.failed")
          && (event.requestId.startsWith(ANSWER_REQUEST_PREFIX) || event.requestId.startsWith(QUEUE_REQUEST_PREFIX))) return;
        if (event.type === "session.opened") {
          openedSessions.add(event.sessionId);
          listener(steer && !event.capabilities.includes("prompt.steer")
            ? { ...event, capabilities: [...event.capabilities, "prompt.steer"] }
            : event);
          const pending = bufferedTimeline.get(event.sessionId);
          bufferedTimeline.delete(event.sessionId);
          try {
            for (const item of pending?.frames() ?? []) {
              const historical = item as ProviderEvent;
              if (visibleTimeline(historical)) listener(presentTimeline(historical));
            }
          } finally { pending?.close(); }
          return;
        }
        if (event.type === "timeline.item" && !openedSessions.has(event.sessionId)) {
          const pending = bufferedTimeline.get(event.sessionId) ?? new ReplaySpool();
          try { pending.append(event); }
          catch {
            pending.close();
            bufferedTimeline.delete(event.sessionId);
            listener({ type: "session.runtime_failed", sessionId: event.sessionId,
              error: { message: "Incomplete history: provider replay staging failed" } });
            return;
          }
          bufferedTimeline.set(event.sessionId, pending);
          return;
        }
        if (event.type === "session.closed" || event.type === "session.runtime_failed") {
          openedSessions.delete(event.sessionId);
          bufferedTimeline.get(event.sessionId)?.close();
          bufferedTimeline.delete(event.sessionId);
          listener(event);
          return;
        }
        if (
          event.type === "session.permission" &&
          isCortexQuestionRequest(event.request)
        ) {
          const actions = event.request.actions ?? [];
          // Paseo sends the visible answer string back in
          // updatedInput.answers. Prefixing the ordinal makes that value
          // unique even when two source options share a label.
          const displayActions = actions.map((action, index) => ({
            id: action.id,
            label: `${index + 1}. ${action.label}`,
          }));
          questionActions.set(event.request.id, displayActions);
          if (typeof event.request.input?.questionId === "string") {
            questionIds.set(event.request.id, event.request.input.questionId);
          }
          const questionText =
            typeof event.request.input?.question === "string"
              ? event.request.input.question
              : event.request.title ?? "Choose an option";
          listener({
            ...event,
            request: {
              ...event.request,
              kind: "question",
              // The marker is a wire-only discriminator. Do not expose
              // it in Paseo's question card input/details.
              input: {
                questions: [
                  {
                    question: questionText,
                    header: "Cortex",
                    options: displayActions.map((action) => ({ label: action.label })),
                    multiSelect: false,
                    // Paseo 0.9 shows an "Other..." field next to the
                    // options; its text reaches Cortex via sendTypedAnswer.
                    ...(questionIds.has(event.request.id) ? { allowOther: true } : {}),
                  },
                ],
              },
              metadata: Object.fromEntries(
                Object.entries(event.request.metadata ?? {}).filter(
                  ([key]) =>
                    key !== CORTEX_QUESTION_INPUT_MARKER &&
                    key !== "questionId" &&
                    key !== "optionPrefix",
                ),
              ),
            },
          });
          return;
        }
        if (event.type === "session.config") {
          listener({ ...event, config: withAccessModeVisuals(event.config) });
          return;
        }
        listener(presentTimeline(event));
      });
      return () => {
        listeners.delete(subscriber);
      };
    },
  };
}
