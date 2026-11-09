import { createMemo, createSignal, For, Show } from "solid-js";
import { DAY, FEATURES, previewClear, type EngineId } from "@cortex/workspace-domain";
import { UnmergedDialog } from "./archive";
import { useWs } from "./ctx";
import type { WsDialog } from "./dialogs";
import { I, Modal } from "./kit";
import { ProjectPicker } from "./project-picker";
import { Seg, Stepper } from "./settings-kit";

type Of<K extends WsDialog["kind"]> = Extract<WsDialog, { kind: K }>;
const Head = (props: { title: string; onClose: () => void }) => <div class="dlg-h"><h2>{props.title}</h2><button class="ib" aria-label="Cancel" onClick={() => props.onClose()}><I n="x" /></button></div>;
const FINGERPRINT = "4F2A 91C7 D03E", OTHER_FINGERPRINT = "4F2A 91C7 77B1";

/** Pairing keeps its shape: nothing is sent, and no machine appears, until both sides show the same fingerprint. */
function AddMachine(props: { onClose: () => void }) {
  const ws = useWs();
  const [step, setStep] = createSignal(1);
  const [code, setCode] = createSignal("");
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [differs, setDiffers] = createSignal(false);
  const connect = () => {
    const fault = ws.demo?.pairFault() ?? null;
    if (!name().trim()) { setError("Give this machine a name."); return; }
    if (!/^[A-Z0-9]{4}-[A-Z0-9]{4}$/.test(code().trim())) { setError("Enter the code shown on the other machine, like 7KQ2-M9XD."); return; }
    if (fault === "expired" || fault === "connector") {
      ws.demo?.clearPairFault();
      setError(fault === "expired" ? "This code expired. Make a new one on the other machine." : "The server connector is unreachable. Nothing was sent.");
      return;
    }
    setDiffers(fault === "fingerprint");
    if (fault) ws.demo?.clearPairFault();
    setError(null); setStep(3);
  };
  const confirm = () => {
    const id = ws.newId("machine");
    ws.ok({ type: "pair_machine", id, name: name(), code: code().trim(), fingerprintsMatch: !differs(), now: ws.store.clock() }, () => { props.onClose(); });
  };
  const dot = (n: number) => <span class={`step-n${step() === n ? " on" : step() > n ? " done" : ""}`}><Show when={step() > n} fallback={n}><I n="check" s={12} /></Show></span>;
  return <>
    <Head title="Add a remote machine" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="row ws-steps-row">{dot(1)}<span class="cap muted">Get a code</span><span class="vr ws-step-line"></span>{dot(2)}<span class="cap muted">Enter it</span><span class="vr ws-step-line"></span>{dot(3)}<span class="cap muted">Compare</span></div>
      <Show when={step() === 1}><div class="ws-fields">
        <div>On the other machine, start the app and open Settings, Remote access, then turn on Allow remote access. It shows a pairing code, a QR code and a link.</div>
        <div class="note-box plain"><I n="cloud" s={14} /><span>Remote machines connect through the server connector. No port needs opening. Nothing is sent before you confirm in step 3.</span></div>
      </div></Show>
      <Show when={step() === 2}><div class="ws-fields">
        <label class="fld"><span class="lab">Pairing code or link</span><input class="inp mono" data-autofocus aria-label="Pairing code or link" placeholder="7KQ2-M9XD" value={code()} onInput={e => { setCode(e.currentTarget.value.toUpperCase()); setError(null); }} /></label>
        <label class="fld"><span class="lab">Name for this machine</span><input class="inp" aria-label="Name for this machine" placeholder="GPU box" value={name()} onInput={e => { setName(e.currentTarget.value); setError(null); }} /></label>
        <Show when={error()}><div class="err" role="alert">{error()}</div></Show>
      </div></Show>
      <Show when={step() === 3}><div class="ws-fields">
        <div>Both sides now show a fingerprint. Confirm only if they are exactly the same.</div>
        <div class="fpr"><div><span class="lab">This machine sees</span><code>{FINGERPRINT}</code></div><div><span class="lab">{name()} shows</span><code>{differs() ? OTHER_FINGERPRINT : FINGERPRINT}</code></div></div>
        <Show when={differs()} fallback={<div class="note-box plain"><I n="loader" s={14} /><span>Connecting through the server connector. If the fingerprints differ, stop here: no project data has been sent.</span></div>}>
          <div class="note-box bad" role="alert"><I n="alert" s={14} /><span>The fingerprints differ. Stopped, no project data has been sent.</span></div>
        </Show>
      </div></Show>
    </div>
    <div class="dlg-f">
      <button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button>
      <Show when={step() === 1}><button class="btn btn-primary btn-lg" data-autofocus onClick={() => setStep(2)}>I have a code</button></Show>
      <Show when={step() === 2}><button class="btn btn-primary btn-lg" onClick={connect}>Connect</button></Show>
      <Show when={step() === 3 && !differs()}><button class="btn btn-primary btn-lg" onClick={confirm}>Fingerprints match, confirm</button></Show>
    </div>
  </>;
}

