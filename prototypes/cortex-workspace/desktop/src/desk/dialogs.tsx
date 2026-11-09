import { createMemo, createSignal, For, Show } from "solid-js";
import { SCENARIOS, type Scenario } from "../fixtures";
import { useDesk, type ConfirmOpts, type PaletteMode } from "./ctx";
import { Icon, type IconName } from "./icons";
import { draftTitle, numOf } from "./model";
import { projectSignal } from "./sidebar";
import { Btn, Kbd, Modal, Reason, SelectMenu } from "./ui";

export function CreateTaskDialog(props: { open: boolean; projectId: string; onClose: () => void }) {
  const desk = useDesk();
  const [project, setProject] = createSignal(props.projectId);
  const [description, setDescription] = createSignal("");
  const [start, setStart] = createSignal<"discuss" | "draft">("discuss");
  const [tried, setTried] = createSignal(false);
  const ps = () => desk.data.projects().find(p => p.project.id === project()) ?? desk.data.projects()[0];
  const blockedReason = () => desk.data.offline() ? "This device is offline; tasks can't be created" : ps()?.disconnected ? `${ps()!.project.name} machine is disconnected; tasks can't be created` : null;
  // The description is required; the title is generated from it (by a model in the product, by a first-sentence rule in this mock).
  const empty = () => !description().trim();
  const reason = () => blockedReason() ?? (empty() ? "Describe the task" : null);
  const showError = () => tried() && empty() && !blockedReason();
  const close = () => { setDescription(""); setTried(false); props.onClose(); };
  const submit = () => {
    setTried(true);
    if (reason() || !ps()) return;
    const p = ps()!;
    const text = description().trim(), title = draftTitle(text);
    desk.store.createTask(title, p.project.id, text);
    const tasks = desk.store.state().fixture.tasks;
    const task = tasks[tasks.length - 1];
    const num = numOf(tasks.length);
    desk.receipt(`task:${task.id}`, `Mock outcome: task recorded in memory only. No agent was started. The title was generated from the description by a mock rule (no model was called); rename it any time.${start() === "draft" ? " Saved as draft." : ""}`,
      `Created #${num} in ${p.project.name} (mock)`);
    setDescription(""); setTried(false);
    props.onClose();
    desk.go(`/p/${p.slug}/t/${num}/discussion`);
  };
  return <Modal open={props.open} onClose={close} label="New task" class="create-dialog" initial="#ct-desc">
    <form class="dialog-body" onSubmit={e => { e.preventDefault(); submit(); }} novalidate>
      <h2 class="dialog-title">New task</h2>
      <div class="field">
        <span class="field-label" id="ct-project-label">Project</span>
        <SelectMenu id="ct-project" labelId="ct-project-label" label="Project" value={project()} onChange={setProject}
          options={desk.data.projects().map(p => ({ value: p.project.id, label: `${p.project.name}${p.disconnected ? " (disconnected)" : ""}` }))} />
      </div>
      <div class="field">
        <label class="field-label" for="ct-desc">Description</label>
        <textarea id="ct-desc" class="input grow" rows="6" value={description()} required aria-invalid={showError()}
          aria-describedby={showError() ? "ct-desc-err ct-desc-help" : "ct-desc-help"} placeholder="What should be done, and what counts as done?"
          onInput={e => setDescription(e.currentTarget.value)}
          onKeyDown={e => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); submit(); } }} />
        <Show when={showError()}><p class="field-error" id="ct-desc-err"><Icon name="alert" size={14} /> Describe the task</p></Show>
        <p class="field-help" id="ct-desc-help">The title is generated from the description; you can rename the task later. Ctrl+Enter creates.</p>
      </div>
      <fieldset class="field radios">
        <legend class="field-label">Start</legend>
        <label class="radio-row"><input type="radio" name="ct-start" checked={start() === "discuss"} onChange={() => setStart("discuss")} /> Discuss first</label>
        <label class="radio-row"><input type="radio" name="ct-start" checked={start() === "draft"} onChange={() => setStart("draft")} /> Save as draft</label>
      </fieldset>
      <div class="dialog-foot">
        <Reason id="ct-reason" text={showError() ? null : reason()} icon={blockedReason() ? "plugOff" : "info"} />
        <div class="btn-row end">
          <Btn onClick={close}>Cancel</Btn>
          <Btn type="submit" variant="primary" mock disabled={Boolean(reason())} aria-describedby="ct-reason">Create and open</Btn>
        </div>
      </div>
    </form>
  </Modal>;
}

