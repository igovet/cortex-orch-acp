// @vitest-environment jsdom
import { createRoot, createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, emptySession, type WorkspaceState } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { QuestionNotifications } from "../src/notifications";
import { openPlace, parseTarget, targetPlace, type NotificationTarget } from "../src/notification-target";
import { hrefOf, type Route } from "../src/router";
import { createTabs } from "../src/workspace-tabs";
import { draftTask, flush } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { vi.restoreAllMocks(); dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); history.replaceState(null, "", "/"); });

function world(): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  s.projects = [{ id: "p", name: "P", machineId: s.machines[0].id, repository: "/p", openBranch: "main", defaultBranch: "main" }];
  s.tasks = [draftTask()];
  const session = emptySession("t");
  session.decisions = [{ id: "q", kind: "question", from: "worker", workerId: null, at: 1, text: "Which?", options: [], answer: null, withdrawn: false }];
  const planning = emptySession("plan-task");
  planning.decisions = [{ id: "pq", kind: "question", from: "worker", workerId: null, at: 1, text: "Scope?", options: [], answer: null, withdrawn: false }];
  s.sessions = [session, planning];
  s.sprints = [{ id: "s", projectId: "p", description: "Plan", title: "Sprint work", planningTaskId: "plan-task", runMode: "manual", targetBranch: null, planning: "question", conversation: [], plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null }];
  return s;
}
const question: NotificationTarget = { kind: "question", project: "p", task: "t", decision: "q" };
const planning: NotificationTarget = { kind: "planning", project: "p", sprint: "s", decision: "pq" };

it("each notification kind opens its own place, at its question while it waits", () => {
  const s = world(), m = s.machines[0].id;
  expect(targetPlace(s, question, m)).toEqual({ route: { name: "task", machine: m, task: "t" }, reveal: "[id=\"decision-q\"]" });
  expect(targetPlace(s, planning, m)).toEqual({ route: { name: "sprint", machine: m, project: "p", sprint: "s", tab: "planning" }, reveal: "[id=\"decision-pq\"]" });
});

it("a stale target opens the nearest valid place: the task, else the project's board, else Home", () => {
  const s = world(), m = s.machines[0].id;
  s.sessions[0].decisions[0].answer = { text: "A", at: 2 };
  expect(targetPlace(s, question, m)).toEqual({ route: { name: "task", machine: m, task: "t" }, reveal: null });
  const board: Route = { name: "project", machine: m, project: "p", view: "board", scope: { kind: "all" } };
  expect(targetPlace({ ...s, tasks: [] }, question, m)).toEqual({ route: board, reveal: null });
  expect(targetPlace({ ...s, sprints: [{ ...s.sprints[0], archivedAt: 5 }] }, planning, m)).toEqual({ route: board, reveal: null });
  expect(targetPlace({ ...s, tasks: [], projects: [] }, question, m)).toEqual({ route: { name: "home", machine: m }, reveal: null });
});

it("only the exact target shape is accepted from a click", () => {
  expect(parseTarget(question)).toEqual(question);
  expect(parseTarget(planning)).toEqual(planning);
  for (const bad of [null, "q", { ...question, href: "/x" }, { ...question, task: "" }, { ...question, task: "a b" }, { kind: "planning", project: "p", task: "t", decision: "q" }, { kind: "url", href: "/" }])
    expect(parseTarget(bad)).toBeNull();
});

function tabsAt(...hrefs: string[]) {
  return createRoot(stop => {
    const tabs = createTabs();
    tabs.routerOf(tabs.activeId()).go(hrefs[0]);
    for (const href of hrefs.slice(1)) { tabs.add(); tabs.routerOf(tabs.activeId()).go(href); }
    tabs.select(tabs.ids()[0]);
    return { tabs, stop };
  });
}

it("focuses a tab that already shows the place instead of opening a duplicate, and moves it to the place", () => {
  const route: Route = { name: "task", machine: "local", task: "t" };
  const { tabs, stop } = tabsAt("/m/local/inbox", "/m/local/p/p/board", "/m/local/t/t/changes");
  const go = vi.fn();
  const shown = openPlace(tabs, go, route);
  expect(go).not.toHaveBeenCalled();
  expect(shown).toBe(tabs.ids()[2]);
  expect(tabs.activeId()).toBe(tabs.ids()[2]);
  expect(tabs.routerOf(shown).path()).toBe(hrefOf(route));
  expect(tabs.ids()).toHaveLength(3);
  stop();
});

it("prefers a tab at the exact place, and otherwise opens the place in the active tab through its guarded go", () => {
  const route: Route = { name: "task", machine: "local", task: "t" };
  const { tabs, stop } = tabsAt("/m/local/inbox", "/m/local/t/t/changes", "/m/local/t/t");
  expect(openPlace(tabs, vi.fn(), route)).toBe(tabs.ids()[2]);
  tabs.select(tabs.ids()[0]);
  const go = vi.fn();
  expect(openPlace(tabs, go, { name: "task", machine: "local", task: "other" })).toBe(tabs.ids()[0]);
  expect(go).toHaveBeenCalledWith({ name: "task", machine: "local", task: "other" });
  expect(tabs.activeId()).toBe(tabs.ids()[0]);
  stop();
});

