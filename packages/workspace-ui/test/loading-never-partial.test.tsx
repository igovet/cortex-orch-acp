import { createRoot } from "solid-js";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, type ProjectFacts, type WorkspaceCommand } from "@cortex/workspace-domain";
import { createWorkspaceStore } from "../src/store";
import { flush, scriptedPort } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); });
const factsOf = (projectId: string, sprints: ProjectFacts["sprints"] = []): ProjectFacts => ({ projectId, openBranch: "main", defaultBranch: "main", repo: null, workers: [], reviews: [], sprints, overlays: {} });
const board = () => {
  const w = emptyWorkspace();
  const project = (id: string) => ({ id, machineId: w.machines[0].id, name: id, repository: `/${id}`, openBranch: "main", defaultBranch: "main" });
  return { ...w, projects: [project("p"), project("q")] };
};

it("keeps the sidebar list unsettled until every project's facts have arrived, not project by project", async () => {
  const s = scriptedPort(); (s.port as { readsFacts?: boolean }).readsFacts = true;
  let facts: ProjectFacts[] = [factsOf("p")];
  s.port.query = (async (op: string) => op === "capabilities.get" ? { everything: true, operations: [] } : op === "project.snapshot" ? [{ kind: "projection", scope: "workspace", version: "1", state: board() }] : op === "project.facts" ? facts : []) as typeof s.port.query;
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await flush();
  const [p, q] = store.state().projects;
  // The fast project is read; the slow one's facts are still under way: neither may show its rows yet.
  expect(store.projectSettled(p)).toBe(true);
  expect(store.projectSettled(q)).toBe(false);
  s.emit({ kind: "facts", facts: factsOf("q") });
  await flush();
  expect(store.projectSettled(q)).toBe(true);
});

it("a created sprint that is not in the model yet is loading until a read that began after the reply delivered its project", async () => {
  const s = scriptedPort(); (s.port as { readsFacts?: boolean }).readsFacts = true;
  let gate: Promise<void> | null = null, open!: () => void;
  s.port.query = (async (op: string) => {
    if (op === "capabilities.get") return { everything: true, operations: [] };
    if (op === "project.snapshot") return [{ kind: "projection", scope: "workspace", version: "1", state: board() }];
    if (op !== "project.facts") return [];
    await gate;
    return [factsOf("p", []), factsOf("q")];
  }) as typeof s.port.query;
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await flush();
  // Settled and unknown: this is the confirmed "not found".
  expect(store.sprintPending("sp1", "p")).toBe(false);
  gate = new Promise<void>(r => { open = r; });
  const created = store.dispatch({ type: "create_sprint", projectId: "p", description: "x", runMode: "manual" } as unknown as WorkspaceCommand);
  await flush();
  s.receipt(true, "sp1");
  await flush();
  // The reply is in, the confirming read is still under way: a skeleton, never "Not found".
  expect(store.sprintPending("sp1", "p")).toBe(true);
  open(); await created; await flush(); await flush();
  // A read that began after the reply delivered project p without the sprint: now it is confirmed missing.
  expect(store.sprintPending("sp1", "p")).toBe(false);
});
