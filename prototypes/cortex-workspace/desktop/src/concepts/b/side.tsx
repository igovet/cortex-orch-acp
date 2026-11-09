import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { LOCAL_DATA_NOTE, treeRows } from "./data";
import type { BModel, NeedKind, SideView, Task } from "./model";
import { ActionRow, Badge, Glyph, MockButton, Receipts, TaskListState, VirtualListbox, type GlyphName } from "./ui";

export const VIEWS: { id: SideView; label: string; keys: string; glyph: GlyphName }[] = [
  { id: "files", label: "Files", keys: "Ctrl+Shift+E", glyph: "files" },
  { id: "search", label: "Search", keys: "Ctrl+Shift+F", glyph: "search" },
  { id: "git", label: "Git", keys: "Ctrl+Shift+G", glyph: "branch" },
  { id: "mission", label: "Mission Control", keys: "Ctrl+Shift+M", glyph: "mission" },
  { id: "board", label: "Board", keys: "Ctrl+Shift+B", glyph: "board" },
  { id: "settings", label: "Settings", keys: "", glyph: "gear" },
];

export function ActivityBar(props: { m: BModel }) {
  const m = props.m;
  const [roving, setRoving] = createSignal(0);
  const refs: HTMLButtonElement[] = [];
  const onKey = (e: KeyboardEvent, i: number) => {
    const n = VIEWS.length;
    const next = e.key === "ArrowDown" ? (i + 1) % n : e.key === "ArrowUp" ? (i - 1 + n) % n : e.key === "Home" ? 0 : e.key === "End" ? n - 1 : -1;
    if (next < 0) return;
    e.preventDefault();
    setRoving(next);
    refs[next].focus();
  };
  const name = (v: typeof VIEWS[number]) => v.id === "mission" && m.needsYou().length
    ? `${v.label}, ${m.needsYou().length} decision${m.needsYou().length === 1 ? "" : "s"} need you` : v.label;
  return <nav id="cb-activity" class="cb-activity" aria-label="Views" tabindex="-1">
    <For each={VIEWS}>{(v, i) =>
      <button ref={el => (refs[i()] = el)} type="button" class="cb-act" tabindex={roving() === i() ? 0 : -1}
        aria-pressed={m.ui.sideVisible && m.ui.side === v.id} aria-label={name(v)}
        data-tip={v.keys ? `${v.label} (${v.keys})` : v.label}
        onClick={() => m.showSide(v.id)} onKeyDown={e => onKey(e, i())} onFocus={() => setRoving(i())}>
        <Glyph name={v.glyph} />
        <Show when={v.id === "mission" && m.needsYou().length > 0}><span class="cb-count" aria-hidden="true">{m.needsYou().length}</span></Show>
      </button>}
    </For>
  </nav>;
}

export function SidePanel(props: { m: BModel }) {
  const m = props.m;
  const title = () => VIEWS.find(v => v.id === m.ui.side)!.label;
  return <aside id="cb-side" class="cb-side" aria-labelledby="cb-side-title" tabindex="-1">
    <div class="cb-panel-h">
      <h2 id="cb-side-title" tabindex="-1">{title()}</h2>
      <button type="button" class="cb-icon-btn" aria-label="Hide side panel (Ctrl+B)" title="Hide side panel (Ctrl+B)" onClick={() => m.toggleSide()}><Glyph name="close" /></button>
    </div>
    <div class="cb-side-body">
      <Switch>
        <Match when={m.ui.side === "files"}><FilesView m={m} /></Match>
        <Match when={m.ui.side === "search"}><SearchView m={m} /></Match>
        <Match when={m.ui.side === "git"}><GitView m={m} /></Match>
        <Match when={m.ui.side === "mission"}><MissionView m={m} /></Match>
        <Match when={m.ui.side === "board"}><BoardSideView m={m} /></Match>
        <Match when={m.ui.side === "settings"}><SettingsView m={m} /></Match>
      </Switch>
    </div>
  </aside>;
}

