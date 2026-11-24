import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Workspace, createRealPort } from "../../../packages/workspace-ui/src";
import { createDesktopTransport } from "./transport";
import { createWindowChrome } from "./window";
import { createTerminalPort } from "../../../packages/workspace-ui/src/terminal-client";
import { createVoicePort } from "../../../packages/workspace-ui/src/voice-client";
import { pinLogoSource } from "../../../packages/workspace-ui/src/startup";

const native = createDesktopTransport({
  invoke: (body) => invoke("workspace_request", body),
  listen: (listener) => listen("workspace-transport", event => listener(event.payload)),
});
// One for the window's life: a prop is read again on every access, and a second chrome would close past the first one's guard.
const chrome = createWindowChrome(getCurrentWindow(), async () => {
  const root = document.getElementById("root")!;
  root.inert = true;
  try { await native.close(() => invoke<void>("workspace_close")); }
  finally { root.inert = false; }
});
// not_started has no line: it is the normal state before a project is activated, not something to act on.
const setupCopy: Record<string, string> = {
  downloading: "Installing the qualified adapter…", connecting: "Connecting to your native account…",
  refreshing_models: "Discovering available models…", download_failed: "Adapter download failed. Check network access and restart the app.",
  login_required: "Native login required. Sign in with codex login or claude /login, then restart the app.",
  opted_out: "Automatic connection is disabled.", catalog_failed: "Model discovery failed. Check the sandbox and native login, then restart the app.",
  setup_failed: "Engine setup failed. Check native login and sandbox availability, then restart the app.",
  downloaded: "Adapter installed; connection setup pending.",
};
// A click on one of this window's notifications: the shell sends back only the target it validated and kept for that notification.
const notificationClicks = (listener: (target: unknown) => void) => {
  const stop = listen("workspace-navigate", event => listener(event.payload));
  return () => { void stop.then(unlisten => unlisten(), () => {}); };
};
// The machine service raises the notifications when it can: the window tells it what the focused window shows and which
// kinds the owner wants, and collects the place of a click that started this window.
const notificationService = {
  report: (report: unknown) => native.transport.request("notifications.report", report) as Promise<{ raising?: boolean }>,
  take: () => native.transport.request("notifications.take", {}) as Promise<{ target?: unknown }>,
};
// The logo the page painted before any script ran. The startup screen takes over this very element, so it is never redrawn.
const boot = document.getElementById("boot");
const logo = boot?.querySelector("img") ?? undefined;
// Its relative path is fixed to an absolute address now, before the router moves the page to /m/… (startup.tsx says why).
pinLogoSource(logo);
function Desktop() {
  const [setup, setSetup] = createSignal<[string, string][]>([]);
  // One line per state, not one per engine: two engines in the same state say the same thing once.
  const notices = () => [...new Set(setup().map(([, phase]) => phase))].map(phase =>
    [setup().filter(([, p]) => p === phase).map(([engine]) => engine === "codex-acp" ? "Codex" : "Claude").join(" and "), setupCopy[phase]] as const);
  onMount(() => {
    let active = true, timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const status = await native.transport.request("service.status", {}) as { engines?: Record<string, string> };
        if (active) setSetup(Object.entries(status.engines ?? {}).filter(([, phase]) => phase !== "ready" && setupCopy[phase]));
      } catch { if (active) setSetup([]); }
      if (active) timer = setTimeout(() => void poll(), 3000);
    };
    void poll();
    onCleanup(() => { active = false; clearTimeout(timer); });
  });
  return <Workspace logo={logo} openResultTarget={target => invoke<void>("workspace_open_result_target", { target })} notify={({ label, target }) => invoke<void>("workspace_notify", { label, target })} notificationClicks={notificationClicks} notificationService={notificationService} voice={createVoicePort(native.transport)} terminal={createTerminalPort(native.transport)} port={createRealPort({ transport: native.transport, registry: true, localCode: true, actorId: "local-peer", boardStorage: {getItem:key => localStorage.getItem(key),setItem:(key,value) => localStorage.setItem(key,value)} })} chrome={chrome}
    notice={<><Show when={chrome.closeError()}>{reason => <div class="strip bad" role="alert">{reason()}</div>}</Show><Show when={setup().length}><div class="strip info" role="status"><For each={notices()}>{([engines, copy]) =>
      <span>{engines}: {copy} </span>
    }</For></div></Show></>} />;
}
// GTK measures the frameless window's edge resize zone as 5 pixels times the display scale; scrollers stop short of it.
document.documentElement.style.setProperty("--ws-edge", `${5 * Math.max(1, Math.round(devicePixelRatio)) + 1}px`);
// The logo is taken over only once it has arrived (or failed): an image moved while it is still loading can end up as the engine's
// missing-image glyph instead of the logo. Until then the static page keeps showing it; a load that never ends does not hold the app.
const settled = !logo || logo.complete ? Promise.resolve() : new Promise<void>(done => {
  setTimeout(done, 1500);
  logo.addEventListener("load", () => done(), { once: true });
  logo.addEventListener("error", () => done(), { once: true });
});
void settled.then(() => {
  render(() => <Desktop />, document.getElementById("root")!);
  boot?.remove();
});
// The engine's page menu (Back, Reload) is not part of this app. Text fields and selectable text keep Copy and Paste.
document.addEventListener("contextmenu", e => {
  const editing = (e.target as Element).closest?.("input, textarea, [contenteditable='true']");
  if (!editing && !getSelection()?.toString()) e.preventDefault();
});
window.addEventListener("pagehide", () => native.dispose(), { once: true });
