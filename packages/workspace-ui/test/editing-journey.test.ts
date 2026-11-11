import { createConnection } from "node:net";
import { expect, it } from "vitest";
import { capabilities } from "@cortex/workspace-contract";
import { applyFacts, applyProjection, emptyWorkspace, taskEditable, type WorkspaceCommand } from "@cortex/workspace-domain";
import { createRealPort, type RealTransport } from "../src/port/real";
import { buildIndex } from "../src/selectors";
const socket = process.env.WORKSPACE_EDIT_SOCKET, project = process.env.WORKSPACE_EDIT_PROJECT;

it.skipIf(!socket || !project)("saves and edits through the production port, then closes and rereads an archived sprint", async () => {
  let sequence = 0;
  const streams = new Set<ReturnType<typeof createConnection>>();
  const rpc = (method: string, params: unknown): Promise<any> => new Promise((resolve,reject) => {
    const id = String(++sequence), stream = createConnection(socket!); streams.add(stream);
    let data = Buffer.alloc(0);
    const timer = setTimeout(() => { stream.destroy(); reject(new Error("fixture timeout")); },10000);
    stream.on("close",() => { clearTimeout(timer); streams.delete(stream); });
    stream.on("error",reject);
    stream.on("connect",() => { const body=Buffer.from(JSON.stringify({jsonrpc:"2.0",id,method,params})), header=Buffer.alloc(4); header.writeUInt32BE(body.length); stream.write(Buffer.concat([header,body])); });
    stream.on("data",chunk => { data=Buffer.concat([data,chunk]); if(data.length<4 || data.length<4+data.readUInt32BE(0))return; const reply=JSON.parse(data.subarray(4,4+data.readUInt32BE(0)).toString()); stream.end(); if(reply.error)reject(new Error(reply.error.message)); else resolve(reply.result); });
  });
  const transport: RealTransport = {request:async (operation,message) => {
    if(operation==="capabilities.get") { const status=await rpc("projects.status",{projectId:project}); return capabilities("editing-fixture","0",status.capabilities.operations); }
    if(operation==="machine.call" && (message as {op:string}).op==="config.read") {
      const reply=await rpc("machine.call",message);
      reply.result.config.catalogue["codex-acp"].models=[{id:"fixture-route",routeId:"fixture-route",label:"Fixture engine",efforts:[],observed:true}];
      return reply;
    }
    return rpc(["service.capabilities","projects.list","projects.status","machine.call","executions.control"].includes(operation)?operation:"workspace",message);
  },subscribe:() => () => {}};
  const port = createRealPort({transport,actorId:"owner",registry:true});
  const command = (payload: WorkspaceCommand) => port.command({requestId:"edit-flow-"+(++sequence),operation:payload.type,expectedVersion:null,payload});
  const read = async () => {
    let state = emptyWorkspace();
    for (const update of await port.query("project.snapshot",{})) state=applyProjection(state,update);
    for (const facts of await port.query("project.facts",{})) state=applyFacts(state,facts);
    return state;
  };
  try {
    await port.query("capabilities.get",{});
    await read();
    // Existing fixture engine, no new login, account or live model call.
    const sprint = await command({type:"create_sprint",projectId:project!,description:"Plan this isolated fixture",runMode:"manual",checkout:{kind:"main"},settings:{model:"fixture-route",effort:"",mode:"auto",language:"en"}});
    expect(sprint.ok,JSON.stringify(sprint)).toBe(true);
    const id=sprint.entityId!;
    // Materialized empty queue, just as the owner can add to an existing created sprint.
    const patched=await rpc("machine.call",{op:"product.sprint",projectId:project,params:{action:"patch",sprintId:id,patch:{planning:"created",queueState:"running"}}});
    expect(patched.ok).toBe(true);
    const made=await command({type:"create_task",projectId:project!,sprintId:id,description:"Original",checkout:{kind:"worktree"}});
    expect(made.ok,JSON.stringify(made)).toBe(true);
    let state=await read(), task=state.tasks.find(t => t.id===made.entityId)!;
    expect(state.sprints.find(s => s.id===id)!.taskIds).toContain(task.id);
    expect(task.sprintId).toBe(id);
    expect(task.checkout.kind).toBe("main");
    expect(taskEditable(task)).toBe(true);
    const file={name:"scope.txt",mimeType:"text/plain",data:"aGk="};
    const edit={type:"edit_task" as const,taskId:task.id,expectedVersion:task.sourceVersion!,title:"Owner title",description:"Revised prompt",attachments:[file],settings:task.settings,checkout:{kind:"main" as const}};
    expect((await command(edit)).ok).toBe(true);
    expect((await command({...edit,description:"Stale overwrite"})).outcome).toBe("stale_version");
    state=await read(); task=state.tasks.find(t => t.id===made.entityId)!;
    expect(task.title).toBe("Owner title"); expect(task.description).toBe("Revised prompt");
    expect(await port.readTaskAttachments!(task.id)).toEqual([file]);
    expect((await command({type:"close_sprint",sprintId:id,confirmed:false})).ok).toBe(false);
    expect((await command({type:"close_sprint",sprintId:id,confirmed:true})).ok).toBe(true);
    state=await read();
    expect(state.sprints.find(s => s.id===id)!.archivedAt).toBeGreaterThan(0);
    const index=buildIndex(state,Date.now());
    expect(index.summaries.get(project!)!.sprints).toEqual([]);
    expect(index.byId.get(task.id)!.sprint).toBeNull();
    expect(index.byId.get(task.id)!.task.executionId).toBeNull();
  } finally { for(const stream of streams) stream.destroy(); }
},45000);
