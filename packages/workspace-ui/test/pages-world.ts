import type { Sprint, Task, WorkspaceState } from "@cortex/workspace-domain";
import { conversationWorld } from "./conversation-world";
import { draftTask } from "./support";

// Fake data for the page captures: the conversation world plus a draft, an accepted task, a closed task, a sprint being
// planned and the model choices a connected machine offers. Disposable and opt-in; no service, engine or store is involved.
// `sprint`: "running" makes the sprint's tasks created and its queue running, with the draft not yet started in it (a task
// that can still be edited, and room to add more); "closed" is that sprint after Close sprint, as the service reports it.
export function pagesWorld(sprintState: "planning" | "running" | "closed" = "planning"): WorkspaceState {
  const s = conversationWorld();
  s.options = {
    models: [{ id: "opus", label: "Opus 5.5", engine: "claude", efforts: ["low", "medium", "high"] }, { id: "gpt", label: "GPT-5.5", engine: "codex", efforts: ["low", "medium", "high"] }],
    efforts: [{ id: "low", label: "Low" }, { id: "medium", label: "Medium" }, { id: "high", label: "High" }],
    modes: [{ id: "guarded", label: "Ask before changes" }, { id: "auto", label: "Work on its own" }],
    languages: [{ id: "auto", label: "Same as the message" }, { id: "ru", label: "Russian" }],
  };
  const settings = { model: "opus", effort: "high", mode: "guarded" as const, language: "auto" as const };
  for (const t of s.tasks) t.settings = settings;
  // A waiting question puts its task in Needs you, as the service reports it.
  Object.assign(s.tasks.find(t => t.id === "c-ask")!, { status: "needs_you", issue: "Question: Which concurrency level should the regression test assert?" });
  const task = (id: string, patch: Partial<Task>): Task => ({ ...draftTask(), id, projectId: "p", settings, ...patch });
  s.tasks.push(
    task("c-draft", { title: "Add rate limits to the login endpoint", description: "Limit failed logins to five a minute per account and per address." }),
    task("c-accepted", { title: "Rotate the signing keys monthly", description: "Rotate the signing keys on the first day of every month.", status: "ready_to_merge",
      checkout: { kind: "worktree", branch: "cw/rotate-signing-keys", path: "/home/owner/worktrees/atlas-api/rotate-signing-keys" }, workspaceDigest: "d2",
      result: { version: 2, digest: "d2", checks: [], at: Date.UTC(2026, 10, 9, 16, 5), summary: "Signing keys now rotate monthly." }, acceptance: { version: 2, digest: "d2", at: Date.UTC(2026, 10, 9, 17, 0) } }),
    task("c-closed", { title: "Remove the legacy session table", description: "Drop the table nothing reads any more.", status: "done", doneAt: Date.UTC(2026, 10, 1, 9, 0), work: "merged",
      archive: { at: Date.UTC(2026, 10, 2, 9, 0), previousStatus: "done", sizeMb: 1.2 } }),
  );
  const sprint: Sprint = { id: "sp", projectId: "p", title: "Harden authentication", description: "Make login and token refresh safe under load: rate limits, a lockout and a regression suite.",
    runMode: "manual", checkout: { kind: "worktree" }, targetBranch: null, planning: "ready", materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null, settings: { model: "opus", effort: "high" },
    conversation: [
      { kind: "note", text: "I split this into three tasks. The lockout depends on the rate limits, and the suite covers both." },
    ],
    plan: { version: 1, items: [
      { key: "a", title: "Add rate limits to login", description: "Five failed logins a minute per account and per address.", dependsOn: [] },
      { key: "b", title: "Lock an account after repeated failures", description: "Lock for fifteen minutes after ten failures.", dependsOn: ["a"] },
      { key: "c", title: "Regression suite for login and refresh", description: "Cover the limits, the lockout and the refresh race.", dependsOn: ["a", "b"] },
    ] } };
  if (sprintState !== "planning") {
    Object.assign(sprint, { planning: "created", materializedVersion: 1, taskIds: ["c-draft", "c-work"], queueState: sprintState === "closed" ? "idle" : "running", runMode: "automatic" });
    for (const t of s.tasks) if (t.id === "c-draft" || t.id === "c-work") t.sprintId = "sp";
    if (sprintState === "closed") sprint.archivedAt = Date.UTC(2026, 10, 10, 12, 0);
  }
  s.sprints = [sprint];
  return s;
}
