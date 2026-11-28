import { createEffect, createMemo, createSignal, createUniqueId, For, on, onCleanup, Show, useContext } from "solid-js";
import { Portal } from "solid-js/web";
import { capabilityOf, effectiveCeiling, FEATURES, machineDefaults, machineEffort, resolveEffort, routingFor, type CommandReceipt, type SprintRunMode, type TaskSettings } from "@cortex/workspace-domain";
import { createAttachments, createDictation, MessageComposer, RouteBar } from "./composer";
import { useWs } from "./ctx";
import { createRequestFeedback, RequestStatus } from "./request-feedback";
import { FieldEdit } from "./field-edit";
import { createUnsent, ScreenActive } from "./freshness";
import { Tile } from "./frame";
import { I, MenuItem, Pop } from "./kit";
import { ActionBar, Toolbar } from "./layout";
import { sessionOptions } from "./port/real";
import type { Route } from "./router";
import { sprintModeDescription, projectsOn, readyStep, sprintTitle } from "./selectors";
import { TaskWorkspaceDock, TaskWorkspacePanel } from "./task-workspace";

/** A branch-safe name, cut at a word so a long title never ends mid-word. */
function slug(text: string) {
  const full = text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  if (full.length <= 32) return full || "task";
  const cut = full.slice(0, 33), end = cut.lastIndexOf("-");
  return end > 0 ? cut.slice(0, end) : full.slice(0, 32);
}
const NAMING_DELAY = 500;
type TitleState = "empty" | "naming" | "named" | "failed";

