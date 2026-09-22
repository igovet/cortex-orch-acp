import { isHiddenOperation } from "../shared/timeline-visibility.ts";
import { presentToolSemantics } from "../shared/tool-presentation.ts";
import { gatewayPresentation } from "../shared/gateway-presentation.ts";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
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

type CommandArgv = [string, ...string[]];

type LaunchConfig = {
  argv: string[];
  env?: Record<string, string>;
  shim?: string;
};

const MAX_BUFFERED_TIMELINE_ITEMS = 4096;
const MAX_BUFFERED_TIMELINE_BYTES = 16 * 1024 * 1024;
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
    REPLAY_USER_MESSAGE_ID.test(event.item.messageId)
  ) {
    const { messageId: _messageId, ...item } = event.item;
    return { ...event, item: { ...item, type: "user_message" } };
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
  const command = firstString(input, ["command", "cmd"]);
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

export function presentTimeline(event: ProviderEvent): ProviderEvent {
  const projected = nativeToolDetail(restoreReplayUserMessage(repairFailedTimelineError(event)));
  if (projected.type !== "timeline.item" || projected.item.type !== "tool_call") return projected;
  const item = projected.item;
  // Paseo's overview mode never groups a `plan` detail. A worker card must
  // stay its own row: grouped, it would become the run's visible latest call
  // and the client's worker-row transform would hide every call before it.
  if (item.callId.startsWith("worker:")) {
    return { ...projected, item: { ...item, detail: { type: "plan", text: item.name } } };
  }
  const rawInput = item.metadata?.cortexRawInput ?? (item.detail.type === "unknown" ? item.detail.input : item.detail);
  const semantics = presentToolSemantics({ name: item.name, rawInput });
  return { ...projected, item: { ...item, name: semantics.label,
    metadata: { ...item.metadata, cortexOriginalName: item.name } } };
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
  const configHomes = [
    process.env.XDG_CONFIG_HOME,
    join(homedir(), ".config"),
  ].filter((value, index, values): value is string =>
    Boolean(value) && values.indexOf(value) === index,
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
  // This fallback is for source-tree development only. Packaged daemon
  // launches must use the owner-private launch record above.
  try {
    return resolve(dirname(fileURLToPath(import.meta.url)), "acp-load-shim.mjs");
  } catch {
    throw new Error("Cortex ACP Paseo load shim path is unavailable");
  }
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

  // A desktop daemon can restart with a different XDG_CONFIG_HOME from the
  // shell that ran setup. Keep the normal location first, then try the
  // user's conventional HOME/.config location so the durable setup record
  // remains discoverable across that restart.
  const configHomes = [
    process.env.XDG_CONFIG_HOME,
    join(homedir(), ".config"),
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

  // Development fallback. Packaged Paseo bundles may not preserve this URL;
  // production setup writes the owner-private launch config above.
  const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
  const entry = resolve(repositoryRoot, "src", "cortex_acp.py");
  if (!existsSync(entry)) {
    throw new Error(`Cortex ACP entry point is missing: ${entry}`);
  }
  const shim = bundledShimPath();
  return shimmedCommand(shim, ["python3", "-B", entry]);
}

/**
 * Paseo's stock ACP bridge labels every `session/request_permission` as a
 * tool permission. Cortex marks only its ask_user requests with the stable
 * name/input marker below; this provider changes that one presentation field
 * to `question`. `send()` remains delegated to the stock ACP bridge, so the
 * selectedActionId is returned to the exact Cortex ACP session unchanged.
 */
/** Paseo offers prompt.steer and the stock ACP bridge lacks it. */
export function emulatesSteer(offered: readonly string[], supported: readonly string[]): boolean {
  return offered.includes("prompt.steer") && !supported.includes("prompt.steer");
}

/** A steer prompt delivered as the bridge's follow-up (interrupt, then send). */
export function steerAsFollowUp<T extends { type: string }>(input: T): T {
  const prompt = (input as { prompt?: { delivery?: string } }).prompt;
  if (input.type !== "session.prompt" || prompt?.delivery !== "steer") return input;
  return { ...input, prompt: { ...prompt, delivery: "auto" } } as T;
}

export function createCortexProvider(): ProviderRegistration {
  const base = runAcpProvider({
    // Register the native provider under `cortex` for new chats. The setup
    // command removes only the exact owned generic registration; old generic
    // chats may therefore become inaccessible. The plugin manifest remains
    // `cortex-acp`.
    id: "cortex",
    label: "Cortex",
    description: "Cortex ACP orchestration",
    command: cortexCommand(),
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

  return {
    ...base,
    async connect(request) {
      const connection = await base.connect(request);
      // Paseo sends a message typed during a running turn as a "steer"
      // prompt, which the stock ACP bridge does not support ("Provider does
      // not support prompt.steer"). Cortex takes it the way the bridge
      // delivers any follow-up: the running turn is interrupted and the
      // message steers the same task.
      const steer = emulatesSteer(request.capabilities, connection.capabilities);
      const capabilities = steer ? [...connection.capabilities, "prompt.steer"] : connection.capabilities;
      const questionActions = new Map<
        string,
        Array<{ id: string; label: string }>
      >();
      const openedSessions = new Set<string>();
      const bufferedTimeline = new Map<string, ProviderEvent[]>();
      const bufferedTimelineBytes = new Map<string, number>();
      return {
        ...connection,
        capabilities,
        send(input: Parameters<typeof connection.send>[0]) {
          if (steer) input = steerAsFollowUp(input);
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
            void result.finally(() => questionActions.delete(input.permissionId));
          }
          return result;
        },
        close() {
          questionActions.clear();
          openedSessions.clear();
          bufferedTimeline.clear();
          bufferedTimelineBytes.clear();
          return connection.close();
        },
        onEvent(listener) {
          return connection.onEvent((event) => {
            if (!visibleTimeline(event)) return;
            if (event.type === "session.opened") {
              openedSessions.add(event.sessionId);
              listener(steer && !event.capabilities.includes("prompt.steer")
                ? { ...event, capabilities: [...event.capabilities, "prompt.steer"] }
                : event);
              const pending = bufferedTimeline.get(event.sessionId);
              bufferedTimeline.delete(event.sessionId);
              bufferedTimelineBytes.delete(event.sessionId);
              for (const historical of pending ?? []) {
                if (visibleTimeline(historical)) listener(presentTimeline(historical));
              }
              return;
            }
            if (event.type === "timeline.item" && !openedSessions.has(event.sessionId)) {
              const pending = bufferedTimeline.get(event.sessionId) ?? [];
              const eventBytes = JSON.stringify(event).length;
              const pendingBytes = bufferedTimelineBytes.get(event.sessionId) ?? 0;
              if (
                pending.length >= MAX_BUFFERED_TIMELINE_ITEMS ||
                pendingBytes + eventBytes > MAX_BUFFERED_TIMELINE_BYTES
              ) {
                throw new Error(
                  `Paseo timeline history buffer exceeded limits for ${event.sessionId}`,
                );
              }
              pending.push(event);
              bufferedTimeline.set(event.sessionId, pending);
              bufferedTimelineBytes.set(event.sessionId, pendingBytes + eventBytes);
              return;
            }
            if (event.type === "session.closed" || event.type === "session.runtime_failed") {
              openedSessions.delete(event.sessionId);
              bufferedTimeline.delete(event.sessionId);
              bufferedTimelineBytes.delete(event.sessionId);
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
            listener(presentTimeline(event));
          });
        },
      };
    },
  };
}
