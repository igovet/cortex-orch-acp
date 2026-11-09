import { pathMissing, pathProblem, type ConnectOutcome, type ConnectResult } from "@cortex/workspace-domain";
import type { McpServer } from "./types";
import { MOCK_GENERIC_TOOLS, MOCK_SERVER_TOOLS } from "./config-preset";

const FIXED: Record<ConnectOutcome, { message: string; field?: string; manual?: boolean }> = {
  command_not_found: { message: "Command not found.", field: "command" },
  exited: { message: "The server closed at once. It may need a path or a variable." },
  timeout: { message: "No answer in 20 seconds." },
  path_refused: { message: "A path was refused: it is missing, private, inside a project, or a symlink." },
  needs_variable: { message: "The server reports a missing environment variable." },
  not_supported: { message: "This machine's runtime cannot list tools.", manual: true },
};
/**
 * Simulated. No runtime operation can list a server's tools today, so this returns the fixture's list or one of the fixed
 * failures. Only fixed text is ever shown, never what a server printed.
 */
export function mockConnect(s: McpServer, forced: ConnectOutcome | null, now: number): ConnectResult {
  const fail = (code: ConnectOutcome): ConnectResult => ({ ok: false, code, ...FIXED[code] });
  if (forced) return fail(forced);
  if (!s.command.trim()) return fail("command_not_found");
  if (/broken/.test(`${s.name} ${s.command}`)) return fail("exited");
  if ([...s.readPaths, ...s.writePaths].some(p => pathMissing(p) || pathProblem(p))) return fail("path_refused");
  const text = `${s.name} ${s.command} ${s.args.join(" ")}`, known = MOCK_SERVER_TOOLS.find(k => text.includes(k.match));
  return { ok: true, observation: { at: now, name: known?.name ?? (s.name || "server"), version: known?.version ?? "1.0.0", tools: structuredClone(known?.tools ?? MOCK_GENERIC_TOOLS) } };
}
