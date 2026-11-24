import { render } from "solid-js/web";
import { createRoot } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Decision as WireDecision } from "@cortex/workspace-contract";
import { emptyConfig, LOCAL_MACHINE_ID, type NativeTaskDelta, type TimelineEntry } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { STEP_PREFIX } from "../src/tool-names";
import { conversationOf } from "../src/port/history";
import { firstWindowEntries } from "../src/first-view";
import { capabilitiesWire, flush, scriptedTransport, wireDecision, wireTask } from "./support";

/**
 * A native service whose history reads behave like the core's: `history.page` pages forward and names a forward cursor;
 * `history.earlier` reads from the end backwards (`prevCursor` for the rows before, `tailCursor` on the newest page only).
 * `backward: false` is a service that does not know `history.earlier` and refuses it as the core's decoder does.
 * As in the core (`projections/history.rs`, `bound`), every cursor it hands out is bound to the page size it was read with
 * (`np1.<limit>.<row>`, `nt1.<limit>.<row>`, `nh.<limit>.<top>.<consumed>`), and a read that names a cursor with another
 * page size is refused with `invalid_cursor`.
 */
function service(options: { backward?: boolean; earlierDelay?: number; decisions?: WireDecision[] } = {}) {
  let version = 1, high = 3, failNext = false, resetNext = false, repeatNext = false, heldReply: Promise<void> | undefined, heldForward: Promise<void> | undefined;
  const task = () => wireTask({ version: String(version), description: "Index the repository", executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
  const rows: Record<string, unknown>[] = [];
  const at = (n: number) => new Date(Date.UTC(2026, 10, 17, 10) + n * 1000).toISOString();
  const add = (kind: string, text: string, toolStatus: string | null = null) => rows.push({ historyId: `h${rows.length}`, sequence: String(rows.length + 1), revision: "1", kind, text, toolStatus,
    recordedAt: at(rows.length), availability: "available" });
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(high), data });
  const reads: { operation: string; cursor: string | null }[] = [];
  /** The row count each backward read asked for, in order. */
  const limits: number[] = [];
  /** The row count each forward read asked for, in order. */
  const forwardLimits: number[] = [];
  /** Reads refused because their cursor was bound to another page size. */
  const refused: { operation: string; cursor: string; limit: number }[] = [];
  /** The fields of a cursor after its kind and page size; refused when it was read with another page size. */
  const position = (operation: string, cursor: string, limit: number) => {
    const [, bound, ...rest] = cursor.split(".");
    if (bound !== String(limit)) { refused.push({ operation, cursor, limit }); throw new Error("Refused: invalid_cursor"); }
    return rest.map(Number);
  };
  const forward = (p: { cursor: string | null; limit: number }) => {
    const last = rows.length - 1;
    let after = -1, top = last;
    if (p.cursor?.startsWith("nt1.")) [after] = position("history.page", p.cursor, p.limit);
    else if (p.cursor) [top, after] = position("history.page", p.cursor, p.limit);
    const items = rows.slice(after + 1, top + 1).slice(0, p.limit);
    const consumed = after + items.length, next = consumed < top ? `nh.${p.limit}.${top}.${consumed}` : null;
    return { items, nextCursor: next, highWatermark: String(top + 1), tailCursor: `nt1.${p.limit}.${next ? consumed : top}` };
  };
  const earlier = (p: { cursor: string | null; limit: number }) => {
    const before = p.cursor ? position("history.earlier", p.cursor, p.limit)[0] : rows.length, from = Math.max(0, before - p.limit);
    const repeat = repeatNext && p.cursor !== null;
    if (repeat) repeatNext = false;
    return { items: rows.slice(from, before).filter(row => row.kind !== "server_filtered"), nextCursor: null, highWatermark: String(rows.length), ...(from > 0 ? { prevCursor: repeat ? p.cursor : `np1.${p.limit}.${from}` } : {}), ...(p.cursor ? {} : { tailCursor: `nt1.${p.limit}.${rows.length - 1}` }) };
  };
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"]),
    "native.board": () => envelope({ type: "board", value: { items: [task()], nextCursor: null, highWatermark: String(high) } }),
    "native.query": async raw => {
      const { operation, payload } = raw as { operation: string; payload: { cursor: string | null; limit: number } };
      reads.push({ operation, cursor: payload.cursor });
      if (operation === "history.earlier") limits.push(payload.limit);
      if (operation === "history.page") {
        forwardLimits.push(payload.limit);
        if (payload.cursor && heldForward) { const reply = heldForward; heldForward = undefined; await reply; }
        return envelope({ type: "history", value: forward(payload) });
      }
      if (options.backward === false) throw new Error("Refused: invalid_request");
      if (payload.cursor && resetNext) { resetNext = false; throw new Error("Refused: history_reset"); }
      if (payload.cursor && heldReply) { const reply = heldReply; heldReply = undefined; await reply; }
      if (payload.cursor && options.earlierDelay) await new Promise(resolve => setTimeout(resolve, options.earlierDelay));
      if (payload.cursor && failNext) { failNext = false; throw new Error("Refused: storage_unavailable"); }
      return envelope({ type: "history", value: earlier(payload) });
    },
    "decisions.list": () => ({ items: options.decisions ?? [], nextCursor: null, highWatermark: "1" }),
    "machine.call": raw => {
      const op = (raw as { op: string }).op;
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      return { ok: true, result: op === "repo.snapshot" ? null : op === "workers" ? [] : { tasks: {}, sprints: [] } };
    },
  });
  /** A row was written: the task's version moves, as the service's stream says. */
  const changed = () => {
    const base = version, from = high + 1;
    version++; high++;
    const delta: NativeTaskDelta = { projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" }, fromSequence: String(from), throughSequence: String(high),
      changes: [{ kind: "task", id: "task-1", baseVersion: String(base), newVersion: String(version), operation: { type: "upsert", data: { kind: "task_projection", data: task() } } }] } };
    s.send({ type: "native_delta", ...delta });
  };
  const holdNext = () => {
    let release!: () => void;
    heldReply = new Promise<void>(resolve => { release = resolve; });
    return release;
  };
  const holdForward = () => {
    let release!: () => void;
    heldForward = new Promise<void>(resolve => { release = resolve; });
    return release;
  };
  return { s, rows, add, at, changed, reads, limits, forwardLimits, refused, holdNext, holdForward, fail: () => { failNext = true; }, reset: () => { resetNext = true; },
    /** The next earlier read with a cursor names that same cursor as the one before it. */
    repeat: () => { repeatNext = true; } };
}
const step = (call: string, status: string) => STEP_PREFIX + JSON.stringify({ title: "Read a.ts", callId: call, kind: "read", path: `a${call}.ts`, status });
/** `n` rows of a long task: an owner message every 50 rows, between them the coordinator's message and a tool call in turn. */
function long(svc: ReturnType<typeof service>, n: number) {
  for (let i = 0; i < n; i++) {
    if (i % 50 === 0) svc.add("user", `Message ${i}`);
    else if (i % 2 === 1) svc.add("assistant", `Reply ${i}.`);
    else svc.add("tool", step(`c${i}`, "completed"), "completed");
  }
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.unstubAllGlobals(); vi.restoreAllMocks(); });
/** These cases are about wire page boundaries: a screen of 100 entries makes the first window one whole wire page of
 *  100 rows, as each case lays out its rows. Without it, the first window is the window's own height (`firstWindowEntries`). */
