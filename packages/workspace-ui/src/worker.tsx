import { createComputed, createEffect, createMemo, createSignal, For, Index, Match, on, onCleanup, Show, Switch, untrack, type Accessor, type Setter } from "solid-js";
import { createOpenState, DiffBox, Fold, OutputBox, StepRow, type OpenState } from "./feed";
import { FEATURES, type Activity, type Check, type Message, type Worker } from "@cortex/workspace-domain";
import { AttachChips, createAttachments, createDictation, MessageComposer } from "./composer";
import { useWs } from "./ctx";
import { createTicker, createUnsent } from "./freshness";
import { A, Count, createFollow, I, Loading, MenuItem, Modal, Pop } from "./kit";
import { ActionBar } from "./layout";
import type { Route } from "./router";
import { clock, refusalLine, refusalReason } from "./selectors";
import { displayStep, feedOf, isHiddenActivity, oneRowPerCall, stepFailed, viewOfActivity, type FeedItem, type FeedRun } from "./tool-names";
import { contextLine, duration, elapsedOf, isFinished, recordedCoordinatorRoute, reviewLine, tokens, verificationFace, workerModel, workerState, type SessionModel } from "./session-model";

import { ResultText } from "./result-text";
import { CopySource, MessageTools } from "./message-copy";
import { SentMessages } from "./sent";
import { useTaskRoutes } from "./task-place";
import type { FollowPosition } from "./follow";

export const CORTEX = "cortex", VERIFICATION = "verification";

/** The live line. A worker's page derives it from that worker's `activity`; the Conversation passes the coordinator's own `text`. */
export function WorkingIndicator(props: { activity?: Activity[]; text?: string; showAllTools?: boolean }) {
  const label = () => {
    if (props.text !== undefined) return props.text;
    const last = props.activity?.filter(a => !isHiddenActivity(a, props.showAllTools)).at(-1);
    if (last?.command?.state === "running") return "Running command…";
    const detail = displayStep(last?.args);
    if (detail?.status === "in_progress" && detail.kind === "edit") return "Editing…";
    if (detail?.status === "in_progress" && detail.kind === "read") return "Reading…";
    return "Working…";
  };
  return <div class="ws-working" role="status" aria-live="polite"><I n="loader" s={12} class="spin" /><span>{label()}</span></div>;
}
/** The same last line while the task waits for the owner's answer and nothing runs: the header's state, not a spinner. */
export function WaitingIndicator() {
  return <div class="ws-working ws-waiting" role="status" aria-live="polite"><I n="question" s={12} class="st-warn" /><span>Waiting for you</span></div>;
}

export const CHECK_ICON: Record<Check["outcome"], { icon: string; tone: string }> = {
  passed: { icon: "check-circle", tone: "st-ok" }, failed: { icon: "x-circle", tone: "st-bad" }, not_run: { icon: "circle", tone: "faint" }, unverified: { icon: "help", tone: "faint" },
};
const checkSource = (c: Check, none: string) => c.source === "runtime" ? "Recorded by runtime" : c.source === "worker" ? "Reported by worker" : c.outcome === "not_run" ? "Not run" : c.outcome === "unverified" ? "Unverified" : none;

const Face = (props: { w: Worker }) => {
  const f = () => workerState(props.w);
  return <span class={`st ${f().tone}`}><I n={f().icon} s={12} class={f().spin ? "spin" : undefined} />{f().label}</span>;
};
const Fact = (props: { label: string; children: string }) => <span><span class="l">{props.label}</span>{props.children}</span>;
/** A figure nobody has reported is left out, not shown as a placeholder. */
const Figure = (props: { label: string; value: string | null }) => <Show when={props.value}>{v => <Fact label={props.label}>{v()}</Fact>}</Show>;
/** A retried worker names the model of each earlier attempt, so a change of model is never silent. */
const fallbacks = (w: Worker, label: (r: Worker["route"]) => string | null) => (w.routeAttempts?.length ?? 0) > 1 ? w.routeAttempts!.map(a => `${a.n}: ${label(a.route) ?? "not recorded"}`).join(", ") : null;
function Grid(props: { w: Worker; now: number }) {
  const ws = useWs();
  return <div class="d-grid">
  <Figure label="Model" value={workerModel(props.w, ws.route)} />
  <Figure label="Why this model" value={props.w.route?.why ?? null} />
  <Figure label="Attempts" value={fallbacks(props.w, ws.route)} />
  <Show when={props.w.attempt}>{a => <Fact label="Attempt">{`${a().n} of ${a().of}`}</Fact>}</Show>
  <Show when={elapsedOf(props.w, props.now) !== null}><Fact label="Elapsed">{duration(elapsedOf(props.w, props.now))}</Fact></Show>
  <Figure label="Context" value={contextLine(props.w.context)} />
  <Figure label="Usage" value={props.w.usage === null ? null : tokens(props.w.usage)} />
</div>;
}

