import { createEffect, createMemo, createSignal, onCleanup, type Accessor } from "solid-js";
import type { AttachmentInput, CommandReceipt, WorkspaceCommand, WorkspaceState } from "@cortex/workspace-domain";

/** Where a message the owner sends reads: a task's conversation, its queue, one worker's activity, or a sprint's planning. */
export type SendPlace =
  | { kind: "task"; taskId: string }
  | { kind: "queue"; taskId: string }
  | { kind: "worker"; taskId: string; workerId: string }
  | { kind: "sprint"; sprintId: string };

/**
 * A message the owner sent from this window, shown at once where it will read while the machine confirms it. `key` stays
 * the same through a retry; `requestId` is the identity of the latest send, which the machine keeps on a message it saves.
 */
export type Sending = {
  key: string; requestId: string; place: SendPlace; text: string; attachments?: AttachmentInput[];
  state: "sending" | "sent" | "failed"; reason?: string; command: WorkspaceCommand;
  /** What already stood in its place when it was sent: none of these is its record. */
  known: string[];
  /** A task conversation's last line when it was sent: its record reads after it. Earlier conversation read later, above
   *  the lines on screen, is never taken for it. */
  after?: string;
};

/** How long a confirmed message waits to find its record before the record is trusted to be the one shown. */
export const SENT_HOLD_MS = 60_000;

export const placeKey = (p: SendPlace) => p.kind === "sprint" ? `sprint:${p.sprintId}` : p.kind === "worker" ? `worker:${p.taskId}:${p.workerId}` : `${p.kind}:${p.taskId}`;

/** The sends that show at once: a message, a queued message, a message to a worker and a message to the planner. */
export function placeOf(command: WorkspaceCommand): SendPlace | null {
  switch (command.type) {
    case "send_message": return command.workerId ? { kind: "worker", taskId: command.taskId, workerId: command.workerId } : command.mode === "queue" ? { kind: "queue", taskId: command.taskId } : { kind: "task", taskId: command.taskId };
    case "request_changes": return { kind: "task", taskId: command.taskId };
    case "plan_message": return { kind: "sprint", sprintId: command.sprintId };
    default: return null;
  }
}

type Candidate = { id: string; text: string; ref?: string; index?: number };
/** The task whose conversation a place reads in: the task itself, or the runtime task planning a sprint. A sprint that plans
 *  without one (an earlier version's planning chat) reads in its own conversation. */
function conversationTask(place: SendPlace, state: WorkspaceState): string | null {
  if (place.kind === "task") return place.taskId;
  if (place.kind !== "sprint") return null;
  const id = state.sprints.find(s => s.id === place.sprintId)?.planningTaskId;
  return id && state.tasks.some(t => t.id === id) ? id : null;
}
const timelineCandidates = (taskId: string, state: WorkspaceState) => (state.sessions.find(s => s.taskId === taskId)?.timeline ?? []).flatMap((e, index): Candidate[] => {
  if (e.kind === "said") return [{ id: e.id, text: e.text, index }];
  if (e.kind === "message") { const m = state.messages.find(x => x.id === e.messageId); return [{ id: e.id, text: m?.text ?? "", ref: e.messageId, index }]; }
  return [];
});
/** The owner's lines that already read in a place, each with a stable id; `ref` is the saved message's own id, when it has one. */
function candidates(place: SendPlace, state: WorkspaceState): Candidate[] {
  switch (place.kind) {
    case "task": return timelineCandidates(place.taskId, state);
    case "queue": return state.messages.filter(m => m.taskId === place.taskId && m.workerId === null).map(m => ({ id: m.id, text: m.text, ref: m.id }));
    case "worker": return state.messages.filter(m => m.taskId === place.taskId && m.workerId === place.workerId).map(m => ({ id: m.id, text: m.text, ref: m.id }));
    // A message to a sprint's planning task reads once it is in that task's conversation, not when its sprint's copy is saved.
    case "sprint": { const task = conversationTask(place, state); if (task) return timelineCandidates(task, state); }
      return (state.sprints.find(s => s.id === place.sprintId)?.conversation ?? []).flatMap((e, i): Candidate[] => e.kind === "message" ? [{ id: String(i), text: e.text }] : []);
  }
}

/**
 * The owner's sent messages, from the press until their record reads in the place they were sent to. A record matches by
 * the send's own identity when the machine keeps one (a saved, queued or worker message is saved under the send's request
 * id), otherwise by its text among the lines that arrived after the send. Each record answers one send, in order, so the
 * message reads once and in the place it was first shown. A refused send stays, with its reason, until it is sent again or
 * taken back into the composer.
 */
