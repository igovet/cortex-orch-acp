import { createEffect, createSignal, Show } from "solid-js";
import { useWs } from "./ctx";
import { Strip } from "./layout";

/** Observe exact pending question identities, not polling cycles or lifecycle guesses. */
export function QuestionNotifications() {
  const ws = useWs(), seen = new Set<string>();
  const [error, setError] = createSignal("");
  createEffect(() => {
    const state = ws.store.state(), route = ws.router.route(), enabled = ws.panels.notifications();
    if (ws.store.mode !== "real") return;
    const labels = new Set<string>();
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
        seen.add(d.id); fresh = true;
      }
      if (fresh && (!open || document.hidden) && enabled && ws.notify)
        labels.add(task?.title ?? sprint?.title ?? "Sprint planning");
    }
    for (const label of labels) void ws.notify!(label).catch(() =>
      setError("Desktop notification could not be delivered. Questions remain in Inbox."));
  });
  return <Show when={error()}><Strip class="ws-conn" role="alert" tone="warn" action={<button class="btn btn-sm" onClick={() => setError("")}>Dismiss</button>}>{error()}</Strip></Show>;
}
