import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { capabilityOf, FEATURES, machineDefaults, type TaskSettings } from "@cortex/workspace-domain";
import { attachmentEvents, AttachmentStatus, AttachButton, AttachChips, createAttachments, createDictation, MicBar, MicButton, RouteBar, withFiles } from "./composer";
import { useWs } from "./ctx";
import { Tile } from "./frame";
import { I, MenuItem, Pop } from "./kit";
import type { Route } from "./router";
import { projectsOn, sprintTitle } from "./selectors";

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
  const [text, setText] = createSignal("");
  const [tried, setTried] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const defaults = machineDefaults(ws.machine());
  const [chosen, setCheckout] = createSignal<"worktree" | "main">(defaults.checkout);
  // A folder without Git has no branches: the task works in the folder itself, and nothing offers a branch choice.
  const noGit = () => summary()?.project.hasGit === false;
  const separate = () => ws.can(FEATURES.taskWorktrees) && !noGit();
  const noWorktree = () => noGit() ? "A worktree needs Git. This folder has no Git repository, so the task works in the folder itself." : ws.t("noTaskWorktrees");
  const checkout = () => separate() ? chosen() : "main";
  const [runMode, setRunMode] = createSignal<"manual" | "automatic">("manual");
  const [chosenRoute, setSettings] = createSignal<TaskSettings>(defaults.settings);
  let routePicked = false;
  createEffect(() => {
    const options = ws.store.state().options.models.filter(m => !m.unavailable), values = machineDefaults(ws.machine()).settings;
    setSettings(current => {
      // Keep an explicit choice only while the machine still offers it.
      const preferred = routePicked ? current : values;
      const model = options.find(m => m.id === preferred.model) ?? options[0];
      return { ...preferred, model: model?.id ?? "", effort: model ? model.efforts && !model.efforts.includes(preferred.effort) ? model.efforts[0] ?? "" : preferred.effort : "" };
    });
  });
  // With no choice per task, this is the machine's own, which may arrive after the page opens.
  const settings = () => ws.can(FEATURES.taskRoute) ? chosenRoute() : machineDefaults(ws.machine()).settings;
  const [sprintId, setSprintId] = createSignal<string | null>(props.route.sprint ?? null);
  const [target, setTarget] = createSignal<string | null>(null);
  const [otherBranch, setOtherBranch] = createSignal<string | null>(null);
  const [titleState, setTitleState] = createSignal<TitleState>("empty");
  const [renamed, setRenamed] = createSignal<string | null>(null);
  const [renaming, setRenaming] = createSignal<string | null>(null);
  const projects = createMemo(() => projectsOn(ws.store.index(), ws.machine().id).filter(s => s.project.activated !== false));
  const [picked, setPicked] = createSignal<string | null>(props.route.project);
  const summary = () => projects().find(p => p.project.id === picked()) ?? projects().find(p => !p.offline) ?? projects()[0];
  const blocked = () => !summary() ? "Add a project to this machine first" : summary()!.offline ? `${summary()!.machine.name} is offline` : null;
  const empty = () => !text().trim();
  const draftTitle = () => ws.demo?.title.draft(text()) ?? "";
  const title = () => renamed() ?? draftTitle();
  const branch = () => `cw/${slug(title())}`;
  const kind = () => props.route.kind;
  const defaultBranch = () => summary()?.project.defaultBranch ?? "";
  const openBranch = () => summary()?.project.openBranch ?? "";
  const branches = () => summary() ? [...new Set([summary()!.project.defaultBranch, summary()!.project.openBranch])].filter(Boolean) : [];
  const mergeInto = () => target() ?? defaultBranch();
  const sprints = () => summary()?.sprints.filter(s => s.planning === "created") ?? [];
  const pickedSprint = () => sprints().find(s => s.id === sprintId()) ?? null;
  const swap = (next: "task" | "sprint") => ws.go({ ...props.route, kind: next }, { replace: true });
  const mic = createDictation();
  const files = createAttachments();
  const description = () => withFiles(text(), files.files());
  const append = (spoken: string) => write(text().trim() ? `${text().trimEnd()} ${spoken}` : spoken);
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
  // Only a real switch of project clears them, not the first projection arriving under a deep link.
  createEffect(on(projectId, (_id, previous) => { if (previous === undefined) return; setSprintId(null); setTarget(null); }, { defer: true }));
  const saveTitle = () => { const v = renaming(); if (v?.trim()) { clearTimeout(naming); setRenamed(v.trim()); setTitleState("named"); } setRenaming(null); };

  const create = async (start: boolean) => {
    setTried(true);
    if (saving() || files.busy()) return;
    const s = summary();
    if (empty() || blocked() || !s) return;
    setSaving(true);
    try {
    if (kind() === "sprint") {
      const receipt = await ws.run({ type: "create_sprint", projectId: s.project.id, description: description(), runMode: runMode(), settings: settings(), targetBranch: target() });
      if (!receipt.ok || !receipt.entityId) return;
      // Route and target are recorded before the planning start.
      ws.go({ name: "sprint", machine: s.machine.id, project: s.project.id, sprint: receipt.entityId, tab: "planning" });
      return;
    }
    const receipt = await ws.run({
      type: "create_task", projectId: s.project.id, description: text(), attachments:files.files(), settings: settings(), sprintId: pickedSprint()?.id,
      checkout: checkout() === "main" ? { kind: "main" } : { kind: "worktree", branch: branch(), path: `worktrees/${slug(title())}-${ws.unique()}` },
    });
    if (!receipt.ok || !receipt.entityId) return;
    const taskId = receipt.entityId;
    // The task exists now: a refused target must not keep the owner on this form, where saving again would make a second task.
    if (target() && ws.can(capabilityOf("set_target"))) { const saved = await ws.run({ type: "set_target", taskId, branch: target() }); if (!saved.ok) { ws.go({ name: "task", machine: s.machine.id, task: taskId }); return; } }
    if (renamed() !== null) { if (ws.can(capabilityOf("rename_task"))) void ws.run({ type: "rename_task", taskId, title: renamed()! }); }
    else if (ws.can(capabilityOf("generate_title"))) void ws.run({ type: "generate_title", taskId, fail: titleState() === "failed" });
    if (start) {
      const started = await ws.run({ type: "start_task", taskId, now: ws.store.clock() });
      if (started.ok) { ws.go({ name: "task", machine: s.machine.id, task: taskId }); return; }
    }
    ws.go({ name: "project", machine: s.machine.id, project: s.project.id, view: "board", scope: { kind: "all" } });
    } finally { setSaving(false); }
  };

  let saveButton: HTMLButtonElement | undefined, startButton: HTMLButtonElement | undefined;
  /** The whole save, from the first command to the page change, keeps the pressed button loading. */
  const submit = (start: boolean, control?: HTMLElement) => { void ws.busy(create(start), control); };

  const Target = (p: { hint: string }) => <Show when={mergeInto() && !noGit()}><div class="fld ws-inline">
    <span class="lab">Merge into</span>
    <span class="row"><button class="pick mono" popovertarget="m-new-target" style={{ height: "28px" }} aria-label={`Merge into ${mergeInto()}`}><I n="branch" s={14} class="faint" />{mergeInto()}<I n="chevron-down" s={12} /></button>
      <span class="hint">{p.hint}</span></span>
    <Show when={otherBranch() !== null}><span class="row ws-other">
      <input class="inp mono" aria-label="Other branch" placeholder="branch name" value={otherBranch()!} onInput={e => setOtherBranch(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())}
        onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); if (otherBranch()!.trim()) { setTarget(otherBranch()!.trim()); setOtherBranch(null); } } else if (e.key === "Escape") { e.stopPropagation(); setOtherBranch(null); } }} />
    </span></Show>
  </div></Show>;

  return <div class="create"><div class="create-in">
    <div class="create-top">
      <div class="seg tall" role="group" aria-label="What to create">
        <button aria-current={kind() === "task" ? "true" : "false"} onClick={() => swap("task")}>Task</button>
        <button aria-current={kind() === "sprint" ? "true" : "false"} {...ws.needs("create_sprint")} onClick={() => { if (!ws.gate("create_sprint")) swap("sprint"); }}>Sprint</button>
      </div>
      <button class="pick" popovertarget="m-new-project" aria-disabled={summary() ? undefined : "true"} data-why={summary() ? undefined : "No project is open"}>
        <Show when={summary()} fallback="No project"><Tile name={summary()!.project.name} n={Math.max(0, projects().indexOf(summary()!))} />{summary()!.project.name}</Show><I n="chevron-down" s={12} />
      </button>
      <Pop id="m-new-project" class="pop-anchor" align="left" label="Project" style={{ width: "260px" }}>
        <div class="mi-h">Project on {ws.machine().name}</div>
        <For each={projects()}>{s => <MenuItem label={s.project.name} sub={s.offline ? "Offline" : undefined} check={s === summary()} onSelect={() => setPicked(s.project.id)} />}</For>
      </Pop>
      <Show when={kind() === "task" && sprints().length > 0}>
        <button class="pick" popovertarget="m-new-sprint"><I n="layers" s={14} class="faint" /><span class="trunc">{pickedSprint() ? sprintTitle(pickedSprint()!) : "No sprint"}</span><I n="chevron-down" s={12} /></button>
        <Pop id="m-new-sprint" class="pop-anchor" align="left" label="Sprint" style={{ width: "260px" }}>
          <MenuItem label="No sprint" check={!pickedSprint()} onSelect={() => setSprintId(null)} />
          <For each={sprints()}>{s => <MenuItem icon="layers" label={sprintTitle(s)} check={s.id === sprintId()} onSelect={() => setSprintId(s.id)} />}</For>
        </Pop>
      </Show>
      <span class="grow"></span>
      <Show when={ws.store.mode === "demo"}><span class="cap faint">Defaults come from {ws.machine().name}</span></Show>
    </div>
    <Show when={!noGit()}><Pop id="m-new-target" class="pop-anchor" align="left" label="Merge into" style={{ width: "280px" }}>
      <div class="mi-h">Merge into</div>
      <For each={branches()}>{b => <MenuItem label={<span class="mono">{b}</span>} sub={b === defaultBranch() ? "Git default branch" : undefined} check={b === mergeInto()} onSelect={() => setTarget(b === defaultBranch() ? null : b)} />}</For>
      <div class="mi-sep"></div>
      <MenuItem label="Other branch…" onSelect={() => setOtherBranch(target() ?? "")} />
    </Pop></Show>

    <div class="composer" style={{ "margin-top": "2px" }} {...attachmentEvents(files, () => kind() === "task" && !saving())}>
      <MicBar mic={mic} onInsert={append} />
      <AttachChips files={files.files()} onRemove={files.remove} />
      <AttachmentStatus a={files} />
      <textarea aria-label={kind() === "sprint" ? "Describe the sprint" : "Describe the task"} placeholder={kind() === "sprint" ? "Describe the sprint." : ws.store.mode === "demo" ? "Describe the task. Use @ to add files, or paste images and logs." : "Describe the task."}
        value={text()} onInput={e => write(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())}
        onKeyDown={e => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); const start = ws.store.mode === "demo" && kind() === "task"; submit(start, start ? startButton : saveButton); } }} />
      <div class="c-bar">
        <AttachButton a={files} disabled={kind() === "sprint" ? "Attachments are not available for sprints yet. Describe what the files contain, or attach them to a task." : undefined} />
        <RouteBar id="new" disabled={ws.can(FEATURES.taskRoute) ? undefined : ws.t("machineRoute")} settings={settings()} onChange={patch => { routePicked = true; setSettings({ ...settings(), ...patch }); }} />
        <div class="c-end"><MicButton mic={mic} /></div>
      </div>
    </div>
    <div class="ws-slot"><Show when={tried() && (empty() || blocked())}><div class="err">{blocked() ?? "Describe what needs doing."}</div></Show></div>

    <Show when={kind() === "task"} fallback={<div style={{ display: "grid", gap: "14px" }}>
      <div class="note-box plain"><I n="layers" s={14} /><span>Start planning opens a planning conversation that uses the model you choose here: it reads the description, asks questions and proposes linked tasks. No task runs until you create the tasks and start them.</span></div>
      <div class="fld">
        <span class="lab">How tasks run after planning <span class="faint">You can switch this later</span></span>
        <div class="two">
          <label class="rc"><input type="radio" name="sprint-run" checked={runMode() === "manual"} onChange={() => setRunMode("manual")} /><span><span class="t">Manual</span><span class="d">You start each ready task. Starting one never starts its neighbours.</span></span></label>
          <label class="rc"><input type="radio" name="sprint-run" checked={runMode() === "automatic"} onChange={() => setRunMode("automatic")} /><span><span class="t">Automatic</span><span class="d">One task at a time, in dependency order. Questions and failures pause the queue.</span></span></label>
        </div>
      </div>
      <Target hint="Git default branch for this sprint. Each task can override it." />
    </div>}>
      <div style={{ display: "grid", gap: "14px" }}>
        <Show when={!ws.demo}><div class="note-box plain">The task gets its title after saving. Until then, its description is shown. Click the title to change it.</div></Show>
        <Show when={ws.demo}><dl class="title-prev">
          <dt>Title</dt>
          <Show when={renaming() === null} fallback={<dd><input class="inp" aria-label="Task title" value={renaming()!} onInput={e => setRenaming(e.currentTarget.value)} ref={el => queueMicrotask(() => { el.focus(); el.select(); })}
            onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); saveTitle(); } else if (e.key === "Escape") { e.stopPropagation(); setRenaming(null); } }} onBlur={saveTitle} /></dd>}>
            <Show when={titleState() === "empty"}><dd><span class="muted">Written from your description. Click the title to change it.</span></dd></Show>
            <Show when={titleState() === "naming"}><dd><span class="shimmer">{draftTitle()}</span> <span class="faint">Naming…</span></dd></Show>
            <Show when={titleState() === "named"}><dd><button class="tt ws-edit" onClick={() => setRenaming(title())}>{title()}</button></dd></Show>
            <Show when={titleState() === "failed"}><dd><button class="tt muted ws-edit" onClick={() => setRenaming(draftTitle())}>{draftTitle()}</button></dd><dd class="row"><span class="cap st-warn">Title not generated</span><button class="btn btn-sm" onClick={generate}>Retry</button></dd></Show>
          </Show>
        </dl></Show>
        <div class="fld">
          <span class="lab">Where it works</span>
          <div class="two">
            <label class={`rc${separate() ? "" : " ws-rc-off"}`}><input type="radio" name="checkout" disabled={!separate()} checked={checkout() === "worktree"} onChange={() => setCheckout("worktree")} /><span><span class="t">Separate worktree</span><span class="d"><Show when={separate()} fallback={noWorktree()}>New task branch <span class="mono">{ws.demo ? branch() : "cw/…"}</span> from the currently open branch <span class="mono">{openBranch()}</span>. Your open checkout is not touched.</Show></span></span></label>
            <label class="rc"><input type="radio" name="checkout" checked={checkout() === "main"} onChange={() => setCheckout("main")} /><span><span class="t">{noGit() ? "Project folder" : "Main checkout"}</span><span class="d">Works in <span class="mono">{summary()?.project.repository ?? "the project"}</span><Show when={openBranch() && !noGit()}> on the open branch <span class="mono">{openBranch()}</span></Show>.</span></span></label>
          </div>
          <Show when={checkout() === "main"}><div class="note-box"><I n="alert" s={14} class="st-warn" /><span>The task edits the same files you have open, and your uncommitted changes count as part of the task. Rules for two tasks writing here at once are not settled yet.</span></div></Show>
        </div>
        <Target hint="Git default branch. The task stops at Ready to merge for your confirmation." />
      </div>
    </Show>

    <div class="create-foot">
      <Show when={kind() === "task"} fallback={<><span class="cap faint note">Planning runs on the chosen model.</span><button class="btn btn-primary btn-lg" {...ws.needs("create_sprint")} onClick={e => submit(false, e.currentTarget)}>Start planning</button></>}>
        <span class="cap faint note">{ws.store.mode === "real" ? "Ctrl+Enter saves a draft." : ws.t("newTaskNote")}</span>
        <button class="btn btn-line btn-lg" ref={saveButton} {...ws.needs("create_task")} onClick={e => submit(false, e.currentTarget)}>Save as draft</button>
        <button class="btn btn-primary btn-lg" ref={startButton} {...ws.needs("start_task")} onClick={e => { if (!ws.gate("start_task")) submit(true, e.currentTarget); }}>Start now</button>
      </Show>
    </div>
  </div></div>;
}
