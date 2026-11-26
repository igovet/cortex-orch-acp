import { capabilityOf, taskEditable, type CommandReceipt, type WorkspaceCommand } from "@cortex/workspace-domain";
import type { Ws } from "./ctx";
import { onScreen } from "./freshness";
import { archiveBlocked, archiveLabel, gitItems } from "./git-actions-model";
import { revealCard, revealKey } from "./reveal-intent";
import { repoOf, taskCheckout } from "./review-model";
import type { Route } from "./router";
import { closeBlocked, EXTERNAL_DONE_LABEL, externalDoneBlocked, offersExternalDone, REWORK_LABEL, resumeState, reworkBinding, sessionModel, unconfirmedState } from "./session-model";
import { automaticAcceptanceHold, decisionRoute, ownerHold, REVISION_WAIT, reviewDeadEnd, type TaskView } from "./selectors";

export type TaskAction = {
  id: string;
  label: string;
  icon: string;
  enabled: boolean;
  why?: string;
  keys?: string;
  sub?: string;
  danger?: boolean;
  group: "task" | "workspace";
  run: () => void | Promise<CommandReceipt>;
};

/** Open at the panel that owns the next decision. */
export const taskRoute = (v: TaskView): Route => automaticAcceptanceHold(v) ? decisionRoute(v) : { name: "task", machine: v.machine.id, task: v.id };

type ActionScope = Pick<Ws, "go" | "select" | "inspectable" | "gate" | "can" | "t" | "open" | "run" | "stopTask" | "closeTask" | "archiveTask" | "returnForRework" | "noOpener" | "externalLabel"> & {
  store: Pick<Ws["store"], "index" | "state" | "clock" | "pendingTask">;
  router: Pick<Ws["router"], "route">;
  panels: Pick<Ws["panels"], "pins" | "togglePin">;
};

