// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Message, type ReviewComment, type Task, type WorkspaceCommand } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { Composer } from "../src/composer";
import { COPY } from "../src/copy";
import { WsContext, type Ws } from "../src/ctx";
import { Diff } from "../src/diff";
import { ReviewDialogs, type WsDialog } from "../src/dialogs";
import { createRealPort } from "../src/port/real";
import { sessionModel } from "../src/session-model";
import { createSettingsState } from "../src/settings-state";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, draftTask, flush, scriptedPort, scriptedTransport, snapshot, wireDecision, wireTask } from "./support";

const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"];
const png = { name: "clip.png", mimeType: "image/png", data: btoa("fixture png") };
const NEW_TASK = `/m/${LOCAL_MACHINE_ID}/new?project=p`, NEW_SPRINT = `${NEW_TASK}&kind=sprint`, TASK = `/m/${LOCAL_MACHINE_ID}/t/task-1`;

type Sent = Record<string, unknown>;
function machine(options: { attachments?: boolean; started?: boolean; product?: { sprints?: Sent[]; messages?: Partial<Message>[] }; history?: Sent[]; saved?: typeof png[]; nativeClipboard?: typeof png[]; decisions?: Sent[]; workers?: Sent[] } = {}) {
  const product = { tasks: {}, sprints: (options.product?.sprints ?? []) as Sent[], messages: (options.product?.messages ?? []) as Partial<Message>[] };
  const history = options.history ?? [];
  const config = structuredClone(emptyWorkspace().machines[0].config);
  config.surfaces.user_defaults = { ...config.surfaces.user_defaults, coordinator: { engine: "claude-agent-acp", model: "opus", effort: "high", parameters: {} }, mode: "auto", language: "ru" };
  config.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus", efforts: ["medium", "high"], observed: true }];
  config.setup["claude-agent-acp"].status = "connected";
  config.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
  const tasks = options.started === false ? [wireTask()] : [wireTask({ executionId: "e", phase: "execution", runtimeState: "EXECUTING", executionState: "running" })];
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call", "executions.control", "native.clipboard_image", "native.take_dropped_files", "native.save_attachment"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => ({ ...capabilitiesWire(OFFERED), attachments: options.attachments !== false }),
    "project.snapshot": () => snapshot(String(4 + history.length), tasks),
    "decisions.list": () => ({ items: options.decisions ?? [], nextCursor: null, highWatermark: "4" }),
    "decisions.answer": message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "d", entityVersion: "3", recordedAt: "2026-11-09T00:00:00Z" }),
    "history.page": () => ({ items: history, nextCursor: null, highWatermark: String(history.length) }),
    "executions.get": () => ({ executionId: "e", taskId: "task-1", version: "2", runtimeTaskId: "r", acpSessionId: "s", runtimePlanRevision: "0", ownerEpoch: "3", state: "running", warnings: [], resultId: null }),
    "executions.prompt": () => ({ outcome: "recorded" }),
    "executions.control": () => ({ outcome: "recorded" }),
    "tasks.create": message => ({ requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "task-2", entityVersion: "1", recordedAt: "2026-11-09T00:00:00Z" }),
    "native.clipboard_image": () => options.nativeClipboard ?? [],
    "native.take_dropped_files": () => [],
    "native.save_attachment": () => ({ path: "/home/owner/clip.png" }),
    "machine.call": message => {
      const { op, params } = message as { op: string; params?: Sent & { action?: string; message?: Partial<Message>; messageId?: string; attachments?: boolean } };
      if (op === "product.read") return { ok: true, result: params?.attachments ? options.saved ?? [] : product };
      if (op === "product.message" && params?.action === "add") { product.messages.push(params.message!); return { ok: true, result: {} }; }
      if (op === "product.message" && params?.action === "remove") { product.messages = product.messages.filter(m => m.id !== params.messageId); return { ok: true, result: {} }; }
      if (op === "product.sprint" && params?.action === "create") { product.sprints.push({ id: "sp", projectId: "p", description: params.description, attachments: params.attachments, title: "S", planning: "planning", runMode: "manual", targetBranch: null, conversation: [], plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null }); return { ok: true, result: { sprintId: "sp" } }; }
      if (op === "repo.snapshot") return { ok: true, result: { projectId: "p", remote: "", tree: [], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "main", defaultBranch: "main", checkouts: [] } };
      if (op === "workers") return { ok: true, result: options.workers ?? [] };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      return { ok: true, result: {} };
    },
  });
  return s;
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
async function open(s: ReturnType<typeof machine>, path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  await settled();
}
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
// A text button by its words, or an icon button, such as the paper plane, by its accessible name.
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim().startsWith(text) || b.getAttribute("aria-label") === text)!;
const fill = (label: string, value: string) => { const el = $<HTMLTextAreaElement>(`[aria-label="${label}"]`)!; el.value = value; el.dispatchEvent(new Event("input", { bubbles: true })); };
const calls = (s: ReturnType<typeof machine>, op: string) => s.calls.filter(c => c.operation === op);
const machineCalls = (s: ReturnType<typeof machine>, op: string) => s.calls.filter(c => c.operation === "machine.call" && (c.message as { op: string }).op === op).map(c => (c.message as { params: Sent }).params);