function FilesView(props: { m: BModel }) {
  const m = props.m;
  const rows = createMemo(() => treeRows(m.files().map(f => f.path), m.ui.expanded));
  const [cur, setCur] = createSignal(m.ui.g1Active ?? "src");
  const refs = new Map<string, HTMLLIElement>();
  const focusRow = (path: string) => { setCur(path); refs.get(path)?.focus(); };
  const tabStop = (path: string, i: number) => cur() === path || (i === 0 && !rows().some(r => r.path === cur()));
  const onKey = (e: KeyboardEvent, i: number) => {
    const list = rows(); const r = list[i];
    switch (e.key) {
      case "ArrowDown": if (i < list.length - 1) focusRow(list[i + 1].path); break;
      case "ArrowUp": if (i > 0) focusRow(list[i - 1].path); break;
      case "Home": focusRow(list[0].path); break;
      case "End": focusRow(list[list.length - 1].path); break;
      case "ArrowRight":
        if (r.dir && !m.ui.expanded[r.path]) m.setExpanded(r.path, true);
        else if (r.dir && list[i + 1] && list[i + 1].level > r.level) focusRow(list[i + 1].path);
        break;
      case "ArrowLeft": {
        if (r.dir && m.ui.expanded[r.path]) { m.setExpanded(r.path, false); break; }
        const parent = r.path.split("/").slice(0, -1).join("/");
        if (parent) focusRow(parent);
        break;
      }
      case "Enter": case " ":
        if (r.dir) m.setExpanded(r.path, !m.ui.expanded[r.path]); else m.openFile(r.path);
        break;
      default: return;
    }
    e.preventDefault();
  };
  return <>
    <p class="cb-note">Synthetic workspace. {LOCAL_DATA_NOTE}</p>
    <ul class="cb-tree" role="tree" aria-label="Workspace files">
      <For each={rows()}>{(r, i) =>
        <li role="treeitem" ref={el => refs.set(r.path, el)} tabindex={tabStop(r.path, i()) ? 0 : -1} class="cb-tree-row"
          aria-level={r.level} aria-expanded={r.dir ? !!m.ui.expanded[r.path] : undefined}
          aria-selected={!r.dir && m.ui.g1Active === r.path}
          style={{ "padding-left": `${0.25 + (r.level - 1) * 0.875}rem` }}
          onClick={() => { setCur(r.path); if (r.dir) m.setExpanded(r.path, !m.ui.expanded[r.path]); else m.openFile(r.path); }}
          onKeyDown={e => onKey(e, i())}>
          <span class="cb-twisty" aria-hidden="true">{r.dir ? (m.ui.expanded[r.path] ? "▾" : "▸") : ""}</span>
          <span class="cb-tree-name">{r.name}</span>
          <Show when={!r.dir && m.dirty(r.path)}><span class="cb-tag">Unsaved</span></Show>
          <Show when={!r.dir && m.isConflicted(r.path)}><span class="cb-tag t-warn">Conflict</span></Show>
        </li>}
      </For>
    </ul>
  </>;
}

function SearchView(props: { m: BModel }) {
  const m = props.m;
  return <div class="cb-pad cb-stack">
    <label class="cb-field">Search files
      <input id="cb-search-input" type="search" value={m.ui.query} onInput={e => m.setQuery(e.currentTarget.value)} autocomplete="off" />
    </label>
    <label class="cb-check"><input type="checkbox" checked={m.ui.regex} onChange={e => m.setRegex(e.currentTarget.checked)} /> Regex</label>
    <Show when={m.ui.query} fallback={<p class="cb-muted">Type to search the synthetic workspace. Results open in editor group 1.</p>}>
      <Switch>
        <Match when={m.search().error}>
          <div class="cb-banner t-danger" role="note"><Glyph name="x" /><p>{m.search().error}</p></div>
        </Match>
        <Match when={m.search().hits.length === 0}>
          <div class="cb-empty">
            <p>No files match <code>{m.ui.query}</code>. Try fewer characters or turn off Regex.</p>
            <button type="button" class="cb-btn" onClick={() => { m.setQuery(""); document.getElementById("cb-search-input")?.focus(); }}>Clear query</button>
          </div>
        </Match>
        <Match when={true}>
          <p class="cb-muted" role="status">{m.search().hits.length} match{m.search().hits.length === 1 ? "" : "es"}</p>
          <ul class="cb-plain">
            <For each={m.search().hits}>{h =>
              <li><button type="button" class="cb-row-btn" onClick={() => m.openFile(h.path, h.line)}>
                <span class="cb-mono">{h.path}{h.line ? `:${h.line}` : ""}</span>
                <span class="cb-muted cb-hit">{h.text}</span>
              </button></li>}
            </For>
          </ul>
        </Match>
      </Switch>
    </Show>
  </div>;
}

