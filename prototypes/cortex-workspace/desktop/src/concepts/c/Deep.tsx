import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import type { Task } from "../../fixtures";
import { useC, uid, type DeepSurface } from "./ctx";
import { clock, eventsForExecution, eventsForTask, executionFor, executionGlyph, executionLabel, executionTone, fileDiffs, waitLabel, type FixtureEvent } from "./model";
import { Actions, Badge, MockButton, Receipt, VirtualList } from "./ui";

const TITLES: Record<DeepSurface, string> = {
  discussion: "Full discussion", diff: "Full diff", code: "Code", git: "Git", worker: "Worker detail",
};
const KIND: Record<string, string> = { discussion: "Discussion", plan: "Plan (reported)", check: "Check record", snapshot: "Snapshot" };

/** L3 deep mode: full-width surface; the queue collapses to the rail. */
export function DeepView(p: { surface: DeepSurface; task: Task | null; onSurface: (s: DeepSurface) => void }) {
  return <section class="c-deep" aria-labelledby="c-deep-heading">
    <div class="c-deep-head">
      <h2 id="c-deep-heading" tabindex="-1">{TITLES[p.surface]}<Show when={p.task}>{t => <span class="c-deep-task"> · {t().title}</span>}</Show></h2>
      <nav class="c-deep-tabs" aria-label="Deep mode surfaces">
        <For each={Object.keys(TITLES) as DeepSurface[]}>{s =>
          <button type="button" class="c-tab" aria-current={p.surface === s ? "page" : undefined} onClick={() => p.onSurface(s)}>{TITLES[s]}</button>
        }</For>
      </nav>
    </div>
    <Switch>
      <Match when={p.surface === "discussion"}><Discussion task={p.task} /></Match>
      <Match when={p.surface === "diff"}><Diff /></Match>
      <Match when={p.surface === "code"}><Code /></Match>
      <Match when={p.surface === "git"}><Git /></Match>
      <Match when={p.surface === "worker"}><Worker task={p.task} /></Match>
    </Switch>
  </section>;
}

function NoTask() {
  return <p class="c-muted">Select a task in the attention queue or All tasks first.</p>;
}

function EventLine(p: { e: FixtureEvent; showTask?: boolean }) {
  return <span class="c-evline"><span class="c-mono">#{p.e.sequence} · {clock(p.e.occurredAt)}</span>
    <Show when={p.showTask}>{" "}· <span class="c-mono">{p.e.taskId}</span></Show> · {KIND[p.e.kind] ?? p.e.kind} · {p.e.text} <span class="c-prov">· fixture</span></span>;
}

function Discussion(p: { task: Task | null }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const [history, setHistory] = createSignal(false);
  const events = () => p.task ? eventsForTask(f(), p.task.id) : [];
  return <div class="c-deep-body">
    <Show when={p.task} fallback={<NoTask />}>
      <h3 class="c-h3">Task timeline ({events().length} entries · fixture)</h3>
      <Show when={events().length} fallback={<p class="c-muted">No timeline entries for this task. Nothing has run.</p>}>
        <ol class="c-timeline"><For each={events()}>{e => <li><EventLine e={e} /></li>}</For></ol>
      </Show>
    </Show>
    <h3 class="c-h3">Project history</h3>
    <button type="button" class="c-btn" aria-expanded={history()} aria-controls="c-history" onClick={() => setHistory(!history())}>
      {history() ? "Hide project history" : `Show project history (${f().events.length.toLocaleString("en-US")} events)`}
    </button>
    <div id="c-history">
      <Show when={history()}>
        <p class="c-muted">Windowed list: only visible rows are rendered. Scroll with the mouse, or focus the list and use arrow keys, Page Up/Down, Home and End.</p>
        <VirtualList items={f().events} rowEm={2} heightRem={22} scale={c.textScale()}
          label={`Project history, ${f().events.length} events`} row={e => <EventLine e={e} showTask />} />
      </Show>
    </div>
  </div>;
}

