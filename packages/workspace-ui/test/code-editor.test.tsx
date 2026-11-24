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

const rev = (n: number) => n.toString(16).padStart(64, "0");
const token = "a".repeat(64);
function disk(files: Record<string, string> = { "a.txt": "one\ntwo\n", "b.txt": "bee\n" }, methods: string[] = []) {
  let revision = 0;
  const state = { files: { ...files }, revisions: {} as Record<string, string>, writer: null as string | null, readOnly: null as string | null, saves: [] as { file: string; text: string; revision: string | null }[], lose: false, loseAfterWrite: false, refuse: null as string | null };
  const put = (file: string, text: string) => { state.files[file] = text; state.revisions[file] = rev(++revision); };
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
    "native.reveal_file": () => ({ outcome: "opened" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params: { file: string; text: string; revision: string | null; leaseToken?: string } };
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
      if (op === "repo.file.lease.acquire") {
        if (state.writer) return { ok: false, code: "task_file_lease_held", holderTaskId: state.writer };
        return { ok: true, result: { leaseToken: token, expiresInMs: 60000, renewAfterMs: 20000 } };
      }
      if (op === "repo.file.lease.renew") return { ok: true, result: { expiresInMs: 60000 } };
      if (op === "repo.file.lease.release") return { ok: true, result: { released: true } };
      if (op === "repo.file.save") {
        if (params.leaseToken !== token || state.writer) return { ok: false, code: "edit_lease_lost" };
        state.saves.push({ file: params.file, text: params.text, revision: params.revision });
        if (state.lose) throw new Error("service_timeout");
        if (state.refuse) return { ok: false, code: "refused", message: state.refuse };
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
async function open(d: ReturnType<typeof disk>, path = CODE(), chrome?: WindowChrome, localCode = true) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace chrome={chrome} port={createRealPort({ transport: d.transport, actorId: "owner", registry: true, localCode })} />, root);
  for (let i = 0; i < 40 && !document.querySelector(".cm-content"); i++) await settled();
  await settled();
}
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim() === text || b.getAttribute("aria-label") === text);
const fileActions = () => {
  const trigger = $<HTMLButtonElement>('.ecrumb button[aria-label="File actions"]')!;
  trigger.click();
  const menu = document.getElementById(trigger.getAttribute("popovertarget")!)!;
  expect(menu.getAttribute("role")).toBe("menu");
  return [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
};
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
    expect($(".cm-content")!.getAttribute("aria-label")).toBe("Read a.txt");
    type("cannot type");
    expect(d.calls.some(c => (c.message as { op?: string })?.op?.startsWith("repo.file.lease."))).toBe(false);
    await click("Edit");
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
    expect(d.state.saves).toEqual([{ file: "a.txt", text: "one\ntwo\nthree\n", revision: rev(1) }]);
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nthree\n");
    expect(status()).toBe("Saved");
    expect(tab()).toBe("a.txt");
    type("four\n");
    await settled();
    await click("Save a.txt");
    expect(d.state.saves.at(-1)!.revision).toBe(rev(3));
  });

  it("saves with Ctrl+S from the editor", async () => {
    const d = disk();
    await open(d);
    await click("Edit");
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
    await click("Edit");
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
    expect(status()).toBe("Read-only");
    await click("Edit");
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
    await click("Edit");
    type("mine\n");
    await settled();
    d.put("a.txt", "worker\n");
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBe("worker\n");
    expect(tab()).toBe("a.txt, unsaved");
    expect($(".strip.warn[role=alert]")!.textContent).toContain("Your draft is kept");
    await click("Keep mine");
    expect(d.state.saves.at(-1)).toEqual({ file: "a.txt", text: "one\ntwo\nmine\n", revision: rev(3) });
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
    expect($(".strip.warn[role=alert]")).toBeNull();
    expect(status()).toBe("Saved");
  });

  it("a read that started before a save cannot undo it on screen", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const d = disk();
    await open(d);
    await click("Edit");
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
    await click("Edit");
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
    await click("Edit");
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
    await click("Edit");
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
  it("shows the task holding the file and retries only on Edit", async () => {
    const d = disk();
    d.state.writer = "task-1";
    await open(d);
    await click("Edit");
    expect($(".strip.warn")!.textContent).toContain("Task Fix the login race is editing this file.");
    expect(view().state.readOnly).toBe(true);
    expect(d.state.saves).toEqual([]);
    d.state.writer = null;
    await click("Edit");
    expect(view().state.readOnly).toBe(false);
  });

  it("locks on lease loss at Save, preserving text for an explicit retry", async () => {
    const d = disk();
    await open(d);
    await click("Edit");
    type("mine\n");
    await settled();
    d.state.writer = "another-task";
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\n");
    expect(tab()).toBe("a.txt, unsaved");
    expect(view().state.readOnly).toBe(true);
    expect($(".strip.warn")!.textContent).toContain("Edit lease lost");
    d.state.writer = null;
    await click("Edit");
    await click("Save a.txt");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
  });

  it("renews after 20 seconds and locks with the draft intact when renewal stalls past the deadline", async () => {
    const d = disk();
    const machine = d.answers["machine.call"]!;
    const renewals = vi.fn();
    let finishRenewal!: (reply: unknown) => void;
    const stalled = new Promise(resolve => { finishRenewal = resolve; });
    const renewed = { ok: true, result: { expiresInMs: 60000 } };
    d.answers["machine.call"] = message => {
      const { op, params } = message as { op: string; params: unknown };
      if (op !== "repo.file.lease.renew") return machine(message);
      renewals(params);
      return renewals.mock.calls.length === 1 ? renewed : stalled;
    };
    await open(d);
    vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
    button("Edit")!.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(view().state.readOnly).toBe(false);
    type("my draft\n");

    await vi.advanceTimersByTimeAsync(19999);
    expect(renewals).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(renewals).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
      file: "a.txt", checkoutId: "p:main", leaseToken: token,
    }));
    await vi.advanceTimersByTimeAsync(20000);
    expect(renewals).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(39999);
    expect(view().state.readOnly).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(view().state.readOnly).toBe(true);
    expect($(".strip.warn")!.textContent).toContain("Edit lease lost");
    expect(tab()).toBe("a.txt, unsaved");
    expect(doc()).toBe("one\ntwo\nmy draft\n");

    finishRenewal(renewed);
    await vi.advanceTimersByTimeAsync(0);
    expect(view().state.readOnly).toBe(true);
    expect(d.calls.some(c => (c.message as { op?: string })?.op === "repo.file.save")).toBe(false);
    expect(d.state.files["a.txt"]).toBe("one\ntwo\n");
  });

  it("keeps a nonlocal shell read-only without lease or language requests", async () => {
    const d = disk();
    await open(d, CODE(), undefined, false);
    expect(button("Edit")).toBeUndefined();
    expect(button("Save a.txt")).toBeUndefined();
    expect(d.calls.some(c => /repo\.(lsp|file\.lease)\./.test(String((c.message as { op?: string })?.op)))).toBe(false);
  });
});

