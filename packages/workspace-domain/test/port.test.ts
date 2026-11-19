import { describe, expect, it } from "vitest";
import { applyFacts, applyMachineFacts, applyProjection, compareVersions, emptyWorkspace, LOCAL_MACHINE_ID, overlayTask, projectionKey, runBlocker, type Project, type ProjectFacts, type ProjectionUpdate, type Sprint, type Task, type WorkspaceState } from "../src";
import { canRun, capabilityOf, noCapabilities, WIRE_OPERATIONS } from "../src/capabilities";

const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "P", repository: "/p", openBranch: "", defaultBranch: "" };

it("keeps a normalized history message once while retaining its attachment facts", () => {
  const state = emptyWorkspace();
  state.tasks = [{id:"t",projectId:"p",status:"working",process:"live",checkout:{kind:"main",branch:"main"},acceptance:null,result:null} as Task];
  state.sessions = [{taskId:"t",timeline:[{id:"h",kind:"said",text:"Look here"}],decisions:[]} as unknown as WorkspaceState["sessions"][number]];
  const facts: ProjectFacts = {projectId:"p",openBranch:"main",defaultBranch:"main",repo:null,workers:[],reviews:[],sprints:[],overlays:{},
    messages:[{id:"sent",taskId:"t",workerId:null,text:" Look here\n",mode:"send_now",delivery:"recorded",at:1,deliveredAt:1,failure:null,attachments:["clipboard.png"]}]};
  const next = applyFacts(state, facts);
  expect(next.sessions[0].timeline).toHaveLength(1);
  expect(next.messages[0].attachments).toEqual(["clipboard.png"]);
  expect(applyFacts(next, facts).sessions[0].timeline).toHaveLength(1);
});

it("does not add a message sent before the part of a conversation that was read as if it had just been said", () => {
  const state = emptyWorkspace();
  state.tasks = [{id:"t",projectId:"p",status:"working",process:"live",checkout:{kind:"main",branch:"main"},acceptance:null,result:null} as Task];
  state.sessions = [{taskId:"t",timeline:[{id:"h",kind:"said",text:"Later"}],decisions:[],historyEarlier:{state:"available",since:1000}} as unknown as WorkspaceState["sessions"][number]];
  const message = (id: string, text: string, at: number) => ({id,taskId:"t",workerId:null,text,mode:"send_now" as const,delivery:"delivered" as const,at,deliveredAt:at,failure:null});
  const facts: ProjectFacts = {projectId:"p",openBranch:"main",defaultBranch:"main",repo:null,workers:[],reviews:[],sprints:[],overlays:{},
    messages:[message("old","Long ago",500), message("new","Just now",2000)]};
  const ids = (s: WorkspaceState) => s.sessions[0].timeline.map(e => e.id);
  expect(ids(applyFacts(state, facts))).toEqual(["h", "facts:message:new"]);
  // Read to its beginning, the conversation holds every message it does not record.
  state.sessions = [{...state.sessions[0], historyEarlier: undefined}];
  expect(ids(applyFacts(state, facts))).toEqual(["h", "facts:message:old", "facts:message:new"]);
});

it("a cancelled or archived Hybrid member is skipped like the service skips it, so Continue queue stays open", () => {
  const state = emptyWorkspace();
  const sprint = { planning: "created", runMode: "hybrid", taskIds: ["a", "b"] } as Sprint;
  state.tasks = [{ id: "a", title: "One", status: "done", closedAs: "cancel" } as Task, { id: "b", title: "Two", status: "draft", closedAs: null } as Task];
  expect(runBlocker(state, sprint)).toBeNull();
  state.tasks[0] = { id: "a", title: "One", status: "draft", closedAs: null, archive: { at: 1 } } as unknown as Task;
  expect(runBlocker(state, sprint)).toBeNull();
});

it("keeps the Hybrid review blocker but uses recorded queue holds in Automatic", () => {
  const state = emptyWorkspace();
  const sprint = { planning: "created", runMode: "hybrid", taskIds: ["t"] } as Sprint;
  state.tasks = [{ id: "t", title: "One", status: "draft" } as Task];
  expect(runBlocker(state, sprint)).toBeNull();
  expect(runBlocker(state, { ...sprint, runMode: "manual" })).toContain("Select Hybrid or Automatic");
  state.tasks[0].status = "review";
  expect(runBlocker(state, sprint)).toBe("Review One first");
  const automatic: Sprint = {...sprint, runMode:"automatic"};
  expect(runBlocker(state, automatic)).toBeNull();
  automatic.automaticQueue = {kind:"continuation",taskId:"t",code:"external_blocker",reason:"External blocker is still present",needsOwner:false};
  expect(runBlocker(state, automatic)).toBe("External blocker is still present");
});

