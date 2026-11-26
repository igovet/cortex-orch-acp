import type { SideChatEntry, SideChatPort } from "@cortex/workspace-domain";

/** One `machine.call` envelope as the service answers it. */
export type MachineReply = { ok?: boolean; result?: Record<string, unknown>; code?: string; message?: string } | null | undefined;

const STATES = new Set(["running", "done", "failed", "interrupted"]);
const text = (v: unknown) => typeof v === "string" ? v : undefined;

/** One side-chat entry the machine answered, or null when it is not one. Only the named fields are kept. */
export function sideChatEntry(raw: unknown): SideChatEntry | null {
  if (typeof raw !== "object" || raw === null) return null;
  const e = raw as Record<string, unknown>;
  if (typeof e.id !== "string" || typeof e.question !== "string" || typeof e.state !== "string" || !STATES.has(e.state)) return null;
  const route = typeof e.route === "object" && e.route !== null ? e.route as Record<string, unknown> : null;
  return {
    id: e.id, question: e.question, at: typeof e.at === "number" ? e.at : 0, state: e.state as SideChatEntry["state"],
    ...(text(e.answer) !== undefined ? { answer: text(e.answer) } : {}),
    ...(text(e.reason) !== undefined ? { reason: text(e.reason) } : {}),
    ...(route ? { route: {
      engine: text(route.engine), model: text(route.model), effort: text(route.effort) ?? null,
      fileAccess: route.fileAccess === "read_tools" || route.fileAccess === "context_only" ? route.fileAccess : undefined,
    } } : {}),
  };
}

/**
 * The side chat over `product.task`. It changes no task and no repository, so it drops no cache and sends no command:
 * a lost reply is just no answer yet. `projectOf` names the task's project, or null while the machine is off.
 */
export function createSideChatPort(call: (projectId: string, params: Record<string, unknown>) => Promise<MachineReply>, projectOf: (taskId: string) => string | null): SideChatPort {
  const ask = async (taskId: string, params: Record<string, unknown>): Promise<MachineReply> => {
    const projectId = projectOf(taskId);
    if (!projectId) return { ok: false, code: "offline", message: "The machine service is unavailable." };
    return await call(projectId, { taskId, ...params }).catch(() => ({ ok: false, code: "unknown", message: "The machine did not answer." }));
  };
  return {
    async history(taskId) {
      const reply = await ask(taskId, { action: "side_chat_history" });
      const entries = reply?.ok === true && Array.isArray(reply.result?.entries) ? reply.result.entries : [];
      return entries.map(sideChatEntry).filter((e): e is SideChatEntry => e !== null);
    },
    async ask({ taskId, question, model, effort }) {
      const reply = await ask(taskId, { action: "side_question", question, ...(model ? { model } : {}), ...(effort ? { effort } : {}) });
      const entry = reply?.ok === true ? sideChatEntry(reply.result?.entry) : null;
      if (entry) return { ok: true, entry };
      return { ok: false, code: reply?.code ?? "unknown", message: reply?.message ?? "The machine did not answer." };
    },
    async poll(taskId, entryId) {
      const reply = await ask(taskId, { action: "side_chat_poll", entryId });
      return reply?.ok === true ? sideChatEntry(reply.result?.entry) : null;
    },
    async clear(taskId) {
      const reply = await ask(taskId, { action: "side_chat_clear" });
      return reply?.ok === true;
    },
  };
}