function GitView(props: { m: BModel }) {
  const m = props.m;
  const git = () => m.fx().git;
  const conflicts = () => m.gitConflicts();
  return <div class="cb-pad cb-stack">
    <p class="cb-muted">Worktree: Synthetic Mosaic (fixture) · branch <span class="cb-mono">{git().branch}</span></p>
    <Show when={conflicts().length > 0}>
      <div class="cb-banner t-danger" role="note"><Glyph name="warn" />
        <p>Merge stopped: {conflicts().length} file{conflicts().length === 1 ? " has" : "s have"} conflicts on <code>{git().branch}</code>. {git().detail} No auto-stash or reset.</p>
      </div>
    </Show>
    <section aria-labelledby="cb-git-conflicted" class="cb-stack-s">
      <h3 id="cb-git-conflicted" class="cb-group-h">Conflicted ({m.unresolved().length})</h3>
      <Show when={conflicts().length} fallback={<p class="cb-muted">No conflicted files in this fixture state.</p>}>
        <ul class="cb-plain cb-stack-s">
          <For each={conflicts()}>{f =>
            <li class="cb-git-row">
              <span class="cb-mono">{f.path}</span>
              <Show when={m.ui.gitResolved[f.id]} fallback={<Badge tone="warn" glyph="warn" text="Conflict" prov="fixture" />}>
                <Badge tone="mock" glyph="flask" text="Marked resolved (mock)" />
              </Show>
              <div class="cb-actions-btns">
                <button type="button" class="cb-btn" onClick={() => m.openFile(f.path)}>Open file</button>
                <button type="button" class="cb-btn" onClick={() => m.openMerge()}>Compare</button>
                <MockButton label="Mark resolved" disabled={!!m.block() || !!m.ui.gitResolved[f.id]} onClick={() => m.markResolved(f.id, f.path)} />
              </div>
            </li>}
          </For>
        </ul>
        <ActionRow reason={m.block()}><MockButton label="Abort merge" disabled={!!m.block()} onClick={m.askAbortMerge} /></ActionRow>
      </Show>
    </section>
    <section aria-labelledby="cb-git-staged" class="cb-stack-s">
      <h3 id="cb-git-staged" class="cb-group-h">Staged ({m.ui.staged.length})</h3>
      <Show when={m.ui.staged.length} fallback={<p class="cb-muted">Nothing staged.</p>}>
        <ul class="cb-plain"><For each={m.ui.staged}>{p => <li class="cb-mono">{p}</li>}</For></ul>
      </Show>
    </section>
    <section aria-labelledby="cb-git-unstaged" class="cb-stack-s">
      <h3 id="cb-git-unstaged" class="cb-group-h">Unstaged ({m.unstaged().length})</h3>
      <Show when={m.unstaged().length} fallback={<p class="cb-muted">No unstaged changes.</p>}>
        <ul class="cb-plain cb-stack-s">
          <For each={m.unstaged()}>{p =>
            <li class="cb-git-row">
              <span class="cb-mono">{p}</span><span class="cb-muted">Changed in working tree</span>
              <div class="cb-actions-btns">
                <button type="button" class="cb-btn" onClick={() => m.openFile(p)}>Open file</button>
                <MockButton label={`Stage ${p.split("/").pop()}`} disabled={!!m.block()} onClick={() => m.stage(p)} />
              </div>
            </li>}
          </For>
        </ul>
      </Show>
    </section>
    <section aria-labelledby="cb-git-commit" class="cb-stack-s">
      <h3 id="cb-git-commit" class="cb-group-h">Commit</h3>
      <label class="cb-field">Commit message
        <textarea rows="2" value={m.ui.commitMessage} onInput={e => m.setUi("commitMessage", e.currentTarget.value)} />
      </label>
      <ActionRow reason={m.commitBlock()}><MockButton label="Commit" primary disabled={!!m.commitBlock()} onClick={m.commit} /></ActionRow>
    </section>
    <section aria-labelledby="cb-git-remote" class="cb-stack-s">
      <h3 id="cb-git-remote" class="cb-group-h">Branch and remote</h3>
      <label class="cb-field">Branch
        <select disabled aria-describedby="cb-branch-why"><option>{git().branch}</option></select>
      </label>
      <p id="cb-branch-why" class="cb-reason">Branch switching is not part of this prototype.</p>
      <ActionRow reason={m.block()}>
        <MockButton label="Pull" disabled={!!m.block()} onClick={m.pull} />
        <MockButton label="Push" disabled={!!m.block()} onClick={m.askPush} />
      </ActionRow>
      <Show when={m.ui.pushUnknown}>
        <div class="cb-banner t-neutral" role="note"><Glyph name="question" />
          <p><strong>Outcome unknown.</strong> Push to <code>origin/{git().branch}</code> did not confirm. Check the remote before trying again.</p>
          <MockButton label="Inspect remote" disabled={!!m.block()} onClick={m.inspectRemote} />
        </div>
      </Show>
    </section>
    <Receipts m={m} where="git" />
  </div>;
}