/** A note of who said what and whether it arrived. Delivered never claims the worker read it. */
function MessageBubble(props: { m: Accessor<Message>; item: Activity; workerActive: boolean; attempt: number | null }) {
  const ws = useWs();
  const m = () => props.m();
  const off = () => m().delivery === "withdrawn";
  const bad = () => m().delivery === "failed" || m().delivery === "unverified";
  const stamp = () => m().at === null ? "You" : `You, ${clock(m().at)}`;
  const retry = () => ws.run({ type: "retry_delivery", messageId: m().id });
  return <div class={`me${off() ? " off" : bad() ? " bad" : ""}`}>
    <div class="b"><ResultText breaks text={m().text} /><AttachChips files={m().attachments} open /></div>
    <div class="ms">
      <Switch>
        <Match when={m().delivery === "sending"}><I n="loader" s={12} class="spin ws-wait" /><span>Sending…</span></Match>
        <Match when={m().delivery === "recorded"}><I n="clock" s={12} /><span>{stamp()}. Pending delivery at the next step</span>
          <span class="act"><button class="lnk" {...ws.needs("withdraw_message")} onClick={() => ws.run({ type: "withdraw_message", messageId: m().id })}>Withdraw</button></span></Match>
        <Match when={m().delivery === "delivered"}><I n="check" s={12} /><span>{stamp()}.{m().deliveredAt === null ? " Delivered." : ` Delivered ${clock(m().deliveredAt)}.`} Delivered does not mean read.</span></Match>
        <Match when={off()}><I n="undo" s={12} /><span>Withdrawn by you, before delivery</span></Match>
        <Match when={bad()}><I n="alert" s={12} class="st-bad" /><span class="st-bad">{m().failure ?? "Delivery not confirmed"}</span>
          <span class="act">
            <Show when={props.workerActive}><button class="lnk" {...ws.needs("redirect_message")} onClick={() => ws.run({ type: "redirect_message", messageId: m().id, now: ws.store.clock() })}>Send</button>
              <button class="lnk" {...ws.needs("retry_delivery")} onClick={retry}>{props.attempt && /attempt/.test(m().failure ?? "") ? `Retry on attempt ${props.attempt}` : "Retry"}</button></Show>
            <button class="lnk" {...ws.needs("withdraw_message")} onClick={() => ws.run({ type: "withdraw_message", messageId: m().id })}>Discard</button>
          </span></Match>
      </Switch>
      <CopySource text={m().text} />
    </div>
  </div>;
}

function commandStatus(a: Activity): { tone: string; icon?: string; spin?: boolean; text: string } {
  const c = a.command!, took = c.took ? `, ${c.took}` : "";
  switch (c.state) {
    case "running": return { tone: "st-run", icon: "loader", spin: true, text: "Running" };
    case "stopping": return { tone: "st-warn", icon: "loader", spin: true, text: "Stop requested" };
    case "stopped": return { tone: "st-idle", icon: "stop", text: c.reason ? `Stopped: ${c.reason}` : "Stopped" };
    // An exit code nobody reported is left out, never filled in.
    case "passed": return { tone: "st-ok", icon: "check-circle", text: `Passed${c.exit === null ? "" : `, exit ${c.exit}`}${took}` };
    case "failed": return { tone: "st-bad", icon: "x-circle", text: `Failed${c.exit === null ? "" : `, exit ${c.exit}`}${took}` };
    // The engine ran it itself and reported no outcome; saying nothing is truer than a label nobody can act on.
    case "native": return { tone: "", text: "" };
  }
}

/** Recorded input worth opening a row for: an empty `{}` is not. */
const hasInput = (args?: string) => !!args && !/^(?:\{\s*\}|\[\s*\]|null)$/.test(args.trim());

/** What a step that carries no selected detail shows when opened: its recorded input and output, rows, diff and command tail. */
function StepExtra(props: { a: Activity; w: Worker; onStopCommand: (a: Activity) => void }) {
  const ws = useWs();
  const a = () => props.a;
  const detail = createMemo(() => displayStep(a().args));
  const canStop = () => a().command?.state === "running" && props.w.status === "working";
  return <>
    <Show when={!detail() && (hasInput(a().args) || a().output)}><div class="card2 ev-detail">
      <Show when={hasInput(a().args)}><div class="ev-dh">Input</div><div class="args">{a().args}</div></Show>
      <Show when={a().output}>{out => <><div class="ev-dh">Output</div><OutputBox label="Output">{out()}</OutputBox></>}</Show>
    </div></Show>
    {/* By place, not by object: a refreshed record of the step rebuilds its rows, and they must not redraw under a reader selecting text. */}
    <Show when={a().rows}>{rows => <div class="card2"><div class="toolrows"><Index each={rows()}>{r => <div><I n={r().icon} s={12} class="faint" /><code>{r().code}</code><span class="r">{r().note}</span></div>}</Index></div></div>}</Show>
    <Show when={!detail() && a().diff}>{d => <DiffBox label="Diff" lines={d().lines} />}</Show>
    <Show when={!detail() && (a().command?.tail || a().command?.state === "running")}>
      <OutputBox label="Command output" class="tail">{a().command!.tail || "Waiting for output…"}</OutputBox>
    </Show>
    <Show when={canStop()}><div class="ws-step-acts"><button class="btn btn-sm" {...ws.needs("stop_command")} onClick={() => { if (!ws.gate("stop_command")) props.onStopCommand(a()); }}>Stop command…</button></div></Show>
  </>;
}

/** One tool step of the feed: the same row as Conversation, with what only a worker's record knows. */
function ActivityStep(props: { a: Activity; w: Worker; state: OpenState; onStopCommand: (a: Activity) => void }) {
  const a = () => props.a;
  const view = createMemo(() => viewOfActivity(a()));
  const failed = () => stepFailed(a());
  // An edit with no recorded lines is its path and counts on one plain row: it opens on nothing.
  const hasExtra = () => !!a().rows || a().command?.state === "running" || (!displayStep(a().args) && !!(hasInput(a().args) || a().output || a().diff?.lines.length || a().command?.tail));
  const status = () => a().command ? commandStatus(a()) : null;
  return <StepRow view={view()} taskId={props.w.taskId} tone={failed() ? "st-bad" : a().command?.state === "running" ? "st-run" : undefined} icon={failed() ? "alert" : undefined} hasExtra={hasExtra()} state={props.state} stateKey={`step:${a().sequence}`} extra={<StepExtra a={a()} w={props.w} onStopCommand={props.onStopCommand} />}
    status={<>
      <Show when={a().note}><span class="st st-ok"><I n="check-circle" s={12} />{a().note}</span></Show>
      <Show when={a().refusal}>{code => <span class="st st-bad">Refused: {refusalReason(code())}</span>}</Show>
      <Show when={a().diff}>{d => <span class="tnum"><span class="add">+{d().add}</span> <span class="del">−{d().del}</span></span>}</Show>
      <Show when={status()?.text}><span class={`st ${status()!.tone}`}><Show when={status()!.icon}><I n={status()!.icon!} s={12} class={status()!.spin ? "spin" : undefined} /></Show>{status()!.text}</span></Show>
    </>} />;
}

