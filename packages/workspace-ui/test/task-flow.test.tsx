import { render } from "solid-js/web";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type PlanItem, type Project, type Sprint, type Task, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { conversationOf, type FeedRow } from "../src/port/history";
import { createRealPort } from "../src/port/real";
import { hrefOf, parseRoute } from "../src/router";
import { capabilitiesWire, draftTask, flush, projection, scriptedPort, scriptedTransport, snapshot, wireDecision, wireTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); Reflect.deleteProperty(Element.prototype, "animate"); Reflect.deleteProperty(window, "matchMedia"); });

const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim().startsWith(text) || b.getAttribute("aria-label") === text)!;
/** Chooses an option of a question of the form; choosing alone sends nothing. */
const choose = (label: string) => all(".dc-panel .opt").find(o => o.textContent?.includes(label))!.click();
/** Presses the form's one Send answers button. */
const sendAll = async () => { button("Send answers").click(); await settled(); };
const write = (field: HTMLTextAreaElement | HTMLInputElement, value: string) => { field.value = value; field.dispatchEvent(new Event("input", { bubbles: true })); };
async function mount(port: Parameters<typeof Workspace>[0]["port"], path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port} />, root);
  await settled();
}

const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
const task = (id: string, patch: Partial<Task> = {}): Task => ({ ...draftTask(), id, title: `Task ${id}`, ...patch });
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Coffee landing", title: "Coffee landing", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: [], queueState: "idle", pauseReason: null, ...patch });
function world(tasks: Task[], sprints: Sprint[] = []): WorkspaceState {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(project);
  state.tasks.push(...tasks);
  state.sprints.push(...sprints);
  return state;
}
const everything = () => scriptedPort({ capabilities: { everything: true, operations: [] } });
const BOARD = `/m/${LOCAL_MACHINE_ID}/p/p`;

describe("machine Board", () => {
  const href = `/m/${LOCAL_MACHINE_ID}/board`;
  const card = (id: string) => $(`.board-wrap .card[data-task="${id}"]`);
  const column = (id: string) => card(id)?.closest(".col")?.getAttribute("data-col");

  it("round-trips a machine Board without changing the project Board route", () => {
    const route = { name: "board" as const, machine: "Desk / remote" };
    expect(hrefOf(route)).toBe("/m/Desk%20%2F%20remote/board");
    expect(parseRoute(hrefOf(route))).toEqual(route);
    const scopedList = { ...route, project: "Cedar & Birch", view: "list" as const };
    const url = new URL(hrefOf(scopedList), "https://example.test");
    expect(parseRoute(url.pathname, url.search)).toEqual(scopedList);
    expect(parseRoute(`${BOARD}/board`)).toMatchObject({ name: "project", view: "board" });
    expect(parseRoute(`${href}/extra`).name).toBe("notfound");
  });

  it("opens from Home, groups both projects by live status, and opens the selected task", async () => {
    const s = everything();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}`);
    const snapshot = (running: boolean) => {
      const state = world([
        task("a", running ? { status: "working", process: "live" } : {}),
        task("b", { projectId: "q", status: "working", process: "live" }),
        task("done", { projectId: "q", status: "done" }),
        task("closed", { status: "closed_unverified", closedAs: "unconfirmed" }),
        task("archived", { status: "done", archive: { at: 1, previousStatus: "done" } }),
        task("elsewhere", { projectId: "r" }),
      ]);
      state.machines.push({ ...state.machines[0], id: "other", name: "Other machine", kind: "remote" });
      state.projects.push({ ...project, id: "q", name: "Birch" }, { ...project, id: "r", name: "Remote", machineId: "other" });
      return state;
    };
    s.emit(projection("1", snapshot(false)));
    await settled();
    const home = all(".sidebar .nav").find(a => a.textContent?.trim() === "Home")!;
    const nav = home.nextElementSibling as HTMLAnchorElement;
    expect(nav.textContent?.trim()).toBe("Board");
    expect(nav.getAttribute("href")).toBe(href);
    expect(nav.getAttribute("aria-current")).toBeNull();
    nav.click();
    expect(location.pathname).toBe(href);
    expect(nav.getAttribute("aria-current")).toBe("page");
    expect($(".titlebar")?.textContent).toContain("Board");
    expect(column("a")).toBe("todo");
    expect(column("b")).toBe("work");
    expect(column("done")).toBe("done");
    expect(column("closed")).toBe("closed_unverified");
    expect(card("archived")).toBeNull();
    expect(card("elsewhere")).toBeNull();
    expect(card("a")?.querySelector(".card-project")?.textContent).toBe("Cedar");
    expect(card("b")?.querySelector(".card-project")?.textContent).toBe("Birch");
    expect(card("b")?.querySelector(".card-s")?.textContent).toContain("Working");

    const unchanged = card("b");
    s.emit(projection("2", snapshot(true)));
    await settled();
    expect(column("a")).toBe("work");
    expect(card("b")).toBe(unchanged);
    card("a")!.querySelector<HTMLAnchorElement>(".card-link")!.click();
    expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/a`);
    expect(nav.getAttribute("aria-current")).toBeNull();
  });

  it("distinguishes an empty machine from one with no board tasks", async () => {
    const s = everything();
    await mount(s.port, href);
    const empty = emptyWorkspace();
    empty.machines[0].connection = "connected";
    s.emit(projection("1", empty));
    await settled();
    expect($(".note-panel")?.textContent).toContain("No projects on this machine yet");
    s.emit(projection("2", world([])));
    await settled();
    expect($(".note-panel")?.textContent).toContain("No tasks to show on this machine");
  });

  it("shares the work toolbar and sprint groups without adding project actions to the machine Board", async () => {
    const s = everything();
    await mount(s.port, href);
    const member = task("member", { title: "Sprint member", sprintId: "sp" });
    const other = task("other", { title: "Other project task", projectId: "q" });
    const state = world([member, other], [sprint({ taskIds: [member.id] })]);
    state.projects.push({ ...project, id: "q", name: "Birch" });
    s.emit(projection("1", state));
    await settled();
    expect($('[aria-label="Board options"]')?.textContent).toContain("All projects");
    expect($(".col-grp")?.textContent).toContain("Coffee landing");
    expect(card(other.id)?.querySelector(".card-project")?.textContent).toBe("Birch");
    expect($(".quick")).toBeNull();
    expect($('[aria-label="More project areas"]')).toBeNull();
    const toggle = $<HTMLButtonElement>('.pstrip button[aria-label="Filter"]')!;
    toggle.click();
    const filter = $<HTMLInputElement>('.pstrip input[aria-label="Filter tasks"]')!;
    write(filter, "other");
    expect(card(member.id)).toBeNull();
    expect(card(other.id)).not.toBeNull();
    write(filter, "missing");
    expect($(".note-panel")?.textContent).toContain("No tasks match this filter");
    filter.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.activeElement).toBe(toggle);
    expect(card(member.id)).not.toBeNull();
    expect(location.pathname).toBe(href);
    $<HTMLAnchorElement>('.pstrip a[role="tab"][href$="view=list"]')!.click();
    expect(location.search).toBe("?view=list");
    expect($(".ws-list .board-list-title")?.textContent).toContain("Birch");
    all<HTMLButtonElement>('[role="menuitem"]').find(el => el.textContent?.includes("Cedar"))!.click();
    expect(location.search).toBe("?project=p&view=list");
    expect($(".ws-list")?.textContent).toContain(member.title);
    expect($(".ws-list")?.textContent).not.toContain(other.title);
    $<HTMLButtonElement>('.titlebar button[aria-label="Back"]')!.click();
    expect(location.search).toBe("?view=list");
    expect($(".ws-list")?.textContent).toContain(other.title);
  });
});

