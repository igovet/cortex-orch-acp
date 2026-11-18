import { createEffect, createMemo, createSignal, createUniqueId, For, onCleanup, Show } from "solid-js";
import type { ProjectSkill } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { I, keepInside, place } from "./kit";
import { same } from "./stable";

/**
 * Inline suggestions in a message field: `@` names a project file, `/` at the start of a line names a command of this
 * screen or a project skill. Whatever is accepted lands in the field as plain text (a command runs instead), so drafts,
 * copy, queued and failed sends keep it exactly as written.
 */

/** A `/` command of the screen it is offered on: an action that already exists there, never a new one. */
export type SlashCommand = { name: string; label: string; why?: string; run: () => void };
export type SuggestOptions = {
  /** The project whose files `@` names. Left out where no project is known: no `@` list. */
  projectId?: string;
  /** The task whose own changed files join the project's files. */
  taskId?: string;
  /** `/` commands, where this field has any. */
  commands?: () => SlashCommand[];
  /** `/` lists the project's skills too. */
  skills?: boolean;
};
export type Trigger = { kind: "file" | "slash"; start: number; query: string };
type Option = { id: string; kind: "file" | "command" | "skill"; label: string; sub?: string; why?: string; file?: string; command?: SlashCommand; skill?: ProjectSkill };
/** The same option as far as the list shows and accepts it. A command's `run` is looked up again when it is picked. */
const sameOption = (a: Option, b: Option) => a.kind === b.kind && a.label === b.label && a.sub === b.sub && a.why === b.why && a.file === b.file && same(a.skill, b.skill);

/** The field's hint where `@` files and `/` commands and skills are all offered. */
export const SUGGEST_HINT = "Message the agent, tag @files, or use /commands and /skills";
export const MAX_SHOWN = 20;
/** How long typing rests before the file list is looked through again. */
export const FILE_DEBOUNCE_MS = 120;
const MAX_QUERY = 200;
const SKILLS_FRESH_MS = 60_000;

/**
 * What the caret is in: an `@` word (at the start or after a space or an opening bracket, so an e-mail address is not one) or
 * a `/` word that begins a line, leading spaces allowed. The word runs from its sign to the caret, with no space in it.
 */
