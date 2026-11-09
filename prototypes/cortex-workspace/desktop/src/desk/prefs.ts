import { createEffect, createMemo, createSignal, onCleanup, untrack, type Accessor } from "solid-js";

/** User preferences: persisted in localStorage, never cleared by the prototype Reset (they are not fixture state). */
export const THEME_KEY = "cw.theme";
export type ThemePref = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

function storage(): Storage | undefined {
  try { return typeof localStorage === "undefined" ? undefined : localStorage; } catch { return undefined; }
}
export function readStored<T>(key: string, fallback: T, valid: (v: unknown) => boolean): T {
  try {
    const raw = storage()?.getItem(key);
    if (raw === null || raw === undefined) return fallback;
    const value = JSON.parse(raw) as unknown;
    return valid(value) ? value as T : fallback;
  } catch { return fallback; }
}
export function writeStored(key: string, value: unknown) {
  try { storage()?.setItem(key, JSON.stringify(value)); } catch { /* storage unavailable: preference lasts for this session only */ }
}
/** A signal that writes through to localStorage on every explicit set. */
export function persisted<T>(key: string, fallback: T, valid: (v: unknown) => boolean): [Accessor<T>, (v: T) => void] {
  const [get, set] = createSignal<T>(readStored(key, fallback, valid));
  return [get, (v: T) => { set(() => v); writeStored(key, v); }];
}

export function readThemePref(): ThemePref {
  // Stored as a bare string (the pre-paint script in index.html reads it without JSON parsing).
  let raw: string | null | undefined;
  try { raw = storage()?.getItem(THEME_KEY); } catch { raw = null; }
  return raw === "light" || raw === "dark" || raw === "system" ? raw : "system";
}
export const resolveTheme = (pref: ThemePref, systemDark: boolean): ResolvedTheme => pref === "system" ? (systemDark ? "dark" : "light") : pref;

export function applyTheme(theme: ResolvedTheme) {
  const root = document.documentElement;
  root.classList.add("theme-swap"); // disables transitions for one frame: the swap is instant
  root.dataset.theme = theme;
  root.style.colorScheme = theme;
  const later = typeof requestAnimationFrame === "function" ? requestAnimationFrame : (f: () => void) => setTimeout(f, 16);
  later(() => later(() => root.classList.remove("theme-swap")));
}

/** System / Light / Dark. System follows prefers-color-scheme live; an explicit choice ignores system changes. */
export function createTheme() {
  const mq = typeof window !== "undefined" && typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: dark)") : undefined;
  const [pref, setPrefSignal] = createSignal<ThemePref>(readThemePref());
  const [systemDark, setSystemDark] = createSignal(mq?.matches ?? false);
  const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
  mq?.addEventListener?.("change", onChange);
  onCleanup(() => mq?.removeEventListener?.("change", onChange));
  const resolved = createMemo(() => resolveTheme(pref(), systemDark()));
  createEffect(() => applyTheme(resolved()));
  return {
    pref, resolved, systemDark,
    setPref(next: ThemePref) {
      setPrefSignal(next);
      try { storage()?.setItem(THEME_KEY, next); } catch { /* session only */ }
    },
    label: () => `Theme: ${pref() === "system" ? "System" : pref() === "light" ? "Light" : "Dark"}, currently ${resolved()}`,
  };
}
export type Theme = ReturnType<typeof createTheme>;

export const TEXT_SIZES = [100, 125, 150, 200] as const;
export type TextSize = typeof TEXT_SIZES[number];
export type MotionPref = "system" | "on" | "off";
export type SortMode = "fixture" | "name" | "activity";

const isNum = (v: unknown) => typeof v === "number" && Number.isFinite(v);
const isBool = (v: unknown) => typeof v === "boolean";
const isStrArr = (v: unknown) => Array.isArray(v) && v.every(x => typeof x === "string");
const isRecord = (v: unknown) => typeof v === "object" && v !== null && !Array.isArray(v);

export function createPrefs() {
  const [sidebarWidth, setSidebarWidth] = persisted<number>("cw.sidebarWidth", 0, isNum);
  const [sidebarHidden, setSidebarHidden] = persisted<boolean>("cw.sidebarHidden", false, isBool);
  const [listPaneHidden, setListPaneHidden] = persisted<boolean>("cw.listPaneHidden", false, isBool);
  const [expanded, setExpanded] = persisted<string[]>("cw.expanded", [], isStrArr);
  const [sort, setSort] = persisted<SortMode>("cw.sort", "fixture", v => v === "fixture" || v === "name" || v === "activity");
  const [lastView, setLastView] = persisted<Record<string, string>>("cw.lastView", {}, isRecord);
  const [textSize, setTextSize] = persisted<TextSize>("cw.textSize", 100, v => (TEXT_SIZES as readonly unknown[]).includes(v));
  const [contentSize, setContentSize] = persisted<TextSize>("cw.contentSize", 100, v => (TEXT_SIZES as readonly unknown[]).includes(v));
  const [motion, setMotion] = persisted<MotionPref>("cw.motion", "system", v => v === "system" || v === "on" || v === "off");
  const [planOpen, setPlanOpen] = persisted<boolean>("cw.planOpen", false, isBool);
  const [singleKeys, setSingleKeys] = persisted<boolean>("cw.singleKeys", true, isBool);
  const [terminalOpen, setTerminalOpen] = persisted<boolean>("cw.terminalOpen", false, isBool);
  const [sinceOpen, setSinceOpen] = persisted<boolean>("cw.sinceOpen", false, isBool);
  createEffect(() => {
    const root = document.documentElement;
    root.style.fontSize = `${16 * textSize() / 100}px`;
    root.style.setProperty("--content-scale", String(contentSize() / 100));
    root.dataset.motion = motion();
  });
  return {
    sidebarWidth, setSidebarWidth, sidebarHidden, setSidebarHidden, listPaneHidden, setListPaneHidden,
    expanded, toggleExpanded(id: string) { const cur = untrack(expanded); setExpanded(cur.includes(id) ? cur.filter(x => x !== id) : [...cur, id]); },
    sort, setSort, lastView, setLastView(slug: string, view: string) {
      const cur = untrack(lastView);
      if (cur[slug] !== view) setLastView({ ...cur, [slug]: view });
    },
    textSize, setTextSize, contentSize, setContentSize, motion, setMotion, planOpen, setPlanOpen, singleKeys, setSingleKeys,
    terminalOpen, setTerminalOpen, sinceOpen, setSinceOpen,
  };
}
export type Prefs = ReturnType<typeof createPrefs>;
