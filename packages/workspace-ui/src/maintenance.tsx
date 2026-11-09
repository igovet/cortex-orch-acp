import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { useWs } from "./ctx";
import { Srow } from "./settings-kit";

type Status = { notes?: string; id?: string; phase?: string; message?: string; version?: string; current?: string; bytes?: number; destination?: string; rollback?: boolean; privilegePrompt?: boolean };
const terminal = (s: Status) => ["done", "failed", "cancelled", "ready", "unknown"].includes(s.phase ?? "");
const identity = () => crypto.randomUUID().replaceAll("-", "");

/** Poll only the acknowledged identity. A missing reply is never permission to repeat a mutation. */
export function useMaintenance(key: string) {
  const ws = useWs();
  const [status, setStatus] = createSignal<Status>({});
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined, closed = false, generation = 0;
  const call = (params: Record<string, unknown>) => ws.store.machineAction("maintenance", params) as Promise<Status>;
  const poll = async (id: string, run = generation) => {
    try {
      const next = await call({action:"read", id});
      if (closed || run !== generation) return;
      if (next.id !== id) throw new Error("Operation identity mismatch");
      setStatus(next); setError(""); setBusy(!terminal(next));
      if (!terminal(next)) timer = setTimeout(() => void poll(id, run), 500);
      else setBusy(false);
    } catch { if (!closed && run === generation) { setBusy(false); setError("Status is unavailable. Recheck this operation; do not start it again blindly."); } }
  };
  const read = () => { const id = status().id; if (id) { clearTimeout(timer); generation++; void poll(id); } };
  const start = async (params: Record<string, unknown>) => {
    if (busy()) return;
    clearTimeout(timer); generation++; setError(""); setBusy(true);
    const id = identity();
    // Keep the delivery identity before sending so even a lost reply can be inspected.
    setStatus({id, phase:"starting", message:"Starting…"});
    try { localStorage.setItem(key, id); } catch { /* operation identity only */ }
    try { const next = await call({action:"start", id, ...params}); setStatus(next); void poll(id); }
    catch (e) { setBusy(false); setError(e instanceof Error ? e.message : "Outcome unknown. Recheck before retrying."); }
  };
  const cancel = async () => { const id = status().id; if (id) { await call({action:"cancel", id}); read(); } };
  onMount(() => { try { const id = localStorage.getItem(key); if (id) { setStatus({id}); setBusy(true); void poll(id); } } catch { /* unavailable storage */ } });
  onCleanup(() => { closed = true; generation++; clearTimeout(timer); });
  return {status, setStatus, error, setError, busy, start, cancel, read, call};
}

