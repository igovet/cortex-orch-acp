import { render } from "solid-js/web";
import { undo } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace, type WindowChrome } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, snapshot, wireTask } from "./support";

/** The real editor on the real port, over a disk that answers as the machine helper does. */
const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"];
const repoFacts = { projectId: "p", remote: "", tree: ["a.txt", "b.txt"], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "dev", defaultBranch: "main",
  checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "dev", path: "/cedar", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }] };

function disk(files: Record<string, string> = { "a.txt": "one\ntwo\n", "b.txt": "bee\n" }, methods: string[] = []) {
  let revision = 0;
  const state = { files: { ...files }, revisions: {} as Record<string, string>, writer: null as string | null, readOnly: null as string | null, saves: [] as { file: string; text: string; revision: string | null }[], lose: false, loseAfterWrite: false, refuse: null as string | null };
  const put = (file: string, text: string) => { state.files[file] = text; state.revisions[file] = `r${++revision}`; };
  for (const [file, text] of Object.entries(files)) put(file, text);
  const config = structuredClone(emptyWorkspace().machines[0].config);
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call", ...methods], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(OFFERED),
    "project.snapshot": () => snapshot("4", [wireTask({ executionId: "e", phase: "not_started", runtimeState: null, executionState: "not_started" })]),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
    "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
    "native.open_editor": () => ({ outcome: "opened" }),
    "native.open_logs": () => ({ outcome: "opened" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params: { file: string; text: string; revision: string | null } };
      if (op === "repo.snapshot") return { ok: true, result: repoFacts };
      if (op === "product.read") return { ok: true, result: { tasks: {}, sprints: [], messages: [] } };
      if (op === "workers") return { ok: true, result: [] };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      if (op === "repo.file") {
        const text = state.files[params.file];
        return { ok: true, result: text === undefined ? { file: params.file, text: null, revision: null, reason: "This file is not in the checkout." } : { file: params.file, text, revision: state.revisions[params.file], readOnly: state.readOnly, writer: state.writer } };
      }
      if (op === "repo.file.save") {
        state.saves.push({ file: params.file, text: params.text, revision: params.revision });
        if (state.lose) throw new Error("service_timeout");
        if (state.refuse) return { ok: false, code: "refused", message: state.refuse };
        if (state.writer) return { ok: true, result: { outcome: "writer_active", writer: state.writer } };
        if ((state.revisions[params.file] ?? null) !== params.revision) return { ok: true, result: { outcome: "conflict", revision: state.revisions[params.file] ?? null } };
        put(params.file, params.text);
        if (state.loseAfterWrite) throw new Error("service_timeout");
        return { ok: true, result: { outcome: "saved", revision: state.revisions[params.file], size: params.text.length } };
      }
      return { ok: true, result: {} };
    },
  });
  return { ...s, state, put };
}

let dispose: (() => void) | undefined;
beforeEach(() => {
  localStorage.clear(); sessionStorage.clear();
  // jsdom has no layout. These answer the editor's measuring with nothing, which is all a test without pixels needs.
  Range.prototype.getClientRects = () => ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => ({ x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, toJSON() {} });
});
afterEach(() => { vi.useRealTimers(); dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const CODE = (file = "a.txt") => `/m/${LOCAL_MACHINE_ID}/p/p/code?file=${file}`;
const settled = async () => { for (let i = 0; i < 14; i++) await flush(); };
async function open(d: ReturnType<typeof disk>, path = CODE(), chrome?: WindowChrome) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace chrome={chrome} port={createRealPort({ transport: d.transport, actorId: "owner", registry: true })} />, root);
  for (let i = 0; i < 40 && !document.querySelector(".cm-content"); i++) await settled();
  await settled();
}
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim() === text || b.getAttribute("aria-label") === text);
const crumbButton = (text: string) => all(".ecrumb button").find(b => b.textContent?.trim() === text);
const view = () => EditorView.findFromDOM($(".cm-editor")!)!;
const doc = () => view().state.doc.toString();
const until = async (text: string) => { for (let i = 0; i < 20 && !($(".cm-editor") && doc() === text); i++) await settled(); };
const type = (text: string) => view().dispatch({ changes: { from: view().state.doc.length, insert: text } });
const click = async (text: string) => { button(text)!.click(); await settled(); };
const status = () => $(".ws-save-state")?.textContent ?? "";
const tab = () => $(".etab")!.getAttribute("aria-label");

