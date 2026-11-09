import { createConnection } from "node:net";
import { expect, it } from "vitest";
import { createComponent } from "solid-js";
import { render } from "solid-js/web";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import type { WorkspaceCommand } from "@cortex/workspace-domain";
const socket = process.env.WORKSPACE_CONTRACT_SOCKET;
function rpc(operation: string, message: unknown = {}): Promise<any> {
  return new Promise((resolve, reject) => {
    const s = createConnection(socket!); let data = "";
    s.setTimeout(10000, () => s.destroy(new Error("fixture timeout")));
    s.on("error", reject); s.on("connect", () => s.write(JSON.stringify({operation,message})+"\n"));
    s.on("data", chunk => { data += chunk; if (!data.includes("\n")) return; s.end(); const r=JSON.parse(data); if(r.error) reject(new Error(r.error)); else resolve(r.result); });
  });
}
it.skipIf(!socket)("planning creates no execution; its plan materializes into dependent unstarted user tasks", async () => {
  const f=await rpc("fixture"), calls:string[]=[];
  const p=createRealPort({actorId:"local-owner", registry:true, transport:{request:(op,body)=>{calls.push(op);return rpc(op,body);},subscribe:()=>()=>{}}});
  const command=(payload:WorkspaceCommand)=>p.command({requestId:crypto.randomUUID(),operation:payload.type,expectedVersion:null,payload});
  await p.query("capabilities.get",{}); await p.query("project.snapshot",{});
  const created=await command({type:"create_sprint",projectId:f.projectId,description:"Implement subtract.py. Ask the owner whether negative results are allowed before implementation. Separately implement multiply.py with unit tests.",runMode:"automatic",settings:{model:"fixture",effort:"",mode:"auto",language:"en"}});
  expect(created.ok).toBe(true);
  expect(calls).not.toContain("tasks.create"); expect(calls).not.toContain("tasks.start");
  await rpc("fixture.tick");
  let product=await rpc("machine.call",{op:"product.read",projectId:f.projectId});
  let sprint=product.result.sprints[0];
  expect(sprint.planning).toBe("question");
  expect(sprint.conversation.at(-1)).toMatchObject({kind:"question",text:"Are negative subtraction results allowed?",options:["Yes","No"]});
  const planningFacts=await p.query("project.facts",{});
  expect(planningFacts[0].sprints[0].conversation.at(-1)?.text).toContain("negative subtraction");
  history.replaceState(null,"",`/m/local/p/${f.projectId}/s/${created.entityId}/planning`);
  const root=document.createElement("div"); document.body.append(root);
  const dispose=render(()=>createComponent(Workspace,{port:p}),root);
  try {
    const deadline=Date.now()+8000;
    while(!root.textContent?.includes("Are negative subtraction results allowed?") && Date.now()<deadline)
      await new Promise(r=>setTimeout(r,25));
    expect(root.textContent).toContain("Are negative subtraction results allowed?");
    expect([...root.querySelectorAll("button")].some(b=>b.textContent?.includes("No"))).toBe(true);
  } finally { dispose(); root.remove(); }
  expect((await command({type:"answer_planning",sprintId:created.entityId!,answer:"No"})).ok).toBe(true);
  await rpc("fixture.tick");
  product=await rpc("machine.call",{op:"product.read",projectId:f.projectId});
  sprint=product.result.sprints[0];
  expect(sprint.planning).toBe("ready"); expect(sprint.taskIds).toEqual([]);
  const materialized=await command({type:"materialize_plan",sprintId:created.entityId!,version:1});
  expect(materialized.ok).toBe(true);
  expect(calls).not.toContain("tasks.start");
  const current=await rpc("machine.call",{op:"product.read",projectId:f.projectId});
  const ids=current.result.sprints[0].taskIds;
  expect(ids).toHaveLength(2);
  expect(current.result.tasks[ids[1]].dependencies).toEqual([ids[0]]);
  const snapshot=await p.query("project.snapshot",{});
  const update=snapshot[0]; if(update.scope!=="project") throw new Error("project required");
  expect(update.slice.tasks).toHaveLength(2);
  expect(update.slice.tasks.every(t=>t.status==="draft" && t.executionId===null)).toBe(true);
  const subtraction = update.slice.tasks.find(t=>t.description.includes("subtract.py"))!;
  const multiplication = update.slice.tasks.find(t=>t.description.includes("multiply.py"))!;
  expect(subtraction.description).toContain("Negative results are not allowed.");
  expect(subtraction.description).not.toContain("multiply.py");
  expect(multiplication.description).not.toContain("subtract.py");
  expect(multiplication.description).not.toContain("negative results");
  expect(update.slice.tasks.every(t=>!t.description.includes('"text":') && !t.description.includes('"at":'))).toBe(true);
});