const PAGE_SCREEN = () => 100;
function open(s: ReturnType<typeof service>["s"], firstWindow?: () => number) {
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true, firstWindow })); });
  const session = () => store.state().sessions.find(x => x.taskId === "task-1");
  const timeline = () => session()?.timeline ?? [];
  const texts = () => timeline().flatMap((e: TimelineEntry) => e.kind === "cortex" || e.kind === "said" ? [e.text] : []);
  return { store, session, timeline, texts };
}

it("opens a 22,000-row conversation on its latest part in a bounded number of reads, and reads only new rows after it", async () => {
  const svc = service();
  long(svc, 22_000);
  const { session, texts } = open(svc.s, PAGE_SCREEN);
  await vi.waitFor(() => expect(texts().at(-1)).toBe("Reply 21999."), { timeout: 4000 });
  // At most four pages, every one of them from the end backwards: never the 220 pages of a read from the beginning.
  expect(svc.reads.length).toBeLessThanOrEqual(4);
  expect(svc.reads.every(r => r.operation === "history.earlier")).toBe(true);
  expect(session()!.historyEarlier).toEqual({ state: "available", since: Date.parse(svc.at(22_000 - svc.reads.length * 100)) });
  // A new row arrives through the forward cursor and is appended, nothing read again.
  const before = svc.reads.length;
  svc.add("tool", step("late", "completed"), "completed"); svc.add("assistant", "Latest words."); svc.changed();
  await vi.waitFor(() => expect(texts().at(-1)).toBe("Latest words."));
  expect(svc.reads.slice(before).every(r => r.operation === "history.page" && r.cursor?.startsWith("nt1."))).toBe(true);
  expect(session()!.historyEarlier?.state).toBe("available");
});