it("a click opens the notification's place in the window and brings its question into view", async () => {
  const s = world(), [state] = createSignal(s);
  let click: ((raw: unknown) => void) | undefined;
  const stopped = vi.fn();
  const clicks = (listener: (raw: unknown) => void) => { click = listener; return stopped; };
  const { tabs, stop } = tabsAt("/m/local/inbox");
  const go = vi.fn((to: Route) => tabs.routerOf(tabs.activeId()).go(to));
  const ws = { store: { mode: "real", state }, router: tabs.active, panels: { notifications: () => true }, machine: () => s.machines[0], go, notify: vi.fn(async () => {}) } as unknown as Ws;
  const root = document.createElement("div"); document.body.append(root);
  const card = document.createElement("div"); card.id = "decision-q"; card.scrollIntoView = vi.fn(); document.body.append(card);
  const unmount = render(() => <WsContext.Provider value={ws}><QuestionNotifications tabs={tabs} clicks={clicks} /></WsContext.Provider>, root);
  dispose = () => { unmount(); stop(); };
  click!({ kind: "url", href: "https://example.invalid" });
  expect(go).not.toHaveBeenCalled();
  click!(question);
  expect(tabs.routerOf(tabs.activeId()).path()).toBe(`/m/${s.machines[0].id}/t/t`);
  await flush();
  expect(card.scrollIntoView).toHaveBeenCalledWith({ block: "center" });
  unmount();
  expect(stopped).toHaveBeenCalled();
  dispose = stop;
});

it("a result, a failed task and a paused sprint open their own places", () => {
  const s = world(), m = s.machines[0].id;
  expect(targetPlace(s, { kind: "result", project: "p", task: "t" }, m)).toEqual({ route: { name: "task", machine: m, task: "t" }, reveal: ".rcard" });
  expect(targetPlace(s, { kind: "task", project: "p", task: "t" }, m)).toEqual({ route: { name: "task", machine: m, task: "t" }, reveal: null });
  expect(targetPlace(s, { kind: "sprint", project: "p", sprint: "s" }, m).route).toEqual({ name: "sprint", machine: m, project: "p", sprint: "s", tab: "planning" });
  const board: Route = { name: "project", machine: m, project: "p", view: "board", scope: { kind: "all" } };
  expect(targetPlace({ ...s, tasks: [] }, { kind: "result", project: "p", task: "t" }, m)).toEqual({ route: board, reveal: null });
  for (const ok of [{ kind: "result", project: "p", task: "t" }, { kind: "task", project: "p", task: "t" }, { kind: "sprint", project: "p", sprint: "s" }])
    expect(parseTarget(ok)).toEqual(ok);
  for (const bad of [{ kind: "result", project: "p", task: "t", decision: "d" }, { kind: "sprint", project: "p", task: "t" }, { kind: "task", project: "p" }])
    expect(parseTarget(bad)).toBeNull();
});

function serviceWindow(opts: { raising: boolean | "refused"; pending?: unknown }) {
  // jsdom never has focus by itself; this window is the focused one.
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  const s = world(), [state, setState] = createSignal(s);
  const { tabs, stop } = tabsAt("/m/local/inbox");
  const go = vi.fn((to: Route) => tabs.routerOf(tabs.activeId()).go(to));
  const notify = vi.fn(async () => {});
  const reports: unknown[] = [];
  const service = {
    report: vi.fn(async (r: unknown) => { reports.push(r); if (opts.raising === "refused") throw new Error("unsupported_method"); return { raising: opts.raising }; }),
    take: vi.fn(async () => ({ target: opts.pending ?? null })),
  };
  const ws = { store: { mode: "real", state }, router: tabs.active, panels: { notifications: () => true }, machine: () => s.machines[0], go, notify } as unknown as Ws;
  const root = document.createElement("div"); document.body.append(root);
  const card = document.createElement("div"); card.className = "rcard"; card.scrollIntoView = vi.fn(); document.body.append(card);
  const unmount = render(() => <WsContext.Provider value={ws}><QuestionNotifications tabs={tabs} service={service} /></WsContext.Provider>, root);
  dispose = () => { unmount(); stop(); };
  return { s, setState, tabs, notify, reports, service, card };
}

it("a window the service started opens the click's place, and the service, not the window, raises notifications", async () => {
  const h = serviceWindow({ raising: true, pending: { kind: "result", project: "p", task: "t" } });
  await flush(); await flush();
  expect(h.service.take).toHaveBeenCalledTimes(1);
  expect(h.tabs.routerOf(h.tabs.activeId()).path()).toBe(`/m/${h.s.machines[0].id}/t/t`);
  expect(h.card.scrollIntoView).toHaveBeenCalledWith({ block: "center" });
  // The open question is the service's to announce; the window stays quiet.
  expect(h.notify).not.toHaveBeenCalled();
  // It reports the task on view in the focused window, with every kind on.
  expect(h.reports.at(-1)).toEqual({ focused: { project: "p", task: "t" }, kinds: ["question", "stop", "result", "failed", "planning", "paused"] });
});

it("a service that does not raise notifications leaves the window's own question notifications", async () => {
  const h = serviceWindow({ raising: "refused" });
  await flush(); await flush();
  h.setState(s => ({ ...s, sessions: s.sessions.map(x => ({ ...x, decisions: x.decisions.map(d => ({ ...d, id: `${d.id}-new` })) })) }));
  await flush();
  expect(h.notify).toHaveBeenCalledWith({ label: "Fix the race", target: { kind: "question", project: "p", task: "t", decision: "q-new" } });
  expect(h.reports[0]).toEqual({ focused: null, kinds: ["question", "stop", "result", "failed", "planning", "paused"] });
});