/** Compute availability without dispatching anything. Every surface uses the same handlers and reasons. */
export function taskActionsFor(v: TaskView, ws: ActionScope): TaskAction[] {
  const { task, id, machine, project } = v;
  const model = sessionModel(ws.store.index(), id);
  const resume = resumeState(v);
  const state = ws.store.state();
  const checkout = taskCheckout(state, task);
  const unknown = v.unknown ? "Unavailable until this task's state is confirmed" : undefined;
  const gate = (type: WorkspaceCommand["type"], why?: string) => why ?? ws.gate(type);
  const result: TaskAction[] = [];
  const add = (id: string, label: string, icon: string, run: TaskAction["run"], why?: string, extra?: Partial<Pick<TaskAction, "keys" | "danger" | "group" | "sub">>) => {
    result.push({ id, label, icon, enabled: !why, why, group: "task", ...extra, run: () => { if (!why) return run(); } });
  };
  const send = (type: "start_task" | "continue_task" | "retry_task" | "confirm_stop") => ws.run({ type, taskId: id, now: ws.store.clock() });
  const reveal = (selectors: string[]) => {
    const route = ws.router.route();
    if (route.name !== "task" || route.task !== id || route.machine !== machine.id || route.tab || route.live)
      ws.go({ name: "task", machine: machine.id, task: id });
    const key = revealKey(machine.id, id, task.executionId);
    queueMicrotask(() => revealCard(key, onScreen(), selectors));
  };
  add("open", "Open task", "arrow-right", () => ws.go(taskRoute(v)));
  add("preview", "Preview in Inspector", "eye", () => ws.select(id), ws.inspectable() ? undefined : "This screen has no inspector", { keys: "Space" });
  const answering = model?.mode === "live" && task.conflict === "none" && (model.open.length > 0 || (v.column === "needs" && task.observedPhase !== "recovery"));
  if (answering) add("answer", "Answer", "message", () => reveal([".dc"]));
  if (model?.mode === "review" && task.status === "review")
    add("review", "Review changes", "check", () => ws.go({ name: "task", machine: machine.id, task: id, tab: "changes", sub: "diff" }));
  if (automaticAcceptanceHold(v))
    add("review_result", "Review result", "check", () => ws.go(decisionRoute(v)));
  if (model) {
    const kind = unconfirmedState(model)?.kind;
    const open = model.open.length;
    const needed = kind === "exit_unconfirmed" || kind === "records"
      || (ownerHold(v)?.nextActor === "owner" ? open === 0 || !answering : open === 0 && !!reviewDeadEnd(task) && model.mode !== "unconfirmed");
    if (needed) add("needed", "See what is needed", "help", () => reveal([".chat-review-hold", ".dc:not(.dc-done), .dc-panel"]));
  }
  if (taskEditable(task)) add("edit", "Edit task…", "pencil", () => ws.open({ kind: "edit-task", taskId: id }), gate("edit_task", unknown));
  if (task.status === "draft") add("start", "Start", "play", () => send("start_task"),
    gate("start_task", unknown ?? (v.fact.text === REVISION_WAIT ? "A sprint revision holds this task back. Apply or reject it on the sprint page first"
      : v.fact.icon === "hourglass" ? "Start is controlled by its dependencies or the sprint queue" : undefined)));
  add("stop", "Stop", "stop", () => ws.stopTask(id),
    gate("stop_task", ["working", "needs_you"].includes(task.status) ? unknown : "This task is not running"), { keys: "Ctrl+." });
  if (resume?.action === "retry_task") add("continue", "Continue", "play", () => send("retry_task"), gate("retry_task", unknown));
  add("resume", "Resume", "play", () => send("continue_task"),
    gate("continue_task", resume?.action === "continue_task" ? unknown : resume?.action === "retry_task" ? "This task is not paused. Use Continue task to go on from the recorded state" : resume?.why || "This task is not paused"));
  if (task.status === "stopping") add("confirm_stop", ws.t("confirmStop"), "check", () => send("confirm_stop"),
    gate("confirm_stop", machine.connection !== "connected" ? `${machine.name} is offline` : undefined));
  if (reworkBinding(v)) add("rework", REWORK_LABEL, "message", () => ws.returnForRework(id), gate("rework_task"));
  const closeWhy = gate("close_task", model ? closeBlocked(model) ?? undefined : unknown);
  add("close", "Close task…", "x", () => ws.closeTask(id, task.integrationFacts?.alreadyInTarget && model?.mode === "review" && task.status === "ready_to_merge" ? "already_in_target" : undefined), closeWhy);
  if (offersExternalDone(v)) add("external-done", EXTERNAL_DONE_LABEL, "check", () => ws.closeTask(id, "external"),
    gate("close_task", model ? externalDoneBlocked(model) ?? undefined : unknown));
  if (task.checkout.kind === "worktree" && task.checkout.id && !task.archive) {
    const cleanup = task.checkout.cleanupRetryReady === true ? undefined : task.checkout.cleanupRetryReady === false
      ? "Worktree cleanup is unavailable." : !checkout ? ws.t("worktreeUnavailable")
      : checkout.files.length ? "Commit or remove uncommitted files first." : undefined;
    const why = unknown ?? (!ws.can("workspace.task_worktrees") ? ws.t("noTaskWorktrees")
      : task.process !== "stopped" ? "Stop the task and wait for its workers first."
      : task.merge && !task.merge.finished ? "Finish or abort the merge first." : cleanup);
    const title = ws.t("closeDeleteWorktreeFolder");
    add("delete_worktree", title, "trash", () => ws.open({
      kind: "confirm", title, action: title, danger: true,
      note: "Close this task and remove its clean worktree folder. The branch and main project folder are kept. If safe removal cannot be confirmed, the worktree is kept.",
      pending: () => ws.store.pendingTask("close_task", id) ? "Closing task…" : undefined,
      go: async () => (await ws.run({ type: "close_task", choice: "remove_worktree", taskId: id, confirmed: true, now: ws.store.clock() })).ok,
    }), gate("close_task", why), { danger: true });
  }
  add("archive", archiveLabel(task), "archive", () => ws.archiveTask(id), gate("archive_task", archiveBlocked(task, machine)));
  add("pin", ws.panels.pins().includes(id) ? "Unpin" : "Pin", "pin", () => ws.panels.togglePin(id));
  const git = gitItems({ checkout, repo: repoOf(state, project.id), task: v, mode: model?.mode, running: null,
    gate: ws.gate, offered: type => ws.can(capabilityOf(type)) });
  for (const item of git) {
    if (item.id === "update") add("update", item.label, item.icon, () => ws.open({ kind: "update", taskId: id }), item.why);
    if (item.id === "view_pr") add("view_pr", item.label, item.icon, () => ws.run({ type: "open_pull_request", projectId: project.id, taskId: id }), item.why);
  }
  const missingWorktree = task.checkout.kind === "worktree" && !checkout;
  const noEditor = ws.noOpener("workspace.open_editor", "noExternal");
  add("open_editor", ws.externalLabel(), "external",
    () => ws.run({ type: "open_editor", projectId: project.id, ...(checkout ? { checkoutId: checkout.id } : {}) }),
    noEditor ?? (missingWorktree ? "Worktree unavailable" : undefined),
    { group: "workspace", sub: !noEditor && missingWorktree ? "This task’s worktree is no longer available. Refresh the project." : undefined });
  add("copy_branch", "Copy branch", "branch", () => { void navigator.clipboard?.writeText(task.checkout.branch); }, undefined, { group: "workspace" });
  add("diagnostics", "Diagnostics", "help", () => ws.go({ name: "settings", section: "diagnostics", machine: machine.id }), undefined, { group: "workspace" });
  return result;
}
