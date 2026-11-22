import { createSignal, For, onCleanup, Show } from "solid-js";
import type { SudoPrompt } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { I } from "./kit";
import { ActionBar } from "./layout";
import "./sudo-prompt.css";

/** How often the machine's open sudo prompts are read again. A prompt waits for minutes, so this is quick enough. */
export const SUDO_POLL_MS = 2000;

const left = (ms: number) => { const m = Math.max(0, Math.ceil(ms / 60000)); return m <= 1 ? "under a minute" : `${m} minutes`; };

/**
 * A command a task runs on the machine asked for root with sudo: the owner types the password here, on the desktop or the
 * phone, or cancels. Only the asking command waits; the task's other work goes on. The password lives in the field only until
 * it is sent, goes to the machine once and is never kept, shown or logged here.
 */
export function SudoPrompts() {
  const ws = useWs();
  const [prompts, setPrompts] = createSignal<SudoPrompt[]>([]);
  const [phoneAnswers, setPhoneAnswers] = createSignal(true);
  const [now, setNow] = createSignal(Date.now());
  let reading = false;
  const read = () => {
    if (reading || ws.store.connection().state !== "connected") return;
    reading = true;
    void ws.store.sudoPrompts().then(next => { if (next) { setPrompts(next.prompts); setPhoneAnswers(next.phoneAnswers); } else setPrompts([]); }, () => {})
      .finally(() => { reading = false; setNow(Date.now()); });
  };
  read();
  const timer = setInterval(read, SUDO_POLL_MS);
  onCleanup(() => clearInterval(timer));
  const phone = () => ws.shell.kind === "phone";
  const open = () => prompts().filter(p => p.expiresAt > now());
  const closed = (promptId: string) => setPrompts(list => list.filter(p => p.promptId !== promptId));
  return <Show when={open().length > 0}>
    <div class="sudo-stack" data-shell={phone() ? "phone" : undefined}>
      <For each={open()}>{p => <SudoCard prompt={p} now={now()} blocked={phone() && !phoneAnswers()} onClosed={() => { closed(p.promptId); read(); }} />}</For>
    </div>
  </Show>;
}

function SudoCard(props: { prompt: SudoPrompt; now: number; blocked: boolean; onClosed: () => void }) {
  const ws = useWs();
  const [busy, setBusy] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  let field: HTMLInputElement | undefined;
  const message = (error: unknown) => error instanceof Error ? error.message : String(error);
  const where = () => [props.prompt.projectName, props.prompt.task, props.prompt.worker].filter(Boolean).join(" · ");
  const submit = (event: Event) => {
    event.preventDefault();
    if (busy() || !field) return;
    const password = field.value;
    field.value = "";
    if (!password) { setProblem("Type the password, or cancel."); field.focus(); return; }
    setBusy(true); setProblem(null);
    void ws.store.answerSudo(props.prompt.promptId, password).then(props.onClosed, error => setProblem(message(error))).finally(() => setBusy(false));
  };
  const cancel = () => {
    if (busy()) return;
    if (field) field.value = "";
    setBusy(true); setProblem(null);
    void ws.store.cancelSudo(props.prompt.promptId).then(props.onClosed, error => setProblem(message(error))).finally(() => setBusy(false));
  };
  return <form class="sudo-card" role="dialog" aria-modal="false" aria-label="Password for sudo" onSubmit={submit}>
    <div class="row"><I n="shield" class="st-warn" /><span class="w500 trunc">Password for sudo</span><span class="cap faint sudo-left tnum" role="timer">Closes in {left(props.prompt.expiresAt - props.now)}</span></div>
    <Show when={where()}>{w => <p class="cap muted trunc">{w()}</p>}</Show>
    <code class="sudo-command">{props.prompt.command}</code>
    <Show when={props.prompt.cwd}>{cwd => <span class="cap faint mono trunc">in {cwd()}</span>}</Show>
    <p class="cap muted">Only this command waits. The password goes to sudo on the machine and is not saved or shown to the model.</p>
    <Show when={!props.blocked} fallback={<div class="note-box plain" role="status"><I n="alert" s={14} class="faint" /><span>Passwords and tokens from phones are turned off on the computer. Answer on the computer, or cancel.</span></div>}>
      <input ref={field} class="inp" type="password" aria-label="Sudo password" placeholder={props.prompt.prompt || "Password"} autocomplete="off" autocapitalize="off" spellcheck={false} enterkeyhint="send" />
    </Show>
    <Show when={problem()}>{why => <div class="cap st-warn" role="alert">{why()}</div>}</Show>
    <ActionBar place="card">
      <button type="button" class="btn btn-line" aria-busy={busy() ? "true" : undefined} onClick={cancel}>Cancel</button>
      <Show when={!props.blocked}><button type="submit" class="btn btn-primary" aria-busy={busy() ? "true" : undefined}>Submit</button></Show>
    </ActionBar>
  </form>;
}
