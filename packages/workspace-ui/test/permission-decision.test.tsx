import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, emptySession, LOCAL_MACHINE_ID, mapDecision, type Decision } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

const paths = ["/home/o/a.txt", "/home/o/b.txt", "/home/o/c.txt", "/home/o/d.txt", "/home/o/e.txt"];
const wire = (patch: Record<string, unknown> = {}) => ({ decisionId: "wd_np1", kind: "permission", questionId: "np1", batchId: null, executionId: "e", acpSessionId: "s", scope: "a",
  prompt: "Owner decision: worker-backend-dev wants to read /home/o/a.txt because it reads outside the project",
  options: [{ optionId: "wo_allow", label: "Allow once" }, { optionId: "wo_deny", label: "Deny" }],
  version: "7", ownerEpoch: "1", requirementRevision: "0", status: "pending", allowedOperations: ["decisions.answer"], askedBy: "backend-dev",
  permission: { operation: "read", paths, reason: "outside_project", reasonText: "it reads outside the project", access: "guarded", rule: "outside_read" }, ...patch });

function world(decisions: Decision[]) {
  const state = withTask(emptyWorkspace(), "Fixture task");
  state.machines[0].connection = "connected";
  state.tasks[0].status = "needs_you"; state.tasks[0].process = "live";
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

// A native permission request a worker's engine sends to the owner is a card in the task's Conversation: who asks, what
// (paths folded past three), why, and only Allow once / Deny, answered with the decision's own option.
it("shows who asks, what and why, folds long path lists and answers with the decision's own option", async () => {
  const d = mapDecision(wire() as never);
  expect(d.kind).toBe("permission");
  const p = await mount([d]);
  const card = document.querySelector<HTMLElement>(".task-panel .timeline .dc")!;
  expect(card.getAttribute("aria-label")).toBe("Permission asked by backend-dev");
  expect(card.textContent).toContain("Asked by backend-dev");
  expect(card.querySelector(".dc-q")!.textContent).toBe("Backend-dev wants to read 5 paths");
  expect([...card.querySelectorAll(".ws-perm-paths li")].slice(0, 3).map(l => l.textContent)).toEqual(paths.slice(0, 3));
  expect(card.querySelector("details summary")!.textContent).toBe("2 more");
  expect(card.textContent).toContain("It is outside the project.");
  expect(card.textContent).toContain("Guarded");
  expect(card.textContent).toContain("Operationread");
  expect(card.textContent).toContain("Resource");
  expect(card.textContent).toContain("Allow once lets this worker continue this request");
  expect(card.querySelector("textarea, input[type=text]")).toBeNull();
  const buttons = [...card.querySelectorAll<HTMLButtonElement>(".ws-dc-acts button")];
  expect(buttons.map(b => b.textContent)).toEqual(["Deny", "Allow once", "Close task…"]);
  expect(buttons.every(b => b.type === "button")).toBe(true);
  buttons[1].click(); await settle();
  const sent = p.commands.at(-1)!;
  expect(sent.payload).toMatchObject({ type: "answer_decision", decisionId: "wd_np1", answer: "Allow once", optionId: "wo_allow" });
});

it("reads an answered permission as allowed once or denied, and a withdrawn one by its reason", async () => {
  const allowed = mapDecision(wire({ decisionId: "wd_a", questionId: "a", status: "answered", allowedOperations: [], answerText: "Allowed once", answeredAt: "2026-11-18T10:00:00Z" }) as never);
  const denied = mapDecision(wire({ decisionId: "wd_b", questionId: "b", status: "answered", allowedOperations: [], answerText: "Denied", answeredAt: "2026-11-18T10:01:00Z" }) as never);
  const gone = mapDecision(wire({ decisionId: "wd_c", questionId: "c", status: "superseded", allowedOperations: [], headline: "The task was paused before you answered." }) as never);
  await mount([allowed, denied, gone]);
  const text = document.querySelector(".task-panel .timeline")!.textContent!;
  expect(text).toContain("Allowed once");
  expect(text).toContain("Denied");
  expect(text).toContain("Withdrawn: The task was paused before you answered.");
});


// A command request shows the command line itself, its working directory and the concrete reason, not only the folder.
it("shows a command card's command line, directory and concrete reason", async () => {
  const command = "rm -rf /home/o/cache " + "x".repeat(300);
  const d = mapDecision(wire({ permission: { operation: "run", paths: ["/home/o/proj"], reason: "uncontained", reasonText: "it runs outside the sandbox", access: "guarded", rule: "outside_command",
    command, cwd: "/home/o/proj", why: "Deletes files: /home/o/cache" } }) as never);
  await mount([d]);
  const card = document.querySelector<HTMLElement>(".task-panel .timeline .dc")!;
  expect(card.querySelector("code.ws-perm-cmd")!.textContent!.startsWith("rm -rf /home/o/cache")).toBe(true);
  expect(card.querySelector("details pre")!.textContent).toBe(command);
  expect(card.textContent).toContain("Directory/home/o/proj");
  expect(card.textContent).toContain("Deletes files: /home/o/cache");
  expect(card.querySelector(".ws-perm-paths")).toBeNull();
});
