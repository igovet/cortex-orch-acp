// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, type Check, type Task, type WorkspaceCommand } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { PUSH_SUMMARY_MAX_BYTES, ReviewDialogs, type WsDialog } from "../src/dialogs";
import { MachineDialogs } from "../src/machine-dialogs";
import { CloseDialog } from "../src/task-chrome";
import { Modal } from "../src/kit";
import { COPY, workspaceCopy } from "../src/copy";
import { createWorkspaceStore } from "../src/store";
import { createSettingsState } from "../src/settings-state";
import { draftTask, flush, projection, scriptedPort, stoppedMerge } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const active = () => document.querySelector<HTMLDialogElement>("dialog[open]");
// A text button by its words, or an icon button, such as the paper plane, by its accessible name.
const button = (text: string) => [...active()!.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === text || b.getAttribute("aria-label") === text)!;
const type = (selector: string, text: string) => {
  const el = active()!.querySelector<HTMLInputElement>(selector)!;
  el.value = text; el.dispatchEvent(new Event("input", { bubbles: true }));
};
const settle = async () => { await flush(); await flush(); };

function mount(accepted = true, remote = "origin", checks: Check[] = [{ name: "Check", outcome: "not_run", detail: "", technicalReason: "Executable unavailable" }], reviewActions?: Task["reviewActions"], conflict = false, cleanup?: NonNullable<Task["integrationFacts"]>["deleteWorktree"]) {
  const state = emptyWorkspace(), cap = { everything: true, operations: [] };
  state.machines[0].connection = "connected";
  state.projects.push({ id: "p", machineId: state.machines[0].id, name: "Project", repository: "/p", openBranch: "dev", defaultBranch: "main" });
  state.tasks.push({ ...draftTask(), checkout: { kind: "main", branch: "dev" }, status: accepted ? "ready_to_merge" : "review", result: { version: 1, digest: "digest", summary: "Result", checks }, reviewActions, workspaceDigest: "digest", acceptance: accepted ? { version: 1, digest: "digest" } : null, ...(conflict ? { executionId: "e", conflict: "conflict", merge: stoppedMerge() } : {}) });
  state.repos.push({ projectId: "p", remote, tree: [], sources: {}, commits: [], branches: [], fetchedAt: null,
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "dev", path: "/p", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null }] });
  if (cleanup) {
    state.tasks[0].checkout = { kind: "worktree", id: "p:wt:task", path: "/task", branch: "cw/task" };
    state.tasks[0].integrationFacts = { targetBranch: "main", alreadyInTarget: false, pushStatus: null, deleteWorktree: cleanup };
    state.repos[0].checkouts.push({ ...state.repos[0].checkouts[0], kind: "worktree", id: "p:wt:task", path: "/task", branch: "cw/task" });
  }
  const script = scriptedPort({ capabilities: cap });
  const actions: {op: string; params: Record<string, unknown>; resolve: (v: Record<string, unknown>) => void; reject: (e: Error) => void}[] = [];
  script.port.machineAction = (op, params) => new Promise((resolve, reject) => actions.push({op, params, resolve, reject}));
  let ws!: Ws, trigger!: HTMLButtonElement;
  let open!: (d: WsDialog) => void, closeTask!: () => void;
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => {
    const store = createWorkspaceStore(script.port, { initial: { state, version: "1", capabilities: cap } });
    const [dialog, setDialog] = createSignal<WsDialog | null>(null);
    const [closing, setClosing] = createSignal<string | null>(null);
    ws = {
      store, dialog, open: setDialog, closeDialog: () => setDialog(null), run: store.dispatch,
      ok(command: Parameters<Ws["ok"]>[0], then: Parameters<Ws["ok"]>[1]) { void store.dispatch(command).then(r => { if (r.ok) then(r); }); },
      busy: <T,>(work: Promise<T>) => work,
      needs: () => ({}), gate: () => undefined, can: () => true,
      machine: () => store.state().machines[0], settings: createSettingsState(),
      t: (key: keyof typeof COPY) => COPY[key], route: () => null, newId: () => "new", go: vi.fn(),
    } as unknown as Ws;
    open = d => { trigger.focus(); setDialog(d); };
    closeTask = () => { trigger.focus(); setClosing("t"); };
    return <WsContext.Provider value={ws}><button ref={trigger}>Trigger</button><main tabindex="-1" />
      <ReviewDialogs /><MachineDialogs /><CloseDialog taskId={closing()} onClose={() => setClosing(null)} />
    </WsContext.Provider>;
  }, root);
  return { ws, script, state, actions, open, closeTask, trigger: () => trigger };
}