describe("machine facts", () => {
  it("lets an overview-only update leave engine usage and the composer's options as they were", () => {
    const usage = [{ engine: "codex" as const, connected: true, usedPercent: 40, resetAt: null, observedAt: null, window: "weekly" as const }];
    const options = { models: [{ id: "m", label: "M", engine: "Codex", efforts: [] }], efforts: [], modes: [], languages: [] };
    const settled = applyMachineFacts(emptyWorkspace(), { machineId: LOCAL_MACHINE_ID, engineUsage: usage, options, patch: { config: emptyWorkspace().machines[0].config } });
    const next = applyMachineFacts(settled, { machineId: LOCAL_MACHINE_ID, patch: { retentionDays: 12, ghAvailable: false } });
    expect(next.engineUsage).toBe(usage);
    expect(next.options).toBe(options);
    expect(next.machines[0]).toMatchObject({ retentionDays: 12, ghAvailable: false });
    // A settings read that does carry usage still replaces it.
    expect(applyMachineFacts(next, { machineId: LOCAL_MACHINE_ID, engineUsage: [], patch: {} }).engineUsage).toEqual([]);
  });
});

it("projects one automatic acceptance history entry and removes it with the acceptance overlay", () => {
  const state=emptyWorkspace();
  state.tasks=[{id:"t",projectId:"p",status:"review",process:"stopped",checkout:{kind:"main",branch:"main"},acceptance:null,result:null} as Task];
  state.sessions=[{taskId:"t",timeline:[],decisions:[]} as unknown as WorkspaceState["sessions"][number]];
  const facts: ProjectFacts={projectId:"p",openBranch:"main",defaultBranch:"main",repo:null,workers:[],reviews:[],sprints:[],
    overlays:{t:{status:"ready_to_merge",acceptance:{version:1,digest:"d",automatic:true,at:123}}}};
  const next=applyFacts(state,facts);
  expect(next.sessions[0].timeline).toEqual([{id:"facts:automatic-acceptance",kind:"line",icon:"check-circle",text:"Automatically accepted v1",at:123,rule:false}]);
  expect(applyFacts(next,facts).sessions[0].timeline).toHaveLength(1);
  expect(applyFacts(next,{...facts,overlays:{t:{acceptance:null}}}).sessions[0].timeline).toEqual([]);
});

it("keeps the automatic acceptance marker through the owner overlay", () => {
  const task = {id:"t",status:"review",process:"stopped",checkout:{kind:"main",branch:"main"},acceptance:null,result:null} as Task;
  const next = overlayTask(task, {status:"ready_to_merge",acceptance:{version:7,digest:"d",automatic:true}}, "main");
  expect(next.acceptance).toEqual({version:7,digest:"d",automatic:true});
});

describe("versions", () => {
  it("orders decimal strings beyond the safe integer range", () => {
    expect(compareVersions("9007199254740993", "9007199254740992")).toBe(1);
    expect(compareVersions("2", "10")).toBe(-1);
    expect(compareVersions("7", "7")).toBe(0);
  });
  it("refuses anything that is not a decimal", () => {
    expect(() => compareVersions("1.5", "2")).toThrow();
    expect(() => compareVersions("", "2")).toThrow();
  });
});

