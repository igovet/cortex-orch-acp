import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

// All archive controls use the same confirmation and send one non-destructive command.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
async function mount(s: WorkspaceState, path: string) {
  s.machines[0].connection = "connected";
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  return p;
}
const done = () => {
  const s = withTask(emptyWorkspace(), "Finished"); const t = s.tasks[0];
  Object.assign(t, { status: "done", doneAt: 1000, work: "unmerged", workFacts: { commits: 2, files: 3, sizeMb: 4 } });
  return s;
};
const text = () => document.body.textContent ?? "";

it("Archive on a done task with unmerged work sends one command and opens no dialog", async () => {
  const p = await mount(done(), `/m/${LOCAL_MACHINE_ID}/p/p/board`);
  document.querySelector<HTMLElement>('[aria-label="Actions for Finished"]')!.click(); await settle();
  const archive = [...document.querySelectorAll<HTMLElement>('#m-task [role="menuitem"]')].find(b => b.textContent?.trim().startsWith("Archive"))!;
  expect(archive.getAttribute("aria-disabled")).not.toBe("true");
  archive.click(); await settle();
  expect(p.commands).toHaveLength(1);
  expect(p.commands[0].payload).toMatchObject({ type: "archive_task", taskId: "t" });
  p.receipt(); await settle();
  expect(p.commands).toHaveLength(1);
  expect(document.querySelector("dialog[open]")).toBeNull();
  expect(text()).not.toMatch(/has unmerged work|Keep the work|Delete the work and archive/);
});

it("confirms an unfinished task from the title band and sends nothing on Cancel", async () => {
  const s = done();
  Object.assign(s.tasks[0], { status: "working", process: "live", doneAt: null });
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/t/t`);
  const archive = document.querySelector<HTMLButtonElement>(".tb-archive")!;
  expect(archive.textContent).toContain("Archive…");
  archive.click(); await settle();
  let dialog = document.querySelector<HTMLDialogElement>("dialog[open]")!;
  expect(dialog.textContent).toContain("Move it to the Archive without checking its result?");
  expect(p.commands).toHaveLength(0);
  [...dialog.querySelectorAll("button")].find(b => b.textContent?.trim() === "Cancel")!.click();
  await settle();
  expect(p.commands).toHaveLength(0);
  archive.click(); await settle();
  dialog = document.querySelector<HTMLDialogElement>("dialog[open]")!;
  [...dialog.querySelectorAll("button")].find(b => b.textContent?.trim() === "Archive")!.click();
  await settle();
  expect(p.commands.map(c => c.payload)).toEqual([expect.objectContaining({ type: "archive_task", taskId: "t" })]);
  p.receipt(); await settle();
  expect(document.querySelector("dialog[open]")).toBeNull();
});

it("shows an unfinished outcome and the unconfirmed stop in Archive", async () => {
  const s = done();
  Object.assign(s.tasks[0], { status: "working", doneAt: null, process: "unknown",
    archive: { at: 1000, previousStatus: "working", unfinished: true, stopUnconfirmed: true } });
  await mount(s, `/m/${LOCAL_MACHINE_ID}/archive`);
  expect(text()).toContain("Unfinished");
  expect(text()).toContain("Process stop was not confirmed.");
});

it("shows a closed-unverified task in the Archive rather than the board", async () => {
  const s = done();
  Object.assign(s.tasks[0], { status: "closed_unverified", closedAs: "unconfirmed", process: "unknown",
    archive: { at: 1000, previousStatus: "closed_unverified", unfinished: true } });
  await mount(s, `/m/${LOCAL_MACHINE_ID}/archive`);
  expect(text()).toContain("Closed by you · Not verified");
  expect(document.querySelector('[aria-label="Select Finished"]')).not.toBeNull();
});

it("a saved record that still carries the retired question shows no question anywhere", async () => {
  const s = done();
  Object.assign(s.tasks[0], { archiveQuestion: true, archiveAskAfter: 5 });
  await mount(s, `/m/${LOCAL_MACHINE_ID}/archive`);
  expect(text()).not.toMatch(/Waiting for your decision|Decide…|unmerged work|wait for you/i);
  await mount(s, `/m/${LOCAL_MACHINE_ID}/inbox`);
  expect(text()).not.toMatch(/holding this task out of the Archive|Unmerged work/);
});
