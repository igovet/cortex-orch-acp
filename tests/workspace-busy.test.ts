import { afterEach, expect, it, vi } from "../packages/workspace-ui/node_modules/vitest";
import { createConnection } from "node:net";
import { createRoot } from "../packages/workspace-ui/node_modules/solid-js/dist/solid.js";
import { dependencyReady, emptyWorkspace, runBlockerOf } from "../packages/workspace-domain/src";
import { createWorkspaceStore } from "@busy/store";
import { createRealPort } from "@busy/port";
import { capabilitiesWire, draftTask, scriptedPort, scriptedTransport, withTask } from "../packages/workspace-ui/test/support";

const realSocket = process.env.WORKSPACE_BUSY_SOCKET;
function serviceRequest(method: string, params: unknown): Promise<any> {
  return new Promise((resolve, reject) => {
    const socket = createConnection(realSocket!);
    let data = Buffer.alloc(0);
    socket.setTimeout(10000, () => socket.destroy(new Error("fixture timeout")));
    socket.on("error", reject);
    socket.on("connect", () => {
      const body = Buffer.from(JSON.stringify({jsonrpc:"2.0",id:1,method,params}));
      const prefix = Buffer.alloc(4); prefix.writeUInt32BE(body.length);
      socket.write(Buffer.concat([prefix,body]));
    });
    socket.on("data", chunk => {
      data = Buffer.concat([data,chunk]);
      if (data.length < 4 || data.length < data.readUInt32BE(0)+4) return;
      socket.end();
      const reply = JSON.parse(data.subarray(4).toString());
      if (reply.error) reject(new Error("fixture service refused")); else resolve(reply.result);
    });
  });
}
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); vi.useRealTimers(); });

it.skipIf(!realSocket)("measures real add and refresh against both disposable copied repositories", async () => {
  const transport = {
    request: async (operation: string, body: any) => {
      if (operation === "capabilities.get") return capabilitiesWire(["capabilities.get"]);
      // This benchmark deliberately never discovers native accounts or activates an engine.
      if (operation === "machine.call" && !body.projectId) throw new Error("machine discovery excluded");
      return serviceRequest(operation, body);
    },
    subscribe: () => () => {},
  };
  const port = createRealPort({actorId:"owner",registry:true,transport});
  const store = createRoot(d => {dispose=d; return createWorkspaceStore(port);});
  await store.reload();
  for (const name of ["Business-sell-data","AI-Frontier-Index"]) {
    const started = performance.now();
    const added = await store.dispatch({type:"add_project",machineId:"local",name,repository:process.env.WORKSPACE_BUSY_FIXTURES+"/"+name});
    const addMs = performance.now()-started;
    expect(added.ok).toBe(true);
    const read = performance.now(); await store.reload(); const refreshMs = performance.now()-read;
    console.info(JSON.stringify({fixture:name,addMs:Math.round(addMs),refreshMs:Math.round(refreshMs),readIssue:store.readIssue() === null ? null : "read_failed"}));
    if (!process.env.WORKSPACE_BUSY_BASELINE) expect(store.readIssue()).toBeNull();
    if (!process.env.WORKSPACE_BUSY_BASELINE) {
      expect(addMs).toBeLessThan(3000);
      expect(refreshMs).toBeLessThan(1000);
    }
  }
});
it.each(["as_is", "external", "cancel"] as const)("owner closure %s has consistent dependency and queue controls", choice => {
  const task = { ...draftTask(), status: "done" as const, closedAs: choice, notVerifiedByCortex: true };
  expect(dependencyReady(task)).toBe(choice !== "cancel");
  const state = withTask(emptyWorkspace(), "Closed");
  state.tasks = [task];
  const sprint = { planning:"created", runMode:"automatic", taskIds:[task.id] } as any;
  expect(runBlockerOf(state, sprint) === null).toBe(choice !== "cancel");
  expect(task.acceptance).toBeNull();
  if (choice === "cancel") expect(dependencyReady({...task,work:"merged"})).toBe(false);
});

it("keeps a transient refresh quiet and retries without a command", async () => {
  vi.useFakeTimers();
  const s = scriptedPort(), state = withTask(emptyWorkspace(), "Known");
  let failed = false;
  s.port.query = (async (op: string) => {
    if (op === "capabilities.get") return {everything:true,operations:[]};
    if (op === "project.snapshot") return [];
    if (op === "project.facts" && !failed) { failed=true; throw Object.assign(new Error("temporary"), {code:"machine_tool_timeout"}); }
    return [];
  }) as typeof s.port.query;
  const store = createRoot(d => {dispose=d;return createWorkspaceStore(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});});
  await store.reload();
  expect(store.readIssue()).toBeNull();
  expect(store.state().tasks[0].title).toBe("Known");
  await vi.advanceTimersByTimeAsync(3000);
  expect(store.connection().state).toBe("connected");
  expect(s.commands).toHaveLength(0);
});

it("shows persistent transient failures instead of hiding them forever", async () => {
  const s = scriptedPort(), state = withTask(emptyWorkspace(),"Known");
  s.port.query = (async (op: string) => op === "capabilities.get" ? {everything:true,operations:[]} :
    op === "project.snapshot" ? [] : op === "project.facts" ? Promise.reject(Object.assign(new Error("temporary"),{code:"machine_tool_timeout"})) : []) as typeof s.port.query;
  const store = createRoot(d => {dispose=d;return createWorkspaceStore(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});});
  await store.reload(); await store.reload();
  expect(store.readIssue()).toBeNull();
  await store.reload();
  expect(store.readIssue()).toBe("temporary");
  expect(s.commands).toHaveLength(0);
});

it("does not couple required product facts to a slow repository snapshot", async () => {
  let release!: (value: unknown) => void;
  const slow = new Promise(resolve => {release=resolve;});
  const s = scriptedTransport({
    "service.capabilities": () => ({methods:["machine.call"],machine:true}),
    "projects.list": () => [{projectId:"p",displayName:"P",canonicalRoot:"/p",activation:null}],
    "capabilities.get": () => capabilitiesWire(["capabilities.get"]),
    "machine.call": (request: any) => request.op === "repo.snapshot" ? slow : {ok:true,result:{tasks:{},sprints:[],messages:[]}},
  });
  const port = createRealPort({transport:s.transport,actorId:"owner",registry:true,projects:[{projectId:"p",name:"P",repository:"/p",openBranch:"main",defaultBranch:"main",activated:false}]});
  await port.query("capabilities.get", {});
  const read = port.query("project.facts", {});
  const result = await Promise.race([read, new Promise(resolve=>setTimeout(()=>resolve("blocked"),100))]);
  await read;
  await port.query("project.facts", {});
  expect(s.calls.filter(c => c.operation === "machine.call" && (c.message as any).op === "repo.snapshot")).toHaveLength(1);
  release({ok:true,result:null});
  expect(result).not.toBe("blocked");
});
