import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

it("a worktree task with no file changes shows the report, not a merge", async () => {
  const s = withTask(emptyWorkspace(), "Report task"), t = s.tasks[0];
  s.machines[0].connection = "connected";
  Object.assign(t.checkout, { kind: "worktree", branch: "cw/path-line-1" });
  Object.assign(t, { status: "ready_to_merge", result: { version: 1, digest: "d", summary: "Done", checks: [] }, workspaceDigest: "d",
    acceptance: { version: 1, digest: "d", at: 1, open: {} },
    integrationFacts: { targetBranch: "main", alreadyInTarget: true, noChanges: true, sameBranch: false, commitAvailable: false, pendingPaths: [], otherUncommitted: false, reason: null, pushStatus: "unknown" } });
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", "/m/local/t/t/changes");
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  expect(root.querySelector("[data-no-file-changes]")?.textContent).toContain("No file changes");
  expect([...root.querySelectorAll("button")].some(b => /^Merge/.test(b.textContent ?? ""))).toBe(false);
});
