import { createEffect, createRoot } from "solid-js";
import { expect, it, vi } from "vitest";
import { LOCAL_MACHINE_ID, type Sprint, type TaskOverlay } from "@cortex/workspace-domain";
import { createRealPort } from "../src/port/real";
import { sideRows, activeProject, type SideRow } from "../src/selectors";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, flush, scriptedTransport, wireTask } from "./support";

const sprint: Sprint = {
  id: "sp", projectId: "p", title: "Sprint", description: "Make a sprint", runMode: "hybrid",
  planning: "planning", targetBranch: null, conversation: [], plan: null, materializedVersion: null,
  taskIds: [], queueState: "idle", pauseReason: null,
};
const running = (id: string) => wireTask({
  taskId: id, description: id, version: "2", executionId: `exec-${id}`,
  phase: "execution", executionState: "running", runtimeState: "EXECUTING",
});
const plannerDraft = wireTask({ taskId: "planner", description: sprint.description, planningSprintId: sprint.id });
const planner = { ...running("planner"), planningSprintId: sprint.id };
const draft = wireTask({ taskId: "draft", description: "Second member" });
const member = running("member");
const standalone = running("standalone");
const rowKey = (r: SideRow) => r.kind === "sprint" ? `sprint:${r.sprint.id}` : `${r.nested ? "child" : "task"}:${r.view.id}`;
function holdReply() {
  let release!: () => void, sampled!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  const started = new Promise<void>(resolve => { sampled = resolve; });
  return { hold: () => { sampled(); return held; }, started, release };
}