type Option = { id: string; group: "Projects" | "Tasks" | "Commands"; label: string; context?: string; hint?: string; icon?: IconName; mono?: { name: string; index: number }; trailing?: string; run: () => void };

export function Palette(props: { open: boolean; mode: PaletteMode; onClose: () => void }) {
  const desk = useDesk();
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  const [mode, setMode] = createSignal<PaletteMode>(props.mode);
  let list: HTMLDivElement | undefined;
  const run = (fn: () => void) => { props.onClose(); setQuery(""); queueMicrotask(fn); };
  const options = createMemo<Option[]>(() => {
    const q = query().trim().toLowerCase();
    const m = mode();
    const match = (s: string) => !q || s.toLowerCase().includes(q);
    const projects: Option[] = desk.data.projects().filter(p => match(p.project.name)).map(p => {
      const sig = projectSignal(p);
      return { id: `p-${p.index}`, group: "Projects", label: p.project.name, mono: { name: p.project.name, index: p.index },
        trailing: `${sig ? (sig.icon ? sig.text : `Needs you ${sig.text}`) + " · " : ""}${p.taskCount} tasks`, run: () => desk.go(`/p/${p.slug}`) };
    });
    if (m === "projects") return projects;
    const tasks: Option[] = q ? desk.data.tasks().filter(t => match(t.task.title) || t.ref.includes(q)).slice(0, 20).map(t => ({
      id: `t-${t.num}`, group: "Tasks", label: `${t.ref} ${t.task.title}`, context: t.project.name, icon: "file" as IconName,
      run: () => desk.go(`/p/${t.slug}/t/${t.num}/discussion`),
    })) : [];
    const cmd = (label: string, fn: () => void, icon: IconName, hint?: string): Option => ({ id: `c-${label}`, group: "Commands", label, icon, hint, run: fn });
    const commands: Option[] = [
      cmd("New task", () => desk.openCreate(), "plus", "Ctrl Alt N"),
      cmd("Go to project…", () => desk.openPalette("projects"), "grid", "Ctrl O"),
      cmd("Needs you", () => desk.go("/needs-you"), "inbox", "Ctrl Alt D"),
      cmd("All projects", () => desk.go("/"), "grid", "Ctrl Alt H"),
      cmd("Toggle sidebar", () => desk.prefs.setSidebarHidden(!desk.prefs.sidebarHidden()), "panelLeft", "Ctrl B"),
      cmd("Toggle task list", () => desk.prefs.setListPaneHidden(!desk.prefs.listPaneHidden()), "panelList", "Ctrl \\"),
      cmd("Theme: System", () => desk.theme.setPref("system"), "monitor"),
      cmd("Theme: Light", () => desk.theme.setPref("light"), "sun"),
      cmd("Theme: Dark", () => desk.theme.setPref("dark"), "moon"),
      cmd("Keyboard shortcuts", () => desk.openHelp(), "keyboard", "?"),
      cmd("Settings", () => desk.go("/settings/appearance"), "settings", "Ctrl ,"),
      cmd("Prototype: Next step", () => desk.setLocal(l => ({ ...l, step: l.step + 1 })), "flask"),
      cmd("Prototype: Reset", () => desk.reset(), "flask"),
      ...SCENARIOS.map(s => cmd(`Prototype: Scenario ${s[0].toUpperCase()}${s.slice(1)}`, () => desk.store.selectScenario(s as Scenario), "flask")),
    ].filter(o => match(o.label));
    return [...projects, ...tasks, ...commands];
  });
  const clamp = () => Math.min(active(), Math.max(0, options().length - 1));
  const move = (d: number) => {
    const n = options().length;
    if (!n) return;
    const next = (clamp() + d + n) % n;
    setActive(next);
    queueMicrotask(() => list?.querySelector(`#opt-${next}`)?.scrollIntoView?.({ block: "nearest" }));
  };
  const close = () => { setQuery(""); setActive(0); props.onClose(); };
  return <Modal open={props.open} onClose={close} label={props.mode === "projects" ? "Open project" : "Command palette"} class="palette" initial="#palette-input">
    <div class="palette-box">
      <div class="palette-input-row"><Icon name="search" />
        <input id="palette-input" role="combobox" aria-expanded="true" aria-controls="palette-list" aria-autocomplete="list"
          aria-activedescendant={options().length ? `opt-${clamp()}` : undefined} placeholder={props.mode === "projects" ? "Open project…" : "Search projects, tasks and commands"}
          value={query()} onInput={e => { setQuery(e.currentTarget.value); setActive(0); }} aria-label={props.mode === "projects" ? "Open project" : "Search"}
          onKeyDown={e => {
            if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
            else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
            else if (e.key === "Enter") { e.preventDefault(); const o = options()[clamp()]; if (o) run(o.run); }
          }} />
      </div>
      <div class="palette-list" id="palette-list" role="listbox" ref={list} aria-label="Results">
        <Show when={options().length} fallback={<div class="palette-empty">
          <p>No matches for "{query()}"</p>
          <button type="button" class="palette-opt" onClick={() => { setMode("all"); setQuery(query()); }}><Icon name="search" /><span>Search all tasks in all projects</span></button>
        </div>}>
          <For each={options()}>{(o, i) => <>
            <Show when={i() === 0 || options()[i() - 1].group !== o.group}><div class="palette-group" role="presentation">{o.group}</div></Show>
            <div id={`opt-${i()}`} role="option" class={`palette-opt ${clamp() === i() ? "active" : ""}`} aria-selected={clamp() === i()}
              onMouseMove={() => setActive(i())} onClick={() => run(o.run)}>
              <Show when={o.mono} fallback={<Icon name={o.icon ?? "chevronRight"} />}><span class={`mono-tile hue-${o.mono!.index % 8}`} style={{ "--tile": "1rem" }} aria-hidden="true">{o.mono!.name[0]}</span></Show>
              <span class="palette-label">{o.label}</span>
              <Show when={o.context}><span class="palette-ctx">{o.context}</span></Show>
              <Show when={o.trailing}><span class="palette-ctx">{o.trailing}</span></Show>
              <Show when={o.hint}><kbd class="kbd-hint">{o.hint}</kbd></Show>
            </div>
          </>}</For>
        </Show>
      </div>
    </div>
  </Modal>;
}

