import type { EngineKey, McpObservation, McpServer, McpTool, Problem } from "./types";
import { engineLabel, ROLE_CATALOG } from "./catalog";

export type ToolLevel = "off" | "writing" | "read_only" | "coordinator";
export type GrantColumn = "coordinator" | "read_only" | "writing" | "always";
export type ToolRow = { name: string; description: string | null; hint: McpTool["hint"]; level: ToolLevel; always: boolean; state: "granted" | "new" | "not_offered"; checked: boolean };
export type ConnectOutcome = "command_not_found" | "exited" | "timeout" | "path_refused" | "needs_variable" | "not_supported";
export type ConnectResult = { ok: true; observation: McpObservation } | { ok: false; code: ConnectOutcome | "offline"; message: string; field?: string; manual?: boolean };

const copy = (s: McpServer): McpServer => structuredClone(s);
const drop = (list: string[], tools: string[]) => list.filter(x => !tools.includes(x));
const add = (list: string[], tools: string[]) => [...list, ...tools.filter(t => !list.includes(t))];
/** With no list of its own the coordinator follows read-only. */
export const coordinatorList = (s: McpServer) => s.coordinatorTools ?? s.readOnlyTools;

/**
 * Each tool sits on one rung: coordinator ⊆ read-only ⊆ writing. A tick lifts a tool up to that rung and an untick drops
 * it below, so the lists stay subsets of each other whatever the owner does and a save can never fail that rule.
 */
export function toolLevel(s: McpServer, tool: string): ToolLevel {
  if (!s.allowedTools.includes(tool)) return "off";
  if (!s.readOnlyTools.includes(tool)) return "writing";
  return coordinatorList(s).includes(tool) ? "coordinator" : "read_only";
}
export function setLevel(s: McpServer, tools: string[], level: ToolLevel): McpServer {
  const next = copy(s);
  if (level === "off") {
    next.allowedTools = drop(next.allowedTools, tools);
    if (next.alwaysLoadTools) next.alwaysLoadTools = drop(next.alwaysLoadTools, tools);
  } else next.allowedTools = add(next.allowedTools, tools);
  next.readOnlyTools = level === "off" || level === "writing" ? drop(next.readOnlyTools, tools) : add(next.readOnlyTools, tools);
  if (next.coordinatorTools) next.coordinatorTools = level === "coordinator" ? add(next.coordinatorTools, tools) : drop(next.coordinatorTools, tools);
  return next;
}
const CASCADE: Record<string, string> = {
  "coordinator:writing": "Also given to read-only and writing workers.", "coordinator:off": "Also given to read-only and writing workers.", "coordinator:read_only": "",
  "read_only:off": "Also given to writing workers.",
};
export function setGrant(s: McpServer, tool: string, column: GrantColumn, on: boolean): { server: McpServer; note: string | null } {
  const was = toolLevel(s, tool);
  if (column === "always") {
    if (on && was === "off") return { server: s, note: null };
    const next = copy(s);
    next.alwaysLoadTools = on ? add(next.alwaysLoadTools ?? [], [tool]) : drop(next.alwaysLoadTools ?? [], [tool]);
    return { server: next, note: null };
  }
  if (column === "coordinator" && !s.coordinatorTools) return { server: s, note: null };
  if (on) {
    const rungs: ToolLevel[] = ["off", "writing", "read_only", "coordinator"];
    if (rungs.indexOf(was) >= rungs.indexOf(column)) return { server: s, note: null };
    return { server: setLevel(s, [tool], column), note: CASCADE[`${column}:${was}`] || null };
  }
  const to: ToolLevel = column === "coordinator" ? "read_only" : column === "read_only" ? "writing" : "off";
  if (column === "coordinator" && was !== "coordinator") return { server: s, note: null };
  if (column === "read_only" && was !== "read_only" && was !== "coordinator") return { server: s, note: null };
  if (column === "writing" && was === "off") return { server: s, note: null };
  const note = column === "read_only" && was === "coordinator" ? "Also removed from the coordinator."
    : column === "writing" && was === "coordinator" ? "Also removed from read-only workers and the coordinator."
      : column === "writing" && was === "read_only" ? "Also removed from read-only workers." : null;
  return { server: setLevel(s, [tool], to), note };
}
export function setCoordinatorMode(s: McpServer, mode: "same" | "choose"): McpServer {
  const next = copy(s);
  next.coordinatorTools = mode === "same" ? null : [...(s.coordinatorTools ?? s.readOnlyTools)];
  return next;
}

