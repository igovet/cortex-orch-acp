import { createSignal, onCleanup, Show } from "solid-js";
import { useWs } from "./ctx";
import { I } from "./kit";
import { writeClipboard } from "./message-copy";

export type ObjectId = { kind: "task" | "sprint" | "project"; id: string };

/** The parent keys this control to the current object, including pending clipboard feedback. */
export function CopyObjectId(props: { object: ObjectId }) {
  const ws = useWs();
  const [result, setResult] = createSignal<"idle" | "copied" | "manual">("idle");
  let attempt = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let field: HTMLInputElement | undefined;
  let button: HTMLButtonElement | undefined;
  const label = () => ws.t(({ task: "copyTaskId", sprint: "copySprintId", project: "copyProjectId" } as const)[props.object.kind]);
  onCleanup(() => { attempt++; clearTimeout(timer); });
  async function copy() {
    const current = ++attempt;
    clearTimeout(timer);
    setResult("idle");
    const copied = await writeClipboard(props.object.id);
    if (current !== attempt) return;
    setResult(copied ? "copied" : "manual");
    if (copied) timer = setTimeout(() => setResult("idle"), 1600);
    else { field?.focus(); field?.select(); }
  }
  return <span class="ws-object-id">
    <button ref={button} type="button" class="ib" aria-label={label()} data-tooltip={label()} onClick={() => void copy()}>
      <I n={result() === "copied" ? "check" : "copy"} s={14} />
    </button>
    <span aria-live="polite">{result() === "copied" ? ws.t("objectIdCopied") : ""}</span>
    <Show when={result() === "manual"}><label class="ws-object-id-manual" onFocusOut={() => setResult("idle")}>
      {ws.t("objectIdManual")}
      <input class="inp mono" ref={field} readonly value={props.object.id} onClick={e => e.currentTarget.select()}
        onKeyDown={e => { if (e.key === "Escape") { e.stopPropagation(); button?.focus(); } }} />
    </label></Show>
  </span>;
}