describe("editing a checkout file", () => {
  it("opens a labelled editor and saves only changed text, bound to the revision it read", async () => {
    const d = disk();
    await open(d);
    expect($(".cm-content")!.getAttribute("aria-label")).toBe("Edit a.txt");
    expect(doc()).toBe("one\ntwo\n");
    expect(button("Save a.txt")!.getAttribute("aria-disabled")).toBe("true");
    expect(button("Save a.txt")!.dataset.why).toBe("No changes to save");
    type("three\n");
    await settled();
    expect(status()).toBe("Unsaved");
    expect(tab()).toBe("a.txt, unsaved");
    expect(button("Save a.txt")!.getAttribute("aria-disabled")).toBeNull();
    await click("Save a.txt");
    expect(d.state.saves).toEqual([{ file: "a.txt", text: "one\ntwo\nthree\n", revision: "r1" }]);
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nthree\n");
    expect(status()).toBe("Saved");
    expect(tab()).toBe("a.txt");
    type("four\n");
    await settled();
    await click("Save a.txt");
    expect(d.state.saves.at(-1)!.revision).toBe("r3");
  });

  it("saves with Ctrl+S from the editor", async () => {
    const d = disk();
    await open(d);
    type("x");
    await settled();
    $(".cm-content")!.dispatchEvent(new KeyboardEvent("keydown", { key: "s", code: "KeyS", keyCode: 83, ctrlKey: true, bubbles: true, cancelable: true }));
    await settled();
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nx");
  });

  it("offers a find panel, and keeps the file's own line endings", async () => {
    const d = disk({ "a.txt": "one\r\ntwo\r\n" });
    await open(d);
    expect(doc()).toBe("one\ntwo\n");
    type("x");
    await settled();
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBe("one\r\ntwo\r\nx");
    $(".cm-content")!.dispatchEvent(new KeyboardEvent("keydown", { key: "f", code: "KeyF", keyCode: 70, ctrlKey: true, bubbles: true, cancelable: true }));
    await settled();
    expect($(".cm-search input[name=search]")).not.toBeNull();
    expect($(".cm-search input[name=replace]")).not.toBeNull();
  });

  it("will not rewrite a file with mixed line endings", async () => {
    const d = disk({ "a.txt": "one\r\ntwo\n" });
    await open(d);
    expect($(".cm-content")!.getAttribute("aria-label")).toBe("Read a.txt");
    expect($(".ed .strip.info")!.textContent).toContain("mixes line endings");
    expect(button("Save a.txt")!.getAttribute("aria-disabled")).toBe("true");
  });

  it("shows why a file cannot be edited and stays read-only", async () => {
    const d = disk();
    d.state.readOnly = "This file is a link. Open the file it points to.";
    await open(d);
    expect($(".cm-content")!.getAttribute("aria-label")).toBe("Read a.txt");
    expect($(".ed .strip.info")!.textContent).toContain("is a link");
    expect(button("Save a.txt")!.dataset.why).toContain("is a link");
  });
});

