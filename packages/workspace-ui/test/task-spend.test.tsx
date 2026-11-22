// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, type Task, type TaskSpend } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { QuestionNotifications, stallHours } from "../src/notifications";
import { SpendLine, spendLine, spendRows, StallBadge, stalledHours } from "../src/task-spend";
import { draftTask, flush } from "./support";

const H = 3_600_000;
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; stallHours()[1](3); });
const spend = (over: Partial<TaskSpend> = {}): TaskSpend => ({ workingMs: 3 * H + 12 * 60_000, workingSince: 0, lastProgressAt: null, stalledSince: 0,
  sessions: 2, turns: 42, reportedTurns: 0, tokens: null, lastFailures: ["review asked for changes", "verification failed"],
  engines: [{ engine: "claude-agent-acp", model: "opus", modelLabel: "Opus", sessions: 1, turns: 30, reportedTurns: 0, tokens: null },
    { engine: "codex-acp", model: "gpt-5", sessions: 1, turns: 12, reportedTurns: 0, tokens: null }], ...over });
const working = (s: TaskSpend): Task => ({ ...draftTask(), status: "working", spend: s });

it("shows spend from what was recorded and says tokens were not reported instead of zero", () => {
  const rows = Object.fromEntries(spendRows(spend()));
  expect(rows["Time working"]).toBe("3 h 12 min");
  expect(rows["Turns"]).toBe("42");
  expect(rows["Tokens"]).toBe("Not reported");
  expect(rows["Claude · Opus"]).toBe("1 session, 30 turns, tokens not reported");
  expect(rows["Last attempts hit"]).toBe("review asked for changes; verification failed");
  const partial = Object.fromEntries(spendRows(spend({ tokens: 1_250_000, reportedTurns: 30 })));
  expect(partial["Tokens"]).toBe("1.3M, reported for 30 of 42 turns");
  expect(spendLine(spend())).toBe("3 h 12 min working · 2 sessions · 42 turns · tokens not reported");
  expect(spendRows(undefined)).toEqual([]);
  expect(spendLine({ ...spend(), workingMs: 0, sessions: 0, turns: 0 })).toBeNull();
});

it("counts a working task as stuck only past the owner's threshold, never a paused one or with the setting off", () => {
  expect(stalledHours(working(spend()), 3 * H - 1, 3)).toBeNull();
  expect(stalledHours(working(spend()), 4 * H, 3)).toBe(4);
  expect(stalledHours(working(spend()), 4 * H, 6)).toBeNull();
  expect(stalledHours(working(spend()), 9 * H, 0)).toBeNull();
  expect(stalledHours({ ...working(spend()), status: "paused" }, 9 * H, 3)).toBeNull();
  expect(stalledHours(working(spend({ stalledSince: null })), 9 * H, 3)).toBeNull();
});

it("renders the no-progress badge and the spend line", async () => {
  const [now, setNow] = createSignal(2 * H);
  const [task] = createSignal(working(spend()));
  const ws = { store: { clock: now } } as unknown as Ws;
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <WsContext.Provider value={ws}><StallBadge task={task()} /><SpendLine task={task()} /></WsContext.Provider>, root);
  expect(root.querySelector(".ws-stall")).toBeNull();
  expect(root.querySelector(".ws-spend")?.textContent).toBe("Spent 3 h 12 min working · 2 sessions · 42 turns · tokens not reported");
  dispose(); root.innerHTML = "";
  setNow(3 * H + 60_000);
  dispose = render(() => <WsContext.Provider value={ws}><StallBadge task={task()} /></WsContext.Provider>, root);
  await flush();
  expect(root.querySelector(".ws-stall")?.textContent).toBe("No progress for 3 h");
  expect(root.textContent).not.toContain("Cortex");
});

it("reports the no-progress kind and threshold to the service, and leaves it out when off", async () => {
  const initial = emptyWorkspace();
  const [state] = createSignal(initial);
  const report = vi.fn(async (_report: unknown) => ({ raising: true }));
  const ws = { store: { mode: "real", state }, router: { route: () => ({ name: "home" }) }, panels: { notifications: () => true }, machine: () => initial.machines[0] } as unknown as Ws;
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <WsContext.Provider value={ws}><QuestionNotifications service={{ report, take: async () => ({}) }} /></WsContext.Provider>, root);
  await flush();
  expect(report).toHaveBeenLastCalledWith(expect.objectContaining({ kinds: expect.arrayContaining(["stalled"]), stallHours: 3 }));
  stallHours()[1](0);
  await flush();
  const last = report.mock.calls.at(-1)![0] as { kinds: string[]; stallHours?: number };
  expect(last.kinds).not.toContain("stalled");
  expect(last.stallHours).toBeUndefined();
});