it("shows local diagnostics with marks and a list and opens a definition via F12", async () => {
  const d = disk({ "sample.rs": "fn main() {}\n", "b.txt": "target\n" });
  const call = d.answers["machine.call"]!;
  const caps = d.answers["service.capabilities"]!;
  d.answers["service.capabilities"] = msg => ({ ...(caps(msg) as object), codeLsp: true });
  const eventId = "b".repeat(32), documentId = "c".repeat(64);
  const range = { start: { line: 0, character: 3 }, end: { line: 0, character: 7 } };
  const snapshot = { eventId, version: 1, generation: 1, status: "ready", diagnostics: [{ range, message: "Example warning", severity: 2 }] };
  d.answers["machine.call"] = message => {
    switch ((message as { op: string }).op) {
      case "repo.lsp.open": return { ok: true, result: { ...snapshot, documentId } };
      case "repo.lsp.status": return { ok: true, result: snapshot };
      case "repo.lsp.definition": return { ok: true, result: { version: 1, generation: 1, targets: [{ checkoutId: "p:main", file: "b.txt", range }] } };
      case "repo.lsp.close": return { ok: true, result: { closed: true } };
      default: return call(message);
    }
  };
  await open(d, CODE("sample.rs"));
  expect(view().state.readOnly).toBe(true);
  expect($(".ws-code-diagnostics")?.textContent).toContain("Warning: Example warning");
  expect($(".cm-diag-range")?.textContent).toBe("main");
  expect($(".cm-diag-gutter .cm-diag-note")?.textContent).toBe("!");
  $(".cm-content")!.dispatchEvent(new KeyboardEvent("keydown", { key: "F12", bubbles: true, cancelable: true }));
  await until("target\n");
  expect(doc()).toBe("target\n");
  expect($(".ws-code-diagnostics")).toBeNull();
  expect(d.calls.some(c => (c.message as { op?: string })?.op === "repo.lsp.close")).toBe(true);
});