function Diff() {
  const c = useC();
  const diffs = () => fileDiffs(c.store.state().fixture);
  return <div class="c-deep-body">
    <p class="c-muted">Synthetic diff derived from the fixture file record (base → local). Side by side when there is room, otherwise unified.</p>
    <For each={diffs()}>{d => <section class="c-diff" aria-label={`Diff of ${d.file.path}`}>
      <h3 class="c-h3 c-mono">{d.file.path} <span class="c-muted">· Modified · +1 −1</span></h3>
      <div class="c-diff-split" role="table" aria-label="Side-by-side diff">
        <div role="row" class="c-diff-row c-diff-headrow"><span role="columnheader">Base</span><span role="columnheader">Local</span></div>
        <div role="row" class="c-diff-row">
          <span role="cell" class="c-del"><span aria-hidden="true">− </span><span class="c-sr">Removed: </span>{d.lines[0].text}</span>
          <span role="cell" class="c-add"><span aria-hidden="true">+ </span><span class="c-sr">Added: </span>{d.lines[1].text}</span>
        </div>
      </div>
      <ol class="c-diff-unified" aria-label="Unified diff">
        <For each={d.lines}>{line => <li class={line.kind === "add" ? "c-add" : "c-del"}>
          <span aria-hidden="true">{line.kind === "add" ? "+ " : "− "}</span><span class="c-sr">{line.kind === "add" ? "Added: " : "Removed: "}</span>{line.text}
        </li>}</For>
      </ol>
    </section>}</For>
  </div>;
}