/** Consecutive tool steps as one block, collapsed until the owner opens it. */
function Run(props: { run: FeedRun<Activity>; runKey: string; state: OpenState; w: Worker; onStopCommand: (a: Activity) => void }) {
  const ws = useWs();
  const items = createMemo(() => new Map(props.run.items.map(a => [a.sequence, a])));
  const running = () => props.w.status === "working" ? props.run.items.find(a => a.command?.state === "running") : undefined;
  return <Fold text={props.run.text} open={props.state.is(props.runKey)} loading={props.run.loading} onToggle={() => props.state.toggle(props.runKey)}
    right={<Show when={running()}>{c => <button class="btn btn-sm" {...ws.needs("stop_command")} onClick={() => { if (!ws.gate("stop_command")) props.onStopCommand(c()); }}>Stop command…</button>}</Show>}>
    {() => <For each={props.run.items.map(a => a.sequence)}>{sequence => <ActivityStep a={items().get(sequence)!} w={props.w} state={props.state} onStopCommand={props.onStopCommand} />}</For>}
  </Fold>;
}

function Row(props: { item: Activity; w: Worker; message: (id: string) => Message | undefined }) {
  const a = () => props.item;
  const message = () => a().messageId ? props.message(a().messageId!) : undefined;
  return <Switch fallback={<div class="ev-note"><div class="tx ai">{a().text}<Show when={a().code}>{" "}<code>{a().code}</code></Show></div></div>}>
    <Match when={a().kind === "note"}>{(() => {
      const text = () => a().text.replace(/^.*?\[(?:worker message|final response)\] /, "");
      return <><div class="ev-note"><div class="tx ai"><ResultText text={text()} /></div></div><MessageTools text={text()} /></>;
    })()}</Match>
    <Match when={a().kind === "divider"}><div class="divide">{a().text}</div></Match>
    <Match when={message()}>{m => <MessageBubble m={m} item={a()} workerActive={props.w.status === "working"} attempt={props.w.attempt?.n ?? null} />}</Match>
  </Switch>;
}

/** Sending stays with the worker's current attempt, and it can never widen the worker's scope or access. While the worker
 *  runs and the field is empty, the action button is Stop, as in the task's own composer: it opens the stop dialog, where
 *  the owner may say why. Typing turns it back into Send. */
function WorkerComposer(props: { model: Accessor<SessionModel>; w: Worker; onSend?: () => void; onStop?: () => void }) {
  const ws = useWs();
  const key = () => `${props.model().view.id}/${props.w.id}`;
  const [text, setText] = createSignal(ws.drafts.get(key()) ?? "");
  const write = (v: string) => { setText(v); ws.drafts.set(key(), v); };
  const mic = createDictation();
  const files = createAttachments();
  createUnsent(() => !!text().trim() || files.files().length > 0);
  const idBase = () => "live";
  const closed = () => props.model().mode === "offline" ? "The machine is disconnected. Use Re-check state above before messaging."
    : props.model().view.unknown ? "This task's state is not confirmed. Use Re-check state above before messaging."
    : props.w.status === "working" ? undefined : `${props.w.role} is not running, so it cannot be messaged`;
  const empty = () => !text().trim();
  // A machine that cannot reach one worker still reaches Cortex, which runs them all. The message says who it is for, and the field says where it goes.
  const direct = () => ws.can(FEATURES.workerMessages);
  const send = (value: string) => {
    if (!value.trim() || closed() || files.busy()) return;
    const taskId = props.model().view.id, now = ws.store.clock();
    // A direct message reads in this feed from the press, and leaves the field then; one that did not arrive keeps its text
    // and files on its line, with Retry and Edit. Through the coordinator it reads in the task's conversation, so the field
    // keeps it until the machine has it.
    if (direct()) { const attachments = files.files(); write(""); files.clear(); void ws.run({ type: "send_message", taskId, workerId: props.w.id, text: value, mode: "send_now", now, attachments }); return; }
    ws.ok({ type: "send_message", taskId, text: `For ${props.w.role}: ${value}`, mode: "send_now", now, attachments:files.files() }, () => { write(""); files.clear(); });
    // Sent from this feed, it brings this feed's end back as a direct message does, though it reads in the conversation.
    props.onSend?.();
  };
  createEffect(() => {
    const back = ws.store.sends.returned({ kind: "worker", taskId: props.model().view.id, workerId: props.w.id });
    if (!back) return;
    ws.store.sends.take();
    write(untrack(text).trim() ? `${untrack(text).trimEnd()}\n${back.text}` : back.text); files.add(back.attachments ?? []);
  });
  createEffect(() => { if (closed()) mic.stop(); });
  // The worker's recorded model and effort: read-only, because a message never changes a worker's route.
  const route = () => ws.route(props.w.route);
  const live = () => props.w.status === "working" || props.w.status === "waiting_you";
  const stop = () => !!props.onStop && live() && empty() && files.files().length === 0;
  const stopEffect = () => props.w.status === "working" ? `Stop ends only ${props.w.role}; the task keeps running. Write a message to send it instead.`
    : `Stop ends only ${props.w.role}; the task keeps running.`;
  return <MessageComposer group="Worker composer" label={`Message ${props.w.role}`} placeholder={direct() ? `Message ${props.w.role} directly, tag @files…` : `Message ${props.w.role}, tag @files…`} text={text()} onText={write} files={files} mic={mic} off={closed()}
    suggest={{ projectId: props.model().view.project.id, taskId: props.model().view.id }}
    attachOff={direct() && !ws.can(FEATURES.workerAttachments) ? "Files can't be sent to one worker. Attach them to a message about the task instead." : undefined}
    effect={stop() ? stopEffect() : direct() ? `To ${props.w.role}, this attempt only. It does not widen scope or access.` : `To the coordinator about ${props.w.role}. It decides how to pass it on and does not widen scope or access.`}
    route={<div class="c-route"><Show when={route()}>{r => <span class="sel mc-fixed" role="note" aria-label={`Recorded model: ${r()}`}><I n="cpu" s={14} /><span class="sel-t">{r()}</span></span>}</Show></div>}
    delivery={<Show when={direct()}>
      <button type="button" class="sel" popovertarget={`m-delivery-${idBase()}`} aria-label="Delivery: Next step"><I n="clock" s={14} /><span class="sel-t">Next step</span><I n="chevron-down" s={12} /></button>
      <Pop id={`m-delivery-${idBase()}`} class="pop-up" align="up" label="Delivery to this worker" style={{ width: "300px" }}>
        <div class="mi-h">Delivery to this worker</div>
        <MenuItem label="Next step" sub="Delivered at the worker's next tool call" check />
        <MenuItem label="Interrupt now" disabled="Unavailable: not qualified for this engine" />
      </Pop>
    </Show>}
    send={stop() ? { label: "Stop worker…", needs: "stop_worker", go: () => props.onStop?.(), run: { icon: "stop" } }
      : { label: `Send to ${props.w.role}`, needs: "send_message", why: files.busy() ? "Attachments are still loading" : empty() ? "Write a message first" : undefined, go: () => send(text()), insertAndSend: send }} />;
}

