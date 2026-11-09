import { createEffect, createMemo, createSignal, For, Match, onCleanup, Show, Switch } from "solid-js";
import { CONNECTION_ENGINE, DEFAULT_WORKTREE_ROOT, engineLabel, ENGINES, LANGUAGES, type EngineKey, type Machine } from "@cortex/workspace-domain";
import type { ThemePref } from "./theme";
import { sizeText } from "./archive-model";
import { useWs } from "./ctx";
import { A, I, MenuItem, NameEdit, Pop } from "./kit";
import { Orchestration, useReadOnly } from "./orchestration";
import type { Route } from "./router";
import { clock, dayClock, engineLimits, plainVersion, projectsOn } from "./selectors";
import { HELP } from "./orch-help";
import { Adv, Hint, Pick, SaveBar, Seg, Srow, Stepper, Sw } from "./settings-kit";
import { useDraft } from "./settings-state";
import { Updates } from "./maintenance";
import { duration } from "./session-model";
import { VoiceSettings } from "./voice-settings";

const Page = (props: { title: string; lead: string; children: unknown; bar?: unknown }) => <>
  <div class="set-main"><div class="set-in">
    <div><h1 class="set-title">{props.title}</h1><p class="set-lead">{props.lead}</p></div>
    {props.children as never}
  </div></div>
  {props.bar as never}
</>;

const THEMES: { id: ThemePref; label: string; icon: string }[] = [{ id: "system", label: "System", icon: "monitor" }, { id: "light", label: "Light", icon: "sun" }, { id: "dark", label: "Dark", icon: "moon" }];
/** The only place the theme changes. */
function Appearance() {
  const ws = useWs();
  return <Page title="Appearance" lead="How the app looks on this computer.">
    <div class="surface">
      <Srow t="Theme" d="System follows your desktop setting, and it is the default.">
        <div class="seg tall" role="group" aria-label="Theme"><For each={THEMES}>{t =>
          <button aria-current={ws.theme.pref() === t.id ? "true" : "false"} aria-pressed={ws.theme.pref() === t.id} onClick={() => ws.theme.setPref(t.id)}><I n={t.icon} s={14} />{t.label}</button>
        }</For></div>
      </Srow>
      <Srow t="Notifications" d="Notify when a question arrives outside the open task or sprint. The desktop notification service must be available."><Sw label="Desktop notifications" checked={ws.panels.notifications()} disabled={ws.notify ? undefined : "Desktop notifications require the native app"} onChange={on => ws.panels.setNotifications(on)} /></Srow>
      <Srow t="Autosave" d="Saves a file about a second after you stop typing. It waits while a task works in the checkout and after any conflict. Off by default."><Sw label="Autosave files" checked={ws.panels.autosave()} onChange={on => ws.panels.setAutosave(on)} /></Srow>
    </div>
  </Page>;
}

const SHORTCUTS: [string, string][] = [["New task", "Ctrl+N"], ["New sprint", "Ctrl+Shift+N"], ["Search and actions", "Ctrl+K"], ["Inbox", "Ctrl+I"], ["Sidebar", "Ctrl+B"], ["Inspector", "Ctrl+Alt+B"], ["Terminal", "Ctrl+J"], ["Stop task", "Ctrl+."], ["Queue message", "Ctrl+Enter"]];
const Shortcuts = () => <Page title="Shortcuts" lead="Close, Cancel, Delete and Merge have no shortcut on purpose.">
  <div class="surface"><For each={SHORTCUTS}>{([what, keys]) => <div class="srow ws-short"><span class="t ws-w400">{what}</span><span class="kbd ws-kbd">{keys}</span></div>}</For></div>
</Page>;