/** A planning run on the real port, with the machine answering as the service does. */
function planning(options: { decisions: ReturnType<typeof wireDecision>[]; plan?: PlanItem[] | null; conversation?: Sprint["conversation"] }) {
  const config = structuredClone(emptyWorkspace().machines[0].config);
  const held = { sprint: sprint({ planning: options.plan ? "ready" : "planning", planningTaskId: "task-1", materializedVersion: null, plan: options.plan ? { version: 1, items: options.plan } : null, conversation: options.conversation ?? [] }) };
  const tick = { at: 0 };
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call", "executions.control"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"]),
    "project.snapshot": () => snapshot("4", [wireTask({ executionId: "e", phase: "planning", runtimeState: "PLANNING", executionState: "running" })]),
    "decisions.list": () => ({ items: options.decisions, nextCursor: null, highWatermark: "4" }),
    "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
    "decisions.answer": message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "b", entityVersion: "3", recordedAt: "2026-11-05T00:00:00Z" }),
    "machine.call": message => {
      const { op } = message as { op: string };
      if (op === "repo.snapshot") return { ok: true, result: null };
      // Every read builds new objects, and the machine's own stamp moves, as on the real service.
      if (op === "product.read") return { ok: true, result: structuredClone({ tasks: { "task-1": { planning: true } }, sprints: [held.sprint], messages: [] }) };
      if (op === "workers") return { ok: true, result: [] };
      if (op === "overview") return { ok: true, result: { name: `Desk`, retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: ++tick.at, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      return { ok: true, result: {} };
    },
  });
  return { ...s, port: createRealPort({ transport: s.transport, actorId: "owner", registry: true }) };
}
const SPRINT = `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`;
const questions = [
  wireDecision({ decisionId: "d1", questionId: "q1", batchId: "b", executionId: "e", prompt: "Who is it for?", options: [{ optionId: "o1", label: "Home buyers" }, { optionId: "o2", label: "Offices" }] }),
  wireDecision({ decisionId: "d2", questionId: "q2", batchId: "b", executionId: "e", prompt: "What should a visitor do?", options: [{ optionId: "o3", label: "Only look" }] }),
  wireDecision({ decisionId: "d3", questionId: "q3", batchId: "b", executionId: "e", prompt: "Which style?", options: [{ optionId: "o4", label: "Warm" }] }),
];

