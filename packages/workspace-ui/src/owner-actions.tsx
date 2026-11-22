import { createEffect, createMemo, createSignal, For, onCleanup, Show, untrack } from "solid-js";
import type { OwnerAction } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { I } from "./kit";
import { ActionBar } from "./layout";
import { notificationKinds } from "./notifications";
import { queueTerminalCommand } from "./terminal-command";
import "./owner-actions.css";

/** How often the machine's open owner actions are read again. An action waits until the owner acts, so this is quick enough. */
export const OWNER_ACTIONS_POLL_MS = 5000;
/** Where this app keeps the actions it has already announced, so a relaunch does not announce them again. */
export const SEEN_OWNER_ACTIONS_KEY = "cw.notified-owner-actions";
const SEEN_LIMIT = 200;
const storage = () => { try { return typeof localStorage === "undefined" ? undefined : localStorage; } catch { return undefined; } };
function readSeen(): string[] {
  try {
    const saved: unknown = JSON.parse(storage()?.getItem(SEEN_OWNER_ACTIONS_KEY) ?? "[]");
    return Array.isArray(saved) ? saved.filter((id): id is string => typeof id === "string").slice(-SEEN_LIMIT) : [];
  } catch { return []; }
}
function writeSeen(ids: Set<string>) {
  try { storage()?.setItem(SEEN_OWNER_ACTIONS_KEY, JSON.stringify([...ids].slice(-SEEN_LIMIT))); } catch { /* this run still dedupes */ }
}
const keyOf = (a: OwnerAction) => `${a.projectId}|${a.actionId}`;
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

/**
 * Access a task lacks and only the owner can give: a sign-in to GitHub, Codex or Claude, or sudo. The task does not stop; the
 * card says what is missing and offers what the machine supports: the sign-in in this app's terminal, a pasted token, or a
 * check that the owner has signed in. Once the machine sees the access (or the owner dismisses the card) the task retries the
 * blocked work by itself. A token lives in its field only until it is sent; it goes to the machine once and is never kept here.
 */
export function OwnerActions() {
  const ws = useWs();
  const [actions, setActions] = createSignal<OwnerAction[]>([]);
  const [phoneTokens, setPhoneTokens] = createSignal(true);
  let reading = false;
  const read = () => {
    if (reading || ws.store.connection().state !== "connected") return;
    reading = true;
    void ws.store.ownerActions().then(next => {
      if (next) { setActions(next.actions); setPhoneTokens(next.phoneTokens); } else setActions([]);
    }, () => {}).finally(() => { reading = false; });
  };
  read();
  const timer = setInterval(read, OWNER_ACTIONS_POLL_MS);
  onCleanup(() => clearInterval(timer));
  const phone = () => ws.shell.kind === "phone";

  // A new action is announced once through the window's own notifications, like a new question.
  const seen = new Set<string>(readSeen());
  const [kinds] = notificationKinds();
  createEffect(() => {
    const list = actions();
    untrack(() => {
      let changed = false;
      for (const a of list) {
        const key = keyOf(a);
        if (seen.has(key)) continue;
        seen.add(key); changed = true;
        const route = ws.router.route();
        const onIt = route.name === "task" && route.task === a.taskId && !document.hidden;
        if (ws.notify && a.taskId && ws.panels.notifications() && kinds().includes("question") && !onIt)
          void ws.notify({ label: a.task ? `${a.title}: ${a.task}` : a.title, target: { kind: "task", project: a.projectId, task: a.taskId } }).catch(() => {});
      }
      if (changed) writeSeen(seen);
    });
  });

  const closed = (key: string) => { setActions(list => list.filter(x => keyOf(x) !== key)); read(); };
  // Cards are keyed by action, so a fresh read keeps what a card is showing (a note, a problem, a half-typed token).
  const keys = createMemo(() => actions().map(keyOf), undefined, { equals: (x, y) => x.length === y.length && x.every((k, i) => k === y[i]) });
  const byKey = createMemo(() => new Map(actions().map(a => [keyOf(a), a])));
  return <Show when={actions().length > 0}>
    <div class="owner-stack" data-shell={phone() ? "phone" : undefined}>
      <For each={keys()}>{key => <Show when={byKey().get(key)}>{a => <OwnerCard action={a()} phone={phone()} tokensBlocked={phone() && !phoneTokens()} onClosed={() => closed(key)} onRead={read} />}</Show>}</For>
    </div>
  </Show>;
}