const machineIcon = (m: Machine) => m.kind === "local" ? "laptop" : "server";
function Machines() {
  const ws = useWs();
  const list = () => ws.store.state().machines;
  const count = (m: Machine) => projectsOn(ws.store.index(), m.id).length;
  const reconnect = (m: Machine) => ws.demo ? ws.demo.reconnect(m.id) : void ws.store.reload();
  return <Page title="Machines" lead="A machine is a running copy of this app. Projects live on a machine. You work on one at a time, while Inbox and Archive cover them all.">
    <div class="set-h">{list().length} machine{list().length === 1 ? "" : "s"}<span class="r"><button class="btn btn-primary" {...ws.needs("pair_machine")} onClick={() => ws.open({ kind: "add-machine" })}><I n="plus" s={14} />Add remote machine…</button></span></div>
    <div class="surface"><For each={list()}>{m => <div class="mcard">
      <span class="mi2"><I n={machineIcon(m)} /></span>
      <span class="ws-free"><span class="tt">{m.name}</span><span class="ds">
        <span>{m.kind === "local" ? "Local" : "Remote, through the server connector"}</span>
        <span>{count(m)} project{count(m) === 1 ? "" : "s"}{m.connection === "connected" ? "" : ", last known"}</span>
        <Show when={m.version}><span>Version {plainVersion(m.version!)}</span></Show>
        <Show when={m.latencyMs !== null && m.connection === "connected"}><span>{m.latencyMs} ms observed</span></Show>
      </span></span>
      <span class="row">
        <Show when={m.connection === "connected"} fallback={<span class="st st-idle"><I n="wifi-off" s={12} />Offline, last seen {clock(m.lastConfirmedAt)}</span>}><span class="st st-ok"><I n="check-circle" s={12} />Connected</span></Show>
        <Show when={m.connection === "connected"} fallback={<button class="btn btn-line btn-sm" onClick={() => reconnect(m)}>Retry now</button>}>
          <Show when={m.id === ws.machine().id} fallback={<button class="btn btn-line btn-sm" onClick={() => ws.go({ name: "home", machine: m.id })}>Switch</button>}>
            <A class="btn btn-line btn-sm" to={{ name: "settings", section: "overview", machine: m.id }}>Settings</A>
          </Show>
        </Show>
      </span>
    </div>}</For></div>
    <div class="cap faint">While a machine is offline its projects stay visible with their last known state, and anything that needs the machine is turned off with the reason.</div>
  </Page>;
}

function Overview() {
  const ws = useWs();
  const m = () => ws.machine(), off = useReadOnly();
  const limits = createMemo(() => engineLimits(ws.store.state(), m()).rows.filter(r => r.usedPercent !== null));
  const count = () => projectsOn(ws.store.index(), m().id).length;
  const rename = (name: string) => { if (name.trim() && name !== m().name) ws.run({ type: "rename_machine", machineId: m().id, name }); };
  const remove = () => ws.open({ kind: "confirm", title: `Remove ${m().name}?`, danger: true, action: "Remove machine", note: `This app forgets the machine, its ${count()} projects and their tasks. Nothing on the machine itself is touched.`, go() { const id = m().id; const local = ws.store.state().machines.find(x => x.kind === "local")!; ws.go({ name: "home", machine: local.id }); return ws.run({ type: "remove_machine", machineId: id, confirmed: true }).then(r => r.ok); } });
  return <Page title={m().name} lead={m().kind === "local" ? "The process you started on this computer." : "A remote process you connected to through the server connector."}>
    <div class="surface">
      <Srow t="Name" d="Shown in the machine switcher."><input class="inp ws-name" aria-label="Machine name" value={m().name} disabled={!!off()} onChange={e => rename(e.currentTarget.value)} /></Srow>
      <Srow t="Connection" d={m().kind === "local" ? "Local process, no connector involved." : "Through the server connector."}>
        <Show when={m().connection === "connected"} fallback={<span class="st st-idle"><I n="wifi-off" s={12} />Offline</span>}><span class="st st-ok"><I n="check-circle" s={12} />Connected</span></Show>
      </Srow>
      <Show when={m().lastConfirmedAt}><Srow t="Last confirmed" d="Latest answer from the process."><span class="tnum">{clock(m().lastConfirmedAt)}</span></Srow></Show>
      <Srow t="Projects" d="Repositories added to this machine."><A class="lnk" to={{ name: "home", machine: m().id }}>{count()} project{count() === 1 ? "" : "s"}</A></Srow>
      <Show when={m().version}><Srow t="Version" d="What is installed on this machine."><span class="tnum">{plainVersion(m().version!)}</span></Srow></Show>
      <Show when={m().diskMb !== null}><Srow t="Disk used" d="Database and saved reports."><A class="lnk" to={{ name: "settings", section: "storage", machine: m().id }}>{sizeText(m().diskMb!)}</A></Srow></Show>
      <Show when={limits().length > 0}><Srow t="Engine limits" d="Used this week."><A class="lnk" to={{ name: "settings", section: "connections", machine: m().id }}>{limits().map(l => `${l.label} ${l.usedPercent}%`).join(", ")}</A></Srow></Show>
    </div>
    <div class="set-h">Open machine settings</div>
    <div class="two">
      <A class="rc" to={{ name: "settings", section: "orchestration", machine: m().id }}><span><span class="t">Orchestration</span><span class="d">Models, effort, access and routing for new tasks</span></span></A>
      <A class="rc" to={{ name: "settings", section: "storage", machine: m().id }}><span><span class="t">Storage and archive</span><span class="d">{m().retentionDays} days retention, {m().autoArchiveDays} days before auto archive</span></span></A>
    </div>
    <Show when={m().kind === "remote"}><div class="row"><button class="btn btn-line btn-sm" {...ws.needs("remove_machine")} onClick={remove}>Remove machine…</button></div></Show>
  </Page>;
}