export function triggerAt(text: string, caret: number, slash: boolean): Trigger | null {
  let i = caret - 1;
  while (i >= 0 && !/\s/.test(text[i]) && text[i] !== "@" && !(slash && text[i] === "/" && lineStart(text, i))) i--;
  if (i < 0 || caret - i - 1 > MAX_QUERY) return null;
  const query = text.slice(i + 1, caret);
  if (text[i] === "@" && (i === 0 || /[\s([{"'`]/.test(text[i - 1]))) return { kind: "file", start: i, query };
  if (slash && text[i] === "/" && lineStart(text, i)) return { kind: "slash", start: i, query };
  return null;
}
function lineStart(text: string, at: number) {
  for (let j = at - 1; j >= 0; j--) { if (text[j] === "\n") return true; if (text[j] !== " " && text[j] !== "\t") return false; }
  return true;
}

const lower = (s: string) => s.toLocaleLowerCase();
const lowered = new WeakMap<readonly string[], string[]>();
/**
 * The best `max` paths for `query`: a file name that starts with it, then one that holds it, then a path that holds it, then a
 * path that holds its letters in order. Shorter and shallower paths first within each. Case and script do not matter.
 */
export function rankFiles(paths: readonly string[], query: string, max = MAX_SHOWN): string[] {
  const q = lower(query);
  let low = lowered.get(paths);
  if (!low) { low = paths.map(lower); lowered.set(paths, low); }
  const best: { path: string; score: number }[] = [];
  for (let n = 0; n < paths.length; n++) {
    const p = low[n], base = p.slice(p.lastIndexOf("/") + 1);
    let score: number;
    if (!q) score = p.split("/").length * 1000 + p.length;
    else if (base.startsWith(q)) score = p.length;
    else if (base.includes(q)) score = 10_000 + base.indexOf(q) * 100 + p.length;
    else if (p.includes(q)) score = 100_000 + p.indexOf(q) * 10 + p.length;
    else {
      let at = 0, gaps = 0, last = -1;
      for (let k = 0; k < p.length && at < q.length; k++) if (p[k] === q[at]) { if (last >= 0) gaps += k - last - 1; last = k; at++; }
      if (at < q.length) continue;
      score = 1_000_000 + gaps * 10 + p.length;
    }
    if (best.length === max && score >= best[max - 1].score) continue;
    let k = best.findIndex(b => b.score > score);
    if (k < 0) k = best.length;
    best.splice(k, 0, { path: paths[n], score });
    if (best.length > max) best.pop();
  }
  return best.map(b => b.path);
}
/** A file mention as text: `@path`, quoted when the path holds a space. */
export const mention = (path: string) => /\s/.test(path) ? `@"${path}"` : `@${path}`;
/** What the model receives for a skill picked from `/`: an ordinary sentence naming the project skill and where it is. */
export const skillText = (s: ProjectSkill) => s.pathOnly ? `Use the project skill at ${s.path}.` : `Use the project skill ${s.name} (${s.path}).`;

/** Paths picked in this window, newest first: what `@` offers before anything is typed. */
const recent = new Map<string, string[]>();
const remember = (projectId: string, path: string) => recent.set(projectId, [path, ...(recent.get(projectId) ?? []).filter(p => p !== path)].slice(0, 6));
type SkillRead = { at: number; pending: Promise<ProjectSkill[]>; value: ProjectSkill[] | null };
const skillReads = new WeakMap<object, Map<string, SkillRead>>();

/**
 * The suggestion state of one field. `onInput`, `onKeyDown` and `onCaret` come from the field; `key` answers true when it
 * used the key, so Enter accepts instead of sending while options are shown.
 */
export function createSuggest(props: { options: () => SuggestOptions | undefined; text: () => string; setText: (text: string) => void; field: () => HTMLTextAreaElement | undefined }) {
  const ws = useWs();
  const listId = createUniqueId();
  const [trigger, setTrigger] = createSignal<Trigger | null>(null);
  const [dismissed, setDismissed] = createSignal<number | null>(null);
  const [files, setFiles] = createSignal<{ query: string; paths: string[] } | null>(null);
  const [skills, setSkills] = createSignal<ProjectSkill[] | null>(null);
  const [loading, setLoading] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined, asked = 0, composing = false;
  let load: { id: string; done: Promise<void> } | null = null;
  onCleanup(() => clearTimeout(timer));

  const opts = () => props.options();
  const repo = (projectId: string) => ws.store.state().repos.find(r => r.projectId === projectId);
  const tree = (projectId: string, taskId?: string): string[] | null => {
    const r = repo(projectId);
    if (!r) return null;
    const own = taskId ? r.checkouts.find(c => c.taskId === taskId)?.files.map(f => f.path) ?? [] : [];
    return own.length ? [...new Set([...r.tree, ...own])] : r.tree;
  };
  /** The project's files, read once (with its repository) the first time something is typed after `@` and they are not here yet. */
  const lookUp = async (query: string) => {
    const o = opts(), at = ++asked;
    if (!o?.projectId) return;
    const id = o.projectId;
    if (!query) { setFiles({ query, paths: recent.get(id) ?? [] }); setLoading(false); return; }
    if (!tree(id, o.taskId)) {
      // One read for every keystroke that waits on it; it is not repeated for this field once it answered.
      if (load?.id !== id) load = { id, done: ws.store.refreshProjectFacts(id, true).catch(() => {}) };
      setLoading(true);
      await load.done;
      // A later keystroke asked again: this answer is for words no longer in the field.
      if (at !== asked) return;
    }
    setLoading(false);
    setFiles({ query, paths: rankFiles(tree(id, o.taskId) ?? [], query) });
  };
  const readSkills = (projectId: string) => {
    let cache = skillReads.get(ws.store);
    if (!cache) { cache = new Map(); skillReads.set(ws.store, cache); }
    let read = cache.get(projectId);
    if (!read || (read.value && Date.now() - read.at > SKILLS_FRESH_MS)) {
      const entry: SkillRead = { at: Date.now(), value: read?.value ?? null, pending: ws.store.listProjectSkills(projectId).then(r => r?.skills ?? []) };
      entry.pending = entry.pending.then(v => (entry.value = v), () => (entry.value = entry.value ?? []));
      cache.set(projectId, read = entry);
    }
    if (read.value) setSkills(read.value);
    const current = read;
    void current.pending.then(() => { if (opts()?.projectId === projectId && current.value) setSkills(current.value); });
  };

  const update = () => {
    const el = props.field();
    if (!el || composing) return;
    const o = opts();
    const t = o ? triggerAt(props.text(), el.selectionStart ?? props.text().length, !!(o.commands || (o.skills && o.projectId))) : null;
    const usable = t && (t.kind === "slash" || o?.projectId) ? t : null;
    if (!usable || usable.start !== dismissed()) setDismissed(null);
    const was = trigger();
    setTrigger(usable);
    if (!usable) { clearTimeout(timer); asked++; return; }
    if (was?.kind !== usable.kind || was.start !== usable.start || was.query !== usable.query) setActiveKey(null);
    if (usable.kind === "file") {
      clearTimeout(timer);
      if (was?.kind === "file" && was.start === usable.start && was.query === usable.query) return;
      // Nothing typed yet looks nothing up: only what was picked before is shown.
      if (!usable.query) { void lookUp(""); return; }
      timer = setTimeout(() => void lookUp(usable.query), FILE_DEBOUNCE_MS);
    } else if (o?.skills && o.projectId && (was?.kind !== "slash" || was.start !== usable.start)) readSkills(o.projectId);
  };

  const listed = (): Option[] => {
    const t = trigger();
    if (!t || dismissed() === t.start) return [];
    if (t.kind === "file") return (files()?.paths ?? []).map(p => ({ id: `f:${p}`, kind: "file" as const, label: p, file: p }));
    const q = lower(t.query);
    const fits = (name: string) => lower(name).includes(q);
    const commands = (opts()?.commands?.() ?? []).filter(c => fits(c.name) || fits(c.label))
      .sort((a, b) => Number(!lower(a.name).startsWith(q)) - Number(!lower(b.name).startsWith(q)))
      .map(c => ({ id: `c:${c.name}`, kind: "command" as const, label: `/${c.name}`, sub: c.label, why: c.why, command: c }));
    const named = (opts()?.skills ? skills() ?? [] : []).filter(s => fits(s.name) || fits(s.description ?? ""))
      .sort((a, b) => Number(!lower(a.name).startsWith(q)) - Number(!lower(b.name).startsWith(q)))
      .map(s => ({ id: `s:${s.path}`, kind: "skill" as const, label: `/${s.name}`, sub: s.description ?? s.path, skill: s }));
    return [...commands, ...named].slice(0, MAX_SHOWN);
  };
  // A background update (a new fact, an event, another task's change) rebuilds the commands and this field's options; the
  // list keeps every option that still reads the same as the old object, so its rows, highlight and scroll stay put.
  const options = createMemo<Option[]>(prev => {
    const next = listed();
    const byId = new Map(prev.map(o => [o.id, o]));
    const out = next.map(o => { const old = byId.get(o.id); return old && sameOption(old, o) ? old : o; });
    return out.length === prev.length && out.every((o, i) => o === prev[i]) ? prev : out;
  }, []);
  /** The highlighted option, by its key: an update that keeps it keeps it highlighted wherever it now stands. None is the first. */
  const [activeKey, setActiveKey] = createSignal<string | null>(null);
  const active = () => { const k = activeKey(), i = k === null ? -1 : options().findIndex(o => o.id === k); return i < 0 ? 0 : i; };
  const setActive = (to: number | ((at: number) => number)) => {
    const at = typeof to === "function" ? to(active()) : to;
    setActiveKey(options()[at]?.id ?? null);
  };
  /** Words shown in the list when it has no option to pick. */
  const notice = createMemo(() => {
    const t = trigger();
    if (!t || dismissed() === t.start || options().length) return null;
    if (t.kind === "file") {
      if (!t.query) return "Type to search project files";
      if (loading() || files()?.query !== t.query) return "Searching project files…";
      return "No matching files";
    }
    return null;
  });
  const open = () => options().length > 0 || notice() !== null;

  const replace = (t: Trigger, insert: string) => {
    const el = props.field(), text = props.text();
    const caret = el?.selectionStart ?? text.length;
    let end = Math.max(caret, t.start + 1 + t.query.length);
    while (end < text.length && !/\s/.test(text[end])) end++;
    const tail = text.slice(end);
    const glue = insert && !/^\s/.test(tail) ? " " : "";
    const next = text.slice(0, t.start) + insert + glue + (insert ? tail : tail.replace(/^ /, ""));
    const at = t.start + insert.length + glue.length;
    props.setText(next);
    setTrigger(null); asked++; clearTimeout(timer);
    queueMicrotask(() => { const f = props.field(); if (f) { f.focus(); f.setSelectionRange(at, at); } });
  };
  const accept = (option: Option | undefined) => {
    const t = trigger();
    if (!option || !t || option.why) return;
    if (option.kind === "file") { const id = opts()?.projectId; if (id) remember(id, option.file!); replace(t, mention(option.file!)); }
    else if (option.kind === "skill") replace(t, skillText(option.skill!));
    else { const run = (opts()?.commands?.() ?? []).find(c => c.name === option.command!.name) ?? option.command!; replace(t, ""); run.run(); }
  };
  const key = (e: KeyboardEvent): boolean => {
    if (e.isComposing || e.keyCode === 229 || !open()) return false;
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); setDismissed(trigger()?.start ?? null); return true; }
    const list = options();
    if (!list.length || e.altKey || e.ctrlKey || e.metaKey) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setActive(i => (i + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
      return true;
    }
    if ((e.key === "Enter" && !e.shiftKey) || (e.key === "Tab" && !e.shiftKey)) { e.preventDefault(); accept(list[Math.min(active(), list.length - 1)]); return true; }
    return false;
  };
  const activeId = () => open() && options().length ? `${listId}-${Math.min(active(), options().length - 1)}` : undefined;
  return {
    listId, options, notice, open, active, setActive, accept, key, activeId, trigger,
    onInput: () => update(),
    /** The caret moved without typing: a click, or the arrow keys the list did not use. */
    onCaret: () => update(),
    onCompositionStart: () => { composing = true; },
    onCompositionEnd: () => { composing = false; update(); },
    close: () => { setTrigger(null); asked++; clearTimeout(timer); },
  };
}
export type Suggest = ReturnType<typeof createSuggest>;

/**
 * The list, in the top layer above the field, so opening it never moves anything. The field keeps focus and the keyboard;
 * a press picks an option without taking focus away from it.
 */
export function SuggestList(props: { s: Suggest; field: () => HTMLTextAreaElement | undefined }) {
  let pop: HTMLDivElement | undefined;
  // Placed against the field each time what it lists changes, as the field may have grown a line meanwhile.
  const fit = () => queueMicrotask(() => {
    const field = props.field();
    if (!pop?.isConnected || !field) return;
    const top = pop as HTMLDivElement & { showPopover?: () => void; matches(s: string): boolean };
    try { if (!top.matches(":popover-open")) top.showPopover?.(); } catch { /* a window with no top layer shows it in place */ }
    place(pop, field, "left");
    pop.style.minWidth = `${Math.min(field.getBoundingClientRect().width, 520)}px`;
    keepInside(pop);
  });
  const show = (el: HTMLDivElement) => { pop = el; fit(); };
  createEffect(() => { props.s.options(); props.s.notice(); fit(); });
  const heading = (o: Option, i: number) => {
    const list = props.s.options();
    if (i > 0 && list[i - 1].kind === o.kind) return null;
    return o.kind === "file" ? null : o.kind === "command" ? "Commands" : "Project skills";
  };
  return <Show when={props.s.open()}>
    <div ref={show} popover="manual" class="ws-suggest" onMouseDown={e => e.preventDefault()}>
      <Show when={props.s.notice()}>{n => <div class="ws-suggest-note cap muted" role="status">{n()}</div>}</Show>
      <Show when={props.s.options().length}>
        <div id={props.s.listId} role="listbox" aria-label={props.s.trigger()?.kind === "file" ? "Project files" : "Commands and skills"}>
          <For each={props.s.options()}>{(o, i) => <>
            <Show when={heading(o, i())}>{h => <div class="mi-h" role="presentation">{h()}</div>}</Show>
            <div id={`${props.s.listId}-${i()}`} role="option" class="mi ws-suggest-o" aria-selected={i() === Math.min(props.s.active(), props.s.options().length - 1) ? "true" : "false"}
              aria-disabled={o.why ? "true" : undefined} data-why={o.why}
              onMouseEnter={() => props.s.setActive(i())} onClick={() => props.s.accept(o)}>
              <I n={o.kind === "file" ? "file" : o.kind === "command" ? "play" : "sparkles"} />
              <span class="grow trunc"><span class={o.kind === "file" ? "mono" : undefined}>{o.label}</span><Show when={o.sub || o.why}><span class="sub trunc">{o.why ?? o.sub}</span></Show></span>
            </div>
          </>}</For>
        </div>
      </Show>
    </div>
  </Show>;
}
