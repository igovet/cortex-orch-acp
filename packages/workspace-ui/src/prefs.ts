import { createSignal, onCleanup, untrack } from "solid-js";
import { persisted } from "./theme";
import type { PlanView } from "./sprint-space";

export const SIDEBAR_MIN = 220, SIDEBAR_MAX = 360, SIDEBAR_DEFAULT = 260;
export type ProjectSort = "default" | "name" | "activity";
/** Where speech is turned into text. The provider is still an open choice for this product, so none is the default. */
export type VoicePrefs = { provider: string; language: string; microphone: string };
/** What has taken the sidebar away for now, without touching the owner's choice: Full view, or a window with no room for it. */
export type SidebarYield = "full" | "room" | null;
/** What the palette can ask of the sprint plan on view. `fit` and `full` are null while the plan is not in the Graph view. */
export type GraphActions = { show: (() => void) | null; fit: (() => void) | null; full: (() => void) | null; fullOn: boolean };

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
  const [planView, setPlanView] = persisted<PlanView>("cw.plan.view", "list", v => v === "list" || v === "graph");
  const [convHidden, setConvHidden] = persisted<boolean>("cw.sprint.convHidden", false, isBool);
  // Null until the owner first presses the toggle: until then the minimap follows the canvas width.
  const [minimap, setMinimap] = persisted<boolean | null>("cw.graph.minimap", null, v => v === null || isBool(v));
  const [links, setLinks] = persisted<"direct" | "chain">("cw.graph.links", "direct", v => v === "direct" || v === "chain");
  // Held by the screen on view. Nothing here is stored: a yield ends as soon as its screen lets go.
  const holds = new Map<object, { why: Exclude<SidebarYield, null>; opened: () => void }>();
  const [held, setHeld] = createSignal<SidebarYield>(null);
  const heldNow = (): SidebarYield => { let out: SidebarYield = null; for (const h of holds.values()) { if (h.why === "full") return "full"; out = h.why; } return out; };
  const graphs = new Map<object, GraphActions>();
  const [graph, setGraph] = createSignal<GraphActions | null>(null);
  const [overlayOpen, setOverlayOpen] = createSignal(false);
  const query = mediaQuery(1440);
  const [wide, setWide] = createSignal(query?.matches ?? true);
  const onWidth = (e: MediaQueryListEvent) => setWide(e.matches);
  query?.addEventListener?.("change", onWidth);
  onCleanup(() => query?.removeEventListener?.("change", onWidth));
  return {
    sidebarHidden, sidebarWidth, inspectorHidden, overlayOpen, terminalOpen, expanded, pins, sort, setSort, lastMachine, setLastMachine, wide,
    notifications, setNotifications, autosave, setAutosave, voice, setVoice, advanced,
    planView, setPlanView, convHidden, setConvHidden, minimap, setMinimap, links, setLinks, graph,
    setAdvanced(id: string, open: boolean) { setAdvanced({ ...untrack(advanced), [id]: open }); },
    /** The sidebar as drawn: the owner's choice, unless a screen holds it away for now. */
    sidebarShown: () => !sidebarHidden() && held() === null,
    sidebarYield: held,
    /** A screen takes the sidebar away (`why`) or lets it go (null). `opened` runs when the owner asks for it back while there is no room. */
    yieldSidebar(owner: object, why: SidebarYield, opened: () => void = () => {}) {
      if (why) holds.set(owner, { why, opened }); else holds.delete(owner);
      const now = heldNow();
      if (now !== untrack(held)) setHeld(now);
    },
    /** The palette's view of the sprint plan on screen; the screen sets it while shown and clears it after. */
    offerGraph(owner: object, actions: GraphActions | null) {
      if (actions) graphs.set(owner, actions); else graphs.delete(owner);
      setGraph(() => [...graphs.values()].at(-1) ?? null);
    },
    /** Full view writes no preference, so the sidebar stays as it is. With no room the owner's press asks the screen to make room. */
    toggleSidebar() {
      const now = untrack(held);
      if (now === "full") return;
      if (now === "room" && !untrack(sidebarHidden)) { for (const h of holds.values()) h.opened(); return; }
      setSidebarHidden(!untrack(sidebarHidden));
    },
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
