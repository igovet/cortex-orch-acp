import { createConnection } from "node:net";
import { expect, it } from "vitest";
import { capabilities } from "@cortex/workspace-contract";
import { createRealPort, type RealTransport } from "../src/port/real";
import type { WorkspaceCommand } from "@cortex/workspace-domain";

const socket=process.env.WORKSPACE_SPRINT_SOCKET;
const project=process.env.WORKSPACE_SPRINT_PROJECT;
it.skipIf(!socket||!project)("manual and automatic sprint planning, questions, dependent tasks and run order through real service",async()=>{
  let sequence=0;
  const streams=new Set<ReturnType<typeof createConnection>>();
  const rpc=(method:string,params:unknown):Promise<any>=>new Promise((resolve,reject)=>{
    const id=String(++sequence),stream=createConnection(socket!);streams.add(stream);
    let data=Buffer.alloc(0);
    const timer=setTimeout(()=>{stream.destroy();reject(new Error("fixture timeout"));},10000);
    stream.on("close",()=>{clearTimeout(timer);streams.delete(stream);});
    stream.on("error",reject);
    stream.on("connect",()=>{const body=Buffer.from(JSON.stringify({jsonrpc:"2.0",id,method,params})),header=Buffer.alloc(4);header.writeUInt32BE(body.length);stream.write(Buffer.concat([header,body]));});
    stream.on("data",chunk=>{data=Buffer.concat([data,chunk]); if(data.length<4||data.length<4+data.readUInt32BE(0))return; const reply=JSON.parse(data.subarray(4,4+data.readUInt32BE(0)).toString());stream.end(); if(reply.error)reject(new Error(method+":"+(params as {operation?:string}).operation+":"+reply.error.message));else resolve(reply.result);});
  });
  const transport:RealTransport={
    async request(operation,message){
      if(operation==="capabilities.get"){
        const status=await rpc("projects.status",{projectId:project});
        expect(status.ready).toBe(true);
        return capabilities("sprint-fixture","0",status.capabilities.operations);
      }
      if(operation==="machine.call" && (message as {op:string}).op==="config.read") {
        const reply=await rpc("machine.call",message);
        // The only connected engine in this fixture is FakeSessionRuntime.
        reply.result.config.catalogue["codex-acp"].models=[{id:"fixture-route",routeId:"fixture-route",label:"Fixture engine",efforts:[],observed:true}];
        return reply;
      }
      return rpc(["service.capabilities","projects.list","projects.status","machine.call","executions.prompt","executions.control"].includes(operation)?operation:"workspace",message);
    },subscribe:()=>()=>{}
  };
  const p=createRealPort({transport,actorId:"owner",registry:true});
  const command=(payload:WorkspaceCommand)=>p.command({requestId:"sprint-"+(++sequence),operation:payload.type,expectedVersion:null,payload});
  const machine=async(op:string,params:unknown={})=>{const reply=await rpc("machine.call",{op,projectId:project,params});expect(reply.ok,reply.code).toBe(true);return reply.result;};
  const product=()=>machine("product.read");
  const query=(operation:string,target?:string)=>rpc("workspace",{protocol:{major:1,minor:2},requestId:"sprint-read",projectId:project,operation,target});
  const wait=async(check:()=>Promise<any>)=>{const deadline=Date.now()+18000;while(Date.now()<deadline){const value=await check();if(value)return value;await new Promise(r=>setTimeout(r,50));}throw new Error("sprint condition not reached: "+JSON.stringify((await query("tasks.list")).items.map((t:any)=>({phase:t.phase,executionState:t.executionState,warnings:t.warnings}))));};
  async function answer(taskId:string){
    const row=await wait(async()=>{const t=await query("tasks.get",taskId);return t.phase==="waiting"?t:null;});
    const d=(await query("decisions.list",row.executionId)).items[0];
    // Every answer uses the latest identity after a refresh, just like the screen.
    for(let attempt=0;attempt<3;attempt++){
      await p.query("project.snapshot",{});
      const receipt=await command({type:"answer_decision",taskId,decisionId:d.decisionId,answer:"Continue"});
      if(receipt.ok)return;
      expect(receipt.outcome).toBe("stale_version");
    }
    throw new Error("question answer stayed stale");
  }
  try{
    await p.query("capabilities.get",{});await p.query("project.snapshot",{});await p.query("project.facts",{});await p.query("machine.facts",{});
    for(const runMode of ["manual","automatic"] as const){
      const created=await command({type:"create_sprint",projectId:project!,description:"Plan this isolated fixture",runMode,settings:{model:"fixture-route",effort:"",mode:"auto",language:"en"}});
      expect(created.ok,JSON.stringify(created)).toBe(true);
      const id=created.entityId!;
      const sprint=async()=> (await product()).sprints.find((s:any)=>s.id===id);
      expect((await sprint()).planningTaskId).toBeUndefined();
      await wait(async()=>(await sprint()).planning==="question");
      expect((await command({type:"answer_planning",sprintId:id,answer:"Continue"})).ok).toBe(true);
      await wait(async()=>(await sprint()).planning==="ready");
      await p.query("project.snapshot",{});
      const materialized=await command({type:"materialize_plan",sprintId:id,version:1});
      expect(materialized.ok,JSON.stringify(materialized)).toBe(true);
      const tasks=(await sprint()).taskIds;
      expect(tasks).toHaveLength(2);
      expect((await product()).tasks[tasks[1]].dependencies).toEqual([tasks[0]]);
      expect((await query("tasks.get",tasks[1])).executionId).toBeNull();
      if(runMode==="manual"){
        expect((await query("tasks.get",tasks[0])).executionId).toBeNull();
        await p.query("project.snapshot",{});
        expect((await command({type:"start_task",taskId:tasks[0]})).ok).toBe(true);
      }else expect((await command({type:"run_sprint",sprintId:id})).ok).toBe(true);
      await answer(tasks[0]);
      await wait(async()=>(await product()).tasks[tasks[0]].status==="ready_to_merge");
      if(runMode==="manual"){
        expect((await query("tasks.get",tasks[1])).executionId).toBeNull();
        await p.query("project.snapshot",{});
        expect((await command({type:"start_task",taskId:tasks[1]})).ok).toBe(true);
      }
      await wait(async()=>(await product()).tasks[tasks[1]].status==="ready_to_merge");
      if(runMode==="automatic")await wait(async()=>(await sprint()).queueState==="complete");
    }
  }finally{for(const stream of streams)stream.destroy();}
},55000);
