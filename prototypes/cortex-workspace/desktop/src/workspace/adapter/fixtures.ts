import { applyCommand, beginMerge, setProvider, createInitialWorkspace, createTask, DAY, generateMockTitle, newWorker, validatePlan, weeklyUsage, type Check, type WorkspaceState } from "../domain";
import { layoutConflict, seedReview } from "./review-fixture";
import { seedSessions } from "./session-fixture";
import { seedAudit, seedMachines, seedUsageBilling } from "./sprint-fixture";

export const WORKSPACE_FIXTURE_NOW = Date.UTC(2030, 0, 1, 2, 46, 40);
const id = (kind: string, n: number) => "mock-" + kind + "-" + String(n).padStart(5, "0");
export const WORKSPACE_CASES = {
  offline: id("task", 0), permission: id("task", 1), question: id("task", 2),
  failedCheck: id("task", 3), stale: id("task", 4), merged: id("task", 5), conflict: id("task", 6),
  working: id("task", 228), paused: id("task", 229), stopping: id("task", 230),
  closedAsIs: id("task", 231), cancelled: id("task", 232), readyToMerge: id("task", 233),
  archive29: id("task", 234), archive12: id("task", 235), archive2: id("task", 236),
  blockedPurge: id("task", 237), titleFailed: id("task", 238), titlePending: id("task", 239),
  unmergedQuestion: id("task", 240), sprintFirst: id("task", 241), session: id("task", 244),
  review: id("task", 245), merge: id("task", 246), mergeNext: id("task", 247), rollups: id("task", 248), mergedWorktree: id("task", 249),
} as const;
type Seed = {
  projects: { id: string; machineId: string; name: string; repository: string; branch: string }[];
  tasks: { id: string; projectId: string; title: string; description: string }[];
  events: WorkspaceState["events"];
};