describe("owner text is never lost", () => {
  it("a lost reply stays unconfirmed, never clean, and is not sent again on its own", async () => {
    const d = disk();
    await open(d);
    await click("Edit");
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
    await click("Edit");
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
    await click("Edit");
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
    await click("Edit");
    type("x".repeat(512 * 1024 + 1));
    await settled();
    await click("Save a.txt");
    expect(d.state.saves).toEqual([]);
    expect($(".strip.bad[role=alert]")!.textContent).toContain("too large to save here");
  });

  it("closing a dirty tab asks first, and Cancel keeps it", async () => {
    const d = disk();
    await open(d);
    await click("Edit");
    type("mine\n");
    await settled();
    await click("Close a.txt");
    expect($("dialog[open]")!.getAttribute("data-dialog")).toBe("Unsaved changes");
    // Named by its visible title.
    expect(document.getElementById($("dialog[open]")!.getAttribute("aria-labelledby")!)?.textContent).toBe("Save changes to a.txt?");
    await click("Cancel");
    expect(doc()).toBe("one\ntwo\nmine\n");
    await click("Close a.txt");
    await click("Save");
    expect(d.state.files["a.txt"]).toBe("one\ntwo\nmine\n");
  });

  it("switching files and leaving the screen keep the text, its tab and its undo", async () => {
    const d = disk();
    await open(d);
    await click("Edit");
    type("mine\n");
    await settled();
    all<HTMLButtonElement>(".tn").find(b => b.textContent?.includes("b.txt"))!.click();
    await until("bee\n");
    expect(doc()).toBe("bee\n");
    expect(all(".etab").map(t => t.getAttribute("aria-label"))).toContain("a.txt, unsaved");
    all(".etab").find(t => t.textContent?.includes("a.txt"))!.click();
    await until("one\ntwo\nmine\n");
    expect(doc()).toBe("one\ntwo\nmine\n");
    expect(view().state.readOnly).toBe(true);
    expect(d.calls.some(c => (c.message as { op?: string })?.op === "repo.file.lease.release")).toBe(true);
    await click("Edit");
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
    await click("Edit");
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
    await click("Edit");
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
    await click("Edit");
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
    await click("Edit");
    type("x");
    await new Promise(resolve => setTimeout(resolve, 1200));
    expect(d.state.saves).toEqual([]);
    dispose?.(); dispose = undefined; document.body.innerHTML = "";
    localStorage.setItem("cw.ws.autosave", "true");
    const e = disk();
    await open(e);
    await click("Edit");
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
    await click("Edit");
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
  it("puts exactly three commands in one file actions menu and opens the file palette", async () => {
    await open(disk());
    expect(all(".ecrumb button[popovertarget]")).toHaveLength(1);
    const labels = all(".ecrumb button").map(b => b.getAttribute("aria-label") ?? b.textContent?.trim());
    expect(labels).not.toContain("Go to file");
    expect(labels).not.toContain("Open in VS Code");
    expect(labels).not.toContain("Open in default app");
    const items = fileActions();
    expect(items).toHaveLength(3);
    expect(items.map(item => item.querySelector(".grow")?.firstChild?.textContent)).toEqual([
      "Go to file", "Open in VS Code", "Show in file manager",
    ]);
    items[0].click();
    await settled();
    expect($('dialog[open] input[aria-label="Go to file"]')).not.toBeNull();
  });

  it("says why when this computer has no openers and dispatches neither command", async () => {
    const d = disk();
    await open(d);
    const [, external, reveal] = fileActions();
    expect(external.getAttribute("aria-disabled")).toBe("true");
    expect(external.dataset.why).toContain("No external editor was found");
    expect(reveal.getAttribute("aria-disabled")).toBe("true");
    expect(reveal.dataset.why).toContain("No file manager or folder opener was found");
    external.click();
    reveal.click();
    await settled();
    expect(d.calls.some(c => ["native.open_editor", "native.reveal_file"].includes(c.operation))).toBe(false);
  });

  it("opens the checkout's file at its line in VS Code", async () => {
    const d = disk(undefined, ["native.open_editor", "native.open_editor.vscode", "native.open_logs"]);
    await open(d, CODE() + "&line=2");
    const [, external] = fileActions();
    expect(external.textContent).toBe("Open in VS Code");
    external.click();
    await settled();
    expect(d.calls.filter(c => c.operation === "native.open_editor").map(c => c.message)).toEqual([{ projectId: "p", checkoutId: "p:main", file: "a.txt", line: 2 }]);
  });

  it("reveals exactly the current checkout file without a line", async () => {
    const d = disk(undefined, ["native.reveal_file"]);
    await open(d, CODE("b.txt") + "&line=1");
    const [, external, reveal] = fileActions();
    expect(external.getAttribute("aria-disabled")).toBe("true");
    expect(reveal.getAttribute("aria-disabled")).toBeNull();
    reveal.click();
    await settled();
    expect(d.calls.filter(c => c.operation === "native.reveal_file").map(c => c.message)).toEqual([{ projectId: "p", checkoutId: "p:main", file: "b.txt" }]);
  });

  it("names the default application when only a system opener exists", async () => {
    const d = disk(undefined, ["native.open_editor"]);
    await open(d);
    const [, external] = fileActions();
    expect(external.textContent).toBe("Open in default app");
    external.click();
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
