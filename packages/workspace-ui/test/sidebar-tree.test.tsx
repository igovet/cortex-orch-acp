import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Sprint } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const world = () => { const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected"; return s; };
async function mount(state: ReturnType<typeof world>, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const sprintOf = (id: string, runMode: Sprint["runMode"], taskIds: string[]): Sprint =>
  ({ id, projectId: "p", title: `${id} sprint`, description: "", runMode, targetBranch: null, planning: "created", conversation: [],
    plan: null, materializedVersion: 1, taskIds, queueState: "running", pauseReason: null, planningTaskId: "plan" });
const hrefs = (parent: ParentNode) => [...parent.querySelectorAll<HTMLAnchorElement>(".task-row")].map(a => a.textContent);

it.each(["hybrid", "automatic"] as const)("lists a running %s sprint and its tasks as an indented tree in Active work", async mode => {
  const s = world(), base = s.tasks[0];
  s.tasks = [
    { ...base, id: "plan", title: "Planning run", status: "closed" as never, sprintId: null },
    { ...base, id: "a", title: "First step", status: "working", sprintId: "sp" },
    { ...base, id: "b", title: "Second step", status: "draft", sprintId: "sp" },
  ];
  s.sprints = [sprintOf("sp", mode, ["a", "b"])];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const block = document.querySelector<HTMLElement>(".sb-work")!;
  expect(block).not.toBeNull();
  expect(block.querySelector(".sb-work-group")?.textContent).toBe("Sprints in progress");
  const rows = [...block.querySelectorAll<HTMLElement>(".task-row")];
  expect(rows.map(r => r.querySelector(".trunc")?.textContent)).toEqual(["Sprint: sp sprint", "First step"]);
  expect(rows[0].querySelector(".sb-row-project")?.textContent).toBe(s.projects[0].name);
  expect(rows[1].classList.contains("in-sprint")).toBe(true);
  expect(rows[0].classList.contains("in-sprint")).toBe(false);
});

it("expands and folds a project's tree from its chevron, and remembers it", async () => {
  const s = world(), base = s.tasks[0];
  s.tasks = [{ ...base, id: "a", title: "First step", status: "working", sprintId: "sp" }];
  s.sprints = [sprintOf("sp", "hybrid", ["a"])];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const tree = () => document.querySelector(".sidebar .proj-wrap + .tasks");
  const toggle = () => document.querySelector<HTMLButtonElement>(".sidebar .proj-x")!;
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  expect(tree()?.querySelectorAll(".task-row:not(.all)").length).toBe(2);
  toggle().click(); await settle();
  expect(toggle().getAttribute("aria-expanded")).toBe("false");
  expect(tree()).toBeNull();
  toggle().click(); await settle();
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  expect(tree()?.querySelectorAll(".task-row:not(.all)").length).toBe(2);
});

it("opens a collapsed project tree when its name is clicked as well as navigating", async () => {
  const s = world(), base = s.tasks[0];
  s.tasks = [{ ...base, id: "a", title: "First step", status: "working" }];
  localStorage.setItem("cw.ws.expanded", JSON.stringify({ p: false }));
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const tree = () => document.querySelector(".sidebar .proj-wrap + .tasks");
  expect(tree()).toBeNull();
  document.querySelector<HTMLAnchorElement>(".sidebar .proj")!.click(); await settle();
  expect(tree()).not.toBeNull();
});