function Connections() {
  const ws = useWs();
  const m = () => ws.machine(), off = useReadOnly();
  const limits = createMemo(() => engineLimits(ws.store.state(), m()));
  const stateOf = (c: Machine["connections"][number]) => c.state === "connected" ? { text: "Configured", tone: "st-ok", icon: "check-circle" } : c.state === "needs_login" ? { text: "Sign in needed", tone: "st-warn", icon: "alert" } : { text: "Unavailable", tone: "st-idle", icon: "x-circle" };
  const waiting = () => m().connections.filter(c => c.state !== "connected");
  const guard = (run: () => void) => () => { if (!off()) run(); };
  const setupText = { not_downloaded: "Not downloaded", downloaded_no_login: "Downloaded, sign in on this machine", ready_to_connect: "Ready to connect", connected: "Connected" };
  const setupNote = (e: EngineKey) => { const s = m().config.setup[e]; return [setupText[s.status], s.automatic ? "automatic setup on" : "", s.reason ?? "", s.autoConnectDisabled ? "automatic connection off" : ""].filter(Boolean).join(", "); };
  const enableOpenCode = () => ws.open({ kind: "prompt", title: "Enable OpenCode", label: "Connection name", value: "OpenCode", action: "Enable", hint: "OpenCode is never downloaded. It uses the one on this machine.", go: name => ws.run({ type: "enable_opencode", machineId: m().id, name }).then(r => r.ok) });
  return <Page title="Engine connections" lead="How this machine reaches each engine and how much of their limits is used. Secrets are never shown.">
    <div class="set-h">Engines: download and connect<Hint text={HELP.engineSetup} label="engine setup" /></div>
    <div class="surface"><For each={ENGINES}>{e => <Srow t={e.label} d={setupNote(e.id)}>
      <Show when={e.id === "opencode"} fallback={<Show when={m().config.setup[e.id].status === "not_downloaded"}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("download_engine")} onClick={guard(() => ws.run({ type: "download_engine", machineId: m().id, engine: e.id }))}>Download engine</button></Show>}>
        <Show when={m().config.setup.opencode.status !== "connected"}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("enable_opencode")} onClick={guard(enableOpenCode)}>Enable OpenCode…</button></Show>
      </Show>
    </Srow>}</For></div>
    <div class="set-h">Connections<Hint text={HELP.connections} label="connections" /><span class="r"><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("add_connection")} onClick={guard(() => ws.open({ kind: "add-connection" }))}><I n="plus" s={12} />Add connection…</button></span></div>
    <Show when={waiting().length > 0}><div class="note-box"><I n="alert" s={14} class="st-warn" /><span>To finish setup, sign in to {waiting().map(c => c.label).join(", ")} on this machine, then press Check.</span></div></Show>
    <div class="surface"><div class="gt">
      <div class="gr crow th"><span>Connection</span><span>State</span><span></span><span></span></div>
      <For each={m().connections}>{c => <div class="gr crow">
        <span class="trunc"><NameEdit class="w500" label="Connection name" value={c.label} disabled={off() ?? ws.gate("rename_connection")} onSave={name => { void ws.run({ type: "rename_connection", machineId: m().id, connectionId: c.id, name }); }} /><span class="sub">{c.note}, {c.sessions} session{c.sessions === 1 ? "" : "s"} max{c.checkedAt ? `, checked ${clock(c.checkedAt)}` : ""}</span></span>
        <span><span class={`st ${stateOf(c).tone}`}><I n={stateOf(c).icon} s={12} />{stateOf(c).text}</span></span>
        <span class="row ws-gap4"><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("check_connection")} onClick={guard(() => ws.run({ type: "check_connection", machineId: m().id, connectionId: c.id, now: ws.store.clock() }))}>Check</button>
          <button class="btn btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("refresh_models")} onClick={guard(() => ws.run({ type: "refresh_models", machineId: m().id, now: ws.store.clock() }))}>Refresh models</button></span>
        <button class="ib ws-ib" popovertarget={off() ? undefined : `m-conn-${c.id}`} aria-label={`More for ${c.label}`} aria-disabled={off() ? "true" : undefined} data-why={off()}><I n="more" s={14} /></button>
        <Pop id={`m-conn-${c.id}`} class="pop-right" align="right" label={c.label}>
          <MenuItem icon="sliders" label="Maximum sessions…" needs="set_sessions" onSelect={() => ws.open({ kind: "prompt", title: `Maximum sessions on ${c.label}`, label: "Sessions, 1 to 64", value: String(c.sessions), action: "Save", go: text => ws.run({ type: "set_sessions", machineId: m().id, connectionId: c.id, sessions: Number(text) }).then(r => r.ok) })} />
          <MenuItem icon="trash" label="Remove…" danger needs="remove_connection" onSelect={() => ws.open({ kind: "confirm", title: `Remove ${c.label}?`, danger: true, action: "Remove", note: "Tasks already running keep their route. New tasks can no longer use it.", go: () => ws.run({ type: "remove_connection", machineId: m().id, connectionId: c.id }).then(r => r.ok) })} />
        </Pop>
      </div>}</For>
    </div></div>
    <A class="lnk" to={{name:"settings", section:"orchestration", machine:m().id, page:"models"}}>View connected engine models and observed context</A>
    <For each={m().connections}>{c => <Adv id={`conn-${c.id}`} label={`Details of ${c.label}`}><div class="surface ws-pad ws-kv">
      <For each={[["Engine", engineLabel(CONNECTION_ENGINE[c.engine])], ["Provider", c.detail.provider], ["Sign-in", c.detail.authMode], ["Key kind", c.detail.keyKind], ["Endpoint", c.detail.endpoint], ["Launch command", c.detail.launchCommand], ["Credential", c.detail.credential], ["Qualified", c.detail.qualified ? "Yes" : "No"], ["Created", c.detail.createdAt], ["Updated", c.detail.updatedAt]].filter(([, v]) => v)}>{([k, v]) => <><span class="cap muted">{k}</span><span class="cap mono trunc">{v}</span></>}</For>
    </div></Adv>}</For>
    <div class="set-h">Limits</div>
    <Show when={limits().rows.some(r => r.usedPercent !== null)} fallback={<div class="surface ws-empty">{limits().note}</div>}>
      <div class="surface"><div class="gt">
        <div class="gr urow th"><span>Engine</span><span>Window</span><span>Used</span><span>Resets</span><span>Basis</span></div>
        <For each={limits().rows.filter(r => r.usedPercent !== null)}>{r => <div class="gr urow">
          <span class="w500">{r.label}</span><span>This week</span>
          <Show when={r.usedPercent !== null} fallback={<span class="muted">Unavailable</span>}><span class="u-used"><span class="bar"><i style={{ width: `${r.usedPercent}%` }}></i></span><span class="tnum">{r.usedPercent}% used</span></span></Show>
          <span class="tnum">{r.resetAt ? dayClock(r.resetAt) : ""}</span><span class="cap muted">{r.observedAt ? `Observed ${clock(r.observedAt)}` : ""}</span>
        </div>}</For>
      </div></div>
    </Show>
    <p class="hint">Percentages come from each provider and are a snapshot, not a live meter. An unknown value is never counted as zero.</p>
  </Page>;
}

