import { createConnection } from "node:net";
import { existsSync, writeFileSync, readFileSync } from "node:fs";
import { render } from "solid-js/web";
import { expect, it } from "vitest";
import { capabilities, PROTOCOL } from "@cortex/workspace-contract";
import { Workspace } from "../src";
import { createRealPort, type RealTransport, type TransportEvent } from "../src/port/real";

const socket = process.env.WORKSPACE_JOURNEY_SOCKET;
const project = process.env.WORKSPACE_JOURNEY_PROJECT;
it.skipIf(!socket || !project)("owner screens survive window detach through the real runtime", async () => {
  let sequence = 0, dispose: (() => void) | undefined;
  const sockets = new Set<ReturnType<typeof createConnection>>();
  const listeners = new Set<(e: TransportEvent) => void>();
  const readErrors: string[] = [];
  function rpc(method: string, params: unknown): Promise<any> {
    const id = String(++sequence);
    return new Promise((resolve, reject) => {
      const stream = createConnection(socket!); sockets.add(stream);
      let data = Buffer.alloc(0);
      const timer = setTimeout(() => { stream.destroy(); reject(new Error("socket timeout")); }, 12000);
      stream.on("error", reject);
      stream.on("close", () => { clearTimeout(timer); sockets.delete(stream); });
      stream.on("connect", () => {
        const body = Buffer.from(JSON.stringify({jsonrpc:"2.0",id,method,params}));
        const header = Buffer.alloc(4); header.writeUInt32BE(body.length); stream.write(Buffer.concat([header,body]));
      });
      stream.on("data", chunk => {
        data = Buffer.concat([data,chunk]);
        while (data.length >= 4 && data.length >= 4 + data.readUInt32BE(0)) {
          const size = data.readUInt32BE(0); expect(size).toBeLessThanOrEqual(1048576);
          const reply = JSON.parse(data.subarray(4,4+size).toString()); data=data.subarray(4+size);
          if (reply.method === "service.event") {
            if (reply.params.projectId) for (const f of listeners) f({type:"invalidated",projectId:reply.params.projectId});
          } else {
            expect(reply.id).toBe(id);
            if (reply.error) reject(new Error(reply.error.message)); else resolve(reply.result);
            if (method !== "service.subscribe") stream.end();
          }
        }
      });
    });
  }
  const transport: RealTransport = {
    async request(operation, message) {
      if (operation === "capabilities.get") {
        const projects = await rpc("projects.list", {});
        const active = projects.filter((p: any) => p.activation !== null);
        let operations: string[] = [];
        for (const p of active) {
          const status = await rpc("projects.status", {projectId:p.projectId});
          expect(status.ready).toBe(true); operations=status.capabilities.operations;
        }
        return {...capabilities("journey-test", "0", operations), attachments: active.length > 0};
      }
      return rpc(["service.capabilities","projects.list","projects.register","projects.activate","projects.status","machine.call","executions.cancel","executions.prompt","executions.control"].includes(operation) ? operation : "workspace", message);
    },
    subscribe(listener) { listeners.add(listener); return () => {listeners.delete(listener);}; },
  };
  const all = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
  const button = (text: string) => all("button").find(b => b.textContent?.trim().startsWith(text) || b.textContent?.trim().endsWith(text))!;
  function input(label: string, value: string) {
    const el=document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!;
    expect(el.disabled).toBe(false);
    el.value=value; el.dispatchEvent(new Event("input",{bubbles:true}));
  }
  async function until(test: () => boolean, priorReceipt?: string, timeoutMs = 12000) {
    const deadline=Date.now()+timeoutMs;
    while (!test() && Date.now()<deadline) {
      const error = document.querySelector(".ws-receipt")?.textContent;
      if (error && error !== priorReceipt) throw new Error("Fixture UI action failed: "+error);
      await new Promise(r=>setTimeout(r,30));
    }
    if (!test()) {
      const projects = await rpc("projects.list",{});
      const p = projects[0];
      const page = p?.activation ? await rpc("workspace",{protocol:PROTOCOL,requestId:"diagnostic",projectId:p.projectId,operation:"tasks.list"}) : {items:[]};
      throw new Error(JSON.stringify({readErrors,workerProgress:existsSync(project!+"/worker-progress") ? readFileSync(project!+"/worker-progress","utf8") : "",fixtureErrors:existsSync(project!+"/worker-errors") ? readFileSync(project!+"/worker-errors","utf8") : "",workerCards:all(".wcard").map(e=>e.textContent?.slice(0,160)),labels:all("textarea").map(e=>e.getAttribute("aria-label")),path:location.pathname,connected:document.querySelector(".machine .st")?.textContent,tasks:page.items.map((t:any)=>({phase:t.phase,state:t.executionState,warnings:t.warnings})),questionCards:document.querySelectorAll(".dc").length, fixturePrompt:document.querySelector(".dc-q")?.textContent, offers:(await rpc("projects.status",{projectId:p.projectId})).capabilities.operations, decisions:page.items[0]?.executionId ? (await rpc("workspace",{protocol:PROTOCOL,requestId:"diagnostic-decisions",projectId:p.projectId,operation:"decisions.list",target:page.items[0].executionId})).items.length : 0}));
    }
  }
  async function mount(path = "/") {
    history.replaceState(null,"",path);
    const root=document.createElement("div"); document.body.append(root);
    const port = createRealPort({transport,actorId:"peer",registry:true});
    const query = port.query;
    port.query = (operation, parameters) => query(operation, parameters).catch(error => {
      readErrors.push(operation + ": " + String(error)); throw error;
    });
    dispose=render(()=><Workspace port={port}/>,root);
    await until(()=>document.querySelector(".machine .st")?.textContent === "Connected");
  }
  const detach = () => { dispose?.(); document.body.innerHTML=""; };
  try {
    localStorage.clear(); sessionStorage.clear();
    await rpc("service.subscribe", {});
    await mount();
    button("Add project").click();
    await until(()=>!!document.querySelector('dialog[open] [role="option"]'));
    [...document.querySelectorAll<HTMLElement>('dialog[open] [role="option"]')].find(o=>o.textContent?.startsWith("Search for folder"))!.click();
    await until(()=>!!document.querySelector('dialog[open] input'));
    input("Search for folder",project!);
    await until(()=>!!document.querySelector('dialog[open] [role="option"]'));
    document.querySelector('dialog[open] input')!.dispatchEvent(new KeyboardEvent("keydown",{key:"Enter",bubbles:true}));
    await until(()=>!!button("Activate"));
    expect(existsSync(project!+"/.cortex")).toBe(false);
    button("Activate").click();
    await until(()=>!button("Activate"));
    button("New task").click();
    input("Describe the task","Explain this disposable project.");
    button("Save as draft").click();
    await until(()=>!!document.querySelector(".card-link"));
    expect(existsSync(project!+"/turns")).toBe(false);
    (document.querySelector(".card-link") as HTMLElement).click();
    button("Start").click();
    await until(()=>document.body.textContent!.includes("Which direction"));
    const path=location.pathname;
    detach(); await mount(path);
    expect(document.body.textContent).toContain("Which direction");
    all("a").find(e=>e.getAttribute("href")==="/m/local/inbox")!.click();
    await until(()=>document.body.textContent!.includes("Decisions from this machine"));
    expect(document.body.textContent).not.toContain("Nothing needs you right now");
    all("a").find(e=>e.getAttribute("href")===path)!.click();
    await until(()=>!!button("Other answer"));
    writeFileSync(project!+"/expect-attachments","");
    button("Other answer").click();
    input("Your answer","first line\nsecond line");
    const choose = (selector: string, name: string, body: string) => {
      const el = document.querySelector<HTMLInputElement>(selector)!;
      Object.defineProperty(el, "files", {configurable:true,value:[new File([body], name, {type:"text/plain"})]});
      el.dispatchEvent(new Event("change",{bubbles:true}));
    };
    choose('.ws-dc-other input[type="file"]', "notes.txt", "answer fixture");
    await until(()=>document.querySelector(".ws-dc-other .ws-att")?.textContent?.includes("notes.txt") === true);
    button("Send answer").click();
    // A late runtime event can invalidate the question CAS. Refresh is read-only;
    // the owner reviews and explicitly presses Send again, never automatic replay.
    await until(()=>existsSync(project!+"/streaming") || !!document.querySelector(".ws-receipt"));
    if (!existsSync(project!+"/streaming")) {
      expect(document.querySelector(".ws-receipt")?.textContent).toMatch(/changed on the machine/);
      expect((document.querySelector('[aria-label="Your answer"]') as HTMLTextAreaElement).value).toBe("first line\nsecond line");
      const reviewedRefusal = document.querySelector(".ws-receipt")!.textContent!;
      button("Send answer").click();
      // The old receipt may remain rendered while this explicit owner retry is in flight.
      await until(()=>existsSync(project!+"/streaming"), reviewedRefusal);
    }
    expect(existsSync(project!+"/answer-resource-received")).toBe(true);
    detach(); await mount(path);
    expect(document.body.textContent).not.toContain("Runtime-owned journey result.");
    expect(document.body.textContent).toMatch(/Working|Executing|Planning/);
    await until(()=>document.body.textContent!.includes("Streaming from the fake engine."));
    input("Message", "Held across relaunch");
    choose('.dock input[type="file"]', "queued.txt", "queue fixture");
    await until(()=>document.querySelector(".dock .composer .ws-att")?.textContent?.includes("queued.txt") === true);
    button("Queue").click();
    await until(()=>!!document.querySelector(".queue"));
    detach(); await mount(path);
    await until(()=>document.querySelector(".queue")?.textContent?.includes("Held across relaunch") === true);
    expect(document.querySelector(".queue")?.textContent).toContain("queued.txt");
    button("Edit").click();
    await until(()=>!document.querySelector(".queue") && (document.querySelector('[aria-label="Message"]') as HTMLTextAreaElement)?.value === "Held across relaunch");
    expect((document.querySelector('[aria-label="Message"]') as HTMLTextAreaElement).value).toBe("Held across relaunch");
    expect(document.querySelector(".dock .composer")?.textContent).toContain("queued.txt");
    input("Message", "Focus on durable conversation");
    button("Steer").click();
    await until(()=>document.querySelector(".timeline")?.textContent?.includes("Focus on durable conversation") === true);
    expect(document.querySelector(".queue")).toBeNull();
    detach(); await mount(path);
    expect(document.querySelector(".timeline")?.textContent).toContain("Focus on durable conversation");
    expect(document.querySelector(".timeline")?.textContent).toContain("first line");
    writeFileSync(project!+"/release-stream","");
    await until(()=>document.body.textContent!.includes("Runtime-owned journey result."));
    expect(document.body.textContent).not.toMatch(/accepted by you|reviewed and checked/);
    expect(document.querySelector(".rcard")).toBeTruthy();
    expect(document.querySelector(".rcard-h")?.textContent).toContain("Result v1");
    expect(existsSync(project!+"/steer-applied")).toBe(true);
    expect(existsSync(project!+"/resources-received")).toBe(true);
    expect(document.querySelector(".mock-chip")).toBeNull();
    button("Request changes…").click();
    const feedback = document.querySelector("dialog textarea") as HTMLTextAreaElement;
    feedback.value="Owner review feedback"; feedback.dispatchEvent(new Event("input",{bubbles:true}));
    button("Send and reopen").click();
    await until(()=>existsSync(project!+"/review-returned"));
    await until(()=>document.querySelector(".rcard-h")?.textContent?.includes("Result v2") === true);
    expect(document.body.textContent).toContain("Review feedback returned to Cortex.");
    const registered=await rpc("projects.list",{});
    const request={protocol:PROTOCOL,requestId:"read",projectId:registered[0].projectId,operation:"tasks.list"};
    expect((await rpc("workspace",request)).items).toHaveLength(1);
    // The rest of the app reads the same machine: its Git history, its storage and the owner's own decisions.
    const go = (to: string) => { history.pushState(null,"",to); window.dispatchEvent(new PopStateEvent("popstate")); };
    const text = () => document.body.textContent!;
    const taskPath = location.pathname, id = registered[0].projectId;
    go(`/m/local/p/${id}/git`);
    await until(()=>text().includes("Journey history"));
    go(`/settings/machine/local/storage`);
    await until(()=>text().includes("Storage and archive") && text().includes("journey"));
    go(`/settings/machine/local/orchestration/defaults`);
    await until(()=>text().includes("Coordinator"));
    go(taskPath);
    await until(()=>!!document.querySelector(".ws-title"));
    (document.querySelector(".ws-title") as HTMLElement).click();
    await until(()=>!!document.querySelector('[aria-label="Task title"]'));
    const title = document.querySelector<HTMLInputElement>('[aria-label="Task title"]')!;
    title.value = "Renamed by the owner"; title.dispatchEvent(new Event("input",{bubbles:true}));
    title.dispatchEvent(new KeyboardEvent("keydown",{key:"Enter",bubbles:true}));
    await until(()=>text().includes("Renamed by the owner"));
    button("New task").click();
    writeFileSync(project!+"/start-worker-scenario", "");
    input("Describe the task", "Exercise worker controls");
    button("Save as draft").click();
    await until(()=>all(".card-link").some(e => e.textContent?.includes("Exercise worker controls")));
    all(".card-link").find(e => e.textContent?.includes("Exercise worker controls"))!.click();
    button("Start").click();
    await until(()=>existsSync(project!+"/worker-one") && existsSync(project!+"/worker-two"));
    const workers = async (page: {workerId: string; cursor: string} | {} = {}) => {
      const tasks = (await rpc("workspace", request)).items;
      const task = tasks.find((t:any)=>t.description === "Exercise worker controls");
      const list = await rpc("machine.call",{op:"workers",projectId:id,params:{taskIds:[task.taskId],...page}});
      expect(list.ok).toBe(true);
      return list.result as any[];
    };
    let list=await workers();
    expect(list).toHaveLength(2);
    const one=list.find(w=>w.goal.includes("Receive owner guidance")), two=list.find(w=>w.goal.includes("Wait for command control"));
    expect(one).toBeTruthy(); expect(two).toBeTruthy();
    const workerPath=location.pathname + "/workers/";
    go(workerPath + one.id);
    // Routing renders the cached worker before the live refresh makes it writable.
    // Synthetic input on that disabled field is ignored by Solid's delegated handler.
    await until(()=>document.querySelector<HTMLTextAreaElement>('[aria-label="Message backend-dev"]')?.disabled === false);
    writeFileSync(project!+"/expect-older-history","");
    input("Message backend-dev", "Keep the worker message verbatim\nsecond line");
    const sendGuidance = document.querySelector<HTMLElement>('[aria-label="Send to backend-dev"]')!;
    expect(sendGuidance.getAttribute("aria-disabled")).not.toBe("true");
    sendGuidance.click();
    await until(()=>existsSync(project!+"/worker-guidance-delivered"));
    // This fixture persists 205 separate runtime events before publishing its result.
    await until(()=>text().includes("Worker guidance completed."), undefined, 30000);
    detach(); await mount(workerPath + one.id);
    await until(()=>text().includes("Worker guidance completed."));
    button("Open report").click();
    await until(()=>document.querySelector("dialog[open] .ws-result-copy h1")?.textContent === "Guidance result");
    expect(all("dialog[open] .ws-result-copy p").some(p => p.textContent === "Received the owner guidance at a Cortex call. No files changed.")).toBe(true);
    expect(document.querySelector("dialog[open] .ws-result-copy pre")).toBeNull();
    document.querySelector<HTMLElement>('[aria-label="Close report"]')!.click();
    list=await workers();
    // The snapshot is a tail of at most 200 rows. This guidance precedes the 205
    // retained events, so it arrives once, with its delivery state, on an older page.
    let tail=list.find(w=>w.id===one.id);
    expect(tail.activity.length).toBeLessThanOrEqual(200);
    expect(tail.activity.some((a:any)=>a.text === "Retained fixture event 204")).toBe(true);
    const guidance=[...tail.messages];
    while (tail.activityCursor) {
      [tail]=await workers({workerId:one.id,cursor:tail.activityCursor});
      guidance.push(...tail.messages);
    }
    expect(guidance).toEqual([expect.objectContaining({text:"Keep the worker message verbatim\nsecond line",delivery:"delivered"})]);
    // Every worker entry opens the full stream; no intermediate history link.
    // The byte budget can split even 205 calls across several retained pages.
    const openFolds=async()=>{ for (const f of all('.stream button.fold[aria-expanded="false"]')) f.click(); await new Promise(r => setTimeout(r, 30)); };
    const paths=()=>all(".stream .step-t").map(e => e.textContent);
    await openFolds();
    for (let pages=0; pages<10 && !paths().includes("fixture-history/0.txt"); pages++) {
      await until(()=>!!button("Load older activity"));
      button("Load older activity").click();
      await until(()=>!button("Loading older activity…"));
      expect(document.querySelector(".stream [role=alert]")).toBeNull();
      await openFolds();
    }
    const retainedPaths=paths();
    expect(Array.from({length:205},(_,i)=>`fixture-history/${i}.txt`).filter(path => !retainedPaths.includes(path))).toEqual([]);
    expect(retainedPaths).toContain("fixture-history/0.txt");
    expect(text()).toContain("Worker received owner guidance and inspected the project.");
    expect(text()).toContain("Keep the worker message verbatim");
    go(workerPath + two.id);
    await until(()=>!!button("Stop command…"));
    button("Stop command…").click();
    const reason=all("dialog textarea")[0] as HTMLTextAreaElement;
    reason.value="Owner requested a focused stop"; reason.dispatchEvent(new Event("input",{bubbles:true}));
    all("dialog button").find(b=>b.textContent?.trim()==="Stop command")!.click();
    await until(()=>existsSync(project!+"/worker-command-stopped"));
    button("Stop worker…").click();
    all("dialog button").find(b=>b.textContent?.trim()==="Stop worker")!.click();
    await until(()=>text().includes("Cancelled"));
    list=await workers();
    expect(list.find(w=>w.id===two.id)?.status).toBe("cancelled");
    expect(list.find(w=>w.id===one.id)?.result.summary).toBe("Worker guidance completed.");
    const workerTask=(await rpc("workspace", request)).items.find((t:any)=>t.description==="Exercise worker controls");
    await rpc("executions.cancel",{projectId:id,executionId:workerTask.executionId});
  } finally {
    detach(); for (const s of sockets) s.destroy(); listeners.clear();
  }
}, 60000);
