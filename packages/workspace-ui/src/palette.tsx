import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { taskRoute } from "./board";
import { useWs } from "./ctx";
import { I, Modal } from "./kit";
import { rowIcon, sprintRoute, sprintTitle } from "./selectors";
import { closeBlocked, sessionModel } from "./session-model";

type Entry = { id: string; group: string; label: string; sub?: string; icon: string; keys?: string; disabled?: string; run: () => void };
const RESULT_LIMIT = 8;

/** One field for tasks, projects and sprints; ">" lists actions. It launches the same actions the buttons run, never a text command. */
export function Palette(props: { open: boolean; onClose: () => void }) {
  const ws = useWs();
  const [text, setText] = createSignal("");
  const [active, setActive] = createSignal(0);
  createEffect(on(() => props.open, open => { if (open) { setText(""); setActive(0); } }));
  const m = () => ws.machine().id;
  const taskActions = (): Entry[] => {
    const r = ws.router.route(), model = r.name === "task" ? sessionModel(ws.store.index(), r.task) : null;
    if (!model) return [];
    const blocked = closeBlocked(model) ?? undefined, id = model.view.id;
    return [
      { id: "a:stop", group: "Task", label: "Stop task", icon: "stop", keys: "Ctrl+.", disabled: model.mode === "live" ? ws.gate("stop_task") : "This task is not running", run: () => ws.stopTask(id) },
      { id: "a:resume", group: "Task", label: "Resume task", icon: "play", disabled: model.mode === "paused" ? ws.gate("continue_task") : "This task is not paused", run: () => ws.run({ type: "continue_task", taskId: id, now: ws.store.clock() }) },
      { id: "a:close", group: "Task", label: "Close task…", icon: "x", disabled: blocked ?? ws.gate("close_task"), run: () => ws.closeTask(id) },
    ];
  };
  const actions = createMemo((): Entry[] => [
    ...taskActions(),
    { id: "a:new", group: "Actions", label: "New task", icon: "plus", keys: "Ctrl+N", run: () => ws.newItem("task") },
    { id: "a:sprint", group: "Actions", label: "New sprint", icon: "layers", keys: "Ctrl+Shift+N", disabled: ws.gate("create_sprint"), run: () => ws.newItem("sprint") },
    { id: "a:home", group: "Actions", label: "Go to Home", icon: "home", run: () => ws.go({ name: "home", machine: m() }) },
    { id: "a:inbox", group: "Actions", label: "Go to Inbox", icon: "inbox", keys: "Ctrl+I", run: () => ws.go({ name: "inbox", machine: m() }) },
    { id: "a:archive", group: "Actions", label: "Go to Archive", icon: "archive", run: () => ws.go({ name: "archive", machine: m() }) },
    { id: "a:side", group: "Actions", label: ws.panels.sidebarHidden() ? "Show sidebar" : "Hide sidebar", icon: "panel-left", keys: "Ctrl+B", run: () => ws.panels.toggleSidebar() },
    { id: "a:ins", group: "Actions", label: ws.panels.inspectorVisible() ? "Hide inspector" : "Show inspector", icon: "panel-right", keys: "Ctrl+Alt+B", disabled: ws.inspectable() ? undefined : "This screen has no inspector", run: () => ws.panels.toggleInspector() },
    { id: "a:term", group: "Actions", label: ws.panels.terminalOpen() ? "Hide terminal" : "Show terminal", icon: "panel-bottom", keys: "Ctrl+J", disabled: ws.terminalAvailable() ? undefined : "Open a project or task to use a terminal", run: () => ws.toggleTerminal() },
    { id: "a:settings", group: "Actions", label: "Open settings", icon: "settings", keys: "Ctrl+,", run: () => ws.go({ name: "settings", section: "appearance", machine: null }) },
    { id: "a:orch", group: "Actions", label: "Orchestration settings", icon: "sliders", run: () => ws.go({ name: "settings", section: "orchestration", machine: m() }) },
    { id: "a:clean", group: "Actions", label: "Run cleanup now", icon: "archive", disabled: ws.machine().connection === "connected" ? ws.gate("run_cleanup") : `${ws.machine().name} is offline`, run: () => ws.run({ type: "run_cleanup", machineId: m(), now: ws.store.clock() }) },
    { id: "a:machine", group: "Actions", label: "Add remote machine", icon: "server", disabled: ws.gate("pair_machine"), run: () => ws.open({ kind: "add-machine" }) },
    { id: "a:project", group: "Actions", label: "Add project", icon: "folder-plus", disabled: ws.gate("add_project"), run: () => ws.open({ kind: "add-project" }) },
    { id: "a:help", group: "Actions", label: "Help and shortcuts", icon: "help", keys: "?", run: () => ws.openHelp() },
  ]);
  const found = createMemo((): { entries: Entry[]; hidden: number } => {
    const raw = text().trim(), index = ws.store.index();
    if (raw.startsWith(">")) {
      const q = raw.slice(1).trim().toLowerCase();
      return { entries: actions().filter(a => a.label.toLowerCase().includes(q)), hidden: 0 };
    }
    if (!raw) return { entries: actions(), hidden: 0 };
    const q = raw.toLowerCase();
    const tasks = index.views.filter(v => v.task.title.toLowerCase().includes(q));
    const projects = [...index.summaries.values()].filter(s => s.project.name.toLowerCase().includes(q));
    const sprints = index.state.sprints.filter(s => sprintTitle(s).toLowerCase().includes(q));
    return {
      hidden: Math.max(0, tasks.length - RESULT_LIMIT),
      entries: [
        ...projects.map((s): Entry => ({ id: `p:${s.project.id}`, group: "Projects", label: s.project.name, sub: s.machine.name, icon: "folder", run: () => ws.go({ name: "project", machine: s.machine.id, project: s.project.id, view: "board", scope: { kind: "all" } }) })),
        ...sprints.map((s): Entry => {
          const owner = index.summaries.get(s.projectId)!;
          return { id: `s:${s.id}`, group: "Sprints", label: sprintTitle(s), sub: owner.project.name, icon: "layers", run: () => ws.go(sprintRoute(owner.machine.id, s)) };
        }),
        ...tasks.slice(0, RESULT_LIMIT).map((v): Entry => ({ id: `t:${v.id}`, group: "Tasks", label: v.task.title, sub: `${v.project.name}, ${v.fact.text}`, icon: rowIcon(v), run: () => ws.go(taskRoute(v)) })),
      ],
    };
  });
  createEffect(on(found, () => setActive(0)));
  const pick = (entry: Entry | undefined) => {
    if (!entry || entry.disabled) return;
    props.onClose();
    entry.run();
  };
  const onKey = (e: KeyboardEvent) => {
    const count = found().entries.length;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (count) setActive(i => (i + (e.key === "ArrowDown" ? 1 : count - 1)) % count);
    } else if (e.key === "Enter") { e.preventDefault(); pick(found().entries[active()]); }
  };
  const optionId = (i: number) => `ws-pal-${i}`;
  return <Modal open={props.open} onClose={props.onClose} label="Search and actions" class="ws-palette wide">
    <div class="ws-pal-h"><I n="search" class="faint" />
      <input class="ws-pal-in" data-autofocus role="combobox" aria-expanded="true" aria-controls="ws-pal-list" aria-activedescendant={found().entries.length ? optionId(active()) : undefined}
        aria-label="Search tasks, projects and sprints, or type > for actions" placeholder="Search tasks, projects and sprints. Type > for actions."
        value={text()} onInput={e => setText(e.currentTarget.value)} onKeyDown={onKey} />
    </div>
    <div id="ws-pal-list" class="ws-pal-b" role="listbox" aria-label="Results">
      <Show when={found().entries.length > 0} fallback={<p class="ws-pal-none">No matches</p>}>
        <For each={found().entries}>{(entry, i) => <>
          <Show when={i() === 0 || found().entries[i() - 1].group !== entry.group}><div class="mi-h" role="presentation">{entry.group}</div></Show>
          <div id={optionId(i())} class="mi ws-pal-o" role="option" aria-selected={active() === i()} aria-disabled={entry.disabled ? "true" : undefined} data-why={entry.disabled}
            onMouseMove={() => setActive(i())} onClick={() => pick(entry)}>
            <I n={entry.icon} /><span class="grow trunc">{entry.label}<Show when={entry.sub}><span class="sub trunc">{entry.sub}</span></Show></span>
            <Show when={entry.keys}><span class="kbd">{entry.keys}</span></Show>
          </div>
        </>}</For>
        <Show when={found().hidden > 0}><p class="ws-pal-none">{found().hidden} more tasks match. Keep typing to narrow them.</p></Show>
      </Show>
    </div>
  </Modal>;
}