/** Worktrees live outside every repository by default, so a project never sees another task's files as its own. */
function Worktrees() {
  const ws = useWs();
  const m = () => ws.machine(), off = useReadOnly();
  const d = useDraft<string>({ key: () => `${m().id}:worktrees`, saved: () => m().worktreeRoot, name: () => "Worktrees", where: () => m().name, commit: path => ws.run({ type: "set_worktree_root", machineId: m().id, path }) });
  const defaultRoot = () => m().defaultWorktreeRoot ?? (ws.store.mode === "demo" ? DEFAULT_WORKTREE_ROOT : undefined);
  const example = () => `${d.value().replace(/\/+$/, "")}/<project-key>/<task-id>`;
  return <Page title="Worktrees" lead="Where this machine keeps the separate checkouts of tasks that do not run in the main checkout." bar={<SaveBar d={d} />}>
    <div class="surface">
      <Srow t="Folder" d="New worktrees go here, one folder per project. Existing worktrees stay where they are.">
        <input class="inp mono ws-path" aria-label="Worktree folder" value={d.value()} disabled={!!off()} onInput={e => d.set(e.currentTarget.value)} />
        <button class="btn btn-line btn-sm" aria-disabled={off() || !defaultRoot() || d.value() === defaultRoot() ? "true" : undefined} data-why={defaultRoot() ? undefined : "The machine has not reported its default folder yet"} onClick={() => { const path = defaultRoot(); if (!off() && path) d.set(path); }}>Use default</button>
      </Srow>
      <div class="srow" style={{ "grid-template-columns": "minmax(0, 1fr) minmax(0, 1.5fr)" }}><span><span class="t">Example path</span><span class="d">What a new worktree would look like.</span></span><span class="mono cap muted trunc">{example()}</span></div>
    </div>
  </Page>;
}