it("opens a dense conversation on about one screen of its newest entries, then reads the rest in order as it is scrolled up", async () => {
  const svc = service();
  // Every row is an owner message of its own: a page of rows is as many entries.
  const all = Array.from({ length: 300 }, (_, i) => `Owner message ${i}`);
  for (const text of all) svc.add("user", text);
  const { store, session, timeline, texts } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"), { timeout: 4000 });
  const screen = firstWindowEntries();
  // One read, asking for one screen of rows: never a whole wire page of a hundred.
  expect(svc.limits).toEqual([screen]);
  expect(screen).toBeLessThan(100);
  expect(texts()).toEqual(all.slice(-screen));
  // The service double holds the core's binding: the held cursor read with another page size is refused.
  const held = svc.reads.length;
  const cursor = `np1.${screen}.${all.length - screen}`;
  await expect(svc.s.transport.request("native.query", { projectId: "p", operation: "history.earlier", payload: { target: "task-1", cursor, limit: 100 } })).rejects.toThrow("invalid_cursor");
  expect(svc.refused).toEqual([{ operation: "history.earlier", cursor, limit: 100 }]);
  svc.refused.length = 0; svc.reads.length = held; svc.limits.length = 1;
  for (let i = 0; i < Math.ceil(all.length / screen) + 2 && session()?.historyEarlier; i++) {
    const n = timeline().length;
    store.loadEarlierHistory("task-1");
    await vi.waitFor(() => expect(timeline().length > n || !session()?.historyEarlier).toBe(true));
    expect(session()?.historyEarlier?.state).not.toBe("failed");
  }
  // Read to the beginning: every entry once, in order, nothing lost between the parts.
  expect(session()!.historyEarlier).toBeUndefined();
  expect(texts()).toEqual(all);
  const ids = timeline().map(e => e.id);
  expect(new Set(ids).size).toBe(ids.length);
  // Every page of the chain asked for the first window's page size, and none was refused or read afresh.
  expect(svc.limits.every(limit => limit === screen)).toBe(true);
  expect(svc.limits.length).toBe(Math.ceil(all.length / screen));
  expect(svc.refused).toEqual([]);
  expect(svc.reads.filter(r => r.operation === "history.earlier" && r.cursor === null)).toHaveLength(1);
});

it("opens a tool-heavy conversation whose first window takes several pages, all of one page size, and reads it to its beginning", async () => {
  const svc = service();
  // Nine tool steps fold into one entry before each reply: a page of one screen's rows makes far fewer entries than a screen.
  for (let i = 0; i < 400; i++) {
    if (i % 10 === 9) svc.add("assistant", `Reply ${i}.`);
    else svc.add("tool", step(`c${i}`, "completed"), "completed");
  }
  const { store, session, timeline, texts } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"), { timeout: 4000 });
  const screen = firstWindowEntries();
  expect(texts().at(-1)).toBe("Reply 399.");
  // The first window needed more than one page, each of them one screen's rows, within the 150-row budget.
  expect(svc.limits.length).toBeGreaterThanOrEqual(2);
  expect(svc.limits.every(limit => limit === screen)).toBe(true);
  expect(svc.limits.length * screen).toBeLessThan(150 + screen);
  expect(svc.refused).toEqual([]);
  for (let i = 0; i < Math.ceil(400 / screen) + 2 && session()?.historyEarlier; i++) {
    const n = timeline().length;
    store.loadEarlierHistory("task-1");
    await vi.waitFor(() => expect(timeline().length > n || !session()?.historyEarlier).toBe(true));
    expect(session()?.historyEarlier?.state).not.toBe("failed");
  }
  expect(session()!.historyEarlier).toBeUndefined();
  expect(svc.limits.every(limit => limit === screen)).toBe(true);
  expect(svc.refused).toEqual([]);
  // Read to the beginning, it reads exactly as a whole read from the beginning does: no gap, nothing twice.
  const whole = service({ backward: false });
  for (let i = 0; i < 400; i++) {
    if (i % 10 === 9) whole.add("assistant", `Reply ${i}.`);
    else whole.add("tool", step(`c${i}`, "completed"), "completed");
  }
  const shown = timeline().map(e => e.kind), said = texts();
  const reference = open(whole.s);
  await vi.waitFor(() => expect(reference.texts().at(-1)).toBe("Reply 399."), { timeout: 4000 });
  expect(said).toEqual(reference.texts());
  expect(shown).toEqual(reference.timeline().map(e => e.kind));
});

