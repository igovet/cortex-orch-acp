import { createEffect, createMemo, createSignal, For, onCleanup, Show, untrack, type Accessor } from "solid-js";
import { useWs } from "./ctx";
import { onScreen } from "./freshness";
import { Strip } from "./layout";
import { NOTIFICATION_KINDS, openPlace, parseTarget, reveal, targetPlace, type Notification, type NotificationClicks, type NotificationKind, type NotificationReport, type NotificationService } from "./notification-target";
import { Srow, Sw } from "./settings-kit";
import { persisted } from "./theme";
import type { Tabs } from "./workspace-tabs";
export type { Notification, NotificationClicks, NotificationService, NotificationTarget, Notify } from "./notification-target";

const ALL_KINDS: NotificationKind[] = NOTIFICATION_KINDS.map(k => k.id);
let kindsPref: [Accessor<NotificationKind[]>, (kinds: NotificationKind[]) => void] | undefined;
/** The kinds the owner wants announced on this computer; all of them until the owner turns one off. */
export const notificationKinds = () => kindsPref ??= persisted<NotificationKind[]>("cw.ws.notificationKinds", ALL_KINDS,
  v => Array.isArray(v) && v.every(k => ALL_KINDS.includes(k as NotificationKind)));

/** One switch per kind, under the Notifications switch. */
export function NotificationKinds(props: { disabled?: string }) {
  const [kinds, setKinds] = notificationKinds();
  return <For each={NOTIFICATION_KINDS}>{k =>
    <Srow t={k.label}><Sw label={k.label} checked={kinds().includes(k.id)} disabled={props.disabled}
      onChange={on => setKinds(on ? ALL_KINDS.filter(x => x === k.id || kinds().includes(x)) : kinds().filter(x => x !== k.id))} /></Srow>
  }</For>;
}

/** Where this app keeps the questions it has already seen, so a relaunch does not announce them again. */
export const SEEN_QUESTIONS_KEY = "cw.notified-questions";
/** The newest ids kept; an older question is long answered or long announced. */
const SEEN_LIMIT = 500;
const storage = () => { try { return typeof localStorage === "undefined" ? undefined : localStorage; } catch { return undefined; } };
function readSeen(): string[] {
  try {
    const saved: unknown = JSON.parse(storage()?.getItem(SEEN_QUESTIONS_KEY) ?? "[]");
    return Array.isArray(saved) ? saved.filter((id): id is string => typeof id === "string").slice(-SEEN_LIMIT) : [];
  } catch { return []; }
}
function writeSeen(ids: Set<string>) {
  try { storage()?.setItem(SEEN_QUESTIONS_KEY, JSON.stringify([...ids].slice(-SEEN_LIMIT))); } catch { /* unavailable storage: this run still dedupes */ }
}

/**
 * Observe exact pending question identities, not polling cycles or lifecycle guesses. The identities already seen are kept in
 * this app's local storage: after a relaunch a question that was announced (or seen open) before stays quiet, while one that
 * arrived while the app was closed is announced once.
 *
 * Each notification names its place (the task's Conversation at that question, or the sprint's planning). A click on it opens
 * that place as opening it by hand would: a tab already showing it comes forward instead of a second one, and a place that is
 * gone or a question already answered opens the nearest valid place (the task, else the project's board) without a dialog.
 */