function Storage() {
  const ws = useWs();
  const m = () => ws.machine(), off = useReadOnly();
  const d = useDraft<{ retention: number; auto: number }>({ key: () => `${m().id}:storage`, saved: () => ({ retention: m().retentionDays, auto: m().autoArchiveDays }), name: () => "Storage and archive", where: () => m().name, commit: v => ws.run({ type: "save_storage", machineId: m().id, retentionDays: v.retention, autoArchiveDays: v.auto }) });
  const total = () => m().storage.reduce((sum, s) => sum + s.mb, 0);
  const last = () => m().cleanup;
  return <Page title="Storage and archive" lead="How long this machine keeps finished work, and what it uses." bar={<SaveBar d={d} />}>
    <div class="set-h">Archive rules</div>
    <div class="surface">
      <Srow t="Auto archive" d="Tasks move from Done to Archive after this many days. Unmerged work asks you first."><Stepper label="Auto archive" value={d.value().auto} suffix="days" min={1} max={90} disabled={off()} onChange={auto => d.set({ ...d.value(), auto })} /></Srow>
      <Srow t="Retention" d="Archived tasks are cleaned up after this many days. 30 by default, set per machine."><Stepper label="Retention" value={d.value().retention} suffix="days" min={1} max={365} disabled={off()} onChange={retention => d.set({ ...d.value(), retention })} /></Srow>
      <Srow t="Space reclamation failures" d="Deleted records whose SQLite space could not be reclaimed. Deletion is not rolled back; close readers before maintenance."><span class={m().reclamationFailures ? "st st-warn" : "cap muted"}>{m().reclamationFailures ?? 0} records</span></Srow>
      <Srow t="Last cleanup" d={last() ? `${clock(last()!.at)}: removed ${last()!.removed} task${last()!.removed === 1 ? "" : "s"}, ${last()!.skipped} skipped and waiting for you. ${last()!.reclamationFailed ?? 0} records failed space reclamation.${last()!.freedMb ? ` ${sizeText(last()!.freedMb)} freed.` : ""}` : "No cleanup has run on this machine yet."}>
        <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("run_cleanup")} onClick={() => { if (!off()) ws.run({ type: "run_cleanup", machineId: m().id, now: ws.store.clock() }); }}>Run cleanup now</button>
      </Srow>
    </div>
    <Show when={m().storage.length > 0}>
      <div class="set-h">Space used<span class="r"><span class="cap muted tnum">{sizeText(total())} in total</span></span></div>
      <div class="surface ws-pad"><For each={m().storage}>{s => <div class="dbar"><span class="trunc">{s.label}</span><span class="bar"><i style={{ width: `${Math.round(s.mb / total() * 100)}%` }}></i></span><span class="tnum muted ws-right">{sizeText(s.mb)}</span></div>}</For></div>
    </Show>
    <div class="set-h">Old tasks outside the Archive</div>
    <div class="surface"><Srow t="Clear old tasks" d="Pick a date, preview what would go, then confirm. Never runs by itself."><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("clear_old")} onClick={() => { if (!off()) ws.open({ kind: "clear-old" }); }}>Clear old tasks…</button></Srow></div>
  </Page>;
}

