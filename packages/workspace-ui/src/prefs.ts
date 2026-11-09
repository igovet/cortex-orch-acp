import { createSignal, onCleanup, untrack } from "solid-js";
import { persisted } from "./theme";

export const SIDEBAR_MIN = 220, SIDEBAR_MAX = 360, SIDEBAR_DEFAULT = 260;
export type ProjectSort = "default" | "name" | "activity";
/** Where speech is turned into text. The provider is still an open choice for this product, so none is the default. */
export type VoicePrefs = { provider: string; language: string; microphone: string };

const isBool = (v: unknown) => typeof v === "boolean";
const isStrings = (v: unknown) => Array.isArray(v) && v.every(x => typeof x === "string");
const isRecord = (v: unknown) => typeof v === "object" && v !== null && !Array.isArray(v);
const mediaQuery = (px: number) => typeof window.matchMedia === "function" ? window.matchMedia(`(min-width: ${px}px)`) : undefined;

/**
 * Panel layout and small UI facts. Every panel keeps one fixed toggle and its state survives a reload; Reset never touches these.
 * The inspector is docked on wide windows and an overlay on narrow ones, as in the mockups, so it has one state for each.
 */
export function createPanels() {
  const [sidebarHidden, setSidebarHidden] = persisted<boolean>("cw.ws.sidebarHidden", false, isBool);
  const [sidebarWidth, storeWidth] = persisted<number>("cw.ws.sidebarWidth", SIDEBAR_DEFAULT, v => typeof v === "number" && v >= SIDEBAR_MIN && v <= SIDEBAR_MAX);
  const [inspectorHidden, setInspectorHidden] = persisted<boolean>("cw.ws.inspectorHidden", false, isBool);
  const [terminalOpen, setTerminalOpen] = persisted<boolean>("cw.ws.terminalOpen", false, isBool);
  const [expanded, setExpanded] = persisted<Record<string, boolean>>("cw.ws.expanded", {}, isRecord);
  const [pins, setPins] = persisted<string[]>("cw.ws.pins", [], isStrings);
  const [sort, setSort] = persisted<ProjectSort>("cw.ws.sort", "default", v => v === "default" || v === "name" || v === "activity");
  const [lastMachine, setLastMachine] = persisted<string>("cw.ws.machine", "", v => typeof v === "string");
  const [notifications, setNotifications] = persisted<boolean>("cw.ws.notifications", true, isBool);
  const [autosave, setAutosave] = persisted<boolean>("cw.ws.autosave", false, isBool);
  const [voice, setVoice] = persisted<VoicePrefs>("cw.ws.voice", { provider: "none", language: "same", microphone: "default" }, isRecord);
  const [advanced, setAdvanced] = persisted<Record<string, boolean>>("cw.ws.advanced", {}, isRecord);
  const [overlayOpen, setOverlayOpen] = createSignal(false);
  const query = mediaQuery(1440);
  const [wide, setWide] = createSignal(query?.matches ?? true);
  const onWidth = (e: MediaQueryListEvent) => setWide(e.matches);
  query?.addEventListener?.("change", onWidth);
  onCleanup(() => query?.removeEventListener?.("change", onWidth));
  return {
    sidebarHidden, sidebarWidth, inspectorHidden, overlayOpen, terminalOpen, expanded, pins, sort, setSort, lastMachine, setLastMachine, wide,
    notifications, setNotifications, autosave, setAutosave, voice, setVoice, advanced,
    setAdvanced(id: string, open: boolean) { setAdvanced({ ...untrack(advanced), [id]: open }); },
    toggleSidebar: () => setSidebarHidden(!untrack(sidebarHidden)),
    setSidebarWidth: (px: number) => storeWidth(Math.round(Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, px)))),
    inspectorVisible: () => wide() ? !inspectorHidden() : overlayOpen(),
    toggleInspector() {
      if (untrack(wide)) setInspectorHidden(!untrack(inspectorHidden));
      else setOverlayOpen(!untrack(overlayOpen));
    },
    closeOverlay: () => setOverlayOpen(false),
    toggleTerminal: () => setTerminalOpen(!untrack(terminalOpen)),
    setExpanded(id: string, open: boolean) { setExpanded({ ...untrack(expanded), [id]: open }); },
    togglePin(id: string) { const cur = untrack(pins); setPins(cur.includes(id) ? cur.filter(x => x !== id) : [...cur, id]); },
  };
}
export type Panels = ReturnType<typeof createPanels>;
