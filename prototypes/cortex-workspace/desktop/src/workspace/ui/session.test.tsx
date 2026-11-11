import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { WORKSPACE_CASES } from "../adapter/fixtures";
import { WORKSPACE_SESSION_WORKERS as W } from "../adapter/session-fixture";
import Workspace from "./index";
import { hrefOf, parseRoute, type Route } from "@cortex/workspace-ui/router";

const LOCAL = "mock-machine-00000", REMOTE = "mock-machine-00001", CEDAR = "mock-project-00001";
const SESSION = `/m/${LOCAL}/t/${WORKSPACE_CASES.session}`;

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const tick = () => new Promise(r => setTimeout(r, 0));
const wait = (ms: number) => new Promise(r => setTimeout(r, ms));
async function mount(path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace autoplay={false} />, root);
  await tick();
}
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const named = (selector: string, name: string) => all(selector).find(el => (el.getAttribute("aria-label") ?? el.textContent ?? "").includes(name))!;
const click = async (el: Element) => { (el as HTMLElement).click(); await tick(); };
const type = async (el: HTMLTextAreaElement | HTMLInputElement, text: string) => { el.value = text; el.dispatchEvent(new InputEvent("input", { bubbles: true })); await tick(); };
const key = async (el: EventTarget, k: string, opts: KeyboardEventInit = {}) => { el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...opts })); await tick(); };
const composer = () => $<HTMLTextAreaElement>('.dock textarea');
const queueTexts = () => all(".qrow .grow").map(el => el.textContent);
const titleActions = () => $(".tb-actions").textContent!;
const sim = async () => { await click($(".mock-chip")); await click(named("#m-mock .mi", "Simulate the next step")); };

describe("task routes", () => {
  it("round-trips every task view through its URL", () => {
    const base = { name: "task", machine: LOCAL, task: "t" } as const;
    const routes: Route[] = [base, { ...base, tab: "workers" }, { ...base, tab: "changes" }, { ...base, tab: "workers", worker: "w" }, { ...base, tab: "workers", worker: "w", live: true }];
    for (const route of routes) expect(parseRoute(hrefOf(route))).toEqual(route);
    expect(parseRoute(`/m/${LOCAL}/t/t/nope`).name).toBe("notfound");
    expect(parseRoute(`/m/${LOCAL}/t/t/workers/w/nope`).name).toBe("notfound");
  });
});

