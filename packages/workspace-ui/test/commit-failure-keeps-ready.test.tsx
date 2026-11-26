import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, type ProjectFacts } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

it("a refused commit keeps the Ready panel behind the dialog through a refresh that lacks the ready state", async () => {
  const s = withTask(emptyWorkspace(), "Shared task"), t = s.tasks[0];
  s.machines[0].connection = "connected";
  const ready = { status: "ready_to_merge", result: { version: 1, digest: "d", summary: "Done", checks: [{ name: "Test", outcome: "passed" }] }, workspaceDigest: "d",
    acceptance: { version: 1, digest: "d", at: 1, open: {} },
    integrationFacts: { targetBranch: "main", alreadyInTarget: false, sameBranch: true, commitAvailable: true, pendingPaths: ["mine.txt"], otherUncommitted: false, reason: null, pushStatus: "unknown" } };
  Object.assign(t, ready);
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", "/m/local/t/t/changes");
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  // The diff panel is the Changes tab without the Ready panel's own section: record every frame in which it shows.
  const readyShown = () => !!document.querySelector('section[aria-label="Finish task"]');
  const frames: boolean[] = [];
  const watch = new MutationObserver(() => frames.push(readyShown()));
  expect(readyShown()).toBe(true);
  [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => /^Commit to/.test(b.textContent ?? ""))!.click(); await settle();
  const confirm = [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Confirm commit to main");
  expect(confirm).toBeTruthy();
  watch.observe(document.body, { subtree: true, childList: true });
  confirm!.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: "integrate" });
  p.receipt(false, undefined, "unsupported", "Refused"); await settle();
  // The refresh after the refusal: a board read without the overlay that lifts the task to Ready to merge.
  const raw = structuredClone(s); Object.assign(raw.tasks[0], { status: "review", integrationFacts: undefined, acceptance: null });
  p.emit(projection("2", raw)); await settle();
  expect(readyShown()).toBe(true);
  // The facts read that follows confirms the ready state again.
  const facts: ProjectFacts = { projectId: "p", openBranch: "main", defaultBranch: "main", repo: null, workers: [], reviews: [], sprints: [],
    overlays: { t: ready } as unknown as ProjectFacts["overlays"] };
  p.emit({ kind: "facts", facts }); await settle();
  expect(readyShown()).toBe(true);
  watch.disconnect();
  expect(frames.every(Boolean)).toBe(true);
});
