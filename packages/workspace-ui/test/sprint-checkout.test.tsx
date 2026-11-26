import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, LOCAL_MACHINE_ID, type Task, type WorkspaceCommand } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { buildIndex, homeView } from "../src/selectors";
import { sessionModel, stateChip } from "../src/session-model";
import { pagesWorld } from "./pages-world";
import { flush, projection, scriptedPort } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const named = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
async function mount(path: string, hasGit = true, worktrees = true) {
  const p = scriptedPort({ capabilities: { everything: worktrees, operations: worktrees ? [] : ["workspace.create_sprint", "tasks.create"] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/${path}`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root); await settle();
  const world = pagesWorld();
  world.projects[0].hasGit = hasGit;
  world.machines[0].config.surfaces.user_defaults.checkout = "worktree";
  world.sprints[0].checkout = { kind: "worktree" };
  p.emit(projection("1", world)); await settle();
  return { p, world };
}
const choose = async (kind: "main" | "worktree") => {
  const input = document.querySelectorAll<HTMLInputElement>('input[name="checkout"]')[kind === "main" ? 1 : 0];
  input.click(); await settle();
};

it.each(["main", "worktree"] as const)("saves %s for a new sprint using the same composer and checkout controls as a task", async kind => {
  const { p } = await mount("new?kind=sprint&project=p");
  expect(document.querySelector('input[name="checkout"]:checked')?.closest("label")?.textContent).toContain("Worktree");
  await choose(kind);
  expect(named("Attach files")).toBeDefined();
  expect(named("Voice input")).toBeDefined();
  const area = document.querySelector<HTMLTextAreaElement>(".mc textarea")!;
  area.value = "Make these two changes"; area.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  named("Start planning").click(); await settle();
  expect(p.commands.map(c => c.payload)).toContainEqual(expect.objectContaining({ type: "create_sprint", checkout: kind === "main" ? { kind } : { kind, shared: true }, description: "Make these two changes" }));
  p.receipt(false); await settle();
  expect(area.value).toBe("Make these two changes");
  expect(document.querySelector('input[name="checkout"]:checked')?.closest("label")?.textContent).toContain(kind === "main" ? "Project folder" : "Worktree");
  expect(p.commands.some(c => (c.payload as WorkspaceCommand).type === "start_task")).toBe(false);
});

it("explains the three sprint choices and shows the service refusal on the sprint", async () => {
  const { p, world } = await mount("new?kind=sprint&project=p");
  const labels = [...document.querySelectorAll('input[name="sprint-run"]')].map(el => el.closest("label")!.textContent);
  expect(labels).toMatchObject([expect.stringContaining("You start each ready task"), expect.stringContaining("You confirm each merge"),
    expect.stringContaining("Done on their own in the current branch")]);

  dispose?.(); document.body.innerHTML = "";
  const sprint = await mount("p/p/s/sp/plan"), state = structuredClone(sprint.world);
  Object.assign(state.sprints[0], {runMode:"automatic",planning:"created",queueState:"paused", taskIds:["not-yet-loaded"],
    automaticQueue:{kind:"start_refused",taskId:null,code:"native_worktree_unavailable",reason:"Automatic needs the project folder.",needsOwner:false}});
  sprint.p.emit(projection("2",state)); await settle();
  expect(document.querySelector('[role="status"]')?.textContent).toContain("Automatic needs the project folder.");
  const mode = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Sprint run mode"] button')].find(b => b.textContent === "Automatic")!;
  expect(document.getElementById(mode.getAttribute("aria-describedby")!.split(" ")[0])?.textContent).toContain("work questions and permission requests");
  expect(p.commands).toEqual([]);
  expect(world.sprints[0].runMode).toBe("manual");
});

it("offers Continue for dependency holds and a task link at the retry limit", async () => {
  const { p } = await mount("p/p/s/sp/plan");
  const state = pagesWorld("running"), sprint = state.sprints[0], [taskId] = sprint.taskIds;
  Object.assign(sprint, {runMode:"automatic",planning:"created",queueState:"paused",
    automaticQueue:{kind:"continuation",taskId,code:"continuation_cap",reason:"Open the task to resume",needsOwner:true}});
  p.emit(projection("2", state)); await settle();
  const button = () => [...document.querySelectorAll("button")].find(el => el.textContent?.trim() === "Continue queue")!;
  expect(button().getAttribute("aria-disabled")).toBe("true");
  expect([...document.querySelectorAll("a")].some(el => el.textContent === "Open task" && el.href.includes(taskId))).toBe(true);

  const blocked = structuredClone(state);
  blocked.sprints[0].automaticQueue = {kind:"settlement_blocked",taskId,code:"dependencies_blocked",reason:"Check dependencies",needsOwner:true};
  p.emit(projection("3", blocked)); await settle();
  expect(button().getAttribute("aria-disabled")).not.toBe("true");
  button().click(); await settle();
  expect(p.commands.map(c => c.payload)).toContainEqual(expect.objectContaining({type:"run_sprint",sprintId:sprint.id}));
});

it("shows only work decisions and exact integration holds in Automatic attention", () => {
  const state = pagesWorld("running"), sprint = state.sprints[0], [id, other] = sprint.taskIds;
  const task = state.tasks.find(t => t.id === id)!;
  const change = (patch: Partial<Task>) => Object.assign(task, patch);
  const index = () => buildIndex(state, 0);
  const view = () => index().byId.get(id)!;
  const attention = () => homeView(index(), LOCAL_MACHINE_ID).attention.filter(v => sprint.taskIds.includes(v.id));
  const chip = () => stateChip(sessionModel(index(), id)!);
  change({status:"ready_to_merge",process:"stopped",acceptance:{automatic:true,version:1,digest:"d"}});
  expect(view().column).toBe("work");
  expect(chip().text).toMatch(/awaiting automatic finish/);
  expect(attention()).toEqual([]);
  sprint.automaticQueue = {kind:"owner_integration",taskId:other,code:"different_target",reason:"Confirm another target",needsOwner:true};
  expect(view().column).toBe("work");
  sprint.automaticQueue.taskId = id;
  expect(attention().map(v => v.id)).toEqual([id]);
  expect(chip().text).toBe("Needs you");

  delete sprint.automaticQueue;
  change({status:"paused",process:"stopped",automaticContinuation:{policy:"automatic_continuation",continuationCount:8,
    lastContinuationReason:"Verification failed",pause:{eventId:"evt",kind:"continuation_cap",reason:"coordinator_no_progress_limit"}}});
  expect(chip().text).toBe("The coordinator stopped making progress");
  expect(attention()).toEqual([]);
  change({status:"working",process:"live",automaticContinuation:{...task.automaticContinuation!,pause:null}});
  expect(chip().text).toBe("Automatic continuation: Verification failed");
  const session = emptySession(id);
  state.sessions = [...state.sessions.filter(s => s.taskId !== id), session];
  for (const kind of ["question", "permission"] as const) {
    session.decisions = [{id:"q",kind,text:"Recorded request",from:"Cortex",workerId:null,at:null,options:[],answer:null,withdrawn:false}];
    expect(attention().map(v => v.id)).toEqual([id]);
    expect(view().ownerDecision?.kind).toBe(kind);
  }
  change({status:"done",process:"stopped",integration:{method:"commit",targetBranch:"dev",digest:"d",automatic:true,committed:false}});
  expect(view().column).toBe("done");
  expect(chip().text).toBe("Done · Already in dev");
  expect(attention()).toEqual([]);

  sprint.runMode = "hybrid";
  change({status:"ready_to_merge",integration:null,automaticContinuation:undefined});
  expect(view().column).toBe("merge");
});

it("disables the worktree choice on a non-Git project even on a supporting service", async () => {
  await mount("new?kind=task&project=p", false, true);
  const input = document.querySelector<HTMLInputElement>('input[name="checkout"]')!;
  expect(input.disabled).toBe(true);
  expect(input.closest("label")?.textContent).toContain("needs a Git repository");
});

it("offers one worktree for the whole sprint by default and per-task worktrees as Manual only", async () => {
  const { p } = await mount("new?kind=sprint&project=p");
  const scope = () => [...document.querySelectorAll<HTMLInputElement>('input[name="worktree-scope"]')];
  const mode = (value: string) => [...document.querySelectorAll<HTMLInputElement>('input[name="sprint-run"]')].find(input => input.closest("label")?.querySelector(".t")?.textContent === value)!;
  expect(scope().map(i => i.closest("label")?.querySelector(".t")?.textContent)).toEqual(["One worktree for the whole sprint", "Separate worktree per task"]);
  expect(scope()[0].checked).toBe(true);
  mode("Hybrid").click(); await settle();
  expect(mode("Hybrid").checked).toBe(true);
  scope()[1].click(); await settle();
  expect(mode("Manual").checked).toBe(true);
  expect(mode("Hybrid").disabled).toBe(true);
  expect(mode("Automatic").disabled).toBe(true);
  expect(mode("Hybrid").closest("label")?.getAttribute("data-why")).toMatch(/Manual mode only/);
  const area = document.querySelector<HTMLTextAreaElement>(".mc textarea")!;
  area.value = "Per task"; area.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  named("Start planning").click(); await settle();
  expect(p.commands.map(c => c.payload)).toContainEqual(expect.objectContaining({ type: "create_sprint", runMode: "manual", checkout: { kind: "worktree", shared: false } }));
});

it("keeps the sprint worktree available with Automatic and Hybrid", async () => {
  await mount("new?kind=sprint&project=p");
  const mode = (value: string) => [...document.querySelectorAll<HTMLInputElement>('input[name="sprint-run"]')].find(input => input.closest("label")?.querySelector(".t")?.textContent === value)!;
  const inputs = () => [...document.querySelectorAll<HTMLInputElement>('input[name="checkout"]')];
  for (const value of ["Automatic", "Hybrid"]) {
    mode(value).click(); await settle();
    expect(inputs()[0].disabled).toBe(false);
    expect(inputs()[0].checked).toBe(true);
    expect(mode(value).checked).toBe(true);
  }
});

it.each([false, true])("selects Project folder when worktrees are unsupported (Git: %s)", async hasGit => {
  await mount("new?kind=sprint&project=p", hasGit, false);
  const inputs = [...document.querySelectorAll<HTMLInputElement>('input[name="checkout"]')];
  expect(inputs[0].disabled).toBe(true);
  expect(inputs[1].checked).toBe(true);
});

it.each(["planning", "summary"] as const)("shows the saved choice on the sprint %s page and keeps legacy mode unknown", async tab => {
  const { p, world } = await mount(`p/p/s/sp/${tab}`);
  expect(document.querySelector('[aria-label="Sprint checkout"]')?.textContent).toBe("Tasks: Separate worktree per task");
  const updated = structuredClone(world);
  updated.sprints[0].checkout = { kind: "main" };
  p.emit(projection("2", updated)); await settle();
  expect(document.querySelector('[aria-label="Sprint checkout"]')?.textContent).toBe("Tasks: Project folder");
  const legacy = structuredClone(updated);
  delete legacy.sprints[0].checkout;
  p.emit(projection("3", legacy)); await settle();
  expect(document.querySelector('[aria-label="Sprint checkout"]')).toBeNull();
});

it("shows the sprint checkout and limits a per-task worktree sprint to Manual", async () => {
  const { p, world } = await mount("p/p/s/sp/plan");
  const buttons = () => [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Sprint run mode"] button')];
  expect(document.querySelector('[aria-label="Sprint checkout"]')?.textContent).toContain("Separate worktree per task");
  expect(buttons().filter(b => b.getAttribute("aria-disabled") === "true").map(b => b.textContent)).toEqual(["Hybrid", "Automatic"]);
  const shared = structuredClone(world);
  shared.sprints[0].checkout = { kind: "worktree", shared: true };
  p.emit(projection("3", shared)); await settle();
  expect(document.querySelector('[aria-label="Sprint checkout"]')?.textContent).toContain("One worktree for the whole sprint");
  expect(buttons().some(b => b.getAttribute("aria-disabled") === "true" && b.dataset.why)).toBe(false);
});

it("offers the one sprint branch merge once every task of a shared worktree sprint is done", async () => {
  const { p, world } = await mount("p/p/s/sp/plan");
  const merge = () => [...document.querySelectorAll("button")].find(b => b.textContent === "Merge sprint branch…");
  const state = structuredClone(world);
  state.sprints[0].checkout = { kind: "worktree", shared: true };
  state.sprints[0].planning = "created";
  state.sprints[0].taskIds = ["c-draft", "c-work"];
  const members = state.tasks.filter(t => t.id === "c-draft" || t.id === "c-work");
  members.forEach(t => { t.sprintId = "sp"; });
  expect(members.length).toBeGreaterThan(0);
  members.forEach(t => { t.status = "done"; });
  p.emit(projection("4", state)); await settle();
  expect(merge()).toBeDefined();
  state.sprints[0].merge = { state: "merged", target: "main", branch: "cw/x", commit: null, at: 1 };
  p.emit(projection("5", state)); await settle();
  expect(merge()).toBeUndefined();
  expect(document.body.textContent).toContain("Merged into main");
});