const KEYS: [string, string][] = [
  ["Ctrl K", "Command palette"], ["Ctrl O", "Open project"], ["Ctrl Alt N", "New task"], ["Ctrl Alt D", "Needs you"], ["Ctrl Alt H", "All projects"],
  ["Ctrl B", "Toggle sidebar"], ["Ctrl \\", "Toggle task list (task workspace)"], ["Ctrl Alt 1–6", "Project views or task tabs"], ["Ctrl ,", "Settings"],
  ["Ctrl `", "Toggle terminal (Code and Git)"], ["Arrows, J K H L", "Move between cards (board focused)"], ["Home / End", "First / last item"],
  ["Enter", "Open task"], ["Esc", "Close popover or dialog; in a task, return focus to the list"], ["Alt ← / Alt →", "Back / Forward"], ["? or Ctrl /", "This dialog"],
];
export function HelpDialog(props: { open: boolean; onClose: () => void }) {
  return <Modal open={props.open} onClose={props.onClose} label="Keyboard shortcuts" class="help-dialog">
    <div class="dialog-body">
      <h2 class="dialog-title">Keyboard shortcuts</h2>
      <p class="tertiary sm-text">Browser-hosted prototype: some shortcuts may be taken by the browser or desktop. Every action is also available from a visible control and the palette. Decision actions have no shortcuts.</p>
      <table class="keys-table"><tbody>
        <For each={KEYS}>{([k, v]) => <tr><td><Kbd>{k}</Kbd></td><td>{v}</td></tr>}</For>
      </tbody></table>
      <div class="btn-row end"><Btn onClick={props.onClose}>Close</Btn></div>
    </div>
  </Modal>;
}

/** Destructive confirmations focus the safe button first. */
export function ConfirmDialog(props: { opts: ConfirmOpts | null; onDone: (ok: boolean) => void }) {
  return <Modal open={Boolean(props.opts)} onClose={() => props.onDone(false)} label={props.opts?.title ?? "Confirm"} class="confirm-dialog" initial=".safe-btn" describedby="confirm-body">
    <div class="dialog-body">
      <h2 class="dialog-title">{props.opts?.title}</h2>
      <p id="confirm-body">{props.opts?.body}</p>
      <div class="btn-row end">
        <Btn class="safe-btn" onClick={() => props.onDone(false)}>Cancel</Btn>
        <Btn variant={props.opts?.danger ? "danger" : "primary"} mock={props.opts?.mock} onClick={() => props.onDone(true)}>{props.opts?.confirm}</Btn>
      </div>
    </div>
  </Modal>;
}
