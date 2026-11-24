import { createEffect, createMemo, createSignal, For, on, onCleanup, Show, type JSX } from "solid-js";
import type { CommandOutput, CommandOutputRead, CommandOutputRef } from "@cortex/workspace-domain";
import { CopySource } from "./message-copy";
import { useWs } from "./ctx";
import { I } from "./kit";
import { taskCheckout } from "./review-model";
import { goToCode } from "./result-links";
import { detailFailed, displayStep, editDiff, isProjectRoot, PROJECT_ROOT, hasRecordedDetail, privateStep, stepHasContent, type DisplayStep, type StepView } from "./tool-names";

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
    if (isProjectRoot(file) || file.replace(/\/+$/, "") === checkout.path) { goToCode(ws, task.projectId, checkout.id, task.id); return; }
    let path = file;
    if (path.startsWith(checkout.path + "/")) path = path.slice(checkout.path.length + 1);
    if (path.startsWith("/") || path.split("/").includes("..")) { setError("This file is outside the task checkout."); return; }
    try {
      await ws.buffers.open(task.projectId, checkout.id, path);
      goToCode(ws, task.projectId, checkout.id, task.id, path);
    } catch { setError("The file could not be opened. Reconnect and try again."); }
  };
  return { open, error };
}

/** Identical target, command and diff presentation wherever a step opens. */
export function StepDetail(props: { step: DisplayStep; taskId: string; metadataOnly?: boolean }) {
  const file = useOpenFile(() => props.taskId), error = file.error, open = () => file.open(props.step.path);
  const diff = createMemo(() => editDiff(props.step));
  const failure = () => props.step.failure?.trim();
  const shownPath = () => isProjectRoot(props.step.path) ? PROJECT_ROOT : props.step.path;
  return <Show when={props.step.path || (!props.metadataOnly && (props.step.query || props.step.command)) || diff() || props.step.notice || failure()}><div class="ws-step-detail">
    <Show when={failure()}>{f => <div class="ws-step-failure st-bad" role="note">{f()}</div>}</Show>
    <Show when={props.step.notice}><div class="cap st-warn">{props.step.notice}</div></Show>
    <Show when={props.step.path}><button class="lnk ws-step-path" onClick={() => void open()} aria-label={`Open ${shownPath()} in Code`}>{shownPath()}</button></Show>
    <Show when={error()}><div role="alert">{error()}</div></Show>
    <Show when={!props.metadataOnly && props.step.query}><div>Search: <code>{props.step.query}</code></div></Show>
    <Show when={!props.metadataOnly && props.step.command}><pre><code>{props.step.command}</code></pre>
      <OutputBox label="Command output">{props.step.output || (props.step.status === "in_progress" || props.step.status === "pending" ? "Waiting for output…" : "No output was recorded.")}</OutputBox>
    </Show>
    <Show when={diff()}>{d => <DiffBox label="Edit" lines={d().lines} />}</Show>
  </div></Show>;
}

const TEXT_PREVIEW = 1000;
function RecordedText(props: { label: string; text?: string | null; missing: string; box?: string; partial?: boolean }) {
  const [all, setAll] = createSignal(false);
  const long = () => (props.text?.length ?? 0) > TEXT_PREVIEW;
  const shown = () => all() || !long() ? props.text : props.text!.slice(0, TEXT_PREVIEW) + "…";
  return <section aria-label={props.label}>
    <div class="ev-dh">{props.label}</div>
    <Show when={props.text != null} fallback={<div>{props.missing}</div>}>
      <Show when={props.text !== ""} fallback={<div>Recorded empty.</div>}>
        <Show when={props.label === "Input"} fallback={<OutputBox label={props.box ?? props.label}>{shown()}</OutputBox>}>
          <pre class="args" role="region" tabIndex={0} aria-label="Input"><code>{shown()}</code></pre>
        </Show>
      </Show>
      <div class="ws-step-acts">
        <Show when={long()}><button type="button" class="lnk" aria-expanded={all()} onClick={() => setAll(v => !v)}>{all() ? "Show preview" : "Show all retained text"}</button></Show>
        <CopySource text={props.text} label={`Copy ${props.label.toLowerCase()} (${props.partial ? "loaded text" : "retained text"})`} />
      </div>
    </Show>
  </section>;
}