describe("task session", () => {
  it("shows the brief, plan, workers and usage from the fixture, and says it is mock", async () => {
    await mount(SESSION);
    expect($(".brief").textContent).toContain("random 401s");
    expect($(".brief").textContent).toContain("Mock session");
    expect($(".plan-h").textContent).toContain("Executing 2 of 5 done");
    expect(all(".node .who").map(n => n.textContent)).toEqual(["Code Explorer", "Test Author", "Backend Dev", "QA Engineer", "Migration Dev"]);
    expect($(".wrow .role").textContent).toBe("Backend Dev");
    expect($(".wrow .route").textContent).toBe("Opus, High");
    expect($(".wrow .el").textContent).toMatch(/^\d+m \d+s$/);
    expect(named('[role="tab"]', "Workers").textContent).toContain("2 active of 6");
    expect($(".planbar").textContent).toContain("41%");
    expect($(".planbar").textContent).toContain("182k");
    expect($(".composer .ring")).toBeTruthy();
    expect(titleActions()).toContain("Working");
    expect(titleActions()).toContain("Answer");
    expect(titleActions()).toContain("Stop");
  });

  it("lists every worker with route, activity, context and usage, and leaves out what nobody reported", async () => {
    await mount(`${SESSION}/workers`);
    expect(all(".wg").map(g => g.textContent)).toEqual(["Coordinator", "Execution3", "Review1", "Verification", "Finished2"]);
    const card = (name: string) => all(".wcard").find(c => c.querySelector(".l1")!.textContent!.includes(name))!;
    expect(card("Backend Dev").textContent).toContain("Opus, High");
    expect(card("Backend Dev").textContent).toContain("Context 38%");
    expect(card("Backend Dev").textContent).toContain("Usage 64k");
    expect(card("Migration Dev").textContent).toContain("Route not assigned");
    expect(card("Migration Dev").textContent).not.toContain("Context");
    expect(card("Migration Dev").textContent).not.toContain("Usage");
    expect(card("Migration Dev").textContent).not.toMatch(/unavailable/i);
    expect(card("Migration Dev").textContent).not.toMatch(/\b0\b/);
    expect(card("Runtime verification").textContent).toContain("Automatic, not a worker");
  });

  it("keeps a finished worker readable and points questions to Cortex", async () => {
    await mount(`${SESSION}/workers/${W.author}`);
    const detail = $(".detail").textContent!;
    expect(detail).toContain("Added tests/auth/test_race.py");
    expect(detail).toContain("Recorded by runtime");
    expect(detail).toContain("Full auth suite");
    expect(detail).toContain("Test database was busy");
    expect(detail).toContain("Attempt 1");
    expect(detail).toContain("Side effects of its commands may not appear");
    expect($(".detail textarea")).toBeNull();
    await click(named(".detail .btn", "Ask about this worker"));
    expect(location.pathname).toBe(SESSION);
    expect(composer().value).toBe("About Test Author: ");
  });

  it("says where a worker opens below the two-pane width", async () => {
    await mount(SESSION);
    await click(all(".wrow")[1]);
    expect(location.pathname).toBe(`${SESSION}/workers/${W.qa}`);
    expect($(".detail").textContent).toContain("Answer it in the conversation");
  });

  it("answers a question in place, and the worker carries on", async () => {
    await mount(SESSION);
    const needs = () => $(".tb-actions").textContent!;
    await click(all(".dc .opt")[0]);
    expect($(".dc")).toBeNull();
    expect(all(".qa-a").some(a => a.textContent!.includes("50 parallel refreshes"))).toBe(true);
    expect(needs()).not.toContain("Answer");
    expect(needs()).toContain("Working");
    expect(all(".wrow")[1].textContent).toContain("Working");
  });

  it("takes a free-text answer, and number keys pick an option", async () => {
    await mount(SESSION);
    await click(named(".opt", "Other answer"));
    const input = $<HTMLInputElement>('.ws-dc-other textarea');
    await type(input, "Ten");
    await key(input, "Enter");
    expect(all(".qa-a").some(a => a.textContent!.includes("Ten"))).toBe(true);
  });

  it("shows a permission card with the safe choice first", async () => {
    await mount(`/m/${LOCAL}/t/${WORKSPACE_CASES.permission}`);
    expect(all(".ws-dc-acts .btn").map(b => b.textContent)).toEqual(["Deny", "Allow once"]);
    expect($(".dc").textContent).toContain("Mock execution remains blocked");
    await click(all(".ws-dc-acts .btn")[1]);
    expect(all(".qa-a").some(a => a.textContent!.includes("You allowed once"))).toBe(true);
  });

  it("renames the title in place, and the rename survives", async () => {
    await mount(SESSION);
    await click($(".ws-title"));
    const input = $<HTMLInputElement>(".ws-title-in");
    await type(input, "Race fix");
    await key(input, "Enter");
    expect($(".ws-title").textContent).toBe("Race fix");
    expect($(".brief").textContent).not.toMatch(/Title|Rename/);
  });

  it("changes model, effort, access and language for this task only, and lists what is unavailable", async () => {
    await mount(SESSION);
    await click(named("#m-model-task .mi", "Sonnet"));
    await click(named("#m-effort-task .mi", "Medium"));
    await click(named("#m-mode-task .mi", "Guarded"));
    await click(named("#m-lang-task .mi", "Russian"));
    const bar = $(".composer .c-bar").textContent!;
    for (const text of ["Sonnet", "Medium", "Guarded", "Russian"]) expect(bar).toContain(text);
    expect(named("#m-model-task .mi", "Local model").getAttribute("aria-disabled")).toBe("true");
    expect(named("#m-effort-task .mi", "Max").getAttribute("aria-disabled")).toBe("true");
    expect($(".inspector").textContent).toContain("Sonnet, Medium");
    await mount(`/m/${LOCAL}/t/${WORKSPACE_CASES.working}`);
    expect($(".composer .c-bar").textContent).toContain("Opus");
  });
});