export function Updates() {
  const ws = useWs(), job = useMaintenance("cw.update");
  const [channel, setChannel] = createSignal<Status>({});
  const [candidate, setCandidate] = createSignal<Status>({});
  const [checking, setChecking] = createSignal(false);
  const [consent, setConsent] = createSignal(false);
  const [source, setSource] = createSignal<string>();
  onMount(() => { try { setSource(localStorage.getItem("cw.update.source") ?? undefined); } catch { /* unavailable storage */ } });
  onMount(() => { void job.call({action:"update.status"}).then(setChannel, () => job.setError("Update status is unavailable.")); });
  const check = async () => {
    setChecking(true); job.setError(""); setConsent(false);
    try { setCandidate(await job.call({action:"update.check"})); }
    catch (e) { job.setError(e instanceof Error ? e.message : "The release server could not be reached. Retry Check."); }
    finally { setChecking(false); }
  };
  const install = (rollback = false) => {
    const from = rollback ? source() : job.status().id;
    if (!consent() || !from) return;
    if (!rollback) { setSource(from); try { localStorage.setItem("cw.update.source", from); } catch { /* unavailable storage */ } }
    setConsent(false);
    void job.start({kind:rollback ? "rollback" : "install", source:from, confirmed:true});
  };
  return <div class="set-main"><div class="set-in">
    <h1 class="set-title">Updates</h1>
    <p class="set-lead">Release packages update only after your consent and the system privilege prompt. Dev builds are separate.</p>
    <div class="surface">
      <Srow t="Channel" d={channel().message ?? "Reading installed channel…"}><span>{channel().phase === "dev" ? "Dev" : channel().phase ? "Release" : ""}</span></Srow>
      <Show when={channel().current}><Srow t="Installed version" d="Reported by the Debian package manager."><span class="mono">{channel().current}</span></Srow></Show>
      <Show when={channel().phase === "idle"}>
        <Srow t="GitHub releases" d={candidate().message ?? "Stable releases only. Downgrades are refused."}><button class="btn btn-line" disabled={checking() || job.busy()} onClick={() => void check()}>{checking() ? "Checking…" : "Check for updates"}</button></Srow>
        <Show when={candidate().phase === "available"}><Srow t={`Version ${candidate().version}`} d="Download first; verify its exact archive and published SHA-256 before installing."><button class="btn btn-line" disabled={job.busy()} onClick={() => { setSource(undefined); try { localStorage.removeItem("cw.update.source"); } catch { /* unavailable storage */ } void job.start({kind:"download"}); }}>Download and verify</button></Srow></Show>
        <Show when={channel().privilegePrompt === false}><p class="err">pkexec is unavailable. Use your system package manager; no in-app installation can start.</p></Show>
      </Show>
    </div>
    <Show when={candidate().notes}><div class="surface ws-pad" style={{"white-space":"pre-wrap"}}>{candidate().notes}</div></Show>
    <Show when={job.status().id}>
      <p role="status">{job.status().message} {job.status().bytes ? `${Math.round(job.status().bytes! / 1024 / 1024)} MiB received` : ""}</p>
      <button class="btn btn-line" onClick={job.read}>Recheck operation</button>
      <Show when={job.busy() && job.status().phase === "downloading"}><button class="btn" onClick={() => void job.cancel().catch(() => job.setError("Cancellation could not be confirmed. Recheck."))}>Cancel download</button></Show>
    </Show>
    <Show when={job.status().phase === "ready" || (source() && ["done","failed"].includes(job.status().phase ?? ""))}>
      <label class="ws-check"><input type="checkbox" checked={consent()} onChange={e => setConsent(e.currentTarget.checked)} />I consent to changing the installed release package. Active work should be stopped first.</label>
      <Show when={job.status().phase === "ready"}><button class="btn btn-primary" disabled={!consent() || !channel().privilegePrompt} onClick={() => install()}>Install with system prompt</button></Show>
      <Show when={source()}><button class="btn btn-line" disabled={!consent() || job.busy()} onClick={() => install(true)}>Roll back retained package</button><p class="hint">Rollback is available only if apt retained the exact previous archive. If none exists it is refused. A failed install is never silently rolled back.</p></Show>
    </Show>
    <Show when={job.status().phase === "unknown"}>
      <label class="ws-check"><input type="checkbox" checked={consent()} onChange={e => setConsent(e.currentTarget.checked)} />I checked the system package manager and no installation is running.</label>
      <button class="btn btn-line" disabled={!consent()} onClick={() => void job.call({action:"acknowledge",id:job.status().id,confirmed:true}).then(s => {job.setStatus(s); setConsent(false);}, () => job.setError("Acknowledgement was not confirmed. Recheck."))}>Allow work after manual check</button>
    </Show>
    <Show when={job.error()}><p class="err" role="alert">{job.error()}</p></Show>
  </div></div>;
}

export function CloneProject(props: { added: (path: string) => void }) {
  const ws = useWs(), job = useMaintenance("cw.clone");
  const [url, setUrl] = createSignal(""), [destination, setDestination] = createSignal("");
  const start = () => void job.start({kind:"clone", url:url().trim(), destination:destination().trim()});
  return <div class="ws-fields ws-pad">
    <label class="fld"><span class="lab">GitHub URL or local bare repository</span><input class="inp" aria-label="GitHub repository URL" placeholder="https://github.com/owner/repository" value={url()} disabled={job.busy()} onInput={e => setUrl(e.currentTarget.value)} /></label>
    <label class="fld"><span class="lab">New destination folder</span><input class="inp" aria-label="Clone destination" placeholder="~/projects/repository" value={destination()} disabled={job.busy()} onInput={e => setDestination(e.currentTarget.value)} /></label>
    <button class="btn btn-line" disabled={job.busy()} onClick={() => void ws.store.pickFolder().then(parent => { if (parent) setDestination(parent.replace(/\/$/, "") + "/" + (url().split("/").pop()?.replace(/\.git$/, "") || "repository")); })}>Choose parent folder…</button>
    <p class="hint">Runs on {ws.machine().name}. A local source must be an absolute bare Git repository path. Existing folders are never overwritten. Private repositories require your existing SSH access; no credential is copied or requested here.</p>
    <button class="btn btn-primary" disabled={!url().trim() || !destination().trim() || job.busy()} onClick={start}>Clone repository</button>
    <Show when={job.status().id}><p role="status">{job.status().message}</p><button class="btn btn-line" onClick={job.read}>Recheck clone</button></Show>
    <Show when={job.busy()}><button class="btn" onClick={() => void job.cancel().catch(() => job.setError("Cancellation could not be confirmed. Recheck."))}>Cancel clone</button></Show>
    <Show when={job.status().phase === "done" && job.status().destination}><button class="btn btn-primary" onClick={() => props.added(job.status().destination!)}>Add cloned project</button></Show>
    <Show when={job.error()}><p class="err" role="alert">{job.error()}</p></Show>
    <p class="hint">Closing this dialog leaves the machine operation visible when you reopen Clone from GitHub. Cancel leaves any partial folder for inspection.</p>
  </div>;
}