function McSection(props: { m: BModel; id: string; title: string; count: number | string; children: any }) {
  const open = () => !props.m.ui.collapsed[props.id];
  return <section class="cb-mc-sec" aria-labelledby={`cb-mc-${props.id}`}>
    <h3 class="cb-mc-h">
      <button type="button" id={`cb-mc-${props.id}`} class="cb-mc-toggle" aria-expanded={open()} aria-controls={`cb-mc-body-${props.id}`} onClick={() => props.m.toggleSection(props.id)}>
        <span aria-hidden="true">{open() ? "▾" : "▸"}</span> {props.title} <span class="cb-mc-count">({props.count})</span>
        <Show when={props.m.conn() !== "connected"}><span class="cb-muted"> · as of {props.m.lastConfirmed()}</span></Show>
      </button>
    </h3>
    <Show when={open()}><div id={`cb-mc-body-${props.id}`} class="cb-mc-body">{props.children}</div></Show>
  </section>;
}

function TaskRow(props: { m: BModel; taskId: string; sub: "discussion" | "decisions" | "result" | "workers"; title: string; badge?: any; needId?: string }) {
  const m = props.m;
  const active = () => m.ui.split && m.ui.g2Active === `task:${props.taskId}`;
  return <button type="button" class="cb-mc-row" classList={{ selected: active() }} data-need={props.needId}
    aria-current={active() ? "true" : undefined}
    onClick={() => m.openTask(props.taskId, false, props.sub)} onDblClick={() => m.openTask(props.taskId, true, props.sub)}
    onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); m.openTask(props.taskId, true, props.sub); } }}>
    <span class="cb-row-title cb-clamp2">{props.title}</span>
    <span class="cb-row-meta">{props.badge}<span class="cb-mono cb-muted">{props.taskId}</span></span>
  </button>;
}

const NEED_GLYPH: Record<NeedKind, GlyphName> = { Question: "message", Permission: "key", "Result acceptance": "filecheck" };

function MissionView(props: { m: BModel }) {
  const m = props.m;
  const recent = () => {
    const created = m.tasks().filter(t => m.isCreated(t.id));
    return [...created.reverse(), ...m.tasks().filter(t => !m.isCreated(t.id)).slice(0, 8)].slice(0, 10);
  };
  return <div class="cb-mc">
    <p class="cb-note">Select previews a task in group 2 (italic tab); Enter or double-click pins it. Nothing opens automatically.</p>
    <TaskListState m={m}>
      <McSection m={m} id="needs" title="Needs you" count={m.needsYou().length}>
        <Show when={m.needsYou().length} fallback={<p class="cb-empty">Nothing needs your decision. Last checked at fixture load · observed (fixture).</p>}>
          <For each={["Question", "Permission", "Result acceptance"] as NeedKind[]}>{kind =>
            <Show when={m.needsYou().some(n => n.kind === kind)}>
              <h4 class="cb-mc-group">{kind}</h4>
              <ul class="cb-plain">
                <For each={m.needsYou().filter(n => n.kind === kind)}>{n =>
                  <li><TaskRow m={m} taskId={n.taskId} sub={n.sub} title={n.title} needId={n.id}
                    badge={<Badge tone="warn" glyph={NEED_GLYPH[kind]} text={kind === "Question" ? "Your answer needed" : kind === "Permission" ? "Your permission needed" : "Result review needed"} />} /></li>}
                </For>
              </ul>
            </Show>}
          </For>
        </Show>
      </McSection>
      <McSection m={m} id="running" title="Running" count={m.conn() === "connected" ? m.running().length : "Unknown"}>
        <ul class="cb-plain">
          <For each={m.running()}>{e => {
            const info = () => m.execInfo(e.taskId);
            return <li><TaskRow m={m} taskId={e.taskId} sub="workers" title={m.taskMap().get(e.taskId)?.title ?? e.taskId}
              badge={<Badge tone={info().tone} glyph={info().glyph} text={info().text} prov={info().prov} />} /></li>;
          }}</For>
        </ul>
        <p class="cb-note">Elapsed time is not recorded in the fixture.</p>
      </McSection>
      <McSection m={m} id="changed" title="Changed since last visit" count={m.changed().length}>
        <Show when={m.changed().length} fallback={<p class="cb-muted">No changes since your last visit.</p>}>
          <ul class="cb-plain">
            <For each={m.changed()}>{c =>
              <li><TaskRow m={m} taskId={c.taskId} sub={c.sub} title={c.text}
                badge={<Badge tone={c.tone} glyph={c.tone === "danger" ? "x" : c.tone === "warn" ? "history" : "flask"} text={c.tone === "danger" ? "Failed" : c.tone === "warn" ? "Stale" : "Mock"} />} /></li>}
            </For>
          </ul>
        </Show>
      </McSection>
      <McSection m={m} id="recent" title="Recent tasks" count={recent().length}>
        <ul class="cb-plain">
          <For each={recent()}>{t => {
            const info = () => m.execInfo(t.id);
            return <li><TaskRow m={m} taskId={t.id} sub="discussion" title={t.title}
              badge={<Badge tone={info().tone} glyph={info().glyph} text={info().text} />} /></li>;
          }}</For>
        </ul>
      </McSection>
    </TaskListState>
    <div class="cb-mc-links">
      <button type="button" class="cb-btn" onClick={() => m.openBoard()}>Open board ({m.tasks().length})</button>
      <button type="button" class="cb-btn" onClick={() => m.openNewTask()}>New task…</button>
      <button type="button" class="cb-btn" onClick={() => m.openHistory()}>Project history ({m.events().length.toLocaleString("en-US")} events)</button>
    </div>
  </div>;
}