describe("projections", () => {
  it("does not turn a pending repository read into a no-Git fact", () => {
    for (const hasGit of [undefined, true]) {
      const state = {...emptyWorkspace(), projects:[{...project,hasGit}]};
      const facts: ProjectFacts = {projectId:"p",openBranch:"",defaultBranch:"",repo:null,workers:[],reviews:[],overlays:{},sprints:[]};
      expect(applyFacts(state, facts).projects[0].hasGit).toBe(hasGit);
      expect(applyFacts(state, {...facts,hasGit:true}).projects[0].hasGit).toBe(true);
      expect(applyFacts(state, {...facts,hasGit:false}).projects[0].hasGit).toBe(false);
    }
  });

  it("a reopened runtime task cannot retain acceptance or Ready to merge", () => {
    const task = {id:"t",status:"working",process:"live",observedPhase:"planning",checkout:{kind:"main",branch:"main"},acceptance:null,result:null} as Task;
    const next = overlayTask(task, {status:"ready_to_merge",automaticReady:true,acceptance:{version:1,digest:"old",at:1,open:null}}, "main");
    expect(next.status).toBe("working");
    expect(next.acceptance).toBeNull();
    expect(next.observedPhase).toBe("planning");
  });
  it("installs native owner settings and real check observations without certifying completion", () => {
    const task = {id:"t",status:"review",process:"stopped",checkout:{kind:"main",branch:"main"},acceptance:null,result:null} as Task;
    const result = {kind:"work" as const,version:7,sourceVersion:"7",digest:"observed",summary:"Native checks",checks:[{name:"true",outcome:"passed" as const,detail:""}],acceptanceAvailable:false};
    const settings = {model:"selected",effort:"",mode:"guarded" as const,language:"ru" as const};
    const next = overlayTask(task, {settings,result}, "main");
    expect(next.settings).toEqual(settings);
    expect(next.result).toEqual(result);
    expect(next.status).toBe("review");
    expect(next.acceptance).toBeNull();
  });
  it("a project slice replaces only that project", () => {
    const start = emptyWorkspace();
    start.projects.push({ ...project, id: "other" });
    const task = { id: "t", projectId: "p" } as Task;
    const update: ProjectionUpdate = { kind: "projection", version: "1", scope: "project", slice: { project, tasks: [task], sessions: [] } };
    const next = applyProjection(start, update);
    expect(next.projects.map(p => p.id).sort()).toEqual(["other", "p"]);
    expect(next.tasks).toEqual([task]);
    expect(start.tasks).toEqual([]);
    expect(projectionKey(update)).toBe("project:p");
  });
  it("a later slice drops a task the project no longer reports", () => {
    const first: ProjectionUpdate = { kind: "projection", version: "1", scope: "project", slice: { project, tasks: [{ id: "t", projectId: "p" } as Task], sessions: [] } };
    const second: ProjectionUpdate = { kind: "projection", version: "2", scope: "project", slice: { project, tasks: [], sessions: [] } };
    expect(applyProjection(applyProjection(emptyWorkspace(), first), second).tasks).toEqual([]);
  });
  it("a whole-workspace update replaces everything", () => {
    const state = emptyWorkspace();
    expect(applyProjection(emptyWorkspace(), { kind: "projection", version: "1", scope: "workspace", state })).toBe(state);
    expect(projectionKey({ kind: "projection", version: "1", scope: "workspace", state })).toBe("workspace");
  });
  // The screens compare lists by position: a reordered list is a changed one, and redraws everything that shows it.
  it("reading a project's unchanged facts again leaves every list in its order", () => {
    const sprint = (id: string, projectId: string) => ({ id, projectId }) as Sprint;
    const facts = (projectId: string, sprints = [sprint(`s-${projectId}`, projectId)]) => ({ projectId, openBranch: "main", defaultBranch: "main", repo: null, workers: [], reviews: [], overlays: {}, sprints }) as unknown as ProjectFacts;
    let state = emptyWorkspace();
    for (const id of ["a", "b", "c"]) state = applyFacts(state, facts(id));
    expect(applyFacts(state, facts("a")).sprints.map(s => s.id)).toEqual(["s-a", "s-b", "s-c"]);
    // Records that changed still arrive at the end.
    expect(applyFacts(state, facts("a", [sprint("s-a", "a"), sprint("s-a2", "a")])).sprints.map(s => s.id)).toEqual(["s-b", "s-c", "s-a", "s-a2"]);
  });
});

describe("capabilities", () => {
  it("names a contract operation for the commands the contract has, and a local one for the rest", () => {
    expect(capabilityOf("create_task")).toBe("tasks.create");
    expect(capabilityOf("start_task")).toBe("tasks.start");
    expect(capabilityOf("answer_decision")).toBe("decisions.answer");
    expect(capabilityOf("run_sprint")).toBe("workspace.run_sprint");
    expect(WIRE_OPERATIONS).toEqual(["tasks.create", "tasks.start", "decisions.answer"]);
  });
  it("fails closed: nothing is allowed before the machine says so", () => {
    expect(canRun(noCapabilities(), "create_task")).toBe(false);
    expect(canRun({ everything: false, operations: ["tasks.create"] }, "create_task")).toBe(true);
    expect(canRun({ everything: false, operations: ["tasks.create"] }, "stop_task")).toBe(false);
  });
});

describe("empty workspace", () => {
  it("starts with one local machine and no project, task, engine or option", () => {
    const state = emptyWorkspace();
    expect(state.machines).toHaveLength(1);
    expect(state.machines[0]).toMatchObject({ id: LOCAL_MACHINE_ID, kind: "local", connection: "disconnected", version: null, connections: [] });
    expect([state.projects, state.tasks, state.sprints, state.engineUsage, state.options.models, state.options.efforts, state.options.modes, state.options.languages].every(list => list.length === 0)).toBe(true);
    expect(JSON.stringify(state)).not.toMatch(/mock/i);
  });
});
