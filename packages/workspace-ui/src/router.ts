export const PROJECT_VIEWS = ["board", "list", "code", "git", "worktrees", "knowledge", "settings"] as const;
export type ProjectView = typeof PROJECT_VIEWS[number];
export const APP_SECTIONS = ["appearance", "voice", "updates", "shortcuts", "machines"] as const;
export const MACHINE_SECTIONS = ["overview", "orchestration", "connections", "worktrees", "storage", "diagnostics", "remote"] as const;
export type SettingsSection = typeof APP_SECTIONS[number] | typeof MACHINE_SECTIONS[number];
export const ORCHESTRATION_PAGES = ["defaults", "models", "routing", "effective", "policy", "tools", "profiles", "files"] as const;
export type OrchestrationPage = typeof ORCHESTRATION_PAGES[number];
/** `planning` is the planning chat, `plan` the plan's List and Graph; each has the whole width. */
export const SPRINT_TABS = ["planning", "plan", "summary"] as const;
export type SprintTab = typeof SPRINT_TABS[number];
export type Scope = { kind: "all" } | { kind: "standalone" } | { kind: "sprint"; id: string };
export const TASK_TABS = ["workers", "changes", "code", "terminal"] as const;
export type TaskTab = typeof TASK_TABS[number];
/** A place in the code: a file, and the line to land on. */
export type CodeSpot = { file?: string; line?: number };

export type Route =
  | { name: "root" }
  | { name: "home"; machine: string }
  | { name: "inbox"; machine: string }
  | { name: "archive"; machine: string }
  | { name: "new"; machine: string; kind: "task" | "sprint"; project: string | null; sprint?: string }
  | ({ name: "project"; machine: string; project: string; view: ProjectView | null; scope: Scope; checkout?: string } & CodeSpot)
  | { name: "sprint"; machine: string; project: string; sprint: string; tab: SprintTab }
  | ({ name: "task"; machine: string; task: string; tab?: TaskTab; worker?: string; live?: true; sub?: "diff" | "conflict" } & CodeSpot)
  | { name: "settings"; section: SettingsSection; machine: string | null; page?: OrchestrationPage; server?: string; tab?: string }
  | { name: "notfound"; path: string };

const has = <T extends string>(list: readonly T[], value: string | undefined): value is T => (list as readonly (string | undefined)[]).includes(value);

/** Every view is a real History API route, so Back, Forward and deep links always work. */
export function parseRoute(pathname: string, search = ""): Route {
  const parts = pathname.replace(/\/+$/, "").split("/").filter(Boolean).map(decodeURIComponent);
  const query = new URLSearchParams(search);
  const notFound: Route = { name: "notfound", path: pathname };
  const file = query.get("file") ?? undefined, lineText = query.get("line");
  const spot: CodeSpot = { ...(file ? { file } : {}), ...(lineText && /^\d+$/.test(lineText) ? { line: Number(lineText) } : {}) };
  if (parts.length === 0) return { name: "root" };
  if (parts[0] === "settings") {
    if (parts[1] === "app" && parts.length === 3 && has(APP_SECTIONS, parts[2])) return { name: "settings", section: parts[2], machine: null };
    if (parts[1] === "machine" && parts[2] && parts.length === 4 && has(MACHINE_SECTIONS, parts[3])) return { name: "settings", section: parts[3], machine: parts[2] };
    const tab = query.get("tab") ?? undefined, tabbed = tab ? { tab } : {};
    if (parts[1] === "machine" && parts[2] && parts.length === 5 && parts[3] === "orchestration" && has(ORCHESTRATION_PAGES, parts[4])) return { name: "settings", section: "orchestration", machine: parts[2], page: parts[4], ...tabbed };
    if (parts[1] === "machine" && parts[2] && parts.length === 6 && parts[3] === "orchestration" && parts[4] === "tools") return { name: "settings", section: "orchestration", machine: parts[2], page: "tools", server: parts[5], ...tabbed };
    return notFound;
  }
  if (parts[0] !== "m" || !parts[1]) return notFound;
  const machine = parts[1], rest = parts.slice(2);
  if (rest.length === 0) return { name: "home", machine };
  if (rest.length === 1 && rest[0] === "inbox") return { name: "inbox", machine };
  if (rest.length === 1 && rest[0] === "archive") return { name: "archive", machine };
  if (rest.length === 1 && rest[0] === "new") {
    const kind = query.get("kind") === "sprint" ? "sprint" : "task", sprint = query.get("sprint");
    return { name: "new", machine, kind, project: query.get("project"), ...(sprint && kind === "task" ? { sprint } : {}) };
  }
  if (rest[0] === "t" && rest[1]) {
    const task = rest[1], tail = rest.slice(2);
    if (tail.length === 0) return { name: "task", machine, task };
    if (tail.length === 1 && has(TASK_TABS, tail[0])) return { name: "task", machine, task, tab: tail[0], ...(tail[0] === "code" ? spot : {}) };
    if (tail[0] === "changes" && tail.length === 2 && (tail[1] === "diff" || tail[1] === "conflict")) return { name: "task", machine, task, tab: "changes", sub: tail[1] };
    if (tail[0] === "workers" && tail[1] && tail.length === 2) return { name: "task", machine, task, tab: "workers", worker: tail[1] };
    if (tail[0] === "workers" && tail[1] && tail[2] === "live" && tail.length === 3) return { name: "task", machine, task, tab: "workers", worker: tail[1], live: true };
    return notFound;
  }
  if (rest[0] === "p" && rest[1]) {
    const project = rest[1], tail = rest.slice(2);
    if (tail[0] === "s" && tail[1] && tail.length <= 3) {
      const view = tail[2] ?? "board";
      if (has(SPRINT_TABS, view)) return { name: "sprint", machine, project, sprint: tail[1], tab: view };
      return view === "board" || view === "list" ? { name: "project", machine, project, view, scope: { kind: "sprint", id: tail[1] } } : notFound;
    }
    const scope: Scope = query.get("scope") === "standalone" ? { kind: "standalone" } : { kind: "all" };
    if (tail.length === 0) return { name: "project", machine, project, view: null, scope };
    const checkout = query.get("checkout") ?? undefined;
    if (tail.length === 1 && has(PROJECT_VIEWS, tail[0])) return { name: "project", machine, project, view: tail[0], scope, ...(checkout ? { checkout } : {}), ...(tail[0] === "code" ? spot : {}) };
  }
  return notFound;
}