/** The clipboard a real paste event carries in WebKitGTK. */
function paste(target: Element, clipboard: { types?: string[]; files?: File[]; items?: unknown[]; text?: string }) {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: { types: clipboard.types ?? [], files: clipboard.files ?? [], items: clipboard.items ?? [], getData: (type: string) => type === "text/plain" ? clipboard.text ?? "" : "" } });
  target.dispatchEvent(event);
  return event;
}
function pick(form: ParentNode, files: File[]) {
  const input = form.querySelector<HTMLInputElement>('input[type="file"]')!;
  Object.defineProperty(input, "files", { configurable: true, value: files });
  input.dispatchEvent(new Event("change", { bubbles: true }));
}
const field = (label: string) => $<HTMLTextAreaElement>(`[aria-label="${label}"]`)!;
const alertText = (scope: ParentNode = document) => scope.querySelector('[role="alert"]')?.textContent ?? "";

describe("pasting into a form", () => {
  it("attaches a clipboard image the page sees only as its text/html, the case WebKitGTK reports for a browser copy", async () => {
    const s = machine({ nativeClipboard: [png] });
    await open(s, NEW_TASK);
    const event = paste(field("Describe the task"), { types: ["text/html"] });
    await settled();
    expect(event.defaultPrevented).toBe(true);
    expect(calls(s, "native.clipboard_image")).toHaveLength(1);
    const chip = $(".composer .ws-att")!;
    expect(chip.textContent).toContain("clip.png");
    expect(chip.querySelector<HTMLImageElement>("img.ws-att-thumb")?.src).toBe(`data:image/png;base64,${png.data}`);
    $<HTMLElement>('[aria-label="Remove clip.png"]')!.click();
    expect($(".composer .ws-att")).toBeNull();
  });

  it("attaches an image found only in clipboard items", async () => {
    const s = machine();
    await open(s, NEW_TASK);
    const file = new File(["png"], "", { type: "image/png" });
    paste(field("Describe the task"), { types: ["Files"], items: [{ kind: "file", type: "image/png", getAsFile: () => file }] });
    await settled();
    expect($(".composer .ws-att")?.textContent).toContain("pasted.png");
    expect(calls(s, "native.clipboard_image")).toHaveLength(0);
  });

  it("leaves a text paste alone", async () => {
    const s = machine();
    await open(s, NEW_TASK);
    const event = paste(field("Describe the task"), { types: ["text/plain"], text: "plain words" });
    await settled();
    expect(event.defaultPrevented).toBe(false);
    expect(calls(s, "native.clipboard_image")).toHaveLength(0);
    expect($(".composer .ws-att")).toBeNull();
    expect(alertText()).toBe("");
  });

  it("says so when a non-text paste has no image, and clears the message on the next good attach", async () => {
    const s = machine();
    await open(s, NEW_TASK);
    paste(field("Describe the task"), { types: ["text/uri-list"] });
    await settled();
    expect(alertText()).toContain("no image to attach");
    pick($(".composer")!, [new File(["x"], "a.txt", { type: "text/plain" })]);
    await settled();
    expect(alertText()).toBe("");
    expect($(".composer .ws-att")?.textContent).toContain("a.txt");
  });

  it("shows a message instead of swallowing a paste or drop when attachments are off", async () => {
    const s = machine({ attachments: false, nativeClipboard: [png] });
    await open(s, NEW_TASK);
    paste(field("Describe the task"), { types: [] });
    await settled();
    expect(alertText()).toBe("Attachments are not available on this machine.");
    expect($(".composer .ws-att")).toBeNull();
  });

  it("does not swallow a drop silently either", async () => {
    const s = machine({ attachments: false });
    await open(s, NEW_TASK);
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", { value: { files: [new File(["x"], "a.txt")], types: ["Files"] } });
    field("Describe the task").dispatchEvent(drop);
    await settled();
    expect(alertText()).toContain("not available");
  });
});