it("continues the first window forward with its own page size, page after page, and never rereads it", async () => {
  const svc = service();
  for (let i = 0; i < 300; i++) svc.add("user", `Owner message ${i}`);
  const { session, texts } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"), { timeout: 4000 });
  const screen = firstWindowEntries();
  expect(svc.limits).toEqual([screen]);
  // More new rows than one page: the forward read continues through the service's next cursors, all bound to that size.
  const added = screen * 2 + 3;
  for (let i = 0; i < added; i++) svc.add("user", `New message ${i}`);
  svc.changed();
  await vi.waitFor(() => expect(texts().at(-1)).toBe(`New message ${added - 1}`), { timeout: 4000 });
  const forwards = svc.reads.filter(r => r.operation === "history.page");
  expect(forwards.length).toBe(Math.ceil(added / screen));
  expect(forwards[0]!.cursor).toBe(`nt1.${screen}.299`);
  expect(forwards.slice(1).every(r => r.cursor?.startsWith(`nh.${screen}.`))).toBe(true);
  expect(svc.forwardLimits.every(limit => limit === screen)).toBe(true);
  expect(svc.refused).toEqual([]);
  // Only the new rows were read: the first window was never read again, and nothing shows twice.
  expect(svc.reads.filter(r => r.operation === "history.earlier")).toHaveLength(1);
  expect(texts()).toEqual([...Array.from({ length: screen }, (_, i) => `Owner message ${300 - screen + i}`), ...Array.from({ length: added }, (_, i) => `New message ${i}`)]);
  // A later change continues from the newest forward cursor, again with the same page size.
  svc.add("user", "Last message"); svc.changed();
  await vi.waitFor(() => expect(texts().at(-1)).toBe("Last message"));
  expect(svc.reads.at(-1)!.cursor).toBe(`nt1.${screen}.${300 + added - 1}`);
  expect(svc.forwardLimits.every(limit => limit === screen)).toBe(true);
  expect(svc.refused).toEqual([]);
});

it("reads a 20,050-entry conversation to the end through forward history pages", async () => {
  const svc = service({ backward: false });
  // Each row is a separate owner message so the normal transcript folding does not hide lost rows.
  const expected = Array.from({ length: 20_050 }, (_, i) => `Message ${i + 1}`);
  for (const text of expected) svc.add("user", text);

  const { session, timeline } = open(svc.s);
  const entries = () => timeline().filter(e => e.id.startsWith("history:"));
  // The port publishes the collected forward read as a unit. Wait for publication, not for the expected count.
  await vi.waitFor(() => expect(entries().length).toBeGreaterThan(0), { timeout: 4000 });

  expect(entries()).toHaveLength(expected.length);
  expect(entries().at(-1)).toMatchObject({ kind: "said", text: "Message 20050" });
  expect(entries().map(e => e.kind === "said" ? e.text : null)).toEqual(expected);
  expect(session()!.historyEarlier).toBeUndefined();
  expect(svc.reads[0]).toEqual({ operation: "history.earlier", cursor: null });
  expect(svc.reads[1]).toEqual({ operation: "history.page", cursor: null });
  expect(svc.reads.slice(2).length).toBeGreaterThan(0);
  expect(svc.reads.slice(2).every(r => r.operation === "history.page" && r.cursor !== null)).toBe(true);
});

it("puts each earlier part before the rows held, keeps every entry already shown, and stops at the beginning", async () => {
  const svc = service();
  long(svc, 700);
  const { store, session, timeline, texts } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"), { timeout: 4000 });
  const shown = timeline().map(e => e.id);
  store.loadEarlierHistory("task-1");
  await vi.waitFor(() => expect(timeline().length).toBeGreaterThan(shown.length));
  // What was shown stays, in order and under the same identities, after what was read before it.
  const now = timeline().map(e => e.id);
  expect(now.slice(now.length - shown.length)).toEqual(shown);
  // New rows still append while earlier ones are read.
  svc.add("tool", step("late", "completed"), "completed"); svc.add("assistant", "Streaming on."); svc.changed();
  await vi.waitFor(() => expect(texts().at(-1)).toBe("Streaming on."));
  // Each press reads at least one page of the first window's size (one screen's rows), so reaching the beginning of 700
  // rows takes as many presses as such pages.
  for (let i = 0; i < Math.ceil(700 / firstWindowEntries()) + 2 && session()?.historyEarlier; i++) {
    const n = timeline().length;
    store.loadEarlierHistory("task-1");
    await vi.waitFor(() => expect(timeline().length > n || !session()?.historyEarlier).toBe(true));
  }
  // The beginning is reached: the conversation is no longer partial, and reads exactly as a whole read of it does.
  expect(session()!.historyEarlier).toBeUndefined();
  const whole = service({ backward: false });
  long(whole, 700); whole.add("tool", step("late", "completed"), "completed"); whole.add("assistant", "Streaming on.");
  const reference = open(whole.s);
  await vi.waitFor(() => expect(reference.texts().at(-1)).toBe("Streaming on."), { timeout: 4000 });
  expect(texts()).toEqual(reference.texts());
});