describe("steer and queue", () => {
  const write = (text: string) => type(composer(), text);

  it("steers with Enter: it is sent, on the record, and cannot be edited", async () => {
    await mount(SESSION);
    const before = all(".mine").length;
    await write("Also log a warning when the lock wait exceeds 2 seconds.");
    await key(composer(), "Enter");
    expect(all(".mine").length).toBe(before + 1);
    expect(all(".mine").at(-1)!.textContent).toContain("exceeds 2 seconds");
    expect(all(".mine-meta").at(-1)!.textContent).toContain("Steer received, applies after the current step");
    expect(composer().value).toBe("");
    expect(queueTexts()).not.toContain("Also log a warning when the lock wait exceeds 2 seconds.");
  });

  it("queues with Ctrl+Enter: it stays unsent, editable and marked as such", async () => {
    await mount(SESSION);
    const sent = all(".mine").length;
    await write("Then update the changelog.");
    await key(composer(), "Enter", { ctrlKey: true });
    expect(all(".mine").length).toBe(sent);
    expect(queueTexts()).toEqual(["When the lock lands, also update the auth runbook.", "Then split the fix into two commits for review.", "Then update the changelog."]);
    expect($(".queue-h").textContent).toContain("Queued 3, not sent yet");
    expect($(".queue-h").textContent).toContain("Saved on this machine · not sent");
  });

  it("offers Stop only while the composer is empty, and never sends an empty message", async () => {
    await mount(SESSION);
    const send = () => all(".composer .ws-send").map(b => b.textContent);
    expect(send()).toEqual(["Stop"]);
    expect(named(".composer .c-send", "Queue").getAttribute("aria-disabled")).toBe("true");
    await write("Hold on");
    expect(send()[0]).toContain("Steer");
    await write("   ");
    expect(send()).toEqual(["Stop"]);
    const sent = all(".mine").length;
    await key(composer(), "Enter");
    expect(all(".mine").length).toBe(sent);
  });

  it("keeps Shift+Enter for a new line", async () => {
    await mount(SESSION);
    const sent = all(".mine").length;
    await write("First line");
    await key(composer(), "Enter", { shiftKey: true });
    expect(all(".mine").length).toBe(sent);
    expect(composer().value).toBe("First line");
  });

  it("reorders, removes and sends queued messages", async () => {
    await mount(SESSION);
    await key(all(".qrow .grip")[0], "ArrowDown");
    expect(queueTexts()).toEqual(["Then split the fix into two commits for review.", "When the lock lands, also update the auth runbook."]);
    await tick();
    expect(document.activeElement).toBe(all(".qrow .grip")[1]);

    const sent = all(".mine").length;
    await click(all(".qrow .btn").find(b => b.textContent === "Send now")!);
    expect(all(".mine").length).toBe(sent + 1);
    expect(all(".mine").at(-1)!.textContent).toBe("Then split the fix into two commits for review.");
    expect(queueTexts()).toEqual(["When the lock lands, also update the auth runbook."]);

    await click(named(".qrow .ib", "Remove from queue"));
    expect($(".queue")).toBeNull();
  });

  it("Edit takes a queued message out of the queue and back into the composer, keeping what was typed", async () => {
    await mount(SESSION);
    await write("Already typed");
    await click(all(".qrow .btn").find(b => b.textContent === "Edit")!);
    expect(queueTexts()).toEqual(["Then split the fix into two commits for review."]);
    expect(composer().value).toBe("Already typed\nWhen the lock lands, also update the auth runbook.");
    await tick();
    expect(document.activeElement).toBe(composer());
  });

  it("a queued message shows its attachments, and Edit brings them back to the composer to send again", async () => {
    await mount(SESSION);
    const before = queueTexts().length;
    const file = new File(["x"], "trace.log");
    const picker = $<HTMLInputElement>('.composer input[type="file"]');
    Object.defineProperty(picker, "files", { value: [file], configurable: true });
    picker.dispatchEvent(new Event("change", { bubbles: true }));
    await expect.poll(() => $(".composer .ws-att")?.textContent).toContain("trace.log");
    await write("With the trace");
    await key(composer(), "Enter", { ctrlKey: true });
    expect(queueTexts().length).toBe(before + 1);
    expect($(".composer .ws-att")).toBeNull();
    const row = all(".qrow").at(-1)!;
    expect(row.querySelector(".ws-att")!.textContent).toContain("trace.log");
    await click([...row.querySelectorAll<HTMLElement>(".btn")].find(b => b.textContent === "Edit")!);
    expect(queueTexts().length).toBe(before);
    expect(composer().value).toBe("With the trace");
    expect($(".composer .ws-att").textContent).toContain("trace.log");
  });

  it("keeps a draft when moving between tabs", async () => {
    await mount(SESSION);
    await write("Half a thought");
    await click(named('[role="tab"]', "Workers"));
    await click(named('[role="tab"]', "Conversation"));
    expect(composer().value).toBe("Half a thought");
  });

  it("gives the dock no surface of its own, so the chat shows around the queue", async () => {
    const { readFileSync } = await import(/* @vite-ignore */ `node:${"fs"}`) as { readFileSync(path: URL, encoding: "utf8"): string };
    const file = (name: string) => readFileSync(new URL(name, import.meta.url), "utf8");
    const css = file("../../../../../../packages/workspace-ui/src/mockup-styles.css") + file("../../../../../../packages/workspace-ui/src/workspace.css");
    const bodies = [...css.matchAll(/(?:^|\n)\.(?:dock|queue)\s*\{([^}]*)\}/g)].map(m => m[1]);
    expect(bodies.length).toBeGreaterThanOrEqual(2);
    for (const body of bodies) expect(body).not.toMatch(/background|box-shadow|border/);
  });
});

