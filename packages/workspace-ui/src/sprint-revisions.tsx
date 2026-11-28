import { createSignal, For, Show } from "solid-js";
import type { LiveSprintGraph, Sprint, SprintRevision, SprintRevisionChange } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { editNotice } from "./live-graph";
import { workspaceCopy } from "./copy";

/*
 * Revisions of a sprint's unstarted drafts that a sprint task proposed after the owner changed it. The service validates,
 * reviews and applies them (Automatic) or waits for the owner's Apply or Reject (Hybrid, Manual). Proposed cancellations are
 * only shown: nothing here cancels a task.
 */

type Titles = (taskId: string) => string;

/** The card heading: which task proposed the changes. */
export const revisionHeading = (r: SprintRevision, title: Titles, locale?: string) => workspaceCopy("sprintRevisionFrom", locale).replace("{task}", r.taskId ? title(r.taskId) : "?");

/** One change in words. */
export function changeWords(c: SprintRevisionChange, title: Titles): string {
  if (c.op === "add") return `New task: ${c.title ?? c.key ?? ""}`;
  const target = c.memberId ? title(c.memberId) : "?";
  if (c.op === "propose_cancel") return `Proposed for cancellation: ${target}${c.reason ? ` — ${c.reason}` : ""}`;
  const parts = [c.title ? "title" : "", c.description ? "description" : "", c.dependsOn ? "links" : ""].filter(Boolean);
  return `Change ${target}: ${parts.join(", ")}`;
}

const STATUS: Record<SprintRevision["status"], string> = {
  review: "In independent review", changes_requested: "Review asked for changes", approved: "Approved",
  awaiting_owner: "Waiting for your decision", applied: "Applied", rejected: "Rejected", apply_refused: "Could not be applied", superseded: "Replaced by a newer revision",
};
export const revisionStatus = (r: SprintRevision) => r.status === "awaiting_owner" && r.applied ? "Applied; the proposed cancellations wait for your decision" : STATUS[r.status];

/** "changed by task X" for a member an applied revision changed, or null. */
export function changedBy(graph: LiveSprintGraph | null, taskId: string, title: Titles, locale?: string): string | null {
  const r = [...(graph?.revisions ?? [])].reverse().find(x => x.applied && (x.updated.includes(taskId) || x.added.includes(taskId)));
  return r ? workspaceCopy(r.added.includes(taskId) ? "sprintRevisionAddedBy" : "sprintRevisionChangedBy", locale).replace("{task}", r.taskId ? title(r.taskId) : "?") : null;
}

/** What one change proposes, field by field: the current value beside the proposed one, so the owner sees the difference. */
function ChangeDiff(props: { change: SprintRevisionChange; title: Titles; current: (taskId: string) => { title: string; description: string } | null }) {
  const ws = useWs();
  const c = () => props.change;
  const was = () => c().memberId ? props.current(c().memberId!) : null;
  const links = (ids: string[]) => ids.length ? ids.map(id => props.title(id) === "Task not loaded" ? id : props.title(id)).join(", ") : "None";
  return <li class="sp-rev-change">
    <span class="w500">{changeWords(c(), props.title)}</span>
    <Show when={c().corrects}>{id => <div>{ws.t("sprintCorrectsTask")} {links([id()])}</div>}</Show>
    <Show when={c().op !== "propose_cancel"}>
      <dl class="sp-rev-diff">
        <Show when={c().title}>
          <dt class="cap muted">Title</dt>
          <dd><Show when={c().op === "update" && was()}><del class="sp-rev-old">{was()!.title}</del></Show><ins class="sp-rev-new">{c().title}</ins></dd>
        </Show>
        <Show when={c().description}>
          <dt class="cap muted">Description</dt>
          <dd><Show when={c().op === "update" && was()?.description}><del class="sp-rev-old">{was()!.description}</del></Show><ins class="sp-rev-new">{c().description}</ins></dd>
        </Show>
        <Show when={c().dependsOn}>{deps => <><dt class="cap muted">Depends on</dt><dd><ins class="sp-rev-new">{links(deps())}</ins></dd></>}</Show>
      </dl>
    </Show>
  </li>;
}

/** Revisions waiting for the owner, as cards with Apply and Reject; with `history`, every revision with its outcome. */
export function SprintRevisions(props: { sprint: Sprint; graph: LiveSprintGraph | null; history?: boolean }) {
  const ws = useWs();
  const [busy, setBusy] = createSignal(false);
  const [notice, setNotice] = createSignal<string | null>(null);
  const title: Titles = id => ws.store.index().byId.get(id)?.task.title ?? "Task not loaded";
  const current = (id: string) => { const task = ws.store.index().byId.get(id)?.task; return task ? { title: task.title, description: task.description } : null; };
  const shown = () => (props.graph?.revisions ?? []).filter(r => props.history ? r.status !== "superseded" : r.status === "awaiting_owner");
  const act = async (r: SprintRevision, apply: boolean) => {
    const g = props.graph;
    if (busy() || !g) return;
    setBusy(true); setNotice(null);
    try {
      const result = await ws.store.editLiveGraph(apply
        ? { action: "apply_revision", sprintId: props.sprint.id, revisionId: r.revisionId, expectedGraphRevision: g.graphRevision }
        : { action: "reject_revision", sprintId: props.sprint.id, revisionId: r.revisionId });
      setNotice(editNotice(result));
    } finally { setBusy(false); }
  };
  return <Show when={shown().length > 0}>
    <section class="sp-revisions" aria-label={props.history ? workspaceCopy("sprintRevisionHistory") : "Sprint changes"}>
      <Show when={props.history}><div class="w500">{workspaceCopy("sprintRevisionHistory")}</div></Show>
      <For each={[...shown()].reverse()}>{r => <div class="sp-revision" role="group" aria-label={revisionHeading(r, title)}>
        <div class="sp-rev-head"><span class="w500">{revisionHeading(r, title)}</span><span class="cap muted">{revisionStatus(r)}</span></div>
        <Show when={r.reason}><p class="sp-rev-reason">{r.reason}</p></Show>
        <Show when={r.startedAffected?.length}><div>{ws.t("sprintAffectedStarted")}<ul><For each={r.startedAffected}>{id => <li>{ws.store.index().byId.get(id)?.task.title ?? id}</li>}</For></ul></div></Show>
        <ul class="sp-rev-changes">{<For each={r.changes}>{c => <ChangeDiff change={c} title={title} current={current} />}</For>}</ul>
        <Show when={!props.history && r.status === "awaiting_owner"}>
          <div class="row gap sp-rev-actions">
            <Show when={!r.applied}><button class="btn btn-primary btn-sm" disabled={busy()} onClick={() => void act(r, true)}>Apply</button></Show>
            <button class="btn btn-line btn-sm" disabled={busy()} onClick={() => void act(r, false)}>{r.applied ? "Dismiss" : "Reject"}</button>
          </div>
          <Show when={r.changes.some(c => c.op === "propose_cancel")}><span class="cap faint">Nothing is cancelled for a proposal; cancel the task yourself if you agree.</span></Show>
        </Show>
      </div>}</For>
      <Show when={notice()}><div class="cap" role="status">{notice()}</div></Show>
    </section>
  </Show>;
}