const cases: { name: string; dialog?: WsDialog; prepare?: () => void; action: string; operation: WorkspaceCommand["type"] }[] = [
  { name: "review feedback", dialog: { kind: "changes", taskId: "t", text: "Keep my feedback" }, action: "Send and reopen", operation: "request_changes" },
  { name: "acceptance", dialog: { kind: "accept", taskId: "t" }, prepare: () => { const el = active()!.querySelector<HTMLInputElement>('input[type="checkbox"]')!; el.checked = true; el.dispatchEvent(new Event("change", { bubbles: true })); }, action: "Accept v1", operation: "accept_result" },
  { name: "merge", dialog: { kind: "merge", taskId: "t", how: "squash" }, action: "Confirm merge into main", operation: "integrate" },
  { name: "conflict guidance", dialog: { kind: "cortex", taskId: "t" }, action: "Hand to the app", operation: "resolve_with_cortex" },
  { name: "push", dialog: { kind: "push", checkoutId: "p:main" }, action: "Push", operation: "git_push" },
  { name: "revert", dialog: { kind: "revert", checkoutId: "p:main", path: "a.txt" }, action: "Revert", operation: "git_revert" },
  { name: "revert commit", dialog: { kind: "revert-commit", checkoutId: "p:main", sha: "abc", message: "Fix" }, action: "Revert commit", operation: "revert_commit" },
  { name: "amend", dialog: { kind: "amend", checkoutId: "p:main", message: "Fix", pushed: false }, action: "Amend", operation: "git_commit" },
  { name: "branch", dialog: { kind: "branch", projectId: "p" }, prepare: () => type("input", "topic"), action: "Create branch", operation: "create_branch" },
  { name: "profile", dialog: { kind: "switch-profile", profileId: "work", name: "Work", changes: [], drafts: 1 }, action: "Discard and switch", operation: "activate_profile" },
  { name: "task closure", action: "Close task", operation: "close_task" },
];

it.each(cases)("$name stays open pending/refused, then closes and returns focus only on success", async c => {
  const h = mount(c.name !== "acceptance", undefined, undefined, undefined, c.operation === "resolve_with_cortex");
  h.ws.settings.setDraft(h.state.machines[0].id + ":routing", { kept: true });
  if (c.dialog) h.open(c.dialog); else h.closeTask();
  c.prepare?.();
  button(c.action).click();
  expect(active()).not.toBeNull();
  expect(h.script.commands.at(-1)?.operation).toBe(c.operation);
  const status = active()?.querySelector(".ws-request-status");
  if (c.operation === "accept_result" || c.operation === "close_task") expect(status?.textContent).toMatch(/^Requesting (acceptance|closure)…$/);
  h.script.receipt(false);
  await settle();
  expect(active()?.querySelector('[role="alert"]')?.textContent).toBe("Refused");
  if (status) expect(status.textContent).toBe("");
  if (c.name === "profile") expect(h.ws.settings.draftsOn(h.state.machines[0].id)).toBe(1);
  if (c.name === "review feedback") expect(active()!.querySelector("textarea")!.value).toBe("Keep my feedback");
  button(c.action).click();
  h.script.receipt(true);
  await settle();
  expect(active()).toBeNull();
  expect(document.activeElement).toBe(h.trigger());
  if (c.name === "profile") expect(h.ws.settings.draftsOn(h.state.machines[0].id)).toBe(0);
});

