import { createEffect, createMemo, createSignal, onCleanup, type Accessor } from "solid-js";

/** User preferences: persisted in localStorage. They are the owner's, so nothing a backend does ever clears them. */
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
