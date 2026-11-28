import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch, type JSX } from "solid-js";
import { render } from "solid-js/web";
import type { PhoneFrame, PhoneWorkspace } from "./ctx";
import { I } from "./kit";
import { machineLine, machineRefusal, UPDATE_NEEDED } from "./phone-words";
import type { NotificationClicks } from "./notification-target";
import { workspaceCopy } from "./copy";
import { PAIR_LINK_PREFIX, type PhoneMachine, notificationWords, type PhoneNotificationState, type PhoneShell, type PhoneState, type PhoneTap } from "./phone-shell";
import { LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { parseRoute } from "./router";
import { createRealPort } from "./port/real";
import { discardPairingDrafts } from "./local-drafts";
import { relayProblem } from "./remote-access";
import { createTheme, persisted, type Theme, type ThemePref } from "./theme";
import { dropHover } from "./touch";
import { Workspace } from "./workspace";
import "./phone.css";

/** Plain words for every way pairing can end without a machine. */
export const PAIR_WORDS: Record<string, string> = {
  invalid_link: `This is not a pairing link. Copy the whole link from the computer; it starts with ${PAIR_LINK_PREFIX}`,
  unsupported_pairing_link_version: "This pairing link was made by an incompatible version. Update the app or the computer so both match.",
  remote_incompatible_version: UPDATE_NEEDED,
  unreachable: "The computer could not be reached. Check that it is on, that the desktop app is running, and that both are online.",
  expired: "This pairing link has expired. Make a new one on the computer, in Settings, Remote access.",
  refused: "The computer refused this pairing link. Make a new one on the computer and try again.",
  denied: "The computer denied this phone. Nothing was paired.",
  camera_denied: "Camera access is off for this app. Allow it in the phone's settings, or paste the pairing link instead.",
  camera_unavailable: "No camera is available. Paste the pairing link instead.",
};
const failure = (error: unknown) => error instanceof Error ? error.message : String(error);
const wordsFor = (code: string) => PAIR_WORDS[code] ?? "Pairing did not finish. Nothing was paired.";

type Flow = { kind: "start" } | { kind: "scan" } | { kind: "camera"; code: string } | { kind: "paste"; error?: string }
  | { kind: "opening" } | { kind: "check"; machineName: string; fingerprint: string } | { kind: "failed"; code: string };
type Page = { kind: "settings" } | { kind: "machine"; id: string } | { kind: "appearance" } | { kind: "lock" } | { kind: "relay" } | { kind: "about" };

export type PhoneAppProps = {
  shell: PhoneShell;
  /** Receives the app's Back once it exists. Back answers false when nothing was left to close or go back to: the app may close. */
  onBack?: (back: () => boolean) => void;
  /** The logo shown while the first machine is read. */
  logo?: JSX.Element;
};

function Bar(props: { title: string; icon?: "arrow-left" | "x"; label?: string; onBack?: () => void; end?: JSX.Element }) {
  return <div class="ph-bar">
    <Show when={props.onBack} fallback={<span class="ib" aria-hidden="true"></span>}><button class="ib" aria-label={props.label ?? "Back"} onClick={() => props.onBack?.()}><I n={props.icon ?? "arrow-left"} /></button></Show>
    <span class="ph-bar-t trunc">{props.title}</span>
    {props.end}
  </div>;
}
const Screen = (props: { label: string; children: JSX.Element }) => <section class="ph-screen" role="region" aria-label={props.label}>{props.children}</section>;

/**
 * The Android phone: no machine of its own. It pairs with computers running the desktop app, keeps one of them active, and
 * shows that machine through the same interface as the desktop. Switching machines tears the whole workspace down and
 * starts the next one from its Home.
 */
export function PhoneApp(props: PhoneAppProps) {
  const shell = props.shell;
  const theme = createTheme();
  // No pointer rests over anything on a touch screen: hover would only leave the last touched row lit.
  onCleanup(dropHover());
  const [state, setState] = createSignal<PhoneState | null>(null);
  const [flow, setFlow] = createSignal<Flow[]>([]);
  const [pages, setPages] = createSignal<Page[]>([]);
  const [sheet, setSheet] = createSignal(false);
  const [readIssue, setReadIssue] = createSignal<string | null>(null);
  let workspace: PhoneWorkspace | null = null;
  let scanTurn = 0, pairTurn = 0;

  const load = () => shell.state().then(next => { setState(next); setReadIssue(null); }, error => setReadIssue(failure(error)));
  void load();
  onCleanup(shell.subscribe(next => setState(next)));

  const machines = () => state()?.machines ?? [];
  const activeId = createMemo(() => { const s = state(); return s?.active && s.machines.some(m => m.id === s.active) ? s.active : null; });
  const machineOf = (id: string) => machines().find(m => m.id === id);
  const activePairing = createMemo(() => {
    const id = activeId(), machine = id ? machineOf(id) : undefined;
    return machine ? JSON.stringify([machine.id, machine.pairedAt ?? null]) : null;
  });
  const topFlow = () => flow().at(-1);
  const topPage = () => pages().at(-1);
  const push = (step: Flow) => setFlow(list => [...list, step]);
  const replace = (step: Flow) => setFlow(list => [...list.slice(0, -1), step]);
  const openPage = (page: Page) => setPages(list => [...list, page]);
  const closePage = () => setPages(list => list.slice(0, -1));
  const doneAll = () => { setFlow([]); setPages([]); setSheet(false); };

  // The phone's own notifications: on by default once a machine is paired, turned on or off in Settings.
  const notifier = shell.notifications;
  const [notifyOn, setNotifyOn] = persisted<boolean>("cw.ws.notifications", true, v => typeof v === "boolean");
  const [notifyState, setNotifyState] = createSignal<PhoneNotificationState | null>(null);
  const [notifyIssue, setNotifyIssue] = createSignal<string | null>(null);
  const notifyWanted = createMemo(() => notifyOn() && machines().length > 0);
  const applyNotify = (on: boolean) => notifier?.set(on, notificationWords()).then(next => { setNotifyState(next); setNotifyIssue(null); },
    error => setNotifyIssue(`Not changed: ${failure(error)}`));
  // Applied once the machines are known, and again only when the wish changes (never on a connection change).
  const known = createMemo(() => state() !== null);
  createEffect(() => { if (notifier && known()) void applyNotify(notifyWanted()); });
  // A tap opens its machine (switching to it when another is open) and then the place, through the workspace's own notification path.
  let tapped: PhoneTap | null = null;
  const tapListeners = new Map<string, (target: unknown) => void>();
  const deliverTap = () => {
    const tap = tapped, listener = tap && tapListeners.get(tap.machineId);
    if (tap && listener) { tapped = null; listener(tap.target); }
  };
  onCleanup(notifier?.taps(tap => {
    tapped = tap; doneAll();
    if (tap.machineId !== activeId()) void shell.select(tap.machineId); else deliverTap();
  }) ?? (() => undefined));
  const clicksFor = (id: string): NotificationClicks => listener => {
    tapListeners.set(id, listener);
    deliverTap();
    return () => { if (tapListeners.get(id) === listener) tapListeners.delete(id); };
  };
  const notifySettings = { on: notifyOn, state: notifyState, issue: notifyIssue, set: (on: boolean) => setNotifyOn(on),
    battery: () => notifier?.battery().catch(error => setNotifyIssue(failure(error))) };

  // A page about a machine that is gone (forgotten here or elsewhere) closes.
  createEffect(() => { const page = topPage(); if (page?.kind === "machine" && state() && !machineOf(page.id)) closePage(); });

  const [draftName, setDraftName] = createSignal("");
  const begin = (link: string) => {
    const text = link.trim();
    if (!text.startsWith(PAIR_LINK_PREFIX)) { replace({ kind: "paste", error: wordsFor("invalid_link") }); return; }
    replace({ kind: "opening" });
    const turn = ++pairTurn;
    shell.pairBegin(text).then(offer => {
      if (turn !== pairTurn) return;
      setDraftName(offer.machineName);
      replace({ kind: "check", ...offer });
      return shell.pairFinish(offer.machineName).then(async ({ machineId }) => {
        if (turn !== pairTurn) return;
        const name = draftName().trim();
        if (name && name !== offer.machineName) await shell.rename(machineId, name).catch(() => undefined);
        await shell.select(machineId);
        doneAll();
      });
    }).catch(error => {
      if (turn !== pairTurn) return;
      const code = failure(error);
      if (code === "cancelled") setFlow(list => list.slice(0, -1));
      else replace({ kind: "failed", code });
    });
  };
  const scan = (replacing = false) => {
    if (replacing) replace({ kind: "scan" }); else push({ kind: "scan" });
    const turn = ++scanTurn;
    shell.scan().then(text => {
      if (turn !== scanTurn || topFlow()?.kind !== "scan") return;
      if (text === null) setFlow(list => list.slice(0, -1));
      else begin(text);
    }, error => {
      if (turn !== scanTurn || topFlow()?.kind !== "scan") return;
      const code = failure(error);
      replace({ kind: "camera", code: code === "camera_denied" ? code : "camera_unavailable" });
    });
  };
  const paste = (replacing = false) => { scanTurn++; if (replacing) replace({ kind: "paste" }); else push({ kind: "paste" }); };
  const cancelPairing = () => { pairTurn++; void shell.pairCancel().catch(() => undefined); setFlow(list => list.slice(0, -1)); };

  /** Android Back: the sheet, then the pairing step, then the phone's own page, then the workspace. */
  const back = (): boolean => {
    if (sheet()) { setSheet(false); return true; }
    const step = topFlow();
    if (step) {
      if (step.kind === "check" || step.kind === "opening") cancelPairing();
      else { scanTurn++; setFlow(list => list.slice(0, -1)); }
      return true;
    }
    if (pages().length) { closePage(); return true; }
    return workspace?.back() ?? false;
  };
  props.onBack?.(back);

  const select = (id: string) => { setSheet(false); if (id !== activeId()) void shell.select(id); };
  const frameFor = (id: string): PhoneFrame => ({
    kind: "phone",
    machine: () => { const m = machineOf(id); return { name: m?.name ?? "", state: m?.state ?? "offline", path: m?.path ?? null, lastConnectedAt: m?.lastConnectedAt ?? null, pairedAt: m?.pairedAt }; },
    openMachines: () => setSheet(true),
    openSettings: () => openPage({ kind: "settings" }),
    retry: () => void shell.retry(id).catch(() => undefined),
    attach: next => { workspace = next; },
  });

  return <div class="ph-app">
    <Show when={state()} fallback={<Show when={readIssue()} fallback={props.logo}>{why => <Screen label="Not loaded"><div class="ph-body center"><h1>The app could not start</h1><p class="muted">{why()}</p><button class="btn btn-primary ph-wide" onClick={() => void load()}>Try again</button></div></Screen>}</Show>}>
      <Show when={activePairing()} keyed fallback={<Show when={machines().length > 0} fallback={<Connect onScan={() => scan()} onPaste={() => paste()} />}>
        <Screen label="Machines"><Bar title="Machines" /><div class="ph-body"><MachineList machines={machines()} active={null} onPick={select} /></div>
          <div class="ph-foot"><button class="btn btn-line" onClick={() => push({ kind: "start" })}><I n="plus" s={14} />Add machine</button></div></Screen>
      </Show>}>{pairing => {
        const [id] = JSON.parse(pairing) as [string, number | null];
        return <MachineWorkspace id={id} shell={shell} frame={frameFor(id)} theme={theme} logo={props.logo} clicks={clicksFor(id)} />;
      }}</Show>
      <Show when={pages().length > 0}><Switch>
        <Match when={topPage()?.kind === "settings"}><PhoneSettings shell={shell} theme={theme} machines={machines()} active={activeId()} notify={notifySettings} onBack={closePage} onOpen={openPage} onAdd={() => push({ kind: "start" })} /></Match>
        <Match when={(() => { const p = topPage(); return p?.kind === "machine" ? machineOf(p.id) : undefined; })()}>{m => <MachinePage shell={shell} machine={m()} active={m().id === activeId()} onBack={closePage}
          onSettings={() => { setPages([]); workspace?.openMachineSettings(); }} onSwitch={() => { doneAll(); void shell.select(m().id); }} />}</Match>
        <Match when={topPage()?.kind === "appearance"}><AppearancePage theme={theme} onBack={closePage} /></Match>
        <Match when={topPage()?.kind === "lock"}><LockPage shell={shell} onBack={closePage} /></Match>
        <Match when={topPage()?.kind === "relay"}><RelayPage shell={shell} onBack={closePage} /></Match>
        <Match when={topPage()?.kind === "about"}><AboutPage shell={shell} onBack={closePage} /></Match>
      </Switch></Show>
      <Show when={topFlow()}>{step => <Switch>
        <Match when={step().kind === "start"}><Connect onBack={() => setFlow(list => list.slice(0, -1))} onScan={() => scan(true)} onPaste={() => paste(true)} /></Match>
        <Match when={step().kind === "scan"}><Screen label="Scan QR code"><Bar title="Scan QR code" onBack={() => { scanTurn++; setFlow(list => list.slice(0, -1)); }} />
          <div class="ph-body center"><p class="muted">Point the camera at the code on the computer.</p></div>
          <div class="ph-foot"><div class="ph-note"><I n="lock" s={14} /><span>The link works once and expires in a few minutes. Nothing is sent before both sides confirm.</span></div>
            <button class="btn btn-line" onClick={() => paste(true)}>Paste pairing link instead</button></div></Screen></Match>
        <Match when={step().kind === "camera" && (step() as Extract<Flow, { kind: "camera" }>)}>{c => <Screen label="Scan QR code"><Bar title="Scan QR code" onBack={() => setFlow(list => list.slice(0, -1))} />
          <div class="ph-body center"><p role="alert">{wordsFor(c().code)}</p></div>
          <div class="ph-foot"><button class="btn btn-primary" onClick={() => paste(true)}>Paste pairing link</button>
            <Show when={c().code === "camera_denied"}><button class="btn btn-line" onClick={() => scan(true)}>Try the camera again</button></Show></div></Screen>}</Match>
        <Match when={step().kind === "paste" && (step() as Extract<Flow, { kind: "paste" }>)}>{p => <PastePage error={p().error} onBack={() => setFlow(list => list.slice(0, -1))} onGo={begin} />}</Match>
        <Match when={step().kind === "opening"}><Screen label="Add machine"><Bar title="Add machine" icon="x" label="Cancel" onBack={cancelPairing} />
          <div class="ph-body center"><div class="ph-note" role="status"><I n="loader" s={14} class="spin" /><span>Reaching the computer…</span></div></div></Screen></Match>
        <Match when={step().kind === "check" && (step() as Extract<Flow, { kind: "check" }>)}>{c => <Screen label="Add machine"><Bar title="Add machine" icon="x" label="Cancel" onBack={cancelPairing} />
          <div class="ph-body">
            <div class="ph-row"><I n="laptop" /><span class="ph-row-t"><span class="w500 trunc">{c().machineName}</span></span></div>
            <p>Both sides show a fingerprint. Continue only if they are exactly the same.</p>
            <div class="ph-fpr"><div><span class="cap muted">This phone sees</span><code>{c().fingerprint}</code></div><div><span class="cap muted">On the computer</span><span class="cap">The same value shows next to Allow.</span></div></div>
            <label class="ph-field"><span class="cap muted">Name for this machine</span><input class="inp" aria-label="Name for this machine" value={draftName()} onInput={e => setDraftName(e.currentTarget.value)} /></label>
            <div class="ph-note" role="status"><I n="loader" s={14} class="spin" /><span>Waiting for you to press Allow on the computer. No project data has been sent.</span></div>
          </div>
          <div class="ph-foot"><button class="btn btn-line" onClick={cancelPairing}>Cancel</button></div></Screen>}</Match>
        <Match when={step().kind === "failed" && (step() as Extract<Flow, { kind: "failed" }>)}>{f => <Screen label="Add machine"><Bar title="Add machine" onBack={() => setFlow(list => list.slice(0, -1))} />
          <div class="ph-body center"><p role="alert">{wordsFor(f().code)}</p></div>
          <div class="ph-foot"><button class="btn btn-primary" onClick={() => scan(true)}><I n="expand" s={14} />Scan QR code</button><button class="btn btn-line" onClick={() => paste(true)}>Paste pairing link</button></div></Screen>}</Match>
      </Switch>}</Show>
      <Show when={sheet()}>
        <div class="ph-sheet-layer">
          <button class="ws-scrim" aria-label="Close machines" onClick={() => setSheet(false)}></button>
          <div class="ph-sheet" role="dialog" aria-label="Machines">
            <div class="ph-h" style={{ padding: "0 8px 0 16px", height: "auto", "min-height": "44px" }}><span>Machines</span><button class="btn" onClick={() => { setSheet(false); openPage({ kind: "settings" }); }}>Manage</button></div>
            <MachineList machines={machines()} active={activeId()} onPick={select} />
            <div class="ph-foot"><button class="btn btn-line" onClick={() => { setSheet(false); push({ kind: "start" }); }}><I n="plus" s={14} />Add machine</button></div>
          </div>
        </div>
      </Show>
    </Show>
  </div>;
}

/** The workspace of one machine. Created whole for that machine and disposed whole when another becomes active. */
function pairingDraftNamespace(machine: { id: string; pairedAt?: number }) {
  return Number.isSafeInteger(machine.pairedAt) && machine.pairedAt! > 0
    ? ["phone", machine.id, String(machine.pairedAt)] : undefined;
}

function MachineWorkspace(props: { id: string; shell: PhoneShell; frame: PhoneFrame; theme: Theme; logo?: JSX.Element; clicks?: NotificationClicks }) {
  const namespace = pairingDraftNamespace({ id: props.id, pairedAt: props.frame.machine().pairedAt });
  // Every peer uses /m/local. Only this history entry’s pairing can identify which peer opened the URL.
  let restore = false;
  if (namespace && history.state?.phonePairing === JSON.stringify(namespace)) {
    try {
      const route = parseRoute(location.pathname, location.search);
      restore = "machine" in route && route.machine === LOCAL_MACHINE_ID;
    } catch { /* A malformed URL opens Home. */ }
  }
  try { localStorage.removeItem("cw.ws.tabs"); } catch { /* no storage */ }
  if (!restore) history.replaceState(null, "", "/");
  const port = createRealPort({ transport: props.shell.transport(props.id), registry: true, actorId: "phone" });
  return <Workspace port={port} shell={props.frame} theme={props.theme} logo={props.logo} voice={props.shell.voice} notificationClicks={props.clicks}
    draftNamespace={namespace} />;
}

function Connect(props: { onScan: () => void; onPaste: () => void; onBack?: () => void }) {
  return <Screen label="Connect to your machine">
    <Show when={props.onBack}><Bar title="Add machine" onBack={props.onBack} /></Show>
    <div class="ph-body center">
      <h1>Connect to your machine</h1>
      <p class="muted">This phone works with projects on a computer running the desktop app. Nothing runs on the phone itself.</p>
      <ol class="ph-steps">
        <li><span class="n">1</span><span>On the computer, open Settings, then Remote access.</span></li>
        <li><span class="n">2</span><span>Turn on Allow remote access. It shows a QR code.</span></li>
        <li><span class="n">3</span><span>Scan it here and confirm on the computer.</span></li>
      </ol>
    </div>
    <div class="ph-foot">
      <button class="btn btn-primary" onClick={() => props.onScan()}><I n="expand" s={14} />Scan QR code</button>
      <button class="btn btn-line" onClick={() => props.onPaste()}>Paste pairing link</button>
    </div>
  </Screen>;
}

function PastePage(props: { error?: string; onBack: () => void; onGo: (link: string) => void }) {
  const [text, setText] = createSignal("");
  let field: HTMLInputElement | undefined;
  onMount(() => field?.focus());
  const go = () => { if (text().trim()) props.onGo(text()); };
  return <Screen label="Paste pairing link"><Bar title="Paste pairing link" onBack={props.onBack} />
    <div class="ph-body">
      <label class="ph-field"><span class="cap muted">Pairing link</span><input ref={field} class="inp mono" aria-label="Pairing link" placeholder={`${PAIR_LINK_PREFIX}…`} value={text()} onInput={e => setText(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") go(); }} /></label>
      <Show when={props.error}>{why => <p class="ph-err" role="alert">{why()}</p>}</Show>
      <p class="cap muted">On the computer, press Copy link under the QR code in Settings, Remote access. The link works once.</p>
    </div>
    <div class="ph-foot"><button class="btn btn-primary" aria-disabled={text().trim() ? undefined : "true"} data-why={text().trim() ? undefined : "Paste the link first"} onClick={go}>Continue</button></div>
  </Screen>;
}

function MachineList(props: { machines: PhoneMachine[]; active: string | null; onPick: (id: string) => void }) {
  return <div class="ph-group" role="list"><For each={props.machines}>{m => <button class="ph-row" role="listitem" aria-current={m.id === props.active ? "true" : undefined} onClick={() => props.onPick(m.id)}>
    <I n="laptop" /><span class="ph-row-t"><span class="w500 trunc">{m.name}</span><span class={`cap trunc ${machineLine(m).tone}`}>{machineLine(m).text}</span><Show when={machineRefusal(m.state)}>{why => <span class="cap muted">{why()}</span>}</Show></span>
    <Show when={m.id === props.active}><I n="check" /></Show>
  </button>}</For></div>;
}

const THEME_WORDS: Record<ThemePref, string> = { system: "System", light: "Light", dark: "Dark" };
type NotifySettings = { on: () => boolean; state: () => PhoneNotificationState | null; issue: () => string | null; set: (on: boolean) => void; battery: () => void };

function PhoneSettings(props: { shell: PhoneShell; theme: Theme; machines: PhoneMachine[]; active: string | null; notify?: NotifySettings; onBack: () => void; onOpen: (page: Page) => void; onAdd: () => void }) {
  const [lock, setLock] = createSignal<{ enabled: boolean; available: boolean } | null>(null);
  const [relay, setRelay] = createSignal<{ url: string | null; isDefault: boolean } | null>(null);
  void props.shell.appLock?.read().then(setLock, () => setLock(null));
  void props.shell.relay?.read().then(setRelay, () => setRelay(null));
  return <Screen label="Settings"><Bar title="Settings" onBack={props.onBack} />
    <div class="ph-body">
      <div class="ph-h"><span>Machines</span><Show when={props.machines.length > 0}><span>{props.machines.length}</span></Show></div>
      <div class="ph-group"><For each={props.machines}>{m => <button class="ph-row" onClick={() => props.onOpen({ kind: "machine", id: m.id })}>
        <I n="laptop" /><span class="ph-row-t"><span class="w500 trunc">{m.name}</span><span class={`cap trunc ${machineLine(m).tone}`}>{machineLine(m).text}</span><Show when={machineRefusal(m.state)}>{why => <span class="cap muted">{why()}</span>}</Show></span><I n="chevron-right" s={14} class="faint" />
      </button>}</For></div>
      <button class="btn btn-line ph-wide" onClick={() => props.onAdd()}><I n="plus" s={14} />Add machine</button>
      <div class="ph-h"><span>This phone</span></div>
      <div class="ph-group">
        <button class="ph-row" onClick={() => props.onOpen({ kind: "appearance" })}><span class="ph-row-t">Appearance</span><span class="cap muted ph-row-v">{THEME_WORDS[props.theme.pref()]}</span><I n="chevron-right" s={14} class="faint" /></button>
        <Show when={lock()?.available}><button class="ph-row" onClick={() => props.onOpen({ kind: "lock" })}><span class="ph-row-t">App lock</span><span class="cap muted ph-row-v">{lock()!.enabled ? "Fingerprint" : "Off"}</span><I n="chevron-right" s={14} class="faint" /></button></Show>
        <Show when={props.shell.relay}><button class="ph-row" onClick={() => props.onOpen({ kind: "relay" })}><span class="ph-row-t">Relay server</span><Show when={relay()}>{r => <span class="cap muted ph-row-v">{r().isDefault ? "Default" : "Custom"}</span>}</Show><I n="chevron-right" s={14} class="faint" /></button></Show>
        <Show when={props.shell.notifications && props.notify}>{n => <label class="ph-row"><span class="ph-row-t"><span>{workspaceCopy("phoneNotify")}</span><span class="cap muted">{workspaceCopy("phoneNotifyNote")}</span></span>
          <span class="sw"><input type="checkbox" aria-label={workspaceCopy("phoneNotify")} checked={n().on()} onChange={e => n().set(e.currentTarget.checked)} /><i></i></span></label>}</Show>
        <button class="ph-row" onClick={() => props.onOpen({ kind: "about" })}><span class="ph-row-t">About</span><span class="cap muted ph-row-v tnum">{props.shell.version}</span><I n="chevron-right" s={14} class="faint" /></button>
      </div>
      <Show when={props.shell.notifications && props.notify}>{n => <>
        <Show when={n().on() && n().state()?.enabled && n().state()?.permission === "denied"}><p class="ph-note" role="status">{workspaceCopy("phoneNotifyBlocked")}</p></Show>
        <Show when={n().on() && n().state()?.enabled && n().state()?.batteryExempt === false}><div class="ph-note" role="status"><span>{workspaceCopy("phoneNotifyBattery")}</span></div>
          <button class="btn btn-line ph-wide" onClick={() => n().battery()}>{workspaceCopy("phoneNotifyBatteryButton")}</button></Show>
        <Show when={n().issue()}>{why => <p class="ph-err" role="alert">{why()}</p>}</Show>
      </>}</Show>
    </div>
  </Screen>;
}

function MachinePage(props: { shell: PhoneShell; machine: PhoneMachine; active: boolean; onBack: () => void; onSettings: () => void; onSwitch: () => void }) {
  const [editing, setEditing] = createSignal<string | null>(null);
  const [asking, setAsking] = createSignal(false);
  const [issue, setIssue] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const save = () => {
    const name = (editing() ?? "").trim();
    if (!name) { setIssue("Enter a name."); return; }
    if (name === props.machine.name) { setEditing(null); return; }
    setSaving(true);
    props.shell.rename(props.machine.id, name).then(() => { setEditing(null); setIssue(null); }, error => setIssue(`Not saved: ${failure(error)}`)).finally(() => setSaving(false));
  };
  const forget = () => {
    const namespace = pairingDraftNamespace(props.machine);
    return props.shell.forget(props.machine.id).then(() => {
      if (namespace) discardPairingDrafts(namespace);
      props.onBack();
    }, error => { setAsking(false); setIssue(`Not forgotten: ${failure(error)}`); });
  };
  const line = () => machineLine(props.machine);
  return <Screen label={props.machine.name}><Bar title={props.machine.name} onBack={props.onBack} />
    <div class="ph-body">
      <div class="ph-group">
        <div class="ph-row"><span class="ph-row-t"><span class="cap muted">Name</span>
          <Show when={editing() !== null} fallback={<button class="w500 trunc" style={{ "text-align": "left" }} aria-label={`Name: ${props.machine.name}. Press to edit`} onClick={() => { setIssue(null); setEditing(props.machine.name); }}>{props.machine.name}</button>}>
            <span class="ph-edit"><input class="inp" aria-label="Machine name" value={editing() ?? ""} onInput={e => setEditing(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") save(); if (e.key === "Escape") setEditing(null); }} />
              <button class="btn btn-primary" aria-disabled={saving() ? "true" : undefined} data-why={saving() ? "Saving" : undefined} onClick={save}>Save</button><button class="btn" onClick={() => setEditing(null)}>Cancel</button></span>
          </Show></span></div>
        <div class="ph-row"><span class="ph-row-t"><span class="cap muted">Connection</span><span class={line().tone}>{line().text}</span><Show when={machineRefusal(props.machine.state)}>{why => <span class="muted">{why()}</span>}</Show></span></div>
      </div>
      <Show when={issue()}>{why => <p class="ph-err" role="alert">{why()}</p>}</Show>
      <Show when={props.active} fallback={<button class="btn btn-primary ph-wide" onClick={() => props.onSwitch()}>Switch to this machine</button>}>
        <button class="btn btn-line ph-wide" onClick={() => props.onSettings()}><I n="settings" s={14} />Machine settings</button>
      </Show>
      <Show when={asking()} fallback={<button class="btn btn-line ph-wide" onClick={() => setAsking(true)}>Forget this machine…</button>}>
        <div class="ph-note" role="alertdialog" aria-label={`Forget ${props.machine.name}?`}><span><span class="w500">Forget {props.machine.name}?</span> This phone disconnects from it and has to be paired again. Nothing on the computer is deleted.</span></div>
        <div class="ph-foot" style={{ padding: "0" }}><button class="btn btn-primary" onClick={() => void forget()}>Forget</button><button class="btn btn-line" onClick={() => setAsking(false)}>Cancel</button></div>
      </Show>
    </div>
  </Screen>;
}

function AppearancePage(props: { theme: Theme; onBack: () => void }) {
  return <Screen label="Appearance"><Bar title="Appearance" onBack={props.onBack} />
    <div class="ph-body">
      <p class="cap muted">System follows the phone's setting, and it is the default.</p>
      <div class="ph-group" role="radiogroup" aria-label="Theme"><For each={(["system", "light", "dark"] as const)}>{pref =>
        <button class="ph-row" role="radio" aria-checked={props.theme.pref() === pref ? "true" : "false"} onClick={() => props.theme.setPref(pref)}><span class="ph-row-t">{THEME_WORDS[pref]}</span><Show when={props.theme.pref() === pref}><I n="check" /></Show></button>
      }</For></div>
    </div>
  </Screen>;
}

function LockPage(props: { shell: PhoneShell; onBack: () => void }) {
  const [lock, setLock] = createSignal<{ enabled: boolean; available: boolean } | null>(null);
  const [issue, setIssue] = createSignal<string | null>(null);
  void props.shell.appLock?.read().then(setLock, error => setIssue(failure(error)));
  const turn = (on: boolean) => props.shell.appLock?.write(on).then(() => props.shell.appLock!.read().then(setLock), error => setIssue(`Not changed: ${failure(error)}`));
  return <Screen label="App lock"><Bar title="App lock" onBack={props.onBack} />
    <div class="ph-body">
      <Show when={lock()?.available} fallback={<p class="muted">{issue() ?? "App lock is not available on this phone."}</p>}>
        <div class="ph-group"><label class="ph-row"><span class="ph-row-t"><span>Unlock with fingerprint</span><span class="cap muted">Asks for your fingerprint each time the app opens. Off by default.</span></span>
          <span class="sw"><input type="checkbox" aria-label="Unlock with fingerprint" checked={lock()!.enabled} onChange={e => { const on = e.currentTarget.checked; e.currentTarget.checked = lock()!.enabled; void turn(on); }} /><i></i></span></label></div>
        <Show when={issue()}>{why => <p class="ph-err" role="alert">{why()}</p>}</Show>
      </Show>
    </div>
  </Screen>;
}

function RelayPage(props: { shell: PhoneShell; onBack: () => void }) {
  const [relay, setRelay] = createSignal<{ url: string | null; isDefault: boolean } | null>(null);
  const [text, setText] = createSignal("");
  const [issue, setIssue] = createSignal<string | null>(null);
  const read = () => props.shell.relay!.read().then(r => { setRelay(r); setText(r.isDefault ? "" : r.url ?? ""); }, error => setIssue(failure(error)));
  void read();
  const write = (url: string | null) => props.shell.relay!.write(url).then(() => { setIssue(null); return read(); }, error => setIssue(`Not saved: ${failure(error)}`));
  const save = () => { const why = relayProblem(text()); setIssue(why); if (!why) void write(text().trim()); };
  return <Screen label="Relay server"><Bar title="Relay server" onBack={props.onBack} />
    <div class="ph-body">
      <p class="cap muted">The relay forwards encrypted traffic when the phone cannot reach a computer directly. It cannot read it.</p>
      <Show when={relay()}>{r => <div class="ph-group"><div class="ph-row"><span class="ph-row-t"><span class="cap muted">In use</span><span class="mono trunc">{r().isDefault ? "Default" : r().url}</span><Show when={r().isDefault && r().url}><span class="cap muted mono trunc">{r().url}</span></Show></span></div></div>}</Show>
      <label class="ph-field"><span class="cap muted">Your own relay server</span><input class="inp mono" aria-label="Relay server address" placeholder="https://relay.example.org" value={text()} onInput={e => setText(e.currentTarget.value)} /></label>
      <Show when={issue()}>{why => <p class="ph-err" role="alert">{why()}</p>}</Show>
    </div>
    <div class="ph-foot">
      <button class="btn btn-primary" onClick={save}>Save</button>
      <Show when={relay() && !relay()!.isDefault}><button class="btn btn-line" onClick={() => void write(null)}>Use default</button></Show>
    </div>
  </Screen>;
}

function AboutPage(props: { shell: PhoneShell; onBack: () => void }) {
  return <Screen label="About"><Bar title="About" onBack={props.onBack} />
    <div class="ph-body"><div class="ph-group">
      <div class="ph-row"><span class="ph-row-t"><span class="cap muted">Version</span><span class="tnum">{props.shell.version}</span></span></div>
      <div class="ph-row"><span class="ph-row-t"><span class="cap muted">This phone</span><span class="trunc">{props.shell.deviceName}</span></span></div>
    </div></div>
  </Screen>;
}

/**
 * The Android app's entry: mounts the phone into `root` and hands back what the native side calls. `back()` closes the
 * top-most sheet, menu, dialog or drawer, else goes back; false means nothing was left, so the app may close.
 */
export function mountPhone(root: HTMLElement, shell: PhoneShell, options: { logo?: JSX.Element } = {}): { back: () => boolean; dispose: () => void } {
  let back: () => boolean = () => false;
  const dispose = render(() => <PhoneApp shell={shell} logo={options.logo} onBack={b => { back = b; }} />, root);
  return { back: () => back(), dispose };
}
