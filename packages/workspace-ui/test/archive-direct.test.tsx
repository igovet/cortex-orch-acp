import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

// Archiving is one command: the task goes to the Archive with its work and nothing asks.
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

it("a saved record that still carries the retired question shows no question anywhere", async () => {
  const s = done();
  Object.assign(s.tasks[0], { archiveQuestion: true, archiveAskAfter: 5 });
  await mount(s, `/m/${LOCAL_MACHINE_ID}/archive`);
  expect(text()).not.toMatch(/Waiting for your decision|Decide…|unmerged work|wait for you/i);
  await mount(s, `/m/${LOCAL_MACHINE_ID}/inbox`);
  expect(text()).not.toMatch(/holding this task out of the Archive|Unmerged work/);
});
