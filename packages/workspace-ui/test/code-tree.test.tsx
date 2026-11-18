import { render } from "solid-js/web";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type FolderEntry } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { codeTree, treeRows } from "../src/review-model";
import { capabilitiesWire, flush, scriptedTransport, snapshot, wireTask } from "./support";

/**
 * Code's tree shows the whole project folder: the snapshot's Git files at once, and what Git ignores (`.env`, build
 * output) from the machine's on-demand folder reads, dimmed. An ignored folder is read only when the owner opens it.
 */
const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"];
const entry = (name: string, dir = false, ignored: boolean | null = true): FolderEntry => ({ name, dir, link: false, ignored });
const FOLDERS: Record<string, { entries: FolderEntry[]; more: number; exact: boolean }> = {
  target: { entries: [entry("debug", true), entry("out.txt")], more: 3, exact: true },
  "target/debug": { entries: [entry("app")], more: 0, exact: true },
};
function machine() {
  const files: Record<string, string> = { "a.txt": "one\n", "src/main.ts": "main\n", ".env": "API_KEY=fake-fixture\n", "target/out.txt": "built\n" };
  const repoFacts = { projectId: "p", remote: "", tree: [".gitignore", "a.txt", "src/main.ts"], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "main", defaultBranch: "main",
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/cedar", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }] };
  const config = structuredClone(emptyWorkspace().machines[0].config);
  const lists: { dirs: string[]; overview: boolean }[] = [];
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(OFFERED),
    "project.snapshot": () => snapshot("4", [wireTask({ executionId: "e", phase: "not_started", runtimeState: null, executionState: "not_started" })]),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
    "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params: { file: string; dirs: string[]; overview: boolean } };
      if (op === "repo.snapshot") return { ok: true, result: repoFacts };
      if (op === "product.read") return { ok: true, result: { tasks: {}, sprints: [], messages: [] } };
      if (op === "workers") return { ok: true, result: [] };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      if (op === "repo.list") {
        lists.push({ dirs: params.dirs, overview: params.overview });
        return { ok: true, result: {
          overview: params.overview ? { truncated: false, entries: [{ path: ".env", dir: false, ignored: true }, { path: "target", dir: true, ignored: true }] } : null,
          folders: params.dirs.map(dir => ({ dir, ...FOLDERS[dir] })),
        } };
      }
      if (op === "repo.file") {
        const text = files[params.file];
        return { ok: true, result: text === undefined ? { file: params.file, text: null, revision: null, reason: "This file is not in the checkout." } : { file: params.file, text, revision: "r1", readOnly: null, writer: null } };
      }
      return { ok: true, result: {} };
    },
  });
  return { ...s, lists };
}

let dispose: (() => void) | undefined;
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear();
  Range.prototype.getClientRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} });
});
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settled = async () => { for (let i = 0; i < 14; i++) await flush(); };
const rows = () => [...document.querySelectorAll<HTMLElement>(".tree[role=tree] .tn")];
const row = (name: string) => rows().find(r => r.querySelector(".trunc")?.textContent === name);