export function createSends(state: Accessor<WorkspaceState>, dispatch: (command: WorkspaceCommand) => Promise<CommandReceipt>) {
  const [entries, setEntries] = createSignal<Sending[]>([]);
  /** Records that already answered a send, by place: a later send of the same words never takes them. */
  const claimed = new Map<string, Set<string>>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  let again: { command: WorkspaceCommand; key: string } | null = null;
  const [returned, setReturned] = createSignal<{ place: string; text: string; attachments?: AttachmentInput[] } | null>(null);
  onCleanup(() => { for (const t of timers.values()) clearTimeout(t); timers.clear(); });

  const matched = createMemo(() => {
    const s = state(), hits = new Map<string, string>(), byPlace = new Map<string, Sending[]>();
    for (const e of entries()) { const k = placeKey(e.place); byPlace.set(k, [...(byPlace.get(k) ?? []), e]); }
    for (const [k, list] of byPlace) {
      const all = candidates(list[0].place, s), used = new Set(claimed.get(k));
      for (const e of list) {
        const task = e.after === undefined ? null : conversationTask(e.place, s);
        const from = task === null ? -1 : (s.sessions.find(x => x.taskId === task)?.timeline ?? []).findIndex(x => x.id === e.after);
        const free = all.filter(c => !used.has(c.id) && !e.known.includes(c.id) && (from < 0 || (c.index ?? Infinity) > from));
        const hit = free.find(c => c.ref === e.requestId) ?? free.find(c => c.text.trim() === e.text.trim());
        if (hit) { used.add(hit.id); hits.set(e.key, hit.id); }
      }
    }
    return hits;
  });
  // A settled send whose record arrived is that record from now on.
  createEffect(() => {
    const hits = matched(), done = entries().filter(e => e.state !== "sending" && hits.has(e.key));
    if (!done.length) return;
    for (const e of done) {
      const k = placeKey(e.place);
      claimed.set(k, new Set([...(claimed.get(k) ?? []), hits.get(e.key)!]));
      clearTimeout(timers.get(e.key)); timers.delete(e.key);
    }
    setEntries(list => list.filter(e => !done.includes(e)));
  });

  const update = (key: string, patch: Partial<Sending>) => setEntries(list => list.map(e => e.key === key ? { ...e, ...patch } : e));
  return {
    /** Records a send as it is pressed. Nothing is recorded for a command that is not a message. */
    begin(requestId: string, command: WorkspaceCommand) {
      const place = placeOf(command);
      if (!place) return;
      const text = "text" in command ? command.text : "", attachments = "attachments" in command ? command.attachments : undefined;
      const known = candidates(place, state()).map(c => c.id);
      // The last line the runtime recorded (the window's own lines come and go with the facts).
      const task = conversationTask(place, state());
      const after = task !== null ? [...(state().sessions.find(x => x.taskId === task)?.timeline ?? [])].reverse().find(x => !x.id.startsWith("facts:"))?.id : undefined;
      if (again?.command === command) {
        const key = again.key; again = null;
        update(key, { requestId, state: "sending", reason: undefined, known, after });
        return;
      }
      setEntries(list => [...list, { key: requestId, requestId, place, text, attachments, state: "sending", command, known, after }]);
    },
    settle(requestId: string, receipt: CommandReceipt) {
      const entry = entries().find(e => e.requestId === requestId);
      if (!entry) return;
      if (!receipt.ok) { update(entry.key, { state: "failed", reason: receipt.message || "The message was not sent." }); return; }
      update(entry.key, { state: "sent" });
      const key = entry.key;
      timers.set(key, setTimeout(() => { timers.delete(key); setEntries(list => list.filter(e => e.key !== key)); }, SENT_HOLD_MS));
    },
    /** The sends of one place whose record does not read there yet, in the order they were sent. */
    shown(place: SendPlace): Sending[] {
      const k = placeKey(place), hits = matched();
      return entries().filter(e => placeKey(e.place) === k && !hits.has(e.key));
    },
    /** Sends a refused message again, in the same place. */
    retry(key: string) {
      const entry = entries().find(e => e.key === key && e.state === "failed");
      if (!entry) return Promise.resolve(undefined);
      again = { command: entry.command, key };
      return dispatch(entry.command).finally(() => { if (again?.key === key) again = null; });
    },
    /** Takes a refused message back into its composer, files and all. */
    edit(key: string) {
      const entry = entries().find(e => e.key === key && e.state === "failed");
      if (!entry) return;
      setEntries(list => list.filter(e => e.key !== key));
      setReturned({ place: placeKey(entry.place), text: entry.text, attachments: entry.attachments });
    },
    /** A message taken back for the composer of this place, once: the composer that takes it clears it. */
    returned: (place: SendPlace) => { const r = returned(); return r && r.place === placeKey(place) ? r : null; },
    take: () => setReturned(null),
  };
}
export type Sends = ReturnType<typeof createSends>;