/** A reason for a stop: the shared writing surface with dictation. The stop itself is the dialog's own confirmation. */
function ReasonField(props: { label: string; value: string; onText: (value: string) => void }) {
  const files = createAttachments(), mic = createDictation();
  return <div class="fld"><span class="lab">{props.label}</span>
    <MessageComposer class="mc-inline" label={props.label} placeholder="Write the reason…" text={props.value} onText={props.onText} files={files} mic={mic} attachOff="A stop reason is text only. Files cannot be sent with it." />
  </div>;
}

export function StopCommand(props: { target: { worker: Worker; item: Activity } | null; onClose: () => void }) {
  const ws = useWs();
  const [reason, setReason] = createSignal("");
  // Keyed by the command, not by the object a refresh replaces: what the owner typed survives a background update.
  const commandKey = createMemo(() => props.target ? `${props.target.worker.id}/${props.target.item.sequence}` : "");
  createEffect(on(commandKey, key => { if (key) setReason(""); }));
  const stop = () => {
    const t = props.target!;
    ws.ok({ type: "stop_command", workerId: t.worker.id, sequence: t.item.sequence, reason: reason() }, () => { props.onClose(); });
  };
  return <Modal open={!!props.target} onClose={props.onClose} label="Stop this command?">
    <Show when={props.target}>{t => <>
      <div class="dlg-h"><h2>Stop this command?</h2><button class="ib" aria-label="Keep it running" onClick={() => props.onClose()}><I n="x" /></button></div>
      <div class="dlg-b">
        <div class="card2"><div class="card2-h"><I n="terminal" s={12} /><code>{t().item.text}</code><span class="r"><span class="st st-run"><I n="loader" s={12} class="spin" />Running</span></span></div></div>
        <div class="cap muted">{t().worker.role} keeps working and will not repeat this command. The task stays in Working.</div>
        <ReasonField label="Reason, sent to the worker" value={reason()} onText={setReason} />
      </div>
      <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Keep running</button>
        <button class="btn btn-primary btn-lg" aria-disabled={reason().trim() ? undefined : "true"} data-why={reason().trim() ? undefined : "Say why, so the worker can adapt"} {...ws.needs("stop_command")} onClick={() => { if (reason().trim()) stop(); }}>Stop command</button></ActionBar>
    </>}</Show>
  </Modal>;
}

function StopWorker(props: { worker: Worker | null; onClose: () => void }) {
  const ws = useWs();
  const [reason, setReason] = createSignal("");
  const workerKey = createMemo(() => props.worker?.id ?? "");
  createEffect(on(workerKey, key => { if (key) setReason(""); }));
  return <Modal open={!!props.worker} onClose={props.onClose} label="Stop this worker?">
    <Show when={props.worker}>{w => <>
      <div class="dlg-h"><h2>Stop {w().role}?</h2><button class="ib" aria-label="Keep working" onClick={() => props.onClose()}><I n="x" /></button></div>
      <div class="dlg-b">
        <div class="cap muted">Only this worker stops. The task keeps running, and its messages that have not arrived are marked as not delivered.</div>
        <ReasonField label="Reason, optional" value={reason()} onText={setReason} />
      </div>
      <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Keep working</button>
        <button class="btn btn-primary btn-lg" {...ws.needs("stop_worker")} onClick={() => { ws.ok({ type: "stop_worker", workerId: w().id, reason: reason().trim() || undefined, now: ws.store.clock() }, () => { props.onClose(); }); }}>Stop worker</button></ActionBar>
    </>}</Show>
  </Modal>;
}

/** The coordinator's transcript records its reply as it streamed, one piece per row. Pieces next to each other are one message,
 *  so they are read as one, word for word: shown apart, every piece became its own paragraph of one or two words. */
function joinStreamedText(list: Activity[], showPlumbing: boolean): Activity[] {
  const out: Activity[] = [];
  // A hidden step of the runtime's own still ends a message: the text before and after it are two messages, though nothing shows between them.
  let ended = false;
  for (const a of list) {
    if (a.kind !== "message" && a.kind !== "divider" && isHiddenActivity(a, showPlumbing)) { if (a.kind !== "note") ended = true; continue; }
    const last = out.at(-1);
    if (!ended && last?.kind === "note" && a.kind === "note") out[out.length - 1] = { ...last, text: last.text + a.text };
    else out.push(a);
    ended = false;
  }
  return out;
}

