import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, orderReasonText, parseLiveSprintGraph, type LiveSprintGraph, type Project, type Sprint, type SprintGraphEdit, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { changedBy, changeWords, revisionHeading } from "../src/sprint-revisions";
import { draftTask, flush, projection, scriptedPort } from "./support";

/*
 * A sprint task's proposed revision of the remaining drafts: the owner applies or rejects it on its card in a Hybrid or
 * Manual sprint; a proposed cancellation is only shown; applied changes say which task made them.
 */

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === text);

const REV = "a".repeat(64);
const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
const tasks = (): Task[] => {
  const t = (id: string, title: string, patch: Partial<Task> = {}): Task => ({ ...draftTask(), id, title, description: title, sprintId: "sp", ...patch });
  return [t("t1", "Lay the schema", { status: "working", process: "live", executionId: "e1" }), t("t2", "Write the importer"), t("t3", "Write the exporter")];
};
const sprint: Sprint = { id: "sp", projectId: "p", description: "Data", title: "Data", runMode: "hybrid", targetBranch: null, planning: "created",
  conversation: [], plan: { version: 1, items: [] }, materializedVersion: 1, taskIds: ["t1", "t2", "t3"], queueState: "running", pauseReason: null };
const wire = (status: string, applied = false) => ({ revisionId: "sr_1", taskId: "t1", status, applied, reason: "The owner switched to CSV",
  changes: [{ op: "update", member_id: "t2", expected_version: "2", description: "Import CSV" }, { op: "propose_cancel", member_id: "t3", reason: "Not needed" },
    { op: "add", key: "n1", corrects: "t1", title: "Validate CSV", description: "Validate", dependsOn: ["t2"] }],
  startedAffected: ["t1", "unloaded-card"], held: ["t2", "t3"], updated: applied ? ["t2"] : [], added: applied ? ["t9"] : [] });
const graph = (status: string, applied = false): LiveSprintGraph => parseLiveSprintGraph({ contractVersion: 1, sprintId: "sp", graphRevision: REV, complete: true,
  priority: ["t1", "t2", "t3"], order: { mode: "hybrid", queueState: "running", working: [{ taskId: "t1", startOrder: "1" }],
    notStarted: [{ taskId: "t2", rank: null, reason: "sprint_revision_pending", next: false, conditional: false }, { taskId: "t3", rank: null, reason: "sprint_revision_pending", next: false, conditional: false }] },
  members: ["t1", "t2", "t3"].map(id => ({ taskId: id, readStatus: "confirmed", dependencies: [], editable: id !== "t1", canBePredecessor: true, approvedPlanKey: null })),
  revisions: [wire(status, applied)] })!;
function world(): WorkspaceState {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(project);
  state.tasks.push(...tasks());
  state.sprints.push(sprint);
  return state;
}
async function open(path: string, answer: () => LiveSprintGraph) {
  const port = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const edits: SprintGraphEdit[] = [];
  port.port.sprintGraph = { read: async () => ({ kind: "graph", graph: answer() }), edit: async edit => { edits.push(edit); return { outcome: "changed" }; } };
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port.port} />, root);
  await settled();
  port.emit(projection("1", world()));
  await settled();
  return edits;
}

describe("sprint revisions", () => {
  it("parses the service's revisions and words each change", () => {
    const g = graph("awaiting_owner");
    expect(g.revisions?.[0]).toMatchObject({ revisionId: "sr_1", taskId: "t1", status: "awaiting_owner", held: ["t2", "t3"] });
    const title = (id: string) => ({ t1: "Lay the schema", t2: "Write the importer", t3: "Write the exporter" } as Record<string, string>)[id] ?? id;
    expect(revisionHeading(g.revisions![0], title, "ru-RU")).toBe("Изменения спринта от задачи Lay the schema");
    expect(revisionHeading(g.revisions![0], title, "en")).toBe("Sprint changes from task Lay the schema");
    expect(g.revisions![0].changes.map(c => changeWords(c, title))).toEqual([
      "Change Write the importer: description", "Proposed for cancellation: Write the exporter — Not needed", "New task: Validate CSV"]);
    expect(orderReasonText("sprint_revision_pending")).toBe("Waits for a sprint revision");
    expect(changedBy(graph("applied", true), "t2", title, "ru-RU")).toBe("изменено задачей Lay the schema");
    expect(changedBy(graph("applied", true), "t9", title, "en")).toBe("added by task Lay the schema");
    expect(changedBy(graph("awaiting_owner"), "t2", title)).toBeNull();
  });

  it("offers Apply and Reject on the owner card and sends the revision with the graph revision seen", async () => {
    const edits = await open(`/m/${LOCAL_MACHINE_ID}/p/p/s/sp/plan`, () => graph("awaiting_owner"));
    expect(document.body.textContent).toContain("Sprint changes from task Lay the schema");
    expect(document.body.textContent).toContain("Nothing is cancelled for a proposal");
    expect(document.body.textContent).toContain("Affected tasks that already startedLay the schemaunloaded-card");
    expect(document.body.textContent).toContain("Corrects task Lay the schema");
    button("Apply")!.click();
    await settled();
    expect(edits).toEqual([{ action: "apply_revision", sprintId: "sp", revisionId: "sr_1", expectedGraphRevision: REV }]);
    button("Reject")!.click();
    await settled();
    expect(edits[1]).toEqual({ action: "reject_revision", sprintId: "sp", revisionId: "sr_1" });
  });

  it("shows the revision history in the sprint Summary without owner buttons", async () => {
    await open(`/m/${LOCAL_MACHINE_ID}/p/p/s/sp/summary`, () => graph("applied", true));
    expect(document.body.textContent).toContain("Sprint change history");
    expect(document.body.textContent).toContain("Applied");
    expect(button("Apply")).toBeUndefined();
  });
});