it("never shows a message whose beginning lies in the part not read, nor a settled question or answer from it", async () => {
  // 250 rows: the newest two pages (rows 50 to 249) begin in the middle of a streamed message (rows 45 to 99). The owner
  // answered "main" at row 10, before them, and later wrote "main" again as a message of their own (row 180).
  const time = (n: number) => new Date(Date.UTC(2026, 10, 17, 10) + n * 1000).toISOString();
  const answered = wireDecision({ decisionId: "old", executionId: "execution", prompt: "Which branch?", status: "answered", answerText: "main",
    createdAt: time(5), answeredAt: time(10), allowedOperations: [] } as Partial<WireDecision>);
  const svc = service({ decisions: [answered] });
  for (let i = 0; i < 10; i++) svc.add("assistant", `early-${i} `);
  svc.add("user", "main");
  for (let i = 11; i < 45; i++) svc.add("tool", step(`s${i}`, "completed"), "completed");
  for (let i = 45; i < 100; i++) svc.add("assistant", `cut-${i} `);
  for (let i = 100; i < 120; i++) svc.add("tool", step(`t${i}`, "completed"), "completed");
  for (let i = 120; i < 180; i++) svc.add("assistant", `whole-${i} `);
  svc.add("user", "main");
  for (let i = 181; i < 250; i++) svc.add("tool", step(`u${i}`, "completed"), "completed");
  const { store, session, timeline } = open(svc.s, PAGE_SCREEN);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"), { timeout: 4000 });
  expect(session()!.historyEarlier!.since).toBe(Date.parse(time(50)));
  // The message cut at its start is left out; the turn of steps after it and the next message are whole.
  expect(timeline().map(e => e.kind)).toEqual(["fold", "cortex", "said", "fold"]);
  expect((timeline()[1] as { text: string }).text.startsWith("whole-120")).toBe(true);
  // The answer given before the part read does not hide the later message of the same words; its settled question is not
  // put at the top of these rows.
  expect((timeline()[2] as { text: string }).text).toBe("main");
  expect(timeline().some(e => e.kind === "decision")).toBe(false);
  // Read to the beginning, the message reads whole, the answer reads with its question and the question is in its place.
  for (let i = 0; i < 3 && session()?.historyEarlier; i++) {
    const n = timeline().length;
    store.loadEarlierHistory("task-1");
    await vi.waitFor(() => expect(timeline().length > n || !session()?.historyEarlier).toBe(true));
  }
  expect(session()!.historyEarlier).toBeUndefined();
  expect(timeline().map(e => e.kind)).toEqual(["cortex", "decision", "fold", "cortex", "fold", "cortex", "said", "fold"]);
  expect((timeline()[3] as { text: string }).text.startsWith("cut-45")).toBe(true);
});

it("leaves out a fold whose first steps lie in the part not read, and only when the part before it is not read", () => {
  const row = (n: number, kind: string, text: string, toolStatus: string | null = null) => ({ historyId: `r${n}`, revision: "1", kind, text, toolStatus, recordedAt: new Date(Date.UTC(2026, 10, 17) + n * 1000).toISOString() });
  const rows = [row(0, "tool", step("a", "completed"), "completed"), row(1, "tool", step("b", "completed"), "completed"), row(2, "assistant", "After."), row(3, "user", "Thanks")];
  expect(conversationOf(rows, { description: "", answers: [] }).map(d => d.entry.kind)).toEqual(["fold", "cortex", "said"]);
  expect(conversationOf(rows, { description: "", answers: [], partial: true }).map(d => d.entry.kind)).toEqual(["cortex", "said"]);
  // A part that begins with the owner's message has nothing cut at its start.
  expect(conversationOf(rows.slice(3), { description: "", answers: [], partial: true }).map(d => d.entry.kind)).toEqual(["said"]);
});

it("reads a short conversation in one read and shows it exactly as a read from the beginning does", async () => {
  const fill = (svc: ReturnType<typeof service>) => {
    svc.add("user", "Index the repository");
    svc.add("assistant", "Looking"); svc.add("assistant", " around.");
    svc.add("tool", step("a", "pending"), "pending"); svc.add("tool", step("a", "completed"), "completed");
    svc.add("assistant", "Done.");
  };
  const back = service(), whole = service({ backward: false });
  fill(back); fill(whole);
  const a = open(back.s);
  await vi.waitFor(() => expect(a.texts()).toEqual(["Looking around.", "Done."]), { timeout: 4000 });
  const shown = a.timeline();
  expect(a.session()!.historyEarlier).toBeUndefined();
  dispose?.();
  const b = open(whole.s);
  await vi.waitFor(() => expect(b.texts()).toEqual(["Looking around.", "Done."]), { timeout: 4000 });
  expect(back.reads).toEqual([{ operation: "history.earlier", cursor: null }]);
  expect(b.timeline()).toEqual(shown);
  expect(b.session()!.historyEarlier).toBeUndefined();
  // The service without backward reads was asked once and then read forward from the beginning, as before.
  expect(whole.reads.map(r => r.operation)).toEqual(["history.earlier", "history.page"]);
});