it.each([true, false])("shows the default cleanup choice only when safe (safe: %s)", available => {
  const h = mount(true, "", [], undefined, false, { available, default: true, ...(available ? {} : { reason: "worktree_dirty" }) });
  h.open({ kind: "merge", taskId: "t", how: "squash" });
  const input = active()!.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
  expect(document.getElementById(input.getAttribute("aria-labelledby")!)?.textContent).toBe("Delete worktree folder");
  expect(input.disabled).toBe(!available);
  expect(input.checked).toBe(available);
  if (!available) expect(document.getElementById(input.getAttribute("aria-describedby")!)?.textContent).toContain("uncommitted or untracked");
  button("Confirm merge into main").click();
  expect(h.script.commands.at(-1)?.payload).toMatchObject({ type: "integrate", deleteWorktree: available });
});

it("uses the approved Russian worktree action labels", () => {
  expect(workspaceCopy("deleteWorktreeFolder", "ru-RU")).toBe("Удалить worktree папку");
  expect(workspaceCopy("closeDeleteWorktreeFolder", "ru")).toBe("Закрыть и удалить worktree папку");
});

const paidReason = "paid live check not requested by the owner";
const skipped: Check[] = [
  { name: "Live desktop", outcome: "not_run", kind: "policy", detail: paidReason, technicalReason: paidReason, rerunnable: false },
  { name: "Manual smoke", outcome: "not_run", kind: "manual", detail: "The owner did not authorize this step." },
  { name: "Runtime test", outcome: "not_run", kind: "automated", detail: "", technicalReason: "Executable unavailable", rerunnable: false },
  { name: "Verification", outcome: "passed", detail: "" },
];

it.each([{ kind: "accept", taskId: "t" }, { kind: "merge", taskId: "t", how: "squash" }] as const)(
  "$kind lists each skipped check with its own status and reason",
  dialog => {
    const h = mount(false, "origin", [...skipped, ...(dialog.kind === "accept" ? [{ name: "Open check", outcome: "unverified", detail: "Incomplete receipt" } as const] : [])],
      { accept: true, reason: null, unblockAction: null, acknowledgeOpenChecks: dialog.kind === "accept" });
    h.open(dialog);
    expect(active()!.textContent).toContain("1 passed, 3 not run");
    const rows = [...active()!.querySelectorAll(".ws-rows .l-row")];
    expect(rows.map(row => [row.querySelector(".l-row-t")?.textContent, row.querySelector(".l-row-d")?.textContent])).toEqual([
      ["Live desktop", `Not run: ${paidReason}`],
      ["Manual smoke", "Not run: The owner did not authorize this step."],
      ["Runtime test", "Not run: Executable unavailable"],
      ...(dialog.kind === "accept" ? [["Open check", "Incomplete receipt"]] : []),
    ]);
    expect(active()!.querySelector('input[type="checkbox"]') !== null).toBe(dialog.kind === "accept");
  },
);

it("confirms the displayed SSH URL and sends one push request for the current branch", async () => {
  const remote = "git@github.com:igovet/cortex-orch-acp.git";
  const h = mount(true, remote);
  h.open({ kind: "push", checkoutId: "p:main" });
  expect(active()?.textContent).toContain(`Push dev to ${remote}?`);
  button("Push").click();
  expect(h.script.commands).toHaveLength(1);
  expect(h.script.commands[0].payload).toEqual({ type: "git_push", checkoutId: "p:main", remote, branch: "dev", confirmed: true, summary: "" });
  h.script.receipt(true);
  await settle();
  expect(active()).toBeNull();
});

it("passes the edited summary to the Pull and push confirmation action", async () => {
  const remote = "git@github.com:example/project.git";
  const h = mount(true, remote);
  const seen: unknown[] = [];
  h.open({ kind: "push", checkoutId: "p:main", pullFirst: async push => { seen.push(push); return true; } });
  const field = active()!.querySelector("textarea")!;
  field.value = "Owner-reviewed outgoing changes";
  field.dispatchEvent(new Event("input", { bubbles: true }));
  button("Pull and push").click();
  await settle();
  expect(seen).toEqual([{ type: "git_push", checkoutId: "p:main", remote, branch: "dev", confirmed: true, summary: field.value }]);
  expect(active()).toBeNull();
});

