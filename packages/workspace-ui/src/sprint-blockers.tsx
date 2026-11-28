import type { Sprint } from "@cortex/workspace-domain";
import { createMemo, createSignal, For, Show } from "solid-js";
import type { CopyKey } from "./copy";
import { useWs } from "./ctx";
import { A, I, Keyed, Modal } from "./kit";
import { decisionRoute, type TaskView } from "./selectors";
import { sprintBlockers } from "./sprint-blockers-model";
import "./sprint-blockers.css";

type BlockerStatus = Extract<CopyKey, `sprintBlockersTask${string}`>;
function status(view: TaskView): BlockerStatus {
  if (view.task.archive) return "sprintBlockersTaskUnsatisfied";
  if (view.liveDecision || view.task.status === "needs_you" || view.task.conflict !== "none") return "sprintBlockersTaskDecision";
  switch (view.task.status) {
    case "paused": return "sprintBlockersTaskPaused";
    case "working": return "sprintBlockersTaskWorking";
    case "draft": return "sprintBlockersTaskDraft";
    case "review": return "sprintBlockersTaskReview";
    case "ready_to_merge": return "sprintBlockersTaskIntegration";
    case "stopping": return "sprintBlockersTaskStopping";
    case "failed": case "cancelled": return "sprintBlockersTaskStopped";
    default: return "sprintBlockersTaskUnsatisfied";
  }
}

export function SprintBlockers(props: { sprint: Sprint }) {
  const ws = useWs();
  const t = ws.t;
  const [open, setOpen] = createSignal(false);
  const setClosed = () => setOpen(false);
  const model = createMemo(() => open() ? sprintBlockers(ws.store.index(), props.sprint, ws.store.liveGraph(props.sprint.id)) : null);
  const complete = () => { const m = model(); return m?.kind === "ready" ? m : null; };
  const issue = () => {
    switch (model()?.kind) {
      case "offline": return t("sprintBlockersOffline");
      case "incomplete": return t("sprintBlockersIncomplete");
      case "cycle": return t("sprintBlockersCycle");
      default: return null;
    }
  };
  const linkView = (id: string) => {
    const v = ws.store.index().byId.get(id);
    return v && !v.unknown && v.project.id === props.sprint.projectId && v.sprint?.id === props.sprint.id ? v : null;
  };
  const link = (id: string) => <Show when={linkView(id)}>{v => <A class="sprint-blockers-link" to={decisionRoute(v())}
    onClick={e => { if (e.button === 0 && !e.ctrlKey && !e.shiftKey && !e.metaKey && !e.altKey) setClosed(); }}><span>{v().task.title}</span><I n="arrow-right" s={12} /></A>}</Show>;
  const holds = () => model()?.kind === "offline" ? null : props.sprint.automaticQueue ?? null;
  return <>
    <button class="btn btn-line btn-sm sprint-blockers-trigger" type="button" aria-haspopup="dialog"
      aria-expanded={open()} aria-label={t("sprintBlockers")} data-tooltip="" onClick={() => setOpen(true)}>
      <I n="hourglass" s={14} /><span>{t("sprintBlockers")}</span>
    </button>
    <Modal class="sprint-blockers-dialog" label={t("sprintBlockers")} open={open()} onClose={setClosed}>
      <div class="dlg-h sprint-blockers-head"><h2>{t("sprintBlockers")}</h2>
        <button class="btn btn-sm" type="button" data-autofocus onClick={setClosed}>{t("sprintBlockersClose")}</button></div>
      <div class="dlg-b sprint-blockers-body">
        <Show when={issue()}>{reason => <p role="status" class="sprint-blockers-note">{reason()}</p>}</Show>
        <Show when={model()?.kind !== "offline" && props.sprint.queueState === "paused"}>
          <p class="sprint-blockers-note">{t("sprintBlockersQueuePaused")}</p>
        </Show>
        <Show when={holds()}>{hold => <section class="sprint-blockers-note">
          <strong>{t("sprintBlockersQueueHold")}</strong>
          <p>{hold().reason}</p>
          <Show when={hold().taskId}>{id => link(id())}</Show>
        </section>}</Show>
        <Show when={complete()}>{m => <>
          <dl class="sprint-blockers-stats">
            <div><dt>{t("sprintBlockersBlocked")}</dt><dd>{m().blocked}</dd></div>
            <div><dt>{t("sprintBlockersClear")}</dt><dd>{m().clear}</dd></div>
          </dl>
          <For each={[...m().outputs].filter(([id, reason]) => !m().blockers.some(b => b.view.id === id) && (holds()?.taskId !== id || holds()?.reason !== reason))}>{([id, reason]) =>
            <div>{link(id)}<p class="sprint-blockers-note">{reason}</p></div>
          }</For>
          <Show when={m().blockers.length} fallback={<Show when={m().blocked === 0}><p class="sprint-blockers-note">{t("sprintBlockersQuiet")}</p></Show>}>
            <Show when={m().blockers.length > 1}><p class="sprint-blockers-note">{t("sprintBlockersOverlap")}</p></Show>
            <ol class="sprint-blockers-ranked">
              <Keyed each={m().blockers} by={b => b.view.id}>{(blocker, at) => <li class="sprint-blockers-row">
                <h3>{link(blocker().view.id)}</h3>
                <p class="cap st-warn">{m().outputs.get(blocker().view.id) ?? t(status(blocker().view))} · {t("sprintBlockersAffected")} {blocker().immediate.length + blocker().later.length}</p>
                <Show when={blocker().view.task.status === "paused"}><p class="sprint-blockers-note">{t("sprintBlockersPauseNote")}</p></Show>
                <Show when={blocker().immediate.length}><details open={at() === 0}>
                  <summary>{t("sprintBlockersImmediately")} <span class="sprint-blockers-count">{blocker().immediate.length}</span></summary>
                  <ul><For each={blocker().immediate}>{id => <li>{link(id)}</li>}</For></ul>
                </details></Show>
                <Show when={blocker().later.length}><details>
                  <summary>{t("sprintBlockersLater")} <span class="sprint-blockers-count">{blocker().later.length}</span></summary>
                  <ul><For each={blocker().later}>{id => <li>{link(id)}
                    <Show when={m().outputs.get(id)}>{reason => <span class="sprint-blockers-note">{reason()}</span>}</Show>
                    <Show when={(m().waiting.get(id) ?? []).some(dep => dep !== blocker().view.id)}><span class="sprint-blockers-note">{t("sprintBlockersWaitingOn")}</span></Show>
                    <ul><For each={(m().waiting.get(id) ?? []).filter(dep => dep !== blocker().view.id)}>{dep => <li>{link(dep)}</li>}</For></ul>
                  </li>}</For></ul>
                </details></Show>
                <Show when={blocker().chain.length > 2}><details>
                  <summary>{t("sprintBlockersChain")}</summary>
                  <ol class="sprint-blockers-chain"><For each={blocker().chain}>{id => <li>{link(id)}</li>}</For></ol>
                </details></Show>
              </li>}</Keyed>
            </ol>
          </Show>
        </>}</Show>
      </div>
    </Modal>
  </>;
}