function Diagnostics() {
  const ws = useWs();
  const m = () => ws.machine(), off = useReadOnly();
  const run = () => m().diagnostics;
  const items = () => run()?.items ?? [];
  const recovery = (item: NonNullable<NonNullable<Machine["diagnostics"]>["items"]>[number]) => item.action || (item.ok ? "" : item.name === "npm" ? "Make npm available to the desktop service (not just your terminal), restart the service, then run diagnostics again." : "Run diagnostics again for current details and recovery steps.");
  const report = () => `Diagnostics. Machine: ${m().name}. Runtime: ${m().version ?? "unknown"}. Result: ${run()?.status ?? "not run"}, ${run()?.checks ?? 0} checks.\n${items().map(item => `${item.name}: ${item.ok ? "OK" : "Needs attention"}. ${item.detail}. ${recovery(item)}`).join("\n")}${!items().length && run() ? "\nDetails were not retained. Run diagnostics again." : ""}${ws.t("reportNote")}`;
  return <Page title="Diagnostics" lead="Check that this machine can run tasks.">
    <div class="surface">
      <Srow t="Run diagnostics" d={run() ? `Last run ${clock(run()!.at)}: ${run()!.status}, ${run()!.checks} checks.` : "Not run on this machine yet."}>
        <Show when={run()}>{r => <span class={`st ${r().status === "ok" ? "st-ok" : "st-warn"}`}><I n={r().status === "ok" ? "check-circle" : "alert"} s={12} />{r().status === "ok" ? "Ok" : r().status === "degraded" ? "Degraded" : "Failed"}</span>}</Show>
        <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("run_diagnostics")} onClick={() => { if (!off()) ws.run({ type: "run_diagnostics", machineId: m().id, now: ws.store.clock() }); }}>Run</button>
      </Srow>
      <For each={items()}>{item => <Srow t={item.name} d={`${item.detail || (item.ok ? "Available" : "Needs attention")}${recovery(item) ? `. ${recovery(item)}` : ""}`}><span class={item.ok ? "st st-ok" : "st st-warn"}>{item.ok ? "OK" : "Needs attention"}</span></Srow>}</For>
      <Show when={run() && !items().length}><Srow t="Details unavailable" d="This older diagnostic did not retain details. Run diagnostics again to see which checks need attention and what to do." /></Show>
      <Srow t="Report" d="Copies a report with no secrets in it."><button class="btn btn-line btn-sm" onClick={() => { void navigator.clipboard?.writeText(report()); }}><I n="copy" s={12} />Copy report</button></Srow>
      <Show when={m().version}><Srow t="Build" d="The exact package on this machine."><span class="mono cap">{m().version}</span></Srow></Show>
      <For each={m().connections}>{c => <Srow t={`${c.label} connection`} d="Identifier for support."><span class="mono cap">{c.detail.connectionId}</span></Srow>}</For>
      <Srow t="Logs" d="Private, rotating service events only: timestamps and fixed status categories. No prompts, environment, commands or credentials."><button class="btn btn-line btn-sm" aria-disabled={ws.noOpener("workspace.open_logs", "noLogOpener") ? "true" : undefined} data-why={ws.noOpener("workspace.open_logs", "noLogOpener")} onClick={() => { if (!ws.noOpener("workspace.open_logs", "noLogOpener")) ws.run({ type: "open_logs", machineId: m().id }); }}>Open logs</button></Srow>
    </div>
  </Page>;
}