describe("question language from the runtime", () => {
  it.each([
    { language: "ru", browser: "en-US", heading: "Вопрос", from: "Спрашивает координатор", send: "Отправить ответ",
      choose: "Выберите или напишите ответ", recommendation: "Рекомендую", label: "Ваш ответ" },
    { language: "en", browser: "ru-RU", heading: "Question", from: "Asked by coordinator", send: "Send answer",
      choose: "Choose or write an answer first", recommendation: "Recommended", label: "Your answer" },
  ])("shows one $language question with a $browser browser", async data => {
    vi.spyOn(navigator, "language", "get").mockReturnValue(data.browser);
    const d = { ...questions[0], language: data.language, askedBy: "coordinator",
      options: questions[0].options.map((o, i) => i === 0 ? { ...o, recommended: true } : o) };
    const s = planning({ decisions: [d] });
    s.answers["project.snapshot"] = () => snapshot("4", [wireTask({ executionId: "e", phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" })]);
    const machine = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => (message as { op: string }).op === "product.read" ? { ok: true, result: { tasks: {}, sprints: [], messages: [] } } : machine(message);
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/task-1`);
    expect($(".dc")?.getAttribute("lang")).toBe(data.language);
    expect($(".dc-h")?.firstChild?.nextSibling?.textContent).toBe(data.heading);
    expect($(".ws-dc-from")?.textContent).toBe(data.from);
    expect($(".ws-dc-rec")?.textContent).toBe(data.recommendation);
    expect($(".dc textarea")?.getAttribute("aria-label")).toBe(data.label);
    expect(button(data.send).dataset.why).toBe(data.choose);
    choose("Home buyers");
    await settled();
    expect(button(data.send).getAttribute("aria-disabled")).toBeNull();
    expect(s.calls.filter(c => c.operation === "decisions.answer")).toEqual([]);
  });

  it.each([
    { language: "ru", browser: "en-US", askedBy: "runtime", from: "Вопрос задан автоматически" },
    { language: "en", browser: "ru-RU", askedBy: "runtime", from: "Asked automatically" },
    { language: "ru", browser: "en-US", askedBy: "devops-engineer", from: "Спрашивает devops-engineer" },
  ])("labels $askedBy in a $language task with a $browser browser", async data => {
    vi.spyOn(navigator, "language", "get").mockReturnValue(data.browser);
    const d = { ...questions[0], batchId: null, language: data.language, askedBy: data.askedBy };
    const s = planning({ decisions: [d] });
    s.answers["project.snapshot"] = () => snapshot("4", [wireTask({ executionId: "e", phase: "execution", runtimeState: "EXECUTING", executionState: "running" })]);
    const machine = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => (message as { op: string }).op === "product.read" ? { ok: true, result: { tasks: {}, sprints: [], messages: [] } } : machine(message);
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/task-1`);
    expect($(".ws-dc-from")?.textContent).toBe(data.from);
  });

  it.each([
    { language: "ru", browser: "en-US", heading: "Вопросы (2)", positions: ["Вопрос 1 из 2", "Вопрос 2 из 2"],
      send: "Отправить ответы", progress: "Дано ответов: 1 из 2", incomplete: "Сначала ответьте на все вопросы: 1 из 2" },
    { language: "en", browser: "ru-RU", heading: "2 questions", positions: ["Question 1 of 2", "Question 2 of 2"],
      send: "Send answers", progress: "1 of 2 answered", incomplete: "Answer every question first: 1 of 2 answered" },
  ])("shows a $language batch with a $browser browser", async data => {
    vi.spyOn(navigator, "language", "get").mockReturnValue(data.browser);
    const s = planning({ decisions: questions.slice(0, 2).map(d => ({ ...d, language: data.language })) });
    await mount(s.port, SPRINT);
    expect($(".dc")?.getAttribute("aria-label")).toBe(data.heading);
    expect(all(".dc-panel").map(p => p.getAttribute("aria-label"))).toEqual(data.positions);
    expect(all(".dc-panel > .dc-n").map(p => p.textContent)).toEqual(data.positions);
    choose("Home buyers");
    await settled();
    expect($(".dc-send")?.textContent).toContain(data.progress);
    expect(button(data.send).dataset.why).toBe(data.incomplete);
    button(data.send).click();
    expect(s.calls.filter(c => c.operation === "decisions.answer")).toEqual([]);
    choose("Only look");
    await settled();
    expect(button(data.send).getAttribute("aria-disabled")).toBeNull();
    button(data.send).click();
    await settled();
    expect(s.calls.filter(c => c.operation === "decisions.answer")).toHaveLength(1);
  });
});

describe("several questions at once", () => {
  const answered = (s: ReturnType<typeof planning>) => s.calls.filter(c => c.operation === "decisions.answer").map(c => c.message as { target: string; payload: { batchId: string; answers: { decisionId: string; text: string; optionId: string | null }[] } });

  it("shows every question of a planning run in one form, with the count, each with its own answer", async () => {
    const s = planning({ decisions: questions });
    await mount(s.port, SPRINT);
    expect(all(".dc")).toHaveLength(1);
    expect(all(".dc-tab")).toEqual([]);
    expect($(".dc-h")?.textContent).toBe("3 questions" + $(".dc-h .r")!.textContent);
    expect(all(".dc-panel .dc-q").map(q => q.textContent)).toEqual(["Who is it for?", "What should a visitor do?", "Which style?"]);
    expect(all(".dc-panel").map(p => p.getAttribute("aria-label"))).toEqual(["Question 1 of 3", "Question 2 of 3", "Question 3 of 3"]);
    expect(all(".dc textarea").map(t => t.getAttribute("aria-label"))).toEqual(["Your answer to question 1", "Your answer to question 2", "Your answer to question 3"]);
    // One button sends them all: no question has a paper plane of its own.
    expect(all('.dc [aria-label="Send answer"]')).toEqual([]);
    expect(all(".dc button").filter(b => b.textContent?.trim() === "Send answers")).toHaveLength(1);
  });

  it("offers no model or effort in the answer form: the answer goes into the run's own session", async () => {
    const s = planning({ decisions: questions });
    await mount(s.port, SPRINT);
    expect($(".dc .c-route")).toBeNull();
    expect(all(".dc [aria-label]").filter(el => /^(Coordinator model|Reasoning effort)/.test(el.getAttribute("aria-label")!))).toEqual([]);
  });

  it("choosing an option only prepares the answer; writing adds the owner's words to it", async () => {
    const s = planning({ decisions: questions });
    await mount(s.port, SPRINT);
    choose("Home buyers");
    await settled();
    expect(all(".opt").find(o => o.textContent?.includes("Home buyers"))?.getAttribute("aria-checked")).toBe("true");
    expect(all(".dc-panel .mc-effect")[0]?.textContent).toBe("Answer: Home buyers");
    // The owner may choose an option and add their own words: both stay, as one answer.
    write($<HTMLTextAreaElement>('[aria-label="Your answer to question 1"]')!, "Both, really"); await settled();
    expect(all(".opt").find(o => o.textContent?.includes("Home buyers"))?.getAttribute("aria-checked")).toBe("true");
    expect($<HTMLTextAreaElement>('[aria-label="Your answer to question 1"]')!.value).toBe("Both, really");
    expect($(".dc-send")?.textContent).toContain("1 of 3 answered");
    expect(answered(s)).toEqual([]);
  });

  it("waits for an answer to every question, then sends the whole batch once with the one button", async () => {
    const s = planning({ decisions: questions });
    await mount(s.port, SPRINT);
    choose("Only look"); choose("Home buyers"); await settled();
    expect(button("Send answers").getAttribute("aria-disabled")).toBe("true");
    expect(button("Send answers").dataset.why).toBe("Answer every question first: 2 of 3 answered");
    await sendAll();
    expect(answered(s)).toEqual([]);
    choose("Warm"); await settled();
    expect(button("Send answers").getAttribute("aria-disabled")).toBeNull();
    await sendAll();
    const sent = answered(s);
    expect(sent).toHaveLength(1);
    expect(sent[0].target).toBe("b");
    expect(sent[0].payload.answers.map(a => [a.decisionId, a.text, a.optionId])).toEqual([["d1", "Home buyers", "o1"], ["d2", "Only look", "o3"], ["d3", "Warm", "o4"]]);
    // Sent, the questions read as answered and never come back as open ones, though the machine's record has not caught up.
    expect(all(".dc .qa-a").map(a => a.textContent)).toEqual(["Home buyers", "Only look", "Warm"]);
    expect(button("Send answers")).toBeUndefined();
    expect(all(".dc .opt")).toEqual([]);
    expect($('.dc [role="status"]')?.textContent).toBe("Answers sent; work continues with them.");
    s.send({ type: "invalidated", projectId: "p" }); await settled();
    expect(all(".dc .opt")).toEqual([]);
    expect(answered(s)).toHaveLength(1);
  });

  it("keeps every answer and says why when the machine refuses the batch, and sends again on request", async () => {
    const s = planning({ decisions: questions });
    let refuse = true;
    s.answers["decisions.answer"] = message => ({ requestId: (message as { requestId: string }).requestId, outcome: refuse ? "busy" : "recorded", target: "b", entityVersion: "3", recordedAt: "2026-11-05T00:00:00Z" });
    await mount(s.port, SPRINT);
    choose("Offices"); choose("Only look");
    write($<HTMLTextAreaElement>('[aria-label="Your answer to question 3"]')!, "Warm,\nbut calm"); await settled();
    await sendAll();
    expect(answered(s)).toHaveLength(1);
    expect($('.dc [role="alert"]')?.textContent).toMatch(/Your answers are kept/);
    expect(all(".opt").filter(o => o.getAttribute("aria-checked") === "true").map(o => o.querySelector(".t")?.textContent)).toEqual(["Offices", "Only look"]);
    expect($<HTMLTextAreaElement>('[aria-label="Your answer to question 3"]')!.value).toBe("Warm,\nbut calm");
    expect(all(".dc .qa")).toEqual([]);
    refuse = false;
    await sendAll();
    const sent = answered(s);
    expect(sent).toHaveLength(2);
    expect(sent[1].payload.answers.map(a => a.text)).toEqual(["Offices", "Only look", "Warm,\nbut calm"]);
    expect($('.dc [role="alert"]')).toBeNull();
    expect(all(".dc .qa")).toHaveLength(3);
  });

  it("uses one form for a task's open questions too, and a single question keeps its own Send answer", async () => {
    const tasks = [wireTask({ executionId: "e", phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" })];
    const s = planning({ decisions: questions.slice(0, 2) });
    s.answers["product.read"] = undefined;
    s.answers["project.snapshot"] = () => snapshot("4", tasks);
    const machine = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => (message as { op: string }).op === "product.read" ? { ok: true, result: { tasks: {}, sprints: [], messages: [] } } : machine(message);
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/task-1`);
    expect(all(".timeline .dc")).toHaveLength(1);
    expect(all(".timeline .dc .dc-panel")).toHaveLength(2);
    expect($(".timeline .dc .c-route")).toBeNull();
    choose("Home buyers"); choose("Only look"); await settled();
    await sendAll();
    expect(s.calls.filter(c => c.operation === "decisions.answer").map(c => (c.message as { payload: { answers: { text: string }[] } }).payload.answers.map(a => a.text))).toEqual([["Home buyers", "Only look"]]);
    dispose?.(); document.body.innerHTML = "";
    s.answers["decisions.list"] = () => ({ items: questions.slice(0, 1), nextCursor: null, highWatermark: "4" });
    await mount(createRealPort({ transport: s.transport, actorId: "owner", registry: true }), `/m/${LOCAL_MACHINE_ID}/t/task-1`);
    expect($(".dc-h")?.textContent).toContain("Question");
    expect(button("Send answers")).toBeUndefined();
    expect($('.dc [aria-label="Send answer"]')).not.toBeNull();
    expect($(".timeline .dc .c-route")).toBeNull();
    // Once recorded, the plane leaves and the question reads as answered.
    choose("Home buyers"); await settled();
    $<HTMLElement>('.dc [aria-label="Send answer"]')!.click(); await settled();
    expect($('.dc [aria-label="Send answer"]')).toBeNull();
    expect(all(".dc .qa-a").map(a => a.textContent)).toEqual(["Home buyers"]);
    expect($('.dc [role="status"]')?.textContent).toBe("Answer sent; work continues with it.");
  });

  it("names how many questions wait in the Inbox", async () => {
    const s = planning({ decisions: questions });
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/inbox`);
    expect($(".need .ask")?.textContent).toBe("3 questions: Who is it for?");
  });
});

describe("settled questions", () => {
  it("reads as the question and the answer, each in its own place", async () => {
    const answered = wireDecision({ decisionId: "d1", questionId: "q1", executionId: "e", prompt: "Who is it for?", status: "answered", allowedOperations: [], answerText: "Home buyers", answeredAt: "2026-11-05T23:20:00Z" });
    const s = planning({ decisions: [answered], conversation: [{ kind: "answer", text: "Home buyers" }] });
    await mount(s.port, SPRINT);
    expect(all(".qa")).toHaveLength(1);
    expect($(".qa-q")?.textContent).toBe("Who is it for?");
    expect($(".qa-a")?.textContent).toBe("Home buyers");
    // On this computer's clock, whatever its time zone.
    const at = new Date("2026-11-05T23:20:00Z");
    expect($(".qa-t")?.textContent).toBe(`${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`);
    expect($(".sp-conv")?.textContent).not.toContain("You answered:");
  });

  it("pairs a recorded question with the answer that followed it", async () => {
    const port = everything();
    await mount(port.port, SPRINT);
    port.emit(projection("1", world([], [sprint({ planning: "ready", materializedVersion: null, conversation: [{ kind: "note", text: "Reading the project" }, { kind: "question", text: "Who?" }, { kind: "answer", text: "Owners", at: 1000 }] })])));
    await settled();
    expect(all(".qa-q").map(q => q.textContent)).toEqual(["Who?"]);
    expect(all(".sp-conv .ai").map(q => q.textContent)).toEqual(["Reading the project"]);
  });
});

describe("the plan under a resting pointer", () => {
  const items: PlanItem[] = [{ key: "t1", title: "Write the brief", description: "A very long description.\n\nWith several paragraphs of detail.", dependsOn: [] }, { key: "t2", title: "Build the page", description: "Implement it.", dependsOn: ["t1"] }];

  it("rebuilds nothing when the machine is read again and the pointer stays on a row", async () => {
    const s = planning({ decisions: [], plan: items });
    await mount(s.port, SPRINT);
    const rows = all(".pt-row:not(.th)"), nodes = all(".ws-node"), plan = $(".sp-plan")!;
    expect(rows).toHaveLength(2);
    const changes: MutationRecord[] = [];
    const observer = new MutationObserver(list => changes.push(...list));
    observer.observe(plan, { subtree: true, childList: true, attributes: true, characterData: true });
    for (let i = 0; i < 4; i++) {
      for (const type of ["mouseover", "mousemove"]) rows[1].dispatchEvent(new MouseEvent(type, { bubbles: true }));
      s.send({ type: "invalidated", projectId: "p" });
      await settled();
    }
    await flush();
    observer.disconnect();
    expect(changes).toEqual([]);
    expect(all(".pt-row:not(.th)")).toEqual(rows);
    expect(all(".ws-node")).toEqual(nodes);
  });

  it("keeps a half-written other answer through the same reads", async () => {
    const s = planning({ decisions: questions });
    await mount(s.port, SPRINT);
    const field = $<HTMLTextAreaElement>('[aria-label="Your answer to question 2"]')!;
    field.value = "Half written"; field.dispatchEvent(new Event("input", { bubbles: true }));
    s.send({ type: "invalidated", projectId: "p" }); await settled();
    expect($('[aria-label="Your answer to question 2"]')).toBe(field);
    expect(field.value).toBe("Half written");
  });

  it("lists titles only, and Read more opens the full text beside the plan", async () => {
    const s = planning({ decisions: [], plan: items });
    // The plan has its own tab beside the chat.
    await mount(s.port, SPRINT.replace(/planning$/, "plan"));
    expect($(".sp-plan")?.textContent).not.toContain("several paragraphs");
    expect($(".sp-plan")?.textContent).not.toContain("Acceptance and checks");
    expect($(".ws-read")).toBeNull();
    const more = all(".ws-more-l");
    expect(more.map(b => b.textContent)).toEqual(["Read more", "Read more"]);
    more[0].click(); await settled();
    expect($(".ws-read-t")?.textContent).toBe("Write the brief");
    expect(all(".ws-read-d p").map(p => p.textContent)).toEqual(["A very long description.", "With several paragraphs of detail."]);
    expect(all(".pt-row:not(.th)")[0].dataset.selected).toBe("true");
    more[1].click(); await settled();
    expect($(".ws-read-t")?.textContent).toBe("Build the page");
    expect($(".ws-read")?.textContent).toContain("Tasks 1");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); await settled();
    expect($(".ws-read")).toBeNull();
  });
});

describe("the runtime's feed as a conversation", () => {
  const row = (n: number, kind: string, text: string | null, extra: Partial<FeedRow> = {}): FeedRow => ({ historyId: `h${n}`, revision: "1", kind, text, recordedAt: `2026-11-05T10:00:${String(n).padStart(2, "0")}Z`, ...extra });
  const known = { description: "Create the docs", answers: [] as string[] };
  const entries = (rows: FeedRow[], k = known) => conversationOf(rows, k).map(d => d.entry);

  it("joins streamed chunks back into the message they were", () => {
    const out = entries([row(1, "assistant", "Заново проверяю "), row(2, "system", "Display update: usage_update"), row(3, "assistant", "готовые "), row(4, "assistant", "документы.")]);
    expect(out).toEqual([{ id: "history:h1:1", kind: "cortex", text: "Заново проверяю готовые документы." }]);
  });

  it("never shows a display update or a machine identifier", () => {
    const text = JSON.stringify(entries([
      row(1, "tool", "mcp__cortex__project_read · pending", { toolStatus: "pending" }),
      row(2, "tool", "call:coordinator:es_709b674cb0a5:tc_9c6e3c186cc6", {}),
      row(3, "tool", "call:coordinator:es_709b674cb0a5:tc_9c6e3c186cc6 · completed", { toolStatus: "completed" }),
      row(4, "system", "Display update: agent_message_chunk"),
      row(5, "tool", "verification:1:212092622506616:0 · completed", { toolStatus: "completed" }),
    ]));
    expect(text).not.toMatch(/call:|verification:|es_|tc_|Display update|mcp__|· pending|· completed/);
  });

  it("folds one turn's tools into one line, counting calls and not their events", () => {
    const out = entries([
      row(1, "assistant", "Looking."),
      row(2, "tool", "mcp__cortex__project_read · pending", { toolStatus: "pending" }),
      row(3, "tool", "call:coordinator:es_1:tc_1 · completed", { toolStatus: "completed" }),
      row(4, "tool", "mcp__cortex__project_read · pending", { toolStatus: "pending" }),
      row(5, "tool", "call:coordinator:es_1:tc_2 · failed", { toolStatus: "failed" }),
      row(9, "tool", "Worker: technical-writer — Write the docs · pending", { toolStatus: "pending" }),
      row(10, "assistant", "\n\nDone."),
    ]);
    expect(out).toEqual([
      { id: "history:h1:1", kind: "cortex", text: "Looking." },
      // A worker launch reads in the Workers card, so the fold does not repeat it.
      { id: "history:h2:1", kind: "fold", text: "Worked 3s, read 2 files, 1 tool call needs attention", steps: ["Read ×2"] },
      { id: "history:h10:1", kind: "cortex", text: "Done." },
    ]);
  });

  it("keeps what the owner said, and drops what already reads elsewhere", () => {
    const out = entries([row(1, "user", "Create the docs"), row(2, "user", "Home buyers"), row(3, "user", "Keep it short")], { ...known, answers: ["Home buyers"] });
    expect(out).toEqual([{ id: "history:h3:1", kind: "said", text: "Keep it short" }]);
  });

  it("says when a part could not be read, and invents nothing", () => {
    expect(entries([row(1, "assistant", null, { availability: "unavailable" })])).toEqual([{ id: "history:h1:1", kind: "line", icon: "help", text: "Part of this conversation could not be read", at: null, rule: false }]);
  });
});

describe("a sprint whose tasks the project's runtime has not loaded", () => {
  // The plan lists them while the runtime starts, stops or refuses the project. Every screen reads the one index, so a strict
  // lookup there froze every link of the window: the address changed and nothing on screen did.
  const partial = () => world([task("a")], [sprint({ runMode: "automatic", taskIds: ["not-loaded", "a"] })]);
  const failures = () => {
    const seen: unknown[] = [];
    const onError = (e: ErrorEvent) => { seen.push(e.error ?? e.message); e.preventDefault(); };
    window.addEventListener("error", onError);
    return { seen, stop: () => window.removeEventListener("error", onError) };
  };
  it("keeps every link of the window working", async () => {
    const s = everything(), errors = failures();
    try {
      await mount(s.port, `/m/${LOCAL_MACHINE_ID}`);
      s.emit(projection("1", partial()));
      await settled();
      const link = (label: string) => all<HTMLAnchorElement>("nav.sidebar a").find(a => a.textContent?.includes(label))!;
      link("Inbox").click(); await settled();
      expect($("main h1")?.textContent).toBe("Inbox");
      link("Archive").click(); await settled();
      expect($('main input[aria-label="Filter archived tasks"]')).toBeTruthy();
      link("Cedar").click(); await settled();
      expect($('.card[data-task="a"]')).toBeTruthy();
      expect(errors.seen).toEqual([]);
    } finally { errors.stop(); }
  });
  it("holds the queue until its tasks are loaded", async () => {
    const s = everything(), errors = failures();
    try {
      await mount(s.port, `${BOARD}/s/sp/board`);
      s.emit(projection("1", partial()));
      await settled();
      expect(button("Run sprint").dataset.why).toBe("This sprint's tasks are not loaded yet");
      expect(button("Run sprint").getAttribute("aria-disabled")).toBe("true");
      expect(errors.seen).toEqual([]);
    } finally { errors.stop(); }
  });
});

describe("what needs the owner", () => {
  it("keeps accepted work awaiting merge visibly actionable", async () => {
    const s = everything();
    await mount(s.port, BOARD);
    const result = { version: 1, digest: "d", checks: [], summary: "Ready" };
    s.emit(projection("1", world([task("merge", { status: "ready_to_merge", result, workspaceDigest: "d", acceptance: { version: 1, digest: "d", at: 1000, open: { failed: 0, not_run: 0, unverified: 0 } } })])));
    await settled();
    expect($('.card[data-task="merge"]')?.classList.contains("need-you")).toBe(true);
  });
  const review = () => task("r", { status: "review", result: { version: 1, digest: "d", checks: [], summary: "" }, workspaceDigest: "d" });
  it("stands out on the board, the list and the sidebar", async () => {
    const s = everything();
    await mount(s.port, BOARD);
    s.emit(projection("1", world([review(), task("n", { status: "needs_you", issue: "Which?" }), task("w", { status: "working", process: "live" }), task("d")])));
    await settled();
    const card = (id: string) => $(`.card[data-task="${id}"]`)!;
    expect(card("r").classList.contains("need-you")).toBe(true);
    expect(card("n").classList.contains("need-you")).toBe(true);
    expect(card("w").classList.contains("need-you")).toBe(false);
    expect(card("d").classList.contains("need-you")).toBe(false);
    expect(all(".sidebar .tasks .task-row .sig.st-warn").map(x => x.textContent).sort()).toEqual(["Needs you", "Review"]);
    $(".sb-view-head .seg button:last-child")!.click(); await settled();
    expect(all(".sb-work .task-row .sig.st-warn").map(x => x.textContent)).toEqual(["Review"]);
    all('[role="tab"]').find(t => t.textContent === "List")!.click(); await settled();
    expect(all(".lrow.need-you").length).toBe(2);
    expect(all(".lrow").length).toBe(4);
  });
});

describe("sprints on All work", () => {
  const tasks = () => [task("a", { sprintId: "sp" }), task("b", { sprintId: "sp" }), task("c"), task("w", { sprintId: "sp", status: "working", process: "live" })];
  const sprints = () => [sprint({ taskIds: ["a", "b", "w"] })];

  it("groups a column's sprint tasks under the sprint's name, and names it on each card", async () => {
    const s = everything();
    await mount(s.port, BOARD);
    s.emit(projection("1", world(tasks(), sprints())));
    await settled();
    const todo = $('.col[data-col="todo"]')!;
    const order = [...todo.querySelectorAll<HTMLElement>(".card, .col-grp")].map(el => el.dataset.task ?? `head:${el.textContent}`);
    expect(order).toEqual(["c", "head:Coffee landing2", "a", "b"]);
    expect($('.col[data-col="work"] .col-grp')?.textContent).toContain("Coffee landing");
    expect($('.card[data-task="a"] .card-f')?.textContent).toContain("Coffee landing");
    expect($('.card[data-task="c"] .card-f')?.textContent).not.toContain("Coffee landing");
    expect($<HTMLAnchorElement>(".col-grp")?.getAttribute("href")).toContain("sp");
  });

  it("needs no heading on the sprint's own board, or with no sprint in view", async () => {
    const s = everything();
    await mount(s.port, `${BOARD}?scope=sp`);
    s.emit(projection("1", world([task("c")])));
    await settled();
    expect($(".col-grp")).toBeNull();
  });

  it("groups the list the same way", async () => {
    const s = everything();
    await mount(s.port, `${BOARD}/list`);
    s.emit(projection("1", world(tasks(), sprints())));
    await settled();
    expect(all(".ws-list .col-grp").length).toBe(2);
  });
});

it("puts a background column change straight into normal layout", async () => {
  const s = everything();
  await mount(s.port, BOARD);
  s.emit(projection("1", world([task("a"), task("b")])));
  await settled();
  const destination = $('.col[data-col="work"]')!;
  expect(destination.classList.contains("strip")).toBe(true);
  const neighbor = $('.card[data-task="b"]');
  const animate = vi.fn(() => ({ finished: Promise.resolve() }));
  Object.defineProperty(Element.prototype, "animate", { value: animate, configurable: true });
  s.emit(projection("2", world([task("a", { status: "working", process: "live" }), task("b")])));
  const card = $<HTMLElement>('.card[data-task="a"]')!;
  expect(card.closest(".col")).toBe(destination);
  expect(destination.classList.contains("strip")).toBe(false);
  expect($('.card[data-task="b"]')).toBe(neighbor);
  expect(card.style.visibility).toBe("");
  expect($(".ws-fly, [data-flying]")).toBeNull();
  expect(animate).not.toHaveBeenCalled();
});

it.each([
  { view: "project Board", path: BOARD },
  { view: "machine Board", path: `/m/${LOCAL_MACHINE_ID}/board` },
])("shows a saved preview on the $view without a line while live reads are slow, and says so only once they fail", async ({ path }) => {
  const s = everything();
  await mount(s.port, path);
  s.emit({ kind: "preview", slice: { project, tasks: [task("saved")], sessions: [] },
    facts: { projectId: project.id, openBranch: "main", defaultBranch: "main", repo: null, workers: [], reviews: [], overlays: {}, sprints: [] } });
  expect($('.card[data-task="saved"]')).not.toBeNull();
  // A live read under way, however slow, is no issue: the confirmed saved view stands with no line and no error.
  expect(document.body.textContent).not.toContain("Saved view;");
  expect($('[role="alert"]')).toBeNull();
  s.emit({ kind: "connection", connection: { state: "disconnected", reason: "Connection lost", at: Date.now() } });
  expect(document.body.textContent).toContain("Saved view; live state unavailable");
  expect($('.card[data-task="saved"]')).not.toBeNull();
  s.emit({ kind: "connection", connection: { state: "connected", reason: null, at: Date.now() } });
  s.emit(projection("1", world([task("live")])));
  await settled();
  expect($('.card[data-task="live"]')).not.toBeNull();
  expect(document.body.textContent).not.toContain("Saved view;");
});

it("labels creation in its composer until the real reply arrives", async () => {
  const s = everything();
  await mount(s.port, BOARD);
  s.emit(projection("1", world([task("a")])));
  await settled();
  $(".quick")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await settled();
  const field = $<HTMLTextAreaElement>('textarea[aria-label="Describe the task"]')!;
  write(field, "Make a new task");
  button("Save draft").click();
  expect(s.commands.at(-1)?.operation).toBe("create_task");
  expect($(".create .ws-request-status")?.textContent).toBe("Requesting task creation…");
  s.receipt(false, undefined, "refused", "Task creation refused.");
  await settled();
  expect($(".create .ws-request-status")?.textContent).toBe("");
  expect(field.value).toBe("Make a new task");
  expect(document.body.textContent).toContain("Task creation refused.");
  expect(s.commands).toHaveLength(1);
});

describe("the one loading system", () => {
  const draft = () => world([task("a"), task("b")]);
  const onBoard = async () => { const s = everything(); await mount(s.port, BOARD); s.emit(projection("1", draft())); await settled(); return s; };

  it("marks the pressed button and takes no second press until its action ends", async () => {
    const s = await onBoard();
    const start = all(".card-a button").find(b => b.textContent === "Start")!;
    start.click(); start.click(); start.click();
    expect(s.commands).toHaveLength(1);
    expect(start.hasAttribute("data-busy")).toBe(true);
    expect(start.getAttribute("aria-busy")).toBe("true");
    const status = $('.card[data-task="a"] .ws-request-status')!;
    expect(status.textContent).toBe("Requesting start…");
    expect($('.card[data-task="b"] .ws-request-status')?.textContent).toBe("");
    expect(start.closest(".col")?.getAttribute("data-col")).toBe("todo");
    s.receipt(); await settled();
    expect($('.card[data-task="a"] .ws-request-status')).toBe(status);
    expect(status.textContent).toBe("");
    expect(start.hasAttribute("data-busy")).toBe(false);
    expect(start.hasAttribute("aria-busy")).toBe(false);
    start.click();
    expect(s.commands).toHaveLength(2);
  });

  it("keeps the loader on a refusal, then lets the owner try again", async () => {
    const s = await onBoard();
    const start = all(".card-a button").find(b => b.textContent === "Start")!;
    start.click();
    expect($('.card[data-task="a"] .ws-request-status')?.textContent).toBe("Requesting start…");
    s.receipt(false); await settled();
    expect($('.card[data-task="a"] .ws-request-status')?.textContent).toBe("");
    expect($('.card[data-task="a"] [role="alert"]')?.textContent).toContain("Refused");
    expect(s.commands).toHaveLength(1);
    expect(start.hasAttribute("data-busy")).toBe(false);
    start.click();
    expect(s.commands).toHaveLength(2);
  });

  it("puts a menu's work on the control that opened the menu", async () => {
    const s = await onBoard();
    const trigger = $<HTMLElement>('[aria-label="Actions for Task a"]')!;
    trigger.click(); await settled();
    all('#m-task [role="menuitem"]').find(m => m.textContent?.includes("Start"))!.click();
    expect(trigger.hasAttribute("data-busy")).toBe(true);
    s.receipt(); await settled();
    expect(trigger.hasAttribute("data-busy")).toBe(false);
  });

  it("covers Enter in a composer, on the send button that stands for it", async () => {
    const s = everything();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/w`);
    s.emit(projection("1", world([task("w", { status: "working", process: "live" })])));
    await settled();
    const field = $<HTMLTextAreaElement>('[aria-label="Message"]')!;
    field.value = "Keep it short"; field.dispatchEvent(new Event("input", { bubbles: true }));
    await settled();
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(s.commands).toHaveLength(1);
    expect($(".c-send[data-busy]")).toBeTruthy();
    s.receipt(); await settled();
    expect($(".c-send[data-busy]")).toBeNull();
  });

  it("does not pin later work on an earlier press", async () => {
    const s = await onBoard();
    const quick = $<HTMLElement>(".quick")!;
    quick.click();
    await flush();
    s.emit(projection("2", draft()));
    expect($("[data-busy]")).toBeNull();
  });

  it("draws in the control's own box, and shows nothing for an action that is over at once", () => {
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src/workspace.css"), "utf8");
    expect(css).toMatch(/\[data-busy\]::after[^{]*\{[^}]*position: absolute[^}]*opacity: 0[^}]*150ms/);
    expect(css).not.toMatch(/\[data-busy\][^{]*\{[^}]*(width: auto|padding|margin: 0 )/);
  });
});

describe("stamps and rows that said nothing", () => {
  it("shows no Confirmed stamp on a live project, card, Inbox row or task", async () => {
    const s = everything();
    await mount(s.port, BOARD);
    s.emit(projection("1", world([task("a", { status: "needs_you", issue: "Which?", lastConfirmedAt: Date.now() })])));
    await settled();
    expect($(".main")?.textContent).not.toMatch(/Confirmed|\bold\b/);
    expect(all(".main > .strip.info")).toEqual([]);
    $<HTMLAnchorElement>('[href$="/inbox"]')!.click(); await settled();
    expect($(".need .age")?.textContent).toMatch(/^waiting /);
    expect($(".main")?.textContent).not.toMatch(/Confirmed|\bold\b/);
  });

  it("still says when what is on screen is out of date", async () => {
    const s = everything();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/a`);
    s.emit(projection("1", world([task("a", { lastConfirmedAt: Date.now() })])));
    await settled();
    s.emit({ kind: "connection", connection: { state: "disconnected", reason: null, at: Date.now() } });
    await settled();
    expect($(".taskbar")?.textContent).toMatch(/Last confirmed .* old/);
  });
});

describe("checks in the Inspector", () => {
  it("names each check that did not pass and says what that means", async () => {
    const s = everything();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/r`);
    const checks = [{ name: "Unit tests", outcome: "passed", detail: "" }, { name: "Visual review", outcome: "unverified", detail: "ev_1", source: "runtime" }, { name: "Link check", outcome: "not_run", detail: "", technicalReason: "No browser on this machine" }] as Task["result"] extends infer R ? R extends { checks: infer C } ? C : never : never;
    s.emit(projection("1", world([task("r", { status: "review", result: { version: 1, digest: "d", checks, summary: "" }, workspaceDigest: "d" })])));
    await settled();
    expect(all(".inspector .chk .trunc").map(c => c.textContent)).toEqual(["Visual review", "Link check"]);
    const why = all(".inspector .ws-chk-why").map(c => c.textContent);
    expect(why[0]).toMatch(/could not be confirmed/);
    expect(why[0]).not.toContain("ev_1");
    expect(why[1]).toBe("No browser on this machine");
  });

  it("keeps a check row a row and a checkbox a square, though the mockups name both .chk", () => {
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src/workspace.css"), "utf8");
    expect(css).toContain(".chk:not(input) { appearance: auto; width: auto; height: auto; border: 0;");
    expect(css).toContain("input.chk { display: inline-block; min-height: 0; }");
  });
});

describe("the workers list", () => {
  const worker = (id: string, patch: Partial<Worker> = {}): Worker => ({
    id, taskId: "w", executionId: "e", role: "frontend-dev", status: "completed", lane: "execution", goal: `Goal ${id}`, now: null, waitsFor: null, scope: [],
    route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null, ...patch,
  });
  const live = (workers: Worker[], title = "Task w") => ({ ...world([task("w", { status: "working", process: "live", executionId: "e", title })]), workers });
  const card = (text: string) => all(".wcard").find(c => c.textContent?.includes(text));

  it("keeps a worker's card in place while the machine refreshes, so a click on it is never lost", async () => {
    const s = everything();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/w/workers/${"cortex"}`);
    s.emit(projection("1", live([worker("a"), worker("b", { status: "working" })])));
    await settled();
    const finished = card("Goal a")!, running = card("Goal b")!, coordinator = card("Coordinator")!;
    expect(finished && running && coordinator).toBeTruthy();
    // The task itself moves on while its workers do not, as it does on every turn of a running task.
    s.emit(projection("2", live([worker("a"), worker("b", { status: "working" })], "Task w, later")));
    await settled();
    expect(coordinator.isConnected).toBe(true);
    expect(finished.isConnected).toBe(true);
    expect(running.isConnected).toBe(true);
    finished.click();
    await settled();
    expect(finished.getAttribute("aria-selected")).toBe("true");
    expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/w/workers/a`);
  });

  it("lets the owner inspect worker tabs by keyboard before opening one and distinguishes same-role attempts in the picker", async () => {
    const s = everything();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/w/workers/cortex`);
    s.emit(projection("1", live([worker("a"), worker("b", { status: "working" })])));
    await settled();
    const coordinator = card("Coordinator")!;
    coordinator.focus();
    coordinator.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement).toBe(card("Goal b"));
    expect(coordinator.getAttribute("aria-selected")).toBe("true");
    const select = document.querySelector<HTMLSelectElement>(".ws-worker-picker select")!;
    expect([...select.options].map(option => option.text)).toEqual(expect.arrayContaining(["frontend-dev · Working · Goal b", "frontend-dev · Finished · Goal a"]));
  });

  it("still moves a worker that finishes into Finished", async () => {
    const s = everything();
    await mount(s.port, `/m/${LOCAL_MACHINE_ID}/t/w/workers/b`);
    s.emit(projection("1", live([worker("b", { status: "working" })])));
    await settled();
    expect(all(".wlist .wg").map(g => g.textContent)).toEqual(["Coordinator", "Execution1", "Verification"]);
    s.emit(projection("2", live([worker("b")])));
    await settled();
    expect(all(".wlist .wg").map(g => g.textContent)).toEqual(["Coordinator", "Verification", "Finished1"]);
    expect(card("Goal b")?.getAttribute("aria-selected")).toBe("true");
  });
});

