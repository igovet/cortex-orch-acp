import { createSignal, For, Show } from "solid-js";
import { useDesk } from "./ctx";
import { Icon } from "./icons";
import { absTime, type ProjectSummary, type TaskView } from "./model";
import { Btn, Chip, Group, Notice, Reason, Receipt, Row } from "./ui";

/** Git state for a project (and the task worktree when opened from a task). No Git operation runs. */
export function GitPanel(props: { ps: ProjectSummary; tv?: TaskView }) {
  const desk = useDesk();
  const f = () => desk.data.fixture();
  const conflictHere = () => f().git.projectId === props.ps.project.id;
  const files = () => conflictHere() ? f().files : [];
  const resolution = () => desk.store.state().conflictResolution;
  const remaining = () => conflictHere() && !resolution() ? files().length : 0;
  const blocked = () => desk.blocked(props.ps.project.id);
  const [message, setMessage] = createSignal("");
  const rid = () => `git:${props.ps.project.id}`;
  const commitReason = () => blocked() ?? (remaining() ? `Resolve ${remaining()} remaining conflict${remaining() > 1 ? "s" : ""}` : !message().trim() ? "Enter a commit message" : null);
  const pushed = () => desk.local().pushed[props.ps.project.id];
  return <div class="git-panel">
    <h2 class="sr-only">Git</h2>
    <Show when={blocked()}><Notice tone="danger" icon="plugOff">{blocked()}. Last confirmed {absTime("2030-01-01T00:00:00.000Z")}.</Notice></Show>
    <Show when={conflictHere() && remaining() > 0}>
      <Notice tone="danger" icon="gitMerge" role="status">Merge stopped: {remaining()} file has conflicts on {f().git.branch}. {f().git.detail}</Notice>
    </Show>
    <Group label="Worktree" id={`gw-${props.ps.index}`}>
      <Row label="Path"><span class="mono wrap-any">worktrees/{props.ps.slug}{props.tv ? `/${props.tv.num}` : ""}</span></Row>
      <Row label="Branch"><span class="mono wrap-any"><Icon name="gitBranch" size={14} /> {props.ps.project.branch}</span></Row>
      <Row label="HEAD"><span class="mono">mock-head-{props.ps.index}a7f2 (fixture)</span></Row>
    </Group>
    <Group label={<>Conflicted <span class="tertiary tabular">{remaining()}</span></>} id={`gc-${props.ps.index}`}>
      <Show when={files().length} fallback={<p class="empty-row">No conflicted files.</p>}>
        <For each={files()}>{file => <div class="conflict-file">
          <div class="row">
            <div class="row-text"><div class="row-label mono wrap-any"><span class="git-letter tone-danger-fg" aria-label="unmerged">U</span> {file.path}</div>
              <div class="row-desc">{resolution() ? `Marked resolved with ${resolution() === "local" ? "yours" : "incoming"} (mock). No file or Git mutation occurred.` : "Both sides changed this line."}</div></div>
          </div>
          <div class="conflict-hunks" role="group" aria-label={`Conflict in ${file.path}`}>
            <div class="hunk"><div class="hunk-label">Base</div><pre class="code">{file.base}</pre></div>
            <div class="hunk"><div class="hunk-label">Yours</div><pre class="code hunk-yours">{file.local}</pre></div>
            <div class="hunk"><div class="hunk-label">Incoming</div><pre class="code hunk-incoming">{file.incoming}</pre></div>
          </div>
          <div class="btn-row end">
            <Btn mock disabled={Boolean(blocked())} onClick={() => desk.receipt(rid(), desk.store.resolveConflict("local").message, `Resolved ${file.path} with yours (mock)`)}>Keep yours</Btn>
            <Btn mock disabled={Boolean(blocked())} onClick={() => desk.receipt(rid(), desk.store.resolveConflict("incoming").message, `Resolved ${file.path} with incoming (mock)`)}>Take incoming</Btn>
          </div>
        </div>}</For>
      </Show>
    </Group>
    <Group label="Staged" id={`gs-${props.ps.index}`}>
      <Show when={conflictHere()} fallback={<p class="empty-row">Working tree clean.</p>}>
        <div class="row"><div class="row-label mono"><span class="git-letter" aria-label="modified">M</span> synthetic/tokens.ts</div></div>
      </Show>
    </Group>
    <Group label="Unstaged" id={`gu-${props.ps.index}`}><p class="empty-row">Working tree clean.</p></Group>
    <Group label="Commit" id={`gm-${props.ps.index}`}>
      <div class="row column">
        <label class="field-label" for={`commit-${props.ps.index}`}>Commit message</label>
        <textarea id={`commit-${props.ps.index}`} class="input grow-sm" rows="3" value={message()} onInput={e => setMessage(e.currentTarget.value)} disabled={Boolean(blocked())} />
        <div class="btn-row end">
          <Btn variant="primary" mock disabled={Boolean(commitReason())} aria-describedby={`commit-reason-${props.ps.index}`}
            onClick={() => { desk.receipt(rid(), "Mock commit recorded in memory only. No Git command ran.", `Commit in ${props.ps.project.name} (mock)`); setMessage(""); }}>Commit</Btn>
        </div>
        <Reason id={`commit-reason-${props.ps.index}`} text={commitReason()} icon={remaining() ? "gitMerge" : "info"} />
      </div>
    </Group>
    <div class="btn-row">
      <Btn icon="arrowDown" mock disabled={Boolean(blocked())} onClick={() => desk.receipt(rid(), "Mock pull: no remote was contacted. Nothing changed.")}>Pull</Btn>
      <Btn icon="arrowUp" mock disabled={Boolean(blocked()) || remaining() > 0} onClick={async () => {
        const ok = await desk.confirm({ title: `Push ${props.ps.project.branch}?`, body: "This is a mock. No remote is contacted and nothing leaves this browser tab.", confirm: "Push", mock: true });
        if (!ok) return;
        desk.setLocal(l => ({ ...l, pushed: { ...l.pushed, [props.ps.project.id]: true } }));
        desk.receipt(rid(), "Mock push sent nowhere: outcome unknown. No remote was contacted.", `Push ${props.ps.project.branch} (mock, outcome unknown)`);
      }}>Push</Btn>
      <Btn icon="terminal" onClick={() => desk.prefs.setTerminalOpen(!desk.prefs.terminalOpen())} aria-pressed={desk.prefs.terminalOpen()}>Terminal</Btn>
    </div>
    <Reason text={remaining() > 0 && !blocked() ? "Push is unavailable until the conflict is resolved." : null} />
    <Show when={pushed()}><p class="unknown-outcome"><Chip icon="help">Outcome unknown</Chip>
      <Btn variant="ghost" mock onClick={() => desk.receipt(rid(), "Mock inspection: the fixture has no remote. Outcome stays unknown.")}>Inspect remote</Btn></p></Show>
    <Receipt id={rid()} />
  </div>;
}