describe("a change that did not come from this editor", () => {
  it("an unchanged disk/snapshot refresh sends no editor transactions", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const d = disk();
    await open(d);
    const dispatch = vi.spyOn(view(), "dispatch");
    d.send({ type: "refresh" });
    vi.advanceTimersByTime(3000);
    await settled();
    expect(dispatch).not.toHaveBeenCalled();
    dispatch.mockRestore();
  });

  it("a clean file follows the disk and a dirty one keeps the owner's text", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const d = disk();
    await open(d);
    d.put("a.txt", "worker\n");
    vi.advanceTimersByTime(3000);
    await settled();
    expect(doc()).toBe("worker\n");
    // A clean file says so beside Save, which is why Save is off.
    expect(status()).toBe("No changes");
    type("mine");
    await settled();
    d.put("a.txt", "worker again\n");
    vi.advanceTimersByTime(3000);
    await settled();
    expect(doc()).toBe("worker\nmine");
    expect($(".strip.warn[role=alert]")!.textContent).toContain("changed on disk");
    expect(status()).toBe("Changed on disk");
    expect(button("Save a.txt")!.getAttribute("aria-disabled")).toBe("true");
    expect(d.state.saves).toEqual([]);
  });

  it("a conflict at Save writes nothing; Keep mine saves over exactly what was shown", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    d.put("a.txt", "worker\n");
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBe("worker\n");
    expect(tab()).toBe("a.txt, unsaved");
    expect($(".strip.warn[role=alert]")!.textContent).toContain("Your text is kept here");
    await click("Keep mine");
    expect(d.state.saves.at(-1)).toEqual({ file: "a.txt", text: "one\ntwo\nmine\n", revision: "r3" });
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
    expect($(".strip.warn[role=alert]")).toBeNull();
    expect(status()).toBe("Saved");
  });

  it("a read that started before a save cannot undo it on screen", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    const answer = d.answers["machine.call"]!;
    let release = () => {};
    d.answers["machine.call"] = message => (message as { op: string }).op === "repo.file"
      ? new Promise(resolve => { const old = answer(message); release = () => resolve(old); })
      : answer(message);
    vi.advanceTimersByTime(3000);
    await settled();
    d.answers["machine.call"] = answer;
    await click("Save a.txt");
    expect(status()).toBe("Saved");
    release();
    await settled();
    expect(doc()).toBe("one\ntwo\nmine\n");
    expect(status()).toBe("Saved");
  });

  it("Keep mine meets a second change as another conflict, never as an overwrite", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    d.put("a.txt", "worker\n");
    await click("Save a.txt");
    d.put("a.txt", "worker twice\n");
    await click("Keep mine");
    expect(d.state.files["a.txt"]).toBe("worker twice\n");
    expect($(".strip.warn[role=alert]")).not.toBeNull();
  });

  it("Reload takes the disk's version and one undo brings the owner's text back", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    d.put("a.txt", "worker\n");
    await click("Save a.txt");
    await click("Reload from disk");
    expect(doc()).toBe("worker\n");
    expect(tab()).toBe("a.txt");
    undo(view());
    await settled();
    expect(doc()).toBe("one\ntwo\nmine\n");
    expect(tab()).toBe("a.txt, unsaved");
  });

  it("a deleted file offers to restore the owner's text and never recreates it unasked", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    delete d.state.files["a.txt"]; delete d.state.revisions["a.txt"];
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBeUndefined();
    expect($(".strip.warn[role=alert]")!.textContent).toContain("no longer on disk");
    expect(button("Reload from disk")).toBeUndefined();
    await click("Keep mine");
    expect(d.state.saves.at(-1)!.revision).toBeNull();
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
  });
});

describe("while Cortex works in the checkout", () => {
  it("locks the file until the owner chooses, keeps their text and saves only once the machine confirms quiet", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const d = disk();
    d.state.writer = "task-1";
    await open(d);
    expect($(".strip.warn")!.textContent).toContain("A task is working in this checkout");
    expect($(".cm-content")!.getAttribute("aria-label")).toBe("Read a.txt");
    expect(button("Stop the task first")).toBeDefined();
    await click("Edit anyway");
    expect($(".cm-content")!.getAttribute("aria-label")).toBe("Edit a.txt");
    type("mine\n");
    await settled();
    expect(button("Save a.txt")!.getAttribute("aria-disabled")).toBe("true");
    expect(button("Save a.txt")!.dataset.why).toContain("waits until the task stops");
    expect(d.state.saves).toEqual([]);
    d.state.writer = null;
    vi.advanceTimersByTime(3000);
    await settled();
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
    expect($(".strip.warn")).toBeNull();
  });

  it("a writer that starts during a save keeps the text unsaved", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    d.state.writer = "unconfirmed";
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\n");
    expect(tab()).toBe("a.txt, unsaved");
    expect($(".strip.warn")!.textContent).toContain("could not be confirmed");
    expect(button("Stop the task first")).toBeUndefined();
  });
});