describe("the Settings sidebar", () => {
  it("keeps its links in place when the machine is read again", async () => {
    const s = everything();
    const at = (seen: string) => { const state = world([]); state.machines[0].lastConfirmedAt = seen; return state; };
    await mount(s.port, `/settings/machine/${LOCAL_MACHINE_ID}/orchestration`);
    s.emit(projection("1", at("2026-11-07T10:00:00Z")));
    await settled();
    const link = (name: string) => all('nav[aria-label="Settings"] a.snav').find(t => t.textContent?.trim() === name);
    const before = ["Overview", "Orchestration", "Defaults", "Connections", "Remote access"].map(link);
    expect(before.every(Boolean)).toBe(true);
    // Each machine read moves its last-seen time, and nothing else.
    s.emit(projection("2", at("2026-11-07T10:00:30Z")));
    await settled();
    expect(before.map(node => node!.isConnected)).toEqual([true, true, true, true, true]);
  });

  it("keeps a project's limit field, and what is typed in it, through a machine read", async () => {
    const s = everything();
    const at = (seen: string) => { const state = world([]); state.machines[0].lastConfirmedAt = seen; return state; };
    await mount(s.port, `/settings/machine/${LOCAL_MACHINE_ID}/orchestration/ceiling`);
    s.emit(projection("1", at("2026-11-07T10:00:00Z")));
    await settled();
    // The Project picker in the context bar: "Machine only" first, then the machine's projects.
    all("#m-scope-project .mi")[1]!.click();
    await settled();
    const field = $<HTMLInputElement>('input[aria-label="Project Attempts per task"]')!;
    expect(field).toBeTruthy();
    field.focus();
    field.value = "3";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    s.emit(projection("2", at("2026-11-07T10:00:30Z")));
    await settled();
    expect(field.isConnected).toBe(true);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("3");
  });
});

