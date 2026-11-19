import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type PlanItem, type Project, type Sprint, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };

const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
/** A plan has no item count limit: 120 tasks, the tenth waiting for the ninetieth. */
const items: PlanItem[] = Array.from({ length: 120 }, (_, i) => ({ key: `t${i + 1}`, title: `Item ${i + 1}`, description: `Do item ${i + 1}`,
  dependsOn: i === 9 ? ["t90"] : i > 0 && i !== 10 ? [`t${i}`] : [] }));
const sprint: Sprint = { id: "sp", projectId: "p", description: "A large sprint", title: "Large", runMode: "manual", targetBranch: null, planning: "ready",
  conversation: [], plan: { version: 1, items }, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null };

it("shows a 120-task plan whole and offers to create every task", async () => {
  const state: WorkspaceState = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(project);
  state.sprints.push(sprint);
  const port = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/plan`);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port.port} />, root);
  await settled();
  port.emit(projection("1", state));
  await settled();
  const text = document.body.textContent ?? "";
  expect(text).toContain("Plan v1 proposed: 120 tasks");
  for (const n of [1, 60, 90, 120]) expect(text).toContain(`Item ${n}`);
  const create = [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.getAttribute("aria-label") === "Create 120 tasks from Plan v1");
  expect(create, "Create tasks names every task").toBeTruthy();
  // A double click is one Create tasks: the second press joins the one under way.
  create!.click();
  create!.click();
  await settled();
  expect(port.commands.filter(c => c.operation === "materialize_plan").map(c => c.payload)).toEqual([expect.objectContaining({ sprintId: "sp", version: 1 })]);
});
