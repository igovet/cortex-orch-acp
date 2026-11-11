// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, type WorkspaceCommand } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { ReviewDialogs, type WsDialog } from "../src/dialogs";
import { MachineDialogs } from "../src/machine-dialogs";
import { CloseDialog } from "../src/task-chrome";
import { Modal } from "../src/kit";
import { COPY } from "../src/copy";
import { createWorkspaceStore } from "../src/store";
import { createSettingsState } from "../src/settings-state";
import { draftTask, flush, projection, scriptedPort } from "./support";

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

function mount(accepted = true) {
  const state = emptyWorkspace(), cap = { everything: true, operations: [] };
  state.machines[0].connection = "connected";
  state.projects.push({ id: "p", machineId: state.machines[0].id, name: "Project", repository: "/p", openBranch: "dev", defaultBranch: "main" });
  state.tasks.push({ ...draftTask(), checkout: { kind: "main", branch: "dev" }, status: "review", result: { version: 1, digest: "digest", summary: "Result", checks: [{ name: "Check", outcome: "not_run", detail: "", technicalReason: "Executable unavailable" }] }, workspaceDigest: "digest", acceptance: accepted ? { version: 1, digest: "digest" } : null });
  state.repos.push({ projectId: "p", remote: "origin", tree: [], sources: {}, commits: [], branches: [], fetchedAt: null,
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "dev", path: "/p", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null }] });
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
  { name: "conflict guidance", dialog: { kind: "cortex", taskId: "t" }, action: "Resolve automatically", operation: "resolve_with_cortex" },
  { name: "push", dialog: { kind: "push", checkoutId: "p:main" }, action: "Push", operation: "git_push" },
  { name: "revert", dialog: { kind: "revert", checkoutId: "p:main", path: "a.txt" }, action: "Revert", operation: "git_revert" },
  { name: "revert commit", dialog: { kind: "revert-commit", checkoutId: "p:main", sha: "abc", message: "Fix" }, action: "Revert commit", operation: "revert_commit" },
  { name: "amend", dialog: { kind: "amend", checkoutId: "p:main", message: "Fix", pushed: false }, action: "Amend", operation: "git_commit" },
  { name: "branch", dialog: { kind: "branch", projectId: "p" }, prepare: () => type("input", "topic"), action: "Create branch", operation: "create_branch" },
  { name: "profile", dialog: { kind: "switch-profile", profileId: "work", name: "Work", changes: [], drafts: 1 }, action: "Discard and switch", operation: "activate_profile" },
  { name: "task closure", action: "Close task", operation: "close_task" },
];

it.each(cases)("$name stays open pending/refused, then closes and returns focus only on success", async c => {
  const h = mount(c.name !== "acceptance");
  h.ws.settings.setDraft(h.state.machines[0].id + ":routing", { kept: true });
  if (c.dialog) h.open(c.dialog); else h.closeTask();
  c.prepare?.();
  button(c.action).click();
  expect(active()).not.toBeNull();
  expect(h.script.commands.at(-1)?.operation).toBe(c.operation);
  h.script.receipt(false);
  await settle();
  expect(active()?.querySelector('[role="alert"]')?.textContent).toBe("Refused");
  if (c.name === "profile") expect(h.ws.settings.draftsOn(h.state.machines[0].id)).toBe(1);
  if (c.name === "review feedback") expect(active()!.querySelector("textarea")!.value).toBe("Keep my feedback");
  button(c.action).click();
  h.script.receipt(true);
  await settle();
  expect(active()).toBeNull();
  expect(document.activeElement).toBe(h.trigger());
  if (c.name === "profile") expect(h.ws.settings.draftsOn(h.state.machines[0].id)).toBe(0);
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