function spotQuery(spot: CodeSpot, q = new URLSearchParams()) {
  if (spot.file) q.set("file", spot.file);
  if (spot.line !== undefined) q.set("line", String(spot.line));
  return q.size ? `?${q}` : "";
}
export function hrefOf(route: Route): string {
  const e = encodeURIComponent;
  switch (route.name) {
    case "root": return "/";
    case "home": return `/m/${e(route.machine)}`;
    case "inbox": return `/m/${e(route.machine)}/inbox`;
    case "archive": return `/m/${e(route.machine)}/archive`;
    case "new": {
      const q = new URLSearchParams();
      if (route.kind === "sprint") q.set("kind", "sprint");
      if (route.project) q.set("project", route.project);
      if (route.sprint && route.kind === "task") q.set("sprint", route.sprint);
      return `/m/${e(route.machine)}/new${q.size ? `?${q}` : ""}`;
    }
    case "task": {
      const base = `/m/${e(route.machine)}/t/${e(route.task)}`;
      if (route.worker) return `${base}/workers/${e(route.worker)}${route.live ? "/live" : ""}`;
      if (route.tab === "changes" && route.sub) return `${base}/changes/${route.sub}`;
      return route.tab ? `${base}/${route.tab}${spotQuery(route)}` : base;
    }
    case "project": {
      const base = `/m/${e(route.machine)}/p/${e(route.project)}`;
      if (route.scope.kind === "sprint") return `${base}/s/${e(route.scope.id)}/${route.view ?? "board"}`;
      const q = new URLSearchParams();
      if (route.scope.kind === "standalone") q.set("scope", "standalone");
      if (route.checkout) q.set("checkout", route.checkout);
      return `${base}${route.view ? `/${route.view}` : ""}${spotQuery(route, q)}`;
    }
    case "sprint": return `/m/${e(route.machine)}/p/${e(route.project)}/s/${e(route.sprint)}/${route.tab}`;
    case "settings": return route.machine ? `/settings/machine/${e(route.machine)}/${route.section}${route.page ? `/${route.page}` : ""}${route.server ? `/${e(route.server)}` : ""}${route.tab ? `?tab=${e(route.tab)}` : ""}` : `/settings/app/${route.section}`;
    case "notfound": return route.path;
  }
}

/** A tab's place: its route and its position in its own Back/Forward history. */
export type Router = {
  path: () => string; search: () => string; route: () => Route;
  canBack: () => boolean; canForward: () => boolean; back: () => void; forward: () => void;
  go: (to: string | Route, opts?: { replace?: boolean }) => void;
};
