import { emptySession, emptyWorkspace, type Check, type Decision, type Message, type Task, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { draftTask } from "./support";

// Fake data for the conversation captures: one task waiting on a question, one working with a queued message, one with a
// result to review. Disposable and opt-in; no service, engine or store is involved.
const BRIEF = "Fix the token refresh race. Two concurrent refreshes rotate the same token row, and the second call reads a token that was already revoked.";
const route = { model: "Opus 5.5", effort: "High" };
const worker = (id: string, taskId: string, role: string, patch: Partial<Worker> = {}): Worker => ({
  id, taskId, executionId: `e-${taskId}`, role, status: "working", lane: "execution", goal: "Lock the rotation and add a version column", now: "Editing the refresh handler",
  waitsFor: null, scope: [], route, attempt: null, elapsedMs: 184000, runningSince: null, context: null, usage: null, activity: [], result: null, ...patch,
});
const checks: Check[] = [
  { name: "Unit tests for the auth service", detail: "212 passed", outcome: "passed", source: "runtime" },
  { name: "Concurrency test, 50 parallel refreshes", detail: "Passed in 3.1 s", outcome: "passed", source: "runtime" },
  { name: "Load test against staging", detail: "", outcome: "not_run", kind: "policy", technicalReason: "Paid live check not requested by the owner", source: "runtime" },
];
const result = { version: 1, digest: "digest", checks, at: Date.UTC(2026, 10, 10, 11, 32),
  summary: "The refresh path now locks the token row and checks a version column, so a second concurrent refresh waits and reads the new token.\n\n- Added a migration for the version column.\n- Covered the race with a test that runs 50 parallel refreshes." };

export function conversationWorld(): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  const machineId = s.machines[0].id;
  s.projects = [{ id: "p", machineId, name: "atlas-api", repository: "/home/owner/projects/atlas-api", openBranch: "main", defaultBranch: "main", hasGit: true }];
  const task = (id: string, patch: Partial<Task>): Task => ({ ...draftTask(), id, projectId: "p", title: "Fix token refresh race", description: BRIEF, workspaceDigest: "digest",
    checkout: { kind: "worktree", branch: "cw/fix-token-refresh-race", path: "/home/owner/worktrees/atlas-api/fix-token-refresh-race" }, ...patch });
  s.tasks = [
    task("c-ask", { status: "working", process: "live", executionId: "e-c-ask" }),
    task("c-work", { status: "working", process: "live", executionId: "e-c-work" }),
    task("c-done", { status: "review", result }),
  ];
  s.workers = [
    worker("w-be", "c-ask", "Backend Dev"), worker("w-qa", "c-ask", "QA Engineer", { status: "working", now: null, waitsFor: "your answer" }),
    worker("w-be2", "c-work", "Backend Dev"), worker("w-qa2", "c-work", "QA Engineer", { status: "pending" as Worker["status"], now: null }),
    worker("w-be3", "c-done", "Backend Dev", { status: "completed", now: null, result: { summary: "Locked the rotation.", checks: [] } }),
    worker("w-rv3", "c-done", "Code Reviewer", { status: "completed", lane: "review", now: null, result: { summary: "No findings.", checks: [] } }),
  ];
  const plan = (who: [string, string | null][]) => ({ revision: 2, note: "lock first, then test", nodes: who.map(([title, workerId], i) => ({ id: `n${i}`, title, who: i ? "QA Engineer" : "Backend Dev", workerId })) });
  const question: Decision = { id: "d-level", kind: "question", from: "QA Engineer", workerId: "w-qa", at: Date.now() - 180000, withdrawn: false, answer: null,
    text: "Which concurrency level should the regression test assert?",
    options: [{ label: "50 parallel refreshes", sub: "Matches production peak", optionId: "o50" }, { label: "200 parallel refreshes", sub: "Stress, 40 s per run", optionId: "o200" }, { label: "Both, with 200 marked slow", optionId: "oboth" }] };
  const opening = (id: string) => [
    { id: `${id}-a1`, kind: "cortex" as const, text: "I traced the 401s to two concurrent refreshes rotating the same token row. Plan r2 locks the rotation, adds a version column and covers it with a concurrency test." },
    { id: `${id}-plan`, kind: "plan" as const },
    { id: `${id}-team`, kind: "workers" as const, title: "Team", workerIds: s.workers.filter(w => w.taskId === id).map(w => w.id) },
  ];
  s.sessions = [
    { ...emptySession("c-ask", Date.now() - 900000), plan: plan([["Lock the token rotation", "w-be"], ["Write the concurrency test", "w-qa"]]), decisions: [question],
      timeline: [...opening("c-ask"), { id: "c-ask-q", kind: "decision", decisionId: question.id }] },
    { ...emptySession("c-work", Date.now() - 600000), plan: plan([["Lock the token rotation", "w-be2"], ["Write the concurrency test", "w-qa2"]]),
      timeline: [...opening("c-work"), { id: "c-work-f", kind: "fold", text: "Edited 2 files, ran 3 commands", steps: [] }, { id: "c-work-a2", kind: "cortex", text: "Backend Dev is adding the row lock now. QA Engineer starts once the migration lands." }] },
    { ...emptySession("c-done", Date.now() - 3600000), plan: plan([["Lock the token rotation", "w-be3"], ["Review the change", "w-rv3"]]), changes: { files: 3, add: 84, del: 12 },
      timeline: [...opening("c-done"), { id: "c-done-a2", kind: "cortex", text: "Review and checks finished. The result is ready for you." }] },
  ];
  const queued: Message = { id: "q-1", taskId: "c-work", workerId: null, text: "Keep the old error message for expired tokens, the mobile app matches on it.", mode: "queue", delivery: "queued", at: Date.now() - 60000, deliveredAt: null, failure: null };
  s.messages = [queued];
  return s;
}
