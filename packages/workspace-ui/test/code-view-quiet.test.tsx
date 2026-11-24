import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type FileRead } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createBuffers } from "../src/editor-buffers";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, snapshot, wireTask } from "./support";

/**
 * The window froze once with this exact screen open: Code, reached from Git, on a checkout a running task holds.
 * Nothing in it may redo work that changed nothing, so a busy task's stream of updates costs this screen almost nothing.
 */
const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"];
const LOCK = "Cortex is still writing in this checkout. Stop it before editing.";
const hunk = { header: "@@ -2,0 +3,1 @@", staged: false, lines: [{ kind: "add", old: null, new: 3, text: "three" }] };
function heldCheckout() {
  const repoFacts = { projectId: "p", remote: "", tree: ["a.txt", "b.txt", "docs/c.md"], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "main", defaultBranch: "main",
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/cedar", taskId: "task-1", label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null,
      files: [{ path: "a.txt", status: "M", stage: "unstaged", add: 1, del: 0, hunks: [hunk] }] }] };
  const config = structuredClone(emptyWorkspace().machines[0].config);
  return scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(OFFERED),
    "project.snapshot": () => snapshot("4", [wireTask({ executionId: "e", phase: "running", runtimeState: "EXECUTING", executionState: "running" })]),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
    "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params: { file: string } };
      if (op === "repo.snapshot") return { ok: true, result: repoFacts };
      if (op === "product.read") return { ok: true, result: { tasks: {}, sprints: [], messages: [] } };
      if (op === "workers") return { ok: true, result: [] };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      if (op === "repo.file") return { ok: true, result: { file: params.file, text: "one\ntwo\nthree\n", revision: "r1", readOnly: LOCK, writer: "task-1" } };
      return { ok: true, result: {} };
    },
  });
}

let dispose: (() => void) | undefined;
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear();
  Range.prototype.getClientRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} });
});
afterEach(() => { vi.useRealTimers(); dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settled = async () => { for (let i = 0; i < 14; i++) await flush(); };

describe("Code on a checkout a running task holds", () => {
  it("an update that changes nothing here rebuilds neither the file tree nor the open tab", async () => {
    const d = heldCheckout();
    history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/code?file=a.txt`);
    const root = document.createElement("div");
    document.body.append(root);
    dispose = render(() => <Workspace port={createRealPort({ transport: d.transport, actorId: "owner", registry: true, localCode: true })} />, root);
    for (let i = 0; i < 40 && !document.querySelector(".cm-content"); i++) await settled();
    await settled();
    expect(document.querySelector(".cm-content")!.getAttribute("aria-label")).toBe("Read a.txt");
    const tree = [...document.querySelectorAll(".tree[role=tree] .tn")], tab = document.querySelector(".etab");
    expect(tree.length).toBeGreaterThan(3);
    const reads = d.calls.length;
    // What a running task's events cause: runtime invalidations and a full reread, none of which change this checkout.
    for (let i = 0; i < 4; i++) { d.send({ type: "invalidated", projectId: "p", source: "runtime" }); d.send({ type: "refresh" }); await settled(); }
    expect(d.calls.length).toBeGreaterThan(reads);
    expect([...document.querySelectorAll(".tree[role=tree] .tn")].every((row, i) => row === tree[i])).toBe(true);
    expect(document.querySelector(".etab")).toBe(tab);
  });
});

describe("the open file's disk check", () => {
  it("a poll that finds the same revision, writer and lock replaces no record", async () => {
    const read: FileRead = { text: "one\n", revision: "r1", readOnly: LOCK, writer: "task-1" };
    const buffers = createBuffers({ read: async () => ({ ...read }), remember: () => {} });
    await buffers.open("p", "p:main", "a.txt");
    const before = buffers.list();
    expect(before[0]).toMatchObject({ loaded: true, writer: "task-1", readOnly: LOCK });
    for (let i = 0; i < 3; i++) await buffers.refresh(before[0].key);
    expect(buffers.list()).toBe(before);
    // A real change still reaches the screen.
    read.writer = null;
    await buffers.refresh(before[0].key);
    expect(buffers.list()).not.toBe(before);
    expect(buffers.list()[0].writer).toBeNull();
  });
});
