import { createSignal, onCleanup, Show } from "solid-js";
import { I } from "./kit";

/** Text handed to the clipboard as it is, or false when the clipboard refused it or is not there at all. */
export async function writeClipboard(text: string): Promise<boolean> {
  try {
    const clipboard = globalThis.navigator?.clipboard;
    if (!clipboard?.writeText) return false;
    await clipboard.writeText(text);
    return true;
  } catch { return false; }
}

const SHOWN_MS = 1600;

/** One press of a copy control: "Copied" only after the clipboard took the text, then quiet again after a short while. */
function createCopy(text: () => string | null | undefined) {
  const [state, setState] = createSignal<"idle" | "copied" | "failed">("idle");
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;
  onCleanup(() => { generation++; clearTimeout(timer); });
  const copy = async () => {
    const run = ++generation;
    clearTimeout(timer);
    setState("idle");
    const ok = await writeClipboard(text() ?? "");
    if (run !== generation) return;
    setState(ok ? "copied" : "failed");
    timer = setTimeout(() => { if (run === generation) setState("idle"); }, SHOWN_MS);
  };
  return { state, copy };
}

/** Announced politely, not a status role: the timeline's one status line stays what the task is doing. */
const CopyNote = (props: { state: "idle" | "copied" | "failed" }) => <span class="ws-copy-note" aria-live="polite">
  <Show when={props.state === "copied"}><span class="st-ok">Copied</span></Show>
  <Show when={props.state === "failed"}><span class="st-bad">Not copied</span></Show>
</span>;

/**
 * Copies a message's Markdown source exactly as it was recorded, never what the screen made of it: a view that clips or
 * folds the message still copies the whole of it. "Copied" is said only after the clipboard took the text.
 */
export function CopySource(props: { text: string | null | undefined; label?: string }) {
  const { state, copy } = createCopy(() => props.text);
  const label = () => props.label ?? "Copy message as Markdown";
  return <span class="ws-copy-src">
    <button type="button" class="ws-copy-btn" aria-label={label()} onClick={() => void copy()}>
      <I n={state() === "copied" ? "check" : "copy"} s={12} /><span>Copy</span>
    </button>
    <CopyNote state={state()} />
  </span>;
}

/** A code block's own copy control: it copies the block's lines as written, without the fences around them. */
export function CopyCode(props: { text: string; lang?: string }) {
  const { state, copy } = createCopy(() => props.text);
  return <span class="ws-code-tools">
    <CopyNote state={state()} />
    <button type="button" class="ws-code-copy" aria-label={props.lang ? `Copy ${props.lang} code` : "Copy code"} onClick={() => void copy()}>
      <I n={state() === "copied" ? "check" : "copy"} s={12} /><span>Copy</span>
    </button>
  </span>;
}

/** The row under a message that holds its copy control, on the side the message sits. */
export function MessageTools(props: { text: string | null | undefined; end?: boolean; label?: string }) {
  return <div class={`ws-msg-tools${props.end ? " end" : ""}`}><CopySource text={props.text} label={props.label} /></div>;
}
