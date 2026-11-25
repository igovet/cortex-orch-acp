// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Task } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { hrefOf, type Route } from "../src/router";
import { buildIndex } from "../src/selectors";
import { taskActionsFor } from "../src/task-actions-model";
import { flush, projection, scriptedPort, withTask } from "./support";

const machine = LOCAL_MACHINE_ID;
const taskRoute = { name: "task", machine, task: "t" } as const;
const world = (patch: Partial<Task> = {}) => {
  const s = withTask(emptyWorkspace(), "Unified controls");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], patch);
  return s;
};

it.each([
  ["running", { status: "working", process: "live" }, "stop", true, undefined],
  ["paused", { status: "paused", process: "stopped" }, "resume", true, undefined],
  ["failed", { status: "failed", process: "stopped" }, "resume", false, /failed/i],
  ["waiting for an answer", { status: "needs_you", process: "live" }, "answer", true, undefined],
  ["done", { status: "done", process: "stopped" }, "resume", false, /closed/i],
  ["cancelled", { status: "cancelled", process: "stopped" }, "resume", false, /cancelled/i],
  ["draft", { status: "draft", process: "stopped" }, "start", true, undefined],
  ["waiting for stop evidence", { status: "paused", process: "stopped", warnings: ["process_exit_unconfirmed"] }, "resume", false, /confirm/i],
] satisfies [string, Partial<Task>, string, boolean, RegExp | undefined][])(
  "describes %s from current task facts without dispatching",
  (_state, patch, id, enabled, why) => {
    const s = world(patch);
    const index = buildIndex(s, 0), run = vi.fn(), noop = vi.fn();
    const actions = taskActionsFor(index.byId.get("t")!, {
      store: { index: () => index, state: () => s, clock: () => 0, pendingTask: () => false },
      router: { route: () => taskRoute },
      panels: { pins: () => [], togglePin: noop },
      run, go: noop, select: noop, inspectable: () => true, gate: () => undefined, can: () => true,
      t: key => key, open: noop, stopTask: noop, closeTask: noop, archiveTask: noop, returnForRework: noop,
      noOpener: () => undefined, externalLabel: () => "Open in editor",
    });
    const action = actions.find(a => a.id === id);
    expect(action).toMatchObject({ id, enabled });
    if (why) expect(action?.why).toMatch(why);
    else expect(action?.why).toBeUndefined();
    if (!enabled) action?.run();
    expect(run).not.toHaveBeenCalled();
    expect(noop).not.toHaveBeenCalled();
  },
);

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const get = <T extends HTMLElement = HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const entries = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)].map(el => [el.dataset.taskAction, el.dataset.why]);
const menu = () => entries("#m-task [data-task-action]");
const key = (el: EventTarget, name: string, init: KeyboardEventInit = {}) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init }));

async function mount(route: Route, state = world({ status: "needs_you", process: "live" })) {
  dispose?.();
  document.body.innerHTML = "<div id='fixture'></div>";
  history.replaceState(null, "", hrefOf(route));
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  dispose = render(() => <Workspace port={p.port} />, get("#fixture"));
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}

it("uses the same action ids and disabled reasons in list, board, Home, sidebar, task and palette", async () => {
  await mount(taskRoute);
  get<HTMLButtonElement>('button[aria-label="Task actions"]').click();
  const expected = menu();
  expect(expected.map(([id]) => id)).toEqual(expect.arrayContaining(["answer", "stop", "resume", "archive", "pin", "open_editor", "copy_branch", "diagnostics"]));
  expect(expected.find(([id]) => id === "resume")?.[1]).toBe("This task is not paused");
  key(document, "k", { ctrlKey: true });
  await settle();
  expect(entries('[role="option"][data-task-action]')).toEqual(expected);
  const project = { name: "project", machine, project: "p", scope: { kind: "all" } } as const;
  for (const [route, selector] of [
    [{ ...project, view: "list" }, ".lrow-shell"],
    [{ ...project, view: "board" }, ".card"],
    [{ name: "home", machine }, ".need-shell"],
    [taskRoute, ".task-row-actions"],
  ] satisfies [Route, string][]) {
    await mount(route);
    get<HTMLButtonElement>(`${selector} button[aria-label="Actions for Unified controls"]`).click();
    expect(menu(), selector).toEqual(expected);
  }
});

it("opens the same menu at the pointer or with a context key without opening the row", async () => {
  const project = { name: "project", machine, project: "p", view: "list", scope: { kind: "all" } } as const;
  const p = await mount(project, world());
  const link = get(".lrow-shell a");
  const rightClick = new MouseEvent("contextmenu", { button: 2, bubbles: true, cancelable: true, clientX: 200, clientY: 140 });
  const path = location.href;
  expect(link.dispatchEvent(rightClick)).toBe(false);
  expect(get("#m-task").style.left).toBe("200px");
  expect(get("#m-task").style.top).toBe("140px");
  const entries = menu();
  expect(entries.map(([id]) => id)).toContain("start");
  expect(location.href).toBe(path);
  expect(p.commands).toHaveLength(0);
  expect(key(link, "F10", { shiftKey: true })).toBe(false);
  expect(menu()).toEqual(entries);
  expect(key(link, "ContextMenu")).toBe(false);
  expect(menu()).toEqual(entries);
  get<HTMLButtonElement>('#m-task [data-task-action="archive"]').click();
  expect(document.querySelector("dialog h2")?.textContent).toBe("Archive unfinished task?");
  expect(p.commands).toHaveLength(0);
});
