import { createMemo, createSignal, For, Show, type JSX } from "solid-js";
import { useWs } from "./ctx";
import { I } from "./kit";
import { taskCheckout } from "./review-model";
import { detailFailed, displayStep, editDiff, isProjectRoot, PROJECT_ROOT, stepHasContent, type DisplayStep, type StepView } from "./tool-names";

/** Which folds and steps the owner opened. It lives above the rows, so no live update or redraw of a row can close one. */
export type OpenState = { is: (key: string) => boolean; toggle: (key: string) => void; clear: () => void };
export function createOpenState(): OpenState {
  const [open, setOpen] = createSignal<Record<string, boolean>>({});
  return { is: key => !!open()[key], toggle: key => setOpen(all => ({ ...all, [key]: !all[key] })), clear: () => setOpen({}) };
}

/** Recorded output exactly as written: a long line scrolls sideways inside this box, never wraps or widens its column. Keys,
 *  the wheel with Shift and a trackpad all move it, so it is a named stop in the tab order. */
export function OutputBox(props: { label: string; class?: string; children: JSX.Element }) {
  return <div class={`ws-out ${props.class ?? "args"}`} tabIndex={0} role="region" aria-label={props.label}>{props.children}</div>;
}

/** An edit's changed lines, the same box wherever an edit opens. It is drawn only with lines in it, never as an empty box. */
export function DiffBox(props: { label: string; lines: string[] }) {
  return <Show when={props.lines.length > 0}><OutputBox label={props.label} class="diff">
    <For each={props.lines}>{line => line.startsWith("@@") ? <div class="h">{line}</div>
      : line.startsWith("+") ? <div class="a">{line}</div>
      : line.startsWith("-") ? <div class="d">−{line.slice(1)}</div>
      : <div>{line}</div>}</For>
  </OutputBox></Show>;
}

/** "+N −M" for an edit whose lines are known. */
export const EditCounts = (props: { add: number; del: number }) => <span class="tnum"><span class="add">+{props.add}</span> <span class="del">−{props.del}</span></span>;

/** Opens a step's file in the task's Code tab, or says why it cannot. The project root opens Code on its files. */
function useOpenFile(taskId: () => string) {
  const ws = useWs();
  const [error, setError] = createSignal("");
  const open = async (file?: string) => {
    const task = ws.store.state().tasks.find(t => t.id === taskId());
    if (!task || !file) return;
    const checkout = taskCheckout(ws.store.state(), task);
    if (!checkout) { setError("Checkout is unavailable. Reconnect and try again."); return; }
    if (isProjectRoot(file) || file.replace(/\/+$/, "") === checkout.path) { ws.go({ name: "task", machine: ws.machine().id, task: taskId(), tab: "code" }); return; }
    let path = file;
    if (path.startsWith(checkout.path + "/")) path = path.slice(checkout.path.length + 1);
    if (path.startsWith("/") || path.split("/").includes("..")) { setError("This file is outside the task checkout."); return; }
    try {
      await ws.buffers.open(task.projectId, checkout.id, path);
      ws.go({ name: "task", machine: ws.machine().id, task: taskId(), tab: "code", file: path });
    } catch { setError("The file could not be opened. Reconnect and try again."); }
  };
  return { open, error };
}

/** Identical target, command and diff presentation wherever a step opens. */
export function StepDetail(props: { step: DisplayStep; taskId: string }) {
  const file = useOpenFile(() => props.taskId), error = file.error, open = () => file.open(props.step.path);
  const diff = createMemo(() => editDiff(props.step));
  const failure = () => props.step.failure?.trim();
  const shownPath = () => isProjectRoot(props.step.path) ? PROJECT_ROOT : props.step.path;
  return <Show when={props.step.path || props.step.query || props.step.command || diff() || props.step.notice || failure()}><div class="ws-step-detail">
    <Show when={failure()}>{f => <div class="ws-step-failure st-bad" role="note">{f()}</div>}</Show>
    <Show when={props.step.notice}><div class="cap st-warn">{props.step.notice}</div></Show>
    <Show when={props.step.path}><button class="lnk ws-step-path" onClick={() => void open()} aria-label={`Open ${shownPath()} in Code`}>{shownPath()}</button></Show>
    <Show when={error()}><div role="alert">{error()}</div></Show>
    <Show when={props.step.query}><div>Search: <code>{props.step.query}</code></div></Show>
    <Show when={props.step.command}><pre><code>{props.step.command}</code></pre>
      <OutputBox label="Command output">{props.step.output || (props.step.status === "in_progress" || props.step.status === "pending" ? "Waiting for output…" : "No output was recorded.")}</OutputBox>
    </Show>
    <Show when={diff()}>{d => <DiffBox label="Edit" lines={d().lines} />}</Show>
  </div></Show>;
}

