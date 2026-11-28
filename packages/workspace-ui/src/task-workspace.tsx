import { createContext, createMemo, For, Show, type Accessor, type JSX } from "solid-js";
import type { ProjectRepo, TaskOverlay } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import type { ProjectSummary } from "./selectors";
import "./task-workspace.css";

/** The active composer mounts its controls in the dock when it is visible. */
export const TaskWorkspaceDock = createContext<Accessor<HTMLElement | undefined>>(() => undefined);

export function TaskWorkspacePanel(props: {
  summary: ProjectSummary | undefined;
  checkout: "main" | "worktree";
  sharedSprintCheckout: boolean;
  target: string;
  checkoutControls: JSX.Element;
  targetControls: JSX.Element;
}) {
  const ws = useWs();
  const project = () => props.summary?.project;
  const repo = createMemo<ProjectRepo | undefined>(() => {
    const p = project();
    return p && !p.mock ? ws.store.state().repos.find(repo => repo.projectId === p.id && !repo.mock) : undefined;
  });
  const main = () => repo()?.checkouts.find(row => row.kind === "main" && row.projectId === project()?.id && row.path === project()?.repository);
  const git = () => project()?.hasGit ?? repo()?.hasGit;
  const files = createMemo(() => [...new Set(git() === true ? main()?.files.map(file => file.path) ?? [] : [])]);
  const connected = () => !!props.summary && !props.summary.offline && !ws.store.savedView(props.summary.project) && !ws.store.readIssue() && !props.summary.project.issue;
  const location = (overlay: TaskOverlay | undefined) => {
    if (overlay?.checkout?.kind === "main") return ws.t("taskWorkspaceProjectFolder");
    const linked = overlay?.checkout;
    if (linked?.kind === "worktree" && linked.id && !linked.unavailable && repo()?.checkouts.some(c => c.id === linked.id && c.kind === "worktree" && c.path === linked.path)) return ws.t("taskWorkspaceWorktree");
    return ws.t("taskWorkspaceFolderUnknown");
  };
  const tasksConfirmed = () => {
    const p = project();
    return !!p && !p.mock && connected() && ws.store.projectSettled(p);
  };
  const tasks = createMemo(() => {
    if (!tasksConfirmed()) return [];
    return (props.summary?.views ?? []).filter(v => !v.task.mock && !v.task.archive && !!v.task.executionId
      && (v.unknown || v.task.process === "unknown" || (v.task.process === "live" && (v.task.status === "working" || v.task.status === "needs_you"))));
  });

  return <section class="task-workspace" aria-label={ws.t("taskWorkspaceHeading")}>
    <header class="task-workspace__header"><h2>{ws.t("taskWorkspaceHeading")}</h2></header>
    <div class="task-workspace__section">
      {props.checkoutControls}
      <dl class="task-workspace__facts">
        <dt>{ws.t("taskWorkspaceFolder")}</dt>
        <dd class={props.checkout === "main" ? "mono" : undefined}>{props.checkout === "main" ? project()?.repository || ws.t("taskWorkspaceUnknown")
          : ws.t(props.sharedSprintCheckout ? "taskWorkspaceSprintCheckout" : "taskWorkspaceCreatedAtStart")}</dd>
        <Show when={git() !== false}><dt>{ws.t("taskWorkspaceOpenBranch")}</dt><dd class="mono">
          {main()?.branch || project()?.openBranch || ws.t("taskWorkspaceUnknown")}
        </dd></Show>
        <Show when={props.checkout === "worktree" && !props.sharedSprintCheckout}>
          <dt>{ws.t("taskWorkspaceTaskBranch")}</dt><dd class="mono">cw/…</dd>
        </Show>
      </dl>
      <Show when={git() !== false}>{props.targetControls}
        <Show when={!props.target}><p class="task-workspace__muted">{ws.t("taskWorkspaceTargetUnknown")}</p></Show>
      </Show>
    </div>
    <div class="task-workspace__section">
      <h3>{ws.t("taskWorkspaceLocalChanges")}</h3>
      <Show when={files().length > 0} fallback={<p class="task-workspace__muted">{
        ws.t(git() === false ? "taskWorkspaceNoGit" : git() === true && main() ? "taskWorkspaceNoListedFiles" : "taskWorkspaceFilesUnknown")
      }</p>}>
        <ul class="task-workspace__files"><For each={files().slice(0, 4)}>{path => <li class="mono">{path}</li>}</For></ul>
        <Show when={files().length > 4}><p class="task-workspace__muted">{ws.t("taskWorkspaceMoreFiles").replace("{count}", String(files().length - 4))}</p></Show>
      </Show>
      <p class="task-workspace__effect">{ws.t(props.checkout === "main" ? "taskWorkspaceSharedEffect" : props.sharedSprintCheckout ? "taskWorkspaceInheritedEffect" : "taskWorkspaceSeparateEffect")}</p>
    </div>
    <div class="task-workspace__section">
      <h3>{ws.t("taskWorkspaceOtherTasks")}</h3>
      <Show when={tasks().length > 0} fallback={<p class="task-workspace__muted">{ws.t(tasksConfirmed() ? "taskWorkspaceNoConfirmedTasks" : "taskWorkspaceTasksUnknown")}</p>}>
        <ul class="task-workspace__tasks"><For each={tasks().slice(0, 4)}>{view => {
          const unknown = () => view.unknown || view.task.process === "unknown";
          return <li><span class="task-workspace__task-title">{view.task.title}</span>
            <span>{ws.t(unknown() ? "taskWorkspaceUnknown" : view.task.status === "needs_you" ? "taskWorkspaceWaiting" : "taskWorkspaceActive")}</span>
            <small>{location(project() ? ws.store.taskWorkspaceOverlay(project()!.id, view.id) : undefined)}</small>
          </li>;
        }}</For></ul>
        <Show when={tasks().length > 4}><p class="task-workspace__muted">{ws.t("taskWorkspaceMoreTasks").replace("{count}", String(tasks().length - 4))}</p></Show>
      </Show>
    </div>
  </section>;
}
