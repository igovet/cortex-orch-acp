import { createSignal, onCleanup } from "solid-js";

export const PROJECT_VIEWS = ["overview", "board", "list", "decisions", "git", "machine"] as const;
export type ProjectViewId = typeof PROJECT_VIEWS[number];
export const TASK_TABS = ["discussion", "decisions", "result", "workers", "code", "git"] as const;
export type TaskTab = typeof TASK_TABS[number];
export const SETTINGS_SECTIONS = ["appearance", "keyboard", "about"] as const;
export type SettingsSection = typeof SETTINGS_SECTIONS[number] | "project";

export type Route =
  | { name: "home" }
  | { name: "needs" }
  | { name: "project"; slug: string; view: ProjectViewId | null }
  | { name: "task"; slug: string; num: string; tab: TaskTab }
  | { name: "settings"; section: SettingsSection; slug?: string }
  | { name: "notfound"; path: string };

/** Every view is a real History API route. `/` and `/a` both serve the all-projects dashboard. */
export function parseRoute(pathname: string): Route {
  const parts = pathname.replace(/\/+$/, "").split("/").filter(Boolean).map(decodeURIComponent);
  if (parts.length === 0 || (parts.length === 1 && parts[0] === "a")) return { name: "home" };
  if (parts[0] === "needs-you" && parts.length === 1) return { name: "needs" };
  if (parts[0] === "p" && parts[1]) {
    if (parts.length === 2) return { name: "project", slug: parts[1], view: null };
    if (parts.length === 3 && (PROJECT_VIEWS as readonly string[]).includes(parts[2])) return { name: "project", slug: parts[1], view: parts[2] as ProjectViewId };
    if (parts[2] === "t" && parts[3] && parts.length <= 5) {
      const tab = (parts[4] ?? "discussion") as TaskTab;
      if ((TASK_TABS as readonly string[]).includes(tab)) return { name: "task", slug: parts[1], num: parts[3], tab };
    }
  }
  if (parts[0] === "settings") {
    if (parts.length === 1) return { name: "settings", section: "appearance" };
    if (parts.length === 2 && (SETTINGS_SECTIONS as readonly string[]).includes(parts[1])) return { name: "settings", section: parts[1] as SettingsSection };
    if (parts.length === 3 && parts[1] === "projects") return { name: "settings", section: "project", slug: parts[2] };
  }
  return { name: "notfound", path: pathname };
}

export function hrefOf(route: Route): string {
  switch (route.name) {
    case "home": return "/";
    case "needs": return "/needs-you";
    case "project": return `/p/${route.slug}${route.view ? `/${route.view}` : ""}`;
    case "task": return `/p/${route.slug}/t/${route.num}/${route.tab}`;
    case "settings": return route.section === "project" ? `/settings/projects/${route.slug}` : `/settings/${route.section}`;
    case "notfound": return route.path;
  }
}

/** True once the user has navigated inside the app (not on the initial page load). */
export const navState = { userNavigated: false };

/** Route state. Only user input calls `go`; incoming data never changes the route. */
export function createRouter() {
  const [path, setPath] = createSignal(location.pathname);
  const [search, setSearch] = createSignal(location.search);
  const sync = () => { navState.userNavigated = true; setPath(location.pathname); setSearch(location.search); };
  window.addEventListener("popstate", sync);
  onCleanup(() => window.removeEventListener("popstate", sync));
  return {
    path, search, route: () => parseRoute(path()),
    go(to: string | Route, opts: { replace?: boolean } = {}) {
      const href = typeof to === "string" ? to : hrefOf(to);
      if (href === location.pathname + location.search) return;
      if (opts.replace) history.replaceState(null, "", href); else history.pushState(null, "", href);
      sync();
    },
  };
}
export type Router = ReturnType<typeof createRouter>;