/** Everything a worker did, newest at the bottom. New events never move a reader away from the end; scrolling back to the
 *  end follows again, and Latest brings it back. */
function Stream(props: { model: Accessor<SessionModel>; w: Worker; readOnly?: boolean; onAsk?: () => void; onStop?: () => void; position?: FollowPosition }) {
  const ws = useWs();
  const [command, setCommand] = createSignal<{ worker: Worker; item: Activity } | null>(null);
  let scroller: HTMLDivElement | undefined;
  const [older, setOlder] = createSignal<Activity[]>([]);
  const [olderMessages, setOlderMessages] = createSignal<Message[]>([]);
  const [cursor, setCursor] = createSignal<string | null | undefined>();
  const [historyError, setHistoryError] = createSignal(""), [loadingOlder, setLoadingOlder] = createSignal(false);
  const identity = createMemo(() => props.w.id + "/" + (props.w.attempt?.n ?? ""));
  let generation = 0, observedIdentity = "", arrivals = 0;
  const seenItems = new Set<number>(), runIds = new Map<number, string>(), loadedCursors = new Set<string>();
  const opened = createOpenState();
  createEffect(on(identity, () => { generation++; loadedCursors.clear(); setOlder([]); setOlderMessages([]); setCursor(undefined); setHistoryError(""); setLoadingOlder(false); opened.clear(); }));
  onCleanup(() => { generation++; });
  // Keep live rows when a refreshed service snapshot advances its bounded tail.
  createEffect(() => {
    const activity = props.w.activity.map(a => ({ ...a }));
    setOlder(previous => [...new Map([...previous, ...activity].map(a => [a.sequence, a])).values()]);
  });
  const all = createMemo(() => {
    const rows = oneRowPerCall([...new Map([...older(), ...props.w.activity].map(a => [a.sequence, a])).values()]
      .sort((a,b) => (a.at ?? 0)-(b.at ?? 0) || a.sequence-b.sequence));
    const showPlumbing = ws.panels.showAllTools();
    return props.w.id.startsWith("coordinator:") ? joinStreamedText(rows, showPlumbing)
      : rows.filter(a => a.kind === "message" || a.kind === "divider" || !isHiddenActivity(a, showPlumbing));
  });
  const observed = createMemo(() => {
    const current = identity();
    if (observedIdentity !== current) { observedIdentity = current; seenItems.clear(); }
    // A snapshot can briefly remove rows while Solid reconciles its store.
    // Retain observed identities: neither that reconciliation nor the rolling
    // 200-event tail makes an already seen event a new arrival.
    for (const item of props.w.activity.filter(a => !isHiddenActivity(a, ws.panels.showAllTools()))) {
      if (!seenItems.has(item.sequence)) { seenItems.add(item.sequence); arrivals++; }
    }
    return arrivals;
  });
  // A message sent here counts from the press; its record taking its place is not a second arrival.
  const sentHere = () => ({ kind: "worker" as const, taskId: props.model().view.id, workerId: props.w.id });
  const nextCursor = () => cursor() === undefined ? props.w.activityCursor : cursor();
  const position = props.position;
  // A failed page leaves its cursor available for Retry; the partial feed still cannot restore the saved offset.
  const follow = createFollow(() => scroller, () => observed() + ws.store.sends.shown(sentHere()).length,
    position && { position, ready: () => !loadingOlder() && !nextCursor() });
  // What the owner just sent here comes into view above the composer and following turns on, even for a reader who had
  // scrolled up. A row the reader opens stays in view: the follow controller watches the feed's own toggles.
  let pressedSeen = ws.store.sends.pressed(sentHere());
  createEffect(() => { const n = ws.store.sends.pressed(sentHere()); if (n > pressedSeen) follow.jump(); pressedSeen = n; });
  const loadOlder = async () => {
    const next = nextCursor();
    if (!next || loadingOlder()) return;
    const own = generation;
    setLoadingOlder(true); setHistoryError("");
    try {
      const page = await ws.store.readWorkerActivity(props.w.taskId, props.w.id, next);
      if (own !== generation) return;
      setOlder([...page.activity, ...older()]);
      setOlderMessages([...page.messages ?? [], ...olderMessages()]);
      if (page.activityCursor && (page.activityCursor === next || loadedCursors.has(page.activityCursor))) throw new Error("Activity paging did not advance. Retry to load the remaining history.");
      loadedCursors.add(next);
      setCursor(page.activityCursor ?? null);
      // The shared follow controller preserves the visible row as older activity is prepended.
    } catch (e) { if (own === generation) setHistoryError(e instanceof Error ? e.message : "Activity could not be loaded. Retry."); }
    finally { if (own === generation) setLoadingOlder(false); }
  };
  createEffect(() => {
    if (nextCursor() && !loadingOlder() && !historyError()) untrack(() => void loadOlder());
  });
  let runIdsOf = "", lastShown = new Map<string, FeedItem<Activity>>();
  const shown = createMemo(() => {
    if (runIdsOf !== identity()) { runIdsOf = identity(); runIds.clear(); lastShown = new Map(); }
    const out = feedOf(all(), runIds, lastShown, ws.panels.showAllTools());
    lastShown = new Map(out.map(x => [x.key, x]));
    return out;
  });
  // Rows keep their keys through a streaming update, so a new event only appends.
  const keys = createMemo(() => shown().map(x => x.key), undefined, { equals: (a, b) => a.length === b.length && a.every((k, i) => k === b[i]) });
  // Each row reads its own entry, which is told only when that entry changed: an update to one row of a long feed wakes
  // that row, not every row (test/scroll-perf.driver.mjs, `-live`).
  const slots = new Map<string, { get: Accessor<FeedItem<Activity> | undefined>; set: Setter<FeedItem<Activity> | undefined> }>();
  const slot = (key: string) => {
    let held = slots.get(key);
    if (!held) { const [get, set] = createSignal(lastShown.get(key)); slots.set(key, held = { get, set }); }
    return held;
  };
  createComputed(() => { for (const entry of shown()) slots.get(entry.key)?.set(() => entry); });
  const message = (id: string) => props.model().messages.find(m => m.id === id) ?? olderMessages().find(m => m.id === id);
  const running = () => props.w.status === "working";
  const typing = () => <Show when={running()}><WorkingIndicator activity={all()} showAllTools={ws.panels.showAllTools()} /></Show>;
  const rows = () => <>
    <Show when={loadingOlder()}><Loading class="cap faint" text="Loading recorded activity…" /></Show>
    <Show when={historyError()}><div class="cap st-warn" role="alert">{historyError()} <button class="lnk" onClick={() => void loadOlder()}>Retry</button></div></Show>
    <Show when={ws.t("activityNote")}><div class="cap faint">{ws.t("activityNote")}</div></Show>
    <For each={keys()}>{key => {
      const entry = slot(key).get;
      onCleanup(() => slots.delete(key));
      const stop = (a: Activity) => setCommand({ worker: props.w, item: a });
      return <Show when={entry()}>{e => "run" in e() ? <Run run={(e() as { run: FeedRun<Activity> }).run} runKey={key} state={opened} w={props.w} onStopCommand={stop} />
        : <Row item={(e() as { item: Activity }).item} w={props.w} message={message} />}</Show>;
    }}</For>
    <SentMessages place={sentHere()} worker />
    <Show when={keys().length === 0}><div class="cap faint">No activity recorded yet.</div></Show>
    {typing()}
  </>;
  return <>
    <div class="stream" ref={scroller} onScroll={follow.onScroll}><div class="stream-in">
      {rows()}
      <Show when={isFinished(props.w) && !props.readOnly}><Finished w={props.w} onAsk={props.onAsk} /></Show>
    </div></div>
    <div class="dock-w"><Pill follow={follow} /><Show when={!isFinished(props.w) && !props.readOnly}><WorkerComposer model={props.model} w={props.w} onSend={() => follow.jump()} onStop={props.onStop} /></Show></div>
    <StopCommand target={command()} onClose={() => setCommand(null)} />
  </>;
}
/** The one follow control of every feed: Latest, or "N new" when something arrived while the reader was away from the end.
 *  It floats just above the dock, over the feed's bottom edge and out of the dock's flow, so showing it never resizes the feed
 *  or moves the end. Its band has no surface of its own and takes no pointer: the feed shows and scrolls around the opaque
 *  pill. It shows only while the reader is away from the end. */
