import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, emptySession, LOCAL_MACHINE_ID, mapDecision, type Decision, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

const worker = (id: string, role: string): Worker => ({
  id, taskId: "t", executionId: "e", role, status: "working", lane: "execution", goal: "Commit the staged group", now: null, waitsFor: null, scope: [], route: null, attempt: null,
  elapsedMs: null, runningSince: null, context: null, usage: null, result: null,
  activity: [
    { sequence: 1, kind: "note", text: "All staged correctly. Now commit this first logical group.", at: null },
    { sequence: 2, kind: "command", text: "git commit -m one", at: null, command: { state: "running", exit: null, took: null, tail: "", reason: null } },
  ],
});

function world(workers: Worker[], decisions: Decision[] = [], status: "working" | "needs_you" = "working") {
  const state = withTask(emptyWorkspace(), "Fixture task");
  state.machines[0].connection = "connected";
  state.tasks[0].status = status; state.tasks[0].process = "live";
  state.workers = workers;
  const session = emptySession("t");
  session.timeline = [{ id: "c1", kind: "cortex", text: "Starting the devops worker." }, ...decisions.map(d => ({ id: `e-${d.id}`, kind: "decision" as const, decisionId: d.id }))];
  session.decisions = decisions;
  state.sessions = [session];
  return state;
}
async function mount(workers: Worker[], decisions: Decision[] = [], status: "working" | "needs_you" = "working") {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", world(workers, decisions, status))); await settle();
  return p;
}

// The Conversation speaks for the coordinator alone: a worker's narration, tool steps and running command stay on its page.
it("keeps a running worker's narration and command out of the Conversation and shows them on the worker's page", async () => {
  await mount([worker("w", "devops-engineer")]);
  const timeline = document.querySelector(".task-panel .timeline")!;
  expect(timeline.textContent).toContain("Starting the devops worker.");
  expect(timeline.textContent).not.toContain("All staged correctly");
  expect(timeline.textContent).not.toContain("git commit");
  const line = timeline.querySelector(".ws-working")!;
  expect(line.textContent).toBe("Waiting for devops-engineer…");
  expect(line.textContent).not.toMatch(/Running command/);

  const team = document.querySelector<HTMLButtonElement>(".dock .ws-team-lead")!;
  team.click(); await settle();
  const pane = document.querySelector(".livepane")!;
  expect(pane.textContent).toContain("All staged correctly");
  expect(pane.querySelector(".ws-working")?.textContent).toBe("Running command…");
});

it("names how many workers it waits for when several run, and says Working with none", async () => {
  await mount([worker("w1", "devops-engineer"), worker("w2", "qa-engineer")]);
  expect(document.querySelector(".task-panel .timeline .ws-working")?.textContent).toBe("Waiting for 2 workers…");
  dispose?.(); document.body.innerHTML = "";
  await mount([]);
  expect(document.querySelector(".task-panel .timeline .ws-working")?.textContent).toBe("Working…");
});

// A worker's question needs the owner: it comes from the task's decisions, not the worker's stream, so leaving that stream out of
// the Conversation never hides it. It reads there with the role that asked and is answered there.
it("keeps a worker's question visible and answerable in the Conversation, named by the worker that asked", async () => {
  const asked = mapDecision({ decisionId: "wd_q", kind: "question", questionId: "q", batchId: null, executionId: "e", acpSessionId: "s", scope: "k", prompt: "Push to origin now?",
    options: [{ optionId: "o1", label: "Push" }, { optionId: "o2", label: "Wait" }], version: "1", ownerEpoch: "1", requirementRevision: "1", status: "pending",
    allowedOperations: ["decisions.answer"], askedBy: "devops-engineer" });
  const p = await mount([{ ...worker("w", "devops-engineer"), status: "waiting_you" } as Worker], [asked], "needs_you");
  const timeline = document.querySelector(".task-panel .timeline")!;
  expect(timeline.textContent).toContain("Push to origin now?");
  expect(timeline.querySelector(".ws-dc-from")?.textContent).toBe("Asked by devops-engineer");
  expect(timeline.textContent).not.toContain("All staged correctly");
  [...timeline.querySelectorAll<HTMLElement>(".dc-panel .opt")].find(o => o.textContent?.includes("Push"))!.click(); await settle();
  timeline.querySelector<HTMLElement>('.dc [aria-label="Send answer"]')!.click(); await settle();
  const sent = p.commands.filter(c => c.payload.type === "answer_decision");
  expect(sent).toHaveLength(1);
  expect(sent[0].payload).toMatchObject({ decisionId: "wd_q", answer: "Push" });
  // Answered, it stays in the Conversation as an answered question.
  p.receipt(); await settle();
  p.emit(projection("2", world([worker("w", "devops-engineer")], [{ ...asked, answer: { text: "Push", at: 2 } }]))); await settle();
  expect(document.querySelector(".task-panel .timeline")!.textContent).toContain("Push to origin now?");
});