describe("invalid files", () => {
  it.each([
    ["too many", () => Array.from({ length: 9 }, (_, i) => new File(["x"], `f${i}.txt`)), "Attach at most 8 files."],
    ["too large", () => [new File([new Uint8Array(4 * 1024 * 1024 + 1)], "big.bin")], "Attachments must total 4 MiB or less."],
    ["a name that cannot be sent", () => [new File(["x"], "bad\u0007name.png", { type: "image/png" })], "Rename the file"],
    ["a type that is not recognized", () => [new File(["x"], "odd.bin", { type: "not a type" })], "its file type is not recognized"],
  ])("%s gets a clear message next to the form", async (_name, files, message) => {
    const s = machine();
    await open(s, NEW_TASK);
    pick($(".composer")!, files());
    await settled();
    expect(alertText($(".composer")!)).toContain(message);
    expect($(".composer .ws-att")).toBeNull();
  });

  it("a file that cannot be read says so", async () => {
    const read = vi.spyOn(FileReader.prototype, "readAsDataURL").mockImplementation(function (this: FileReader) { queueMicrotask(() => this.onerror?.(new ProgressEvent("error") as ProgressEvent<FileReader>)); });
    try {
      const s = machine();
      await open(s, NEW_TASK);
      pick($(".composer")!, [new File(["x"], "gone.txt", { type: "text/plain" })]);
      await settled();
      expect(alertText($(".composer")!)).toBe("“gone.txt” could not be read. Choose it again.");
      expect($(".composer .ws-att")).toBeNull();
    } finally { read.mockRestore(); }
  });
});

describe("the new task and new sprint forms", () => {
  it("creates a task with what was pasted", async () => {
    const s = machine({ nativeClipboard: [png] });
    await open(s, NEW_TASK);
    fill("Describe the task", "Fix the login");
    paste(field("Describe the task"), { types: [] });
    await settled();
    button("Save draft").click();
    await settled();
    const sent = calls(s, "tasks.create")[0].message as { payload: { description: string; attachments: unknown[] } };
    expect(sent.payload).toMatchObject({ description: "Fix the login", attachments: [png] });
  });

  it("creates a sprint with real files and no filename line in its description", async () => {
    const s = machine();
    await open(s, NEW_SPRINT);
    fill("Describe the sprint", "Plan the release");
    pick($(".composer")!, [new File(["bytes"], "brief.png", { type: "image/png" })]);
    await settled();
    expect($(".composer .ws-att")?.textContent).toContain("brief.png");
    button("Start planning").click();
    await settled();
    const created = machineCalls(s, "product.sprint").find(c => c.action === "create")!;
    expect(created.description).toBe("Plan the release");
    expect(created.attachments).toEqual([{ name: "brief.png", mimeType: "image/png", data: btoa("bytes") }]);
    expect(String(created.description)).not.toContain("Attached:");
  });
});