it("refuses an over-limit Push summary with visible words and sends a summary at the limit whole", async () => {
  const remote = "git@github.com:example/project.git";
  const h = mount(true, remote);
  h.open({ kind: "push", checkoutId: "p:main" });
  await settle();
  expect(active()!.textContent).toContain("up to 8,000 bytes");
  const field = active()!.querySelector("textarea")!;
  // Two-byte characters reach the byte limit at half as many characters.
  const atLimit = `Owner summary\n\n${"é".repeat((PUSH_SUMMARY_MAX_BYTES - 15) / 2)}.`;
  expect(new TextEncoder().encode(atLimit).length).toBe(PUSH_SUMMARY_MAX_BYTES);
  field.value = `${atLimit}é`;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  const status = active()!.querySelector('[role="status"]')!;
  expect(status.textContent).toBe("The summary is 8,002 bytes. Shorten it to 8,000 bytes or fewer to push; nothing is cut automatically.");
  expect(field.getAttribute("aria-invalid")).toBe("true");
  expect(button("Push").getAttribute("aria-disabled")).toBe("true");
  expect(button("Push").getAttribute("data-why")).toBe(status.textContent);
  button("Push").click();
  expect(h.script.commands).toHaveLength(0);
  field.value = atLimit;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  expect(field.getAttribute("aria-invalid")).toBeNull();
  button("Push").click();
  expect(h.script.commands.map(c => c.payload)).toEqual([{ type: "git_push", checkoutId: "p:main", remote, branch: "dev", confirmed: true, summary: atLimit }]);
  h.script.receipt(true);
  await settle();
  expect(active()).toBeNull();
});

it.each(["native", "key"])("connection (%s) stays open pending/refused, then closes and returns focus only on success", async auth => {
  const h = mount();
  h.open({kind:"add-connection"});
  if (auth === "key") { button("API key").click(); type('[aria-label="Connection name"]', "Work"); type('[aria-label="API key"]', "fixture-only"); }
  const action = auth === "key" ? "Save API-key connection" : "Recheck native login and connect";
  button(action).click(); button("Checking…").click();
  expect(h.actions).toHaveLength(1);
  expect(h.actions[0]).toMatchObject({op:"engine.connection",params:{action:auth === "key" ? "add_key" : "recheck",engine:"claude-agent-acp"}});
  expect(active()).not.toBeNull();
  expect(button("Checking…").disabled).toBe(true);
  h.actions[0].reject(new Error("Refused")); await settle();
  expect(active()?.querySelector('[role="alert"]')?.textContent).toContain("not confirmed");
  if (auth === "key") {
    expect(active()?.querySelector<HTMLInputElement>('[aria-label="Connection name"]')?.value).toBe("Work");
    expect(active()?.querySelector<HTMLInputElement>('[aria-label="API key"]')?.value).toBe("");
    type('[aria-label="API key"]', "fixture-retry");
  }
  button(action).click();
  h.actions[1].resolve({connectionId:"fixture"}); await settle();
  expect(active()).toBeNull();
  expect(document.activeElement).toBe(h.trigger());
});

it("a newer result projection is not a review acknowledgement", async () => {
  const h = mount();
  h.open({ kind: "changes", taskId: "t", text: "Feedback" });
  button("Send and reopen").click();
  h.script.emit(projection("2", { ...h.state, tasks: h.state.tasks.map(t => ({ ...t, result: { ...t.result!, version: 2 } })) }));
  await settle();
  expect(active()?.textContent).toContain("Request changes to v2");
  h.script.receipt(true);
  await settle();
  expect(active()).toBeNull();
  expect(document.activeElement).toBe(h.trigger());
});

