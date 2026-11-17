import { createEffect, createSignal, Show } from "solid-js";
import { useWs } from "./ctx";
import { Strip } from "./layout";

/** Where this app keeps the questions it has already seen, so a relaunch does not announce them again. */
export const SEEN_QUESTIONS_KEY = "cw.notified-questions";
/** The newest ids kept; an older question is long answered or long announced. */
const SEEN_LIMIT = 500;
const storage = () => { try { return typeof localStorage === "undefined" ? undefined : localStorage; } catch { return undefined; } };
function readSeen(): string[] {
  try {
    const saved: unknown = JSON.parse(storage()?.getItem(SEEN_QUESTIONS_KEY) ?? "[]");
    return Array.isArray(saved) ? saved.filter((id): id is string => typeof id === "string").slice(-SEEN_LIMIT) : [];
  } catch { return []; }
}
function writeSeen(ids: Set<string>) {
  try { storage()?.setItem(SEEN_QUESTIONS_KEY, JSON.stringify([...ids].slice(-SEEN_LIMIT))); } catch { /* unavailable storage: this run still dedupes */ }
}

/**
 * Observe exact pending question identities, not polling cycles or lifecycle guesses. The identities already seen are kept in
 * this app's local storage: after a relaunch a question that was announced (or seen open) before stays quiet, while one that
 * arrived while the app was closed is announced once.
 */
export function QuestionNotifications() {
  const ws = useWs(), seen = new Set<string>(readSeen());
  const [error, setError] = createSignal("");
  createEffect(() => {
    const state = ws.store.state(), route = ws.router.route(), enabled = ws.panels.notifications();
    if (ws.store.mode !== "real") return;
    const labels = new Set<string>();
    let changed = false;
    for (const session of state.sessions) {
      const task = state.tasks.find(t => t.id === session.taskId);
      const sprint = state.sprints.find(s => !s.archivedAt && s.planningTaskId === session.taskId);
      if (!task && !sprint) continue;
      const machineId = state.projects.find(p => p.id === (task?.projectId ?? sprint?.projectId))?.machineId;
      if (!state.machines.some(m => m.id === machineId && m.connection === "connected")) continue;
      const open = route.name === "task" && route.task === session.taskId
        || route.name === "sprint" && route.sprint === sprint?.id;
      let fresh = false;
      for (const d of session.decisions) {
        if (d.answer || d.withdrawn || d.suspended || seen.has(d.id)) continue;
        seen.add(d.id); fresh = true; changed = true;
      }
      if (fresh && (!open || document.hidden) && enabled && ws.notify)
        labels.add(task?.title ?? sprint?.title ?? "Sprint planning");
    }
    if (changed) writeSeen(seen);
    for (const label of labels) void ws.notify!(label).catch(() =>
      setError("Desktop notification could not be delivered. Questions remain in Inbox."));
  });
  return <Show when={error()}><Strip class="ws-conn" role="alert" tone="warn" action={<button class="btn btn-sm" onClick={() => setError("")}>Dismiss</button>}>{error()}</Strip></Show>;
}
