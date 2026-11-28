import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, overlayTask, type Decision, type Sprint, type Task, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { targetPlace } from "../src/notification-target";
import { buildIndex, inboxItems } from "../src/selectors";
import { draftTask, flush, projection, scriptedPort } from "./support";

// A sprint planned by a hidden runtime task: the task reads only on its sprint, as an ordinary task's conversation and workers.
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const settle = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const M = LOCAL_MACHINE_ID;
const PROMPT = "Sprint planning (read-only). Study the project and submit the plan with sprint_plan_submit.";

const question: Decision = { id: "d1", kind: "question", from: "", workerId: null, at: Date.now() - 60_000, withdrawn: false, answer: null, text: "Who is the landing page for?",
  options: [{ label: "Home buyers", optionId: "o1" }, { label: "Offices", optionId: "o2" }] };
const worker: Worker = { id: "w1", taskId: "plan-1", executionId: "e-plan", role: "Explorer", status: "working", lane: "execution", goal: "Read the page templates", now: "Reading templates",
  waitsFor: null, scope: [], route: { model: "Opus 5.5", effort: "High" }, attempt: null, elapsedMs: 12_000, runningSince: null, context: null, usage: null, activity: [], result: null };
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Coffee landing page", title: "Coffee landing", runMode: "manual", targetBranch: null,
  planning: "question", planningTaskId: "plan-1", conversation: [], plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null, ...patch });

function world(options: { sprint?: Partial<Sprint>; asking?: boolean; legacy?: boolean; task?: Partial<Task> } = {}): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  s.projects = [{ id: "p", machineId: M, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main", hasGit: true }];
  const planner: Task = { ...draftTask(), id: "plan-1", projectId: "p", title: "Sprint planning task", description: PROMPT, status: "working", process: "live", executionId: "e-plan", ...options.task };
  const other: Task = { ...draftTask(), id: "t-other", projectId: "p", title: "Standalone fix", status: "needs_you", process: "live", executionId: "e-other" };
  s.tasks = options.legacy ? [other] : [planner, other];
  s.workers = options.legacy ? [] : [worker];
  s.sprints = [options.legacy ? sprint({ planningTaskId: undefined, planning: "ready", conversation: [{ kind: "note", text: "Here is the plan I propose." }], ...options.sprint }) : sprint(options.sprint)];
  const asking = options.asking ?? true;
  s.sessions = [{ ...emptySession("plan-1", Date.now() - 300_000), decisions: asking ? [question] : [], timeline: [
    { id: "c1", kind: "cortex", text: "I am studying the project before proposing a plan." },
    { id: "f1", kind: "fold", text: "Worked 12s, read 4 files", steps: [] },
    { id: "team", kind: "workers", title: "Workers", workerIds: ["w1"] },
    ...(asking ? [{ id: "q1", kind: "decision" as const, decisionId: "d1" }] : []),
  ] }];
  return s;
}

async function mount(path: string, state = world(), draftNamespace?: readonly string[]) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} draftNamespace={draftNamespace} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}
const tabs = () => all('[role="tablist"][aria-label="Sprint views"] [role="tab"]').map(t => t.textContent?.trim());
const conversation = () => $('section.task-panel[aria-label="Conversation"]');

