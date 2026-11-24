import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

it.each([1, 2])("renders ordinary option details in a %i-question form", async count => {
  const state = withTask(emptyWorkspace(), "Fixture task");
  state.machines[0].connection = "connected";
  state.tasks[0].status = "needs_you"; state.tasks[0].process = "live";
  const session = emptySession("t");
  session.decisions = Array.from({ length: count }, (_, i) => ({
    id: `d${i}`, kind: "question" as const, from: "", workerId: null, at: null,
    withdrawn: false, answer: null, text: `Question ${i}`,
    options: [{ label: "Proceed", sub: "Keeps the current files", optionId: "a" }, { label: "Stop", optionId: "b" }],
    ...(count > 1 ? { batchId: "batch" } : {}),
  }));
  session.timeline = session.decisions.map(d => ({ id: `e-${d.id}`, kind: "decision" as const, decisionId: d.id }));
  state.sessions = [session];
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  for (let i = 0; i < 8; i++) await flush();
  p.emit(projection("1", state));
  for (let i = 0; i < 8; i++) await flush();
  const details = [...document.querySelectorAll(".dc-opts-rows .opt .sub")];
  expect(details).toHaveLength(count);
  for (const detail of details) {
    expect(detail.textContent).toBe("Keeps the current files");
    expect(detail.previousElementSibling?.textContent).toBe("Proceed");
  }
});