/** Connection settings for a project's machine: shared by the Machine tab and Settings → Projects. */
export function MachinePanel(props: { ps: ProjectSummary }) {
  const desk = useDesk();
  const machine = () => desk.data.machineFor(props.ps.project);
  const offline = () => desk.data.offline();
  const rid = () => `machine:${props.ps.project.id}`;
  const notEnrolled = () => desk.store.state().scenario === "blocked";
  const devices = () => [{ id: `${props.ps.project.id}:desktop`, name: "This desktop browser (mock)", seen: "02:46" }, { id: `${props.ps.project.id}:phone`, name: "Synthetic phone (mock)", seen: "02:31" }]
    .filter(d => !desk.local().revoked.includes(d.id));
  const caps = () => desk.data.fixture().capabilities.filter(c => c.projectId === props.ps.project.id);
  return <div class="machine-panel">
    <Show when={props.ps.disconnected}>
      <Notice tone="danger" icon="plugOff" role="status" action={<Btn mock disabled={offline()} onClick={() => desk.receipt(rid(), "Mock reconnect: nothing was sent. The machine stays disconnected in the fixture.")}>Reconnect</Btn>}>
        {machine()?.name} is disconnected · last confirmed {absTime(machine()?.lastConfirmedAt ?? null)}.
      </Notice>
    </Show>
    <Show when={offline()}><Notice icon="plugZap">This device is offline. States are as of 02:46 (fixture); actions are unavailable.</Notice></Show>
    <Receipt id={rid()} />
    <Group label="Machine" id={`m-${props.ps.index}`}>
      <Row label="Name">{machine()?.name}</Row>
      <Row label="State"><span class={props.ps.disconnected ? "tone-danger-fg" : ""}><Icon name={props.ps.disconnected ? "plugOff" : "plug"} size={14} /> {props.ps.disconnected ? `Disconnected · last confirmed ${absTime(machine()?.lastConfirmedAt ?? null)}` : "Connected"}</span></Row>
      <Row label="Current execution state" desc="Whether running work on this machine is confirmed now">{machine()?.currentExecutionState === "unknown" ? <span>Unknown <Icon name="help" size={14} /></span> : "Confirmed"}</Row>
    </Group>
    <Group label="Relay" id={`r-${props.ps.index}`}>
      <Row label="Relay" desc="Owner-operated relay used for remote access">Default owner relay</Row>
      <div class="row column">
        <label class="field-label" for={`relay-${props.ps.index}`}>Override relay address</label>
        <div class="inline-field">
          <input id={`relay-${props.ps.index}`} class="input" value={desk.local().relay} placeholder="relay host name" disabled={notEnrolled() || Boolean(desk.blocked(props.ps.project.id))}
            aria-describedby={`relay-reason-${props.ps.index}`} onInput={e => desk.setLocal(l => ({ ...l, relay: e.currentTarget.value }))} />
          <Btn mock disabled={notEnrolled() || Boolean(desk.blocked(props.ps.project.id))} onClick={() => desk.receipt(rid(), "Mock connection test: no network request was sent.")}>Test connection</Btn>
        </div>
        <Reason id={`relay-reason-${props.ps.index}`} icon={notEnrolled() ? "lock" : "info"} text={notEnrolled() ? "This device is not enrolled. Enroll it in Devices." : desk.blocked(props.ps.project.id)} />
      </div>
    </Group>
    <Group label="Devices" id={`d-${props.ps.index}`}>
      <Show when={devices().length} fallback={<p class="empty-row">No enrolled devices.</p>}>
        <For each={devices()}>{d => <Row label={d.name} desc={`Last seen ${d.seen} (fixture)${desk.blocked(props.ps.project.id) ? ` · ${desk.blocked(props.ps.project.id)}` : ""}`}>
          <Btn variant="danger-ghost" mock disabled={Boolean(desk.blocked(props.ps.project.id))} onClick={async () => {
            if (!await desk.confirm({ title: `Revoke ${d.name}?`, body: "The device loses access to this machine. This is a mock: nothing is revoked outside this tab.", confirm: "Revoke", danger: true, mock: true })) return;
            desk.setLocal(l => ({ ...l, revoked: [...l.revoked, d.id] }));
            desk.receipt(rid(), `Mock: ${d.name} removed from this list in memory only.`, `Revoked ${d.name} (mock)`);
          }}>Revoke</Btn>
        </Row>}</For>
      </Show>
    </Group>
    <Group label="Capabilities" id={`c-${props.ps.index}`}>
      <Show when={caps().length} fallback={<p class="empty-row">No unavailable capabilities reported.</p>}>
        <For each={caps()}>{c => <Row label={<span><Icon name="circleSlash" size={14} /> {c.name}</span>} desc={c.reason}>Unavailable</Row>}</For>
      </Show>
    </Group>
  </div>;
}

