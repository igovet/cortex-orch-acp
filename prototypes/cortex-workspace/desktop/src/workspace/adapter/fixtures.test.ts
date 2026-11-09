import { describe, expect, it } from "vitest";
import { FIXTURE_VERSION, loadFixtures } from "../../fixtures";
import { boardPhase, DAY, targetBranch } from "../domain";
import { createMockWorkspace } from "./mock";
import { WORKSPACE_FIXTURE_NOW } from "./fixtures";

describe("CW-D01: v3 domain fixtures and mock-port contract", () => {
  it("preserves 500 tasks, 10,000 events, original stable identities and machine ownership", () => {
    const a = loadFixtures(), b = loadFixtures("c");
    expect(FIXTURE_VERSION).toBe("cw-fixtures-v3");
    expect(a).toEqual(b);
    expect(a.machines.map(m => m.kind)).toEqual(["local", "remote"]);
    expect(a.machines[0].name).toBe("This machine");
    expect(a.projects.filter(p => p.machineId === a.machines[0].id).length).toBeGreaterThan(1);
    expect(a.workspace.tasks.map(t => t.id)).toEqual(a.tasks.map(t => t.id));
    expect(a.workspace.events).toHaveLength(10000);
    expect(a.workspace.projects.every(p => a.workspace.machines.some(m => m.id === p.machineId))).toBe(true);
    expect(a.workspace.tasks.every(t => t.description.trim() && a.workspace.projects.some(p => p.id === t.projectId))).toBe(true);
    expect(a.workspace.events.every(e => a.workspace.tasks.some(t => t.id === e.taskId && t.projectId === e.projectId))).toBe(true);
  });
  it("covers lifecycle, stale, offline, conflict, planning and archive countdowns", () => {
    const f = loadFixtures().workspace;
    expect(new Set(f.tasks.map(t => boardPhase(f, t.id)))).toEqual(new Set(["draft", "working", "needs_you", "paused", "review", "ready_to_merge", "done", "archive", "unknown"]));
    expect(f.tasks.some(t => t.status === "stopping" && t.process === "unknown")).toBe(true);
    expect(f.tasks.some(t => t.titleState === "failed")).toBe(true);
    expect(f.tasks.some(t => t.closedAs === "as_is" && t.notVerifiedByCortex)).toBe(true);
    expect(f.tasks.some(t => t.closedAs === "cancel")).toBe(true);
    expect(new Set(f.tasks.map(t => t.checkout.kind))).toEqual(new Set(["main", "worktree"]));
    expect(new Set(f.sprints.map(s => s.planning))).toEqual(new Set(["created", "question"]));
    const days = f.tasks.flatMap(t => t.archive ? [Math.ceil((t.archive.at + 30 * DAY - WORKSPACE_FIXTURE_NOW) / DAY)] : []);
    expect(days).toEqual(expect.arrayContaining([29, 12, 2]));
    expect(f.tasks.some(t => t.archive && t.purgeBlocker)).toBe(true);
    expect(f.engineUsage.map(e => e.engine)).toEqual(["codex", "claude"]);
    expect(f.engineUsage[1].usedPercent).toBeNull();
    expect(f.workers.some(w => w.result?.checks.some(c => c.outcome === "unverified"))).toBe(true);
    expect(f.messages.some(m => m.delivery === "queued")).toBe(true);
  });
  it("the mock port uses isolated fixture snapshots, rejects stale acceptance, and never claims backend evidence", () => {
    const seed = loadFixtures().workspace, p = createMockWorkspace(seed);
    const stale = seed.tasks.find(t => t.result && t.result.digest !== t.workspaceDigest)!;
    const before = p.snapshot();
    expect(p.dispatch({ type: "accept_result", taskId: stale.id, digest: stale.result!.digest })).toMatchObject({ ok: false, mock: true });
    expect(p.snapshot()).toEqual(before);
    const ready = seed.tasks.find(t => t.status === "ready_to_merge")!;
    const branch = targetBranch(seed, ready);
    expect(p.dispatch({ type: "integrate", taskId: ready.id, method: "squash", confirmed: false, digest: ready.result!.digest, targetBranch: branch, now: WORKSPACE_FIXTURE_NOW }).ok).toBe(false);
    expect(seed).toEqual(loadFixtures().workspace);
  });
});