describe("owner text is never lost", () => {
  it("a lost reply stays unconfirmed, never clean, and is not sent again on its own", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    d.state.lose = true;
    await click("Save a.txt");
    expect(status()).toBe("Save not confirmed");
    expect(tab()).toBe("a.txt, unsaved");
    expect(doc()).toBe("one\ntwo\nmine\n");
    expect(d.state.saves).toHaveLength(1);
    await settled();
    expect(d.state.saves).toHaveLength(1);
    d.state.lose = false;
    await click("Save a.txt");
    expect(status()).toBe("Saved");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
  });

  it("a lost reply whose write landed is confirmed by reading the disk", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    d.state.loseAfterWrite = true;
    await click("Save a.txt");
    expect(status()).toBe("Saved");
    expect(tab()).toBe("a.txt");
  });

  it("a refusal says why and keeps the text", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    d.state.refuse = "This file is too large to save here.";
    await click("Save a.txt");
    expect($(".strip.bad[role=alert]")!.textContent).toContain("too large");
    expect(status()).toBe("Not saved");
    expect(tab()).toBe("a.txt, unsaved");
  });

  it("refuses text the machine could not take without sending it", async () => {
    const d = disk();
    await open(d);
    type("x".repeat(512 * 1024 + 1));
    await settled();
    await click("Save a.txt");
    expect(d.state.saves).toEqual([]);
    expect($(".strip.bad[role=alert]")!.textContent).toContain("too large to save here");
  });

  it("closing a dirty tab asks first, and Cancel keeps it", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    await click("Close a.txt");
    expect($("dialog[open]")!.getAttribute("aria-label")).toBe("Unsaved changes");
    await click("Cancel");
    expect(doc()).toBe("one\ntwo\nmine\n");
    await click("Close a.txt");
    await click("Save");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
  });

  it("switching files and leaving the screen keep the text, its tab and its undo", async () => {
    const d = disk();
    await open(d);
    type("mine\n");
    await settled();
    all<HTMLButtonElement>(".tn").find(b => b.textContent?.includes("b.txt"))!.click();
    await until("bee\n");
    expect(doc()).toBe("bee\n");
    expect(all(".etab").map(t => t.getAttribute("aria-label"))).toContain("a.txt, unsaved");
    all(".etab").find(t => t.textContent?.includes("a.txt"))!.click();
    await until("one\ntwo\nmine\n");
    expect(doc()).toBe("one\ntwo\nmine\n");
    undo(view());
    expect(doc()).toBe("one\ntwo\n");
    type("again\n");
    await settled();
    history.pushState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/git`);
    window.dispatchEvent(new PopStateEvent("popstate"));
    await settled();
    expect($(".cm-editor")).toBeNull();
    history.pushState(null, "", CODE());
    window.dispatchEvent(new PopStateEvent("popstate"));
    for (let i = 0; i < 30 && !$(".cm-editor"); i++) await settled();
    await settled();
    expect(doc()).toBe("one\ntwo\nagain\n");
    expect(tab()).toBe("a.txt, unsaved");
    // Both retained states must route new edits through the remounted view, not an old closure.
    all<HTMLButtonElement>(".tn").find(b => b.textContent?.includes("b.txt"))!.click();
    await until("bee\n");
    type("after return");
    await settled();
    await click("Save b.txt");
    expect(d.state.files["b.txt"]).toBe("bee\nafter return");
    all(".etab").find(t => t.textContent?.includes("a.txt"))!.click();
    await until("one\ntwo\nagain\n");
    expect(doc()).toBe("one\ntwo\nagain\n");
  });

  it("the window asks before closing over unsaved text, and closes after saving it", async () => {
    let guard = () => false;
    const closeNow = vi.fn();
    const chrome: WindowChrome = { maximized: () => false, minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn(), startDragging: vi.fn(), guardClose: g => { guard = g; }, closeNow };
    const d = disk();
    await open(d, CODE(), chrome);
    expect(guard()).toBe(false);
    type("mine\n");
    await settled();
    expect(guard()).toBe(true);
    await settled();
    expect($("dialog[open]")!.textContent).toContain("1 file has unsaved changes");
    expect($("dialog[open]")!.textContent).toContain("a.txt");
    await click("Keep working");
    expect(closeNow).not.toHaveBeenCalled();
    expect(guard()).toBe(true);
    await settled();
    await click("Save all and close");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
    expect(closeNow).toHaveBeenCalledOnce();
  });

  it("a save that cannot land keeps the window open with the reason", async () => {
    let guard = () => false;
    const closeNow = vi.fn();
    const chrome: WindowChrome = { maximized: () => false, minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn(), startDragging: vi.fn(), guardClose: g => { guard = g; }, closeNow };
    const d = disk();
    await open(d, CODE(), chrome);
    type("mine\n");
    await settled();
    d.put("a.txt", "worker\n");
    guard();
    await settled();
    await click("Save all and close");
    expect(closeNow).not.toHaveBeenCalled();
    expect($("dialog[open]")!.textContent).toContain("Choose what to keep first");
    await click("Close without saving");
    expect(closeNow).toHaveBeenCalledOnce();
  });
});

describe("autosave", () => {
  it("is off by default and saves about a second after typing when on", async () => {
    const d = disk();
    await open(d);
    type("x");
    await new Promise(resolve => setTimeout(resolve, 1200));
    expect(d.state.saves).toEqual([]);
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    localStorage.setItem("cw.ws.autosave", "true");
    const e = disk();
    await open(e);
    type("y");
    await settled();
    expect(e.state.saves).toEqual([]);
    await new Promise(resolve => setTimeout(resolve, 1200));
    await settled();
    expect(e.state.files["a.txt"]).toBe("one\ntwo\ny");
    expect(status()).toBe("Saved");
  });

  it("waits for a conflict to be settled", async () => {
    localStorage.setItem("cw.ws.autosave", "true");
    const d = disk();
    await open(d);
    type("y");
    d.put("a.txt", "worker\n");
    await new Promise(resolve => setTimeout(resolve, 1200));
    await settled();
    expect(d.state.saves).toHaveLength(1);
    type("z");
    await new Promise(resolve => setTimeout(resolve, 1200));
    await settled();
    expect(d.state.saves).toHaveLength(1);
    expect(d.state.files["a.txt"]).toBe("worker\n");
  });
});

describe("opening things elsewhere", () => {
  it("says why when this computer has no opener", async () => {
    const d = disk();
    await open(d);
    const external = crumbButton("Open in VS Code")!;
    expect(external.getAttribute("aria-disabled")).toBe("true");
    expect(external.dataset.why).toContain("No external editor was found");
    external.click();
    await settled();
    expect(d.calls.some(c => c.operation === "native.open_editor")).toBe(false);
  });

  it("opens the checkout's file at its line in VS Code", async () => {
    const d = disk(undefined, ["native.open_editor", "native.open_editor.vscode", "native.open_logs"]);
    await open(d, CODE() + "&line=2");
    crumbButton("Open in VS Code")!.click();
    await settled();
    expect(d.calls.filter(c => c.operation === "native.open_editor").map(c => c.message)).toEqual([{ projectId: "p", checkoutId: "p:main", file: "a.txt", line: 2 }]);
  });

  it("names the default application when only a system opener exists", async () => {
    const d = disk(undefined, ["native.open_editor"]);
    await open(d);
    expect(crumbButton("Open in VS Code")).toBeUndefined();
    crumbButton("Open in default app")!.click();
    await settled();
    expect(d.calls.some(c => c.operation === "native.open_editor")).toBe(true);
  });

  it("Open logs says why it is off, and works where an opener exists", async () => {
    const off = disk();
    await open(off, `/settings/machine/${LOCAL_MACHINE_ID}/diagnostics`);
    expect(button("Open logs")!.dataset.why).toContain("No folder opener was found");
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    const on = disk(undefined, ["native.open_logs"]);
    await open(on, `/settings/machine/${LOCAL_MACHINE_ID}/diagnostics`);
    expect(button("Open logs")!.getAttribute("aria-disabled")).toBeNull();
    await click("Open logs");
    expect(on.calls.some(c => c.operation === "native.open_logs")).toBe(true);
  });
});