describe("a sprint planned by a runtime task", () => {
  it("shows the planning task's conversation as an ordinary task's, under the sprint's description", async () => {
    await mount(`/m/${M}/p/p/s/sp/planning`);
    expect(tabs()).toEqual(["Conversation", "Workers", "Plan", "Board", "Summary"]);
    expect(conversation()).not.toBeNull();
    expect(all('section[aria-label="Conversation"]')).toEqual([conversation()]);
    // The sprint's own words head it, never the planning task's instructions.
    expect($(".brief")?.textContent).toContain("Coffee landing page");
    expect(document.body.textContent).not.toContain("sprint_plan_submit");
    expect(conversation()!.querySelector(".ai")?.textContent).toContain("I am studying the project");
    expect(conversation()!.querySelector(".fold-t")?.textContent).toBe("Worked 12s, read 4 files");
    // Its question is the ordinary question form, and its team stands above the composer as on a task page.
    expect(conversation()!.querySelector(".dc-q")?.textContent).toBe("Who is the landing page for?");
    expect(conversation()!.querySelector(".ws-team")?.textContent).toContain("Explorer");
  });

  it("answers its question through the sprint's planning", async () => {
    const p = await mount(`/m/${M}/p/p/s/sp/planning`);
    all(".dc .opt").find(o => o.textContent?.includes("Home buyers"))!.click(); await settle();
    all("button").find(b => b.textContent?.trim() === "Send answer" || b.textContent?.trim() === "Send answers" || b.getAttribute("aria-label") === "Send answer")!.click(); await settle();
    const payload = p.commands.at(-1)!.payload as { type: string; sprintId: string; decisionId: string; optionId: string };
    expect(payload).toMatchObject({ type: "answer_planning", sprintId: "sp", decisionId: "d1", optionId: "o1" });
  });

  it("sends a message from the composer to the planning task, through the sprint's planning", async () => {
    const p = await mount(`/m/${M}/p/p/s/sp/planning`, world({ asking: false, sprint: { planning: "planning" } }));
    const field = conversation()!.querySelector<HTMLTextAreaElement>(".dock textarea")!;
    field.value = "Add a pricing section"; field.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const send = all<HTMLButtonElement>(".dock button").find(b => b.getAttribute("aria-label") === "Send message now")!;
    send.click(); await settle();
    const payload = p.commands.at(-1)!.payload as { type: string; sprintId: string; text: string };
    expect(payload).toMatchObject({ type: "plan_message", sprintId: "sp", text: "Add a pricing section" });
    // The message reads in the conversation at once.
    expect(conversation()!.textContent).toContain("Add a pricing section");
  });

  it.each(["message", "said"] as const)("shows saved copies only until the matching runtime %s and answer are visible", async kind => {
    const text = "Use this pricing sheet";
    const file = { name: "prices.csv", mimeType: "text/csv", size: 12, sha256: "a".repeat(64) };
    const state = world({ asking: false, sprint: { planning: "planning",
      plan: { version: 1, items: [{ key: "first", description: "Plan the landing page", dependsOn: [] }] },
      conversation: [
        { kind: "answer", text: "Home buyers" },
        { kind: "message", text, attachments: [file] },
        { kind: "message", text, attachments: ["revised-prices.csv"] },
        { kind: "question", text: "Who should test it?" },
        { kind: "answer", text: "Home buyers" },
        { kind: "note", text: "Retained planning note" },
      ],
    } });
    const p = await mount(`/m/${M}/p/p/s/sp/planning`, state);
    expect(conversation()!.querySelector(".qa")?.textContent).toContain("Home buyers");
    expect(conversation()!.querySelector(".mine .ws-att")?.textContent).toBe("prices.csv");

    const decisions = [{ ...question, answer: { text: "Home buyers", at: 22 } }];
    const messages: WorkspaceState["messages"] = [{ id: "m1", taskId: "plan-1", workerId: null, text, attachments: [file],
      mode: "send_now", delivery: "delivered", at: 23, deliveredAt: 23, failure: null }];
    const timeline = [...state.sessions[0].timeline, { id: "q1", kind: "decision" as const, decisionId: "d1" },
      kind === "message" ? { id: "m-row", kind, messageId: "m1" } : { id: "m-row", kind, text: ` ${text} ` }];
    p.emit(projection("2", { ...state, messages, sessions: [{ ...state.sessions[0], decisions, timeline }] })); await settle();

    const feed = conversation()!;
    const answers = [...feed.querySelectorAll(".qa")];
    expect(answers.map(row => row.textContent)).toEqual([
      expect.stringContaining("Who is the landing page for?"),
      expect.stringContaining("Who should test it?"),
    ]);
    expect(answers.every(row => row.textContent?.includes("Home buyers"))).toBe(true);
    expect([...feed.querySelectorAll(".mine .ws-att")].map(chip => chip.textContent)).toEqual(["prices.csv", "revised-prices.csv"]);
    const rows = [...feed.querySelector(".tl-in")!.children];
    const working = rows.findIndex(row => row.matches(".ws-working"));
    expect(working).toBeGreaterThan(-1);
    expect(rows.findIndex(row => row.textContent?.includes("Retained planning note"))).toBeLessThan(working);
    const actions = feed.querySelector(".conversation-actions")!;
    expect(rows.indexOf(actions)).toBeGreaterThan(working);
    expect(actions.querySelector(".sp-plan-line button")?.textContent).toBe("Open plan");
  });

  it("stops planning from the composer while the planner works", async () => {
    const p = await mount(`/m/${M}/p/p/s/sp/planning`, world({ asking: false, sprint: { planning: "planning" } }));
    all<HTMLButtonElement>(".dock button").find(b => b.getAttribute("aria-label") === "Stop")!.click(); await settle();
    expect(p.commands.at(-1)!.payload).toMatchObject({ type: "cancel_planning", sprintId: "sp" });
  });

  it("keeps the proposed plan for approval on the sprint, and closes sending once its tasks exist", async () => {
    const plan = { version: 1, items: [{ key: "t1", title: "Hero", description: "Build the hero", dependsOn: [] }, { key: "t2", title: "Pricing", description: "Build pricing", dependsOn: ["t1"] }] };
    await mount(`/m/${M}/p/p/s/sp/planning`, world({ asking: false, sprint: { planning: "ready", plan, history: [plan] }, task: { status: "review", process: "stopped" } }));
    expect($(".sp-plan-line")?.textContent).toContain("Plan v1 proposed: 2 tasks");
    expect(all("button").some(b => b.getAttribute("aria-label")?.startsWith("Create 2 tasks"))).toBe(true);
    expect(conversation()!.querySelector(".mc-effect")?.textContent).toContain("Create tasks waits for the revised plan");
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    await mount(`/m/${M}/p/p/s/sp/planning`, world({ asking: false, sprint: { planning: "created", plan, history: [plan], materializedVersion: 1 }, task: { status: "done", process: "stopped" } }));
    expect(conversation()!.querySelector<HTMLTextAreaElement>(".dock textarea")!.placeholder).toBe("Tasks are created. Edit them on the Board.");
    expect(conversation()!.querySelector<HTMLTextAreaElement>(".dock textarea")!.disabled).toBe(true);
    expect($(".sp-plan-line")?.textContent).toContain("Tasks created from Plan v1");
  });

  it("has a Workers tab with the task page's workers view", async () => {
    await mount(`/m/${M}/p/p/s/sp/planning`);
    all<HTMLAnchorElement>('[role="tab"]').find(t => t.textContent?.trim() === "Workers")!.click(); await settle();
    expect(location.pathname).toBe(`/m/${M}/p/p/s/sp/workers`);
    expect($('section[aria-label="Workers"] .wlist')?.textContent).toContain("Explorer");
    expect($(".livepane")).not.toBeNull();
    // A worker opens on the sprint, never on a page of the planning task.
    $<HTMLAnchorElement>('.wlist .wcard[href$="/workers/w1"]')!.click(); await settle();
    expect(location.pathname).toBe(`/m/${M}/p/p/s/sp/workers/w1`);
  });

  it("keeps the planning task out of the sidebar, the board and the Inbox, and counts its question once as the sprint's", async () => {
    await mount(`/m/${M}/p/p/board`);
    expect(document.body.textContent).not.toContain("Sprint planning task");
    expect(all(".card").map(c => c.textContent).join(" ")).toContain("Standalone fix");
    const sprintRow = all(".task-row").find(r => r.textContent?.includes("Sprint: Coffee landing"));
    expect(sprintRow?.querySelector(".sig")?.textContent).toBe("Needs you");
    const items = inboxItems(buildIndex(world(), Date.now()));
    expect(items.filter(i => i.sprintId === "sp")).toHaveLength(1);
    expect(items.some(i => i.view?.id === "plan-1")).toBe(false);
    expect(items.find(i => i.sprintId === "sp")?.ask).toBe("Who is the landing page for?");
  });

  it.each(["draft", "failed"] as const)("keeps a retired %s planner out of Board lists, counts and Inbox", async status => {
    const state = world({ task: { status, process: "stopped", executionId: status === "draft" ? null : "e-plan" } });
    const before = buildIndex(state, Date.now());
    state.tasks[0] = overlayTask(state.tasks[0], { status: "done", closedAs: "cancel", retiredPlanner: true }, "main");
    state.sprints[0] = sprint({ planningTaskId: undefined, planning: status === "draft" ? "cancelled" : "planning" });
    const after = buildIndex(state, Date.now());
    expect(after.views.map(v => v.id)).toEqual(["t-other"]);
    expect(after.summaries.get("p")?.counts).toEqual(before.summaries.get("p")?.counts);
    expect(after.summaries.get("p")?.archived).toBe(0);
    expect(inboxItems(after).some(item => item.view?.id === "plan-1")).toBe(false);
    expect(after.byId.get("plan-1")?.task.closedAs).toBe("cancel");
    await mount(`/m/${M}/p/p/board`, state);
    expect(document.body.textContent).not.toContain("Sprint planning task");
    expect(all(".card").map(c => c.textContent).join(" ")).toContain("Standalone fix");
  });

  it("says the sprint needs the owner as soon as its planning task asks, before the sprint's own state follows", async () => {
    const items = inboxItems(buildIndex(world({ sprint: { planning: "planning" } }), Date.now()));
    expect(items.filter(i => i.sprintId === "sp").map(i => i.kind)).toEqual(["Planning"]);
  });

  it.each([false, true])("keeps the draft, answers and plan after failure (legacy: %s)", async legacy => {
    const path = `/m/${M}/p/p/s/sp/planning`;
    const state = world({ legacy, asking: false, sprint: {
      planning: "planning",
      conversation: [{ kind: "question", text: "Who for?" }, { kind: "answer", text: "Home buyers" }],
      plan: { version: 4, items: [{ key: "first", description: "Plan the new landing page", dependsOn: [] }] },
    } });
    if (!legacy) {
      state.sessions[0].decisions = [{ ...question, answer: { text: "Home buyers", at: 22 } }];
      state.sessions[0].timeline.push({ id: "answered", kind: "decision", decisionId: question.id });
    }
    const p = await mount(path, state);
    const field = $(".dock textarea") as HTMLTextAreaElement;
    field.value = "Include a pricing page";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    const failed = { ...state.sprints[0], planning: "cancelled" as const, planningFailure: { text: "Planning stopped.", step: "run" as const, code: "planning_task_stopped", at: 123 } };
    p.emit(projection("2", { ...state, sprints: [failed] })); await settle();
    expect($(".l-lane [role=alert]")?.textContent).toContain("Planning stopped.");
    expect(conversation()?.textContent).toContain("Home buyers");
    expect(conversation()?.textContent).toContain("Plan v4 proposed");
    expect(field.value).toBe("Include a pricing page");
    all("button").find(b => b.textContent?.trim() === "Change request")!.click(); await settle();
    expect(document.activeElement).toBe(field);
    expect(field.readOnly || field.disabled).toBe(false);
    expect(field.value).toBe("Include a pricing page");
    expect(p.commands).toEqual([]);
    $('.dock [aria-label="Send and resume planning"]')!.click(); await settle();
    expect(p.commands.at(-1)?.payload).toMatchObject({ type: "plan_message", sprintId: "sp", text: "Include a pricing page" });
  });

  it("lists one connected, unarchived sprint diagnostic in Inbox and removes it on recovery", async () => {
    const failure = { text: "Planning stopped.", step: "run", code: "planning_task_stopped", at: 123 } as const;
    const warning = { text: "The service will keep retrying.", step: "internal", code: "planning_pass_failed", at: 456, count: 3 } as const;
    const source = world({ asking: false, sprint: { planning: "cancelled", planningFailure: failure } });
    source.sprints.push(sprint({ id: "warn", planning: "planning", planningWarning: warning, planningTaskId: undefined }));
    source.sprints.push(sprint({ id: "archived", archivedAt: 1, planning: "cancelled", planningFailure: "Old failure" }));
    const rows = () => inboxItems(buildIndex(source, Date.now())).filter(d => d.sprintId);
    expect(rows()).toMatchObject([
      { sprintId: "sp", kind: "Planning failed", tone: "st-bad", ask: failure.text, at: failure.at },
      { sprintId: "warn", kind: "Planning problem", tone: "st-warn", ask: warning.text, at: warning.at },
    ]);
    source.machines[0].connection = "disconnected";
    expect(rows()).toEqual([]);
    source.machines[0].connection = "connected";
    const p = await mount(`/m/${M}/p/p/s/warn/planning`, source);
    expect($(".l-lane [role=status]")?.textContent).toContain(warning.text);
    const recovered = { ...source, sprints: source.sprints.map(s => ({ ...s, planning: "planning" as const, planningWarning: undefined, planningFailure: undefined })) };
    p.emit(projection("2", recovered)); await settle();
    expect($(".l-lane [role=status]")).toBeNull();
    expect(inboxItems(buildIndex(recovered, Date.now())).filter(d => d.sprintId)).toEqual([]);
  });

  it("keeps a question ahead of a warning in the sprint's single Inbox row", () => {
    const state = world({ sprint: { planningWarning: { text: "Retrying", step: "internal", code: "planning_pass_failed", at: 2, count: 3 } } });
    expect(inboxItems(buildIndex(state, Date.now())).filter(d => d.sprintId === "sp")).toMatchObject([{ kind: "Planning", ask: question.text }]);
  });

  it("opens an address of the planning task on its sprint", async () => {
    await mount(`/m/${M}/t/plan-1`);
    expect(location.pathname).toBe(`/m/${M}/p/p/s/sp/planning`);
    expect(conversation()).not.toBeNull();
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    await mount(`/m/${M}/t/plan-1/workers/w1`);
    expect(location.pathname).toBe(`/m/${M}/p/p/s/sp/workers/w1`);
  });

  it("routes a notification about the planning task to its sprint's conversation", () => {
    const place = targetPlace(world(), { kind: "question", project: "p", task: "plan-1", decision: "d1" }, M);
    expect(place.route).toMatchObject({ name: "sprint", sprint: "sp", tab: "planning" });
    expect(place.reveal).toBe('[id="decision-d1"]');
  });

  it("shows only a skeleton while the conversation is unread, then every saved answer, question and status together", async () => {
    const saved = "Only stage";
    const state = world({ asking: false, sprint: { planning: "planning", conversation: [{ kind: "answer", text: saved }] }, task: { planningSprintId: "sp" } });
    const loading = { ...state, sessions: [{ ...state.sessions[0], historyRead: { state: "loading" as const } }] };
    const p = await mount(`/m/${M}/p/p/s/sp/planning`, loading);
    const feed = conversation()!.querySelector(".timeline")!;
    expect(feed.querySelector(".ws-skel, [aria-busy]")).not.toBeNull();
    expect(feed.textContent).not.toContain(saved);
    
    expect(feed.querySelector(".answered")).toBeNull();
    expect(feed.textContent).not.toContain("Coffee landing page");
    p.emit(projection("2", state)); await settle();
    expect(feed.textContent).toContain(saved);
    expect(feed.textContent).toContain("I am studying the project");
    expect(feed.textContent).toContain("Coffee landing page");
  });

  it("keeps saved notes, messages and unanswered questions in Conversation after cancellation", async () => {
    const state = world({ legacy: true, sprint: { planning: "cancelled", conversation: [
      { kind: "note", text: "Here is the plan I propose." },
      { kind: "message", text: "Use the docs too." },
      { kind: "question", text: "What about the examples?" },
    ] } });
    const p = await mount(`/m/${M}/p/p/s/sp/planning`, state);
    expect(tabs()).toEqual(["Conversation", "Plan", "Board", "Summary"]);
    expect(conversation()?.textContent).toContain("Here is the plan I propose.");
    expect(conversation()?.textContent).toContain("Use the docs too.");
    expect(conversation()?.textContent).toContain("What about the examples?");
    expect(conversation()?.querySelector(".dc-q")).toBeNull();
    all<HTMLButtonElement>("button").find(b => b.textContent?.trim() === "Resume planning")!.click();
    await settle();
    expect(p.commands.at(-1)?.payload).toMatchObject({ type: "resume_planning", sprintId: "sp" });
  });

  it.each(["row first", "link first"])("keeps one editor and never lists a planner during split reads (%s)", async order => {
    const savedNote = "A saved planning note.";
    const initial = world({ legacy: true, sprint: { planning: "planning", conversation: [{ kind: "note", text: savedNote }] } });
    const live = world({ asking: false, sprint: { planning: "planning", conversation: initial.sprints[0].conversation }, task: { planningSprintId: "sp" } });
    const p = await mount(`/m/${M}/p/p/s/sp/planning`, initial, ["split-read-fixture"]);
    const panel = conversation()!, feed = panel.querySelector(".timeline");
    const field = panel.querySelector<HTMLTextAreaElement>(".dock textarea")!;
    expect(tabs()).toEqual(["Conversation", "Plan", "Board", "Summary"]);
    expect(feed?.textContent).toContain("Coffee landing page");
    expect(feed?.textContent).toContain("Planning is starting");
    field.value = "Keep this draft";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    const picker = panel.querySelector<HTMLInputElement>('.dock input[type="file"]')!;
    Object.defineProperty(picker, "files", { value: [new File(["example"], "brief.txt", { type: "text/plain" })] });
    picker.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(panel.querySelector(".dock .ws-att")?.textContent).toContain("brief.txt");
    field.focus();
    field.setSelectionRange(3, 3);

    const intermediate = order === "row first" ? { ...initial, tasks: [{ ...live.tasks[0], status: "draft" as const, process: "stopped" as const, executionId: null }, ...initial.tasks] }
      : { ...initial, sprints: live.sprints };
    p.emit(projection("2", intermediate)); await settle();
    expect(conversation()).toBe(panel);
    expect(panel.querySelector(".timeline")).toBe(feed);
    expect(feed?.textContent).toContain(order === "row first" ? "Planning is starting" : "Loading the planning conversation");
    expect($(".sidebar")?.textContent).not.toContain("Sprint planning task");
    expect($(".sidebar")?.textContent).toContain("Standalone fix");
    expect(tabs()).not.toContain("Workers");

    live.sessions[0].timeline.push({ id: "mirrored-note", kind: "cortex", text: savedNote });
    const loading = { ...live, sessions: [{ ...live.sessions[0], historyRead: { state: "loading" as const } }] };
    p.emit(projection("3", loading)); await settle();
    expect(feed?.textContent).toContain("Loading the conversation");
    // Saved notes and the sprint brief are part of the conversation: only its skeleton shows until the first reads are in.
    expect(feed?.textContent).not.toContain(savedNote);
    expect(feed?.textContent).not.toContain("Coffee landing page");
    expect(feed?.textContent).not.toContain("I am studying the project");

    p.emit(projection("4", live)); await settle();
    expect(conversation()).toBe(panel);
    expect(panel.querySelector(".timeline")).toBe(feed);
    expect(panel.querySelector(".dock textarea")).toBe(field);
    expect(document.activeElement).toBe(field);
    expect(field.selectionStart).toBe(3);
    expect(field.value).toBe("Keep this draft");
    expect(panel.querySelector(".dock .ws-att")?.textContent).toContain("brief.txt");
    expect([...feed!.querySelectorAll(".ai")].filter(row => row.textContent === savedNote)).toHaveLength(1);
    expect(feed?.textContent).toContain("I am studying the project");
    expect(tabs()).toEqual(["Conversation", "Workers", "Plan", "Board", "Summary"]);
    expect($(".sidebar")?.textContent).not.toContain("Sprint planning task");
    const replacement = structuredClone(live);
    replacement.tasks[0].executionId = "new-planner-attempt";
    p.emit(projection("5", replacement)); await settle();
    expect(field.value).toBe("");
    expect(panel.querySelector(".dock .ws-att")).toBeNull();
    expect(p.commands).toEqual([]);
  });
});
