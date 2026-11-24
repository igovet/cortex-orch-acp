import type { PhoneMachineView } from "./ctx";
import { clock } from "./selectors";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const dayStart = (at: number) => { const d = new Date(at); d.setHours(0, 0, 0, 0); return d.getTime(); };
/** "today 9:12", "yesterday 22:14" or "2 Nov 9:12". */
export function when(at: number, now = Date.now()): string {
  const days = Math.round((dayStart(now) - dayStart(at)) / 86_400_000);
  const d = new Date(at);
  return days === 0 ? `today ${clock(at)}` : days === 1 ? `yesterday ${clock(at)}` : `${d.getDate()} ${MONTHS[d.getMonth()]} ${clock(at)}`;
}
/** "2 Nov". */
export const day = (at: number) => { const d = new Date(at); return `${d.getDate()} ${MONTHS[d.getMonth()]}`; };

export const UPDATE_NEEDED = "The computer and this app are different versions. Update the app or the computer so both match.";

export function machineRefusal(state: PhoneMachineView["state"]): string | undefined {
  if (state === "rejected") return "This phone is no longer paired with this machine. Pair it again on the machine.";
  if (state === "update_needed") return UPDATE_NEEDED;
}

/** How a machine is reached, in the words of the machine sheet. An unknown path is not guessed. */
export function machineLine(m: Pick<PhoneMachineView, "state" | "path" | "lastConnectedAt">, now = Date.now()): { text: string; tone: string } {
  if (m.state === "connected") return m.path === "direct" ? { text: "Connected · direct", tone: "st-ok" } : m.path === "relay" ? { text: "Connected · through relay", tone: "muted" } : { text: "Connected", tone: "st-ok" };
  if (m.state === "rejected") return { text: "Not paired", tone: "muted" };
  if (m.state === "update_needed") return { text: "Update needed", tone: "muted" };
  if (m.state === "connecting") return { text: "Connecting…", tone: "muted" };
  return { text: m.lastConnectedAt === null ? "Offline" : `Offline · last connected ${when(m.lastConnectedAt, now)}`, tone: "faint" };
}
