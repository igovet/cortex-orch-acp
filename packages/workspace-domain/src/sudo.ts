/**
 * The owner's password prompts for `sudo` in a command a task runs on the machine (`askpass.list` and `askpass.answer` on the
 * machine service). A prompt names the command, the task and the worker; it never carries a password. One answer, a password
 * or a cancel, closes it. `phoneAnswers` says whether the owner lets a paired phone send a password.
 */
export type SudoPrompt = {
  promptId: string; title: string; prompt: string; command: string; cwd: string | null;
  taskId: string | null; task: string | null; worker: string | null; projectId: string | null; projectName: string | null;
  openedAt: number; expiresAt: number;
};
export type SudoPrompts = { prompts: SudoPrompt[]; phoneAnswers: boolean };
export type SudoRequests = {
  "askpass.list": Record<string, never>;
  "askpass.answer": { promptId: string; password: string } | { promptId: string; cancel: true };
};

const isStr = (v: unknown): v is string => typeof v === "string";
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const orNull = (v: unknown): string | null => isStr(v) ? v : null;
/** The prompts as the service answered them, or an error naming what was malformed. */
export function readSudoPrompts(raw: unknown): SudoPrompts {
  const bad = (what: string) => new Error(`Invalid sudo prompts: ${what}`);
  if (!raw || typeof raw !== "object") throw bad("not an object");
  const r = raw as Record<string, unknown>;
  if (!Array.isArray(r.prompts) || typeof r.phoneAnswers !== "boolean") throw bad("prompts or phoneAnswers");
  const prompts = r.prompts.map((x: unknown) => {
    const p = (x ?? {}) as Record<string, unknown>;
    if (!isStr(p.promptId) || !/^[0-9a-f]{1,64}$/.test(p.promptId) || !isStr(p.command) || !isNum(p.openedAt) || !isNum(p.expiresAt)) throw bad("prompt");
    return {
      promptId: p.promptId, title: isStr(p.title) ? p.title : "Password for sudo", prompt: isStr(p.prompt) ? p.prompt : "", command: p.command,
      cwd: orNull(p.cwd), taskId: orNull(p.taskId), task: orNull(p.task), worker: orNull(p.worker),
      projectId: orNull(p.projectId), projectName: orNull(p.projectName), openedAt: p.openedAt, expiresAt: p.expiresAt,
    };
  });
  return { prompts, phoneAnswers: r.phoneAnswers };
}
