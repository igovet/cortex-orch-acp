import { render } from "solid-js/web";
import { createRoot, createSignal } from "solid-js";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type CommandReceipt, type Decision, type Message, type Sprint, type Task, type TimelineEntry, type WorkspaceCommand, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createSends } from "../src/sends";
import { flush, projection, scriptedPort, withTask } from "./support";

// What the owner sends reads at once where it went, and the machine's record takes its place without moving or doubling it.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const named = (label: string, within = "body") => all<HTMLButtonElement>(`${within} button`).find(b => b.getAttribute("aria-label") === label || b.textContent?.trim() === label);

const live = { status: "working", process: "live", executionId: "e" } as Partial<Task>;
const question = (patch: Partial<Decision> = {}): Decision => ({ id: "d1", kind: "question", from: "QA Engineer", workerId: null, at: null, text: "Which concurrency level?", options: [{ label: "50 parallel", optionId: "o1" }, { label: "200 parallel", optionId: "o2" }], answer: null, withdrawn: false, ...patch });
function world(task: Partial<Task>, timeline: TimelineEntry[] = [], decisions: Decision[] = [], messages: Message[] = []) {
  const state = withTask(emptyWorkspace(), "Fix token refresh race");
  Object.assign(state.tasks[0], task);
  const session = emptySession("t");
  session.decisions = decisions;
  session.timeline = [...timeline, ...decisions.map(d => ({ id: `e-${d.id}`, kind: "decision" as const, decisionId: d.id }))];
  state.sessions = [session];
  state.messages = messages;
  state.machines[0].connection = "connected";
  return state;
}
let version = 1;
async function mount(state: WorkspaceState) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection(String(++version), state)); await settle();
  return p;
}
const typeMessage = (text: string) => { const el = $<HTMLTextAreaElement>('.dock [aria-label="Message"]')!; el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); return el; };
const said = (id: string, text: string): TimelineEntry => ({ id, kind: "said", text });
const reply = (id: string, text: string): TimelineEntry => ({ id, kind: "cortex", text });
/** The owner's lines of the conversation, in the order they read. */
const mine = () => all(".timeline .mine").map(m => m.textContent?.trim());
const sends = (p: ReturnType<typeof scriptedPort>) => p.commands.filter(c => c.payload.type === "send_message");

