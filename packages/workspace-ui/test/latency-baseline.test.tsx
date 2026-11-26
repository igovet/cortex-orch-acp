import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptyConfig, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, scriptedTransport, snapshot, wireTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => {
  vi.useFakeTimers({ now: new Date("2026-11-23T12:00:00Z"),
    toFake: ["Date", "performance", "setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"] });
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  localStorage.clear();
  sessionStorage.clear();
});
afterEach(() => {
  dispose?.();
  dispose = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const tick = (ms = 0) => vi.advanceTimersByTimeAsync(ms);
const now = () => performance.now();
const after = <T,>(ms: number, value: T) => new Promise<T>(resolve => setTimeout(() => resolve(value), ms));
const column = () => document.querySelector('.card[data-task="task-1"]')?.closest(".col")?.getAttribute("data-col");
const pending = () => document.querySelector('.card[data-task="task-1"] .ws-request-status')?.textContent;
const expectPlaced = () => {
  expect(column()).toBe("work");
  const cards = document.querySelectorAll('.card[data-task="task-1"]');
  expect(cards).toHaveLength(1);
  expect((cards[0] as HTMLElement).style.visibility).not.toBe("hidden");
};
type Trace = Record<string, number>;
function trace() {
  const origin = now(), points: Trace = {};
  const mark = (name: string) => { points[name] = now() - origin; };
  // Two consecutive scheduled frame samples of the same predicate, not browser paints.
  const sample = (name: string, check: () => void, confirm = false) => {
    requestAnimationFrame(() => {
      check();
      mark(name);
      if (confirm) requestAnimationFrame(() => { check(); mark("stableFrame"); });
    });
  };
  return { points, mark, sample };
}

async function board(native = false) {
  let running = false, snapshotDelay = 0, version = 1;
  const readStarts: number[] = [], readReplies: number[] = [], rpcReplies: number[] = [];
  const task = () => wireTask({ version: String(version), ...(running ? {
    executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING",
  } : {}) });
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call", ...(native ? ["native.board", "native.query"] : [])], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Fixture", canonicalRoot: "/fixture", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.start"]),
    "project.snapshot": async () => {
      readStarts.push(now());
      const reply = snapshot(String(version + 2), [task()]);
      if (snapshotDelay) await after(snapshotDelay, null);
      readReplies.push(now());
      return reply;
    },
    "native.board": () => {
      readStarts.push(now());
      return { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(version + 2),
        data: { type: "board", value: { items: [task()], nextCursor: null, highWatermark: String(version + 2) } } };
    },
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
    "tasks.start": async raw => {
      const { requestId } = raw as { requestId: string };
      const reply = await after(120, { requestId, outcome: "recorded", target: "task-1", entityVersion: "2", recordedAt: "2026-11-23T12:00:00Z" });
      rpcReplies.push(now());
      return reply;
    },
    "machine.call": raw => {
      const { op } = raw as { op: string };
      if (op === "overview") return { ok: true, result: { name: "Fixture" } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "repo.snapshot") return { ok: true, result: null };
      if (op === "workers") return { ok: true, result: [] };
      return { ok: true, result: { tasks: {}, sprints: [], messages: [] } };
    },
  });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p`);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  await tick(64);
  expect(column()).toBe("todo");
  readStarts.length = readReplies.length = 0;
  const commit = () => { running = true; version++; };
  const invalidate = () => s.send({ type: "invalidated", projectId: "p" });
  return { s, commit, invalidate, readStarts, readReplies, rpcReplies, delay: (ms: number) => { snapshotDelay = ms; },
    delta: () => s.send({ type: "native_delta", projectId: "p", batch: { generation: "g",
      streamKey: { scope: "project", project_id: "p" }, fromSequence: "4", throughSequence: "4",
      changes: [{ kind: "task", id: "task-1", baseVersion: "1", newVersion: "2",
        operation: { type: "upsert", data: { kind: "task_projection", data: task() } } }] } }) };
}

it("traces an owner Start from input and pending DOM through receipt and confirmed placement", async () => {
  const b = await board();
  const t = trace(), origin = now();
  b.delay(40);
  const start = [...document.querySelectorAll<HTMLButtonElement>('.card[data-task="task-1"] button')].find(button => button.textContent === "Start");
  expect(start).toBeDefined();
  start!.click();
  expect(pending()).toBe("Requesting start…");
  t.mark("pendingDOM");
  t.sample("pendingFrame", () => { expect(pending()).toBe("Requesting start…"); expect(column()).toBe("todo"); });
  await tick(119);
  expect(b.rpcReplies).toEqual([]);
  expect(column()).toBe("todo");
  b.commit();
  await tick(1);
  expect(b.rpcReplies.map(at => at - origin)).toEqual([120]);
  expect(pending()).toBe("");
  t.mark("clearedDOM");
  expect(column()).toBe("todo"); // A receipt is not the confirmed projection.
  await tick(39);
  expect(column()).toBe("todo");
  await tick(1);
  expectPlaced();
  t.mark("installedDOM");
  t.sample("confirmedFrame", expectPlaced, true);
  await tick(32);
  expect(b.s.calls.filter(call => call.operation === "tasks.start")).toHaveLength(1);
  expect(t.points).toEqual({ pendingDOM: 0, pendingFrame: 16, clearedDOM: 120, installedDOM: 160, confirmedFrame: 176, stableFrame: 192 });
  expect(b.readStarts.map(at => at - origin)).toEqual([120]);
  expect(b.readReplies.map(at => at - origin)).toEqual([160]);
  console.info("latency fixture (virtual ms), Start", { ...t.points, rpcReply: 120, readStart: b.readStarts[0] - origin, readReply: b.readReplies[0] - origin });
});

it("traces a leading and a trailing project invalidation and holds the old card until the read", async () => {
  const b = await board();
  const leading = now();
  b.delay(40);
  b.invalidate();
  await tick();
  expect(b.readStarts).toEqual([leading]);
  await tick(100);
  const t = trace(), origin = now();
  b.commit();
  b.invalidate();
  // The trailing read is due one foreground gap (250 ms) after the leading one began: 150 ms from here. One millisecond
  // before that, it still has not started.
  await tick(149);
  expect(b.readStarts).toEqual([leading]);
  expect(column()).toBe("todo");
  await tick(1);
  expect(b.readStarts).toEqual([leading, origin + 150]);
  await tick(39);
  expect(column()).toBe("todo");
  await tick(1);
  expectPlaced();
  t.mark("installedDOM");
  t.sample("confirmedFrame", expectPlaced, true);
  await tick(32);
  expect(t.points.installedDOM).toBe(190);
  expect(t.points.installedDOM).toBeLessThanOrEqual(250);
  expect(t.points).toEqual({ installedDOM: 190, confirmedFrame: 204, stableFrame: 220 });
  console.info("latency fixture (virtual ms), invalidation", { leadingReadStart: 0, leadingReadReply: b.readReplies[0] - leading,
    readStart: b.readStarts[1] - origin, readReply: b.readReplies[1] - origin, ...t.points });
});

it("traces a contiguous native delta inside the same foreground coalescing window", async () => {
  const b = await board(true);
  b.invalidate(); // Establish the generic refresh window before the native event.
  await tick(100);
  expect(b.readStarts).toHaveLength(1);
  expect(column()).toBe("todo");
  const t = trace();
  b.commit();
  b.delta();
  await tick();
  expectPlaced();
  t.mark("installedDOM");
  t.sample("confirmedFrame", expectPlaced, true);
  await tick(32);
  expect(b.readStarts).toHaveLength(1); // The delta did not need another native board read.
  expect(t.points).toEqual({ installedDOM: 0, confirmedFrame: 12, stableFrame: 28 });
  console.info("latency fixture (virtual ms), delta", t.points);
});
