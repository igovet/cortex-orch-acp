// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, newMcpServer, type Sprint, type Task } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { Pill } from "../src/worker";
import { QueueButton, RunMode } from "../src/sprint";
import { toolCounts } from "../src/orch-mcp";
import { buildIndex } from "../src/selectors";
import { draftTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const src = join(__dirname, "..", "src");
const mount = (view: () => any, ws?: Ws) => {
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => ws ? <WsContext.Provider value={ws}>{view()}</WsContext.Provider> : view(), root);
  return root;
};

// Live run: the floating "2 new" and "Latest" pills lay over a question's options and a result's footer.
it("stands the follow pill in its own opaque band, so it never lies over the timeline's controls", () => {
  const jump = vi.fn();
  const root = mount(() => <Pill follow={{ following: () => false, missed: () => 2, jump } as any} />);
  const pill = root.querySelector<HTMLButtonElement>(".pill-band > .new-pill")!;
  expect(pill.textContent).toBe("2 new");
  pill.click(); expect(jump).toHaveBeenCalledOnce();
  const css = readFileSync(join(src, "workspace.css"), "utf8");
  // The band and the folded composer row take the timeline's own surface: content scrolls behind them, as behind the composer.
  expect(css).toMatch(/\.pill-band, \.mc-fold \{[^}]*background: var\(--bg-canvas\)/);
  expect(css).toMatch(/\.pill-band \{[^}]*display: flex/);
});

it("shows all three modes and dispatches the exact selected mode", () => {
  const run = vi.fn();
  const sprint = { id: "s", runMode: "hybrid" } as Sprint;
  const ws = { gate: () => undefined, needs: () => ({}), run } as unknown as Ws;
  const root = mount(() => <RunMode sprint={sprint} />, ws);
  const buttons = [...root.querySelectorAll("button")];
  expect(buttons.map(b => b.textContent)).toEqual(["Manual", "Hybrid", "Automatic"]);
  expect(buttons.map(b => b.getAttribute("aria-current"))).toEqual(["false", "true", "false"]);
  buttons[1].click();
  expect(run).not.toHaveBeenCalled();
  buttons[0].click();
  buttons[2].click();
  expect(run.mock.calls.map(([command]) => command)).toEqual([
    { type: "set_run_mode", sprintId: "s", runMode: "manual" },
    { type: "set_run_mode", sprintId: "s", runMode: "automatic" },
  ]);
});

// Live run: a disabled "Continue queue" on the board looked like an enabled one.
it("gives a blocked Continue queue the shared disabled look of a main action, and an open one its outlined look", () => {
  const state = emptyWorkspace(), machine = state.machines[0];
  machine.connection = "connected";
  state.projects = [{ id: "p", machineId: machine.id, name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main" }];
  const sprint: Sprint = { id: "s", projectId: "p", title: "Sprint", description: "Scope", runMode: "hybrid", planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: null, targetBranch: null };
  const ws = (task: Task) => {
    const held = { ...state, tasks: [task], sprints: [sprint] };
    return { store: { state: () => held, index: () => buildIndex(held, 1), clock: () => 1 }, gate: () => undefined, needs: () => ({}), run: vi.fn() } as unknown as Ws;
  };
  const blocked = mount(() => <QueueButton sprint={sprint} line small />, ws({ ...draftTask(), sprintId: "s", status: "needs_you", executionId: "e" } as Task));
  const go = [...blocked.querySelectorAll("button")].find(b => b.textContent === "Continue queue")!;
  expect(go.getAttribute("aria-disabled")).toBe("true");
  expect(go.classList.contains("btn-primary")).toBe(true);
  expect(go.classList.contains("btn-line")).toBe(false);
  dispose?.(); document.body.innerHTML = "";
  const open = mount(() => <QueueButton sprint={sprint} line small />, ws({ ...draftTask(), sprintId: "s" }));
  const run = [...open.querySelectorAll("button")].find(b => b.textContent === "Continue queue")!;
  expect(run.getAttribute("aria-disabled")).toBeNull();
  expect(run.classList.contains("btn-line")).toBe(true);
  const layout = readFileSync(join(src, "layout.css"), "utf8");
  expect(layout).toMatch(/:is\(\.btn-primary, \.btn-danger\):is\(\[aria-disabled="true"\], \[disabled\]\):not\(\[data-busy\]\) \{ background: var\(--bg-input\)/);
});

// Live run: "15 writing · 11 read-only · 8 coordinator" read as 34 tools, while the server had 15.
it("names the audience of each tool count so overlapping grants never read as one total", () => {
  const server = { ...newMcpServer(), allowedTools: Array.from({ length: 15 }, (_, i) => `t${i}`) };
  server.readOnlyTools = server.allowedTools.slice(0, 11);
  expect(toolCounts({ ...server, coordinatorTools: server.allowedTools.slice(0, 8) })).toBe("15 tools for writing workers, 11 for read-only roles, 8 for the coordinator");
  expect(toolCounts({ ...server, coordinatorTools: null })).toBe("15 tools for writing workers, 11 for read-only roles, 11 for the coordinator");
  expect(toolCounts({ ...server, allowedTools: ["one"], readOnlyTools: [], coordinatorTools: null })).toBe("1 tool for writing workers, 0 for read-only roles, 0 for the coordinator");
  expect(toolCounts({ ...server, allowedTools: [], readOnlyTools: [], coordinatorTools: null })).toBe("No tools granted");
});
