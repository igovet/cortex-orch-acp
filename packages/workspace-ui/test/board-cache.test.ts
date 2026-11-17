import { describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { readBoards, writeBoards, type CachedBoard } from "../src/port/board-cache";
import { capabilitiesWire, scriptedTransport, wireTask } from "./support";
const project = {projectId:"p",name:"Saved project",repository:"/p",openBranch:"main",defaultBranch:"main",activated:true};
const items = Array.from({length:31},(_,i)=>wireTask({taskId:"task-"+i}));
const saved: CachedBoard = {project,items,product:{tasks:{},sprints:[{
  id:"sprint",projectId:"p",description:"Saved sprint",runMode:"automatic",planning:"created",
  targetBranch:null,conversation:[],plan:null,materializedVersion:1,taskIds:items.map(t=>t.taskId),queueState:"running",pauseReason:null,
}]}};
const memory = () => {let raw:string|null=null;return {getItem:()=>raw,setItem:(_key:string,value:string)=>{raw=value;}};};
describe("confirmed board cache",()=>{
  it("paints 31 real saved tasks synchronously before a cold service read without granting actions",()=>{
    const storage=memory();writeBoards(storage,"owner",[saved]);
    const s=scriptedTransport({"service.capabilities":()=>new Promise(()=>{})});
    const p=createRealPort({transport:s.transport,registry:true,actorId:"owner",boardStorage:storage});
    let dispose!:()=>void;
    const store=createRoot(d=>{dispose=d;return createWorkspaceStore(p);});
    try {
      expect(store.ready()).toBe(true);
      expect(store.state().tasks).toHaveLength(31);
      expect(store.state().sprints[0]?.queueState).toBe("running");
      expect(store.state().projects[0]?.issue).toBeNull();
      expect(store.can("tasks.start")).toBe(false);
      expect(store.connection().state).toBe("connecting");
    } finally {dispose();}
  });
  it("retains the last complete board through bounded failed refreshes and only then reports a real failure",async()=>{
    const storage=memory();writeBoards(storage,"owner",[saved]);
    const s=scriptedTransport({
      "service.capabilities":()=>({methods:["native.query","native.board"],machine:true}),
      "projects.list":()=>[{projectId:"p",displayName:"Saved project",canonicalRoot:"/p",activation:{}}],
      "capabilities.get":()=>capabilitiesWire(["capabilities.get","project.snapshot"]),
      "native.board":()=>{throw Error("storage_format_unsupported");},
      "machine.call":()=>({ok:true,result:saved.product}),
    });
    const p=createRealPort({transport:s.transport,registry:true,actorId:"owner",boardStorage:storage});
    let dispose!:()=>void;
    const store=createRoot(d=>{dispose=d;return createWorkspaceStore(p);});
    try {
      await vi.waitFor(()=>expect(store.connection().state).toBe("connected"));
      expect(store.state().projects[0]?.issue).toBeNull();
      await store.reload();
      expect(store.state().projects[0]?.issue).toBeNull();
      await store.reload();
      expect(store.state().projects[0]?.issue).toContain("could not be read");
      expect(store.state().tasks).toHaveLength(31);
      expect(store.state().sprints[0].queueState).toBe("running");
      expect(readBoards(storage,"owner")[0].items).toHaveLength(31);
    } finally {dispose();}
  });
  it("does not load another actor's cache, corrupt data, incomplete membership or quota failures",()=>{
    const storage=memory();writeBoards(storage,"owner",[saved]);
    expect(readBoards(storage,"other")).toEqual([]);
    storage.setItem("","{");
    expect(readBoards(storage,"owner")).toEqual([]);
    writeBoards(storage,"owner",[{...saved,items:[]}]);
    expect(readBoards(storage,"owner")).toEqual([]);
    expect(()=>writeBoards({getItem:()=>null,setItem:()=>{throw Error("quota");}},"owner",[saved])).not.toThrow();
  });
  it("removes a detached or root-changed cached identity when the registry answers",async()=>{
    const storage=memory();writeBoards(storage,"owner",[saved]);
    const s=scriptedTransport({"service.capabilities":()=>({methods:[]}),"projects.list":()=>[]});
    const p=createRealPort({transport:s.transport,registry:true,actorId:"owner",boardStorage:storage});
    await p.query("capabilities.get",{}).catch(()=>{}); // no ACP capabilities are needed for cache invalidation
    expect(readBoards(storage,"owner")).toEqual([]);
  });
});
