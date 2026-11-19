import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Sprint, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { buildIndex, inScope, sprintView } from "../src/selectors";
import { capabilitiesWire, flush, projection, scriptedPort, scriptedTransport, withTask, wireTask } from "./support";
import { createRealPort } from "../src/port/real";
import { writeBoards } from "../src/port/board-cache";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>("button")];
const button = (text: string) => buttons().find(b => b.textContent?.trim() === text)!;
const world = () => { const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected"; return s; };
async function mount(state: ReturnType<typeof world>, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
const review = () => {
  const s = world(), t = s.tasks[0];
  t.status = "review"; t.workspaceDigest = "digest";
  t.result = { version: 1, digest: "digest", summary: "Implemented the change", checks: [{ name: "Focused tests", detail: "Needs another run", outcome: "failed", command: ["pytest"], rerunnable: true }],
    evidence: [{ reference: "r_f530581c44c2", digest: "a".repeat(64) }] };
  return s;
};

it("selects an offered usable route rather than a stale machine default", async () => {
  const s = world();
  s.machines[0].config.surfaces.user_defaults.coordinator = { engine: "claude-agent-acp", model: "opus[1m]", effort: "high", parameters: {} };
  s.options.models = [{ id: "codex-luna", label: "6 Luna", engine: "Codex", efforts: ["low"] }, { id: "offline", label: "Offline", engine: "Claude", unavailable: "Sign in first" }];
  await mount(s, `/m/${LOCAL_MACHINE_ID}/new?project=p`);
  expect(document.querySelector('[aria-label="Coordinator model: 6 Luna"]')).not.toBeNull();
});

it("leaves no unavailable route selected when every offer is unavailable", async () => {
  const s = world();
  s.options.models = [{ id: "offline", label: "Offline", engine: "Claude", unavailable: "Sign in first" }];
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/new?project=p`);
  const field = document.querySelector<HTMLTextAreaElement>('[aria-label="Describe the task"]')!;
  field.value = "Do this"; field.dispatchEvent(new Event("input", { bubbles: true }));
  document.querySelector<HTMLButtonElement>('[aria-label="Save draft"]')!.click();
  expect((p.commands[0].payload as { settings: { model: string } }).settings.model).toBe("");
});

it("keeps result evidence identities out of the rendered result", async () => {
  await mount(review(), taskPath);
  expect(document.querySelector(".rcard")).not.toBeNull();
  expect(document.querySelector(".rcard")!.textContent).not.toMatch(/r_f530581c44c2|a{64}/);
});

it("disables rerun while pending, blocks repeat input and releases on refusal", async () => {
  const p = await mount(review(), taskPath);
  const rerun = document.querySelector<HTMLButtonElement>('[aria-label="Rerun Focused tests"]')!;
  rerun.click(); rerun.click();
  expect(p.commands.filter(c => c.operation === "rerun_check")).toHaveLength(1);
  expect(rerun.disabled).toBe(true);
  expect(rerun.getAttribute("aria-busy")).toBe("true");
  p.receipt(false); await settle();
  expect(rerun.disabled).toBe(false);
});

it("disables Send and reopen for the entire request and preserves input on failure", async () => {
  const p = await mount(review(), taskPath);
  button("Request changes…").click(); await settle();
  const field = document.querySelector<HTMLTextAreaElement>("dialog textarea")!;
  field.value = "Please fix"; field.dispatchEvent(new Event("input", { bubbles: true }));
  const send = document.querySelector<HTMLButtonElement>('dialog [aria-label="Send and reopen"]')!; send.click(); send.click();
  expect(p.commands.filter(c => c.operation === "request_changes")).toHaveLength(1);
  expect(send.disabled).toBe(true);
  expect(send.getAttribute("aria-busy")).toBe("true");
  p.receipt(false); await settle();
  expect(send.disabled).toBe(false); expect(field.value).toBe("Please fix");
});

it("shows a worker's role and actual state without claiming it is unassigned", async () => {
  const s = world(); s.tasks[0].status = "working"; s.tasks[0].process = "live"; s.tasks[0].executionId = "e";
  const worker: Worker = { id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "completed", lane: "execution", goal: "Implement UI", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null };
  s.workers = [worker];
  await mount(s, taskPath + "/workers/w");
  expect(document.body.textContent).toContain("frontend-dev");
  expect(document.body.textContent).not.toMatch(/not assigned/i);
});

it("keeps a sprint whose queue is complete in the sidebar, as Done, until it is closed into the Archive", async () => {
  const s = world();
  // Four tasks ready to finish, as in the live run: the sprint row must not be the one pushed out of the five.
  s.tasks = ["t", "u", "v", "w"].map(id => ({ ...s.tasks[0], id, title: `Work ${id}`, status: "ready_to_merge" as const, sprintId: "sp" }));
  const sprint: Sprint = { id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t", "u", "v", "w"], queueState: "complete", pauseReason: null };
  s.sprints = [sprint];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const row = () => [...document.querySelectorAll<HTMLAnchorElement>(".sidebar .task-row")].find(r => r.textContent?.includes("Sprint: Release"));
  expect(row()?.querySelector(".sig")?.textContent).toBe("Done");
  expect(row()?.getAttribute("href")).toContain("/p/p/s/sp/board");
  expect(document.querySelectorAll(".sidebar .tasks .task-row:not(.all)")).toHaveLength(5);
  // Closed into the Archive, it leaves the sidebar.
  dispose?.(); document.body.innerHTML = "";
  s.sprints = [{ ...sprint, archivedAt: Date.now() }];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  expect(row()).toBeUndefined();
});

it("keeps two automatic task titles in the sidebar while showing continuation as status", async () => {
  const s = world();
  s.tasks = [
    { ...s.tasks[0], id: "t", title: "Deploy_api gateway", status: "working", process: "live", automaticContinuation: { policy: "automatic_continuation", continuationCount: 1, lastContinuationReason: "review_incomplete", pause: null } },
    { ...s.tasks[0], id: "u", title: "Verify_api gateway", status: "working", process: "live", automaticContinuation: { policy: "automatic_continuation", continuationCount: 2, lastContinuationReason: "review_incomplete", pause: null } },
  ];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const rows = [...document.querySelectorAll<HTMLElement>(".sidebar .task-row")].filter(r => r.querySelector(".sig") && !r.textContent?.startsWith("Sprint:"));
  expect(rows.map(r => r.querySelector(".trunc")?.textContent)).toEqual(["Deploy_api gateway", "Verify_api gateway"]);
  expect(rows.every(r => !r.querySelector(".sig")?.textContent?.includes("_"))).toBe(true);
  expect(rows.every(r => r.querySelector(".sig")?.textContent === "Working")).toBe(true);
  expect(rows.map(r => r.getAttribute("aria-label"))).toEqual(["Deploy_api gateway — Working", "Verify_api gateway — Working"]);
});

it("uses the editor's sprint membership for the scoped board and grouping", async () => {
  const s = world();
  const sprint: Sprint = { id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "idle", pauseReason: null };
  s.sprints = [sprint]; // Older task projections may omit the reverse link.
  const index = buildIndex(s, Date.now()), summary = index.summaries.get("p")!;
  expect(inScope(summary, { kind: "sprint", id: "sp" }).map(v => v.id)).toEqual(sprintView(index, sprint).views.map(v => v.id));
  expect(index.byId.get("t")!.sprint?.id).toBe("sp");
  expect(inScope(summary, { kind: "standalone" })).toHaveLength(0);
  await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  expect(document.querySelector('.card[data-task="t"]')).not.toBeNull();
});

it("renders a retained 31-task sprint immediately while first reads fail and refreshes without startup error copy", async () => {
  const items=Array.from({length:31},(_,i)=>wireTask({taskId:"saved-"+i,description:"Retained task "+i}));
  const product={tasks:{},sprints:[{id:"sp",projectId:"p",description:"Retained sprint",runMode:"automatic" as const,
    planning:"created" as const,taskIds:items.map(t=>t.taskId),queueState:"running" as const,pauseReason:null,
    targetBranch:null,conversation:[],plan:null,materializedVersion:1}]};
  writeBoards(localStorage,"owner",[{project:{projectId:"p",name:"Project",repository:"/p",openBranch:"main",defaultBranch:"main",activated:true},items,product}]);
  let cold=true;
  const s=scriptedTransport({
    "service.capabilities":()=>({methods:["native.query","native.board","projects.status"],machine:true}),
    "projects.list":()=>[{projectId:"p",displayName:"Project",canonicalRoot:"/p",activation:{}}],
    "capabilities.get":()=>capabilitiesWire(["capabilities.get","project.snapshot"]),
    "projects.status":()=>({ready:false,state:"starting",reason:null}),
    "native.board":()=>{if(cold) throw Error("snapshot_expired");return {protocol:{major:2,mapping:2},projectId:"p",generation:"g",version:"2",highWatermark:"3",
      data:{type:"board",value:{items,nextCursor:null,highWatermark:"3"}}};},
    "machine.call":raw=>{const op=(raw as {op:string}).op;return {ok:true,result:op==="overview"?{name:"This machine"}:op==="config.read"?{config:{projects:{},revisions:{}},connections:[],tokens:{}}:op==="project.config.read"?{value:{},tokens:{}}:op==="repo.snapshot"?null:product};},
  });
  history.replaceState(null,"",`/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={createRealPort({transport:s.transport,actorId:"owner",registry:true,boardStorage:localStorage})}/>,root);
  const clean=()=>{
    expect(root.querySelectorAll(".card[data-task]")).toHaveLength(31);
    expect(root.textContent).not.toMatch(/data is loading|Task details unavailable|could not be loaded|Queue paused|Waiting for sprint task details|This project is starting/);
  };
  // The retained board is shown with the rest of the first screen, once the machine's own settings are in.
  await settle(); clean();
  cold=false;s.send({type:"refresh"});
  await vi.waitFor(()=>expect(s.calls.filter(c=>c.operation==="native.board").length).toBeGreaterThan(1));
  await settle();clean();
});

it("does not describe pending sprint details as a read failure on the summary tab", async () => {
  const s=world();
  s.sprints=[{id:"sp",projectId:"p",description:"Pending sprint",runMode:"automatic",planning:"created",taskIds:["missing"],
    queueState:"paused",pauseReason:"Service pause",targetBranch:null,conversation:[],plan:null,materializedVersion:1}];
  await mount(s,`/m/${LOCAL_MACHINE_ID}/p/p/s/sp/summary`);
  expect(document.body.textContent).not.toMatch(/Task details unavailable|could not be loaded|Queue paused/);
});

it("reports persistently missing sprint task details instead of pretending the sprint has no tasks", async () => {
  const s = world();
  s.projects[0].issue = "The project failed repeated reads.";
  s.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["missing"], queueState: "idle", pauseReason: null }];
  await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  expect(sprintView(buildIndex(s, Date.now()), s.sprints[0]).state).not.toContain("0 of 0");
  expect(document.body.textContent).not.toContain("No tasks in this sprint yet");
  expect(document.body.textContent).toContain("Task details unavailable");
  expect(button("Retry")).toBeTruthy();
});

it("offers only logged-in, enabled, non-denied routes from the machine catalogue", async () => {
  const config = world().machines[0].config;
  config.catalogue["codex-acp"].models = ["luna", "disabled", "denied"].map(id => ({ id, routeId: "route-" + id, label: id, efforts: ["low"], observed: true }));
  config.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus", efforts: ["high"], observed: true }];
  config.surfaces.routing.engines["codex-acp"].enabled = ["luna", "denied"];
  config.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
  config.surfaces.routing.deny = ["codex-acp/denied"];
  config.setup["codex-acp"].status = "connected";
  config.setup["claude-agent-acp"].status = "downloaded_no_login";
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire(["capabilities.get"]),
    "machine.call": message => ({ ok: true, result: (message as { op: string }).op === "config.read" ? { config, connections: [], tokens: {} } : {} }),
  });
  const port = createRealPort({ transport: s.transport, actorId: "owner", registry: true });
  await port.query("capabilities.get", {});
  const [facts] = await port.query("machine.facts", {});
  expect(facts.options?.models.map(m => m.id)).toEqual(["route-luna"]);
});
