import { createEffect, createSignal, For, onCleanup, Show, useContext } from "solid-js";
import { FEATURES, type RemoteOperation, type RemoteRequests, type RemoteStatus } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { ScreenActive } from "./freshness";
import { I } from "./kit";
import { ActionBar } from "./layout";
import { day, when } from "./phone-words";
import { SetH, Srow, Sw } from "./settings-kit";
import "./remote-access.css";

/** While a pairing is offered or a device waits for Allow, the status is read again this often. */
export const REMOTE_POLL_MS = 2000;
/** The quiet zone around a QR code, in modules. Scanners need it; four is what the standard asks for. */
export const QR_QUIET = 4;

/**
 * A QR code drawn from the machine's module matrix: dark modules on a light plate in either theme, so a camera can read it
 * on a dark screen too. One square per dark module, inside the quiet zone. No image is fetched and nothing is encoded here.
 */
export function QrCode(props: { rows: string[]; label: string }) {
  const size = () => (props.rows[0]?.length ?? 0) + QR_QUIET * 2;
  const path = () => props.rows.flatMap((row, y) => [...row].flatMap((cell, x) => cell === "1" ? [`M${x + QR_QUIET} ${y + QR_QUIET}h1v1h-1z`] : [])).join("");
  return <svg class="ra-qr" viewBox={`0 0 ${size()} ${size()}`} role="img" aria-label={props.label} shape-rendering="crispEdges">
    <rect class="ra-qr-plate" width={size()} height={size()} fill="#ffffff" />
    <path class="ra-qr-dots" d={path()} fill="#000000" />
  </svg>;
}

