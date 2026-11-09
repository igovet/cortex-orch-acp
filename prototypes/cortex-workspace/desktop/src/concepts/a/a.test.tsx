import { afterEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { createMockStore } from "../../store";
import ConceptA from "./index";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

function mount() {
  const store = createMockStore();
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <ConceptA store={store} />, root);
  return { store, root };
}
const tick = () => new Promise(r => setTimeout(r, 0));
const key = (init: KeyboardEventInit) => document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, ...init }));
const buttons = (root: HTMLElement) => [...root.querySelectorAll<HTMLButtonElement>("button")];
const byName = (root: HTMLElement, name: string) => buttons(root).find(b => (b.getAttribute("aria-label") ?? b.textContent ?? "").includes(name));
const openCard = (root: HTMLElement, taskId: string) => {
  const card = [...root.querySelectorAll<HTMLButtonElement>(".a-card")].find(c => c.getAttribute("aria-label")!.includes(taskId));
  expect(card, `card ${taskId}`).toBeTruthy();
  card!.click();
};

describe("concept A Task Desk", () => {
  it("starts on a virtualized seven-column board with persistent mock labelling", () => {
    const { root } = mount();
    expect(root.querySelector("h1")?.textContent).toBe("Task Desk");
    expect(root.querySelectorAll('[role="grid"] [role="columnheader"]')).toHaveLength(7);
    const cards = root.querySelectorAll(".a-card").length;
    expect(cards).toBeGreaterThan(0);
    expect(cards).toBeLessThan(120); // 500 tasks, viewport-bounded DOM
    expect(root.querySelectorAll('.a-card[tabindex="0"]')).toHaveLength(1); // one Tab stop for the board
    expect(root.textContent).toContain("All actions are mock and change only in-memory state");
    expect(root.textContent).toContain("Fixture data, not runtime evidence");
  });

  it("moves card focus with arrow keys and opens a task with tabs reachable by Alt+number", async () => {
    const { root, store } = mount();
    const first = root.querySelector<HTMLButtonElement>('.a-card[tabindex="0"]')!;
    first.focus();
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    const second = document.activeElement as HTMLElement;
    expect(second.dataset.idx).toBe("1");
    second.click();
    await tick();
    expect(document.activeElement?.id).toBe("a-task-title");
    expect(store.state().selectedTaskId).toBe(second.getAttribute("aria-label")!.match(/mock-task-\d+/)![0]);
    key({ key: "3", code: "Digit3", altKey: true });
    expect(root.querySelector("#a-tab-result")?.getAttribute("aria-selected")).toBe("true");
  });

  it("creates a task in memory without implying an agent start", async () => {
    const { root, store } = mount();
    key({ key: "n", ctrlKey: true });
    await tick();
    const title = root.querySelector<HTMLInputElement>("#a-new-title")!;
    expect(document.activeElement).toBe(title);
    title.value = "Add rate limiting to /login";
    title.dispatchEvent(new Event("input", { bubbles: true }));
    root.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    expect(store.state().fixture.tasks).toHaveLength(501);
    expect(root.querySelector("#a-task-title")?.textContent).toBe("Add rate limiting to /login");
    expect(root.textContent).toContain("No agent was started");
    expect(root.textContent).toContain("Not started (mock)");
  });

  it("blocks acceptance of a stale result and accepts a complete one only as a mock", () => {
    const { root, store } = mount();
    openCard(root, "mock-task-00004");
    key({ key: "3", code: "Digit3", altKey: true });
    expect(root.textContent).toContain("Result changed after review");
    expect(byName(root, "Accept mock-result-stale")!.disabled).toBe(true);
    byName(root, "Board")!.click();
    openCard(root, "mock-task-00005");
    key({ key: "3", code: "Digit3", altKey: true });
    byName(root, "Accept mock-result-complete")!.click();
    expect(store.state().acceptedResultIds).toEqual(["mock-result-complete"]);
    expect(root.textContent).toContain("Accepted (mock) · mock-result-complete");
    expect(root.textContent).toContain("Accepting does not commit, push or deploy.");
  });

  it("disables every mock mutation while disconnected and shows the last confirmed time", () => {
    const { root, store } = mount();
    store.selectScenario("disconnected");
    expect(root.textContent).toContain("Machine disconnected. Last confirmed 2030-01-01 00:00 UTC");
    openCard(root, "mock-task-00001");
    key({ key: "2", code: "Digit2", altKey: true });
    const approve = byName(root, "Approve once (mock)")!;
    expect(approve.disabled).toBe(true);
    expect(root.textContent).toContain("Disconnected; actions unavailable.");
    expect(root.textContent).toContain("current state unknown");
  });

  it("keeps commit disabled through a mock code and Git conflict resolution", () => {
    const { root, store } = mount();
    store.selectScenario("conflict");
    openCard(root, "mock-task-00006");
    key({ key: "5", code: "Digit5", altKey: true });
    expect(root.textContent).toContain("Merge stopped: 1 file has conflicts");
    expect(byName(root, "Commit (mock)")!.disabled).toBe(true);
    expect(root.textContent).toContain("Resolve 1 remaining conflict.");
    key({ key: "4", code: "Digit4", altKey: true });
    byName(root, "Save (mock)")!.click();
    expect(root.textContent).toContain("This file changed on disk since you opened it");
    byName(root, "Compare")!.click();
    byName(root, "Keep mine (mock)")!.click();
    expect(store.state().conflictResolution).toBe("local");
    key({ key: "5", code: "Digit5", altKey: true });
    byName(root, "Mark resolved (mock)")!.click();
    expect(byName(root, "Commit (mock)")!.disabled).toBe(true);
    expect(root.textContent).toContain("a mock resolution does not change Git");
  });
});