/** New task or sprint. The description is the only required field; Cortex writes the title, and the owner can always rename it. */
export function NewPage(props: { route: Extract<Route, { name: "new" }> }) {
  const ws = useWs();
  const request = createRequestFeedback();
  const [text, setText] = createSignal("");
  const [tried, setTried] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const defaults = machineDefaults(ws.machine());
  const [chosen, setCheckout] = createSignal<"worktree" | "main">(defaults.checkout);
  let checkoutPicked = false;
  createEffect(() => { const value = machineDefaults(ws.machine()).checkout; if (!checkoutPicked) setCheckout(value); });
  const chooseCheckout = (value: "worktree" | "main") => { checkoutPicked = true; setCheckout(value); };
  // A folder without Git has no branches: the task works in the folder itself, and nothing offers a branch choice.
  const noGit = () => summary()?.project.hasGit === false;
  const noWorktree = () => noGit() ? ws.t("worktreeNeedsGit") : !ws.can(FEATURES.taskWorktrees) ? ws.t("noTaskWorktrees")
    : kind() !== "sprint" && pickedSprint()?.runMode === "automatic" && pickedSprint()?.checkout?.shared !== true ? ws.t("worktreeNeedsManualOrHybrid") : undefined;
  const separate = () => !noWorktree();
  const checkout = () => pickedSprint()?.checkout?.kind ?? (separate() ? chosen() : "main");
  const [runMode, setRunMode] = createSignal<SprintRunMode>("manual");
  // A worktree sprint shares one worktree by default; the owner can ask for one per task, which runs in Manual only.
  const [scope, setScope] = createSignal<"sprint" | "task">("sprint");
  const perTask = () => kind() === "sprint" && checkout() === "worktree" && scope() === "task";
  createEffect(() => { if (perTask() && runMode() !== "manual") setRunMode("manual"); });
  const [chosenRoute, setSettings] = createSignal<TaskSettings>(defaults.settings);
  let routePicked = false;
  // With no choice per task, this is the machine's own, which may arrive after the page opens.
  const settings = () => ws.can(FEATURES.taskRoute) ? chosenRoute() : machineDefaults(ws.machine()).settings;
  const [sprintId, setSprintId] = createSignal<string | null>(props.route.sprint ?? null);
  const formId = createUniqueId();
  const targetMenu = `new-target-${formId}`;
  const checkoutGroup = `checkout-${formId}`;
  const [target, setTarget] = createSignal<string | null>(null);
  const [otherBranch, setOtherBranch] = createSignal<string | null>(null);
  const [titleState, setTitleState] = createSignal<TitleState>("empty");
  const [renamed, setRenamed] = createSignal<string | null>(null);
  const projects = createMemo(() => projectsOn(ws.store.index(), ws.machine().id).filter(s => s.project.activated !== false));
  const [picked, setPicked] = createSignal<string | null>(props.route.project);
  const summary = () => projects().find(p => p.project.id === picked()) ?? projects().find(p => !p.offline) ?? projects()[0];
  const options = createMemo(() => {
    const project = summary()?.project.id, machine = ws.machine();
    return project && ws.store.mode === "real"
      ? sessionOptions(machine.config, routingFor(machine, project), effectiveCeiling(machine, project).allowedRoutes)
      : ws.store.state().options;
  });
  createEffect(() => {
    const models = options().models.filter(m => !m.unavailable), values = machineDefaults(ws.machine()).settings;
    setSettings(current => {
      // Keep an explicit choice only while the selected project still offers it.
      const preferred = routePicked ? current : values;
      const model = models.find(m => m.id === preferred.model) ?? models[0];
      return { ...preferred, model: model?.id ?? "", effort: resolveEffort(routePicked ? preferred.effort : "", machineEffort(ws.machine()), model?.efforts) };
    });
  });
  const blocked = () => !summary() ? "Add a project to this machine first" : summary()!.offline ? `${summary()!.machine.name} is offline`
    : pickedSprint()?.checkout?.kind === "worktree" ? noWorktree() ?? null : null;
  const empty = () => !text().trim();
  const draftTitle = () => ws.demo?.title.draft(text()) ?? "";
  const title = () => renamed() ?? draftTitle();
  const branch = () => `cw/${slug(title())}`;
  const kind = () => props.route.kind;
  const defaultBranch = () => summary()?.project.defaultBranch ?? "";
  const openBranch = () => summary()?.project.openBranch ?? "";
  const branches = () => summary() ? [...new Set([summary()!.project.defaultBranch, summary()!.project.openBranch, pickedSprint()?.targetBranch ?? ""])].filter(Boolean) : [];
  const inheritedTarget = () => pickedSprint()?.targetBranch ?? defaultBranch();
  const mergeInto = () => target() ?? inheritedTarget();
  // The step the task header will name: a project-folder task on the branch it would merge into is finished in place.
  const step = () => readyStep({ checkout: checkout() === "main" ? { kind: "main", branch: openBranch() } : { kind: "worktree", branch: branch(), path: "" } }, mergeInto(), summary()?.project.hasGit);
  const sprints = () => summary()?.sprints.filter(s => s.planning === "created") ?? [];
  const pickedSprint = () => sprints().find(s => s.id === sprintId()) ?? null;
  const swap = (next: "task" | "sprint") => ws.go({ ...props.route, kind: next }, { replace: true });
  const mic = createDictation();
  const files = createAttachments();
  createUnsent(() => !empty() || files.files().length > 0);
  let naming: ReturnType<typeof setTimeout> | undefined;
  const generate = () => {
    clearTimeout(naming);
    // The real service names saved descriptions asynchronously; previews are demo-only.
    if (empty() || !ws.demo) { setTitleState("empty"); return; }
    setTitleState("naming");
    naming = setTimeout(() => setTitleState(ws.demo?.title.fails() ? "failed" : "named"), NAMING_DELAY);
  };
  const write = (value: string) => { setText(value); if (renamed() === null) generate(); };
  onCleanup(() => clearTimeout(naming));
  // A memo of the id: every command publishes fresh snapshot objects, and that must not clear what the owner picked.
  const projectId = createMemo(() => summary()?.project.id);
  const shown = useContext(ScreenActive);
  const dock = useContext(TaskWorkspaceDock);
  const hasTaskPanel = () => kind() === "task" && ws.shell.kind === "desktop" && ws.store.mode === "real" && !ws.demo;
  createEffect(on(() => hasTaskPanel() && shown() ? projectId() : undefined, id => {
    if (id) onCleanup(ws.store.interest(id, "facts"));
  }));
  // Only a real switch of project clears them, not the first projection arriving under a deep link.
  createEffect(on(projectId, (_id, previous) => { if (previous === undefined) return; setSprintId(null); setTarget(null); }, { defer: true }));
  /** The title is kept here until the draft is saved, so Save on it writes nothing to the machine yet. */
  const saveTitle = (title: string): Promise<CommandReceipt> => { clearTimeout(naming); setRenamed(title); setTitleState("named"); return Promise.resolve({ requestId: "new-task-title", ok: true, outcome: "local", message: "" }); };

  const create = async (start: boolean) => {
    setTried(true);
    if (saving() || files.busy()) return;
    const s = summary();
    if (empty() || blocked() || !s) return;
    setSaving(true);
    try {
    if (kind() === "sprint") {
      const receipt = await request.run({ type: "create_sprint", projectId: s.project.id, description: text(), attachments: files.files(), runMode: runMode(), checkout: checkout() === "worktree" ? { kind: "worktree", shared: scope() === "sprint" } : { kind: "main" }, settings: settings(), targetBranch: target(), taskRoute: routePicked ? "sprint" : "automatic" });
      if (!receipt.ok || !receipt.entityId) return;
      // Route and target are recorded before the planning start.
      ws.go({ name: "sprint", machine: s.machine.id, project: s.project.id, sprint: receipt.entityId, tab: "planning" });
      return;
    }
    const receipt = await request.run({
      type: "create_task", projectId: s.project.id, description: text(), attachments:files.files(), settings: settings(), sprintId: pickedSprint()?.id,
      checkout: checkout() === "main" ? { kind: "main" } : { kind: "worktree", branch: branch(), path: `worktrees/${slug(title())}-${ws.unique()}` },
    });
    if (!receipt.ok || !receipt.entityId) {
      if (receipt.entityId && receipt.outcome === "partial_effect") ws.go({ name: "task", machine: s.machine.id, task: receipt.entityId });
      return;
    }
    const taskId = receipt.entityId;
    // The task exists now: a refused target must not keep the owner on this form, where saving again would make a second task.
    if (target() && ws.can(capabilityOf("set_target"))) { const saved = await ws.run({ type: "set_target", taskId, branch: target() }); if (!saved.ok) { ws.go({ name: "task", machine: s.machine.id, task: taskId }); return; } }
    if (renamed() !== null) { if (ws.can(capabilityOf("rename_task"))) void ws.run({ type: "rename_task", taskId, title: renamed()! }); }
    else if (ws.can(capabilityOf("generate_title"))) void ws.run({ type: "generate_title", taskId, fail: titleState() === "failed" });
    if (start) {
      const started = await request.run({ type: "start_task", taskId, now: ws.store.clock() });
      if (started.ok) { ws.go({ name: "task", machine: s.machine.id, task: taskId }); return; }
    }
    ws.go({ name: "project", machine: s.machine.id, project: s.project.id, view: "board", scope: { kind: "all" } });
    } finally { setSaving(false); }
  };

  let form: HTMLDivElement | undefined;
  /** The whole save, from the first command to the page change, keeps the pressed button loading. */
  const submit = (start: boolean, control?: HTMLElement | null) => { void ws.busy(create(start), control); };
  const plane = () => form?.querySelector<HTMLElement>(".mc-send") ?? null;

  const Target = (p: { hint: string }) => <Show when={mergeInto() && !noGit()}><div class="fld ws-inline">
    <span class="lab">{ws.t("taskWorkspaceMergeInto")}</span>
    <span class="row"><button class="pick mono" popovertarget={targetMenu} style={{ height: "28px" }} aria-label={`${ws.t("taskWorkspaceMergeInto")} ${mergeInto()}`}><I n="branch" s={14} class="faint" /><span class="trunc">{mergeInto()}</span><I n="chevron-down" s={12} /></button>
      <span class="hint">{p.hint}</span></span>
    <Pop id={targetMenu} class="pop-anchor" align="left" label={ws.t("taskWorkspaceMergeInto")} style={{ width: "280px" }}>
      <div class="mi-h">{ws.t("taskWorkspaceMergeInto")}</div>
      <For each={branches()}>{b => <MenuItem label={<span class="mono">{b}</span>} sub={b === defaultBranch() ? ws.t("taskWorkspaceGitDefault") : undefined} check={b === mergeInto()} onSelect={() => setTarget(b === inheritedTarget() ? null : b)} />}</For>
      <div class="mi-sep"></div>
      <MenuItem label={ws.t("taskWorkspaceOtherBranch")} onSelect={() => setOtherBranch(target() ?? "")} />
    </Pop>
    <Show when={otherBranch() !== null}><span class="row ws-other">
      <input class="inp mono" aria-label={ws.t("taskWorkspaceOtherBranch")} placeholder={ws.t("taskWorkspaceBranchName")} value={otherBranch()!} onInput={e => setOtherBranch(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())}
        onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); if (otherBranch()!.trim()) { setTarget(otherBranch()!.trim()); setOtherBranch(null); } } else if (e.key === "Escape") { e.stopPropagation(); setOtherBranch(null); } }} />
    </span></Show>
  </div></Show>;

  const Checkout = (p: { panel?: boolean }) => <div class="fld">
    <Show when={!p.panel}><span class="lab">Where it works</span></Show>
    <div class="two" role="radiogroup" aria-label={ws.t("taskWorkspaceFolder")}>
      <label class={`rc${separate() ? "" : " ws-rc-off"}`}><input type="radio" name={checkoutGroup} disabled={!separate()} checked={checkout() === "worktree"} onChange={() => chooseCheckout("worktree")} /><span><span class="t">{kind() === "sprint" ? "Worktree" : ws.t("taskWorkspaceWorktree")}</span><span class="d"><Show when={separate()} fallback={noWorktree()}><Show when={!p.panel} fallback={ws.t("taskWorkspaceCreatedAtStart")}>{kind() === "sprint" ? "One branch named after the sprint" : "New task branch"}<Show when={kind() === "task"}> <span class="mono">{ws.demo ? branch() : "cw/…"}</span></Show> from the currently open branch <span class="mono">{openBranch()}</span>. Your open checkout is not touched.</Show></Show></span></span></label>
      <label class="rc"><input type="radio" name={checkoutGroup} checked={checkout() === "main"} onChange={() => chooseCheckout("main")} /><span><span class="t">{ws.t("taskWorkspaceProjectFolder")}</span><Show when={!p.panel}><span class="d">{kind() === "sprint" ? "All tasks work" : "Works"} in <span class="mono">{summary()?.project.repository ?? "the project"}</span><Show when={openBranch() && !noGit()}> on the open branch <span class="mono">{openBranch()}</span></Show>.</span></Show></span></label>
    </div>
    <Show when={kind() === "sprint" && checkout() === "worktree" && !pickedSprint()}><div class="two" role="radiogroup" aria-label="Worktree scope">
      <label class="rc"><input type="radio" name="worktree-scope" checked={scope() === "sprint"} onChange={() => setScope("sprint")} /><span><span class="t">One worktree for the whole sprint</span><span class="d">Every task works in the same worktree and branch. Finishing a task commits its own files there.</span></span></label>
      <label class="rc"><input type="radio" name="worktree-scope" checked={scope() === "task"} onChange={() => setScope("task")} /><span><span class="t">Separate worktree per task</span><span class="d">Each task gets its own branch and worktree. Runs in Manual mode only.</span></span></label>
    </div></Show>
    <Show when={checkout() === "main" && !p.panel}><div class="note-box"><I n="alert" s={14} class="st-warn" /><span>{kind() === "sprint" ? "All tasks edit" : "The task edits"} the same files you have open, and your uncommitted changes count as part of the work. Rules for two tasks writing here at once are not settled yet.</span></div></Show>
  </div>;
  const TaskCheckout = (p: { panel?: boolean }) => <Show when={!pickedSprint()?.checkout}
    fallback={<div class="cap muted">{ws.t("taskWorkspaceFollowsSprint")} {ws.t(checkout() === "worktree" ? "taskWorkspaceWorktree" : "taskWorkspaceProjectFolder")}.<Show when={checkout() === "worktree" && noWorktree()}> {noWorktree()}</Show></div>}>
    <Checkout panel={p.panel} />
  </Show>;
  const taskTargetHint = () => {
    if (pickedSprint() || !openBranch() || !mergeInto()) return ws.t("taskWorkspaceTargetHint");
    const gate = ws.t(step() === "Ready to merge" ? "taskWorkspaceGateMerge" : "taskWorkspaceGateFinish");
    return target() ? gate : `${ws.t("taskWorkspaceGitDefault")}. ${gate}`;
  };
  const TaskWorkspace = () => <TaskWorkspacePanel summary={summary()} checkout={checkout()} sharedSprintCheckout={pickedSprint()?.checkout?.shared === true}
    target={mergeInto()} checkoutControls={<TaskCheckout panel />} targetControls={<Target hint={taskTargetHint()} />} />;
  const docked = () => shown() ? dock() : undefined;

  return <div class="create"><div class="create-in">
    <Toolbar class="create-top" label="What to create and where" start={<>
      <div class="seg tall" role="group" aria-label="What to create">
        <button aria-current={kind() === "task" ? "true" : "false"} onClick={() => swap("task")}>Task</button>
        <button aria-current={kind() === "sprint" ? "true" : "false"} {...ws.needs("create_sprint")} onClick={() => { if (!ws.gate("create_sprint")) swap("sprint"); }}>Sprint</button>
      </div>
    </>} mid={<>
      <button class="pick l-scope" popovertarget="m-new-project" aria-disabled={summary() ? undefined : "true"} data-why={summary() ? undefined : "No project is open"}>
        <Show when={summary()} fallback="No project"><Tile name={summary()!.project.name} n={Math.max(0, projects().indexOf(summary()!))} /><span class="trunc">{summary()!.project.name}</span></Show><I n="chevron-down" s={12} />
      </button>
      <Show when={kind() === "task" && sprints().length > 0}>
        <button class="pick l-scope" popovertarget="m-new-sprint"><I n="layers" s={14} class="faint" /><span class="trunc">{pickedSprint() ? sprintTitle(pickedSprint()!) : "No sprint"}</span><I n="chevron-down" s={12} /></button>
      </Show>
    </>} end={<Show when={ws.store.mode === "demo"}><span class="cap faint">Defaults come from {ws.machine().name}</span></Show>} />
    <Show when={kind() === "task" && sprints().length > 0}>
      <Pop id="m-new-sprint" class="pop-anchor" align="left" label="Sprint" style={{ width: "260px" }}>
        <MenuItem label="No sprint" check={!pickedSprint()} onSelect={() => setSprintId(null)} />
        <For each={sprints()}>{s => <MenuItem icon="layers" label={sprintTitle(s)} check={s.id === sprintId()} onSelect={() => setSprintId(s.id)} />}</For>
      </Pop>
    </Show>
    <Pop id="m-new-project" class="pop-anchor" align="left" label="Project" style={{ width: "260px" }}>
      <div class="mi-h">Project on {ws.machine().name}</div>
      <For each={projects()}>{s => <MenuItem label={s.project.name} sub={s.offline ? "Offline" : undefined} check={s === summary()} onSelect={() => setPicked(s.project.id)} />}</For>
    </Pop>

    {/* The plane saves a draft, or for a sprint starts planning. Saving never starts the task: Start now below is its own explicit choice. */}
    <div ref={form}><MessageComposer label={kind() === "sprint" ? "Describe the sprint" : "Describe the task"} placeholder={kind() === "sprint" ? "Describe the sprint. Tag @files, use /skills, paste images and logs, or attach files." : "Describe the task. Tag @files, use /skills, paste images and logs, or attach files."}
      suggest={projectId() ? { projectId: projectId()!, skills: true } : undefined}
      text={text()} onText={write} files={files} mic={mic} autofocus sendKey="mod-enter" attachOff={saving() ? "Saving…" : undefined}
      route={<RouteBar id="new" options={options()} disabled={ws.can(FEATURES.taskRoute) ? undefined : ws.t("machineRoute")} settings={settings()} onChange={patch => { routePicked = true; setSettings({ ...settings(), ...patch }); }} />}
      send={kind() === "sprint" ? { label: "Start planning", needs: "create_sprint", go: () => submit(false, plane()) } : { label: "Save draft", needs: "create_task", go: () => submit(false, plane()) }} /></div>
    <RequestStatus text={request.text()} />
    <div class="ws-slot"><Show when={tried() && (empty() || blocked())}><div class="err">{blocked() ?? "Describe what needs doing."}</div></Show></div>

    <Show when={kind() === "task"} fallback={<div style={{ display: "grid", gap: "14px" }}>
      <div class="note-box plain"><I n="layers" s={14} /><span>Start planning opens a planning conversation that uses the model you choose here: it reads the description, asks questions and proposes linked tasks. No task runs until you create the tasks and start them.</span></div>
      <div class="fld">
        <span class="lab">How tasks run after planning <span class="faint">You can switch this later</span></span>
        <div class="two">
          <label class="rc"><input type="radio" name="sprint-run" checked={runMode() === "manual"} onChange={() => setRunMode("manual")} /><span><span class="t">Manual</span><span class="d">{sprintModeDescription.manual}</span></span></label>
          <label class={`rc${perTask() ? " ws-rc-off" : ""}`} data-why={perTask() ? ws.t("separateWorktreesManualOnly") : undefined}><input type="radio" name="sprint-run" disabled={perTask()} checked={runMode() === "hybrid"} onChange={() => setRunMode("hybrid")} /><span><span class="t">Hybrid</span><span class="d">{sprintModeDescription.hybrid}</span></span></label>
          <label class={`rc${perTask() ? " ws-rc-off" : ""}`} data-why={perTask() ? ws.t("separateWorktreesManualOnly") : undefined}><input type="radio" name="sprint-run" disabled={perTask()} checked={runMode() === "automatic"} onChange={() => setRunMode("automatic")} /><span><span class="t">Automatic</span><span class="d">{sprintModeDescription.automatic}</span></span></label>
        </div>
      </div>
      <Checkout />
      <Target hint="Git default branch for this sprint. Each task can override it." />
    </div>}>
      <div style={{ display: "grid", gap: "14px" }}>
        <Show when={!ws.demo}><div class="note-box plain">The task gets its title after saving. Until then, its description is shown. Click the title to change it.</div></Show>
        <Show when={ws.demo}><dl class="title-prev">
          <dt>Title</dt>
          <Show when={titleState() === "empty"}><dd><span class="muted">Written from your description. Click the title to change it.</span></dd></Show>
          <Show when={titleState() === "naming"}><dd><span class="shimmer">{draftTitle()}</span> <span class="faint">Naming…</span></dd></Show>
          <Show when={titleState() === "named"}><dd><FieldEdit label="Task title" value={title()} save={saveTitle} closed={{ class: "tt ws-edit", text: saved => saved }} /></dd></Show>
          <Show when={titleState() === "failed"}><dd><FieldEdit label="Task title" value={draftTitle()} save={saveTitle} closed={{ class: "tt muted ws-edit", text: saved => saved }} /></dd><dd class="row"><span class="cap st-warn">Title not generated</span><button class="btn btn-sm" onClick={generate}>Retry</button></dd></Show>
        </dl></Show>
        <Show when={hasTaskPanel()} fallback={<><TaskCheckout /><Target hint={`Git default branch. The task stops at ${step()} for your confirmation.`} /></>}>
          <Show when={docked()} fallback={<div class="task-workspace__inline"><TaskWorkspace /></div>}>{mount =>
            <Portal mount={mount()}><TaskWorkspace /></Portal>}
          </Show>
        </Show>
      </div>
    </Show>

    <Show when={kind() === "task"}>
      <ActionBar class="create-foot" hint={ws.store.mode === "real" ? "Save draft saves the task without starting it." : ws.t("newTaskNote")}>
        <button class="btn btn-line btn-lg" {...ws.needs("start_task")} onClick={e => { if (!ws.gate("start_task")) submit(true, e.currentTarget); }}><I n="play" />Start now</button>
      </ActionBar>
    </Show>
  </div></div>;
}