const KEYS: [string, string][] = [
  ["Ctrl+K", "Search and actions"], ["Ctrl+N", "New task"], ["Ctrl+Shift+N", "New sprint"], ["Ctrl+I", "Inbox"],
  ["Ctrl+B", "Sidebar"], ["Ctrl+Alt+B", "Inspector"], ["Ctrl+J", "Terminal"], ["Ctrl+\\", "Worker pane"], ["Ctrl+,", "Settings"],
  ["Enter", "Steer: send now"], ["Ctrl+Enter", "Queue: keep it unsent"], ["Ctrl+.", "Stop the task"], ["1 to 8", "Pick an answer to the open question"],
  ["/", "Filter the board or list"], ["Space", "Preview a card in the Inspector"], ["Enter", "Open"], ["Arrow keys", "Move between cards and columns"],
  ["Ctrl+P", "Go to file, in Code"], ["Ctrl+Shift+F", "Search in files, in Code"], ["Ctrl+Enter", "Commit, or send diff comments"],
  ["Alt+Left, Alt+Right", "Back and Forward"], ["?", "This help"],
];
export function Help(props: { open: boolean; onClose: () => void }) {
  return <Modal open={props.open} onClose={props.onClose} label="Help and shortcuts">
    <div class="dlg-h"><h2>Help and shortcuts</h2><button class="ib" data-autofocus aria-label="Close" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b"><dl class="kv ws-keys"><For each={KEYS}>{([keys, what]) => <><dt>{keys}</dt><dd>{what}</dd></>}</For></dl></div>
  </Modal>;
}