function Code() {
  const c = useC();
  const f = () => c.store.state().fixture;
  const file = () => f().files[0];
  const [query, setQuery] = createSignal("");
  const [regex, setRegex] = createSignal(false);
  const [compare, setCompare] = createSignal(false);
  const [text, setText] = createSignal(file()?.local ?? "");
  const [log, setLog] = createSignal<string[]>([]);
  const [command, setCommand] = createSignal("");
  const searchId = uid("search");
  const conflicted = () => file()?.state === "conflict" && !c.store.state().conflictResolution;
  const results = createMemo<{ error: string | null; paths: string[] }>(() => {
    const q = query().trim();
    const paths = f().files.map(x => x.path);
    if (!q) return { error: null, paths };
    if (regex()) {
      try { const re = new RegExp(q); return { error: null, paths: paths.filter(x => re.test(x)) }; }
      catch { return { error: "Invalid regular expression. Nothing was searched.", paths: [] }; }
    }
    const needle = q.replace(/\*/g, "").toLowerCase();
    return { error: null, paths: paths.filter(x => x.toLowerCase().includes(needle)) };
  });
  const saveReason = () => c.blocked() ?? (conflicted() ? "Save is disabled while the file conflict is unresolved. Use Compare, Reload or Save as… (never overwrite)." : null);
  const resolve = (choice: "local" | "incoming") => {
    const outcome = c.store.resolveConflict(choice);
    if (choice === "incoming") setText(file()?.incoming ?? "");
    c.setReceipt("code", `Mock outcome: ${outcome.message}`);
    c.announce(`Mock ${choice} resolution recorded.`);
    queueMicrotask(() => document.getElementById("c-editor-text")?.focus());
  };
  const run = (e: SubmitEvent) => {
    e.preventDefault();
    if (c.blocked() || !command().trim()) return;
    setLog([...log(), `$ ${command()}`, "Mock terminal: command not executed. No process was started."]);
    setCommand("");
  };
  return <div class="c-deep-body c-code" classList={{ "with-terminal": c.terminalOpen() }}>
    <div class="c-tree" role="region" aria-label="Files and search">
      <label for={searchId} class="c-label">Search files</label>
      <input id={searchId} type="search" value={query()} onInput={e => setQuery(e.currentTarget.value)} />
      <label class="c-check"><input type="checkbox" checked={regex()} onChange={e => setRegex(e.currentTarget.checked)} /> Regex</label>
      <Show when={!results().error} fallback={<p class="c-error-text" role="alert">{results().error}</p>}>
        <Show when={results().paths.length} fallback={<div role="status"><p>No files match <code>{query()}</code>. Try fewer characters or turn off Regex.</p>
          <button type="button" class="c-btn" onClick={() => setQuery("")}>Clear query</button></div>}>
          <p class="c-muted">{query() ? `${results().paths.length} match` : "Files (fixture)"}</p>
          <ul class="c-plain c-files"><For each={results().paths}>{path => <li>
            <span class="c-muted c-mono">{path.split("/").slice(0, -1).join("/")}/</span>
            <button type="button" class="c-link c-mono" aria-current={path === file()?.path ? "true" : undefined}>{path.split("/").pop()}</button>
            <Show when={f().files.find(x => x.path === path)?.state === "conflict"}> <Badge tone="danger" glyph="≠" label="Conflict" /></Show>
          </li>}</For></ul>
        </Show>
      </Show>
    </div>
    <div class="c-editor">
      <Show when={file()}>{fl => <>
        <Show when={conflicted()} fallback={<Show when={c.store.state().conflictResolution}>
          <p class="c-banner success" role="status">Mock resolution recorded: {c.store.state().conflictResolution}. No file or Git mutation occurred.</p></Show>}>
          <div class="c-banner danger" role="note">
            <p><Badge tone="danger" glyph="≠" label="File conflict" />{" "}This file changed on disk since you opened it (fixture incoming change). Save is disabled; nothing is overwritten.</p>
            <Actions reason={c.blocked()}>{reasonId => <>
              <button type="button" class="c-btn" aria-expanded={compare()} aria-controls="c-compare" onClick={() => setCompare(!compare())}>Compare</button>
              <MockButton label="Reload" reason={c.blocked()} reasonId={reasonId} onAct={() => c.setReceipt("code", "Mock outcome: reload recorded in memory only. Nothing was read from disk.")} />
              <MockButton label="Save as…" reason={c.blocked()} reasonId={reasonId} onAct={() => c.setReceipt("code", "Mock outcome: no file was written.")} />
            </>}</Actions>
          </div>
        </Show>
        <div id="c-compare">
          <Show when={compare()}>
            <div class="c-compare" role="group" aria-label="Compare versions">
              <section><h3 class="c-h4">Base</h3><pre class="c-pre">{fl().base}</pre></section>
              <section><h3 class="c-h4">Local (yours)</h3><pre class="c-pre">{fl().local}</pre></section>
              <section><h3 class="c-h4">Incoming (disk)</h3><pre class="c-pre">{fl().incoming}</pre></section>
            </div>
            <Show when={conflicted()}>
              <Actions reason={c.blocked()}>{reasonId => <>
                <MockButton label="Keep local" reason={c.blocked()} reasonId={reasonId} onAct={() => resolve("local")} />
                <MockButton label="Take incoming" reason={c.blocked()} reasonId={reasonId} onAct={() => resolve("incoming")} />
              </>}</Actions>
            </Show>
          </Show>
        </div>
        <label class="c-label" for="c-editor-text">Editor: <span class="c-mono">{fl().path}</span> (in memory)</label>
        <textarea id="c-editor-text" class="c-editor-text" rows={8} value={text()} onInput={e => setText(e.currentTarget.value)} spellcheck={false} />
        <Actions reason={saveReason()}>{reasonId => <>
          <MockButton label="Save" tone="primary" reason={saveReason()} reasonId={reasonId} onAct={() => c.setReceipt("code", "Mock outcome: nothing was written to disk.")} />
          <button type="button" class="c-btn" aria-expanded={c.terminalOpen()} aria-controls="c-terminal" aria-keyshortcuts="Control+`" onClick={() => c.setTerminalOpen(!c.terminalOpen())}>Terminal <span class="c-kbd" aria-hidden="true">Ctrl+`</span></button>
          <button type="button" class="c-btn" onClick={() => c.openDeep("git")}>Open Git</button>
        </>}</Actions>
        <Receipt rkey="code" />
      </>}</Show>
    </div>
    <Show when={c.terminalOpen()}>
      <section id="c-terminal" class="c-terminal" aria-label="Mock terminal">
        <h3 class="c-h4">Terminal (mock, no process)</h3>
        <ol class="c-plain c-mono c-term-log" aria-live="polite"><For each={log()}>{line => <li>{line}</li>}</For></ol>
        <form onSubmit={run} class="c-term-form">
          <label for="c-term-input" class="c-label">Command</label>
          <input id="c-term-input" class="c-mono" value={command()} onInput={e => setCommand(e.currentTarget.value)} aria-describedby={c.blocked() ? "c-term-reason" : undefined} />
          <button type="submit" class="c-btn" aria-label="Run (mock)" aria-disabled={c.blocked() ? true : undefined}>Run<span class="c-mock-tag" aria-hidden="true"> · Mock</span></button>
          <Show when={c.blocked()}><p id="c-term-reason" class="c-reason">{c.blocked()}</p></Show>
        </form>
      </section>
    </Show>
  </div>;
}

function Git() {
  const c = useC();
  const f = () => c.store.state().fixture;
  const files = () => f().files.filter(x => (f().git.fileIds as readonly string[]).includes(x.id));
  const resolved = () => !!c.store.state().conflictResolution;
  const abort = async () => {
    const ok = await c.confirm({
      title: "Abort merge? (mock)", effect: `Would stop the merge on ${f().git.branch} and restore the pre-merge state.`,
      scope: `${files().length} conflicted file(s): ${files().map(x => x.path).join(", ")}. This is a mock: no Git command runs.`, confirmLabel: "Abort merge",
    });
    c.setReceipt("git", ok ? "Mock outcome: abort recorded in memory only. No Git command ran." : "Abort cancelled. Nothing changed.");
  };
  return <div class="c-deep-body">
    <div class="c-banner danger" role="note">
      <p><Badge tone="danger" glyph="≠" label="Git conflict" />{" "}Merge stopped: {files().length} file has conflicts on <span class="c-mono">{f().git.branch}</span>. {f().git.detail} No auto-stash or reset is performed.</p>
    </div>
    <h3 class="c-h3">Conflicted files ({files().length})</h3>
    <ul class="c-plain c-gitfiles"><For each={files()}>{file => <li>
      <p><span class="c-mono">{file.path}</span> · <Badge tone="danger" glyph="≠" label={resolved() ? "Conflict (mock choice recorded)" : "Conflict"} /></p>
      <Actions reason={c.blocked()}>{reasonId => <>
        <button type="button" class="c-btn" onClick={() => c.openDeep("code")}>Open file</button>
        <MockButton label="Mark resolved" reason={c.blocked() ?? (resolved() ? null : "Choose Keep local or Take incoming in the editor's Compare first.")} reasonId={reasonId}
          onAct={() => c.setReceipt("git", "Mock outcome: marked resolved in memory only. No Git command ran.")} />
        <MockButton label="Abort merge" tone="danger" reason={c.blocked()} reasonId={reasonId} onAct={() => void abort()} />
      </>}</Actions>
    </li>}</For></ul>
    <h3 class="c-h3">Staged changes</h3>
    <p class="c-muted">None (fixture).</p>
    <Actions reason={c.blocked() ?? "Resolve conflicts before committing."}>{reasonId => <>
      <MockButton label="Commit" reason={c.blocked() ?? "Resolve conflicts before committing."} reasonId={reasonId} onAct={() => undefined} />
      <MockButton label="Push" reason={c.blocked() ?? "Resolve conflicts before committing."} reasonId={reasonId} onAct={() => undefined} />
    </>}</Actions>
    <Receipt rkey="git" />
  </div>;
}

function Worker(p: { task: Task | null }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const e = () => p.task ? executionFor(f(), p.task.id) : null;
  return <div class="c-deep-body">
    <Show when={p.task} fallback={<NoTask />}>
      <Show when={e()} fallback={<p class="c-muted">No execution for this task. Nothing has run; this prototype never starts an agent.</p>}>{x => <>
        <dl class="c-facts compact">
          <div><dt>Execution</dt><dd class="c-mono">{x().id}</dd></div>
          <div><dt>Session</dt><dd class="c-mono">{x().sessionId}</dd></div>
          <div><dt>State</dt><dd><Badge tone={executionTone(executionLabel(x(), c.conn()))} glyph={executionGlyph(executionLabel(x(), c.conn()))} label={executionLabel(x(), c.conn())} /> · fixture</dd></div>
          <div><dt>Wait reason</dt><dd>{waitLabel(x()) ?? "None"}</dd></div>
        </dl>
        <h3 class="c-h3">Execution events ({eventsForExecution(f(), x().id).length} · fixture)</h3>
        <ol class="c-timeline"><For each={eventsForExecution(f(), x().id)}>{ev => <li><EventLine e={ev} /></li>}</For></ol>
      </>}</Show>
    </Show>
  </div>;
}
