import { createConnection } from "node:net";
import { render } from "solid-js/web";
import { expect, it } from "vitest";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";

const socket = process.env.WORKSPACE_CONTRACT_SOCKET;
function rpc(operation: string, message: unknown = {}): Promise<any> {
  return new Promise((resolve, reject) => {
    const s = createConnection(socket!); let data = "";
    s.setTimeout(10000, () => s.destroy(new Error("fixture timeout")));
    s.on("error", reject); s.on("connect", () => s.write(JSON.stringify({operation,message})+"\n"));
    s.on("data", chunk => { data += chunk; if (!data.includes("\n")) return; s.end(); const r=JSON.parse(data); if(r.error) reject(new Error(r.error)); else resolve(r.result); });
  });
}
async function until(test: () => boolean) {
  const end = Date.now() + 8000;
  while (!test() && Date.now() < end) await new Promise(r => setTimeout(r, 25));
  expect(test(), document.body.textContent ?? "").toBe(true);
}
it.skipIf(!socket)("persisted source drift is actionable; a reviewer search is manual, never silently passed or an Accept blocker", async () => {
  const f = await rpc("fixture");
  const p = createRealPort({actorId:"local-owner", registry:true, transport:{request:rpc,subscribe:()=>()=>{}}});
  localStorage.clear(); sessionStorage.clear();
  history.replaceState(null,"",`/m/local/t/${f.taskId}/changes`);
  const root=document.createElement("div"); document.body.append(root);
  const dispose=render(()=><Workspace port={p}/>,root);
  const button=(label:string)=>[...root.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent?.trim()===label);
  try {
    await until(()=>!!button("Accept v1") || !!button("Accept v1 and merge into main"));
    if(f.drift) {
      expect(button("Accept v1")?.getAttribute("aria-disabled")).toBe("true");
      expect(root.textContent).toMatch(/files changed|source changed|worktree changed|no longer matches the worktree/i);
      expect(button("Request a new result…")).toBeDefined();
      button("Accept v1")!.click();
      await new Promise(r=>setTimeout(r,50));
      expect((await rpc("machine.call",{op:"product.read",projectId:f.projectId})).result.tasks[f.taskId].acceptance).toBeUndefined();
    } else if(f.display) {
      await rpc("fixture.tick");
      button("Accept v1")!.click();
      await until(()=>root.textContent?.includes("Accepted v1")===true);
    } else {
      expect(button("Accept v1")!.getAttribute("aria-disabled")).toBeNull();
      expect(root.textContent).toContain("Check manually");
      expect(button("Rerun check")).toBeUndefined();
      button("Accept v1")!.click();
      expect(root.querySelector('dialog[aria-label="Accept with open checks"]')?.hasAttribute("open")).not.toBe(true);
      await until(()=>root.textContent?.includes("Accepted v1")===true);
      const saved=(await rpc("machine.call",{op:"product.read",projectId:f.projectId})).result.tasks[f.taskId];
      expect(saved.acceptance).toBeDefined();
      expect(saved.acceptance.open).toEqual({failed: 0, not_run: 0, unverified: 0});
    }
  } finally {dispose();root.remove();}
});
