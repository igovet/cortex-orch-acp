import { afterEach, expect, it } from "vitest";
import { render } from "solid-js/web";
import { createMockStore } from "../../store";
import ConceptB from "./index";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; document.documentElement.style.fontSize = ""; });

function mount() {
  const store = createMockStore("b");
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <ConceptB store={store} />, root);
  return { store, root };
}
const key = (init: KeyboardEventInit) => window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
const button = (root: HTMLElement, label: string) =>
  [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => (b.getAttribute("aria-label") ?? b.textContent ?? "").includes(label));
const enter = (el: Element) => el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));

it("starts on the editor and opens Mission Control and the same task beside the code", () => {
  const { root, store } = mount();
  expect(root.querySelector("h1")?.textContent).toBe("IDE with Mission Control");
  expect(root.querySelector("#cb-g1 textarea")?.getAttribute("aria-label")).toContain("src/auth/limiter.ts");
  expect(root.querySelector("#cb-g2")).toBeNull();
  key({ key: "M", code: "KeyM", ctrlKey: true, shiftKey: true });
  expect(root.querySelector("#cb-side-title")?.textContent).toBe("Mission Control");
  expect(root.querySelector("#cb-g2")).toBeNull(); // nothing opens automatically
  enter(root.querySelector('[data-need="mock-decision-permission"]')!);
  const g2 = root.querySelector("#cb-g2")!;
  expect(g2.textContent).toContain("Allow mock workspace edit?");
  expect(g2.querySelector('[role=tab][aria-selected=true]')?.textContent).toContain("mock-task-00001");
  expect(root.querySelector("#cb-g1 textarea")).not.toBeNull();
  expect(store.state().selectedTaskId).toBe("mock-task-00001");
  // the same task's result, then the board, open in group 2 while group 1 keeps the code
  enter(root.querySelector('[data-need="mock-result-complete"]')!);
  expect(root.querySelector("#cb-g2")!.textContent).toContain("Accepting does not commit, push or deploy.");
  button(root, "Open board (")!.click();
  expect(root.querySelector("#cb-g2 [role=tab][aria-selected=true]")?.textContent).toContain("Board");
  expect(root.querySelector("#cb-g1 textarea")).not.toBeNull();
});

it("disconnected fixture cannot commit any mock action", () => {
  const { root, store } = mount();
  store.selectScenario("disconnected");
  key({ key: "M", code: "KeyM", ctrlKey: true, shiftKey: true });
  enter(root.querySelector('[data-need="mock-decision-permission"]')!);
  expect(root.textContent).toContain("Machine disconnected. Last confirmed 2030-01-01 00:00 UTC.");
  const approve = button(root, "Approve once (mock)")!;
  expect(approve.disabled).toBe(true);
  approve.click();
  expect(store.state().responses).toEqual({});
  expect(root.textContent).toContain("Disconnected; actions unavailable");
  key({ key: "N", code: "KeyN", ctrlKey: true, shiftKey: true });
  const title = root.querySelector<HTMLInputElement>("#cb-new-title")!;
  title.value = "Synthetic"; title.dispatchEvent(new InputEvent("input", { bubbles: true }));
  expect(button(root, "Create task (mock)")!.disabled).toBe(true);
  key({ key: "s", code: "KeyS", ctrlKey: true });
  expect(root.textContent).toContain("Save unavailable: Disconnected; actions unavailable.");
  const mockButtons = [...root.querySelectorAll<HTMLButtonElement>("button[aria-label$='(mock)']")].filter(b => !b.getAttribute("aria-label")!.startsWith("Reconnect"));
  expect(mockButtons.length).toBeGreaterThan(0);
  expect(mockButtons.every(b => b.disabled)).toBe(true);
  expect(store.state().outcomes).toEqual([]);
});

it("accepts a complete result as mock only and blocks a stale one", () => {
  const { root, store } = mount();
  key({ key: "M", code: "KeyM", ctrlKey: true, shiftKey: true });
  enter(root.querySelector('[data-need="mock-result-complete"]')!);
  button(root, "Accept mock-result-complete (mock)")!.click();
  expect(store.state().acceptedResultIds).toEqual(["mock-result-complete"]);
  expect(root.textContent).toContain("Accepted (mock) · mock-result-complete");
  expect(root.textContent).toContain("Mock outcome: Mock acceptance recorded in memory. Not runtime evidence or a Git action.");
  store.selectScenario("stale");
  const staleRow = [...root.querySelectorAll<HTMLButtonElement>(".cb-mc-row")].find(b => b.textContent?.includes("Result changed after review"))!;
  enter(staleRow);
  expect(root.textContent).toContain("Previous acceptance (mock) is invalidated.");
  const accept = button(root, "Accept mock-result-stale (mock)")!;
  expect(accept.disabled).toBe(true);
  expect(store.state().acceptedResultIds).toEqual(["mock-result-complete"]);
});

it("keeps 500 tasks and 10,000 events viewport-bounded in the DOM", () => {
  const { root } = mount();
  key({ key: "B", code: "KeyB", ctrlKey: true, shiftKey: true });
  const list = root.querySelector("#cb-side-list")!;
  const options = list.querySelectorAll("[role=option]");
  expect(options[0].getAttribute("aria-setsize")).toBe("500");
  expect(options.length).toBeLessThan(60);
  key({ key: "M", code: "KeyM", ctrlKey: true, shiftKey: true });
  button(root, "Project history")!.click();
  const history = root.querySelector("#cb-history")!;
  expect(history.querySelector("[role=option]")?.getAttribute("aria-setsize")).toBe("10000");
  expect(history.querySelectorAll("[role=option]").length).toBeLessThan(80);
  history.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
  expect(history.getAttribute("aria-activedescendant")).toBe("cb-history-9999");
  expect(history.querySelector("#cb-history-9999")?.textContent).toContain("Synthetic history entry 10000");
});

it("shows file and Git conflicts without enabling a real resolution", () => {
  const { root, store } = mount();
  store.selectScenario("conflict");
  key({ key: "G", code: "KeyG", ctrlKey: true, shiftKey: true });
  expect(root.textContent).toContain("Merge stopped: 1 file has conflicts on synthetic-layout.");
  expect(button(root, "Commit (mock)")!.disabled).toBe(true);
  button(root, "Open file")!.click();
  key({ key: "s", code: "KeyS", ctrlKey: true });
  expect(root.textContent).toContain("This file changed on disk since you opened it");
  button(root, "Compare")!.click();
  expect(root.querySelector("#cb-g2")!.textContent).toContain("Incoming (agent edit)");
  expect(root.querySelector("#cb-g1 textarea")?.getAttribute("aria-label")).toContain("synthetic/layout.ts");
  button(root, "Keep local (mock)")!.click();
  expect(store.state().conflictResolution).toBe("local");
  button(root, "Mark resolved (mock)")!.click();
  expect(root.textContent).toContain("marked resolved in memory only. No Git command ran.");
});

it("follows the two-area rule for the terminal", () => {
  const { root } = mount();
  key({ key: "`", code: "Backquote", ctrlKey: true });
  expect(root.querySelector("#cb-bottom")).not.toBeNull();
  key({ key: "M", code: "KeyM", ctrlKey: true, shiftKey: true });
  enter(root.querySelector('[data-need="mock-decision-question"]')!);
  expect(root.querySelector("#cb-bottom")).toBeNull();
  key({ key: "`", code: "Backquote", ctrlKey: true });
  expect(root.querySelector("#cb-g2 [role=tab][aria-selected=true]")?.textContent).toContain("Terminal");
  expect(root.querySelector("#cb-g1 textarea")).not.toBeNull();
});