describe("the Inspector", () => {
  it("keeps Open task in place while the selected task moves on", async () => {
    const s = everything();
    await mount(s.port, BOARD);
    s.emit(projection("1", world([task("w", { status: "working", process: "live" })])));
    await settled();
    $('.card[data-task="w"] .card-link')!.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
    await settled();
    const open = () => all(".inspector a").find(a => a.textContent?.includes("Open task"));
    const before = open()!;
    expect(before).toBeTruthy();
    s.emit(projection("2", world([task("w", { status: "working", process: "live", title: "Task w, later" })])));
    await settled();
    expect(before.isConnected).toBe(true);
  });
});

describe("an owner action during a background read", () => {
  it("reads only the action's project, once, with no full reread and no delayed refresh after it", async () => {
    vi.useFakeTimers({ now: new Date("2026-11-23T12:00:00Z"), toFake: ["Date", "performance", "setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"] });
    try {
      let running = false, version = 1, delay = 0;
      const wire = () => wireTask({ version: String(version), ...(running ? { executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING" } : {}) });
      const s = scriptedTransport({
        "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
        "projects.list": () => [{ projectId: "p", displayName: "Fixture", canonicalRoot: "/fixture", activation: {} }],
        "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.start"]),
        "project.snapshot": async () => {
          const reply = snapshot(String(version + 2), [wire()]);
          if (delay) await new Promise(resolve => setTimeout(resolve, delay));
          return reply;
        },
        "tasks.start": async raw => {
          await new Promise(resolve => setTimeout(resolve, 50));
          running = true; version++;
          return { requestId: (raw as { requestId: string }).requestId, outcome: "recorded", target: "task-1", entityVersion: "2", recordedAt: "2026-11-23T12:00:00Z" };
        },
        "machine.call": raw => {
          const { op } = raw as { op: string };
          if (op === "overview") return { ok: true, result: { name: "Fixture" } };
          if (op === "config.read") return { ok: true, result: { config: emptyWorkspace().machines[0].config, connections: [], tokens: {} } };
          if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
          if (op === "repo.snapshot") return { ok: true, result: null };
          if (op === "workers") return { ok: true, result: [] };
          return { ok: true, result: { tasks: {}, sprints: [], messages: [] } };
        },
      });
      const port = createRealPort({ transport: s.transport, actorId: "owner", registry: true });
      const query = vi.spyOn(port, "query");
      history.replaceState(null, "", BOARD);
      const root = document.createElement("div");
      document.body.append(root);
      dispose = render(() => <Workspace port={port} />, root);
      await vi.advanceTimersByTimeAsync(64);
      const column = () => $('.card[data-task="task-1"]')?.closest(".col")?.getAttribute("data-col");
      expect(column()).toBe("todo");
      // A project event starts a background read that is still running when the owner presses Start.
      delay = 200;
      s.send({ type: "invalidated", projectId: "p" });
      await vi.advanceTimersByTimeAsync(10);
      query.mockClear();
      const reads = s.calls.filter(c => c.operation === "project.snapshot").length;
      all("button").find(b => b.textContent === "Start" && b.closest('.card[data-task="task-1"]'))!.click();
      delay = 0;
      await vi.advanceTimersByTimeAsync(400);
      expect(column()).toBe("work");
      // Long past every gap: nothing more is read.
      await vi.advanceTimersByTimeAsync(5000);
      const scopes = query.mock.calls.filter(([operation]) => operation === "project.snapshot").map(([, parameters]) => parameters);
      expect(scopes).toEqual([{ projectId: "p" }]);
      expect(s.calls.filter(c => c.operation === "project.snapshot").length - reads).toBe(1);
      expect(s.calls.filter(c => c.operation === "tasks.start")).toHaveLength(1);
    } finally { vi.useRealTimers(); }
  });
});

describe("card footer spacing", () => {
  it("reserves no height for the empty request status on a card", () => {
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src/workspace.css"), "utf8");
    expect(css).toMatch(/\.card > \.ws-request-status \{[^}]*min-block-size: 0/);
  });
});