function BoardSideView(props: { m: BModel }) {
  const m = props.m;
  return <div class="cb-side-board">
    <div class="cb-pad cb-actions-btns">
      <button type="button" class="cb-btn" onClick={() => m.openBoard()}>Open board tab</button>
      <button type="button" class="cb-btn" onClick={() => m.openNewTask()}>New task…</button>
    </div>
    <p class="cb-note">Compact list · {m.tasks().length} tasks · phase is a runtime projection (fixture values); no drag and drop.</p>
    <TaskListState m={m}>
      <VirtualListbox<Task> id="cb-side-list" label={`Tasks (${m.tasks().length})`} items={m.tasks()} rowRem={4} rootPx={m.rootPx()}
        keyOf={t => t.id} selectedKey={m.ui.g2Active?.startsWith("task:") ? m.ui.g2Active.slice(5) : null}
        onActivate={(t, pin) => m.openTask(t.id, pin)}
        render={t => <TaskCell m={m} t={t} />} />
    </TaskListState>
  </div>;
}

export function TaskCell(props: { m: BModel; t: Task }) {
  const info = () => props.m.execInfo(props.t.id);
  return <div class="cb-cell">
    <span class="cb-cell-title cb-clamp2" title={props.t.title}>{props.t.title}</span>
    <span class="cb-cell-meta">
      <span class="cb-mono cb-muted">{props.t.id}</span>
      <span class="cb-muted">· {props.t.phase}</span>
      <span class={`cb-inline t-${info().tone}-text`}>· {info().text}</span>
      <Show when={info().wait}><span class="cb-inline t-warn-text">· {info().wait}</span></Show>
    </span>
  </div>;
}

function SettingsView(props: { m: BModel }) {
  const m = props.m;
  return <div class="cb-pad cb-stack">
    <button type="button" class="cb-btn" onClick={() => m.openSettings()}>Connection settings (opens in group 2)</button>
    <fieldset class="cb-fieldset">
      <legend>Text size</legend>
      <For each={[1, 1.25, 1.5, 2]}>{s =>
        <label class="cb-check"><input type="radio" name="cb-text-size" checked={m.ui.textScale === s} onChange={() => m.setTextScale(s)} /> {s * 100}%</label>}
      </For>
      <p class="cb-muted">Sets the root text size; all concept B sizes are in rem. At 200% hide the side panel with Ctrl+B when space is short.</p>
    </fieldset>
    <fieldset class="cb-fieldset">
      <legend>Keyboard</legend>
      <label class="cb-check"><input type="checkbox" checked={m.ui.charKeys} onChange={e => m.setUi("charKeys", e.currentTarget.checked)} /> Character-key shortcuts (<kbd>?</kbd> opens the keyboard reference)</label>
      <button type="button" class="cb-btn" onClick={() => m.setOverlay("keys")}>Keyboard reference (F1)</button>
    </fieldset>
    <fieldset class="cb-fieldset">
      <legend>Board layout</legend>
      <For each={[["auto", "Automatic (List at 476 px or narrower)"], ["board", "Board"], ["list", "List"]] as const}>{([v, label]) =>
        <label class="cb-check"><input type="radio" name="cb-board-mode" checked={m.ui.boardMode === v} onChange={() => m.setUi("boardMode", v)} /> {label}</label>}
      </For>
    </fieldset>
  </div>;
}
