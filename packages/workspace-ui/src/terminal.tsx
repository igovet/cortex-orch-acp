import { createEffect, createSignal, onCleanup, Show } from "solid-js";
import { Terminal as Xterm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { inputChunks, type TerminalPage, type TerminalPort } from "./terminal-client";
import { useWs } from "./ctx";
import { I } from "./kit";

export function LiveTerminal(props: { projectId: string; repository: string; port: TerminalPort }) {
  const ws = useWs();
  let host!: HTMLDivElement, term: Xterm | undefined, fit: FitAddon | undefined;
  let alive = true, timer: ReturnType<typeof setTimeout>, session: string | undefined, cursor = 0;
  let sending = false, pending: string[] = [], pendingBytes = 0, opening = false, resizing = false;
  let epoch = 0, failed = false, wantedSize: { cols: number; rows: number } | undefined;
  const [status, setStatus] = createSignal("Not connected");
  const [error, setError] = createSignal("");
  const [running, setRunning] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const fail = (cause: unknown) => {
    if (!alive) return;
    failed = true; pending = []; pendingBytes = 0; clearTimeout(timer); setRunning(false);
    setError(cause instanceof Error ? cause.message : "Terminal disconnected. Nothing was resent.");
    setStatus("Disconnected");
  };
  const style = () => {
    if (!term || !host) return;
    const css = getComputedStyle(host), token = (name: string) => css.getPropertyValue(name).trim();
    term.options.fontFamily = token("--mono");
    term.options.theme = { background: token("--bg-sidebar"), foreground: token("--fg"), cursor: token("--fg"),
      selectionBackground: ws.theme.resolved() === "dark" ? "#3d4342" : "#dcdce0",
      black: token("--fg-3"), red: token("--danger"), green: token("--ok"), yellow: token("--warn"),
      blue: token("--run"), magenta: token("--merged"), cyan: token("--accent-bright"), white: token("--fg"),
      brightBlack: token("--fg-2"), brightRed: token("--danger"), brightGreen: token("--ok"), brightYellow: token("--warn"),
      brightBlue: token("--run"), brightMagenta: token("--merged"), brightCyan: token("--accent-bright"), brightWhite: token("--fg") };
  };
  async function output(page: TerminalPage, attempt: number) {
    if (!alive || attempt !== epoch) return;
    if (session && page.sessionId !== session) throw new Error("Terminal identity changed. Input was not resent.");
    session = page.sessionId; cursor = page.next;
    if (page.truncated) {
      term!.reset();
      setError("Earlier terminal output was discarded (64 KiB retained).");
    }
    if (page.data.length) await new Promise<void>(resolve => term!.write(new Uint8Array(page.data), resolve));
    if (!alive || attempt !== epoch) return;
    setRunning(!page.exited); setStatus(page.exited ? "Shell exited" : "Running");
  }
  async function poll(attempt: number) {
    if (!alive || failed || attempt !== epoch || !session) return;
    try {
      await output(await props.port.read(props.projectId, session, cursor), attempt);
      if (alive && !failed && attempt === epoch) timer = setTimeout(() => void poll(attempt), 100);
    } catch (cause) { if (attempt === epoch) fail(cause); }
  }
  async function send() {
    if (sending) return;
    sending = true;
    const attempt = epoch;
    try {
      while (alive && !failed && session && attempt === epoch && pending.length) {
        const data = pending.shift()!;
        pendingBytes -= new TextEncoder().encode(data).length;
        await props.port.write(props.projectId, session, data);
      }
    } catch (cause) { if (attempt === epoch) fail(cause); }
    finally { sending = false; if (alive && !failed && pending.length) void send(); }
  }
  const resize = async () => {
    if (!term || !fit || !ws.panels.terminalOpen() || !host.clientWidth) return;
    fit.fit();
    wantedSize = { cols: Math.min(500, term.cols), rows: Math.min(500, term.rows) };
    if (!session || failed || resizing) return;
    resizing = true;
    const attempt = epoch;
    try {
      while (alive && !failed && session && attempt === epoch) {
        const size: { cols: number; rows: number } = wantedSize!;
        await props.port.resize(props.projectId, session, size.cols, size.rows);
        if (size === wantedSize) break;
      }
    } catch (cause) { if (attempt === epoch) fail(cause); }
    finally { resizing = false; }
  };
  async function open() {
    if (opening || busy()) return;
    opening = true; setBusy(true); failed = false; setError(""); setStatus("Opening…");
    const attempt = ++epoch;
    clearTimeout(timer);
    try {
      if (!term) {
        term = new Xterm({ scrollback: 1000, fontSize: 12.5, convertEol: false, allowProposedApi: false, windowOptions: {} });
        fit = new FitAddon(); term.loadAddon(fit); term.open(host); style();
        term.onData(data => {
          if (!running() || failed || !session) return;
          const bytes = new TextEncoder().encode(data).length;
          if (pendingBytes + bytes > 16384) { fail(new Error("Terminal input queue is full. Unsent input was discarded.")); return; }
          pending.push(...inputChunks(data)); pendingBytes += bytes; void send();
        });
      }
      fit!.fit(); term.reset(); session = undefined; cursor = 0;
      const page = await props.port.open(props.projectId, Math.min(500, term.cols), Math.min(500, term.rows));
      await output(page, attempt);
      if (alive && attempt === epoch) { term.focus(); void poll(attempt); }
    } catch (cause) { if (attempt === epoch) fail(cause); }
    finally { opening = false; if (alive) setBusy(false); }
  }
  async function close() {
    if (!session || busy()) return;
    setBusy(true); ++epoch; clearTimeout(timer); pending = []; pendingBytes = 0; setRunning(false);
    try {
      await props.port.close(props.projectId, session);
      session = undefined; setStatus("Closed"); setError("");
    } catch (cause) { fail(cause); }
    finally { if (alive) setBusy(false); }
  }
  createEffect(() => {
    ws.theme.resolved();
    // Theme styles are applied by the frame effect before this one runs.
    queueMicrotask(() => { if (alive) style(); });
  });
  createEffect(() => {
    if (ws.panels.terminalOpen()) {
      if (!term) void open();
      else { void resize(); term.focus(); }
    }
  });
  const observer = new ResizeObserver(() => void resize());
  queueMicrotask(() => { if (alive) observer.observe(host); });
  onCleanup(() => { alive = false; ++epoch; clearTimeout(timer); observer.disconnect(); term?.dispose(); });
  return <section class="terminal ws-terminal" aria-label="Terminal">
    <div class="term-h">
      <span class="term-tab"><I n="terminal" s={12} />Project shell</span>
      <span class="mono trunc">{props.repository}</span>
      <span class="grow" /><span role="status" class="terminal-status">{status()}</span>
      <Show when={!running()}><button class="btn btn-sm" disabled={busy()} onClick={() => void open()}>Reconnect terminal</button></Show>
      <button class="ib" aria-label="Close shell" disabled={busy() || !session} onClick={() => void close()}><I n="x" s={14} /></button>
      <button class="ib" aria-label="Hide terminal" onClick={ws.toggleTerminal}><I n="minus" s={14} /></button>
    </div>
    <Show when={error()}><div class="terminal-error" role="alert">{error()}</div></Show>
    <div class="terminal-screen" ref={host} />
  </section>;
}
