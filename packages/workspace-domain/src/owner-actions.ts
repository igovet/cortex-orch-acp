/**
 * Access a task lacks and only the owner can give (`owner_actions.list` and `owner_actions.act` on the machine service): a
 * sign-in to GitHub, Codex or Claude, or sudo. The task keeps going; once the owner acts (signs in in a terminal, pastes a token
 * where the service's command line takes one, or says it is done and the machine checks again) the runtime retries the blocked
 * work. An action never carries a token or password; `phoneTokens` says whether the owner lets a paired phone send a token.
 */
export type OwnerActionTerminal = { command: string; argv: string[] };
export type OwnerActionToken = { label: string; hint: string };
export type OwnerAction = {
  actionId: string; projectId: string; projectName: string | null; taskId: string | null; task: string | null;
  kind: "access"; service: string; title: string; detail: string;
  terminal: OwnerActionTerminal | null; token: OwnerActionToken | null; recheck: boolean;
  openedAt: number; checkedAt: number | null; lastStatus: string | null;
};
export type OwnerActions = { actions: OwnerAction[]; phoneTokens: boolean };
export type OwnerActionVerb = "recheck" | "terminal" | "dismiss";
export type OwnerActionReply =
  | { outcome: "resolved"; how: "recheck" | "terminal_login" | "token" | "dismissed" }
  | { outcome: "still_needed"; status: string }
  | { outcome: "terminal"; command: string; argv: string[] };
export type OwnerActionRequests = {
  "owner_actions.list": Record<string, never>;
  "owner_actions.act":
    | { projectId: string; actionId: string; action: OwnerActionVerb }
    | { projectId: string; actionId: string; action: "token"; token: string };
};

const isStr = (v: unknown): v is string => typeof v === "string";
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const orNull = (v: unknown): string | null => isStr(v) ? v : null;
/** An action id as the service makes it: 1 to 96 visible ASCII characters. */
const ACTION_ID = /^[\x21-\x7e]{1,96}$/;

/** The open actions as the service answered them, or an error naming what was malformed. */
export function readOwnerActions(raw: unknown): OwnerActions {
  const bad = (what: string) => new Error(`Invalid owner actions: ${what}`);
  if (!raw || typeof raw !== "object") throw bad("not an object");
  const r = raw as Record<string, unknown>;
  if (!Array.isArray(r.actions) || typeof r.phoneTokens !== "boolean") throw bad("actions or phoneTokens");
  const actions = r.actions.map((x: unknown): OwnerAction => {
    const a = (x ?? {}) as Record<string, unknown>;
    if (!isStr(a.actionId) || !ACTION_ID.test(a.actionId) || !isStr(a.projectId) || !a.projectId || a.kind !== "access"
      || !isStr(a.service) || !a.service || !isStr(a.title) || !isStr(a.detail) || typeof a.recheck !== "boolean" || !isNum(a.openedAt))
      throw bad("action");
    let terminal: OwnerActionTerminal | null = null;
    if (a.terminal !== null && a.terminal !== undefined) {
      const t = a.terminal as Record<string, unknown>;
      if (!isStr(t.command) || !t.command || /[\r\n\0]/.test(t.command) || !Array.isArray(t.argv) || !t.argv.length || !t.argv.every(isStr)) throw bad("terminal");
      terminal = { command: t.command, argv: [...t.argv] as string[] };
    }
    let token: OwnerActionToken | null = null;
    if (a.token !== null && a.token !== undefined) {
      const t = a.token as Record<string, unknown>;
      if (!isStr(t.label) || !isStr(t.hint)) throw bad("token");
      token = { label: t.label, hint: t.hint };
    }
    return {
      actionId: a.actionId, projectId: a.projectId, projectName: orNull(a.projectName), taskId: orNull(a.taskId), task: orNull(a.task),
      kind: "access", service: a.service, title: a.title, detail: a.detail, terminal, token, recheck: a.recheck,
      openedAt: a.openedAt, checkedAt: isNum(a.checkedAt) ? a.checkedAt : null, lastStatus: orNull(a.lastStatus),
    };
  });
  return { actions, phoneTokens: r.phoneTokens };
}

/** One `owner_actions.act` reply, or an error. */
export function readOwnerActionReply(raw: unknown): OwnerActionReply {
  const r = (raw ?? {}) as Record<string, unknown>;
  if (r.outcome === "resolved" && (r.how === "recheck" || r.how === "terminal_login" || r.how === "token" || r.how === "dismissed"))
    return { outcome: "resolved", how: r.how };
  if (r.outcome === "still_needed") return { outcome: "still_needed", status: isStr(r.status) ? r.status : "unknown" };
  if (r.outcome === "terminal" && isStr(r.command) && r.command && !/[\r\n\0]/.test(r.command) && Array.isArray(r.argv) && r.argv.every(isStr))
    return { outcome: "terminal", command: r.command, argv: [...r.argv] as string[] };
  throw new Error("Invalid owner action reply");
}
