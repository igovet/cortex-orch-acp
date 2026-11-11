import type { Route } from "./router";

/** A browser-style tab: its own Back/Forward stack over the routes it has shown. */
export type Tab = { id: number; entries: string[]; at: number };
export type TabList = { tabs: Tab[]; active: number };
export type StoredTabs = { tabs: { id: number; href: string }[]; active: number };
export type TabNames = { project: (id: string) => string | undefined; task: (id: string) => string | undefined; sprint: (id: string) => string | undefined };

/** Redirects to the machine's home once the tab is shown, so a new tab needs no machine of its own. */
export const FRESH_HREF = "/";
const MAX_ENTRIES = 50;

export const hrefIn = (tab: Tab) => tab.entries[tab.at];
export const tabOf = (list: TabList, id: number) => list.tabs.find(t => t.id === id);
export const activeOf = (list: TabList) => tabOf(list, list.active) ?? list.tabs[0];
const nextId = (list: TabList) => list.tabs.reduce((n, t) => Math.max(n, t.id), 0) + 1;
const blank = (id: number, href: string): Tab => ({ id, entries: [href], at: 0 });
const put = (list: TabList, tab: Tab): TabList => ({ ...list, tabs: list.tabs.map(t => t.id === tab.id ? tab : t) });

export const firstTabs = (href: string): TabList => ({ tabs: [blank(1, href)], active: 1 });

export function addTab(list: TabList, href = FRESH_HREF): TabList {
  const id = nextId(list);
  return { tabs: [...list.tabs, blank(id, href)], active: id };
}

/** Closing the last tab leaves one fresh tab, never an empty window. */
export function closeTab(list: TabList, id: number): TabList {
  const at = list.tabs.findIndex(t => t.id === id);
  if (at < 0) return list;
  if (list.tabs.length === 1) { const fresh = nextId(list); return { tabs: [blank(fresh, FRESH_HREF)], active: fresh }; }
  const tabs = list.tabs.filter(t => t.id !== id);
  return { tabs, active: list.active === id ? (tabs[at] ?? tabs[at - 1]).id : list.active };
}

export const selectTab = (list: TabList, id: number): TabList => list.active === id || !tabOf(list, id) ? list : { ...list, active: id };

export function moveTab(list: TabList, id: number, to: number): TabList {
  const from = list.tabs.findIndex(t => t.id === id), target = Math.max(0, Math.min(list.tabs.length - 1, to));
  if (from < 0 || from === target) return list;
  const tabs = [...list.tabs];
  tabs.splice(target, 0, ...tabs.splice(from, 1));
  return { ...list, tabs };
}

export function cycleTab(list: TabList, by: number): TabList {
  const at = list.tabs.findIndex(t => t.id === list.active), n = list.tabs.length;
  return selectTab(list, list.tabs[(at + by + n) % n].id);
}

/** Ctrl+1 to Ctrl+8 go to that position, and Ctrl+9 to the last tab, as in a browser. */
export function jumpTab(list: TabList, n: number): TabList {
  const tab = n === 9 ? list.tabs[list.tabs.length - 1] : list.tabs[n - 1];
  return tab ? selectTab(list, tab.id) : list;
}

export function visitTab(list: TabList, id: number, href: string, replace: boolean): TabList {
  const tab = tabOf(list, id);
  if (!tab || hrefIn(tab) === href) return list;
  if (replace) return put(list, { ...tab, entries: tab.entries.map((e, i) => i === tab.at ? href : e) });
  const entries = [...tab.entries.slice(0, tab.at + 1), href].slice(-MAX_ENTRIES);
  return put(list, { ...tab, entries, at: entries.length - 1 });
}

export function stepTab(list: TabList, id: number, by: number): TabList {
  const tab = tabOf(list, id);
  if (!tab) return list;
  const at = tab.at + by;
  return at < 0 || at >= tab.entries.length ? list : put(list, { ...tab, at });
}

export const encodeTabs = (list: TabList): StoredTabs => ({ tabs: list.tabs.map(t => ({ id: t.id, href: hrefIn(t) })), active: list.active });

export function isStoredTabs(value: unknown): value is StoredTabs {
  const v = value as StoredTabs | null;
  if (typeof v !== "object" || v === null || !Array.isArray(v.tabs) || v.tabs.length === 0) return false;
  const ids = v.tabs.map(t => t?.id);
  return new Set(ids).size === ids.length && ids.includes(v.active)
    && v.tabs.every(t => Number.isInteger(t.id) && t.id > 0 && typeof t.href === "string" && t.href.startsWith("/") && !t.href.startsWith("//"));
}

/** The saved tabs come back; a deep link in the window's address opens in the tab that was active. */
export function startTabs(stored: StoredTabs | null, href: string): TabList {
  if (!stored) return firstTabs(href);
  const list: TabList = { tabs: stored.tabs.map(t => blank(t.id, t.href)), active: stored.active };
  return href === FRESH_HREF ? list : visitTab(list, list.active, href, false);
}

export function tabTitle(route: Route, names: TabNames): string {
  switch (route.name) {
    case "root": case "home": return "Home";
    case "inbox": return "Inbox";
    case "archive": return "Archive";
    case "new": return route.kind === "sprint" ? "New sprint" : "New task";
    case "settings": return "Settings";
    case "notfound": return "Not found";
    case "task": return names.task(route.task) ?? "Task";
    case "sprint": return names.sprint(route.sprint) ?? "Sprint";
    case "project": {
      const name = names.project(route.project) ?? "Project";
      return route.view === "code" ? `${name}: Code` : route.view === "git" || route.view === "worktrees" ? `${name}: Git` : name;
    }
  }
}