function OwnerCard(props: { action: OwnerAction; phone: boolean; tokensBlocked: boolean; onClosed: () => void; onRead: () => void }) {
  const ws = useWs();
  const [busy, setBusy] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [note, setNote] = createSignal<string | null>(null);
  let field: HTMLInputElement | undefined;
  const a = () => props.action;
  const sudo = () => a().service === "sudo";
  // The sign-in runs in this window's own terminal only where the window has one; otherwise its command is shown.
  const inTerminal = () => !props.phone && ws.terminal !== undefined;
  const where = () => [a().projectName, a().task].filter(Boolean).join(" · ");
  const machineOf = () => ws.store.state().projects.find(p => p.id === a().projectId)?.machineId ?? ws.machine().id;
  const openTask = () => { const task = a().taskId; if (task) ws.go({ name: "task", machine: machineOf(), task }); };
  const run = (work: () => Promise<void>) => {
    if (busy()) return;
    setBusy(true); setProblem(null); setNote(null);
    void work().catch(error => setProblem(message(error))).finally(() => setBusy(false));
  };
  const settled = (reply: { outcome: string; status?: string }) => {
    if (reply.outcome === "resolved") { props.onClosed(); return; }
    if (reply.outcome === "still_needed") setNote(sudo() ? "sudo still needs a password." : "Still not signed in. Finish the sign-in, then check again.");
    props.onRead();
  };
  const terminal = () => run(async () => {
    const reply = await ws.store.actOwner(a().projectId, a().actionId, "terminal");
    const command = reply.outcome === "terminal" ? reply.command : a().terminal?.command;
    if (!command) return;
    openTask();
    queueTerminalCommand(a().projectId, command);
    if (!ws.panels.terminalOpen()) ws.toggleTerminal();
    setNote("The sign-in runs in the terminal below. Finish it there, then check again.");
  });
  const recheck = () => run(async () => settled(await ws.store.actOwner(a().projectId, a().actionId, "recheck")));
  const dismiss = () => run(async () => settled(await ws.store.actOwner(a().projectId, a().actionId, "dismiss")));
  const submit = (event: Event) => {
    event.preventDefault();
    if (busy() || !field) return;
    const token = field.value;
    field.value = "";
    if (!token) { setProblem("Paste the token first."); field.focus(); return; }
    run(async () => settled(await ws.store.ownerToken(a().projectId, a().actionId, token)));
  };
  return <form class="owner-card" role="dialog" aria-modal="false" aria-label={a().title} onSubmit={submit}>
    <div class="row"><I n={sudo() ? "shield" : "alert"} class="st-warn" /><span class="w500 trunc">{a().title}</span></div>
    <Show when={where()}>{w => <p class="cap muted trunc">{w()}</p>}</Show>
    <p class="owner-detail">{a().detail}</p>
    <Show when={sudo()}><p class="cap muted">When the command asks, a Password for sudo prompt opens here.</p></Show>
    <Show when={!sudo() && a().terminal}>{t => <Show when={!inTerminal()}><>
      <code class="owner-command">{t().command}</code>
      <p class="cap muted">{props.phone ? "Run this in a terminal on the computer, then check again here." : "Run this in a terminal on this computer, then check again here."}</p>
    </></Show>}</Show>
    <Show when={!sudo() && a().token}>{t => <Show when={!props.tokensBlocked} fallback={
      <div class="note-box plain" role="status"><I n="alert" s={14} class="faint" /><span>Passwords and tokens from phones are turned off on the computer. Paste the token there, or sign in on the computer.</span></div>}><>
      <label class="owner-token">
        <span class="cap">{t().label}</span>
        <input ref={field} class="inp" type="password" aria-label={t().label} placeholder="Paste a token" autocomplete="off" autocapitalize="off" spellcheck={false} enterkeyhint="send" />
        <Show when={t().hint}><span class="cap faint">{t().hint}</span></Show>
      </label>
      <p class="cap muted">The token goes to the sign-in on the machine once and is not saved or shown to the model.</p>
    </></Show>}</Show>
    <Show when={note()}>{n => <div class="cap muted" role="status">{n()}</div>}</Show>
    <Show when={problem()}>{why => <div class="cap st-warn" role="alert">{why()}</div>}</Show>
    <ActionBar place="card">
      <Show when={a().taskId}><button type="button" class="btn btn-line" onClick={openTask}>Open task</button></Show>
      <button type="button" class="btn btn-line" aria-busy={busy() ? "true" : undefined} onClick={dismiss}>Dismiss</button>
      <Show when={a().recheck}><button type="button" class="btn btn-line" aria-busy={busy() ? "true" : undefined} onClick={recheck}>{sudo() ? "Check again" : "I've signed in, check again"}</button></Show>
      <Show when={!sudo() && a().token && !props.tokensBlocked}><button type="submit" class="btn btn-line" aria-busy={busy() ? "true" : undefined}>Sign in with token</button></Show>
      <Show when={!sudo() && inTerminal() && a().terminal}><button type="button" class="btn btn-primary" aria-busy={busy() ? "true" : undefined} onClick={terminal}>Open sign-in in terminal</button></Show>
    </ActionBar>
  </form>;
}