type CapturedPage = Extract<CommandOutput["output"], { state: "available" }>;

/** This component is mounted only while the owner has opened this exact command. Pages never enter history. */
function CommandDetail(props: { request: CommandOutputRead; pending: boolean; read: (request: CommandOutputRead) => Promise<CommandOutput> }) {
  const [receipt, setReceipt] = createSignal<CommandOutput>();
  const [page, setPage] = createSignal<CapturedPage>();
  const [text, setText] = createSignal("");
  const [decoding, setDecoding] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  let generation = 0;
  let decoder: TextDecoder | undefined;
  onCleanup(() => { generation++; });

  const load = async (offset?: number) => {
    const run = ++generation;
    setBusy(true);
    setFailed(false);
    try {
      const result = await props.read(offset === undefined ? props.request : { ...props.request, offset });
      if (run !== generation) return;
      const out = result.output;
      if (out.state !== "available") {
        setReceipt(result);
        setPage(undefined);
        setText("");
        return;
      }
      const bytes = out.encoding === "base64" ? Uint8Array.from(atob(out.chunk), c => c.charCodeAt(0)) : new TextEncoder().encode(out.chunk);
      if (out.offset !== (offset ?? 0) || bytes.length !== out.byteLength ||
          out.offset + out.byteLength > out.totalBytes ||
          (out.nextOffset === null ? out.offset + out.byteLength !== out.totalBytes
            : out.byteLength === 0 || out.nextOffset !== out.offset + out.byteLength || out.nextOffset >= out.totalBytes) ||
          (offset !== undefined && out.totalBytes !== page()?.totalBytes)) throw new Error("Invalid command output page");
      if (offset === undefined || !decoder) decoder = new TextDecoder("utf-8", { ignoreBOM: true });
      const chunk = decoder.decode(bytes, { stream: out.nextOffset !== null });
      setText(previous => offset === undefined ? chunk : previous + chunk);
      setDecoding(previous => (offset !== undefined && previous) || out.encoding === "base64");
      setPage(out);
      setReceipt(result);
    } catch {
      if (run === generation) setFailed(true);
    } finally {
      if (run === generation) setBusy(false);
    }
  };
  // A retained command going from running to settled needs a fresh preview, not polling.
  createEffect(on(() => props.pending, () => {
    if (!page()) void load();
  }));

  const input = () => {
    const r = receipt();
    if (!r || (r.argv === null && r.cwd === null)) return undefined;
    return JSON.stringify({ argv: r.argv ?? undefined, cwd: r.cwd ?? undefined }, null, 2);
  };
  const nextOffset = () => page()?.nextOffset;
  return <div class="ws-step-detail">
    <RecordedText label="Input" text={input()} missing={busy() ? "Loading command input…" : "No input was recorded."} />
    <Show when={receipt()}>{r => <div>
      Command {r().status}{r().exitCode !== null ? `, exit ${r().exitCode}` : ""}{r().signal !== null ? `, signal ${r().signal}` : ""}
    </div>}</Show>
    <RecordedText label="Output" box="Command output" text={page() && (text() || page()!.totalBytes === 0) ? text() : undefined} partial={nextOffset() != null}
      missing={busy() ? "Loading command output…" : failed() ? "Command output could not be loaded." : receipt()?.output.state === "pending" && props.pending ? "Waiting for output…" : page() ? "Load more bytes to decode the output." : "Command output is unavailable."} />
    <Show when={page()}>{p => <>
      <div>{p().offset + p().byteLength} of {p().totalBytes} retained bytes loaded.</div>
      <Show when={p().truncated || !p().captureComplete}><div class="st-warn">{p().truncated ? "Output capture was truncated." : "Output capture is incomplete."}</div></Show>
      <Show when={decoding()}><div>Shown as UTF-8 text; invalid bytes are replaced.</div></Show>
    </>}</Show>
    <Show when={page() && failed()}><div role="alert">More output could not be loaded. Only previously loaded text is shown.</div></Show>
    <Show when={busy() && page()}><div role="status">Loading more output…</div></Show>
    <Show when={failed() || nextOffset() != null || (!busy() && receipt()?.output.state === "pending")}>
      <button type="button" class="lnk" disabled={busy()} onClick={() => void load(nextOffset() ?? undefined)}>
        {failed() ? "Retry output read" : nextOffset() != null ? "Load more output" : "Refresh output"}
      </button>
    </Show>
  </div>;
}