async function openCode(m: ReturnType<typeof machine>) {
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/code?file=a.txt`);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: m.transport, actorId: "owner", registry: true })} />, root);
  for (let i = 0; i < 40 && !document.querySelector(".cm-content"); i++) await settled();
  await settled();
}

describe("Code's tree of the whole project folder", () => {
  it("shows .env and an ignored folder, dimmed, from one read on open, and reads that folder only when it is opened", async () => {
    const m = machine();
    await openCode(m);
    // One read on open: what the snapshot leaves out, and no folder yet.
    expect(m.lists).toEqual([{ dirs: [], overview: true }]);
    expect(row(".env")?.getAttribute("aria-label")).toBe(".env, ignored");
    expect(row(".env")?.classList.contains("ws-ignored")).toBe(true);
    expect(row("a.txt")?.classList.contains("ws-ignored")).toBe(false);
    expect(row("src")?.getAttribute("aria-expanded")).toBe("true");
    const target = row("target")!;
    expect(target.classList.contains("ws-ignored")).toBe(true);
    expect(target.getAttribute("aria-expanded")).toBe("false");
    expect(row("debug")).toBeUndefined();
    await settled(); expect(row("target"), "plain settle").toBe(target);
    m.send({ type: "invalidated", projectId: "p", source: "runtime" }); await settled(); expect(row("target"), "invalidated").toBe(target);
    m.send({ type: "refresh" }); await settled(); expect(row("target"), "refresh").toBe(target);
    // Runtime events and a full reread never list folders again: there is no timer either.
    for (let i = 0; i < 3; i++) { m.send({ type: "invalidated", projectId: "p", source: "runtime" }); m.send({ type: "refresh" }); await settled(); }
    expect(m.lists.length).toBe(1);
    // Opening it reads that one folder once, one level.
    expect(row("target")).toBe(target);
    target.click(); await settled();
    expect(m.lists.slice(1)).toEqual([{ dirs: ["target"], overview: false }]);
    expect(row("target")!.getAttribute("aria-expanded")).toBe("true");
    expect(row("out.txt")?.classList.contains("ws-ignored")).toBe(true);
    expect(row("debug")?.getAttribute("aria-expanded")).toBe("false");
    expect(row("app")).toBeUndefined();
    // The rest of a long folder is counted where it would be.
    const note = rows().find(r => r.classList.contains("ws-tree-note"))!;
    expect(note.textContent).toBe("3 more not shown");
    expect(rows().indexOf(note)).toBe(rows().indexOf(row("out.txt")!) + 1);
    // Closing reads nothing; opening again is one more read.
    row("target")!.click(); await settled();
    expect(row("out.txt")).toBeUndefined();
    expect(m.lists.length).toBe(2);
    row("target")!.click(); await settled();
    expect(m.lists.length).toBe(3);
    row("debug")!.click(); await settled();
    expect(m.lists.at(-1)).toEqual({ dirs: ["target/debug"], overview: false });
    expect(row("app")?.classList.contains("ws-ignored")).toBe(true);
    // The owner's refresh rereads the overview and each open folder, in one call.
    document.querySelector<HTMLButtonElement>('[aria-label="Refresh files"]')!.click(); await settled();
    expect(m.lists.at(-1)).toEqual({ dirs: ["target", "target/debug"], overview: true });
  });

  it("opens .env in the editor like any other file", async () => {
    const m = machine();
    await openCode(m);
    row(".env")!.click();
    for (let i = 0; i < 30 && !document.querySelector(".cm-content")?.textContent?.includes("API_KEY"); i++) await settled();
    expect(EditorView.findFromDOM(document.querySelector(".cm-editor")!)!.state.doc.toString()).toBe("API_KEY=fake-fixture\n");
    expect(m.calls.some(c => (c.message as { op?: string; params?: { file?: string } }).op === "repo.file" && (c.message as { params: { file: string } }).params.file === ".env")).toBe(true);
    expect(location.search).toContain("file=.env");
  });
});

describe("the tree model", () => {
  it("merges listed folders under the snapshot's files and keeps a lazy folder closed until opened", () => {
    const tree = codeTree(["a.txt", "src/b.ts"], { truncated: true, entries: [{ path: "src/.env", dir: false, ignored: true }, { path: "node_modules", dir: true, ignored: true }] },
      new Map([["node_modules", { entries: [entry("pkg", true)], more: 0, exact: false }]]));
    expect([...tree.lazy].sort()).toEqual(["node_modules", "node_modules/pkg"]);
    const closed = treeRows([], new Set(["node_modules"]), tree).map(r => [r.kind, r.path, !!r.ignored]);
    expect(closed).toEqual([["file", "a.txt", false], ["dir", "node_modules", true], ["dir", "src", false], ["file", "src/.env", true], ["file", "src/b.ts", false], ["more", "", false]]);
    const open = treeRows([], new Set(["node_modules/pkg"]), tree).map(r => r.path);
    expect(open).toEqual(["a.txt", "node_modules", "node_modules/pkg", "src", "src/.env", "src/b.ts", ""]);
  });
});
