import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { capture, emptyWorkspace, LOCAL_MACHINE_ID, type EngineConnection, type MachineConfig, type Worker, type Message } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, snapshot, wireDecision, wireTask } from "./support";

/** The screens on the real port, with a machine that answers as the helper does, so what they show is what real data gives. */
const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"];
const repo = { projectId: "p", remote: "", tree: ["a.txt"], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "dev", defaultBranch: "main",
  checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "dev", path: "/cedar", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }] };
const worker = (patch: Partial<Worker>): Worker => ({ id: "w1", taskId: "task-1", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Connect the board", now: null, waitsFor: null, scope: [], route: { model: "opus", effort: "high" }, attempt: { n: 1, of: 1 }, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [{ sequence: 1, text: "Reading the board", kind: "note", at: null }], result: null, ...patch } as Worker);

function machine(tasks = [wireTask({ executionId: "e", phase: "execution", runtimeState: "EXECUTING", executionState: "running" })], workers: Worker[] = [], more: { version?: string; connections?: EngineConnection[]; config?: (config: MachineConfig) => void } = {}) {
  const config = structuredClone(emptyWorkspace().machines[0].config);
  config.surfaces.user_defaults = { ...config.surfaces.user_defaults, coordinator: { engine: "claude-agent-acp", model: "opus", effort: "high", parameters: {} }, mode: "auto", language: "ru" };
  config.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus", efforts: ["medium", "high"], observed: true }];
  // A route is offered only from a connected engine with the model enabled, as on a real machine.
  config.setup["claude-agent-acp"].status = "connected";
  config.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
  more.config?.(config);
  const product = {tasks:{},sprints:[],messages:[] as Message[]};
  const history: Record<string,unknown>[] = [];
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call", "executions.control"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(OFFERED),
    "project.snapshot": () => snapshot(String(4 + history.length), tasks),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
    "executions.prompt": message => {
      history.push({historyId:"h"+history.length,sequence:String(history.length+1),revision:"0",kind:"user",text:(message as {text:string}).text,toolStatus:null,availability:"available"});
      return {outcome:"recorded"};
    },
    "executions.control": () => ({outcome:"recorded"}),
    "history.page": () => ({items:history,nextCursor:null,highWatermark:String(history.length)}),
    "executions.get": () => ({ executionId: "e", taskId: "task-1", version: "2", runtimeTaskId: "r", acpSessionId: "s", runtimePlanRevision: "0", ownerEpoch: "3", state: "result", warnings: [], resultId: "res" }),
    "results.get": () => ({ kind: "answer", resultId: "res", taskId: "task-1", executionId: "e", version: "1", text: "Done", evidence: [], checks: [{ name: "pytest", outcome: "passed", evidenceRef: null }], observedWorkspaceDigest: null }),
    "tasks.create": message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "task-2", entityVersion: "1", recordedAt: "2026-11-05T00:00:00Z" }),
    "tasks.start": message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "task-2", entityVersion: "2", recordedAt: "2026-11-05T00:00:00Z" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params?: {action:string;message:Message;messageId:string} };
      if (op === "product.message" && params) {
        if (params.action === "add") product.messages.push(params.message);
        if (params.action === "remove") product.messages = product.messages.filter(m => m.id !== params.messageId);
        return {ok:true,result:{}};
      }
      if (op === "repo.snapshot") return { ok: true, result: repo };
      if (op === "product.read") return { ok: true, result: product };
      if (op === "workers") return { ok: true, result: workers };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: more.version ?? "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: more.connections ?? [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      return { ok: true, result: {} };
    },
  });
  return s;
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

async function open(s: ReturnType<typeof machine>, path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  await settled();
  return root;
}
const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim().startsWith(text));
const type = (label: string, value: string) => { const el = $<HTMLTextAreaElement>(`[aria-label="${label}"]`)!; el.value = value; el.dispatchEvent(new Event("input", { bubbles: true })); };
const prompts = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "executions.prompt").map(c => (c.message as { text: string }).text);
const TASK = `/m/${LOCAL_MACHINE_ID}/t/task-1`;

