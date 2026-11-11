import { createMemo, createSignal, onCleanup, Show } from "solid-js";
import { taskEditable } from "@cortex/workspace-domain";
import { createAttachments, createDictation, MessageComposer, RouteBar } from "./composer";
import { useWs } from "./ctx";
import { I } from "./kit";
import { ActionBar } from "./layout";

/** A frozen version protects the owner's draft from a background update or a competing Start. */
export function EditTask(props: { taskId: string; onClose: () => void }) {
  const ws = useWs();
  const current = createMemo(() => ws.store.index().byId.get(props.taskId));
  const initial = current()!.task;
  const version = initial.sourceVersion;
  const [title, setTitle] = createSignal(initial.title);
  const [text, setText] = createSignal(initial.description);
  const [settings, setSettings] = createSignal({...initial.settings});
  const [checkout, setCheckout] = createSignal(initial.checkout.kind);
  const [loaded, setLoaded] = createSignal(false), [failed, setFailed] = createSignal(false);
  const [saving, setSaving] = createSignal(false), [error, setError] = createSignal("");
  const files = createAttachments(), mic = createDictation();
  let alive = true;
  onCleanup(() => { alive = false; });
  const load = () => {
    setFailed(false);
    ws.store.readTaskAttachments(props.taskId).then(list => { if (alive) { files.set(list); setLoaded(true); } },
      () => { if (alive) setFailed(true); });
  };
  load();
  const off = () => !current() || !taskEditable(current()!.task) ? "This task has started or closed and can no longer be edited."
    : current()!.machine.connection !== "connected" ? "The machine is offline. Your draft stays here."
    : !loaded() ? "Read the saved attachments before editing."
    : saving() ? "Saving…" : undefined;
  const why = () => off() ?? (!version && ws.store.mode === "real" ? "Refresh this task before editing." : !title().trim() || !text().trim() ? "Write a title and description first." : undefined);
  const save = async () => {
    if (why() || files.busy()) return;
    setSaving(true); setError("");
    try {
      const receipt = await ws.run({type:"edit_task",taskId:props.taskId,expectedVersion:version ?? "1",
        title:title(),description:text(),attachments:files.files(),settings:settings(),checkout:{kind:checkout()}});
      if (receipt.ok) props.onClose(); else setError(receipt.message);
    } finally { setSaving(false); }
  };
  return <>
    <div class="dlg-h"><h2>Edit task</h2><button class="ib" aria-label="Cancel editing" onClick={props.onClose}><I n="x" /></button></div>
    <div class="dlg-b">
      <label class="fld"><span class="lab">Title</span><input class="inp" aria-label="Task title" maxLength={200} value={title()} disabled={!!off()} onInput={e => setTitle(e.currentTarget.value)} /></label>
      <Show when={failed()}><div role="alert">Saved attachments could not be read. Nothing was replaced. <button class="btn btn-sm" onClick={load}>Retry</button></div></Show>
      <MessageComposer label="Task description" placeholder="Describe the task." text={text()} onText={setText} files={files} mic={mic} off={off()}
        effect="Saves this task only. It does not start work or change its dependencies."
        route={<RouteBar id="edit-task" disabled={off()} settings={settings()} onChange={patch => setSettings({...settings(),...patch})} />}
        sendKey="mod-enter" send={{label:"Save changes",needs:"edit_task",why:why(),go:() => { void save(); }}} />
      <Show when={current()?.project.hasGit !== false}>
        <label class="fld"><span class="lab">Checkout</span><select class="inp" aria-label="Task checkout" disabled={!!off()} value={checkout()} onChange={e => setCheckout(e.currentTarget.value as "main" | "worktree")}>
          <option value="main">Project folder</option><option value="worktree">Separate worktree</option>
        </select></label>
      </Show>
      <Show when={error()}><div class="err" role="alert">{error()} Your changes stay here. For a stale edit, copy your changes, then close and reopen the editor to review the current task before reapplying them.</div></Show>
    </div>
    <ActionBar place="dialog"><button class="btn btn-line" onClick={props.onClose}>Cancel</button></ActionBar>
  </>;
}