export const Pill = (props: { follow: ReturnType<typeof createFollow> }) => <Show when={!props.follow.following()}>
  <div class="pill-band"><button class="new-pill ws-pill" onClick={() => props.follow.jump()}><I n="arrow-down" s={12} />{props.follow.missed() ? `${props.follow.missed()} new` : "Latest"}</button></div>
</Show>;

/** A goal longer than this shows its first lines with a way to read the rest, so the activity below keeps its room. */
const GOAL_FOLD_CHARS = 280, GOAL_FOLD_LINES = 4;
export const longGoal = (goal: string) => goal.length > GOAL_FOLD_CHARS || goal.split("\n").length > GOAL_FOLD_LINES;
export function Head(props: { w: Worker; now: number }) {
  const [goalOpen, setGoalOpen] = createSignal(false);
  // A memo, so a refreshed worker record with the same id does not fold a goal the owner opened; only another worker does.
  const workerId = createMemo(() => props.w.id);
  createEffect(on(workerId, () => setGoalOpen(false), { defer: true }));
  const folds = () => longGoal(props.w.goal);
  return <div class="d-h">
    <div class="d-title">{props.w.role}<Face w={props.w} /></div>
    <div class={`d-goal${folds() ? goalOpen() ? " ws-goal-open" : " ws-goal-folded" : ""}`} id={`goal-${props.w.id}`}><ResultText breaks text={props.w.goal} /></div>
    <Show when={folds()}><button class="lnk ws-goal-toggle" aria-expanded={goalOpen() ? "true" : "false"} aria-controls={`goal-${props.w.id}`} onClick={() => setGoalOpen(!goalOpen())}>{goalOpen() ? "Show less of the goal" : "Show the whole goal"}</button></Show>
    <Grid w={props.w} now={props.now} />
    <ReviewDetails w={props.w} />
  </div>;
}

/** The one-line reason a worker with no result ended, from what the runtime recorded. */
const noResult = (ended: Worker["endedReason"]) => ended === "stopped_by_owner" ? "You stopped this worker before it reported a result."
  : ended === "cancelled_by_coordinator" ? "The coordinator cancelled this worker (the plan changed) before it reported a result."
  : ended === "worker_lost" ? "This worker was lost (the service restarted or its engine session ended) before it reported a result."
  : "This worker reported no result.";

function ReviewDetails(props: { w: Worker }) {
  const review = () => props.w.review;
  return <Show when={review() && reviewLine(props.w)}><div class="res-sec ws-worker-review" role="status">
    <div class="res-h">Review</div><div>{reviewLine(props.w)}</div>
    <Show when={review()?.notCovered?.length}><ul class="cap muted">{review()!.notCovered!.map(item => <li>{item.item}: {item.reason}</li>)}</ul></Show>
    <Show when={review()?.rerun?.reason}><div class="cap muted">{review()!.rerun!.reason}</div></Show>
  </div></Show>;
}

