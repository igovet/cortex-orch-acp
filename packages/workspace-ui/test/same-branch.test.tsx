import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace } from "@cortex/workspace-domain";
import { archiveRow } from "../src/archive-model";
import { buildIndex } from "../src/selectors";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

it("a local task commit is not labelled Merged in Archive", () => {
  const s = withTask(emptyWorkspace(), "Committed");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], { status: "done", work: "merged", integration: { method: "commit", targetBranch: "main", digest: "d" }, archive: { at: 1, previousStatus: "done" } });
  const index = buildIndex(s, 2), row = archiveRow(index, index.byId.get("t")!);
  expect(row.outcomeText).toBe("Committed");
  expect(row.sub).not.toMatch(/Merged/);
});

for (const tab of ["", "/changes/diff"]) it(`a ready result can be explicitly accepted from ${tab || "conversation"}`, async () => {
  const s = withTask(emptyWorkspace(), "Ready result");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], { status: "ready_to_merge", result: { version: 1, digest: "d", summary: "Done", checks: [{ name: "Test", outcome: "passed" }] }, workspaceDigest: "d", acceptance: null });
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", "/m/local/t/t" + tab);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  const accept = [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Accept v1");
  expect(accept).toBeTruthy(); accept!.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: "accept_result", taskId: "t", digest: "d" });
  expect(p.commands.some(c => c.payload.type === "integrate")).toBe(false);
});

for (const status of ["review", "done"] as const) it(`shared-branch ${status} state has one honest inspector label`, async () => {
  const s = withTask(emptyWorkspace(), "Shared result"), task = s.tasks[0];
  s.machines[0].connection = "connected";
  Object.assign(task, { status, result:{version:1,digest:"d",summary:"Work summary:" + String.fromCharCode(10,10) + "> Disposable" + String.fromCharCode(92) + "_change inspected.",checks:[]}, workspaceDigest:"d" });
  if (status === "done") Object.assign(task, { work:"merged",integration:{method:"commit",targetBranch:"main",digest:"d"} });
  const p = scriptedPort({capabilities:{everything:true,operations:[]}});
  history.replaceState(null,"","/m/local/t/t/changes/diff");
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port}/>,root);
  await settle(); p.emit(projection("1",s)); await settle();
  expect(root.querySelector(".rv-sum")?.textContent).toContain("Disposable_change inspected.");
  const chip = root.querySelector(".tb-actions .chip");
  expect(chip).not.toBeNull();
  expect(chip!.textContent).not.toContain("Ready to merge");
  if (status === "review") expect(chip!.textContent).toContain("Ready to finish");
  const state = root.querySelector(".inspector .ins-sec")?.textContent ?? "";
  if (status === "done") expect(state.match(/Committed to main/g)).toHaveLength(1);
});

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
it("the board calls a shared-branch result Ready to finish, never Merge", async () => {
  const s = withTask(emptyWorkspace(), "Shared card");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], { status: "ready_to_merge" });
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", "/m/local/p/p/board");
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  expect(root.querySelector('[data-col="merge"] .col-h')?.textContent).toContain("Ready to finish");
  expect(root.querySelector(".card")?.textContent).not.toMatch(/Merge/);
});
for (const accepted of [false, true]) for (const known of [false, true]) it(`same branch has no integration fiction (accepted=${accepted}, known=${known})`, async () => {
  const s = withTask(emptyWorkspace(), "Shared task"), t = s.tasks[0];
  s.machines[0].connection = "connected";
  Object.assign(t, { status: "ready_to_merge", result: { version: 1, digest: "d", summary: "Done", checks: [{ name: "Test", outcome: "passed" }] }, workspaceDigest: "d",
    acceptance: accepted ? { version: 1, digest: "d", at: 1, open: {} } : null });
  if (known) Object.assign(t, { integrationFacts: { targetBranch: "main", alreadyInTarget: false, sameBranch: true, commitAvailable: true, pendingPaths: ["mine.txt"], otherUncommitted: true, reason: null, pushStatus: "unknown" } });
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", "/m/local/t/t/changes");
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  const labels = () => [...root.querySelectorAll("button")].map(b => b.textContent?.trim());
  expect(labels().some(x => /merge|squash|create PR/i.test(x ?? ""))).toBe(false);
  expect(root.querySelector(".flow")).toBeNull();
  expect(root.querySelector(".phase-bar")?.textContent ?? "").not.toMatch(/Merged/);
  expect(root.querySelector(".ready")?.textContent).not.toMatch(/merge|squash|pull request/i);
  expect(root.querySelector(".inspector")?.textContent).not.toMatch(/\bmerge[sd]?\b/i);
  expect(root.querySelector(".task-row[aria-current=page] .sig")?.textContent).toBe("Finish");
  if (known) {
    if (!accepted) expect(labels()).toContain("Accept v1");
    expect(root.textContent).toContain("Other tasks");
    const commit = [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Commit to main…")!;
    expect(commit).toBeTruthy(); commit.click(); await settle();
    const dialog = root.querySelector("dialog[open]")!;
    expect(dialog.textContent).toContain("Commit to main");
    expect(dialog.textContent).not.toMatch(/merge|squash|pull request|From/);
    expect(p.commands).toHaveLength(0);
    [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Confirm commit to main")!.click(); await settle();
    if (!accepted) { expect(p.commands.at(-1)?.payload.type).toBe("accept_result"); p.receipt(true); await settle(); }
    expect(p.commands.at(-1)?.payload).toMatchObject({ type: "integrate", method: "commit", targetBranch: "main", confirmed: true });
  } else expect(labels()).toContain("Close as is…");
});
