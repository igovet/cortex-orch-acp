import { createSignal, For, onMount, Show } from "solid-js";
import { useWs } from "./ctx";
import { clock } from "./selectors";
import { SetH, Srow } from "./settings-kit";

/** The machine's tool search path as the service reports it (maintenance `tool_path.*`). */
type ToolPath = { source?: string; shell?: string | null; reason?: string | null; readAt?: number; detected?: string[]; extra?: string[]; rejected?: { entry: string; reason: string }[] };

const WHY: Record<string, string> = { timeout: "it did not finish within a few seconds", no_marker: "it ended without reporting its PATH", spawn_failed: "it could not be started", unsupported_shell: "this shell is not supported", home_unavailable: "the home folder is unavailable", shell_read_pending: "it is still being read" };
const IGNORED: Record<string, string> = { relative: "not an absolute folder", missing: "missing", not_a_directory: "not a folder", world_writable: "anyone can write to it", private_state: "the app's private state", invalid: "not a usable folder", limit: "over the limit" };

/**
 * Where worker commands, the verifier and diagnostics look programs up: the PATH read from the owner's own shell, then the
 * folders the owner adds. Local desktop only; a change applies from each task's next command.
 */
export function ToolSearchPath() {
  const ws = useWs();
  const [path, setPath] = createSignal<ToolPath>();
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [draft, setDraft] = createSignal("");
  const call = (params: Record<string, unknown>) => ws.store.machineAction("maintenance", params) as Promise<ToolPath>;
  const act = async (params: Record<string, unknown>) => {
    if (busy()) return false;
    setBusy(true); setError("");
    try { setPath(await call(params)); return true; }
    catch (e) { setError(e instanceof Error ? e.message : "The machine did not answer. Check again before retrying."); return false; }
    finally { setBusy(false); }
  };
  onMount(() => { void call({ action: "tool_path.read" }).then(setPath, () => setError("The tool search path is shown by the local desktop service only.")); });
  const status = () => {
    const p = path();
    if (!p) return "Reading…";
    if (p.source === "shell") return `Read from your shell (${p.shell ?? "shell"}) ${clock(p.readAt ?? null)}.`;
    if (p.source === "pending") return "Still reading your shell. The usual tool folders are used meanwhile.";
    if (p.source === "fallback") return `Your shell could not be read: ${WHY[p.reason ?? ""] ?? p.reason ?? "unknown"}. The service's PATH and the usual tool folders found ${clock(p.readAt ?? null)} are used.`;
    return "This service does not read your shell.";
  };
  const extra = () => path()?.extra ?? [];
  const save = (folders: string[]) => act({ action: "tool_path.set_extra", directories: folders });
  const add = async () => { const folder = draft().trim(); if (folder && await save([...extra(), folder])) setDraft(""); };
  return <>
    <SetH title="Tool search path" />
    <div class="surface">
      <Srow t="Detected" d={status()}><button class="btn btn-line btn-sm" disabled={busy() || !path() || path()!.source === "unavailable"} onClick={() => void act({ action: "tool_path.reread" })}>{busy() ? "Working…" : "Re-read from my shell"}</button></Srow>
      <Show when={(path()?.detected ?? []).length > 0}><div class="srow"><span class="mono cap" style={{ "white-space": "pre-wrap" }} aria-label="Detected folders">{(path()?.detected ?? []).join("\n")}</span></div></Show>
      <Show when={(path()?.rejected ?? []).length > 0}><Srow t="Ignored" d={(path()?.rejected ?? []).map(r => `${r.entry || "(empty)"}: ${IGNORED[r.reason] ?? r.reason}`).join("; ")} /></Show>
      <Srow t="Extra folders" d="Searched after the detected ones. Absolute folders only; folders inside a project, the app's private state and folders anyone can write to are refused.">
        <input class="inp mono ws-path" aria-label="Extra tool folder" placeholder="/opt/tools/bin" value={draft()} disabled={busy()} onInput={e => setDraft(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") void add(); }} />
        <button class="btn btn-line btn-sm" disabled={busy() || !draft().trim()} onClick={() => void add()}>Add folder</button>
      </Srow>
      <For each={extra()}>{folder => <Srow t={<span class="mono">{folder}</span>}><button class="btn btn-line btn-sm" aria-label={`Remove ${folder}`} disabled={busy()} onClick={() => void save(extra().filter(f => f !== folder))}>Remove</button></Srow>}</For>
    </div>
    <Show when={error()}><p class="err" role="alert">{error()}</p></Show>
    <p class="hint">Reading runs your shell's startup files once, without a terminal, and keeps only PATH. Running tasks use a change from their next command.</p>
  </>;
}