const PAIR_SECONDS = 252;
function RemoteAccess() {
  const ws = useWs();
  if (ws.store.mode === "real") return <Page title="Remote access" lead="Remote machine pairing needs a relay server selected by the owner."><div class="note-box plain">Remote pairing is not implemented. No machine code, fingerprint or reachable-server claim is fabricated. Local projects continue to work.</div></Page>;
  const m = () => ws.machine(), off = useReadOnly();
  const access = () => m().remoteAccess;
  const [left, setLeft] = createSignal(PAIR_SECONDS);
  createEffect(() => {
    if (!access().allowed) return;
    const timer = setInterval(() => setLeft(s => Math.max(0, s - 1)), 1000);
    onCleanup(() => clearInterval(timer));
  });
  const time = () => duration(left() * 1000);
  const revoke = (id: string | null, name: string) => ws.open({ kind: "confirm", title: id ? `Revoke ${name}?` : "Revoke every device?", danger: true, action: "Revoke", note: "It has to be paired again to connect.", go: () => ws.run({ type: "revoke_device", machineId: m().id, deviceId: id, confirmed: true }).then(r => r.ok) });
  return <Page title="Remote access" lead="Let your other devices connect to this machine through the server connector.">
    <div class="surface">
      <Srow t="Allow remote access" d="Off by default. Nothing connects until both sides confirm."><Sw label="Allow remote access" checked={access().allowed} disabled={off()} onChange={allowed => ws.run({ type: "set_remote_access", machineId: m().id, allowed })} /></Srow>
      <Show when={access().allowed}><Srow t="Server connector" d="Relays the connection. It cannot read task content on its own."><span class="st st-ok"><I n="check-circle" s={12} />Reachable</span></Srow></Show>
    </div>
    <Show when={access().allowed}>
      <div class="set-h">Pairing code</div>
      <div class="pair">7KQ2-M9XD</div>
      <div class="cap faint"><Show when={left() > 0} fallback={<>This code expired. <button class="lnk" onClick={() => setLeft(PAIR_SECONDS)}>Make a new one</button></>}>Expires in {time()}. Enter it on the other machine, then compare the fingerprints on both sides.</Show></div>
    </Show>
    <div class="set-h">Paired devices<Show when={access().devices.length > 0}><span class="r"><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} {...ws.needs("revoke_device")} onClick={() => { if (!off()) revoke(null, ""); }}>Revoke all…</button></span></Show></div>
    <Show when={access().devices.length > 0} fallback={<div class="surface ws-empty">No device is paired with this machine.</div>}>
      <div class="surface"><For each={access().devices}>{d => <Srow t={d.name} d={`Paired ${d.paired}${d.lastSeen ? `, last seen ${d.lastSeen}` : ""}.`}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} {...ws.needs("revoke_device")} onClick={() => { if (!off()) revoke(d.id, d.name); }}>Revoke…</button></Srow>}</For></div>
    </Show>
  </Page>;
}

export function SettingsPage(props: { route: Extract<Route, { name: "settings" }> }) {
  return <div class="set-wrap"><Switch>
    <Match when={props.route.section === "appearance"}><Appearance /></Match>
    <Match when={props.route.section === "voice"}><VoiceSettings /></Match>
    <Match when={props.route.section === "updates"}><Updates /></Match>
    <Match when={props.route.section === "shortcuts"}><Shortcuts /></Match>
    <Match when={props.route.section === "machines"}><Machines /></Match>
    <Match when={props.route.section === "overview"}><Overview /></Match>
    <Match when={props.route.section === "orchestration"}><Orchestration page={props.route.page} server={props.route.server} tab={props.route.tab} /></Match>
    <Match when={props.route.section === "connections"}><Connections /></Match>
    <Match when={props.route.section === "worktrees"}><Worktrees /></Match>
    <Match when={props.route.section === "storage"}><Storage /></Match>
    <Match when={props.route.section === "diagnostics"}><Diagnostics /></Match>
    <Match when={props.route.section === "remote"}><RemoteAccess /></Match>
  </Switch></div>;
}