/** Bottom terminal panel. Output is fixture data; commands are recorded, never executed. */
export function TerminalPanel(props: { tv: TaskView }) {
  const desk = useDesk();
  const [lines, setLines] = createSignal<string[]>([]);
  const [height, setHeight] = createSignal(240);
  const [cmd, setCmd] = createSignal("");
  const blocked = () => desk.blocked(props.tv.project.id);
  const output = () => desk.data.eventsFor(props.tv.task.id).slice(-8).map(e => `[${e.occurredAt.slice(11, 19)}] ${e.kind}: ${e.text}`);
  let drag = 0;
  return <section class="terminal" aria-label="Terminal" style={{ height: `${height()}px` }}>
    <div class="term-handle" role="separator" aria-orientation="horizontal" aria-label="Resize terminal" tabindex="0" aria-valuenow={height()} aria-valuemin={120} aria-valuemax={600}
      onKeyDown={e => { if (e.key === "ArrowUp") { e.preventDefault(); setHeight(h => Math.min(600, h + 16)); } if (e.key === "ArrowDown") { e.preventDefault(); setHeight(h => Math.max(120, h - 16)); } }}
      onPointerDown={e => { drag = e.clientY; (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId); }}
      onPointerMove={e => { if (!drag) return; const d = drag - e.clientY; drag = e.clientY; setHeight(h => Math.max(120, Math.min(600, h + d))); }}
      onPointerUp={() => (drag = 0)} />
    <div class="term-head"><Icon name="terminal" size={14} /><span>Fixture terminal: output is recorded; commands are not executed</span>
      <button type="button" class="icon-btn sm" aria-label="Close terminal (Ctrl+`)" title="Close terminal (Ctrl+`)" onClick={() => desk.prefs.setTerminalOpen(false)}><Icon name="x" size={14} /></button></div>
    <pre class="term-out" tabindex="0" aria-label="Terminal output">{[...output(), ...lines()].join("\n")}{blocked() ? `\nOutput paused at 00:00 (${blocked()})` : ""}</pre>
    <form class="term-in" onSubmit={e => { e.preventDefault(); if (!cmd().trim()) return; setLines(l => [...l, `$ ${cmd()}`, "(mock) recorded, not executed"]); setCmd(""); }}>
      <label class="sr-only" for={`term-${props.tv.num}`}>Terminal input</label>
      <span class="mono tertiary" aria-hidden="true">$</span>
      <input id={`term-${props.tv.num}`} class="term-input mono" value={cmd()} onInput={e => setCmd(e.currentTarget.value)} disabled={Boolean(blocked())}
        placeholder={blocked() ? "Input disabled" : "Type a command (recorded only)"} aria-describedby={`term-r-${props.tv.num}`} />
    </form>
    <Reason id={`term-r-${props.tv.num}`} text={blocked()} />
  </section>;
}
