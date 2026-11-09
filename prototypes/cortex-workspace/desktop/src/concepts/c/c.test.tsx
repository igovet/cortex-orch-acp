import { afterEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { createMockStore, type MockStore } from "../../store";
import ConceptC from "./index";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

function mount(store: MockStore = createMockStore("c")) {
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <ConceptC store={store} />, root);
  return { root, store };
}
const flush = () => new Promise(r => setTimeout(r, 0));
const byText = (root: HTMLElement, selector: string, text: string) =>
  Array.from(root.querySelectorAll<HTMLElement>(selector)).find(el => el.textContent?.includes(text));
const queueRow = (root: HTMLElement, taskId: string) => root.querySelector<HTMLButtonElement>(`.c-queue [data-key="${taskId}"]`)!;
const key = (el: Element, k: string, init: KeyboardEventInit = {}) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));

describe("concept C focus workspace", () => {
  it("starts on the attention queue in fixed section order with mock labelling", () => {
    const { root } = mount();
    expect(root.querySelector("h1")?.textContent).toBe("Focus Workspace");
    const heads = Array.from(root.querySelectorAll(".c-queue .c-qhead")).map(h => h.textContent);
    expect(heads[0]).toMatch(/^Needs your decision \(4\)/);
    expect(heads[1]).toMatch(/^Blocked \(1\)/);
    expect(heads[2]).toMatch(/^Changed since last visit \(1\)/);
    expect(heads[3]).toMatch(/^Running \(1\)/);
    expect(heads[4]).toMatch(/Quiet tasks \(493\)/);
    expect(root.querySelector(".c-strip")?.textContent).toContain("All actions are mock and change only in-memory state");
    expect(root.querySelector(".c-status")?.textContent).toContain("Fixture data, not runtime evidence");
    // exactly one Tab stop inside the queue
    expect(root.querySelectorAll('.c-queue [data-roving][tabindex="0"]').length).toBe(1);
  });

  it("browsing with J/K moves focus only; Enter/click selects and focuses the decision heading", async () => {
    const { root, store } = mount();
    const first = queueRow(root, "mock-task-00001");
    first.focus();
    key(first, "j");
    expect(document.activeElement).toBe(queueRow(root, "mock-task-00002"));
    expect(store.state().selectedTaskId).toBe("mock-task-00000");
    queueRow(root, "mock-task-00002").click();
    await flush();
    expect(store.state().selectedTaskId).toBe("mock-task-00002");
    expect(document.activeElement?.id).toBe("c-focus-heading");
    expect(document.activeElement?.textContent).toBe("Which synthetic layout?");
    key(document.activeElement!, "Escape");
    expect(document.activeElement).toBe(queueRow(root, "mock-task-00002"));
  });

  it("validates and records a question answer as mock only", async () => {
    const { root, store } = mount();
    queueRow(root, "mock-task-00002").click();
    await flush();
    root.querySelector<HTMLButtonElement>('[aria-label="Send answer (mock)"]')!.click();
    expect(root.textContent).toContain("Choose an answer before sending.");
    expect(store.state().responses).toEqual({});
    root.querySelector<HTMLInputElement>('input[value="Compact"]')!.click();
    root.querySelector<HTMLButtonElement>('[aria-label="Send answer (mock)"]')!.click();
    expect(store.state().responses["mock-decision-question"]).toBe("Compact");
    await flush();
    expect(document.activeElement?.id).toBe("c-focus-heading");
    expect(root.textContent).toContain("No agent was started");
    expect(queueRow(root, "mock-task-00002").closest("section")?.querySelector(".c-qhead")?.textContent).toMatch(/^Changed since last visit/);
  });

  it("never offers real acceptance for a stale result", async () => {
    const { root, store } = mount();
    store.selectScenario("stale");
    await flush();
    expect(root.textContent).toContain("Result changed after review");
    const accept = root.querySelector<HTMLButtonElement>('[aria-label="Accept mock-result-stale (mock)"]')!;
    expect(accept.getAttribute("aria-disabled")).toBe("true");
    expect(document.getElementById(accept.getAttribute("aria-describedby")!)?.textContent).toContain("Previous acceptance (mock) is invalidated");
    accept.click();
    expect(store.state().acceptedResultIds).toEqual([]);
    expect(store.state().outcomes).toEqual([]);
    expect(root.querySelector(".c-decision")?.textContent).not.toContain("Accepted (mock)");
  });

  it("accepts a complete result only as a labelled mock with evidence open by default", async () => {
    const { root, store } = mount();
    queueRow(root, "mock-task-00005").click();
    await flush();
    expect(root.querySelector<HTMLElement>('[data-l2="3"]')!.getAttribute("aria-expanded")).toBe("true");
    expect(root.querySelector<HTMLElement>('[data-l2="1"]')!.getAttribute("aria-expanded")).toBe("false");
    expect(root.textContent).toContain("Accepting does not commit, push or deploy.");
    root.querySelector<HTMLButtonElement>('[aria-label="Accept mock-result-complete (mock)"]')!.click();
    expect(store.state().acceptedResultIds).toEqual(["mock-result-complete"]);
    expect(root.textContent).toContain("Accepted (mock) · mock-result-complete");
    expect(root.textContent).toContain("not runtime evidence");
  });

  it("disables every mock mutating control while disconnected and shows Unknown for running work", async () => {
    const { root, store } = mount();
    store.selectScenario("disconnected");
    await flush();
    expect(root.textContent).toContain("Machine disconnected.");
    expect(queueRow(root, "mock-task-00000").textContent).toContain("Unknown");
    queueRow(root, "mock-task-00001").click();
    await flush();
    const approve = root.querySelector<HTMLButtonElement>('[aria-label="Approve once (mock)"]')!;
    expect(approve.getAttribute("aria-disabled")).toBe("true");
    approve.click();
    expect(store.state().responses).toEqual({});
    expect(root.textContent).toContain("Disconnected; actions unavailable.");
  });

  it("creates a task in memory only and lists it under Changed since last visit", async () => {
    const { root, store } = mount();
    byText(root, ".c-topbar button", "New task")!.click();
    await flush();
    const create = root.querySelector<HTMLButtonElement>('[aria-label="Create task (mock)"]')!;
    create.click();
    expect(root.textContent).toContain("Describe what should be done.");
    const field = root.querySelector<HTMLTextAreaElement>(".c-composer")!;
    field.value = "Synthetic composer task";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    key(field, "Enter");
    await flush();
    const id = store.state().selectedTaskId;
    expect(id).toMatch(/^mock-created-task-/);
    expect(queueRow(root, id).textContent).toContain("Not started (mock)");
    expect(root.textContent).toContain("No agent was started.");
    expect(store.state().fixture.executions.some(e => e.taskId === id)).toBe(false);
  });

  it("reveals details progressively and reaches board, result/diff and code/Git from the queue", async () => {
    const { root, store } = mount();
    store.selectScenario("conflict");
    await flush();
    const section = root.querySelector<HTMLButtonElement>('[data-l2="5"]')!;
    expect(section.getAttribute("aria-expanded")).toBe("false");
    section.focus();
    key(section, "4", { altKey: true, code: "Digit4" });
    expect(root.querySelector('[data-l2="4"]')!.getAttribute("aria-expanded")).toBe("true");
    byText(root, ".c-l3", "Open full diff")!.click();
    await flush();
    expect(root.querySelector("#c-deep-heading")?.textContent).toContain("Full diff");
    expect(root.querySelector(".c-rail")?.textContent).toContain("need you");
    byText(root, ".c-tab", "Code")!.click();
    expect(root.textContent).toContain("This file changed on disk since you opened it");
    const save = root.querySelector<HTMLButtonElement>('[aria-label="Save (mock)"]')!;
    expect(save.getAttribute("aria-disabled")).toBe("true");
    byText(root, ".c-editor button", "Compare")!.click();
    root.querySelector<HTMLButtonElement>('[aria-label="Keep local (mock)"]')!.click();
    expect(store.state().conflictResolution).toBe("local");
    byText(root, ".c-tab", "Git")!.click();
    expect(root.textContent).toContain("Merge stopped: 1 file has conflicts on");
    root.querySelector<HTMLButtonElement>(".c-railback")!.click();
    await flush();
    expect(root.querySelector(".c-queue")).not.toBeNull();
    byText(root, ".c-mainnav button", "All tasks")!.click();
    await flush();
    expect(root.querySelectorAll(".c-tasklist tbody tr").length).toBe(500);
    byText(root, ".c-seg button", "Board")!.click();
    expect(root.querySelectorAll(".c-col").length).toBe(4);
    root.querySelector<HTMLButtonElement>(".c-card")!.click();
    await flush();
    expect(root.querySelector(".c-queue")).not.toBeNull();
  });

  it("shows empty, error and loading states without stale task content", async () => {
    const { root, store } = mount();
    store.selectScenario("empty");
    await flush();
    expect(root.textContent).toContain("Nothing needs your decision in this view.");
    expect(root.textContent).toContain("No tasks in this project yet.");
    store.selectScenario("error");
    await flush();
    expect(root.textContent).toContain("fixture error E_FIXTURE_READ");
    byText(root, "button", "Try again")!.click();
    expect(store.state().scenario).toBe("default");
    byText(root, ".c-strip button", "Preview loading")!.click();
    expect(root.querySelector(".c-queue")?.textContent).toContain("Loading tasks…");
    expect(root.querySelectorAll(".c-queue .c-skel").length).toBe(4);
  });

  it("windows the 10,000-event project history", async () => {
    const { root } = mount();
    queueRow(root, "mock-task-00000").click();
    await flush();
    byText(root, ".c-l2-toggle", "Why this is asked")!.click();
    byText(root, ".c-l3", "Open full discussion")!.click();
    await flush();
    byText(root, ".c-deep button", "Show project history")!.click();
    const rows = root.querySelectorAll(".c-vrow");
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.length).toBeLessThan(100);
    expect(rows[0].getAttribute("aria-setsize")).toBe("10000");
  });
});