const ENGINES = [{ id: "claude", label: "Claude" }, { id: "codex", label: "Codex" }, { id: "opencode", label: "OpenCode" }] as const;
export function AddConnection(props: { onClose: () => void }) {
  const ws = useWs();
  const [label, setLabel] = createSignal("");
  const [engine, setEngine] = createSignal<EngineId>("claude");
  const [auth, setAuth] = createSignal("native");
  const [key, setKey] = createSignal("");
  const [busy, setBusy] = createSignal(false), [note, setNote] = createSignal(""), [failed, setFailed] = createSignal(false);
  const native = () => engine() === "codex" ? "codex-acp" : "claude-agent-acp";
  const run = async (login: boolean) => {
    if (busy()) return;
    setBusy(true); setNote(""); setFailed(false);
    const secret = key(); setKey("");
    try {
      if (ws.store.mode === "demo") {
        if (!login) ws.ok({type:"add_connection", machineId:ws.machine().id, label:label() || engine(), engine:engine()}, props.onClose);
        else setNote("Native login is not run in the demo.");
        return;
      }
      const result = await ws.store.machineAction(login ? "native.engine_login" : "engine.connection",
        login ? {engine:native()} : auth() === "key" ? {action:"add_key", engine:native(), name:label(), key:secret} : {action:"recheck", engine:native()});
      if (login) setNote(String(result.message)); else {
        if (typeof result.connectionId !== "string" || !result.connectionId) throw new Error("Connection not confirmed");
        await ws.store.reload(); props.onClose();
      }
    } catch { setFailed(true); setNote(login ? "Native login could not open. Check the installed adapter and gnome-terminal." : "The connection was not confirmed. Refresh connections before retrying. Check login or re-enter the key; keys are never shown."); }
    finally { setBusy(false); }
  };
  return <>
    <Head title="Add a connection" onClose={props.onClose} />
    <div class="dlg-b"><div class="ws-fields">
      <div class="fld"><span class="lab">Engine</span><Seg label="Engine" value={engine()} options={ENGINES.filter(e => e.id !== "opencode")} onChange={v => {setEngine(v); setKey("");}} /></div>
      <div class="fld"><span class="lab">Authentication</span><Seg label="Authentication" value={auth()} options={[{id:"native",label:"Native account"},{id:"key",label:"API key"}]} onChange={v => {setAuth(v); setKey("");}} /></div>
      <Show when={auth() === "key"} fallback={<>
        <p class="hint">Open the bundled native CLI login in your terminal. Your existing native account stays in its original HOME. Finish there, then recheck here.</p>
        <button class="btn btn-line" disabled={busy()} onClick={() => void run(true)}>Sign in in terminal</button>
      </>}>
        <label class="fld"><span class="lab">Connection name</span><input class="inp" aria-label="Connection name" maxlength="100" value={label()} disabled={busy()} onInput={e => setLabel(e.currentTarget.value)} /></label>
        <label class="fld"><span class="lab">API key</span><input type="password" class="inp" aria-label="API key" autocomplete="off" spellcheck={false} value={key()} disabled={busy()} onInput={e => setKey(e.currentTarget.value)} /></label>
        <p class="hint">Saved in a private file only you can read, never in chat, command history or logs. The field clears on submission.</p>
      </Show>
      <Show when={note()}><p role={failed() ? "alert" : "status"}>{note()}</p></Show>
    </div></div>
    <div class="dlg-f"><button class="btn btn-line btn-lg" onClick={() => {setKey(""); props.onClose();}}>Cancel</button><button class="btn btn-primary btn-lg" disabled={busy() || (auth() === "key" && (!key().trim() || !label().trim()))} onClick={() => void run(false)}>{busy() ? "Checking…" : auth() === "key" ? "Save API-key connection" : "Recheck native login and connect"}</button></div>
  </>;
}

