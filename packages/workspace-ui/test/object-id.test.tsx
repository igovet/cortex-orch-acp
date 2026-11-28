import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace, type PhoneFrame } from "../src";
import { draftTask, flush, projection, scriptedPort, withTask } from "./support";

const phone: PhoneFrame = { kind: "phone", machine: () => ({ name: "Desk", state: "connected", path: "direct", lastConnectedAt: 1 }), openMachines() {}, openSettings() {}, retry() {}, attach() {} };
let dispose: (() => void) | undefined;
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
  localStorage.clear();
  sessionStorage.clear();
  Reflect.deleteProperty(navigator, "clipboard");
});
async function settle() { for (let i = 0; i < 8; i++) await flush(); }
async function mount(path: string, mobile = false) {
  const s = withTask(emptyWorkspace(), "Visible title");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], { sprintId: "sp", executionId: "different-execution", status: "working", process: "live" });
  s.tasks.push({ ...draftTask(), id: "draft", sprintId: "sp" });
  s.sprints = [{ id: "sp", projectId: "p", title: "Sprint title", description: "", planning: "created", runMode: "manual", targetBranch: null,
    conversation: [], plan: null, materializedVersion: null, taskIds: ["t", "draft"], queueState: "paused", pauseReason: null }];
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/${path}`);
  const h = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const root = document.body.appendChild(document.createElement("div"));
  dispose = render(() => <Workspace port={h.port} shell={mobile ? phone : undefined} />, root);
  await settle();
  h.emit(projection("1", s));
  await settle();
}
const button = () => document.querySelector<HTMLButtonElement>(".ws-object-id button")!;

it.each([
  ["t/t", "task", "t", false], ["p/p/s/sp/board", "sprint", "sp", false],
  ["p/p/s/sp/summary", "sprint", "sp", true], ["p/p/board", "project", "p", true],
] as const)("copies the confirmed object on %s", async (path, kind, id, mobile) => {
  const writeText = vi.fn(async () => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  await mount(path, mobile);
  expect(button().getAttribute("aria-label")).toBe(`Copy ${kind} ID`);
  button().click();
  await settle();
  expect(writeText).toHaveBeenCalledExactlyOnceWith(id);
  expect(document.querySelector(".ws-object-id [aria-live]")?.textContent).toBe("ID copied");
  if (path.endsWith("summary")) {
    const entry = document.querySelector<HTMLAnchorElement>(".ws-revision-request a")!;
    expect(entry.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t`);
    entry.click();
    await settle();
    expect(button().getAttribute("aria-label")).toBe("Copy task ID");
    expect(document.querySelector(".ws-object-id [aria-live]")?.textContent).toBe("");
  }
});
it("selects the canonical ID for manual copy when the clipboard refuses", async () => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async () => { throw Error("denied"); } } });
  await mount("t/t");
  button().click();
  await settle();
  const field = document.querySelector<HTMLInputElement>(".ws-object-id-manual input")!;
  expect(field.value).toBe("t");
  expect(document.activeElement).toBe(field);
  expect([field.selectionStart, field.selectionEnd]).toEqual([0, 1]);
  expect(document.querySelector(".ws-object-id [aria-live]")?.textContent).toBe("");
});