/** Synthetic read models, not runtime evidence. Domain constructors supply invariant defaults. */
export function createWorkspaceFixture(source: Seed): WorkspaceState {
  const state = createInitialWorkspace();
  state.machines[0].lastConfirmedAt = "2030-01-01T02:46:40.000Z";
  applyCommand(state, { type: "add_remote", id: id("machine", 1), name: "Synthetic remote" });
  state.machines[1].lastConfirmedAt = "2030-01-01T00:00:00.000Z";
  const gpu = id("machine", 2);
  seedMachines(state, gpu);
  for (const project of source.projects) applyCommand(state, {
    type: "add_project", id: project.id, machineId: project.id === id("project", 6) ? gpu : project.machineId, name: project.name,
    repository: project.repository, openBranch: project.branch, defaultBranch: project.id === id("project", 2) ? "trunk" : "main",
  });
  const local = state.machines[0].config;
  local.projects[id("project", 5)] = { ceiling: { ...local.surfaces.user_defaults.ceiling, attempts: 3 } };
  local.projects[id("project", 2)] = { routing: setProvider(local.surfaces.routing, "codex-acp", false) };
  for (const [n, raw] of source.tasks.entries()) {
    const task = createTask(state, {
      projectId: raw.projectId, description: raw.description,
      checkout: n % 2 ? { kind: "main" } : { kind: "worktree", branch: "cw/task-" + n, path: "worktrees/task-" + n },
    }, raw.id);
    task.title = raw.title; task.titleState = "generated";
  }
  const at = (n: number) => state.tasks[n];
  const result = (n: number, checks: Check[], stale = false, accepted = false) => {
    const task = at(n), digest = "mock-digest-" + n + "-v1";
    task.status = accepted ? "ready_to_merge" : "review";
    task.workspaceDigest = stale ? digest + "-changed" : digest;
    task.result = { version: 1, digest, checks, summary: "Synthetic result; no checks executed", mock: true };
    task.acceptance = accepted ? { version: 1, digest } : null;
  };
  const passed: Check[] = [{ name: "Synthetic structure", outcome: "passed", detail: "Mock outcome only; no command ran" }];
  at(0).status = "working"; at(0).process = "unknown";
  at(1).status = "needs_you"; at(1).issue = "Mock permission decision";
  at(2).status = "needs_you"; at(2).issue = "Mock question";
  result(3, [{ name: "Synthetic check", outcome: "failed", detail: "Simulated failure" }]);
  result(4, [{ name: "Physical device", outcome: "not_run", detail: "No device check run" }], true);
  result(5, structuredClone(passed), false, true);
  at(5).status = "done"; at(5).work = "merged"; at(5).doneAt = WORKSPACE_FIXTURE_NOW - 2 * DAY;
  at(5).integration = { method: "squash", targetBranch: "main", digest: at(5).result!.digest, mock: true };
  at(6).status = "needs_you"; beginMerge(state, at(6), layoutConflict(at(6).checkout.branch, "main"), null); at(6).issue = "Synthetic Git conflict";
  at(228).status = "working"; at(228).process = "live";
  at(229).status = "paused";
  at(230).status = "stopping"; at(230).process = "unknown";
  for (const n of [231, 232, 240]) { at(n).status = "done"; at(n).doneAt = WORKSPACE_FIXTURE_NOW - 8 * DAY; at(n).notVerifiedByCortex = true; }
  at(231).closedAs = "as_is"; at(232).closedAs = "cancel";
  result(233, structuredClone(passed), false, true);
  const archived: [number, string, number, number | null, "merged" | "as_is" | "cancel"][] = [
    [234, "Sign webhook payloads", 29, 21, "merged"], [235, "Move search to the new index", 12, 18, "merged"], [236, "Migrate the cache layer", 2, 48, "merged"],
    [237, "Add invoice numbering spike", -1, 30, "as_is"], [316, "Drop the old invoice view", 1, 12, "merged"], [317, "Checkout flow copy pass", 22, 9, "as_is"],
    [487, "Try a smaller PDF renderer", 3, 3, "cancel"], [9, "Nightly index rebuild", -1, null, "merged"], [10, "Split the Terraform state", 16, null, "cancel"],
  ];
  for (const [n, title, daysLeft, sizeMb, outcome] of archived) {
    const task = at(n);
    task.title = title; task.status = "done"; task.work = outcome === "merged" ? "merged" : "discard_confirmed"; task.closedAs = outcome === "merged" ? null : outcome;
    task.notVerifiedByCortex = outcome !== "merged"; task.doneAt = WORKSPACE_FIXTURE_NOW - (37 - daysLeft) * DAY;
    task.archive = { at: WORKSPACE_FIXTURE_NOW - (30 - daysLeft) * DAY, previousStatus: "done", sizeMb };
  }
  at(237).unexpectedChanges = true; at(237).purgeBlocker = "New uncommitted files appeared after archiving";
  at(9).purgeBlocker = "Machine offline, cleans up at its next run";
  at(238).titleState = "failed"; at(238).title = "Title unavailable · retry or rename";
  at(239).titleState = "pending"; at(239).title = "Generating title…";
  at(240).archiveQuestion = true; at(240).title = "Spike: async webhook client"; at(240).workFacts = { commits: 5, files: 2, sizeMb: 84 };
  at(240).checkout.branch = "cw/async-webhooks";
  at(488).title = "Batch the embedding job"; at(488).status = "needs_you"; at(488).issue = "Question: which batch size should the job use?";
  at(489).title = "Profile the training loader"; at(489).status = "working"; at(489).process = "live";
  // A writer in Cedar Notes' main checkout, so a second one is refused there.
  at(253).title = "Tidy the notes index"; at(253).status = "working"; at(253).process = "live";

  const projectId = at(241).projectId;
  const sprintId = applyCommand(state, { type: "create_sprint", projectId, description: "Synthetic linked sprint", runMode: "automatic" })!;
  const sprint = state.sprints[0];
  sprint.plan = { version: 1, items: [241, 242, 243].map((n, i) => ({ key: "step-" + i, description: "Synthetic sprint task " + (i + 1), dependsOn: i ? ["step-" + (i - 1)] : [] })) };
  validatePlan(sprint.plan.items);
  sprint.taskIds = [241, 242, 243].map(n => at(n).id); sprint.materializedVersion = 1; sprint.planning = "created";
  [241, 242, 243].forEach((n, i) => {
    at(n).sprintId = sprintId; at(n).dependencies = i ? [at(n - 1).id] : [];
    at(n).title = generateMockTitle(at(n).description);
  });
  seedAudit(state, projectId, WORKSPACE_FIXTURE_NOW);

  state.workers = [0, 228, 5, 3].map((n, i) => {
    const task = at(n);
    task.executionId = id("execution", n);
    return newWorker({
      id: id("worker", i), taskId: task.id, executionId: task.executionId, role: i === 3 ? "QA Engineer" : "Frontend Dev",
      status: n === 0 ? "unknown" as const : n === 228 ? "working" as const : n === 3 ? "failed" as const : "completed" as const,
      goal: generateMockTitle(task.description),
      activity: [{ sequence: 1, kind: "note", text: "Mock worker activity; not a real transcript", at: null, mock: true as const }],
      result: n === 5 || n === 3 ? {
        summary: "Mock finished worker result",
        checks: n === 3 ? [{ name: "Render check", outcome: "unverified" as const, detail: "Not observed" }] : structuredClone(passed), mock: true as const,
      } : null,
    });
  });
  state.messages = [
    { id: id("message", 0), taskId: at(228).id, workerId: null, text: "Synthetic queued clarification", mode: "queue", delivery: "queued", at: null, deliveredAt: null, failure: null, mock: true },
    { id: id("message", 1), taskId: at(228).id, workerId: state.workers[1].id, text: "Synthetic direct message", mode: "send_now", delivery: "sending", at: null, deliveredAt: null, failure: null, mock: true },
  ];
  state.workers[1].activity.push({ sequence: 2, kind: "message", text: "Synthetic direct message", at: null, messageId: id("message", 1), mock: true });
  seedSessions(state, at(244), at(1), at(2));
  seedReview(state, { session: at(244), review: at(245), merge: at(246), next: at(247), rollups: at(248), merged: at(249) });
  seedUsageBilling(state, at(246), at(247), at(250), at(251), at(252));
  state.engineUsage = weeklyUsage([
    { engine: "codex", connected: true, usedPercent: 42, resetAt: "2030-01-07T00:00:00.000Z", observedAt: "2030-01-01T02:46:40.000Z" },
    { engine: "claude", connected: true, usedPercent: null, resetAt: null, observedAt: null },
  ]);
  state.events = structuredClone(source.events);
  return state;
}
