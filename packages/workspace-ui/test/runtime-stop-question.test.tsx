import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, emptySession, LOCAL_MACHINE_ID, mapDecision, type Decision, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

const reviewer: Worker = {
  id: "w-review", taskId: "t", executionId: "e", role: "code-reviewer", status: "completed", lane: "review", goal: "Review", now: null, waitsFor: null, scope: [], route: null, attempt: null,
  elapsedMs: null, runningSince: null, context: null, usage: null, result: { summary: "Blocked", checks: [], reportId: "r_2413b9f526b9" }, activity: [],
};
const wire = (patch: Record<string, unknown>) => mapDecision({ decisionId: "wd_q2", kind: "question", questionId: "q2", batchId: null, executionId: "e", acpSessionId: "s", scope: "task",
  prompt: "The task stopped: the independent review blocked the result.\n\nWhat happened: The reviewer did not accept the result.\n\nWhat you can do:\n1. Continue (recommended) — address the findings.\n2. Finish — close as is.\n\nWhat should happen next?",
  options: [{ optionId: "o1", label: "Continue: let the coordinator decide how to proceed", detail: "address the findings: the coordinator plans the fixes", action: "continue", recommended: true },
    { optionId: "o2", label: "Finish: close the task as is, without certifying unfinished work", detail: "the review findings stay unaddressed", action: "finish" }],
  version: "1", ownerEpoch: "1", requirementRevision: "1", status: "pending", allowedOperations: ["decisions.answer"], runtimeStop: true,
  headline: "The task stopped: the independent review blocked the result.", ...patch });

function world(decisions: Decision[]) {
  const state = withTask(emptyWorkspace(), "Fixture task");
  state.machines[0].connection = "connected";
  state.tasks[0].status = "needs_you"; state.tasks[0].process = "live";
  state.workers = [reviewer];
  const session = emptySession("t");
  session.timeline = decisions.map(d => ({ id: `e-${d.id}`, kind: "decision" as const, decisionId: d.id }));
  session.decisions = decisions;
  state.sessions = [session];
  return state;
}
async function mount(decisions: Decision[]) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", world(decisions))); await settle();
  return p;
}

// The runtime's stop question explains itself: each answer says what it does, the recommended one is marked, and what the
// question names (the review report, the blocking task, the denied path) can be opened from the card.
it("shows each answer's consequence and the recommendation, and opens the review findings and the blocking task", async () => {
  const stop = wire({ targets: [{ kind: "report", reportId: "r_2413b9f526b9", role: "code-reviewer" }, { kind: "task", taskId: "t2", title: "Format all tests", runtimeTaskId: "x" },
    { kind: "rule", path: "docs/**" }, { kind: "task", title: "no id" }] });
  expect(stop.targets).toHaveLength(3);
  await mount([stop]);
  const card = document.querySelector(".task-panel .timeline .dc-panel")!;
  expect(card.textContent).toContain("What happened: The reviewer did not accept the result.");
  const options = [...card.querySelectorAll(".opt")];
  expect(options[0].textContent).toContain("Recommended");
  expect(options[0].querySelector(".sub")?.textContent).toBe("address the findings: the coordinator plans the fixes");
  expect(options[1].textContent).not.toContain("Recommended");
  expect(options[1].querySelector(".sub")?.textContent).toBe("the review findings stay unaddressed");
  expect(card.textContent).toContain("Writing denied by your rule: docs/**");
  const links = [...card.querySelectorAll<HTMLAnchorElement>(".ws-dc-targets a")];
  expect(links.map(a => a.textContent)).toEqual(["Open review findings", "Open task “Format all tests”"]);
  links[0].click(); await settle();
  expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w-review`);
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`); window.dispatchEvent(new PopStateEvent("popstate")); await settle();
  document.querySelector<HTMLAnchorElement>(".task-panel .ws-dc-targets a:nth-of-type(2)")!.click(); await settle();
  expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t2`);
});

// A stop question the runtime replaced says so and shows its one-line reason, never the whole earlier text as a bare line.
it("reads a replaced runtime question as replaced by the one below, and another withdrawn one by its first line", async () => {
  const replaced = mapDecision({ decisionId: "wd_q1", kind: "question", questionId: "q1", batchId: null, executionId: "e", acpSessionId: "s", scope: "task",
    prompt: "The task stopped: the independent review blocked the result.\n\nWhat happened: details", options: [{ optionId: "o", label: "Continue" }, { optionId: "p", label: "Finish" }],
    version: "1", ownerEpoch: "1", requirementRevision: "1", status: "superseded", allowedOperations: [], replacedBy: "wd_q2", headline: "The task stopped: the independent review blocked the result." });
  const withdrawn = mapDecision({ decisionId: "wd_q0", kind: "question", questionId: "q0", batchId: null, executionId: "e", acpSessionId: "s", scope: "task",
    prompt: "Which port?\nmore", options: [{ optionId: "o", label: "80" }, { optionId: "p", label: "443" }], version: "1", ownerEpoch: "1", requirementRevision: "1", status: "superseded", allowedOperations: [] });
  await mount([withdrawn, replaced, wire({})]);
  const lines = [...document.querySelectorAll(".task-panel .timeline .answered")].map(e => e.textContent);
  expect(lines).toEqual(["Withdrawn: Which port?", "Replaced by the question below: The task stopped: the independent review blocked the result."]);
});
