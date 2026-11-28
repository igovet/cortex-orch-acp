import { Show } from "solid-js";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Workspace, createRealPort } from "../../../packages/workspace-ui/src";
import { createDesktopTransport } from "./transport";
import { EngineSetupNotice } from "../../../packages/workspace-ui/src/engine-setup-notice";
import { createWindowChrome } from "./window";
import { createTerminalPort } from "../../../packages/workspace-ui/src/terminal-client";
import { createVoicePort } from "../../../packages/workspace-ui/src/voice-client";
import { pinLogoSource } from "../../../packages/workspace-ui/src/startup";

const native = createDesktopTransport({
  historyOpen: (id, params) => invoke<void>("workspace_history_open", { id, params }),
  historyNext: id => invoke("workspace_history_next", { id }),
  historyClose: id => invoke<void>("workspace_history_close", { id }),
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
  return <Workspace draftNamespace={["desktop", "local-peer"]} logo={logo} openResultTarget={target => invoke<void>("workspace_open_result_target", { target })} notify={({ label, target }) => invoke<void>("workspace_notify", { label, target })} notificationClicks={notificationClicks} notificationService={notificationService} voice={createVoicePort(native.transport)} terminal={createTerminalPort(native.transport)} port={createRealPort({ transport: native.transport, registry: true, localCode: true, actorId: "local-peer", boardStorage: {getItem:key => localStorage.getItem(key),setItem:(key,value) => localStorage.setItem(key,value)} })} chrome={chrome}
    notice={<><Show when={chrome.closeError()}>{reason => <div class="strip bad" role="alert">{reason()}</div>}</Show><EngineSetupNotice read={() => native.transport.request("service.status", {}) as Promise<{ engines?: Record<string, string> }>} /></>} />;
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
