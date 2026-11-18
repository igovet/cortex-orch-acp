import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { onBackButtonPress } from "@tauri-apps/api/app";
import { checkPermissions, requestPermissions, scan, Format } from "@tauri-apps/plugin-barcode-scanner";
import { mountPhone } from "../../../packages/workspace-ui/src";
import { pinLogoSource } from "../../../packages/workspace-ui/src/startup";
import { createPhoneShell, type PhoneInfo } from "./shell";

// The logo the page painted before any script ran. The first screen takes over this very element, so it is never redrawn.
const boot = document.getElementById("boot");
const logo = boot?.querySelector("img") ?? undefined;
pinLogoSource(logo);

void invoke<PhoneInfo>("phone_info").catch((): PhoneInfo => ({ version: "", deviceName: "Phone" })).then(info => {
  const shell = createPhoneShell({
    invoke: (command, args) => invoke(command, args),
    listen: (event, listener) => listen(event, e => listener(e.payload)),
    scanner: {
      permission: () => checkPermissions(),
      request: () => requestPermissions(),
      scan: async () => (await scan({ windowed: false, formats: [Format.QRCode] })).content,
    },
  }, info);
  const app = mountPhone(document.getElementById("root")!, shell, { logo });
  boot?.remove();
  // Android Back closes what is open, then walks history; with nothing left it leaves the app.
  void onBackButtonPress(() => { if (!app.back()) void invoke("phone_exit"); });
});
// The engine's page menu is not part of this app. Text fields and selectable text keep Copy and Paste.
document.addEventListener("contextmenu", e => {
  const editing = (e.target as Element).closest?.("input, textarea, [contenteditable='true']");
  if (!editing && !getSelection()?.toString()) e.preventDefault();
});