describe("after sending", () => {
  it("opens a saved task's file and saves a copy through the desktop", async () => {
    const s = machine({ started: false, saved: [png] });
    await open(s, TASK);
    const open_ = $<HTMLButtonElement>('.brief [aria-label="Open clip.png"]')!;
    expect(open_).not.toBeNull();
    open_.click();
    await settled();
    expect($<HTMLImageElement>("dialog[open] img.ws-att-big")?.src).toBe(`data:image/png;base64,${png.data}`);
    button("Save a copy").click();
    await settled();
    expect(calls(s, "native.save_attachment")[0].message).toEqual({ name: "clip.png", data: png.data });
    expect($("dialog[open]")?.textContent).toContain("Saved to /home/owner/clip.png.");
  });

  it("keeps a sent message's files where the conversation can open them again", async () => {
    const s = machine();
    await open(s, TASK);
    fill("Message", "See this");
    pick($(".dock")!, [new File(["bytes"], "shot.png", { type: "image/png" })]);
    await settled();
    button("Send message now").click();
    await settled();
    expect(calls(s, "executions.prompt")[0].message).toMatchObject({ text: "See this", attachments: [{ name: "shot.png" }] });
    expect(machineCalls(s, "product.message").find(c => c.action === "add")?.message).toMatchObject({ delivery: "recorded", text: "See this", attachments: [{ name: "shot.png" }] });
  });

  it("a message's saved file opens in the feed; a bare name stays plain", async () => {
    const message = (id: string, text: string, attachments: Message["attachments"]): Partial<Message> => ({ id, taskId: "task-1", workerId: null, text, mode: "queue", delivery: "recorded", at: 1, deliveredAt: null, failure: null, attachments });
    const s = machine({ product: { messages: [message("m1", "with bytes", [png]), message("m2", "name only", ["worker-note.txt"])] } });
    await open(s, TASK);
    expect($<HTMLButtonElement>('.mine [aria-label="Open clip.png"]')).not.toBeNull();
    expect([...document.querySelectorAll(".mine .ws-att")].some(el => el.textContent?.includes("worker-note.txt"))).toBe(true);
    expect($('.mine [aria-label="Open worker-note.txt"]')).toBeNull();
    // A name with no kept bytes is not offered as a file: pressing it says why it cannot open.
    const bare = $<HTMLElement>('.mine [aria-label="worker-note.txt, name only"]')!;
    expect(bare.getAttribute("aria-disabled")).toBe("true");
    expect(bare.dataset.why).toContain("Only the name");
    expect(bare.classList.contains("ws-att-open")).toBe(false);
  });

  it("a queued message shows no files in its row, and Send now sends them and leaves them openable in the conversation", async () => {
    const s = machine();
    await open(s, TASK);
    fill("Message", "Сообщение в очереди с картинкой");
    pick($(".dock")!, [new File(["bytes"], "clipboard.png", { type: "image/png" })]);
    await settled();
    button("Queue").click();
    button("Queue message").click();
    await settled();
    const row = $(".queue .qrow")!;
    expect(row.textContent).toContain("Сообщение в очереди с картинкой");
    expect(row.querySelector(".ws-att, .ws-atts")).toBeNull();
    expect(row.textContent).not.toContain("clipboard.png");
    expect(machineCalls(s, "product.message").find(c => c.action === "add")?.message).toMatchObject({ delivery: "queued", attachments: [{ name: "clipboard.png" }] });
    button("Send now").click();
    await settled();
    expect(calls(s, "executions.prompt")[0].message).toMatchObject({ text: "Сообщение в очереди с картинкой", attachments: [{ name: "clipboard.png", mimeType: "image/png", data: btoa("bytes") }] });
    expect($(".queue")).toBeNull();
    const opener = $<HTMLButtonElement>('.mine [aria-label="Open clipboard.png"]')!;
    expect(opener).not.toBeNull();
    opener.click();
    await settled();
    expect($<HTMLImageElement>("dialog[open] img.ws-att-big")?.src).toBe(`data:image/png;base64,${btoa("bytes")}`);
  });

  it("a file in the conversation is a real button the keyboard reaches and presses", async () => {
    const s = machine({ product: { messages: [{ id: "m1", taskId: "task-1", workerId: null, text: "with bytes", mode: "queue", delivery: "recorded", at: 1, deliveredAt: null, failure: null, attachments: [png] }] } });
    await open(s, TASK);
    const opener = $<HTMLButtonElement>('.mine [aria-label="Open clip.png"]')!;
    // A native button: Enter and Space press it as a click does, so the open is the same press.
    expect(opener.tagName).toBe("BUTTON");
    expect(opener.disabled).toBe(false);
    expect(opener.tabIndex).toBe(0);
    expect(opener.getAttribute("aria-disabled")).toBeNull();
    opener.focus();
    expect(document.activeElement).toBe(opener);
    opener.click();
    await settled();
    expect($<HTMLImageElement>("dialog[open] img.ws-att-big")?.src).toBe(`data:image/png;base64,${png.data}`);
  });

  it("does not fail a delivered message when the copy could not be kept", async () => {
    const s = machine();
    const answer = s.answers["machine.call"]!;
    s.answers["machine.call"] = message => (message as { op: string }).op === "product.message" ? { ok: false, code: "queue_full", message: "full" } : answer(message);
    await open(s, TASK);
    fill("Message", "See this");
    pick($(".dock")!, [new File(["bytes"], "shot.png", { type: "image/png" })]);
    await settled();
    button("Send message now").click();
    await settled();
    expect(calls(s, "executions.prompt")).toHaveLength(1);
    expect(field("Message").value).toBe("");
  });

  it("lists a sprint's files on its page, and they open", async () => {
    const s = machine({ product: { sprints: [{ id: "sp", projectId: "p", description: "Plan", attachments: [png], title: "S", planning: "planning", runMode: "manual", targetBranch: null, conversation: [], plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null }] } });
    await open(s, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`);
    expect($('.brief [aria-label="Open clip.png"]')).not.toBeNull();
    fill("Message the planner", "Use this too");
    pick($(".dock")!, [new File(["bytes"], "more.txt", { type: "text/plain" })]);
    await settled();
    expect($(".dock .composer .ws-att")?.textContent).toContain("more.txt");
    button("Send to the planner"); // Present before sending: the form takes attachments.
    expect($('[aria-label="Send to the planner"]')).not.toBeNull();
  });
});

describe("every message form sends its files", () => {
  it("a task question's free-form answer, over several lines, carries its files", async () => {
    const s = machine({ decisions: [wireDecision({ prompt: "Which build?" })] });
    await open(s, TASK);
    // The answer composer sits in the question itself; no separate Other answer step opens it.
    fill("Your answer", "Use the nightly\nand keep the logs");
    pick($(".dc-answer")!, [new File(["bytes"], "log.txt", { type: "text/plain" })]);
    await settled();
    expect($(".dc-answer .ws-att")?.textContent).toContain("log.txt");
    button("Send answer").click();
    await settled();
    const sent = calls(s, "decisions.answer")[0].message as { payload: { answers: unknown[] } };
    expect(sent.payload.answers[0]).toMatchObject({ text: "Use the nightly\nand keep the logs", attachments: [{ name: "log.txt", mimeType: "text/plain", data: btoa("bytes") }] });
  });

  it("a planning answer and a message to the planner carry their files", async () => {
    const sprint = { id: "sp", projectId: "p", description: "Plan", title: "S", planning: "question", runMode: "manual", targetBranch: null, conversation: [{ kind: "question", text: "Which area first?", options: ["UI"], at: 1 }], plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null };
    const s = machine({ product: { sprints: [sprint] } });
    await open(s, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`);
    fill("Your answer", "Start with the API");
    pick($(".dc-answer")!, [new File(["a"], "api.png", { type: "image/png" })]);
    await settled();
    button("Send answer").click();
    await settled();
    const notes = () => machineCalls(s, "product.sprint").filter(c => c.action === "note");
    expect(notes()[0]).toMatchObject({ kind: "answer", text: "Start with the API", attachments: [{ name: "api.png", data: btoa("a") }] });
    fill("Message the planner", "And this");
    pick($(".dock")!, [new File(["b"], "more.txt", { type: "text/plain" })]);
    await settled();
    $<HTMLElement>('[aria-label="Send to the planner"]')!.click();
    await settled();
    expect(notes().at(-1)).toMatchObject({ kind: "message", text: "And this", attachments: [{ name: "more.txt", data: btoa("b") }] });
    expect($(".dock .composer .ws-att")).toBeNull();
  });

  it("a message to one worker carries its files", async () => {
    const worker = { id: "w1", taskId: "task-1", executionId: "e", role: "Builder", status: "working", lane: "execution", goal: "Build", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null };
    const s = machine({ workers: [worker] });
    await open(s, `${TASK}/workers/w1`);
    fill("Message Builder", "Check the log");
    pick(field("Message Builder").closest(".composer")!, [new File(["l"], "w.png", { type: "image/png" })]);
    await settled();
    $<HTMLElement>('[aria-label="Send to Builder"]')!.click();
    await settled();
    const sent = [...calls(s, "executions.control"), ...calls(s, "executions.prompt")].map(c => c.message as { attachments?: { name: string }[] });
    expect(sent.some(m => m.attachments?.[0]?.name === "w.png")).toBe(true);
  });
});

