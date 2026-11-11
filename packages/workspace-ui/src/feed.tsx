import { createMemo, createSignal, For, Show, type JSX } from "solid-js";
import { useWs } from "./ctx";
import { I } from "./kit";
import { taskCheckout } from "./review-model";
import { displayStep, stepHasContent, type DisplayStep, type StepView } from "./tool-names";

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

/** Identical target, command and diff presentation wherever a step opens. */
export function StepDetail(props: { step: DisplayStep; taskId: string }) {
  const ws = useWs();
  const [error, setError] = createSignal("");
  const open = async () => {
    const task = ws.store.state().tasks.find(t => t.id === props.taskId);
    if (!task || !props.step.path) return;
    const checkout = taskCheckout(ws.store.state(), task);
    if (!checkout) { setError("Checkout is unavailable. Reconnect and try again."); return; }
    let path = props.step.path;
    if (path.startsWith(checkout.path + "/")) path = path.slice(checkout.path.length + 1);
    if (path.startsWith("/") || path.split("/").includes("..")) { setError("This file is outside the task checkout."); return; }
    try {
      await ws.buffers.open(task.projectId, checkout.id, path);
      ws.go({ name: "task", machine: ws.machine().id, task: props.taskId, tab: "code", file: path });
    } catch { setError("The file could not be opened. Reconnect and try again."); }
  };
  return <Show when={props.step.path || props.step.query || props.step.command || props.step.newText !== undefined || props.step.notice}><div class="ws-step-detail">
    <Show when={props.step.notice}><div class="cap st-warn">{props.step.notice}</div></Show>
    <Show when={props.step.path}><button class="lnk ws-step-path" onClick={() => void open()} aria-label={`Open ${props.step.path} in Code`}>{props.step.path}</button></Show>
    <Show when={error()}><div role="alert">{error()}</div></Show>
    <Show when={props.step.query}><div>Search: <code>{props.step.query}</code></div></Show>
    <Show when={props.step.command}><pre><code>{props.step.command}</code></pre>
      <OutputBox label="Command output">{props.step.output || (props.step.status === "in_progress" || props.step.status === "pending" ? "Waiting for output…" : "No output was recorded.")}</OutputBox>
    </Show>
    <Show when={props.step.newText !== undefined}><details><summary>View edit</summary><OutputBox label="Edit" class="diff">
      <For each={(props.step.oldText ?? "").split("\n").filter((_,i,lines) => lines.length > 1 || lines[0] !== "")}>{line => <div class="d">−{line}</div>}</For>
      <For each={props.step.newText!.split("\n")}>{line => <div class="a">+{line}</div>}</For>
    </OutputBox></details></Show>
  </div></Show>;
}

/** One tool step: an icon, a plain name and what it worked on. It opens only when there is something to see; otherwise it is a plain line. */
export function StepRow(props: { view: StepView; taskId: string; tone?: string; icon?: string; status?: JSX.Element; extra?: JSX.Element; hasExtra?: boolean; state: OpenState; stateKey: string }) {
  const open = () => props.state.is(props.stateKey);
  const detail = () => props.view.detail;
  const expandable = () => (!!detail() && stepHasContent(detail()!)) || !!props.hasExtra;
  const row = () => <>
    <I n={props.icon ?? props.view.icon} s={14} class={props.tone ?? "faint"} />
    <span class="step-l">{props.view.label}</span>
    <Show when={props.view.target}><span class="step-t">{props.view.target}</span></Show>
    <Show when={props.status}><span class="step-r">{props.status}</span></Show>
  </>;
  return <div class="step">
    <Show when={expandable()} fallback={<div class="step-row">{row()}</div>}>
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
        <Show when={props.loading}><I n="loader" s={12} class="spin" /></Show>
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
    return <StepRow view={props.view(step())} taskId={props.taskId} tone={displayStep(step())?.status === "failed" ? "st-bad" : undefined} state={props.state} stateKey={`${props.scope}:${id}`} />;
  }}</For>;
}