/** What the lists grant a role's class, whether or not the server would run. The consultant has no list. */
export function grantsFor(s: McpServer, role: string): string[] {
  if (role === "coordinator") return coordinatorList(s);
  const access = ROLE_CATALOG.find(r => r.id === role)?.access;
  return access === "consultant" ? [] : access === "read_only" ? s.readOnlyTools : s.allowedTools;
}
/** Why a session of this role on this engine never sees the server, whatever its lists grant. */
function accessBlock(s: McpServer, role: string, engine: EngineKey): string | null {
  if (!s.enabled) return "Server is off.";
  if (s.problem) return s.problem;
  if ([...s.readPaths, ...s.writePaths].some(pathMissing)) return "A path does not exist.";
  if (s.engines && !s.engines.includes(engine)) return `Not offered on ${engineLabel(engine)}.`;
  if (role === "coordinator") return s.roles && !s.roles.includes("coordinator") ? "For workers only." : null;
  if (ROLE_CATALOG.find(r => r.id === role)?.access === "consultant") return "The consultant gets no external servers.";
  return s.roles && !s.roles.includes("worker") ? "For the coordinator only." : null;
}
export type Access = { tools: string[]; granted: string[]; reason: string | null };
/** What one role really gets on one engine, beside what its lists grant. The runtime skips a disabled server, so its grants give nothing. */
export function accessFor(s: McpServer, role: string, engine: EngineKey): Access {
  const granted = grantsFor(s, role), reason = accessBlock(s, role, engine);
  return { tools: reason ? [] : granted, granted, reason };
}

/** Granted tools first, in their stored order, then whatever the server offers that nobody granted. */
export function mcpTools(s: McpServer, seen: McpObservation | null): ToolRow[] {
  const listed = new Map(seen?.tools.map(t => [t.name, t]) ?? []);
  const row = (name: string): ToolRow => {
    const tool = listed.get(name), hand = s.handTools.includes(name), level = toolLevel(s, name);
    const state = level === "off" ? "new" : seen && !tool && !hand ? "not_offered" : "granted";
    return { name, description: tool?.description ?? null, hint: tool?.hint ?? null, level, always: !!s.alwaysLoadTools?.includes(name), state, checked: !!tool };
  };
  return [...s.allowedTools.map(row), ...[...listed.keys()].filter(n => !s.allowedTools.includes(n)).map(row)];
}
/** A hint is the server's own claim, so this only fills the draft for the owner to read. */
export function suggestLevels(s: McpServer, rows: ToolRow[]): McpServer {
  let next = s;
  for (const r of rows.filter(x => x.checked)) next = setLevel(next, [r.name], r.hint === "read" ? "read_only" : r.hint === "destructive" ? "off" : "writing");
  return next;
}
export const toolsDiff = (rows: ToolRow[]) => ({ fresh: rows.filter(r => r.state === "new").length, gone: rows.filter(r => r.state === "not_offered").length });

const INDEX_NEEDS = ["list_projects", "search_graph", "trace_path", "get_code_snippet"];
export const isCodeIndex = (s: McpServer) => /codebase-memory/.test(`${s.name} ${s.command}`);
export type ToolWarning = { tool: string | null; text: string };
export function toolWarnings(s: McpServer, rows: ToolRow[]): ToolWarning[] {
  const out: ToolWarning[] = [];
  for (const r of rows) if (r.hint === "destructive" && (r.level === "read_only" || r.level === "coordinator")) out.push({ tool: r.name, text: "The server says this tool changes things. Read-only roles would get it." });
  if (s.allowedTools.length && !s.readOnlyTools.length) out.push({ tool: null, text: "Reviewers, auditors and other read-only roles get nothing from this server." });
  if (isCodeIndex(s) && INDEX_NEEDS.some(t => !s.readOnlyTools.includes(t))) out.push({ tool: null, text: "All four are needed to use this as the code index." });
  if (s.coordinatorTools?.includes("index_repository")) out.push({ tool: "index_repository", text: "Keep indexing for workers." });
  if ((s.alwaysLoadTools?.length ?? 0) > 12) out.push({ tool: null, text: "Each one costs prompt space in every Claude session." });
  if (s.roles && !s.roles.includes("coordinator") && s.coordinatorTools?.length) out.push({ tool: null, text: "These coordinator tools have no effect." });
  return out;
}

const NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/, ENV = /^[A-Z_][A-Z0-9_]*$/;
const RESERVED_ENV = ["PATH", "HOME", "TMPDIR", "XDG_RUNTIME_DIR"];
export const pathProblem = (path: string) => !path.startsWith("/") ? "Use a full path, starting with /." : path.split("/").includes("..") ? "A path cannot go up with \"..\"." : null;
/** Nothing here can look at a disk, so a missing path is known only by the marker in its name. */
export const pathMissing = (path: string) => path.includes("/missing/");
export function mcpProblems(s: McpServer, others: McpServer[], isNew: boolean): Problem[] {
  const out: Problem[] = [], put = (path: string, message: string) => out.push({ path, message });
  if (isNew) {
    // "new" is the address of the add flow, so a server cannot have it.
    if (s.name === "cortex" || s.name === "new") put("name", "Reserved");
    else if (!NAME.test(s.name)) put("name", "Use letters, digits, dot, dash or underscore, up to 64.");
    else if (others.some(o => o.name === s.name)) put("name", "Already exists");
  }
  if (!s.command.trim() || s.command.length > 4096) put("command", "Enter the program to start.");
  if (s.args.length > 256) put("args", "Up to 256 arguments.");
  if (!s.allowedTools.length) put("allowed_tools", "Grant at least one tool.");
  if (s.allowedTools.length > 512) put("allowed_tools", "Up to 512 tools.");
  if (s.engines && !s.engines.length) put("engines", "Keep one engine. To offer nothing, turn the server off.");
  if (s.runtimeDirEnv.some(v => !ENV.test(v) || RESERVED_ENV.includes(v))) put("runtime_dir_env", "Use A-Z, 0-9, _. PATH, HOME, TMPDIR and XDG_RUNTIME_DIR are reserved.");
  if (new Set(s.runtimeDirEnv).size !== s.runtimeDirEnv.length) put("runtime_dir_env", "Already listed.");
  s.readPaths.forEach((p, i) => { const m = pathProblem(p); if (m) put(`read_paths[${i}]`, m); });
  s.writePaths.forEach((p, i) => { const m = pathProblem(p); if (m) put(`write_paths[${i}]`, m); });
  s.env.forEach((v, i) => { if (!v.name.trim()) put(`env[${i}].name`, "Enter a name."); else if (!v.stored && !v.value) put(`env[${i}].value`, "Enter a value."); });
  return out;
}
export type ServerStatus = { tone: "st-ok" | "st-idle" | "st-warn" | "st-bad"; text: string };
export function serverStatus(s: McpServer): ServerStatus {
  if (s.problem) return { tone: "st-bad", text: s.problem };
  if ([...s.readPaths, ...s.writePaths].some(pathMissing)) return { tone: "st-warn", text: "Will not start: a path does not exist" };
  if (s.unsupportedKeys.length) return { tone: "st-warn", text: "Has settings this app does not manage. Turn on, off or remove only." };
  return s.enabled ? { tone: "st-ok", text: "On" } : { tone: "st-idle", text: "Off" };
}

export type ParsedCommand = { ok: true; command: string; args: string[] } | { ok: false; message: string };
/** Cortex starts a server without a shell, so what a shell would act on is refused here rather than stored as plain text. */
const SHELL_ONLY = "|&;<>()$`*?[]{}";
const NEEDS_SHELL = "This needs a shell, and none runs here. Write the value out, or use single quotes to keep it as text.";
const UNCLOSED = "A quote is not closed.";
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
/**
 * "Paste a command line": the words a POSIX shell would split it into, read character by character and never run. Quoted and
 * unquoted parts of one word join up, as in --dir="/a b". A refusal leaves the caller's fields alone.
 */
export function parseCommandLine(text: string): ParsedCommand {
  const src = text.replace(/\r\n?/g, "\n").trim(), words: string[] = [], refuse = (message: string): ParsedCommand => ({ ok: false, message });
  let word: string | null = null, plain = true;
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    if (c === "'") {
      const end = src.indexOf("'", i + 1);
      if (end < 0) return refuse(UNCLOSED);
      word = (word ?? "") + src.slice(i + 1, end); plain = false; i = end;
    } else if (c === "\"") {
      let part = "";
      for (i++; src[i] !== "\""; i++) {
        if (i >= src.length) return refuse(UNCLOSED);
        if (src[i] === "$" || src[i] === "`") return refuse(NEEDS_SHELL);
        if (src[i] === "\\" && src[i + 1] === "\n") { i++; continue; }
        if (src[i] === "\\" && i + 1 < src.length && "\"\\$`".includes(src[i + 1])) i++;
        part += src[i];
      }
      word = (word ?? "") + part; plain = false;
    } else if (c === "\\") {
      if (i + 1 >= src.length) return refuse("The line ends in a backslash.");
      i++;
      if (src[i] !== "\n") { word = (word ?? "") + src[i]; plain = false; }
    } else if (c === "\n") return refuse("Paste one command at a time.");
    else if (c === " " || c === "\t") {
      if (word !== null) words.push(word);
      word = null; plain = true;
    } else if (SHELL_ONLY.includes(c) || (word === null && c === "#")) return refuse(NEEDS_SHELL);
    else if (word === null && c === "~") return refuse("Use a full path, starting with /.");
    else if (c === "=" && plain && !words.length && word !== null && ENV_NAME.test(word)) return refuse("Add variables under Environment, then paste the command alone.");
    else word = (word ?? "") + c;
  }
  if (word !== null) words.push(word);
  return words[0] ? { ok: true, command: words[0], args: words.slice(1) } : refuse("Paste the command that starts the server.");
}