function Finished(props: { w: Worker; onAsk?: () => void }) {
  const ws = useWs();
  const r = () => props.w.result;
  const face = () => workerState(props.w);
  const [reportOpen, setReportOpen] = createSignal(false);
  const [page, setPage] = createSignal<import("@cortex/workspace-contract").ReportPage | null>(null);
  const [error, setError] = createSignal(""), [loading, setLoading] = createSignal(false);
  let reportGeneration = 0;
  const reportIdentity = createMemo(() => props.w.id + "/" + (r()?.reportId ?? ""));
  createEffect(on(reportIdentity, () => {
    reportGeneration++; setReportOpen(false); setPage(null); setError(""); setLoading(false);
  }));
  onCleanup(() => { reportGeneration++; });
  const closeReport = () => { reportGeneration++; setReportOpen(false); setLoading(false); };
  /** The service hands a report over in chunks; an opened report is only ever shown whole, so a block split across chunks still renders as one. */
  const read = async () => {
    const reportId = r()?.reportId;
    if (!reportId || loading()) return;
    const generation = reportGeneration;
    setLoading(true); setError(""); setPage(null); setReportOpen(true);
    try {
      const first = await ws.store.readReport(props.w.taskId, reportId);
      if (generation !== reportGeneration) return;
      let markdown = first.markdown, cursor = first.nextCursor;
      while (cursor) {
        const next = await ws.store.readReport(props.w.taskId, reportId, cursor);
        if (generation !== reportGeneration) return;
        if (next.sha256 !== first.sha256) throw new Error("The report changed. Reopen it.");
        markdown += next.markdown; cursor = next.nextCursor;
      }
      setPage({ ...first, markdown, nextCursor: null, partial: false });
    } catch (e) { if (generation === reportGeneration) setError(e instanceof Error ? e.message : "Report could not be loaded. Retry."); }
    finally { if (generation === reportGeneration) setLoading(false); }
  };
  return <>
    <Modal open={reportOpen()} onClose={closeReport} label="Worker report">
      <div class="dlg-h"><h2>{page()?.title ?? "Worker report"}</h2><button class="ib" aria-label="Close report" onClick={closeReport}><I n="x" /></button></div>
      <div class="dlg-b">
        <Show when={loading()}><Loading text="Loading report…" /></Show>
        <Show when={error()}><div role="alert">{error()} <button class="lnk" onClick={() => read()}>Retry</button></div></Show>
        <Show when={page()}>{p => <>
          <div class="cap muted">{p().totalCharacters} characters</div>
          <ResultText text={p().markdown} />
        </>}</Show>
      </div>
    </Modal>
    <div class="rcard ws-worker-result">
      <div class="rcard-h"><I n={face().icon} s={14} class={face().tone === "st-idle" ? "faint" : face().tone} />{r()?.outcome === "failed" ? "Reported failure" : "Worker result"}<Show when={r()?.reportId}><span class="r"><button class="btn btn-line btn-sm" onClick={() => void read()}>Open report</button></span></Show><Show when={r()?.summary}>{summary => <CopySource text={summary()} label="Copy the worker result as Markdown" />}</Show></div>
      <div class="rcard-b"><ResultText text={r()?.summary ?? noResult(props.w.endedReason)} />
        <Show when={props.w.resultRefusals}>{x => { const line = () => refusalLine(x(), props.w.status === "completed" && !!r() && r()!.outcome !== "failed"); return <div class={`cap ${line().tone} ws-mt4`} role="note">{line().text}</div>; }}</Show></div>
      <div class="res-sec"><div class="res-h">Checks</div>
        <Show when={(r()?.checks.length ?? 0) > 0} fallback={<div class="cap muted">No checks reported.</div>}>
          <For each={r()!.checks}>{c => <div class="chk"><I n={CHECK_ICON[c.outcome].icon} s={14} class={CHECK_ICON[c.outcome].tone} /><span>{c.name}<Show when={c.outcome === "not_run" || c.outcome === "unverified" || c.outcome === "failed"}> <span class="faint">{c.detail}</span></Show></span><span class="src">{checkSource(c, ws.t("unsourcedCheck"))}</span></div>}</For>
        </Show></div>
      <Show when={r()?.edits?.length}>
        <div class="res-sec"><div class="res-h">Recorded edits</div>
          <For each={r()!.edits}>{e => <div class="chk"><I n="pencil" s={14} class="faint" /><code>{e.path}</code><span class="src tnum"><span class="add">+{e.add}</span> <span class="del">−{e.del}</span></span></div>}</For>
          <div class="cap faint ws-mt4">Edits this worker recorded. Side effects of its commands may not appear. The full task diff is in Changes.</div></div>
      </Show>
      <Show when={r()?.attempts?.length}>
        <div class="res-sec"><div class="res-h">Attempts</div>
          <For each={r()!.attempts}>{a => <div class="chk"><I n={a.ok ? "check-circle" : "x-circle"} s={14} class={a.ok ? "st-ok" : "st-bad"} /><span>Attempt {a.n} <span class="faint">{a.detail}</span></span><span class="src">{a.at === null ? "" : clock(a.at)}</span></div>}</For></div>
      </Show>
      {/* On the result card itself, never floating over it: a floating button covered the result's last lines (its checks). */}
      <Show when={props.onAsk}><div class="res-sec ws-worker-ask"><button class="btn btn-line btn-sm" onClick={() => props.onAsk?.()}>Ask about this worker</button></div></Show>
    </div>
  </>;
}

