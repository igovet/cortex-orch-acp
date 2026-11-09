import { createSignal, For, Show } from "solid-js";
import { useDesk } from "./ctx";
import { Glyph } from "./glyphs";
import { hhmm, type TaskView } from "./model";
import { Badge, MockButton, Receipt } from "./ui";

/** Code tab: file tree or search, editor with lease line and save conflict, fixture terminal. */
export function CodeTab(props: { taskId: string | null }) {
  const desk = useDesk();
  const fixture = () => desk.store.state().fixture;
  const file = () => fixture().files[0];
  const original = () => desk.ui.reloaded ? file().incoming : file().local;
  const text = () => desk.ui.editor[file().id] ?? original();
  const dirty = () => text() !== original();
  const resolution = () => desk.store.state().conflictResolution;
  const [saveAsPath, setSaveAsPath] = createSignal("");
  const [command, setCommand] = createSignal("");
  const matches = () => {
    const q = desk.ui.query;
    if (!q) return [];
    let test: (s: string) => boolean;
    if (desk.ui.regex) {
      try { const re = new RegExp(q, "i"); test = s => re.test(s); } catch { return "invalid" as const; }
    } else test = s => s.toLowerCase().includes(q.toLowerCase());
    return fixture().files.filter(f => test(f.path) || test(f.local));
  };
  const save = () => {
    if (file().state === "conflict" && !desk.ui.reloaded) { desk.setUi("saveConflict", true); desk.focusSoon("a-save-banner"); return; }
    desk.setReceipt("save", "Mock outcome: saved in memory only. No file was written.", true);
  };
  const clearEdit = () => desk.setUi("editor", e => { const next = { ...e }; delete next[file().id]; return next; });
  const runCommand = () => {
    const c = command().trim();
    if (!c) return;
    desk.setUi("terminal", t => [...t, `$ ${c}`, "Not executed: this is a fixture terminal (mock)."]);
    setCommand("");
  };
  return <div class="a-code-tab">
    <p class="a-worktree">{props.taskId
      ? <>Worktree for <span class="a-mono">{props.taskId}</span>: not modelled in the fixture; showing the shared fixture file.</>
      : <>Project code, not tied to a task. Showing the shared fixture file.</>}</p>
    <div class="a-code-grid">
      <nav class="a-tree" aria-label="Files and search">
        <div class="a-seg" role="group" aria-label="Side panel">
          <button type="button" aria-pressed={desk.ui.codeMode === "files"} onClick={() => desk.setUi("codeMode", "files")}>Files</button>
          <button type="button" aria-pressed={desk.ui.codeMode === "search"} onClick={() => desk.setUi("codeMode", "search")}>Search</button>
        </div>
        <Show when={desk.ui.codeMode === "search"} fallback={
          <ul class="a-tree-list" id="a-tree">
            <li><span class="a-muted">synthetic/</span>
              <ul><li><button type="button" class="a-tree-file" aria-current="true" id="a-tree-file" onClick={() => document.getElementById("a-editor-text")?.focus()}>
                <span>{file().path.split("/").pop()}</span> <span class="a-status-letter">U</span><span class="a-muted"> conflict</span>
              </button></li></ul>
            </li>
          </ul>}>
          <div class="a-search">
            <label for="a-search-input">Search files</label>
            <input id="a-search-input" type="search" value={desk.ui.query} onInput={e => desk.setUi("query", e.currentTarget.value)} />
            <label class="a-check"><input type="checkbox" checked={desk.ui.regex} onChange={e => desk.setUi("regex", e.currentTarget.checked)} /> Regex</label>
            <Show when={desk.ui.query}>
              <Show when={matches() !== "invalid"} fallback={<p class="a-error-text">Invalid regular expression. Nothing was searched.</p>}>
                <Show when={(matches() as unknown[]).length} fallback={<div class="a-empty">
                  <p>No files match <code>{desk.ui.query}</code>. Try fewer characters or turn off Regex.</p>
                  <button type="button" onClick={() => { desk.setUi("query", ""); document.getElementById("a-search-input")?.focus(); }}>Clear query</button>
                </div>}>
                  <ul class="a-tree-list"><For each={matches() as ReturnType<typeof fixture>["files"]}>{f =>
                    <li><button type="button" class="a-tree-file" onClick={() => document.getElementById("a-editor-text")?.focus()}>{f.path}</button></li>}</For></ul>
                </Show>
              </Show>
            </Show>
          </div>
        </Show>
      </nav>
      <section class="a-editor" aria-labelledby="a-ed-h">
        <div class="a-ed-head">
          <h3 id="a-ed-h" class="a-ed-tab"><span class="a-mono">{file().path}</span>{dirty() ? " · Unsaved" : ""}</h3>
          <span class="a-muted">{desk.block() ? `Editing paused: ${desk.block()}` : "You can edit · lease held (mock)"}</span>
        </div>
        <Show when={desk.ui.saveConflict}>
          <div class="a-banner a-banner-warn" id="a-save-banner" tabindex="-1" role="group" aria-label="File conflict">
            <Glyph name="history" />
            <p><strong>This file changed on disk since you opened it</strong> (agent edit in the fixture). Saving is paused; nothing will be overwritten.</p>
            <span class="a-actions">
              <button type="button" aria-expanded={desk.ui.compare} onClick={() => desk.setUi("compare", !desk.ui.compare)}>Compare</button>
              <button type="button" onClick={() => desk.confirm({
                title: "Reload the agent's version?", confirmLabel: "Reload",
                effect: "Replaces the editor text with the agent's version from the fixture. Your unsaved edits are discarded from memory. Nothing is written.",
                scope: file().path,
                onConfirm: () => { clearEdit(); desk.setUi({ reloaded: true, saveConflict: false, compare: false }); desk.setReceipt("save", "Reloaded the agent's version into the editor (memory only).", true); },
              })}>Reload</button>
              <button type="button" aria-expanded={desk.ui.saveAs} onClick={() => { desk.setUi("saveAs", !desk.ui.saveAs); setSaveAsPath(`${file().path}.mine`); }}>Save as…</button>
            </span>
          </div>
        </Show>
        <Show when={desk.ui.saveConflict && desk.ui.compare}>
          <section class="a-compare" aria-labelledby="a-cmp-h">
            <h4 id="a-cmp-h">Compare {file().path}</h4>
            <div class="a-compare-grid">
              <div><p class="a-diff-cap">Base</p><pre class="a-code">{file().base}</pre></div>
              <div><p class="a-diff-cap">Yours (editor)</p><pre class="a-code">{text()}</pre></div>
              <div><p class="a-diff-cap">Incoming (agent, fixture)</p><pre class="a-code">{file().incoming}</pre></div>
            </div>
            <div class="a-actions">
              <MockButton label="Keep mine" reason={desk.block()} onClick={() => desk.setReceipt("resolve", `Mock outcome: ${desk.store.resolveConflict("local").message}`, true)} />
              <MockButton label="Keep incoming" reason={desk.block()} onClick={() => desk.setReceipt("resolve", `Mock outcome: ${desk.store.resolveConflict("incoming").message}`, true)} />
            </div>
            <Show when={resolution()}><p class="a-state">Chosen in memory: {resolution() === "local" ? "mine" : "incoming"} (mock). The file and Git are unchanged; mark it resolved in the Git tab.</p></Show>
            <Receipt k="resolve" />
          </section>
        </Show>
        <Show when={desk.ui.saveConflict && desk.ui.saveAs}>
          <div class="a-saveas">
            <label for="a-saveas-input">New path</label>
            <input id="a-saveas-input" class="a-mono" value={saveAsPath()} onInput={e => setSaveAsPath(e.currentTarget.value)} />
            <MockButton label="Save as" reason={desk.block() ?? (saveAsPath().trim() && saveAsPath().trim() !== file().path ? null : "Enter a path different from the original.")}
              onClick={() => desk.setReceipt("save", `Mock outcome: saved as ${saveAsPath().trim()} in memory only. No file was written and the original was not overwritten.`, true)} />
          </div>
        </Show>
        <label class="a-sr" for="a-editor-text">Editor: {file().path}</label>
        <textarea id="a-editor-text" class="a-code a-editor-text" value={text()} readOnly={!!desk.block()} spellcheck={false} rows={6}
          onInput={e => desk.setUi("editor", file().id, e.currentTarget.value)} />
        <div class="a-actions">
          <MockButton label="Save" reason={desk.block() ?? (desk.ui.saveConflict ? "Resolve the file conflict first: Compare, Reload or Save as." : null)} onClick={save} />
          <button type="button" aria-expanded={desk.ui.terminalOpen} aria-controls="a-terminal" onClick={() => desk.setUi("terminalOpen", !desk.ui.terminalOpen)}>
            {desk.ui.terminalOpen ? "Hide terminal" : "Show terminal"}
          </button>
        </div>
        <Receipt k="save" />
      </section>
    </div>
    <Show when={desk.ui.terminalOpen}>
      <section class="a-terminal" id="a-terminal" aria-labelledby="a-term-h">
        <h3 id="a-term-h">Terminal</h3>
        <p class="a-muted">Fixture terminal: output is recorded; commands are not executed.</p>
        <pre class="a-code a-term-out">{desk.ui.terminal.length ? desk.ui.terminal.join("\n") : "No recorded output for this task in the fixture."}</pre>
        <Show when={desk.conn() !== "connected"}>
          <p class="a-reason">Output paused at {hhmm(fixture().machines[0].lastConfirmedAt)}. Input is unavailable.</p>
        </Show>
        <label for="a-term-input">Command (not executed)</label>
        <input id="a-term-input" class="a-mono" value={command()} disabled={!!desk.block()} onInput={e => setCommand(e.currentTarget.value)}
          onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); runCommand(); } }} />
      </section>
    </Show>
  </div>;
}