it.each(["confirm", "prompt"] as const)("%s callbacks keep a refusal open and close an acknowledged action", async kind => {
  const h = mount();
  const command = { type: "rename_machine" as const, machineId: h.state.machines[0].id, name: "Desk" };
  const go = () => h.ws.run(command).then(r => r.ok);
  h.open(kind === "confirm" ? { kind, title: "Stop worker?", note: "One worker", action: "Confirm", go } : { kind, title: "Stop command?", label: "Reason", action: "Confirm", go });
  if (kind === "prompt") type("input", "Keep the reason");
  button("Confirm").click(); h.script.receipt(false); await settle();
  expect(active()?.querySelector('[role="alert"]')?.textContent).toBe("Refused");
  button("Confirm").click(); h.script.receipt(true); await settle();
  expect(active()).toBeNull();
  expect(document.activeElement).toBe(h.trigger());
});

it("unsaved settings keep the dialog, destination and draft after a save refusal", async () => {
  const h = mount(), key = h.state.machines[0].id + ":routing";
  h.ws.settings.setDraft(key, { kept: true });
  h.ws.settings.register({ key, dirty: () => true, save: () => h.ws.run({ type: "rename_machine", machineId: h.state.machines[0].id, name: "Desk" }).then(r => r.ok) });
  h.trigger().focus(); h.ws.settings.setPending("/");
  button("Save here").click(); h.script.receipt(false); await settle();
  expect(active()?.querySelector('[role="alert"]')?.textContent).toBe("Refused");
  expect(h.ws.settings.pending()).toBe("/");
  expect(h.ws.settings.draft(key)).toEqual({ kept: true });
  button("Save here").click(); h.script.receipt(true); await settle();
  expect(active()).toBeNull();
  expect(h.ws.go).toHaveBeenCalledWith("/", { force: true });
  expect(document.activeElement).toBe(h.trigger());
});

it("cancellation returns focus and a reopened dialog does not repeat an old refusal", async () => {
  const h = mount();
  h.open({ kind: "changes", taskId: "t", text: "Feedback" });
  button("Send and reopen").click(); h.script.receipt(false); await settle();
  active()!.dispatchEvent(new Event("cancel", { cancelable: true }));
  expect(active()).toBeNull();
  expect(document.activeElement).toBe(h.trigger());
  h.open({ kind: "changes", taskId: "t" });
  expect(active()?.querySelector('[role="alert"]')).toBeNull();
});

it("unmount closes the native dialog; a removed trigger falls back to the page", () => {
  let visible!: (value: boolean) => void;
  const root = document.createElement("div"); document.body.append(root);
  const trigger = document.createElement("button"); document.body.append(trigger); trigger.focus();
  dispose = render(() => {
    const [open, setOpen] = createSignal(true); visible = setOpen;
    return <><main tabindex="-1" /><Modal open={open()} onClose={() => setOpen(false)} label="Test"><input data-autofocus /></Modal></>;
  }, root);
  trigger.remove(); visible(false);
  expect(document.activeElement).toBe(document.querySelector("main"));
  trigger.focus(); visible(true);
  const dialog = active()!;
  dispose(); dispose = undefined;
  expect(dialog.open).toBe(false);
});

it("names every dialog by its visible title, following it as it changes, and by its label only without one", async () => {
  const [who, setWho] = createSignal("backend-dev");
  const [headed, setHeaded] = createSignal(true);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Modal open onClose={() => {}} label="Stop this worker?">
    {headed() ? <div class="dlg-h"><h2>Stop {who()}?</h2></div> : <p>No title</p>}<button data-autofocus>Keep working</button>
  </Modal>, root);
  await settle();
  const dialog = active()!;
  const name = () => { const by = dialog.getAttribute("aria-labelledby"); return by ? document.getElementById(by)?.textContent : dialog.getAttribute("aria-label"); };
  expect(name()).toBe("Stop backend-dev?");
  expect(dialog.hasAttribute("aria-label")).toBe(false);
  expect(dialog.getAttribute("data-dialog")).toBe("Stop this worker?");
  setWho("frontend-dev"); await settle();
  expect(name()).toBe("Stop frontend-dev?");
  setHeaded(false); await settle();
  expect(dialog.hasAttribute("aria-labelledby")).toBe(false);
  expect(name()).toBe("Stop this worker?");
});