/** The coordinator's own record, read the same way as a worker's feed. It stays current while the task runs. */
function CoordinatorFeed(props: { model: Accessor<SessionModel> }) {
  const ws = useWs();
  const m = () => props.model();
  const [feed, setFeed] = createSignal<Worker | null>(null), [error, setError] = createSignal(""), [loaded, setLoaded] = createSignal(false);
  const id = () => `coordinator:${m().view.id}`;
  const running = () => m().mode === "live" || m().mode === "stopping";
  let busy = false;
  const load = async () => {
    if (busy) return;
    busy = true;
    const taskId = m().view.id;
    try {
      const page = await ws.store.readWorkerActivity(taskId, `coordinator:${taskId}`);
      if (taskId !== m().view.id) return;
      setFeed({ id: `coordinator:${taskId}`, taskId, executionId: "", role: "Coordinator", status: running() ? "working" : "completed", lane: "execution", goal: "", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null, activity: page.activity, activityCursor: page.activityCursor ?? null, messages: page.messages });
      setError("");
    } catch (e) { setError(e instanceof Error ? e.message : "Activity could not be loaded. Retry."); }
    finally { busy = false; setLoaded(true); }
  };
  // Every change the machine reports may be new coordinator activity, and the timer covers the time between reports.
  createEffect(on(() => props.model(), () => void load()));
  createTicker(() => void load(), 2500, running);
  const noFeed = () => ws.store.mode !== "real" && error() !== "" && !feed();
  return <Show when={!noFeed()} fallback={<div class="d-scroll"><div class="res-sec"><div class="res-h">Now</div>{m().session.coordinator.note || ws.t("nothingToReport")}</div></div>}>
    <Show when={error()}><div class="cap st-warn ws-cf-err" role="alert">{error()} <button class="lnk" onClick={() => void load()}>Retry</button></div></Show>
    <Show when={feed()} fallback={<Show when={!error()}><div class="d-scroll"><Show when={loaded()} fallback={<Loading class="res-sec cap faint" text="Loading activity…" />}><div class="res-sec cap faint">No activity recorded yet.</div></Show></div></Show>}>
      {w => <Stream model={props.model} w={w()} readOnly />}
    </Show>
  </Show>;
}

/** The full worker stream, shared by every worker entry point. */
export function WorkerPane(props: { model: Accessor<SessionModel>; pick: string; now: number }) {
  const ws = useWs();
  const m = () => props.model();
  const [stopping, setStopping] = createSignal<Worker | null>(null);
  const positions = new Map<string, FollowPosition>();
  const worker = () => m().workers.find(w => w.id === props.pick) ?? null;
  // The coordinator is messaged from its conversation: the task's page, or the sprint's for the task planning a sprint.
  const routes = useTaskRoutes(() => ({ machine: m().view.machine.id, id: m().view.id }));
  const ask = (w: Worker) => {
    ws.drafts.set(m().view.id, `About ${w.role}: `);
    ws.go(routes.conversation());
  };
  const session = () => m().session;
  const w = () => worker()!;
  const coordinatorRoute = () => ws.route(recordedCoordinatorRoute(m(), ws.store.mode === "demo"));
  const check = () => verificationFace(m());
  const body = () => {
    if (props.pick === CORTEX) return <>
      <div class="d-h"><div class="d-title">Coordinator<span class={`st ${m().view.fact.tone}`}>{m().view.fact.text}</span></div>
        <div class="d-goal">Coordinator for this task. Message it from the composer in Conversation.</div>
        <div class="d-grid"><Figure label="Model" value={coordinatorRoute()} /><Figure label="Elapsed" value={duration(session().coordinator.elapsedMs) || null} /><Figure label="Context" value={contextLine(session().context)} /><Figure label="Usage" value={session().coordinator.usage === null ? null : tokens(session().coordinator.usage!)} /></div></div>
      <CoordinatorFeed model={props.model} />
    </>;
    if (props.pick === VERIFICATION) return <>
      <div class="d-h"><div class="d-title">Runtime verification<span class={`st ${check().tone}`}><I n={check().icon} s={12} class={check().spin ? "spin" : undefined} />{check().label}</span></div>
        <div class="d-goal">Checks that run on their own after review. This is not a worker, so it cannot be messaged.</div></div>
      <div class="d-scroll">
        <Show when={check().checks.length > 0} fallback={<div class="res-sec"><div class="res-h">Planned checks</div>
          <Show when={session().verification.length > 0} fallback={<div class="cap muted">{ws.t("noneRecorded")}</div>}>
            <For each={session().verification}>{c => <div class="chk"><I n="circle" s={14} class="faint" /><code>{c}</code><span class="src">Not run</span></div>}</For>
          </Show></div>}>
          <div class="res-sec"><div class="res-h">Checks</div><For each={check().checks}>{c => <div class="chk"><I n={CHECK_ICON[c.outcome].icon} s={14} class={CHECK_ICON[c.outcome].tone} /><code>{c.name}</code><span class="src">{checkSource(c, ws.t("unsourcedCheck"))}</span></div>}</For></div>
        </Show>
      </div>
    </>;
    if (!worker()) return <div class="note-panel"><span>This worker is not part of the task.</span></div>;

    if (w().status === "pending" || w().status === "not_started") return <>
      <Head w={w()} now={props.now} />
      <div class="d-scroll"><div class="res-sec"><div class="res-h">{w().waitsFor ? "Waits for" : "Starts when"}</div>{w().waitsFor ? `${w().waitsFor}. It starts on its own when that node is done.` : "All execution nodes are done. The review starts on its own."}</div></div>
    </>;
    // The keyed pane owns only the selected worker's DOM; keep positions above it.
    const id = w().id;
    let position = positions.get(id);
    if (!position) positions.set(id, position = {});
    return <>
      <Head w={w()} now={props.now} />
      <Show when={w().status === "waiting_you"}><div class="res-sec">Answer the open question in Conversation.</div></Show>
      <Stream model={props.model} w={w()} onAsk={() => ask(w())} onStop={() => setStopping(w())} position={position} />
    </>;
  };
  // A pane rebuilds only when it shows a different worker or kind of worker: the composer and scroll position must survive new events.
  const kind = createMemo(() => { const w = worker(); return `${props.pick}:${w ? (w.status === "pending" || w.status === "not_started" ? "later" : "live") : "other"}`; });
  return <>
    <Show when={kind()} keyed>{_ => body()}</Show>
    <StopWorker worker={stopping()} onClose={() => setStopping(null)} />
  </>;
}