/** Git tab: worktree identity, conflicted group first, commit form and remote actions. */
export function GitTab(props: { v: TaskView }) {
  const desk = useDesk();
  const git = () => desk.store.state().fixture.git;
  const file = () => desk.store.state().fixture.files[0];
  const conflict = () => props.v.conflict && git().state === "conflict";
  const resolution = () => desk.store.state().conflictResolution;
  const marked = () => desk.ui.markedResolved;
  const pushUnknown = () => desk.ui.push[props.v.id] === "unknown";
  const [message, setMessage] = createSignal("");
  const commitReason = () => desk.block() ?? (conflict()
    ? (marked() ? "The merge is still conflicted in the fixture; a mock resolution does not change Git." : "Resolve 1 remaining conflict.")
    : "Nothing to commit: the fixture has no staged changes.");
  const remoteReason = () => desk.block() ?? (conflict() ? "Finish or abort the merge first." : null);
  return <div class="a-stack">
    <p class="a-worktree">Branch <code class="a-mono">{git().branch}</code> · worktree path: not in fixture · HEAD: not in fixture</p>
    <Show when={conflict()} fallback={
      <div class="a-empty">
        <p class="a-empty-title">No Git changes are modelled for this task in the fixture.</p>
        <Show when={props.v.accepted}><p>Accepted (mock) did not commit or push. Git state is unchanged.</p></Show>
      </div>}>
      <div class="a-banner a-banner-danger" role="group" aria-label="Merge conflict">
        <Glyph name="x" />
        <p><strong>Merge stopped: 1 file has conflicts on <code class="a-mono">{git().branch}</code>.</strong> {git().detail} No stash, reset or automatic resolution is performed.</p>
      </div>
      <section aria-labelledby="a-git-conflicted">
        <h3 id="a-git-conflicted">Conflicted ({marked() ? 0 : 1})</h3>
        <Show when={!marked()} fallback={<p class="a-muted">None</p>}>
          <ul class="a-files"><li>
            <span class="a-mono">{file().path}</span> <Badge tone="danger" glyph="x">Conflict (U)</Badge>
            <span class="a-actions">
              <button type="button" onClick={() => desk.setTab("code")}>Open file</button>
              <MockButton label="Mark resolved" reason={desk.block() ?? (resolution() ? null : "Choose mine or incoming in the editor's Compare view first.")}
                onClick={() => { desk.setUi("markedResolved", true); desk.setReceipt("git-resolve", `Mock outcome: ${file().path} marked resolved in memory only (${resolution() === "local" ? "mine" : "incoming"} chosen). The Git index is unchanged.`, true); }} />
            </span>
          </li></ul>
        </Show>
      </section>
      <Show when={marked()}>
        <section aria-labelledby="a-git-resolved">
          <h3 id="a-git-resolved">Resolved (mock) (1)</h3>
          <ul class="a-files"><li><span class="a-mono">{file().path}</span> · resolved in memory · Git index unchanged</li></ul>
        </section>
      </Show>
      <Receipt k="git-resolve" />
    </Show>
    <section aria-labelledby="a-git-staged">
      <h3 id="a-git-staged">Staged (0)</h3><p class="a-muted">None in the fixture.</p>
      <h3>Unstaged (0)</h3><p class="a-muted">None in the fixture.</p>
    </section>
    <section class="a-commit" aria-labelledby="a-commit-h">
      <h3 id="a-commit-h">Commit</h3>
      <label for="a-commit-msg">Commit message</label>
      <textarea id="a-commit-msg" rows={2} value={message()} onInput={e => setMessage(e.currentTarget.value)} />
      <div class="a-actions"><MockButton label="Commit" reason={commitReason()} onClick={() => desk.setReceipt("git", "Mock outcome: nothing was committed.", true)} /></div>
    </section>
    <div class="a-actions">
      <Show when={conflict()}>
        <MockButton label="Abort merge" tone="danger" reason={desk.block()} onClick={() => desk.confirm({
          title: "Abort the merge?", confirmLabel: "Abort merge",
          effect: "Records a mock abort in memory. Git is not touched; nothing is reset or stashed.",
          scope: `Merge on ${git().branch}`,
          onConfirm: () => desk.setReceipt("git", "Mock outcome: no merge was aborted. Git is untouched; the fixture still reports a conflict.", true),
        })} />
      </Show>
      <MockButton label="Pull" reason={remoteReason()} onClick={() => desk.setReceipt("git", "Mock outcome: nothing was pulled. No remote was contacted.", true)} />
      <MockButton label="Push" reason={remoteReason() ?? (pushUnknown() ? "Push outcome unknown: inspect the remote before trying again." : null)} onClick={() => desk.confirm({
        title: `Push ${git().branch}?`, confirmLabel: "Push",
        effect: "Simulates a push in memory. Never force. No remote is contacted.",
        scope: `Branch ${git().branch} to its upstream remote (remote name not in the fixture)`,
        onConfirm: () => { desk.setUi("push", props.v.id, "unknown"); desk.focusSoon("a-push-state"); },
      })} />
    </div>
    <Receipt k="git" />
    <Show when={pushUnknown()}>
      <div id="a-push-state" tabindex="-1" class="a-state" role="status">
        <Badge tone="neutral" glyph="question">Outcome unknown</Badge>
        <p>Push to the upstream of <code class="a-mono">{git().branch}</code> did not confirm (mock). Check the remote before trying again.</p>
        <MockButton label="Inspect remote" reason={desk.block()} onClick={() => desk.setReceipt("git", "Mock outcome: no remote was contacted. The push outcome stays unknown.", true)} />
      </div>
    </Show>
  </div>;
}