/** Model the flow and scroll bounds in JSDOM (which has no layout). Each row is 50 px unless overridden. */
async function openPage(svc: ReturnType<typeof service>, rowHeight = (_row: Element) => 50, firstWindow: (() => number) | undefined = PAGE_SCREEN) {
  const observers: (() => void)[] = [];
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { observers.push(cb); } observe() {} disconnect() {} });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/task-1`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: svc.s.transport, actorId: "owner", registry: true, firstWindow })} />, root);
  await vi.waitFor(() => expect(root.textContent).toContain("Earlier conversation is not shown."), { timeout: 4000 });
  for (let i = 0; i < 12; i++) await flush();
  const box = root.querySelector<HTMLElement>(".task-panel .timeline")!, inner = box.firstElementChild!;
  let top = 0;
  const viewport = 400;
  const height = () => Array.from(inner.children, rowHeight).reduce((a, b) => a + b, 0);
  const slack = () => box.classList.contains("ws-history-end") ? Math.max(0, viewport - height()) : 0;
  Object.defineProperties(box, { clientHeight: { get: () => viewport }, scrollHeight: { get: () => Math.max(height(), viewport) }, scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, height() - viewport)); } } });
  const rect = Element.prototype.getBoundingClientRect;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (this === box) return { top: 0, bottom: viewport, height: viewport } as DOMRect;
    if (this.parentElement === inner) {
      const rows = [...inner.children];
      const y = slack() + rows.slice(0, rows.indexOf(this)).reduce((n, row) => n + rowHeight(row), 0) - top, h = rowHeight(this);
      return { top: y, bottom: y + h, height: h } as DOMRect;
    }
    return rect.call(this);
  });
  const resize = () => { for (const cb of observers) cb(); };
  return { root, box, inner, resize, height, scrollTo: (v: number) => { top = v; box.dispatchEvent(new Event("scroll")); }, top: () => top };
}

it("scrolling near the top reads the earlier part with a Loading line, and the row the reader looks at stays where it was", async () => {
  const svc = service();
  long(svc, 3000);
  const { root, box, inner, resize, scrollTo, top } = await openPage(svc);
  const release = svc.holdNext();
  // The reader scrolls up to near the top.
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
  scrollTo(120);
  const anchor = [...inner.children].find(row => !row.hasAttribute("data-follow-skip") && row.getBoundingClientRect().bottom > 0)!;
  const y = anchor.getBoundingClientRect().top, rows = inner.children.length;
  try { await vi.waitFor(() => expect(root.textContent).toContain("Loading earlier…")); }
  finally { release(); }
  await vi.waitFor(() => expect(inner.children.length).toBeGreaterThan(rows), { timeout: 4000 });
  resize();
  // The same node, at the same place on screen: the scroll position moved by exactly what was put above it.
  expect(anchor.isConnected).toBe(true);
  expect(anchor.getBoundingClientRect().top).toBe(y);
  expect(top() - 120).toBe((inner.children.length - rows) * 50);
  expect(root.textContent).toContain("Earlier conversation is not shown.");
  expect(root.textContent).not.toContain("Loading earlier…");
  // Nothing new was counted as missed by the reader who scrolled up.
  expect(root.textContent).not.toMatch(/\d+ new/);
});

it("Show earlier reads the earlier part; a failed read says so with Retry, and the beginning shows no such line", async () => {
  const svc = service();
  long(svc, 420);
  const { root, inner, resize } = await openPage(svc);
  const button = (name: string) => [...root.querySelectorAll<HTMLButtonElement>(".ws-earlier button")].find(b => b.textContent === name);
  svc.fail();
  button("Show earlier")!.click();
  await vi.waitFor(() => expect(root.textContent).toContain("Earlier conversation could not be read: "));
  expect(root.querySelector(".ws-earlier [role=alert]")).not.toBeNull();
  const first = inner.querySelector(".ai")!, rows = inner.children.length;
  button("Retry")!.click();
  await vi.waitFor(() => expect(inner.children.length).toBeGreaterThan(rows), { timeout: 4000 });
  resize();
  expect(first.isConnected).toBe(true);
  for (let i = 0; i < 4 && button("Show earlier"); i++) {
    const n = inner.children.length;
    button("Show earlier")!.click();
    await vi.waitFor(() => expect(inner.children.length).not.toBe(n), { timeout: 4000 });
  }
  await vi.waitFor(() => expect(root.querySelector(".ws-earlier")).toBeNull());
  expect(root.textContent).toContain("Message 0");
  expect(root.querySelector(".brief")!.hasAttribute("data-follow-skip")).toBe(false);
});

it("automatically fills a short following feed, anchors its tail, and stops once the marker is a screen above", async () => {
  const svc = service({ earlierDelay: 20 });
  for (let i = 0; i < 2200; i++) {
    if (i === 2000 || (i < 2000 && i % 80 === 0)) svc.add("user", `Sparse message ${i}`);
    else svc.add("system", "Display update: progress");
  }
  const { root, box, height, inner, resize, top } = await openPage(svc);
  const held = [...inner.querySelectorAll(".mine")].at(-1)!;
  expect(held.textContent).toBe("Sparse message 2000");
  expect(height()).toBeLessThan(box.clientHeight);
  const y = held.getBoundingClientRect().top;
  expect(top()).toBe(0);
  expect(svc.reads.map(r => r.cursor)).toEqual([null, "np1.100.2100"]);

  // No scroll input: the layout callback runs even while the feed is following.
  resize();
  const marker = inner.querySelector(".ws-earlier")!;
  await vi.waitFor(() => expect(marker.getBoundingClientRect().bottom).toBeLessThanOrEqual(-box.clientHeight));
  expect(svc.reads.length).toBeGreaterThan(4);
  expect(held.isConnected).toBe(true);
  expect(held.getBoundingClientRect().top).toBe(y);
  const settled = svc.reads.length;
  resize();
  await flush();
  expect(svc.reads).toHaveLength(settled);
  expect(root.querySelector(".ws-earlier button")?.textContent).toBe("Show earlier");
  expect(root.textContent).not.toMatch(/\d+ new/);
});

it("prefetches near the history marker even when a tall brief puts scrollTop more than one screen from zero", async () => {
  const svc = service();
  long(svc, 1000);
  const { box, inner, resize, scrollTo } = await openPage(svc, row => row.classList.contains("brief") ? 900 : 50);
  resize();
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
  scrollTo(850);
  expect(inner.querySelector(".ws-earlier")!.getBoundingClientRect().top).toBe(50);
  // The conversation opened on one page (rows 900 to 999, about one screen); the page before it comes in on its own.
  await vi.waitFor(() => expect(inner.textContent).toContain("Message 800"));
});

it.each(["empty wire pages", "filtered updates", "a cut assistant message", "hidden folds"] as const)(
  "one Show earlier press passes %s to publish more conversation",
  async obstacle => {
    const svc = service();
    // Leave another cursor at success: this is not a no-op exit at the beginning.
    svc.add("user", "Very first owner message");
    for (let i = 0; i < 100; i++) svc.add("user", `Much older message ${i}`);
    svc.add("user", "Older owner message");
    for (let i = 0; i < 300; i++) {
      if (obstacle === "empty wire pages") svc.add("server_filtered", "");
      if (obstacle === "filtered updates") svc.add("system", "Display update: progress");
      if (obstacle === "a cut assistant message") svc.add("assistant", "One streamed message. ");
      if (obstacle === "hidden folds") {
        if (i === 1) svc.add("user", "Index the repository"); // completes the leading fold, without a visible owner row
        else svc.add("tool", STEP_PREFIX + JSON.stringify({ title: "mcp__cortex__read_skill", callId: `hidden-${i}`, status: "completed" }), "completed");
      }
    }
    for (let i = 0; i < 100; i++) svc.add("user", `Held message ${i}`);
    const { root, inner, resize } = await openPage(svc);
    resize(); // The long held feed follows the end, with the marker far above it.
    // One page fills more than a screen, so the conversation opens on it alone.
    expect(svc.reads.map(r => r.cursor)).toEqual([null]);
    root.querySelector<HTMLButtonElement>(".ws-earlier button")!.click();

    await vi.waitFor(() => {
      expect(inner.textContent).toContain("Older owner message");
      expect(root.querySelector(".ws-earlier button")?.textContent).toBe("Show earlier");
    });
    expect(svc.reads.some(r => r.cursor === "np1.100.102")).toBe(true);
    expect(inner.textContent).toContain("Held message 0");
    expect(inner.textContent).not.toContain("Very first owner message");
  },
);

it("stops on an automatic read failure even with the marker still nearby, and resumes only on Retry", async () => {
  const svc = service();
  long(svc, 100);
  for (let i = 0; i < 200; i++) svc.add("system", "Display update: progress");
  const { root, inner, resize } = await openPage(svc);
  const before = svc.reads.length;
  svc.fail();
  resize();
  const button = () => root.querySelector<HTMLButtonElement>(".ws-earlier button");
  await vi.waitFor(() => expect(button()?.textContent).toBe("Retry"));
  resize(); resize();
  await flush();
  expect(svc.reads).toHaveLength(before + 1);

  button()!.click();
  await vi.waitFor(() => expect(inner.textContent).toContain("Message 0"));
  expect(root.querySelector(".ws-earlier")).toBeNull();
});

it("fails rather than publishing a page whose cursor repeats", async () => {
  const svc = service();
  long(svc, 301);
  const { store, session, timeline } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"));
  const held = timeline();
  svc.repeat();
  store.loadEarlierHistory("task-1");
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("failed"));
  expect(timeline()).toEqual(held);
});

it("keeps a concurrently streamed tail without counting it as an earlier addition", async () => {
  const svc = service();
  for (let i = 0; i < 101; i++) svc.add("user", `Older message ${i}`);
  for (let i = 0; i < 100; i++) svc.add("system", "Display update: progress");
  for (let i = 0; i < 100; i++) svc.add("user", `Held message ${i}`);
  const { store, session, texts } = open(svc.s, PAGE_SCREEN);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"));
  const release = svc.holdNext();
  try {
    store.loadEarlierHistory("task-1");
    await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("loading"));
    svc.add("assistant", "Streamed while paging.");
    svc.changed();
    await vi.waitFor(() => expect(texts().at(-1)).toBe("Streamed while paging."));
  } finally { release(); }
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"));
  expect(texts()[0]).toBe("Older message 1");
  expect(texts().at(-1)).toBe("Streamed while paging.");
  expect(svc.reads.some(r => r.cursor === "np1.100.101")).toBe(true);
});

it("keeps recovery from a refused earlier cursor pending while an older forward read finishes", async () => {
  const svc = service();
  for (let i = 0; i < 101; i++) svc.add("user", `Older message ${i}`);
  for (let i = 0; i < 100; i++) svc.add("system", "Display update: progress");
  for (let i = 0; i < 200; i++) svc.add("user", `Held message ${i}`);
  const { store, session } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"));
  const release = svc.holdForward();
  svc.changed();
  await vi.waitFor(() => expect(svc.reads.some(r => r.operation === "history.page" && r.cursor !== null)).toBe(true));
  svc.reset();
  store.loadEarlierHistory("task-1");
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("failed"));
  const fresh = () => svc.reads.filter(r => r.operation === "history.earlier" && r.cursor === null).length;
  expect(fresh()).toBe(1);
  release();
  // The forward reply must not restore the dropped position: the latest slice is read afresh, once.
  await vi.waitFor(() => expect(fresh()).toBe(2), { timeout: 4000 });
  const forwards = svc.reads.filter(r => r.operation === "history.page").length;
  await flush();
  expect(svc.reads.filter(r => r.operation === "history.page")).toHaveLength(forwards);
  expect(fresh()).toBe(2);
  // The same refused position waits for Retry rather than looping.
  expect(session()?.historyEarlier?.state).toBe("failed");
});

it("does not prefetch while the history boundary lies entirely below the viewport of a tall brief", async () => {
  const svc = service();
  long(svc, 1000);
  const { box, inner, resize, scrollTo, top } = await openPage(svc, row => row.classList.contains("brief") ? 1600 : 50);
  // A paused reader at the very top of the brief.
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
  scrollTo(0);
  await flush();
  const before = svc.reads.length;
  resize(); resize();
  await flush();
  expect(top()).toBe(0);
  expect(inner.querySelector(".ws-earlier")!.getBoundingClientRect().top).toBeGreaterThanOrEqual(box.clientHeight);
  expect(svc.reads).toHaveLength(before);
  // Reading down: nothing happens until the marker enters the trigger area.
  scrollTo(1000);
  await flush();
  expect(svc.reads).toHaveLength(before);
  scrollTo(1250);
  await vi.waitFor(() => expect(svc.reads.length).toBeGreaterThan(before));
});

it("counts plumbing folds only when the Conversation tool view displays them", async () => {
  localStorage.setItem("cw.ws.allTools", "true");
  const svc = service();
  for (let i = 0; i < 101; i++) svc.add("user", `Older message ${i}`);
  svc.add("user", "Index the repository"); // ends the leading partial segment, but isn't repeated in the feed
  for (let i = 0; i < 99; i++) {
    svc.add("tool", STEP_PREFIX + JSON.stringify({ title: "mcp__cortex__read_skill", callId: `tool-${i}`, status: "completed" }), "completed");
  }
  for (let i = 0; i < 100; i++) svc.add("user", `Held message ${i}`);
  const { root, inner, resize } = await openPage(svc);
  resize();
  root.querySelector<HTMLButtonElement>(".ws-earlier button")!.click();
  await vi.waitFor(() => expect(inner.textContent).toContain("Worked "));
  expect(svc.reads.map(r => r.cursor)).toEqual([null, "np1.100.201"]);
  expect(inner.textContent).not.toContain("Older message ");
});
