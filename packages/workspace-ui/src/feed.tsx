import { createSignal, For, Show, type JSX } from "solid-js";
import { useWs } from "./ctx";
import { I } from "./kit";
import { taskCheckout } from "./review-model";
import { displayStep, type DisplayStep, type StepView } from "./tool-names";

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
      <div class="args">{props.step.output || (props.step.status === "in_progress" || props.step.status === "pending" ? "Waiting for output…" : "No output was recorded.")}</div>
    </Show>
    <Show when={props.step.newText !== undefined}><details><summary>View edit</summary><div class="diff">
      <For each={(props.step.oldText ?? "").split("\n").filter((_,i,lines) => lines.length > 1 || lines[0] !== "")}>{line => <div class="d">−{line}</div>}</For>
      <For each={props.step.newText!.split("\n")}>{line => <div class="a">+{line}</div>}</For>
    </div></details></Show>
  </div></Show>;
}

/** One tool step: an icon, a plain name and what it worked on. Long commands stay on one line and open with a click. */
export function StepRow(props: { view: StepView; taskId: string; tone?: string; icon?: string; status?: JSX.Element; extra?: JSX.Element; hasExtra?: boolean }) {
  const [open, setOpen] = createSignal(false);
  const detail = () => props.view.detail;
  const expandable = () => !!detail() || !!props.hasExtra;
  const row = () => <>
    <I n={props.icon ?? props.view.icon} s={14} class={props.tone ?? "faint"} />
    <span class="step-l">{props.view.label}</span>
    <Show when={props.view.target}><span class="step-t">{props.view.target}</span></Show>
    <Show when={props.status}><span class="step-r">{props.status}</span></Show>
  </>;
  return <div class="step">
    <Show when={expandable()} fallback={<div class="step-row">{row()}</div>}>
      <button class="ev-toggle step-row" aria-expanded={open() ? "true" : "false"} onClick={() => setOpen(!open())}>{row()}</button>
    </Show>
    <Show when={open()}><div class="step-sub">
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
export function TextSteps(props: { steps: string[]; taskId: string; view: (step: string) => StepView }) {
  return <For each={props.steps}>{step => <StepRow view={props.view(step)} taskId={props.taskId} tone={displayStep(step)?.status === "failed" ? "st-bad" : undefined} />}</For>;
}