export function QuestionNotifications(props: { tabs?: Pick<Tabs, "ids" | "activeId" | "routerOf" | "select">; clicks?: NotificationClicks; service?: NotificationService }) {
  const ws = useWs(), seen = new Set<string>(readSeen());
  const [error, setError] = createSignal("");
  const [kinds] = notificationKinds();
  // Whether the machine service raises the notifications. Until it has answered the window raises none of its own, so a
  // question is never announced twice; a service without them leaves this window's own question notifications.
  const [serviceRaises, setServiceRaises] = createSignal<boolean | null>(props.service ? null : false);
  const open = (raw: unknown) => {
    const target = parseTarget(raw);
    if (!target) return;
    const place = untrack(() => targetPlace(ws.store.state(), target, ws.machine().id));
    if (props.tabs) openPlace(props.tabs, route => ws.go(route), place.route); else ws.go(place.route);
    if (place.reveal) reveal(place.reveal, onScreen);
  };
  const stop = props.clicks?.(open);
  onCleanup(() => stop?.());
  const service = props.service;
  if (service) {
    // A window the service started from a click opens that click's place.
    void service.take().then(reply => { if (reply?.target) open(reply.target); }, () => {});
    const [focused, setFocused] = createSignal(!document.hidden && (document.hasFocus?.() ?? true));
    const sync = () => setFocused(!document.hidden && (document.hasFocus?.() ?? true));
    window.addEventListener("focus", sync); window.addEventListener("blur", sync); document.addEventListener("visibilitychange", sync);
    onCleanup(() => { window.removeEventListener("focus", sync); window.removeEventListener("blur", sync); document.removeEventListener("visibilitychange", sync); });
    const report = createMemo((): NotificationReport => {
      const r = ws.router.route(), on = ws.panels.notifications();
      const task = r.name === "task" ? ws.store.state().tasks.find(t => t.id === r.task) : undefined;
      const place = !focused() ? null : task ? { project: task.projectId, task: task.id }
        : r.name === "sprint" ? { project: r.project, sprint: r.sprint }
        : r.name === "project" && r.scope.kind === "sprint" ? { project: r.project, sprint: r.scope.id } : null;
      return { focused: place, kinds: on ? kinds() : [] };
    }, undefined, { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
    const send = (value: NotificationReport) => void service.report(value)
      // A refused report (a service without the method) leaves the window's own notifications; a lost one changes nothing.
      .then(reply => setServiceRaises(reply?.raising === true), () => setServiceRaises(v => v ?? false));
    createEffect(() => send(report()));
    // What the focused window shows holds at the service for 90 seconds; it is renewed while the window keeps focus.
    createEffect(() => {
      if (!focused()) return;
      const timer = setInterval(() => send(untrack(report)), 60_000);
      onCleanup(() => clearInterval(timer));
    });
  }
  createEffect(() => {
    const state = ws.store.state(), route = ws.router.route(), enabled = ws.panels.notifications() && serviceRaises() === false;
    const wanted = new Set(kinds());
    if (ws.store.mode !== "real") return;
    const raised: Notification[] = [];
    let changed = false;
    for (const session of state.sessions) {
      const task = state.tasks.find(t => t.id === session.taskId);
      const sprint = state.sprints.find(s => !s.archivedAt && s.planningTaskId === session.taskId);
      if (!task && !sprint) continue;
      const machineId = state.projects.find(p => p.id === (task?.projectId ?? sprint?.projectId))?.machineId;
      if (!state.machines.some(m => m.id === machineId && m.connection === "connected")) continue;
      const open = route.name === "task" && route.task === session.taskId
        || route.name === "sprint" && route.sprint === sprint?.id;
      // The first new question is the one the notification opens at.
      let fresh: string | null = null;
      for (const d of session.decisions) {
        if (d.answer || d.withdrawn || d.suspended || seen.has(d.id)) continue;
        seen.add(d.id); fresh ??= d.id; changed = true;
      }
      if (fresh && (!open || document.hidden) && enabled && ws.notify && wanted.has(task ? "question" : "planning"))
        raised.push(task
          ? { label: task.title, target: { kind: "question", project: task.projectId, task: task.id, decision: fresh } }
          : { label: sprint!.title ?? "Sprint planning", target: { kind: "planning", project: sprint!.projectId, sprint: sprint!.id, decision: fresh } });
    }
    if (changed) writeSeen(seen);
    for (const notification of raised) void ws.notify!(notification).catch(() =>
      setError("Desktop notification could not be delivered. Questions remain in Inbox."));
  });
  return <Show when={error()}><Strip class="ws-conn" role="alert" tone="warn" action={<button class="btn btn-sm" onClick={() => setError("")}>Dismiss</button>}>{error()}</Strip></Show>;
}
