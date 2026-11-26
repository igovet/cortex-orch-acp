import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type OwnerAction } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

/** An owner card about a task is also shown inside that task's page, so "Open task" lands on it. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const action: OwnerAction = { projectId: "p", projectName: "Project", taskId: "t", task: "Work", kind: "access", service: "github", actionId: "oa1",
  title: "Sign in to GitHub", detail: "The task needs the GitHub CLI signed in.", recheck: true, openedAt: Date.now(), checkedAt: null, lastStatus: "not_logged_in", terminal: null, token: null } as OwnerAction;

async function mount(path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  (s.port as any).ownerActions = { list: async () => ({ actions: [action], phoneTokens: true }), act: async () => ({ outcome: "resolved", how: "dismissed" }), token: async () => ({ outcome: "resolved", how: "token" }) };
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  const state = withTask(emptyWorkspace(), "Work"); state.machines[0].connection = "connected";
  await settle(); s.emit(projection("1", state)); await settle();
}
const cards = () => document.querySelectorAll('[role="dialog"][aria-label="Sign in to GitHub"]');

it("a task's open owner action is shown once inside that task's page, with its actions", async () => {
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t`);
  expect(cards().length).toBe(1);
  expect(document.querySelector(".owner-in-task")?.contains(cards()[0]!)).toBe(true);
  expect(cards()[0]!.textContent).toContain("The task needs the GitHub CLI signed in.");
  expect([...cards()[0]!.querySelectorAll("button")].map(b => b.textContent?.trim())).toContain("Dismiss");
});

it("away from the task the action stays in the floating stack", async () => {
  await mount(`/m/${LOCAL_MACHINE_ID}`);
  expect(cards().length).toBe(1);
  expect(document.querySelector(".owner-in-task")).toBeNull();
});