describe("stop, continue and close", () => {
  const closeOpen = () => $("dialog[open]");
  const menu = async (label: string) => { await click(named('[aria-label="Task actions"]', "Task actions")); await click(named("#m-task .mi", label)); };

  it("stops without a confirmation, keeps everything and offers Resume", async () => {
    await mount(SESSION);
    await click(named(".tb-actions .btn", "Stop"));
    expect(titleActions()).toContain("Paused");
    expect(titleActions()).toContain("Resume");
    expect(titleActions()).not.toContain("Stop");
    expect($(".phases").textContent).toContain("Paused at Executing 2/5");
    expect(all(".ws-rule").at(-1)!.textContent).toContain("You stopped the task, 1 worker action was interrupted");
    expect(queueTexts().length).toBe(2);
    expect($(".inspector").textContent).toContain("Stop effect");
    expect($(".inspector").textContent).toContain("2 kept");
    expect(document.body.dataset.stop).toBe("stopped");
    expect(composer().placeholder).toContain("Any message resumes");
    expect(all(".dc .opt")[0].getAttribute("aria-disabled")).toBe("true");
    expect($(".dc").textContent).toContain("Preserved while paused");
  });

  it("resumes from the button, and any message resumes too", async () => {
    await mount(SESSION);
    await click(named(".tb-actions .btn", "Stop"));
    await click(named(".tb-actions .btn", "Resume"));
    expect(titleActions()).toContain("Stop");
    expect(all(".ws-rule").at(-1)!.textContent).toContain("You resumed the task");
    await click(named(".tb-actions .btn", "Stop"));
    await type(composer(), "Carry on, but keep the timeout.");
    await key(composer(), "Enter");
    expect(titleActions()).toContain("Stop");
    expect(all(".mine").at(-1)!.textContent).toBe("Carry on, but keep the timeout.");
  });

  it("stays honest about a stop the engine has not confirmed, and blocks Close until it is", async () => {
    await mount(SESSION);
    await click($(".mock-chip"));
    await click(named("#m-mock .mi", "Next stop stays unconfirmed"));
    await click(named(".tb-actions .btn", "Stop"));
    expect(titleActions()).toContain("Stopping");
    expect($(":is(.strip.warn, .l-strip[data-tone=warn])").textContent).toContain("1 worker is not confirmed as stopped");
    const close = named(".tb-actions .btn", "Close task");
    expect(close.getAttribute("aria-disabled")).toBe("true");
    await click(close);
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(named(".composer .c-send", "Resume").getAttribute("aria-disabled")).toBe("true");
    await click(named(":is(.strip.warn, .l-strip[data-tone=warn]) .btn", "Retry"));
    expect(titleActions()).toContain("Paused");
    expect(document.querySelector(":is(.strip.warn, .l-strip[data-tone=warn])")).toBeNull();
  });

  it("shows the fixture's unconfirmed stop as unconfirmed", async () => {
    await mount(`/m/${LOCAL}/t/${WORKSPACE_CASES.stopping}`);
    expect(titleActions()).toContain("Stopping");
    expect($(":is(.strip.warn, .l-strip[data-tone=warn])").textContent).toContain("not confirmed");
  });

  it("states what closing does in numbers, and Close as is keeps the work with a lasting warning", async () => {
    await mount(SESSION);
    await menu("Close task");
    const text = closeOpen().textContent!;
    expect(text).toContain("2 workers will be stopped: Backend Dev and QA Engineer");
    expect(text).toContain("1 open question will be withdrawn");
    expect(text).toContain("2 queued messages and 2 pending worker messages will not be applied");
    expect(text).toContain("the result is marked not verified");
    expect(text).toContain("Changes stay in the worktree: 3 files, +85 −5");
    expect(named("dialog[open] .btn", "Close task").classList.contains("btn-primary")).toBe(true);
    await click(named("dialog[open] .btn", "Close task"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(titleActions()).toContain("Closed as is, not verified");
    expect(all(".ws-rule").at(-1)!.textContent).toContain("You closed the task as is, not verified");
    expect($(":is(.strip.warn, .l-strip[data-tone=warn])").textContent).toContain("Not verified");
    expect(composer().disabled).toBe(true);
    expect($(".queue")).toBeNull();
  });

  it("marks only Cancel as dangerous, and Keep working changes nothing", async () => {
    await mount(SESSION);
    await menu("Close task");
    await click(all<HTMLInputElement>('dialog[open] input[type="radio"]')[1]);
    const cancel = named("dialog[open] [data-bar=dialog] .btn", "Cancel task");
    expect(cancel.classList.contains("btn-danger")).toBe(true);
    await click(named("dialog[open] [data-bar=dialog] .btn", "Keep working"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(titleActions()).toContain("Working");
    await menu("Close task");
    await click(all<HTMLInputElement>('dialog[open] input[type="radio"]')[1]);
    await click(named("dialog[open] [data-bar=dialog] .btn", "Cancel task"));
    expect(titleActions()).toContain("Cancelled");
    expect(all(".ws-rule").at(-1)!.textContent).toContain("You cancelled the task");
    expect(document.querySelector(":is(.strip.warn, .l-strip[data-tone=warn])")).toBeNull();
  });

  it("does not offer to open the task the owner is already in", async () => {
    await mount(SESSION);
    await click(named('[aria-label="Task actions"]', "Task actions"));
    const items = all("#m-task .mi").map(i => i.textContent);
    expect(items.some(i => i!.includes("Open task"))).toBe(false);
    expect(items.some(i => i!.includes("Close task"))).toBe(true);
  });

  it("says a paused task's workers are already stopped", async () => {
    await mount(SESSION);
    await click(named(".tb-actions .btn", "Stop"));
    await click(named(".tb-actions .btn", "Close task"));
    const text = closeOpen().textContent!;
    expect(text).toContain("All workers are already stopped");
    expect(text).toContain("Keep it paused");
    expect(text).toContain("Resume picks it up again");
  });

  it("lets the close dialog fall back to Stop", async () => {
    await mount(SESSION);
    await menu("Close task");
    await click(named("dialog[open] .note-box .lnk", "Stop"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(titleActions()).toContain("Paused");
  });

  it("uses the same Stop from the keyboard and the composer", async () => {
    await mount(SESSION);
    await key(document, ".", { ctrlKey: true });
    expect(titleActions()).toContain("Paused");
    await click(named(".tb-actions .btn", "Resume"));
    await click($(".composer .ws-send"));
    expect(titleActions()).toContain("Paused");
  });

  it("offers Close from a board card's menu, and the card moves to Done", async () => {
    await mount(`/m/${LOCAL}/p/${CEDAR}/board`);
    const card = () => $(`[data-task="${WORKSPACE_CASES.working}"]`);
    await click(card().querySelector('[aria-label^="Actions for"]')!);
    await click(named("#m-task .mi", "Close task"));
    await click(named("dialog[open] [data-bar=dialog] .btn", "Close task"));
    expect(card()?.closest("section")?.getAttribute("aria-label")).not.toBe("Working");
  });
});

describe("live worker", () => {
  const LIVE = `${SESSION}/workers/${W.backend}/live`;
  const send = () => $<HTMLButtonElement>('.dock-w .c-send');
  const box = () => $<HTMLTextAreaElement>(".dock-w textarea");
  const bubble = (text: string) => all(".me").find(m => m.textContent!.includes(text))!;

  it("shows the whole history, with every kind of event", async () => {
    await mount(LIVE);
    const text = $(".stream").textContent!;
    for (const part of ["Attempt 1 ended", "Assignment:", "Read 3 files, searched 2 patterns", "trace_path", "Native, no receipt", "Failed, exit 1", "Mock activity, not a real transcript"]) expect(text).toContain(part);
    expect($(".diff .a").textContent).toContain("query(RefreshToken)");
    expect($(".d-grid").textContent).toContain("38%, 76k of 200k");
    expect($(".d-grid").textContent).toContain("2 of 2");
    expect($(".inspector").textContent).toContain("Selected worker");
  });

  it("filters by kind, and the counters match what shows", async () => {
    await mount(LIVE);
    const filter = (name: string) => named('[aria-label="Filter activity"] button', name);
    const shown = () => all(".stream .ev").length;
    const edits = Number(filter("Edits").querySelector(".n")!.textContent);
    await click(filter("Edits"));
    expect(shown()).toBe(edits);
    expect(all(".stream .me").length).toBe(0);
    expect(all(".stream .divide").length).toBeGreaterThan(0);
    await click(filter("Errors"));
    expect(shown()).toBe(1);
    expect($(".stream .ev").textContent).toContain("mypy auth");
    await click(filter("Messages"));
    expect(all(".stream .me").length).toBeGreaterThan(0);
  });

  it("finds text in the history", async () => {
    await mount(LIVE);
    await click(named(".live-tools .ib", "Find in this activity"));
    await type($<HTMLInputElement>(".ws-find"), "mypy");
    expect(all(".stream .ev").length).toBe(1);
  });

  it("shows each delivery state, and only what is undelivered can be withdrawn", async () => {
    await mount(LIVE);
    expect(bubble("Check the session cleanup").textContent).toContain("Delivered does not mean read");
    expect(bubble("Check the session cleanup").querySelector(".lnk")).toBeNull();
    expect(bubble("Rename the lock helper").classList.contains("off")).toBe(true);
    expect(bubble("Keep the 30 second").textContent).toContain("Sending");
    expect(bubble("Use the existing retry").textContent).toContain("Pending delivery at the next step");
    await click(named(".me .lnk", "Withdraw"));
    expect(bubble("Use the existing retry").textContent).toContain("Withdrawn by you, before delivery");
  });

  it("writes to a running worker, and delivery moves on one step at a time", async () => {
    await mount(LIVE);
    expect(send().getAttribute("aria-disabled")).toBe("true");
    await type(box(), "Keep the lock scope to the rotation only.");
    await key(box(), "Enter");
    expect(box().value).toBe("");
    expect(bubble("Keep the lock scope").textContent).toContain("Sending");
    // The fixture's earlier messages settle first.
    for (let i = 0; i < 6 && !bubble("Keep the lock scope").textContent!.includes("Delivered"); i++) await sim();
    expect(bubble("Keep the lock scope").textContent).toContain("Delivered does not mean read");
  });

  it("offers a recovery for what could not be sent", async () => {
    await mount(LIVE);
    const failed = () => bubble("Also log when the lock wait is long");
    expect(failed().textContent).toContain("connection to the machine dropped");
    await click([...failed().querySelectorAll(".lnk")].find(l => l.textContent === "Retry")!);
    expect(failed().textContent).toContain("Sending");
    const ended = () => bubble("Start with the mobile client");
    expect(ended().textContent).toContain("attempt 1 ended first");
    expect(ended().textContent).toContain("Retry on attempt 2");
    await click([...ended().querySelectorAll(".lnk")].find(l => l.textContent === "Send")!);
    expect(ended().classList.contains("off")).toBe(true);
    await click(named('[role="tab"]', "Conversation"));
    expect(all(".mine").at(-1)!.textContent).toContain("Start with the mobile client");
  });

  it("stops one command with a reason, and the worker and task keep running", async () => {
    await mount(LIVE);
    await click(named(".card2-f .btn", "Stop command"));
    const dialog = $("dialog[open]");
    expect(dialog.textContent).toContain("pytest tests/auth/test_race.py -q");
    expect(dialog.textContent).toContain("will not repeat this command");
    expect(named("dialog[open] [data-bar=dialog] .btn", "Stop command").getAttribute("aria-disabled")).toBe("true");
    await type($<HTMLTextAreaElement>("dialog[open] textarea"), "Hangs on the shared test database.");
    await click(named("dialog[open] [data-bar=dialog] .btn", "Stop command"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect($(".stream").textContent).toContain("Stop requested");
    for (let i = 0; i < 6 && !$(".stream").textContent!.includes("Stopped: Hangs"); i++) await sim();
    expect($(".stream").textContent).toContain("Stopped: Hangs on the shared test database.");
    expect(titleActions()).toContain("Working");
  });

  it("offers the same command stop from the conversation row", async () => {
    await mount(SESSION);
    expect($(".wexp .cmd").textContent).toContain("pytest tests/auth/test_race.py -q");
    await click(named(".wexp .btn", "Stop"));
    expect($("dialog[open]").textContent).toContain("will not repeat this command");
    await type($<HTMLTextAreaElement>("dialog[open] textarea"), "Hangs.");
    await click(named("dialog[open] [data-bar=dialog] .btn", "Stop command"));
    expect(document.querySelector(".wexp")).toBeNull();
  });

  it("stops one worker while the task keeps running", async () => {
    await mount(LIVE);
    await click(named(".d-title .btn", "Stop worker"));
    await click(named("dialog[open] [data-bar=dialog] .btn", "Stop worker"));
    expect($(".d-title").textContent).toContain("Cancelled");
    expect(box()).toBeNull();
    expect(titleActions()).toContain("Needs you");
    expect(bubble("Use the existing retry").textContent).toContain("the worker was stopped first");
  });

  it("turns Follow off when the reader scrolls up, and counts what arrives", async () => {
    await mount(LIVE);
    const scroller = $(".stream"), follow = $<HTMLInputElement>('[aria-label="Follow new activity"]');
    expect(follow.checked).toBe(true);
    expect(document.querySelector(".new-pill")).toBeNull();
    Object.defineProperty(scroller, "scrollTop", { configurable: true, writable: true, value: -300 });
    scroller.dispatchEvent(new Event("scroll"));
    await tick();
    expect(follow.checked).toBe(false);
    expect(document.body.dataset.follow).toBe("off");
    for (let i = 0; i < 6 && !$(".new-pill").textContent!.includes("new"); i++) await sim();
    expect($(".new-pill").textContent).toMatch(/\d+ new/);
    await click($(".new-pill"));
    expect(follow.checked).toBe(true);
    expect((scroller as HTMLElement).scrollTop).toBe(0);
    expect(document.querySelector(".new-pill")).toBeNull();
  });

  it("does not let a finished worker be messaged", async () => {
    await mount(`${SESSION}/workers/${W.explorer}/live`);
    expect(document.querySelector(".dock-w textarea")).toBeNull();
    expect($(".stream").textContent).toContain("Traced the 401 race");
  });
});

describe("voice input", () => {
  const start = async () => { await click(named(".composer .ib", "Voice input")); await wait(1000); };

  it("records into a bar with live text, and sends nothing on its own", async () => {
    await mount(SESSION);
    const sent = all(".mine").length;
    await start();
    expect($(".micbar").textContent).toContain("Simulated");
    expect($(".mic-text").textContent!.length).toBeGreaterThan(3);
    await wait(600);
    expect(all(".mine").length).toBe(sent);
    expect(composer().value).toBe("");
    await click(named(".micbar .btn", "Cancel"));
    expect($(".micbar")).toBeNull();
    expect(composer().value).toBe("");
    expect(all(".mine").length).toBe(sent);
  });

  it("inserts into the editable draft, and sending stays a separate click", async () => {
    await mount(SESSION);
    await type(composer(), "Also");
    const sent = all(".mine").length;
    await start();
    await click(named(".micbar .btn", "Insert"));
    expect(composer().value).toMatch(/^Also and /);
    expect(all(".mine").length).toBe(sent);
    expect($(".micbar")).toBeNull();
  });

  it("sends only through Insert and send", async () => {
    await mount(SESSION);
    const sent = all(".mine").length;
    await start();
    await click(named(".micbar .btn", "Insert and send"));
    expect(all(".mine").length).toBe(sent + 1);
    expect(composer().value).toBe("");
  });

  it("explains a refused microphone or a missing provider, and typing still works", async () => {
    await mount(SESSION);
    await click($(".mock-chip"));
    await click(named("#m-mock .mi", "microphone refused"));
    await click(named(".composer .ib", "Voice input"));
    expect($(".micbar").textContent).toContain("Microphone access was refused. Typing still works.");
    await click(named(".micbar .btn", "Dismiss"));
    await click(named("#m-mock .mi", "no provider"));
    await click(named(".composer .ib", "Voice input"));
    expect($(".micbar").textContent).toContain("No dictation provider");
    expect(composer().disabled).toBe(false);
  });
});

describe("task states the composer must respect", () => {
  it("offers Start on a draft, and starting opens the work", async () => {
    await mount(`/m/${LOCAL}/t/mock-task-00300`);
    expect(composer().disabled).toBe(true);
    await click(named(".composer .ws-send", "Start"));
    expect(titleActions()).toContain("Stop");
    expect(composer().disabled).toBe(false);
    expect($(".ai").textContent).toContain("Mock session");
    expect($(".wrow .role").textContent).toBe("Frontend Dev");
  });

  it("reads as last known and offers nothing on an offline machine", async () => {
    await mount(`/m/${REMOTE}/t/${WORKSPACE_CASES.offline}`);
    expect($(":is(.strip.info, .l-strip[data-tone=info])").textContent).toContain("Last known: Working");
    expect(composer().disabled).toBe(true);
    expect(titleActions()).not.toContain("Stop");
    expect(named('[aria-label="Coordinator model: Opus"]', "Coordinator").getAttribute("aria-disabled")).toBe("true");
  });

  it("keeps a closed task read-only", async () => {
    await mount(`/m/${LOCAL}/t/${WORKSPACE_CASES.closedAsIs}`);
    expect(composer().disabled).toBe(true);
    expect($(":is(.strip.warn, .l-strip[data-tone=warn])").textContent).toContain("Not verified");
    await click(named('[aria-label="Task actions"]', "Task actions"));
    expect(named("#m-task .mi", "Close task").getAttribute("aria-disabled")).toBe("true");
  });

  it("says no shell runs behind the terminal", async () => {
    await mount(`${SESSION}/terminal`);
    await click(named(".note-panel .btn", "Open the bottom panel"));
    expect($(".terminal").textContent).toContain("No shell runs");
  });
});

describe("new task", () => {
  const box = () => $<HTMLTextAreaElement>(".composer textarea");
  const named_ = async () => expect.poll(() => document.querySelector(".title-prev .tt")?.textContent).toBeTruthy();

  it("asks only for a description, then names it, and a click on the title changes it", async () => {
    await mount(`/m/${LOCAL}/new?project=${CEDAR}`);
    expect(document.querySelector('input[aria-label="Task title"]')).toBeNull();
    expect($(".title-prev").textContent).toContain("Written from your description");
    await type(box(), "Limit the export endpoint to sixty requests a minute.");
    expect($(".title-prev").textContent).toContain("Naming…");
    await named_();
    expect($(".title-prev .tt").textContent).toBe("Limit the export endpoint to sixty requests a minute");
    expect($(".rc .mono").textContent).toContain("cw/limit-the-export-endpoint");
    await click($(".title-prev .tt"));
    expect(document.querySelector(".title-prev .btn")).toBeNull();
    const title = $<HTMLInputElement>('.title-prev input');
    await type(title, "Rate-limit exports");
    await key(title, "Enter");
    expect($(".title-prev .tt").textContent).toBe("Rate-limit exports");
    expect($(".rc .mono").textContent).toBe("cw/rate-limit-exports");
  });

  it("keeps the temporary title when naming fails, with Retry", async () => {
    await mount(`/m/${LOCAL}/new?project=${CEDAR}`);
    await click($(".mock-chip"));
    await click(named("#m-mock .mi", "Next title generation fails"));
    await type(box(), "Limit the export endpoint.");
    await expect.poll(() => $(".title-prev").textContent).toContain("Title not generated");
    expect($(".title-prev .tt").textContent).toBe("Limit the export endpoint");
    await click(named("#m-mock .mi", "Next title generation fails"));
    await click(named(".title-prev .btn", "Retry"));
    await expect.poll(() => $(".title-prev").textContent).not.toContain("Title not generated");
  });

  it("offers the main checkout with its warning, and worktree by default", async () => {
    await mount(`/m/${LOCAL}/new?project=${CEDAR}`);
    expect(all<HTMLInputElement>('input[name="checkout"]')[0].checked).toBe(true);
    expect(document.querySelector(".create .note-box")).toBeNull();
    await click(all<HTMLInputElement>('input[name="checkout"]')[1]);
    expect($(".create .note-box").textContent).toContain("Rules for two tasks writing here at once are not settled yet");
  });

  it("starts the task with the chosen settings and target, and opens its session", async () => {
    await mount(`/m/${LOCAL}/new?project=${CEDAR}`);
    await type(box(), "Limit the export endpoint.");
    await named_();
    await click(named("#m-model-new .mi", "GPT-6.1 Sol"));
    await click(named("#m-new-target .mi", "Other branch"));
    const other = $<HTMLInputElement>('[aria-label="Other branch"]');
    await type(other, "release/2.4");
    await key(other, "Enter");
    expect($('.pick[aria-label^="Merge into"]').textContent).toContain("release/2.4");
    await click(named(".create-foot .btn", "Start now"));
    expect(location.pathname).toContain("/t/");
    expect($(".ws-title").textContent).toBe("Limit the export endpoint");
    expect($(".composer .c-bar").textContent).toContain("GPT-6.1 Sol");
    expect($(".inspector").textContent).toContain("release/2.4");
    expect($(".branch-chip").textContent).toContain("cw/limit-the-export-endpoint");
  });

  it("cuts a long branch name at a word", async () => {
    await mount(`/m/${LOCAL}/new?project=${CEDAR}`);
    await type(box(), "Limit the data export endpoint to sixty requests per minute.");
    await named_();
    expect($(".rc .mono").textContent).toBe("cw/limit-the-data-export-endpoint");
  });

  it("can dictate a description, and it only lands in the draft", async () => {
    await mount(`/m/${LOCAL}/new?project=${CEDAR}`);
    await click(named(".composer .ib", "Voice input"));
    await wait(600);
    await click(named(".micbar .btn", "Insert"));
    expect(box().value.length).toBeGreaterThan(0);
    expect(location.pathname).toBe(`/m/${LOCAL}/new`);
    expect(document.querySelector(".micbar .btn-primary")).toBeNull();
  });
});