function ToolDetail(props: { view: StepView; taskId: string; args?: string; output?: string; tail?: string; active?: boolean; pending?: boolean; commandRef?: CommandOutputRef; fallbackCommand?: string }) {
  const ws = useWs();
  const step = () => props.view.detail;
  const hidden = () => privateStep(props.view);
  const ref = () => props.commandRef ?? step()?.commandOutputRef;
  const pending = () => props.active !== false && (props.pending || /^(pending|in_progress)$/.test(step()?.status ?? ""));
  const read = ws.store.readCommandOutput;
  const request = createMemo(() => {
    const task = ws.store.state().tasks.find(t => t.id === props.taskId), command = ref();
    return !hidden() && task && command && read && ws.shell.kind === "desktop"
      ? JSON.stringify({ projectId: task.projectId, taskId: task.id, ...command }) : undefined;
  });
  const input = () => {
    const d = step();
    const selected = d && { path: d.path, query: d.query, reference: d.ref };
    const command = d?.command ?? props.fallbackCommand;
    if (selected && Object.values(selected).some(v => v !== undefined)) return JSON.stringify({ command, ...selected }, null, 2);
    return command ?? (!displayStep(props.args) ? props.args : undefined);
  };
  const fromTail = () => step()?.output === undefined && props.output === undefined && props.tail !== undefined;
  const output = () => step()?.output ?? props.output ?? props.tail;
  return <>
    <Show when={!hidden() && step()}>{d => <StepDetail step={d()} taskId={props.taskId} metadataOnly />}</Show>
    <Show when={request()} keyed fallback={<div class="ws-step-detail">
      <RecordedText label="Input" text={hidden() ? undefined : input()} missing={hidden() ? "Private or redacted." : "No input was recorded."} />
      <Show when={!hidden() && input()}><div class="cap">Recorded input only; other fields may not have been retained.</div></Show>
      <RecordedText label="Output" box={props.view.kind === "shell" ? "Command output" : undefined}
        text={hidden() || ref() ? undefined : output()}
        missing={hidden() ? "Private or redacted." : ref() ? "Command output is unavailable here." : pending() ? "Waiting for output…" : "No output was recorded."} />
      <Show when={!hidden() && !ref() && fromTail()}><div class="cap">Recorded command tail only.</div></Show>
    </div>}>{key => <CommandDetail request={JSON.parse(key)} pending={pending()} read={read!} />}</Show>
  </>;
}

/** File reads and edits keep their existing presentation. Other calls open on recorded Input and Output. */
export function StepRow(props: { view: StepView; taskId: string; tone?: string; icon?: string; status?: JSX.Element; extra?: JSX.Element; hasExtra?: boolean; args?: string; output?: string; tail?: string; active?: boolean; pending?: boolean; commandRef?: CommandOutputRef; fallbackCommand?: string; state: OpenState; stateKey: string }) {
  const open = () => props.state.is(props.stateKey);
  const detail = () => props.view.detail;
  const recorded = () => hasRecordedDetail(props.view);
  const expandable = () => recorded() || (!!detail() && stepHasContent(detail()!)) || !!props.hasExtra;
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
      <Show when={recorded()} fallback={<Show when={detail()}>{d => <StepDetail step={d()} taskId={props.taskId} />}</Show>}>
        <ToolDetail view={props.view} taskId={props.taskId} args={props.args} output={props.output} tail={props.tail}
          active={props.active} pending={props.pending} commandRef={props.commandRef} fallbackCommand={props.fallbackCommand} />
      </Show>
      <Show when={!privateStep(props.view)}>{props.extra}</Show>
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