const left = (ms: number) => { const s = Math.max(0, Math.ceil(ms / 1000)); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`; };
/** The address a relay override must have. The machine checks it again; this only spares a round trip for an obvious slip. */
export function relayProblem(text: string): string | null {
  const value = text.trim();
  if (!value) return "Enter the relay server address.";
  let url: URL;
  try { url = new URL(value); } catch { return "Enter a full address, starting with https://."; }
  if (url.protocol !== "https:") return "The relay server address must start with https://.";
  if (!url.hostname) return "Enter a full address, starting with https://.";
  return null;
}

/**
 * Settings > Remote access of this computer, driven by its machine service: the switch, pairing a device by its QR code,
 * the requests waiting for Allow, paired devices and the relay. Nothing here is decided by the interface: every change is the
 * service's answer.
 */
export function RemoteAccessBody() {
  const ws = useWs();
  const shown = useContext(ScreenActive);
  const [status, setStatus] = createSignal<RemoteStatus | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [now, setNow] = createSignal(Date.now());
  const [relayText, setRelayText] = createSignal<string | null>(null);
  const [relayIssue, setRelayIssue] = createSignal<string | null>(null);
  const [copied, setCopied] = createSignal(false);
  const offered = () => ws.can(FEATURES.remoteAccess);
  const connected = () => ws.store.connection().state === "connected";
  const message = (error: unknown) => error instanceof Error ? error.message : String(error);

  let reading = 0;
  const read = () => {
    const turn = ++reading;
    return ws.store.remoteAccess("remote.status", {}).then(next => { if (turn === reading) { setStatus(next); setProblem(null); } }, error => { if (turn === reading) setProblem(message(error)); });
  };
  const act = <O extends RemoteOperation>(operation: O, body: RemoteRequests[O]) => {
    if (busy()) return Promise.resolve(false);
    setBusy(true);
    reading++;
    return ws.busy(ws.store.remoteAccess(operation, body)).then(next => { setStatus(next); setProblem(null); return true; }, error => { setProblem(message(error)); return false; }).finally(() => setBusy(false));
  };
  createEffect(() => {
    if (!offered() || !shown()) return;
    void read();
    onCleanup(ws.store.watchRemoteAccess(() => void read()));
  });
  const waiting = () => { const s = status(); return !!s && s.enabled && (s.pairing !== null || s.pending.length > 0); };
  createEffect(() => {
    if (!offered() || !shown() || !waiting()) return;
    const poll = setInterval(() => void read(), REMOTE_POLL_MS);
    const tick = setInterval(() => setNow(Date.now()), 1000);
    setNow(Date.now());
    onCleanup(() => { clearInterval(poll); clearInterval(tick); });
  });

  const turn = (on: boolean) => {
    const s = status();
    if (on || !s?.devices.some(d => d.connected)) return void act("remote.enable", { enabled: on });
    ws.open({ kind: "confirm", title: "Turn off remote access?", note: "Every device is disconnected now. Paired devices stay paired and can connect again once you turn it back on.", action: "Turn off", danger: true, go: () => act("remote.enable", { enabled: false }) });
  };
  const revoke = (deviceId: string, name: string) => ws.open({ kind: "confirm", title: `Revoke ${name}?`, note: "Revoking disconnects it immediately. It has to be paired again to connect.", action: "Revoke", danger: true, go: () => act("remote.devices.revoke", { deviceId }) });
  const copy = (link: string) => void navigator.clipboard?.writeText(link).then(() => { setCopied(true); setTimeout(() => setCopied(false), 2000); }, () => setProblem("The link could not be copied."));
  const saveRelay = () => {
    const text = relayText() ?? "";
    const why = relayProblem(text);
    setRelayIssue(why);
    if (!why) void act("remote.relay.set", { url: text.trim() }).then(ok => { if (ok) setRelayText(null); });
  };

  return <Show when={connected()} fallback={<div class="note-box plain" role="status">{ws.t("disconnected")}</div>}>
    <Show when={offered()} fallback={<div class="note-box plain" role="status">Remote access is not available in this build.</div>}>
      <Show when={problem()}>{why => <div class="note-box" role="alert"><I n="alert" s={14} class="st-warn" /><span>{why()}</span><button class="btn btn-line btn-sm" onClick={() => void read()}>Try again</button></div>}</Show>
      <Show when={status()} fallback={<Show when={!problem()}><div class="cap muted" role="status">Reading remote access…</div></Show>}>{s => <>
        <Show when={s().available} fallback={<div class="note-box plain" role="status"><I n="alert" s={14} class="faint" /><span>Remote access is unavailable on this machine.{s().reason ? ` ${s().reason}` : ""}</span></div>}>
          <div class="surface">
            <Srow t="Allow remote access" d="Off by default. Turning it off disconnects every device."><Sw label="Allow remote access" checked={s().enabled} disabled={busy() ? "Waiting for the machine" : undefined} onChange={turn} /></Srow>
          </div>
          <Show when={s().enabled}>
            <Show when={s().relay.source === "none"}><div class="note-box plain"><I n="wifi-off" s={14} class="faint" /><span>No relay server is set, so pairing works on the same network only.</span></div></Show>
            <div class="surface ra-pair">
              <Show when={s().pairing} fallback={<Srow t="Pair a device" d="Shows a QR code for the phone app. It works once and expires after a few minutes."><button class="btn btn-primary btn-sm" onClick={() => void act("remote.pairing.start", {})}>Show QR code</button></Srow>}>{p => <>
                <Show when={p().expiresAt > now()} fallback={<Srow t="Pair a device" d="This link expired. Make a new one to pair a device."><button class="btn btn-primary btn-sm" onClick={() => void act("remote.pairing.start", {})}>New link</button></Srow>}>
                  <div class="ra-pairbox">
                    <QrCode rows={p().qr} label="Pairing QR code" />
                    <div class="ra-pairinfo">
                      <span class="w500">Pair a device</span>
                      <span class="cap muted">Scan with the phone app, or paste the link there.</span>
                      <span class="cap faint tnum" role="timer">Works once · expires in {left(p().expiresAt - now())}</span>
                      <div class="row ra-row">
                        <button class="btn btn-line" onClick={() => void act("remote.pairing.start", {})}>New link</button>
                        <button class="btn" onClick={() => copy(p().link)}><I n={copied() ? "check" : "copy"} s={14} />{copied() ? "Copied" : "Copy link"}</button>
                        <button class="btn" onClick={() => void act("remote.pairing.cancel", {})}>Stop pairing</button>
                      </div>
                    </div>
                  </div>
                </Show>
              </>}</Show>
            </div>
            <For each={s().pending}>{r => <div class="ra-ask" role="group" aria-label={`${r.deviceName} wants to connect`}>
              <div class="row"><I n="shield" class="st-warn" /><span class="w500 trunc">{r.deviceName} wants to connect</span><span class="cap faint ra-at">{when(r.requestedAt, now())}</span></div>
              <p class="cap muted">Allow it only if both screens show the same fingerprint.</p>
              <div class="ra-fpr"><div><span class="cap muted">This machine shows</span><code>{r.fingerprint}</code></div><div><span class="cap muted">The phone must show</span><code>{r.fingerprint}</code></div></div>
              <ActionBar place="card"><button class="btn btn-line" onClick={() => void act("remote.pairing.decide", { requestId: r.requestId, allow: false })}>Deny</button><button class="btn btn-primary" onClick={() => void act("remote.pairing.decide", { requestId: r.requestId, allow: true })}>Allow</button></ActionBar>
            </div>}</For>
          </Show>
          <SetH title="Devices" count={s().devices.length} />
          <Show when={s().devices.length > 0} fallback={<div class="surface ws-empty">No device is paired with this machine.</div>}>
            <div class="surface"><For each={s().devices}>{d => <Srow t={d.name} d={`Paired ${day(d.pairedAt)}${d.connected ? " · connected now" : d.lastSeenAt !== null ? ` · last connected ${when(d.lastSeenAt, now())}` : ""}`}>
              <button class="btn btn-line btn-sm" onClick={() => revoke(d.deviceId, d.name)}>Revoke…</button>
            </Srow>}</For></div>
          </Show>
          <SetH title="Relay server" />
          <div class="surface">
            <Srow t="Relay server" d={s().relay.source === "default" ? "The built-in relay. It forwards encrypted traffic only." : s().relay.source === "override" ? "Your own relay. It forwards encrypted traffic only." : "None. Devices connect on the same network only."}>
              <Show when={relayText() !== null} fallback={<>
                <Show when={s().relay.url}>{url => <span class="mono cap trunc ra-url">{url()}</span>}</Show>
                <button class="btn btn-line btn-sm" onClick={() => { setRelayIssue(null); setRelayText(s().relay.source === "override" ? s().relay.url ?? "" : ""); }}>Change…</button>
                <Show when={s().relay.source === "override"}><button class="btn btn-sm" onClick={() => void act("remote.relay.set", { url: null })}>Use default</button></Show>
              </>}>
                <input class="inp mono ra-url" aria-label="Relay server address" placeholder="https://relay.example.org" value={relayText() ?? ""} onInput={e => setRelayText(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") saveRelay(); if (e.key === "Escape") setRelayText(null); }} />
                <button class="btn btn-primary btn-sm" onClick={saveRelay}>Save</button>
                <button class="btn btn-sm" onClick={() => setRelayText(null)}>Cancel</button>
              </Show>
            </Srow>
            <Show when={relayIssue()}>{why => <div class="cap st-warn ra-issue" role="alert">{why()}</div>}</Show>
          </div>
        </Show>
      </>}</Show>
    </Show>
  </Show>;
}
