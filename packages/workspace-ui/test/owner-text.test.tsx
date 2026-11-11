import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Sprint, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { ResultText } from "../src/result-text";
import { draftTask, flush, projection, scriptedPort } from "./support";

const OWNER = "1. Make it simpler\n2. Load everything, **without** a limit\n\nFirst line\nSecond line\n\n<img src=x onerror=alert(1)>";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];

function world(patch: Partial<Sprint> | null): WorkspaceState {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push({ id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" });
  state.tasks.push({ ...draftTask(), description: OWNER });
  if (patch) state.sprints.push({ id: "sp", projectId: "p", description: OWNER, title: "A sprint", runMode: "manual", targetBranch: null, planning: "ready", conversation: [], plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null, ...patch });
  return state;
}
async function mount(state: WorkspaceState, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settled();
  s.emit(projection("1", state));
  await settled();
}
const expectFormatted = (root: Element) => {
  expect(root.querySelectorAll("ol li")).toHaveLength(2);
  expect(root.querySelector("ol strong")?.textContent).toBe("without");
  expect(root.querySelectorAll("br").length).toBeGreaterThan(0);
  expect(root.querySelector("img")).toBeNull();
  expect(root.textContent).toContain("<img src=x onerror=alert(1)>");
};

describe("text the owner wrote", () => {
  it("keeps a single line break when asked, and renders the rest as Markdown", () => {
    const root = document.createElement("div");
    document.body.append(root);
    dispose = render(() => <ResultText breaks text={OWNER} />, root);
    expectFormatted(root);
    expect([...root.querySelectorAll("p")].find(p => p.textContent?.startsWith("First"))?.innerHTML).toBe("First line<br>Second line");
  });

  it("still joins a report's wrapped lines into one paragraph", () => {
    const root = document.createElement("div");
    document.body.append(root);
    dispose = render(() => <ResultText text={"One\ntwo"} />, root);
    expect(root.querySelector("p")?.textContent).toBe("One two");
    expect(root.querySelector("br")).toBeNull();
  });

  it("shows a task description with its line breaks and Markdown", async () => {
    await mount(world(null), `/m/${LOCAL_MACHINE_ID}/t/t`);
    expectFormatted($(".brief")!);
  });

  it("shows a sprint request, the planner's notes and the owner's messages with their line breaks and Markdown", async () => {
    await mount(world({ conversation: [{ kind: "note", text: "Planner one\nPlanner two\n\n- **first**" }, { kind: "message", text: "Owner one\nOwner two" }] }), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`);
    expectFormatted($(".brief")!);
    expect($(".sp-conv .ai")?.querySelector("p")?.innerHTML).toBe("Planner one<br>Planner two");
    expect($(".sp-conv .ai strong")?.textContent).toBe("first");
    expect($(".sp-conv .mine p")?.innerHTML).toBe("Owner one<br>Owner two");
  });

  it("renders Read more as Markdown, keeping line breaks in the description and the acceptance", async () => {
    await mount(world({ plan: { version: 1, items: [{ key: "a", title: "Write the brief", description: "# Goal\n\nFirst\nSecond\n\n1. Inspect\n2. Change", acceptance: "- Links work\n- `pnpm test` passes", dependsOn: [] }] } }), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/plan`);
    all(".ws-more-l")[0].click();
    await settled();
    const [description, acceptance] = all(".ws-read-d");
    expect(description.querySelector("h1")?.textContent).toBe("Goal");
    expect(description.querySelector("p")?.innerHTML).toBe("First<br>Second");
    expect(description.querySelectorAll("ol li")).toHaveLength(2);
    expect(acceptance.querySelectorAll("li")).toHaveLength(2);
    expect(acceptance.querySelector("code")?.textContent).toBe("pnpm test");
  });
});
