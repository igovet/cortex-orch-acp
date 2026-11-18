// A manual preview of the phone-only screens with a scripted native side: no machine, no network, no pairing.
// Serve with `npx vite --config vite.config.ts` and open /test/preview/index.html?machines=0|2&dark=1.
import { mountPhone } from "../../../../packages/workspace-ui/src";
import type { PhoneShell, PhoneState } from "../../../../packages/workspace-ui/src/phone-shell";

const query = new URLSearchParams(location.search);
if (query.get("dark")) document.documentElement.dataset.theme = "dark";
const machines = query.get("machines") === "2" ? [
  { id: "a", name: "Workstation", state: "offline" as const, path: null, lastConnectedAt: Date.now() - 3 * 60_000 },
  { id: "b", name: "GPU box", state: "offline" as const, path: null, lastConnectedAt: Date.now() - 26 * 3600_000 },
] : [];
const state: PhoneState = { active: machines[0]?.id ?? null, machines };
const never = new Promise<never>(() => undefined);
const shell: PhoneShell = {
  version: "1.0.6", deviceName: "Pixel 9",
  state: async () => state, subscribe: () => () => undefined,
  scan: async () => { throw new Error("camera_denied"); },
  pairBegin: async link => { if (!link.startsWith("cortex-pair:")) throw new Error("invalid_link"); return { machineName: "igor-workstation", fingerprint: "4F2A 91C7 D03E" }; },
  pairFinish: () => never, pairCancel: async () => undefined,
  select: async () => undefined, rename: async () => undefined, forget: async () => undefined, retry: async () => undefined,
  transport: () => ({ request: () => Promise.reject(new Error("Machine service disconnected")), subscribe: listener => { listener({ type: "connection", state: "disconnected", reason: "Machine service disconnected" }); return () => undefined; } }),
};
const app = mountPhone(document.getElementById("root")!, shell);
(window as unknown as { phoneBack: () => boolean }).phoneBack = app.back;
