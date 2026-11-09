import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort, type ProjectLocator } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, snapshot, wireDecision, wireTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim().endsWith(text))!;
async function settle() { for (let i = 0; i < 8; i++) await flush(); }
function input(label: string, value: string) {
  const el = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[aria-label="${label}"]`)!;
  el.value = value; el.dispatchEvent(new Event("input", { bubbles: true }));
}

it("registers, explicitly activates, saves then starts, reopens a question and streaming, and reads the runtime result", async () => {
  let registered = false, activated = false, phase = "empty", answer: any, refusedOnce = false;
  const task = () => wireTask({ taskId: "t", description: "Explain the project", executionId: phase === "draft" ? null : "e",
    phase: phase === "draft" ? "not_started" : phase === "question" ? "waiting" : phase === "result" ? "result" : "execution",
    runtimeState: phase === "draft" ? null : phase === "question" ? "WAITING_USER" : phase === "result" ? "COMPLETED" : "EXECUTING",
    executionState: phase === "draft" ? "not_started" : phase === "question" ? "waiting_user" : phase === "result" ? "result" : "running",
    version: phase === "draft" ? "1" : "2" });
  let version = 1;
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["projects.list", "projects.register", "projects.activate"], sessionOptions: false }),
    "projects.list": () => registered ? [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: activated ? {} : null }] : [],
    "projects.register": () => { registered = true; return { projectId: "p" }; },
    "projects.status": () => ({ready:activated}),
    "projects.activate": () => { activated = true; return { ready: true }; },
    "capabilities.get": () => capabilitiesWire(activated ? ["project.snapshot","tasks.create","tasks.start","decisions.list","decisions.answer","results.get","history.page"] : []),
    "project.snapshot": () => snapshot(String(++version), phase === "empty" ? [] : [task()]),
    "decisions.list": () => ({ items: phase === "question" ? [wireDecision({ executionId: "e" })] : [], nextCursor: null, highWatermark: String(version) }),
    "history.page": () => ({ items: phase === "stream" || phase === "result" ? [{historyId:"h",sequence:"1",revision:"1",kind:"agent_message_chunk",text:"Observed streamed progress",toolStatus:null,availability:"available"}] : [], nextCursor: null, highWatermark: String(version) }),
    "tasks.create": (m: any) => { expect(m.payload).toEqual({description:"Explain the project",checkout:{kind:"main"}}); phase = "draft"; return {requestId:m.requestId,outcome:"recorded",target:"t",entityVersion:"1",recordedAt:"2026-11-05T00:00:00Z"}; },
    "tasks.start": (m: any) => { phase = "question"; return {requestId:m.requestId,outcome:"recorded",target:"t",entityVersion:"2",recordedAt:"2026-11-05T00:00:00Z"}; },
    "decisions.answer": (m: any) => { if (!refusedOnce) { refusedOnce=true; return {requestId:m.requestId,outcome:"stale_version",target:"d",entityVersion:"3",recordedAt:"2026-11-05T00:00:00Z"}; } answer=m; phase="stream"; return {requestId:m.requestId,outcome:"recorded",target:"d",entityVersion:"3",recordedAt:"2026-11-05T00:00:00Z"}; },
    "executions.get": () => ({executionId:"e",taskId:"t",version:"2",runtimeTaskId:"runtime",acpSessionId:"s",runtimePlanRevision:"0",ownerEpoch:"3",state:"result",warnings:[],resultId:"r"}),
    "results.get": () => ({kind:"answer",resultId:"r",taskId:"t",executionId:"e",version:"1",text:"Authoritative runtime result",evidence:[],checks:[],observedWorkspaceDigest:null}),
  });
  const mount = async (path = "/") => {
    history.replaceState(null, "", path);
    const root = document.createElement("div"); document.body.append(root);
    dispose = render(() => <Workspace port={createRealPort({transport:s.transport, actorId:"peer", registry:true})} />, root);
    await settle(); return root;
  };
  await mount();
  button("Add project").click(); await settle();
  [...document.querySelectorAll<HTMLElement>('dialog[open] [role="option"]')].find(o => o.textContent?.startsWith("Search for folder"))!.click(); await settle();
  input("Search for folder", "/cedar"); await settle();
  document.querySelector('dialog[open] input')!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); await settle();
  expect(activated).toBe(false);
  expect(s.calls.some(c => c.operation === "tasks.create")).toBe(false);
  button("Activate").click(); await settle();
  button("New task").click(); await settle();
  input("Describe the task", "Explain the project");
  button("Save as draft").click(); await settle();
  expect(s.calls.filter(c => c.operation === "tasks.start")).toHaveLength(0);
  (document.querySelector(".card-link") as HTMLElement).click(); await settle();
  button("Start").click(); await settle();
  expect(document.body.textContent).toContain("Which?");
  const path = location.pathname;
  dispose!(); document.body.innerHTML = "";
  await mount(path);
  button("Other answer").click(); await settle();
  input("Your answer", "first line\nsecond line");
  button("Send answer").click(); await settle();
  expect(s.calls.filter(c => c.operation === "decisions.answer")).toHaveLength(1);
  expect((document.querySelector('[aria-label="Your answer"]') as HTMLTextAreaElement).value).toBe("first line\nsecond line");
  expect(document.querySelector(".ws-receipt")?.textContent).toMatch(/changed on the machine/);
  button("Send answer").click(); await settle();
  expect(answer.payload.answers[0]).toMatchObject({questionId:"q",decisionId:"d",optionId:null,text:"first line\nsecond line"});
  expect(document.body.textContent).toContain("Observed streamed progress");
  dispose!(); document.body.innerHTML = "";
  await mount(path);
  expect(document.body.textContent).toContain("Observed streamed progress");
  phase="result"; s.send({type:"invalidated",projectId:"p"}); await settle();
  expect(document.body.textContent).toContain("Authoritative runtime result");
  expect(document.body.textContent).not.toMatch(/accepted by you|reviewed and checked/);
  s.send({type:"connection",state:"disconnected"}); await settle();
  expect(document.body.textContent).toMatch(/Last confirmed.*old/);
  expect(document.querySelector('[aria-label="Task views"]')?.textContent).not.toMatch(/Code|Terminal|Changes/);
  expect(s.calls.filter(c => c.operation === "tasks.start")).toHaveLength(1);
});
