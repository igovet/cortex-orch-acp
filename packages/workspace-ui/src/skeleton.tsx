import { For, Show, type JSX } from "solid-js";
import type { Project } from "@cortex/workspace-domain";
import { useWs, type Ws } from "./ctx";
import { createFirstView, type FirstView } from "./first-view";

// The icon markup of kit.tsx's `I`, here directly: kit's own loading fallback uses this module.
const Icon = (props: { n: string; spin?: boolean }) => <i class={`i s14 i-${props.n}${props.spin ? " spin" : ""}`} aria-hidden="true" />;

/** The machine may not be reached at all: a first read then fails at once instead of waiting out its budget. */
export const unreachable = (ws: Ws) => ws.store.mode === "real" && ["disconnected", "unavailable"].includes(ws.store.connection().state);
/** Whether some project a list shows has not had its tasks read in this window yet (and has no saved board standing in for
 *  them): until it has, an empty list says nothing about it. */
export const projectsPending = (ws: Ws, shown: (p: Project) => boolean = () => true) => ws.store.mode === "real"
  && ws.store.state().projects.some(p => shown(p) && p.activated !== false && !ws.store.readProjects().has(p.id) && !ws.store.savedView(p));

/** The outline each kind of view shows while its first read is under way. */
export type SkeletonKind = "list" | "conversation" | "activity" | "panel";
const SHAPES: Record<SkeletonKind, readonly string[]> = {
  list: ["row", "row", "row", "row", "row"],
  conversation: ["mine", "ai", "ai-short", "fold", "ai", "mine", "ai-short"],
  activity: ["note", "step", "step", "note-short", "step", "note", "step"],
  panel: ["head", "line", "line", "line-short", "line", "line-short"],
};

/**
 * One loading primitive for every view whose first read has not settled: one status line in words, and below it quiet,
 * inert shapes of what is coming, never a made-up row, count or action. It takes the room of the pane it stands in (a
 * desktop pane keeps its own space, the phone's single pane fills its width) and holds still under reduced motion. The
 * region it stands in carries `aria-busy` while it shows.
 */
export function Skeleton(props: { kind: SkeletonKind; label: string; class?: string }) {
  return <div class={`ws-skel ws-skel-${props.kind}${props.class ? ` ${props.class}` : ""}`} data-follow-skip="">
    <span class="ws-skel-status" role="status">{props.label}</span>
    <div class="ws-skel-rows shimmer" aria-hidden="true">
      <For each={SHAPES[props.kind]}>{shape => <div class={`ws-skel-row ws-skel-${shape}`} />}</For>
    </div>
  </div>;
}

/**
 * A list's empty state (board, Inbox, Archive, Home) while its projects' first reads are under way: the list skeleton instead
 * of "nothing here", for at most the first-view budget; then the empty state with the notice and Retry.
 */
export function EmptyUntilRead(props: { name: string; pending: () => boolean; label: string; kind?: SkeletonKind; children: JSX.Element }) {
  const ws = useWs();
  const first = createFirstView({ scope: ws.store, identity: () => `list|${props.name}`, offline: () => unreachable(ws),
    reads: () => [{ section: "every project", state: props.pending() ? "pending" : "settled" }], retry: () => void ws.store.reload() });
  return <Show when={!first.loading()} fallback={<div class="ws-skel-pane" aria-busy="true"><Skeleton kind={props.kind ?? "list"} label={props.label} /></div>}>
    <FirstViewNotice view={first} />{props.children}
  </Show>;
}

/** "the conversation, questions and workers", for the notice. */
const listed = (sections: string[]) => sections.length <= 1 ? sections.join("") : `${sections.slice(0, -1).join(", ")} and ${sections.at(-1)}`;

/**
 * What a view says once its first read ran out of time (or the machine went offline) before every part of it arrived: which
 * parts are not shown, that what is shown is what was confirmed, and Retry. It is never a ready view: it stays until those
 * parts arrive, and says so while Retry reads them again.
 */
export function FirstViewNotice(props: { view: FirstView; class?: string }) {
  const missing = () => props.view.missing();
  return <Show when={missing().length > 0 || props.view.retrying()}>
    <div class={`answered ws-first-notice${props.class ? ` ${props.class}` : ""}`} role={props.view.retrying() ? "status" : "alert"}>
      <Icon n={props.view.retrying() ? "loader" : "alert"} spin={props.view.retrying()} />
      <span class="grow">{props.view.retrying()
        ? `Loading ${listed(props.view.pending().map(r => r.section))} again…`
        : `Couldn’t finish loading ${listed(missing())}. What is shown was confirmed; the rest is not shown yet.`}</span>
      <Show when={!props.view.retrying()}><button type="button" class="lnk" onClick={() => props.view.retry()}>Retry</button></Show>
    </div>
  </Show>;
}