it("shows a sent message before the machine answers, then its record takes its place without a second copy or a move", async () => {
  const before = [said("h1", "Earlier"), reply("h2", "Reply one")];
  const p = await mount(world(live, before));
  typeMessage("Lock the row");
  named("Send message now")!.click();
  // Nothing has answered yet: the message reads already, and the field is free for the next one.
  expect($('.timeline [data-sent="sending"]')?.textContent).toContain("Lock the row");
  expect($(".timeline .ws-sent-meta")?.textContent).toContain("Sending…");
  expect($<HTMLTextAreaElement>('.dock [aria-label="Message"]')!.value).toBe("");
  expect(mine()).toEqual(["Earlier", "Lock the row"]);
  p.receipt(); await settle();
  expect($('.timeline [data-sent="sent"]')?.textContent).toContain("Lock the row");
  // The history reread brings the record, and the reply after it.
  p.emit(projection(String(++version), world(live, [...before, said("h3", "Lock the row"), reply("h4", "On it")]))); await settle();
  expect($(".timeline [data-sent]")).toBeNull();
  expect(mine()).toEqual(["Earlier", "Lock the row"]);
  const record = all(".timeline .mine").find(m => m.textContent?.includes("Lock the row"))!, answer = all(".timeline .ai").find(a => a.textContent?.includes("On it"))!;
  expect(record.compareDocumentPosition(answer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  // A record of the same words that was already there is not this send's record.
  typeMessage("Earlier");
  named("Send message now")!.click(); p.receipt(); await settle();
  expect($('.timeline [data-sent="sent"]')?.textContent).toContain("Earlier");
  p.emit(projection(String(++version), world(live, [...before, said("h3", "Lock the row"), reply("h4", "On it"), said("h5", "Earlier")]))); await settle();
  expect($(".timeline [data-sent]")).toBeNull();
  expect(mine()).toEqual(["Earlier", "Lock the row", "Earlier"]);
});

it("matches a saved copy by the send's own identity", async () => {
  const p = await mount(world(live));
  typeMessage("See the file");
  named("Send message now")!.click(); p.receipt(); await settle();
  const id = sends(p)[0].requestId;
  const saved: Message = { id, taskId: "t", workerId: null, text: "See the file", mode: "send_now", delivery: "recorded", at: 1, deliveredAt: null, failure: null };
  p.emit(projection(String(++version), world(live, [{ id: `facts:message:${id}`, kind: "message", messageId: id }], [], [saved]))); await settle();
  expect($(".timeline [data-sent]")).toBeNull();
  expect(mine()).toEqual(["See the file"]);
});

it("says a send did not arrive, keeps its words on its line, and sends it again in the same place", async () => {
  const p = await mount(world(live, [said("h1", "Earlier")]));
  typeMessage("Lock the row");
  named("Send message now")!.click();
  p.receipt(false); await settle();
  expect($('.timeline [data-sent="failed"]')?.textContent).toContain("Lock the row");
  expect($('.timeline .ws-sent-meta [role="alert"]')?.textContent).toBe("Not sent: Refused");
  expect(mine()).toEqual(["Earlier", "Lock the row"]);
  named("Retry", ".timeline .ws-sent-meta")!.click(); await settle();
  expect(sends(p).map(c => (c.payload as Extract<WorkspaceCommand, { type: "send_message" }>).text)).toEqual(["Lock the row", "Lock the row"]);
  expect(all(".timeline [data-sent]").map(e => e.getAttribute("data-sent"))).toEqual(["sending"]);
  expect(mine()).toEqual(["Earlier", "Lock the row"]);
  p.receipt(); await settle();
  expect($('.timeline [data-sent="sent"]')).not.toBeNull();
});

it("queues a message at once and its saved row replaces it, by the send's identity", async () => {
  const p = await mount(world(live));
  all<HTMLButtonElement>(".mc-delivery button").find(b => b.getAttribute("aria-label") === "Queue")!.click(); await settle();
  typeMessage("Then add the test");
  named("Queue message")!.click();
  expect($('.queue [data-sent="sending"]')?.textContent).toContain("Then add the test");
  expect($(".queue-h")?.textContent).toContain("Queued 1");
  p.receipt(); await settle();
  const id = sends(p)[0].requestId;
  const queued: Message = { id, taskId: "t", workerId: null, text: "Then add the test", mode: "queue", delivery: "queued", at: 1, deliveredAt: null, failure: null };
  p.emit(projection(String(++version), world(live, [], [], [queued]))); await settle();
  expect($(".queue [data-sent]")).toBeNull();
  expect(all(".queue .qrow").map(r => r.querySelector(".qrow-t")?.textContent)).toEqual(["Then add the test"]);
});

it("reads an answer as given at the press, keeps it through a refusal, and settles once on the record", async () => {
  const p = await mount(world(live, [], [question()]));
  all(".dc-panel .opt").find(o => o.textContent?.includes("50 parallel"))!.click(); await settle();
  $<HTMLElement>('.dc [aria-label="Send answer"]')!.click();
  expect(all(".dc .qa-a").map(a => a.textContent)).toEqual(["50 parallel"]);
  expect($('.dc [role="status"]')?.textContent).toBe("Sending the answer…");
  expect($<HTMLElement>(".dc-panel")!.hidden).toBe(true);
  p.receipt(false); await settle();
  expect(all(".dc .qa")).toEqual([]);
  expect(all(".opt").filter(o => o.getAttribute("aria-checked") === "true").map(o => o.querySelector(".t")?.textContent)).toEqual(["50 parallel"]);
  named("Retry", ".dc")!.click(); await settle();
  expect(p.commands.filter(c => c.payload.type === "answer_decision")).toHaveLength(2);
  expect(all(".dc .qa-a").map(a => a.textContent)).toEqual(["50 parallel"]);
  p.receipt(); await settle();
  expect($('.dc [role="status"]')?.textContent).toBe("Answer sent; work continues with it.");
  p.emit(projection(String(++version), world(live, [], [question({ answer: { text: "50 parallel", at: 2 } })]))); await settle();
  expect(all(".timeline .qa-a").map(a => a.textContent)).toEqual(["50 parallel"]);
  expect($(".dc-panel")).toBeNull();
});

const sprint = (conversation: Sprint["conversation"]): Sprint => ({ id: "sp", projectId: "p", description: "Coffee landing", title: "Coffee landing", runMode: "manual", targetBranch: null, planning: "ready", conversation, plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null });
it("matches a worker message by its request id and a planner message among the lines after it, each once", () => createRoot(done => {
  const [state, setState] = createSignal<WorkspaceState>({ ...emptyWorkspace(), sprints: [sprint([{ kind: "message", text: "Shorter" }])] });
  const receipts: string[] = [];
  const sends = createSends(state, command => { receipts.push(command.type); return Promise.resolve({ requestId: "x", ok: true, outcome: "recorded", message: "" } as CommandReceipt); });
  const worker = { kind: "worker" as const, taskId: "t", workerId: "w" }, planner = { kind: "sprint" as const, sprintId: "sp" };
  sends.begin("r1", { type: "send_message", taskId: "t", workerId: "w", text: "Stop editing", mode: "send_now" });
  sends.begin("r2", { type: "plan_message", sprintId: "sp", text: "Shorter" });
  sends.begin("r3", { type: "plan_message", sprintId: "sp", text: "Shorter" });
  expect(sends.shown(worker).map(e => e.text)).toEqual(["Stop editing"]);
  // The earlier line of the same words is not either send's record.
  expect(sends.shown(planner).map(e => e.key)).toEqual(["r2", "r3"]);
  const ok: CommandReceipt = { requestId: "", ok: true, outcome: "recorded", message: "" };
  for (const id of ["r1", "r2", "r3"]) sends.settle(id, ok);
  // A worker's message of other words with the send's id is its record; text alone from before does not count.
  setState(s => ({ ...s, messages: [{ id: "r1", taskId: "t", workerId: "w", text: "Stop editing", mode: "send_now", delivery: "recorded", at: 1, deliveredAt: null, failure: null }],
    sprints: [sprint([{ kind: "message", text: "Shorter" }, { kind: "message", text: "Shorter" }])] }));
  expect(sends.shown(worker)).toEqual([]);
  // One new line answers the first send only.
  expect(sends.shown(planner).map(e => e.key)).toEqual(["r3"]);
  setState(s => ({ ...s, sprints: [sprint([{ kind: "message", text: "Shorter" }, { kind: "message", text: "Shorter" }, { kind: "message", text: "Shorter" }])] }));
  expect(sends.shown(planner)).toEqual([]);
  // A refused send is kept until it is sent again or taken back, with its files.
  sends.begin("r4", { type: "plan_message", sprintId: "sp", text: "Add a map", attachments: [{ name: "map.png" } as never] });
  sends.settle("r4", { ...ok, ok: false, message: "Refused" });
  expect(sends.shown(planner).map(e => [e.state, e.reason])).toEqual([["failed", "Refused"]]);
  sends.edit("r4");
  expect(sends.shown(planner)).toEqual([]);
  expect(sends.returned(planner)).toMatchObject({ text: "Add a map", attachments: [{ name: "map.png" }] });
  expect(receipts).toEqual([]);
  done();
}));