describe("dialogs and comments", () => {
  function mount(options: { status?: Task["status"]; messages?: Message[]; comments?: ReviewComment[]; composer?: boolean } = {}) {
    const state = emptyWorkspace(), cap = { everything: true, operations: [] };
    state.machines[0].connection = "connected";
    state.projects.push({ id: "p", machineId: state.machines[0].id, name: "Project", repository: "/p", openBranch: "dev", defaultBranch: "main" });
    state.messages.push(...(options.messages ?? []));
    state.tasks.push({ ...draftTask(), status: options.status ?? "review", checkout: { kind: "main", branch: "dev" }, result: { version: 1, digest: "d", summary: "R", checks: [], acceptanceAvailable: true, at: null, warnings: [] } as never });
    const script = scriptedPort({ capabilities: cap });
    let openDialog!: (d: WsDialog) => void;
    const root = document.createElement("div"); document.body.append(root);
    dispose = render(() => {
      const store = createWorkspaceStore(script.port, { initial: { state, version: "1", capabilities: cap } });
      const [dialog, setDialog] = createSignal<WsDialog | null>(null);
      const ws = { store, dialog, open: setDialog, closeDialog: () => setDialog(null), run: store.dispatch,
        ok(command: Parameters<Ws["ok"]>[0], then: Parameters<Ws["ok"]>[1]) { void store.dispatch(command).then(r => { if (r.ok) then(r); }); },
        busy: <T,>(work: Promise<T>) => work, needs: () => ({}), gate: () => undefined, can: () => true,
        machine: () => store.state().machines[0], settings: createSettingsState(), t: (key: keyof typeof COPY) => COPY[key], route: () => null, newId: () => "new", go: vi.fn(),
        drafts: new Map<string, string>(), stopTask: vi.fn(),
      } as unknown as Ws;
      openDialog = setDialog;
      return <WsContext.Provider value={ws}><ReviewDialogs /><div id="diff"><Diff file={{ path: "a.ts", hunks: [{ header: "@@", lines: [{ kind: "add", old: null, new: 1, text: "x" }] }] } as never} layout="unified" taskId="t" comments={options.comments ?? []} /></div>
        {options.composer ? <Composer model={() => sessionModel(store.index(), "t")!} onHeight={() => undefined} /> : null}</WsContext.Provider>;
    }, root);
    return { script, open: (d: WsDialog) => openDialog(d) };
  }
  const commandOf = (script: ReturnType<typeof scriptedPort>, type: WorkspaceCommand["type"]) => script.commands.map(c => c.payload as WorkspaceCommand).find(c => c.type === type) as unknown as { attachments?: { name: string }[] };

  it("request changes sends its files", async () => {
    const h = mount();
    h.open({ kind: "changes", taskId: "t" });
    const dlg = $("dialog[open]")!;
    const box = dlg.querySelector<HTMLTextAreaElement>("textarea")!;
    box.value = "Fix this"; box.dispatchEvent(new Event("input", { bubbles: true }));
    pick(dlg, [new File(["bytes"], "fix.png", { type: "image/png" })]);
    await settled();
    expect(dlg.querySelector(".ws-att")?.textContent).toContain("fix.png");
    button("Send and reopen").click();
    await settled();
    expect(commandOf(h.script, "request_changes").attachments?.map(a => a.name)).toEqual(["fix.png"]);
  });

  it("a diff comment attaches and saves its file", async () => {
    const h = mount();
    $<HTMLElement>('[aria-label="Comment on line 1"]')!.click();
    await settled();
    const form = $(".cmt")!;
    const box = form.querySelector<HTMLTextAreaElement>("textarea")!;
    box.value = "Look here"; box.dispatchEvent(new Event("input", { bubbles: true }));
    paste(box, { types: ["Files"], files: [new File(["bytes"], "note.png", { type: "image/png" })] });
    await settled();
    expect(form.querySelector(".ws-att")?.textContent).toContain("note.png");
    button("Save comment").click();
    await settled();
    expect(commandOf(h.script, "add_comment").attachments?.map(a => a.name)).toEqual(["note.png"]);
  });

  it("conflict guidance sends its files", async () => {
    const h = mount();
    h.open({ kind: "cortex", taskId: "t" });
    const dlg = $("dialog[open]")!;
    const box = dlg.querySelector<HTMLTextAreaElement>("textarea")!;
    box.value = "Keep the tz argument"; box.dispatchEvent(new Event("input", { bubbles: true }));
    paste(box, { types: ["Files"], files: [new File(["bytes"], "both.png", { type: "image/png" })] });
    await settled();
    expect(dlg.querySelector(".ws-att")?.textContent).toContain("both.png");
    [...dlg.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === "Resolve automatically")!.click();
    await settled();
    expect(commandOf(h.script, "resolve_with_cortex")).toMatchObject({ guidance: "Keep the tz argument", attachments: [{ name: "both.png" }] });
  });

  it("editing a diff comment keeps, removes and adds files", async () => {
    const old = { name: "old.txt", mimeType: "text/plain", data: btoa("old") };
    const h = mount({ comments: [{ id: "c1", path: "a.ts", line: 1, text: "Look", sent: false, attachments: [png, old] } as never] });
    expect($('.cmt [aria-label="Open clip.png"]')).not.toBeNull();
    button("Edit").click();
    await settled();
    $<HTMLElement>('.cmt [aria-label="Remove old.txt"]')!.click();
    pick($(".cmt")!, [new File(["n"], "new.png", { type: "image/png" })]);
    await settled();
    button("Save comment").click();
    await settled();
    expect(commandOf(h.script, "edit_comment").attachments?.map(a => a.name)).toEqual(["clip.png", "new.png"]);
  });

  const dock = () => $(".dock")!;
  const typeAndAttach = async (name: string) => {
    fill("Message", "See this");
    pick(dock(), [new File(["bytes"], name, { type: "image/png" })]);
    await settled();
    expect(dock().querySelector(".composer .ws-att")?.textContent).toContain(name);
  };

  it("the review discussion sends its files with the change request", async () => {
    const h = mount({ composer: true });
    await typeAndAttach("review.png");
    $<HTMLElement>('.dock [aria-label="Send and reopen"]')!.click();
    await settled();
    expect(commandOf(h.script, "request_changes")).toMatchObject({ text: "See this", attachments: [{ name: "review.png" }] });
  });

  it("resuming a paused task sends its files", async () => {
    const h = mount({ status: "paused", composer: true });
    await typeAndAttach("resume.png");
    button("Send and resume").click();
    await settled();
    expect(commandOf(h.script, "send_message")).toMatchObject({ text: "See this", resume: true, attachments: [{ name: "resume.png" }] });
  });

  it("queueing a message keeps its files", async () => {
    const h = mount({ status: "working", composer: true });
    await typeAndAttach("later.png");
    // Queue is a visible delivery choice; the plane then names what it does.
    button("Queue").click();
    button("Queue message").click();
    await settled();
    expect(commandOf(h.script, "send_message")).toMatchObject({ mode: "queue", attachments: [{ name: "later.png" }] });
  });

  it("editing a queued message brings its files back, and they go with it", async () => {
    const queued = { id: "q1", taskId: "t", workerId: null, text: "Queued text", mode: "queue", delivery: "queued", at: 1, deliveredAt: null, failure: null, attachments: [png] } as Message;
    const h = mount({ status: "working", composer: true, messages: [queued] });
    // The row holds only the text and its controls; the file shows again once the message is opened in Edit.
    expect($(".queue .qrow")?.textContent).toContain("Queued text");
    expect($(".queue .ws-att")).toBeNull();
    expect($(".queue")?.textContent).not.toContain("clip.png");
    [...document.querySelectorAll<HTMLButtonElement>(".queue button")].find(b => b.textContent === "Edit")!.click();
    await settled();
    expect(commandOf(h.script, "remove_queued")).toBeTruthy();
    h.script.receipt();
    await settled();
    expect(field("Message").value).toBe("Queued text");
    expect(dock().querySelector(".composer .ws-att")?.textContent).toContain("clip.png");
    expect(dock().querySelector('.composer [aria-label="Remove clip.png"]')).not.toBeNull();
    button("Send message now").click();
    await settled();
    expect(commandOf(h.script, "send_message")).toMatchObject({ text: "Queued text", attachments: [png] });
  });

  it("in Edit a queued message's file can be removed, and only the kept ones are sent", async () => {
    const other = { name: "other.txt", mimeType: "text/plain", data: btoa("other") };
    const queued = { id: "q1", taskId: "t", workerId: null, text: "Queued text", mode: "queue", delivery: "queued", at: 1, deliveredAt: null, failure: null, attachments: [png, other] } as Message;
    const h = mount({ status: "working", composer: true, messages: [queued] });
    expect($(".queue .ws-att")).toBeNull();
    [...document.querySelectorAll<HTMLButtonElement>(".queue button")].find(b => b.textContent === "Edit")!.click();
    await settled();
    h.script.receipt();
    await settled();
    expect([...dock().querySelectorAll(".composer .ws-att")].map(e => e.textContent)).toEqual(["clip.png", "other.txt"]);
    $<HTMLElement>('.composer [aria-label="Remove other.txt"]')!.click();
    await settled();
    button("Send message now").click();
    await settled();
    expect(commandOf(h.script, "send_message")).toMatchObject({ text: "Queued text", attachments: [png] });
  });
});