it.each(["before the sprint record", "before the member links", "in a native delta"] as const)(
  "keeps sidebar rows coherent when native sprint tasks arrive %s", async ordering => {
    let high = 1, items = [running("one"), running("two")];
    let product: { tasks: Record<string, TaskOverlay>; sprints: Sprint[] } = { tasks: {}, sprints: [] };
    let commitDuringBoard: (() => void) | undefined;
    const holds = new Map<string, ReturnType<typeof holdReply>>();
    const reply = <T,>(op: string, value: T) => {
      const gate = holds.get(op);
      holds.delete(op);
      return gate ? gate.hold().then(() => value) : value;
    };
    const transport = scriptedTransport({
      "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
      "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
      "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "decisions.list"]),
      "decisions.list": () => reply("decisions.list", { items: [], nextCursor: null, highWatermark: "0" }),
      "native.board": () => {
        commitDuringBoard?.();
        commitDuringBoard = undefined;
        return { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2",
          highWatermark: String(high), data: { type: "board", value: { items: structuredClone(items), nextCursor: null, highWatermark: String(high) } } };
      },
      "machine.call": raw => {
        const op = (raw as { op: string }).op;
        if (op === "repo.snapshot") return { ok: true, result: null };
        if (op === "product.board" || op === "product.read") return reply(op, { ok: true, result: structuredClone(product) });
        return { ok: true, result: {} };
      },
    });
    const port = createRealPort({ transport: transport.transport, actorId: "owner", registry: true });
    const capture = (store: ReturnType<typeof createWorkspaceStore>) => {
      const index = store.index(), summary = index.summaries.get("p");
      if (!summary) return null;
      return {
        active: [activeProject(summary)].flatMap(s => s ? sideRows(index, s, undefined, Infinity).map(rowKey) : []),
        projects: sideRows(index, summary).map(rowKey),
        // The project badge and All link count drafts even when they have no preview row.
        standalone: summary.views.filter(v => !v.sprint).map(v => v.id),
      };
    };
    const seen: NonNullable<ReturnType<typeof capture>>[] = [];
    let dispose!: () => void;
    const store = createRoot(d => {
      dispose = d;
      const store = createWorkspaceStore(port);
      createEffect(() => {
        const rows = capture(store);
        // The real sidebar still shows its startup skeleton until this project settles.
        if (rows && store.ready()) seen.push(rows);
      });
      return store;
    });
    try {
      await vi.waitFor(() => expect(store.state().tasks.map(t => t.id)).toEqual(["one", "two"]));
      const baseline = { active: ["task:one", "task:two"], projects: ["task:one", "task:two"], standalone: ["one", "two"] };
      const assertHistory = (from: number, allowed: typeof baseline[]) => {
        const actual = seen.slice(from);
        expect(actual.filter(rows => !allowed.some(expected => JSON.stringify(expected) === JSON.stringify(rows)))).toEqual([]);
      };
      assertHistory(0, [baseline]);

      // The native classification arrives with the draft, even if the sampled product board has no sprint
      // yet. If the sprint record is present, its planner link is still added separately before Start.
      const beforePlanner = seen.length;
      const withParent = { ...baseline, projects: ["sprint:sp", ...baseline.projects] };
      const hasParent = ordering !== "before the sprint record";
      if (hasParent) product = { tasks: {}, sprints: [sprint] };
      high++; items = [...items, plannerDraft];
      await store.reload();
      expect(store.state().tasks.some(t => t.id === "planner" && t.planningSprintId === "sp")).toBe(true);
      expect(capture(store)).toEqual(hasParent ? withParent : baseline);
      const linked = { ...sprint, planningTaskId: "planner" };
      const details = ordering === "in a native delta" ? holdReply() : undefined;
      if (details) holds.set("decisions.list", details);
      if (hasParent) {
        product = { tasks: {}, sprints: [linked] };
        high++; items = [...items.slice(0, -1), planner];
        await store.reload();
        expect(capture(store)).toEqual(withParent);
      }
      assertHistory(beforePlanner, hasParent ? [baseline, withParent] : [baseline]);
      if (hasParent) {
        const parentAt = seen.findIndex(rows => rows.projects.includes("sprint:sp"));
        expect(parentAt).toBeGreaterThanOrEqual(beforePlanner);
        assertHistory(parentAt, [withParent]);
      }

      const beforeMembership = seen.length;
      let staleFacts: ReturnType<typeof port.query<"project.facts">> | undefined;
      const oldFacts = ordering === "before the member links" ? holdReply() : undefined;
      if (oldFacts) {
        holds.set("product.read", oldFacts);
        staleFacts = port.query("project.facts", { projectId: "p", fresh: true, git: false });
        await oldFacts.started;
      }
      const commit = () => {
        high++;
        items = [...items, draft, member, standalone];
        // Materialization installs the reverse links and full list together. The first member can start before
        // the next sampled native board. The ordinary task in that same board has no saved overlay.
        product = {
          tasks: { draft: { sprintId: "sp", planning: false }, member: { sprintId: "sp", planning: false } },
          sprints: [{ ...linked, planning: "created", materializedVersion: 1, queueState: "running", taskIds: ["member", "draft"] }],
        };
      };
      if (details) {
        await details.started;
        const membership = holdReply();
        holds.set("product.board", membership);
        commit();
        transport.send({ type: "native_delta", projectId: "p", batch: { generation: "g",
          streamKey: { scope: "project", project_id: "p" }, fromSequence: String(high), throughSequence: String(high),
          changes: [draft, member, standalone].map(t => ({ kind: "task", id: t.taskId, baseVersion: "0", newVersion: t.version,
            operation: { type: "upsert", data: { kind: "task_projection", data: t } } })),
        } });
        await membership.started;
        details.release();
        // Let both presentation stages return pending (100 ms each). An older details read
        // finishing in this gap must not install the unpaired native rows.
        await new Promise(resolve => setTimeout(resolve, 125));
        try { expect(capture(store)).toEqual(withParent); } finally { membership.release(); }
        await vi.waitFor(() => expect(store.state().tasks.some(t => t.id === "standalone")).toBe(true));
      } else {
        // The old product response is sampled before the write; the later native response sees the commit.
        commitDuringBoard = commit;
        await store.reload();
      }
      const placed = {
        active: ["sprint:sp", "child:member", ...baseline.active, "task:standalone"],
        projects: ["sprint:sp", "child:member", ...baseline.projects, "task:standalone"],
        standalone: [...baseline.standalone, "standalone"],
      };
      oldFacts?.release();
      if (staleFacts) expect(await staleFacts, "older enrichment cannot replace the new membership").toEqual([]);
      expect.soft(capture(store), "the first publication of a new ordinary task is already coherent").toEqual(placed);
      // A subsequent read must not make an extra row disappear or change the established ordering.
      await store.reload();
      await flush();
      expect(capture(store)).toEqual(placed);
      assertHistory(beforeMembership, [hasParent ? withParent : baseline, placed]);
      const placedAt = seen.findIndex(rows => rows.projects.includes("task:standalone"));
      expect(placedAt).toBeGreaterThanOrEqual(0);
      assertHistory(placedAt, [placed]);
    } finally { dispose(); }
  },
);
