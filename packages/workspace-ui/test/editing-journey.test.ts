import { mkdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it } from "vitest";
import { capabilities } from "@cortex/workspace-contract";
import { applyFacts, applyProjection, emptyWorkspace, taskEditable, type WorkspaceCommand } from "@cortex/workspace-domain";
import { createRealPort, type RealTransport } from "../src/port/real";
import { buildIndex } from "../src/selectors";
import { nativeServiceHome, NativeService } from "./native-machine";

let home: string | undefined, service: NativeService | undefined;
afterEach(async () => {
  try { await service?.stop(); }
  finally { if (home) rmSync(home, { recursive:true, force:true }); }
});

it("archives and restores a queued draft through the real port without starting it", async () => {
  // No provider accounts or engine routes. Only the local service and a scratch project.
  home = nativeServiceHome();
  const root = join(home, "projects"), repo = join(root, "project");
  mkdirSync(repo, { recursive:true });
  const native = service = await NativeService.start(home, root);
  const rpc = (method: string, params: unknown) => native.call(method, params) as Promise<any>;
  const {projectId: project} = await rpc("projects.register", {root:repo,displayName:"Editing fixture"});
  await rpc("projects.activate", {projectId:project});
  await expect.poll(async () => (await rpc("projects.status", {projectId:project})).ready, {timeout:5000}).toBe(true);
  let sequence = 0;
  const transport: RealTransport = {request:async (operation,message) => {
    if(operation==="capabilities.get") { const status=await rpc("projects.status",{projectId:project}); return capabilities("editing-fixture","0",status.capabilities.operations); }
    return rpc(["service.capabilities","projects.list","projects.status","native.query","native.board","machine.call","executions.control"].includes(operation)?operation:"workspace",message);
  },subscribe:() => () => {}};
  const port = createRealPort({transport,actorId:"owner",registry:true});
  const command = (payload: WorkspaceCommand) => port.command({requestId:"edit-flow-"+(++sequence),operation:payload.type,expectedVersion:null,payload});
  const read = async () => {
    let state = emptyWorkspace();
    for (const update of await port.query("project.snapshot",{})) state=applyProjection(state,update);
    for (const facts of await port.query("project.facts",{})) state=applyFacts(state,facts);
    return state;
  };
  await port.query("capabilities.get",{});
  await read();
  // Seed the already-created sprint through the service; planning and engines are not under test.
  const sprint = await native.machine("product.sprint", project, {
    action:"create",description:"Isolated sprint fixture",runMode:"manual",checkout:{kind:"main"}
  });
  expect(sprint.ok, JSON.stringify(sprint)).toBe(true);
  const id = (sprint.result as {sprintId:string}).sprintId;
  // Materialized empty queue, just as the owner can add to an existing created sprint.
  const patched=await rpc("machine.call",{op:"product.sprint",projectId:project,params:{action:"patch",sprintId:id,patch:{planning:"created",queueState:"running"}}});
  expect(patched.ok).toBe(true);
  const made=await command({type:"create_task",projectId:project,sprintId:id,description:"Original",checkout:{kind:"worktree"}});
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
  const attachments = await port.readTaskAttachments!(task.id);
  expect(attachments).toHaveLength(1);
  const saved = attachments[0];
  if (!saved || typeof saved !== "object" || !("sha256" in saved)) throw new Error("Expected a retained attachment reference");
  expect(await port.readAttachment!(saved)).toEqual(file);
  expect((await command({type:"close_sprint",sprintId:id,confirmed:false})).ok).toBe(false);
  expect((await command({type:"close_sprint",sprintId:id,confirmed:true})).ok).toBe(true);
  state=await read();
  expect(state.sprints.find(s => s.id===id)!.archivedAt).toBeGreaterThan(0);
  const index=buildIndex(state,Date.now());
  expect(index.summaries.get(project)!.sprints).toEqual([]);
  expect(index.byId.get(task.id)!.sprint).toBeNull();
  expect(index.byId.get(task.id)!.task.executionId).toBeNull();
  expect(index.archive.find(v => v.id === task.id)?.task).toMatchObject({status:"draft",archive:{previousStatus:"draft"}});
  const archived = await command({type:"restore_task",taskId:task.id,now:Date.now()});
  expect(archived.ok,JSON.stringify(archived)).toBe(true);
  state = await read();
  expect(state.tasks.find(t => t.id===task.id)).toMatchObject({status:"draft",sprintId:null,archive:null,executionId:null});
  expect(state.sprints.find(s => s.id===id)?.taskIds).toEqual([]);
  expect((await command({type:"delete_sprint",projectId:project,sprintId:id,confirmed:false})).ok).toBe(false);
  const deleted = await command({type:"delete_sprint",projectId:project,sprintId:id,confirmed:true});
  expect(deleted.ok,JSON.stringify(deleted)).toBe(true);
  expect(deleted.deletedTaskIds).not.toContain(task.id);
  state=await read();
  expect(state.sprints.some(s => s.id===id)).toBe(false);
  expect(state.tasks.some(t => t.id===task.id)).toBe(true);
},45000);