/** Date first, then a preview, then the delete. Never runs by itself, and unmerged work is out of its reach. */
function ClearOld(props: { onClose: () => void }) {
  const ws = useWs();
  const [days, setDays] = createSignal(7);
  const [date, setDate] = createSignal("");
  const [force, setForce] = createSignal(false);
  const cutoff = () => date() ? Date.parse(date() + "T00:00:00Z") : ws.store.now - days() * DAY;
  const preview = createMemo(() => previewClear(ws.store.state(), ws.machine().id, cutoff(), force()));
  const n = () => preview().remove.length;
  const go = () => { if (n()) ws.ok({ type: "clear_old", machineId: ws.machine().id, taskIds: preview().remove, cutoff: cutoff(), force: force(), confirmed: true, now: ws.store.clock() }, () => { props.onClose(); }); };
  return <>
    <Head title="Clear old tasks" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="row"><span>Done more than</span><Stepper label="Older than" value={days()} suffix="days" min={0} max={36500} onChange={v => {setDays(v); setDate("");}} /><span>ago</span></div>
      <label class="fld"><span class="lab">Or before date (UTC)</span><input class="inp" type="date" aria-label="Before date" max={new Date(ws.store.now).toISOString().slice(0,10)} value={date()} onInput={e => setDate(e.currentTarget.value)} /></label>
      <ul class="conseq">
        <li><I n="trash" s={14} /><span>Will be removed: {n()} task{n() === 1 ? "" : "s"}</span></li>
        <Show when={preview().guarded > 0}><li><I n="lock" s={14} /><span>Protected: {preview().guarded} with a live or unknown process</span></li></Show>
        <Show when={preview().kept > 0}><li><I n="branch" s={14} /><span>Kept: {preview().kept} with unmerged work. Those go through the Archive question</span></li></Show>
      </ul>
      <label class="ck"><input type="checkbox" disabled={ws.store.mode === "real"} checked={force()} onChange={e => setForce(e.currentTarget.checked)} /><span>Include stale protected tasks<span class="hint ws-block">Desktop always protects active and unknown executions. Stop or resolve them before deleting.</span></span></label>
    </div>
    <div class="dlg-f"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Keep them</button>
      <button class="btn btn-danger btn-lg" aria-disabled={n() ? undefined : "true"} data-why={n() ? undefined : "No old task can be cleared"} {...ws.needs("clear_old")} onClick={go}>Delete {n()} task{n() === 1 ? "" : "s"}</button></div>
  </>;
}

/** Leaving a section with a draft asks first. Nothing is lost by staying, and a save that fails keeps the owner here. */
function Unsaved() {
  const ws = useWs();
  const s = ws.settings;
  const go = () => { const to = s.pending(); s.setPending(null); if (to) ws.go(to, { force: true }); };
  return <Modal open={s.pending() !== null} onClose={() => s.setPending(null)} label="Unsaved changes">
    <div class="dlg-h"><h2>Unsaved changes</h2></div>
    <div class="dlg-b"><div>This section has changes that are not saved on the machine.</div></div>
    <div class="dlg-f">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => s.setPending(null)}>Stay</button>
      <button class="btn btn-line btn-lg" onClick={() => { s.dropCurrent(); go(); }}>Discard</button>
      <button class="btn btn-primary btn-lg" onClick={() => { void s.saveCurrent().then(saved => { if (saved) go(); }); }}>Save here</button>
    </div>
  </Modal>;
}

/** Switching with changes is a choice about those changes, so the cheap path never drops them quietly. */
function SwitchProfile(props: { d: Of<"switch-profile">; onClose: () => void }) {
  const ws = useWs();
  const go = (keep: "save" | "discard") => { const machineId = ws.machine().id; ws.ok({ type: "activate_profile", machineId, profileId: props.d.profileId, keep }, () => { ws.settings.clearDrafts(machineId); props.onClose(); }); };
  return <>
    <Head title={`Switch to ${props.d.name}?`} onClose={props.onClose} />
    <div class="dlg-b">
      <Show when={props.d.changes.length > 0}><div>This machine differs from the active profile in: {props.d.changes.join(", ")}.</div></Show>
      <Show when={props.d.drafts > 0}><div class="ws-mt6">{props.d.drafts} unsaved draft{props.d.drafts === 1 ? " is" : "s are"} dropped either way.</div></Show>
      <div class="cap muted ws-mt6">Running tasks keep the configuration they started with.</div>
    </div>
    <div class="dlg-f">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-danger btn-lg" {...ws.needs("activate_profile")} onClick={() => go("discard")}>Discard and switch</button>
      <button class="btn btn-primary btn-lg" {...ws.needs("activate_profile")} onClick={() => go("save")}>Save and switch</button>
    </div>
  </>;
}

export function MachineDialogs() {
  const ws = useWs();
  const close = () => ws.closeDialog();
  const open = (name: WsDialog["kind"]) => ws.dialog()?.kind === name;
  const of = <K extends WsDialog["kind"]>(kind: K) => { const x = ws.dialog(); return x?.kind === kind ? x as Of<K> : null; };
  return <>
    <Modal open={open("add-machine")} onClose={close} label="Add a remote machine"><Show when={open("add-machine")}><AddMachine onClose={close} /></Show></Modal>
    <ProjectPicker open={open("add-project")} onClose={close} />
    <Modal open={open("add-connection")} onClose={close} label="Add a connection"><Show when={open("add-connection")}><AddConnection onClose={close} /></Show></Modal>
    <Modal open={open("clear-old")} onClose={close} label="Clear old tasks"><Show when={open("clear-old")}><ClearOld onClose={close} /></Show></Modal>
    <Modal open={open("unmerged")} onClose={close} label="Unmerged work"><Show when={of("unmerged")} keyed>{d => <UnmergedDialog taskId={d.taskId} onClose={close} />}</Show></Modal>
    <Modal open={open("switch-profile")} onClose={close} label="Switch profile"><Show when={of("switch-profile")} keyed>{d => <SwitchProfile d={d} onClose={close} />}</Show></Modal>
    <Unsaved />
  </>;
}