describe("task session on real data", () => {
  it("keeps planning visible in All work before it has created tasks", async () => {
    const s = machine(), answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => (message as {op:string}).op === "product.read"
      ? {ok:true,result:{tasks:{"task-1":{planning:true}},sprints:[{id:"sp",projectId:"p",planningTaskId:"task-1",description:"Plan the next release",title:"Next release",planning:"planning",runMode:"manual",targetBranch:null,conversation:[],plan:null,materializedVersion:null,taskIds:[],queueState:"idle",pauseReason:null}],messages:[]}}
      : answer(message);
    await open(s, `/m/${LOCAL_MACHINE_ID}/p/p`);
    expect(document.querySelector("main")?.textContent).toContain("Sprint: Next release");
    expect(document.querySelector("main")?.textContent).toContain("No tasks created yet");
    const link=[...document.querySelectorAll<HTMLAnchorElement>("main a")].find(a=>a.textContent === "Open planning");
    expect(link?.getAttribute("href")).toContain("/s/sp/planning");
    link!.click(); await settled();
    expect(document.querySelector("main")?.textContent).toContain("Plan the next release");
  });

  it("shows an execution worker question in the task, Inbox and sidebar without a global WAITING_USER", async () => {
    const s = machine();
    s.answers["decisions.list"] = () => ({items:[wireDecision()],nextCursor:null,highWatermark:"4"});
    await open(s, TASK);
    expect(document.querySelector(".timeline")?.textContent).toContain("Which?");
    const inbox = [...document.querySelectorAll<HTMLAnchorElement>("a")].find(a => a.getAttribute("href") === `/m/${LOCAL_MACHINE_ID}/inbox`);
    expect(inbox?.textContent).toContain("1");
    inbox?.click(); await settled();
    expect(document.querySelector(".main")?.textContent ?? document.body.textContent).toContain("Which?");
  });

  it("names the state and the runtime phase, and marks only that phase as now", async () => {
    await open(machine(), TASK);
    expect($(".tb-actions .chip")?.textContent).toContain("Working");
    const steps = all(".phases .phase");
    expect(steps.map(p => p.textContent)).toEqual(["Planned", "Executing", "Independent review", "Verification", "Completion"]);
    expect(steps.filter(p => p.classList.contains("now")).map(p => p.textContent)).toEqual(["Executing"]);
  });

  it("claims no progress for a wait, which says nothing about how far the task got", async () => {
    await open(machine([wireTask({ executionId: "e", phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" })]), TASK);
    expect($(".phases")).toBeNull();
  });

  it("opens the inspector, with what the machine sets for every task and no empty row", async () => {
    await open(machine(), TASK);
    const inspector = $(".inspector")!;
    expect(inspector.textContent).toContain("Branchdev");
    expect(inspector.textContent).toContain("Opus, High");
    expect(inspector.textContent).toContain("Auto");
    expect(inspector.textContent).toContain("Russian");
    expect(inspector.textContent).toContain("What this machine sets for every task.");
    expect(all(".inspector dd").filter(dd => !dd.textContent?.trim())).toEqual([]);
    expect($(".ws-app")?.getAttribute("data-inspector")).toBe("shown");
  });

  it("loads older guidance with its delivery and attachments through the combined cursor", async () => {
    const s = machine(undefined, [worker({activityCursor:"combined",activityPartial:true})]);
    const base = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => {
      const p = message as {op:string;params?:{cursor?:string}};
      if (p.op === "workers" && p.params?.cursor) return {ok:true,result:[worker({
        activity:[{sequence:0,kind:"message",text:"Older guidance",messageId:"old-guidance",at:null}],
        messages:[{id:"old-guidance",taskId:"task-1",workerId:"w1",text:"Older guidance",mode:"send_now",
          delivery:"delivered",at:null,deliveredAt:null,failure:null,attachments:["old-note.txt"]}],
        activityCursor:null,activityPartial:false
      })]};
      return base(message);
    };
    await open(s, `${TASK}/workers/w1/live`);
    button("Load older activity")!.click();
    await settled();
    expect(document.body.textContent).toContain("Older guidance");
    expect(document.body.textContent).toContain("old-note.txt");
    expect(document.body.textContent).toContain("Delivered does not mean read");
    expect(button("Load older activity")).toBeUndefined();
  });

  it("shows the workers in the conversation, in the pane and on their tab", async () => {
    const workers = [worker({}), worker({ id: "w2", role: "code-reviewer", status: "completed", lane: "review", result: { summary: "No findings", checks: [] } })];
    await open(machine(undefined, workers), TASK);
    expect(all(".wrow").map(r => r.textContent)).toEqual([expect.stringContaining("frontend-dev"), expect.stringContaining("code-reviewer")]);
    expect($(".inspector")?.textContent).toContain("1 active, 1 finished");
    dispose?.(); document.body.innerHTML = "";
    await open(machine(undefined, workers), `${TASK}/workers/w2`);
    expect($(".livepane .d-title")?.textContent).toContain("code-reviewer");
    expect($(".livepane")?.textContent).toContain("No findings");
  });

  it("keeps Follow off until the owner returns and folds a failing step", async () => {
    const activity: Worker["activity"] = [
      {sequence:1,kind:"note",text:"Worker prose",at:null},
      {sequence:2,kind:"tool",text:"Read failed",error:true,at:null},
      {sequence:3,kind:"edit",text:"Changed file",at:null},
    ];
    const workers = [worker({activity})], s = machine(undefined, workers);
    await open(s, `${TASK}/workers/w1/live`);
    expect($('[aria-label="Filter activity"]')).toBeNull();
    expect($(".stream")?.textContent).toContain("Worker prose");
    expect($(".stream")?.textContent).not.toContain("Read failed");
    expect($(".stream button.fold")?.textContent).toMatch(/1 tool call needs attention/);
    $(".stream button.fold")!.click(); await settled();
    expect($(".stream")?.textContent).toContain("Read failed");
    const stream = $(".stream")!;
    stream.scrollTop = -100; stream.dispatchEvent(new Event("scroll")); await settled();
    expect(document.body.dataset.follow).toBe("off");
    workers[0] = worker({activity:[activity[1],activity[2],{sequence:4,kind:"note",text:"New arrival",at:null}]});
    s.send({type:"refresh"}); await settled();
    expect(document.body.dataset.follow).toBe("off");
    expect(stream.scrollTop).toBe(-100);
    expect($(".new-pill")?.textContent).toContain("1 new");
    s.send({type:"refresh"}); await settled();
    expect($(".new-pill")?.textContent).toContain("1 new");
    $(".new-pill")!.click(); await settled();
    expect(document.body.dataset.follow).toBe("on");
    expect(stream.scrollTop).toBe(0);
  });

  // Three chunks, with a code block cut in two: only joining before parsing renders it as one block.
  const reportChunks = ["# Outcome\n\nOpening line\n```\nfirst half ", "second half\n```\nMiddle line\n", "Closing line"];
  const reportPage = (at: number) => ({reportId:"r_public",title:"Published report",sha256:"a".repeat(64),markdown:reportChunks[at],totalCharacters:reportChunks.join("").length,nextCursor:at<reportChunks.length-1?`cursor-${at+1}`:null,partial:at<reportChunks.length-1});
  const chunkOf = (request: unknown) => Number((request as {cursor?:string}).cursor?.slice("cursor-".length) ?? 0);
  const finishedReportWorker = async (answer: (request: unknown) => unknown) => {
    const workers = [worker({})];
    const s = machine(undefined, workers);
    s.answers["capabilities.get"] = () => capabilitiesWire([...OFFERED,"reports.read"]);
    s.answers["reports.read"] = answer;
    await open(s, `${TASK}/workers/w1`);
    workers[0] = worker({status:"completed",result:{summary:"Done",checks:[],reportId:"r_public"}});
    s.send({type:"refresh"}); await settled();
    return s;
  };
  const report = () => $('[aria-label="Worker report"]')!;

  it("opens only the referenced public report and shows all of it at once", async () => {
    const s = await finishedReportWorker(request => reportPage(chunkOf(request)));
    button("Open report")!.click(); await settled();
    s.send({type:"refresh"}); await settled();
    expect(report().textContent).toContain("Opening line");
    expect(report().textContent).toContain("Middle line");
    expect(report().textContent).toContain("Closing line");
    expect(report().querySelectorAll("pre")).toHaveLength(1);
    expect(report().querySelector("pre")?.textContent).toBe("first half second half");
    expect(button("Next page")).toBeUndefined();
    expect(report().textContent).not.toContain("Partial report page");
    expect(s.calls.filter(c=>c.operation==="reports.read").map(c=>c.message)).toEqual([
      expect.objectContaining({target:"r_public",limit:4000}),
      expect.objectContaining({target:"r_public",cursor:"cursor-1",limit:4000}),
      expect.objectContaining({target:"r_public",cursor:"cursor-2",limit:4000}),
    ]);
  });

  it("never shows part of a report as the report when a later chunk fails, and Retry reads it whole", async () => {
    let broken = true;
    const s = await finishedReportWorker(request => {
      const at = chunkOf(request);
      if (at === 1 && broken) throw new Error("socket closed");
      return reportPage(at);
    });
    button("Open report")!.click(); await settled();
    expect(report().querySelector('[role="alert"]')).not.toBeNull();
    expect(report().textContent).not.toContain("Opening line");
    broken = false;
    button("Retry")!.click(); await settled();
    expect(report().querySelector('[role="alert"]')).toBeNull();
    expect(report().textContent).toContain("Closing line");
    expect(s.calls.filter(c=>c.operation==="reports.read")).toHaveLength(5);
  });

  it("refuses to stitch a report that changed between chunks", async () => {
    await finishedReportWorker(request => {
      const at = chunkOf(request);
      return at ? {...reportPage(at),sha256:"b".repeat(64)} : reportPage(at);
    });
    button("Open report")!.click(); await settled();
    expect(report().querySelector('[role="alert"]')?.textContent).toContain("The report changed");
    expect(report().textContent).not.toContain("Opening line");
  });

  it("stops reading a report once it is closed, and reopening starts clean", async () => {
    let release = () => {}, reads = 0;
    const held = new Promise<void>(resolve => { release = resolve; });
    await finishedReportWorker(async request => {
      if (++reads === 2) await held;
      return reportPage(chunkOf(request));
    });
    button("Open report")!.click(); await settled();
    $('[aria-label="Close report"]')!.click(); await settled();
    release(); await settled();
    expect(reads).toBe(2);
    expect(report().hasAttribute("open")).toBe(false);
    button("Open report")!.click(); await settled();
    expect(report().querySelector("pre")?.textContent).toBe("first half second half");
    expect(reads).toBe(5);
  });

  it("gives a task with no workers the whole width", async () => {
    await open(machine(), TASK);
    expect($(".detail")).toBeNull();
    expect($(".detail-toggle")).toBeNull();
  });

  it("reports the runtime's verification, never a fixed Not run", async () => {
    await open(machine([wireTask({ executionId: "e", phase: "result", runtimeState: "COMPLETED", executionState: "result" })]), `${TASK}/workers/verification`);
    expect($(".wlist")?.textContent).toContain("1 passed");
    expect($(".livepane")?.textContent).toContain("pytest");
    expect($(".livepane")?.textContent).not.toContain("Not run");
  });

  it("describes Stop as a resumable pause", async () => {
    await open(machine(), TASK);
    expect(all(".tb-actions button").find(b => b.textContent?.includes("Stop"))?.textContent).toBe("Stop");
  });

  it("offers no terminal it does not have", async () => {
    await open(machine(), TASK);
    expect(button("Open terminal here")).toBeUndefined();
  });
});

describe("questions on real data", () => {
  const waiting = wireTask({ executionId: "e", phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" });
  const batch = [wireDecision({ decisionId: "d1", questionId: "q1", batchId: "b", prompt: "Which file?", options: [{ optionId: "o1", label: "First" }] }), wireDecision({ decisionId: "d2", questionId: "q2", batchId: "b", prompt: "Which branch?", options: [{ optionId: "o2", label: "Second" }] })];
  const opt = (text: string) => all(".dc .opt").find(o => o.textContent?.includes(text))!;

  it("shows the exact pending question while paused without submitting an answer", async () => {
    const s = machine([wireTask({executionId:"e",phase:"paused",runtimeState:"PAUSED",executionState:"paused"})]);
    s.answers["decisions.list"] = () => ({items:[{...batch[0],allowedOperations:[]}],nextCursor:null,highWatermark:"4"});
    await open(s,TASK);
    expect(document.body.textContent).toContain("Which file?");
    expect(document.body.textContent).toContain("Resume the task to answer");
    expect(s.calls.filter(c=>c.operation==="decisions.answer")).toEqual([]);
  });

  it("keeps a batch's answers until the last one, then sends them as one", async () => {
    const s = machine([waiting]);
    s.answers["decisions.list"] = () => ({ items: batch, nextCursor: null, highWatermark: "4" });
    s.answers["decisions.answer"] = message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "b", entityVersion: "3", recordedAt: "2026-11-05T00:00:00Z" });
    await open(s, TASK);
    opt("First").click();
    await settled();
    expect(s.calls.filter(c => c.operation === "decisions.answer")).toEqual([]);
    expect(all('.dc-tab[aria-label$="answered"]').map(t => t.dataset.ask)).toEqual(["d1"]);
    expect(all(".dc")).toHaveLength(1);
    opt("Second").click();
    await settled();
    const [sent] = s.calls.filter(c => c.operation === "decisions.answer").map(c => c.message as { target: string; payload: { batchId: string; answers: { text: string }[] } });
    expect(sent.target).toBe("b");
    expect(sent.payload.answers.map(a => a.text)).toEqual(["First", "Second"]);
  });

  it("reopens a batch the machine refused, so nothing reads as answered that was not", async () => {
    const s = machine([waiting]);
    s.answers["decisions.list"] = () => ({ items: batch, nextCursor: null, highWatermark: "4" });
    s.answers["decisions.answer"] = message => ({ requestId: (message as { requestId: string }).requestId, outcome: "busy", target: "b", entityVersion: "2", recordedAt: "2026-11-05T00:00:00Z" });
    await open(s, TASK);
    opt("First").click();
    await settled();
    opt("Second").click();
    await settled();
    expect($(".ws-receipt")?.textContent).toMatch(/still running/i);
    expect(all(".dc-tab")).toHaveLength(2);
    expect(all('.dc-tab[aria-label$="answered"]')).toEqual([]);
  });
});

describe("messages on real data", () => {
  it("uses native empty-clipboard/drop fallbacks in Other answer and preserves multiline answer bytes", async () => {
    const s = machine();
    const attachments = [{name:"clip.png",mimeType:"image/png",data:btoa("fixture png")}];
    s.answers["service.capabilities"] = () => ({methods:["machine.call","native.clipboard_image","native.take_dropped_files"],machine:true});
    s.answers["capabilities.get"] = () => ({...capabilitiesWire(OFFERED),attachments:true});
    s.answers["native.clipboard_image"] = () => attachments;
    s.answers["native.take_dropped_files"] = () => [{name:"drop.txt",mimeType:"text/plain",data:btoa("drop")}];
    s.answers["decisions.list"] = () => ({items:[wireDecision()],nextCursor:null,highWatermark:"4"});
    s.answers["decisions.answer"] = message => ({requestId:(message as any).requestId,target:"d",outcome:"recorded",entityVersion:"3",recordedAt:"2026-11-06T00:00:00Z"});
    await open(s,TASK);
    all("button").find(b => b.textContent?.includes("Other answer"))!.click();
    type("Your answer","first line\nsecond line");
    const textarea = document.querySelector('[aria-label="Your answer"]')!;
    const paste = new Event("paste",{bubbles:true,cancelable:true});
    Object.defineProperty(paste,"clipboardData",{value:{files:[],types:[]}});
    textarea.dispatchEvent(paste);
    const drop = new Event("drop",{bubbles:true,cancelable:true});
    Object.defineProperty(drop,"dataTransfer",{value:{files:[],types:["text/uri-list"]}});
    textarea.dispatchEvent(drop);
    await settled();
    expect(document.querySelector(".ws-dc-other")?.textContent).toMatch(/clip.png.*drop.txt/s);
    button("Send answer")!.click();
    await settled();
    const sent = s.calls.find(c => c.operation === "decisions.answer")!.message as any;
    expect(sent.payload.answers[0]).toMatchObject({text:"first line\nsecond line",attachments:[attachments[0],{name:"drop.txt",data:btoa("drop")}]});
    expect(s.calls.filter(c => c.operation === "native.clipboard_image")).toHaveLength(1);
  });

  it("reads picker, drop and paste bytes; queue Edit restores them and a failed send keeps them", async () => {
    const s = machine();
    s.answers["capabilities.get"] = () => ({...capabilitiesWire(OFFERED),attachments:true});
    await open(s, TASK);
    const file = new File(["drop fixture"],"drop.txt",{type:"text/plain"});
    const picker = document.querySelector<HTMLInputElement>('.dock input[type="file"]')!;
    Object.defineProperty(picker, "files", {configurable:true,value:[new File(["picked"],"pick.txt",{type:"text/plain"})]});
    picker.dispatchEvent(new Event("change",{bubbles:true}));
    const drop = new Event("drop",{bubbles:true,cancelable:true});
    Object.defineProperty(drop,"dataTransfer",{value:{files:[file],types:["Files"]}});
    document.querySelector('[aria-label="Message"]')!.dispatchEvent(drop);
    const paste = new Event("paste",{bubbles:true,cancelable:true});
    Object.defineProperty(paste,"clipboardData",{value:{files:[new File(["png fixture"],"paste.png",{type:"image/png"})]}});
    document.querySelector('[aria-label="Message"]')!.dispatchEvent(paste);
    await settled();
    expect(document.querySelector(".composer")?.textContent).toMatch(/pick.txt.*drop.txt.*paste.png/s);
    type("Message","With files");
    button("Queue")!.click();
    await settled();
    expect($(".queue")?.textContent).toMatch(/pick.txt.*drop.txt.*paste.png/s);
    button("Edit")!.click();
    await settled();
    expect($(".queue")).toBeNull();
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("With files");
    s.answers["executions.prompt"] = () => { throw new Error("fixture lost acknowledgement"); };
    button("Steer")!.click();
    await settled();
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("With files");
    expect(document.querySelector(".dock .composer")?.textContent).toContain("paste.png");
    const sent = s.calls.find(c => c.operation === "executions.prompt")!.message as {attachments:{name:string;data:string}[]};
    expect(sent.attachments.map(a => [a.name,atob(a.data)])).toEqual([["pick.txt","picked"],["drop.txt","drop fixture"],["paste.png","png fixture"]]);
  });

  it("holds a queued message unsent, and Edit brings it back to the field", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Use the other file");
    button("Queue")!.click();
    await settled();
    expect($(".queue")?.textContent).toContain("Use the other file");
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("");
    expect(prompts(s)).toEqual([]);
    button("Edit")!.click();
    await settled();
    expect($(".queue")).toBeNull();
    expect($<HTMLTextAreaElement>('[aria-label="Message"]')?.value).toBe("Use the other file");
    expect(prompts(s)).toEqual([]);
  });

  it("Send now delivers a queued message once, and it then reads as said", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Second thought");
    button("Queue")!.click();
    await settled();
    button("Send now")!.click();
    await settled();
    expect(prompts(s)).toEqual(["Second thought"]);
    expect($(".queue")).toBeNull();
    expect($(".timeline")?.textContent).toContain("Second thought");
  });

  it("keeps a queued message when its send fails, and sends nothing twice", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Keep me");
    button("Queue")!.click();
    await settled();
    s.answers["executions.prompt"] = () => { throw new Error("socket closed"); };
    button("Send now")!.click();
    await settled();
    expect($(".queue")?.textContent).toContain("Keep me");
    expect($(".ws-receipt")?.textContent).toMatch(/not known/);
  });

  it("a steer reaches the machine and stays in the conversation", async () => {
    const s = machine();
    await open(s, TASK);
    type("Message", "Mind the race");
    button("Steer")!.click();
    await settled();
    expect(prompts(s)).toEqual(["Mind the race"]);
    expect($(".timeline")?.textContent).toContain("Mind the race");
  });

  it("targets a worker safe point, never generic coordinator steering", async () => {
    const s = machine(undefined, [worker({})]);
    await open(s, `${TASK}/workers/w1`);
    const field = $<HTMLTextAreaElement>('[aria-label="Message frontend-dev"]')!;
    expect(field.placeholder).toContain("directly");
    expect($(".livepane .c-route")?.textContent).toContain("next");
    type("Message frontend-dev", "Keep the old colors");
    $<HTMLElement>('[aria-label="Send to frontend-dev"]')!.click();
    await settled();
    expect(prompts(s)).toEqual([]);
    expect(s.calls.find(c => c.operation === "executions.control")?.message).toMatchObject({action:"worker_message",workerId:"w1",text:"Keep the old colors"});
  });

  it("confirms a stop for one worker without stopping the whole task", async () => {
    const s = machine(undefined, [worker({})]);
    await open(s, `${TASK}/workers/w1`);
    const stop = all(".livepane button").find(b => b.textContent?.startsWith("Stop"))!;
    expect(stop.getAttribute("aria-disabled")).not.toBe("true");
    stop.click();
    await settled();
    expect($("dialog[open]")).not.toBeNull();
    all("dialog[open] button").find(b => b.textContent?.trim() === "Stop worker")!.click();
    await settled();
    expect(s.calls.find(c => c.operation === "executions.control")?.message).toMatchObject({action:"worker_stop",workerId:"w1"});
  });
});

describe("new task on real data", () => {
  const NEW = `/m/${LOCAL_MACHINE_ID}/new`;
  const radio = (text: string) => all<HTMLLabelElement>("label.rc").find(l => l.textContent?.includes(text))!;

  it("offers task worktrees with the real branch base", async () => {
    await open(machine([]), NEW);
    expect(radio("Separate worktree").querySelector("input")!.disabled).toBe(false);
    expect(radio("Separate worktree").textContent).toMatch(/currently open branch/);
    expect(radio("Separate worktree").querySelector("input")!.checked).toBe(true);
    expect(radio("Main checkout").textContent).toContain("dev");
  });

  it("offers per-task routing and a target branch choice", async () => {
    await open(machine([]), NEW);
    const route = all(".c-route .sel");
    expect(route.map(b => b.textContent?.trim())).toEqual(["Opus", "High", "Auto", "Russian"]);
    for (const select of route) expect(select.getAttribute("aria-disabled")).not.toBe("true");
    expect($('[aria-label="Merge into main"]')).toBeTruthy();
  });

  it("Start now creates the task and starts it", async () => {
    const s = machine([]);
    await open(s, NEW);
    type("Describe the task", "Fix the race");
    button("Start now")!.click();
    await settled();
    expect(s.calls.map(c => c.operation)).toEqual(expect.arrayContaining(["tasks.create", "tasks.start"]));
  });
});

describe("what no screen says", () => {
  const ID = /\b(?:xr|cn)_[0-9a-f]{6,}|sha256|[0-9a-f]{16,}/;
  const claude: EngineConnection = { id: "cn_ab12cd34ef56", engine: "claude", label: "Claude", note: "Native", sessions: 4, state: "connected", checkedAt: null,
    detail: { connectionId: "cn_ab12cd34ef56", provider: "anthropic", authMode: "native", keyKind: null, endpoint: null, launchCommand: "adapter", credential: "native", qualified: true, createdAt: "2026-11-05", updatedAt: "2026-11-05" } };
  const owned = () => machine(undefined, undefined, { version: "1.0.6+sha256.94ba5360d3d1f14d", connections: [claude], config: c => { c.catalogue["claude-agent-acp"].models = [{ id: "opus", routeId: "xr_6b08617e228a19a17e2474f3", label: "Opus 5.5", efforts: ["medium", "high"], observed: true, contextWindow: 1000000 }]; c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"]; } });
  const said = () => [document.body.textContent ?? "", ...[...document.querySelectorAll("[title], [aria-label], [placeholder]")].flatMap(el => ["title", "aria-label", "placeholder"].map(a => el.getAttribute(a) ?? ""))].join("\n");
  const settings = (page: string) => `/settings/machine/${LOCAL_MACHINE_ID}/${page}`;

  it("keeps route, connection and build ids off every page but Diagnostics", async () => {
    for (const path of [settings("overview"), settings("connections"), settings("orchestration/models"), settings("orchestration/defaults"), settings("orchestration/routing"), settings("orchestration/policy"), "/settings/app/machines", `/m/${LOCAL_MACHINE_ID}/new`]) {
      await open(owned(), path);
      expect(said(), path).not.toMatch(ID);
      dispose?.(); dispose = undefined; document.body.innerHTML = "";
    }
    await open(owned(), settings("diagnostics"));
    expect(document.body.textContent).toContain("1.0.6+sha256.94ba5360d3d1f14d");
    expect(document.body.textContent).toContain("cn_ab12cd34ef56");
  });
  it("reads a version as the version people talk about", async () => {
    await open(owned(), settings("overview"));
    expect(document.body.textContent).toContain("1.0.6");
  });
  it("never shows a route id as a model's name, even once the engines stop offering it", async () => {
    const s = machine([wireTask({ executionId: "e", phase: "execution", runtimeState: "EXECUTING", executionState: "running" })], [], { config: c => { c.surfaces.user_defaults.coordinator = null; c.surfaces.user_defaults.routeDefault = { routeId: "xr_6b08617e228a19a17e2474f3", connectionId: "cn_ab12cd34ef56", model: "gone", parameters: {} }; } });
    await open(s, `/m/${LOCAL_MACHINE_ID}/new`);
    expect(said()).not.toMatch(ID);
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    await open(s, settings("orchestration/defaults"));
    expect(document.body.textContent).toContain("No longer registered");
    expect(said()).not.toMatch(ID);
  });
  it("says in words what differs from the profile, and tiers read from light to flagship", async () => {
    const s = machine(undefined, undefined, { connections: [claude], config: c => {
      c.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus 5.5", efforts: ["high"], observed: true }];
      c.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
      c.profiles = [{ id: "default", name: "default", description: "", updatedAt: null, sections: capture({ ...c.surfaces, routing: { ...c.surfaces.routing, engines: { ...c.surfaces.routing.engines, "claude-agent-acp": { ...c.surfaces.routing.engines["claude-agent-acp"], enabled: [] } } } }, { cn_ab12cd34ef56: 2 }) }];
      c.active = "default";
    } });
    await open(s, settings("orchestration/models"));
    const diff = [...document.querySelectorAll(".ws-diff")].map(d => d.textContent).join(" ");
    expect(diff).toContain("Opus 5.5 turned on");
    expect(diff).toContain("Claude: 4 sessions at most, was 2");
    expect(diff).not.toMatch(ID);
    expect(document.querySelector(".chip.st-warn")?.textContent).toBe("Modified (2)");
    expect([...document.querySelector(".tier")!.querySelectorAll("button")].map(b => b.textContent)).toEqual(["L", "S", "D", "F"]);
    expect([...document.querySelectorAll(".sw-l")].map(l => l.textContent?.trim())).toContain("Use");
  });
  it("edits a title by clicking it, with no second control beside it", async () => {
    await open(machine(), `/m/${LOCAL_MACHINE_ID}/t/task-1`);
    expect(document.querySelector(".brief")?.textContent).not.toMatch(/Title (generated|set)|Edit|Rename/);
    expect(document.querySelector(".ws-title")).toBeTruthy();
  });
  it("has nothing to count on an empty project, and says no zero", async () => {
    await open(machine([]), `/m/${LOCAL_MACHINE_ID}`);
    expect([...document.querySelectorAll(".n, .count, .task-row.all")].map(n => n.textContent?.trim())).not.toContain("0");
    expect(document.body.textContent).not.toMatch(/All 0/);
  });
});