/** One tool step: an icon, a plain name and what it worked on. It opens only when there is something to see; otherwise it is a plain line. */
export function StepRow(props: { view: StepView; taskId: string; tone?: string; icon?: string; status?: JSX.Element; extra?: JSX.Element; hasExtra?: boolean; state: OpenState; stateKey: string }) {
  const open = () => props.state.is(props.stateKey);
  const detail = () => props.view.detail;
  const expandable = () => (!!detail() && stepHasContent(detail()!)) || !!props.hasExtra;
  // A plain read or edit row opens its file in Code from its target; an opening row links it in its detail instead.
  const file = useOpenFile(() => props.taskId);
  const linked = () => !expandable() && !!detail()?.path && (props.view.kind === "read" || props.view.kind === "edit");
  const row = () => <>
    <I n={props.icon ?? props.view.icon} s={14} class={props.tone ?? "faint"} />
    <span class="step-l">{props.view.label}</span>
    <Show when={props.view.target}>
      <Show when={linked()} fallback={<span class="step-t">{props.view.target}</span>}>
        <button class="lnk step-t" onClick={() => void file.open(detail()?.path)} aria-label={`Open ${props.view.target} in Code`}>{props.view.target}</button>
      </Show>
    </Show>
    <Show when={props.status}><span class="step-r">{props.status}</span></Show>
  </>;
  return <div class="step">
    <Show when={expandable()} fallback={<><div class="step-row">{row()}</div><Show when={file.error()}><div class="step-sub" role="alert">{file.error()}</div></Show></>}>
      <button class="ev-toggle step-row" aria-expanded={open() ? "true" : "false"} onClick={() => props.state.toggle(props.stateKey)}>{row()}</button>
    </Show>
    <Show when={open() && expandable()}><div class="step-sub">
      <Show when={detail()}>{d => <StepDetail step={d()} taskId={props.taskId} />}</Show>
      {props.extra}
    </div></Show>
  </div>;
}

/** A turn's tool calls as one collapsed line that opens on its steps. The same block in Conversation and in a worker's feed. */
export function Fold(props: { text: string; open: boolean; onToggle: () => void; loading?: boolean; right?: JSX.Element; children: () => JSX.Element }) {
  return <div class="fold-block">
    <div class="fold-h">
      <button class="fold" aria-expanded={props.open ? "true" : "false"} onClick={() => props.onToggle()}>
        <I n={props.open ? "chevron-down" : "chevron-right"} s={14} />
        <Show when={props.loading}><I n="loader" s={12} class="spin ws-wait" /></Show>
        <span class="fold-t">{props.text}</span>
      </button>
      <Show when={props.right}><span class="fold-r">{props.right}</span></Show>
    </div>
    <Show when={props.open}><div class="steps">{props.children()}</div></Show>
  </div>;
}

/** Steps of a Conversation fold, as the feed recorded them. */
export function TextSteps(props: { steps: string[]; taskId: string; view: (step: string) => StepView; state: OpenState; scope: string }) {
  // A step's recorded text changes with every status update; its call id does not.
  const ids = createMemo(() => props.steps.map((step, i) => displayStep(step)?.callId || `${i}:${step}`), undefined, { equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]) });
  const steps = createMemo(() => new Map(props.steps.map((step, i) => [displayStep(step)?.callId || `${i}:${step}`, step])));
  return <For each={ids()}>{id => {
    const step = () => steps().get(id) ?? "";
    const detail = createMemo(() => displayStep(step()));
    const diff = createMemo(() => { const d = detail(); return d ? editDiff(d) : null; });
    return <StepRow view={props.view(step())} taskId={props.taskId} tone={detailFailed(detail()) ? "st-bad" : undefined} state={props.state} stateKey={`${props.scope}:${id}`}
      status={diff() ? <EditCounts add={diff()!.add} del={diff()!.del} /> : undefined} />;
  }}</For>;
}
