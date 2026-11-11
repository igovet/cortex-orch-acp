import { createEffect, createMemo, createSignal, For, Match, onCleanup, Show, Switch } from "solid-js";
import { accessFor, ENGINE_KEYS, engineLabel, ENGINES, mcpProblems, mcpTools, newMcpServer, parseCommandLine, pathMissing, ROLE_CATALOG, serverStatus, setCoordinatorMode, setGrant, setLevel, suggestLevels, toolsDiff, toolWarnings, type ConnectOutcome, type ConnectResult, type EngineKey, type GrantColumn, type McpEnv, type McpObservation, type McpServer, type Problem, type ToolLevel, type ToolRow } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, Count, I, MenuItem, Pop } from "./kit";
import { ActionBar } from "./layout";
import { HELP } from "./orch-help";
import { Bar, orchTo, Shell, slug, useMcp, useReadOnly } from "./orch-shared";
import { clock } from "./selectors";
import { Adv, Chk, Err, Hint, Lines, Lock, Pick, Seg, SetH, Srow, Sw, Tabs, Tags } from "./settings-kit";

const allServer = (s: McpServer) => s.engines ? s.engines.map(engineLabel).join(", ") : "All";
const counts = (s: McpServer) => `${s.allowedTools.length} writing · ${s.readOnlyTools.length} read-only · ${(s.coordinatorTools ?? s.readOnlyTools).length} coordinator`;
const basename = (path: string) => path.split("/").at(-1) ?? path;
const patchServer = (list: McpServer[], name: string, fn: (s: McpServer) => McpServer) => list.map(s => s.name === name ? fn(s) : s);

/** The whole tool list on one screen. Every cell knows its rung, so a tick can only ever produce legal lists. */
function Matrix(props: { server: McpServer; seen: McpObservation | null; set: (s: McpServer) => void; off?: string; check?: () => void; checking?: boolean; problems: Problem[]; prefix: string }) {
  const ws = useWs();
  const s = () => props.server;
  const rows = createMemo(() => mcpTools(s(), props.seen));
  const warnings = createMemo(() => toolWarnings(s(), rows()));
  const [query, setQuery] = createSignal("");
  const [show, setShow] = createSignal<"all" | "granted" | "new" | "gone" | "warnings">("all");
  const [chosen, setChosen] = createSignal<string[]>([]);
  const [note, setNote] = createSignal("");
  const [asRole, setAsRole] = createSignal("worker-security-auditor");
  const [asEngine, setAsEngine] = createSignal<EngineKey>("claude-agent-acp");
  const flagged = (name: string) => warnings().some(w => w.tool === name);
  const visible = createMemo(() => {
    const q = query().trim().toLowerCase(), f = show();
    return rows().filter(r => (!q || `${r.name} ${r.description ?? ""}`.toLowerCase().includes(q)) && (f === "all" || (f === "granted" && r.level !== "off") || (f === "new" && r.state === "new") || (f === "gone" && r.state === "not_offered") || (f === "warnings" && flagged(r.name))));
  });
  const how = (f: typeof show extends () => infer T ? T : never) => f === "all" ? rows().length : f === "granted" ? rows().filter(r => r.level !== "off").length : f === "new" ? rows().filter(r => r.state === "new").length : f === "gone" ? rows().filter(r => r.state === "not_offered").length : rows().filter(r => flagged(r.name)).length;
  const tick = (tool: string, column: GrantColumn, on: boolean) => { const r = setGrant(s(), tool, column, on); setNote(r.note ?? ""); props.set(r.server); };
  const bulk = (level: ToolLevel) => { props.set(setLevel(s(), chosen(), level)); setNote(`${chosen().length} tool${chosen().length === 1 ? "" : "s"} set.`); setChosen([]); };
  const noCoordinator = () => s().roles && !s().roles!.includes("coordinator") ? "This server is for workers only, so its coordinator list has no effect" : undefined;
  const mode = () => s().coordinatorTools ? "choose" : "same";
  const who = () => !s().roles ? "both" : s().roles!.includes("coordinator") ? "coordinator" : "workers";
  const setEngine = (e: EngineKey, on: boolean) => {
    const now = s().engines ?? [...ENGINE_KEYS], next = on ? ENGINE_KEYS.filter(x => x === e || now.includes(x)) : now.filter(x => x !== e);
    if (next.length) props.set({ ...s(), engines: next.length === ENGINE_KEYS.length ? null : next });
  };
  const gets = createMemo(() => accessFor(s(), asRole(), asEngine()));
  const why = () => { const { reason, granted } = gets(); return !reason ? "The exact result for one role on one engine." : granted.length ? `${reason} ${granted.length} granted below.` : reason; };
  const key = (e: KeyboardEvent) => {
    const cell = (e.target as HTMLElement).closest<HTMLElement>("[data-cell]");
    if (!cell) return;
    const [r, c] = cell.dataset.cell!.split(":").map(Number);
    const to = { ArrowUp: [r - 1, c], ArrowDown: [r + 1, c], ArrowLeft: [r, c - 1], ArrowRight: [r, c + 1] }[e.key];
    if (!to) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-cell="${to[0]}:${to[1]}"]`)?.focus();
  };
  return <>
    <div class="surface">
      <Srow t="Who gets this server" d="Unset means coordinator and workers."><Seg label="Who gets this server" value={who()} disabled={props.off} options={[{ id: "both", label: "Coordinator and workers" }, { id: "workers", label: "Workers only" }, { id: "coordinator", label: "Coordinator only" }] as const} onChange={x => props.set({ ...s(), roles: x === "both" ? null : x === "workers" ? ["worker"] : ["coordinator"] })} /></Srow>
      <Srow t="Engines" d="Sessions of these engines receive the server. To offer nothing, turn the server off."><For each={ENGINES}>{e => <label class="ws-check"><Chk label={`Engine ${e.label}`} checked={!s().engines || s().engines!.includes(e.id)} disabled={props.off ?? ((s().engines?.length === 1 && s().engines![0] === e.id) ? "Keep one engine. To offer nothing, turn the server off." : undefined)} onChange={on => setEngine(e.id, on)} /><span>{e.label}</span></label>}</For></Srow>
      <Srow t="Coordinator tools" d={s().coordinatorTools?.length === 0 ? "The coordinator will not get this server." : "Same as read-only follows that list."}><Seg label="Coordinator tools" value={mode()} disabled={props.off ?? noCoordinator()} options={[{ id: "same", label: "Same as read-only" }, { id: "choose", label: "Choose" }] as const} onChange={x => props.set(setCoordinatorMode(s(), x))} /></Srow>
      <Srow t="Check as" d={why()}>
        <Pick id={`m-as-role-${props.prefix}`} label="Check as role" width="170px" value={asRole()} options={[{ id: "coordinator", label: "Coordinator" }, ...ROLE_CATALOG.map(r => ({ id: r.id, label: r.label }))]} onPick={setAsRole} />
        <span class="faint">on</span>
        <Pick id={`m-as-engine-${props.prefix}`} label="Check on engine" width="110px" value={asEngine()} options={ENGINES.map(e => ({ id: e.id, label: e.label }))} onPick={x => setAsEngine(x as EngineKey)} />
        <span class="tnum ws-gets">{gets().tools.length} of {s().allowedTools.length} tools</span>
      </Srow>
    </div>
    <div class="ws-mtool">
      <input class="inp ws-filter" aria-label="Search tools" placeholder="Search tools" value={query()} onInput={e => setQuery(e.currentTarget.value)} />
      <div class="seg" role="group" aria-label="Show tools"><For each={[["all", "All"], ["granted", "Granted"], ["new", "New"], ["gone", "Not offered"], ["warnings", "Warnings"]] as const}>{([id, label]) => <button aria-current={show() === id ? "true" : "false"} onClick={() => setShow(id)}>{label} <Count n={how(id)} /></button>}</For></div>
      <span class="ws-push"></span>
      <button class="btn btn-line btn-sm" popovertarget={chosen().length && !props.off ? `m-bulk-${props.prefix}` : undefined} aria-disabled={chosen().length && !props.off ? undefined : "true"} data-why={props.off ?? (chosen().length ? undefined : "Tick tools on the left first")}>Set selected<I n="chevron-down" s={12} /></button>
      <Pop id={`m-bulk-${props.prefix}`} class="pop-right" align="right" label="Set selected">
        <MenuItem label="Off" onSelect={() => bulk("off")} /><MenuItem label="Writing only" onSelect={() => bulk("writing")} /><MenuItem label="Read-only too" onSelect={() => bulk("read_only")} /><MenuItem label="Coordinator too" disabled={s().coordinatorTools ? undefined : "Choose coordinator tools first"} onSelect={() => bulk("coordinator")} />
      </Pop>
      <button class="btn btn-line btn-sm" aria-disabled={props.seen && !props.off ? undefined : "true"} data-why={props.off ?? (props.seen ? undefined : "Check the server first")} onClick={() => { if (props.seen && !props.off) { props.set(suggestLevels(s(), rows())); setNote("Server says: filled from the server's hints. Hints are claims, so read them before saving."); } }}>Suggest from server</button>
      <Show when={props.check}><button class="btn btn-line btn-sm" aria-disabled={props.off || props.checking || (!ws.demo && !ws.store.discoverMcp) ? "true" : undefined} data-why={props.off} onClick={() => { if (!props.off && !props.checking) props.check!(); }}>Check tools</button></Show>
    </div>
    <div class="cap muted ws-note" role="status" aria-live="polite">{note()}</div>
    <div class="surface ws-mwrap"><div class="gt ws-matrix" role="grid" aria-label={`Tool access for ${s().name || "this server"}`} onKeyDown={key}>
      <div class="gr omxrow th" role="row"><span role="columnheader"><Chk label="Select every shown tool" checked={visible().length > 0 && visible().every(r => chosen().includes(r.name))} onChange={on => setChosen(on ? visible().map(r => r.name) : [])} /></span><span role="columnheader">Tool</span><span role="columnheader">Coordinator</span><span role="columnheader">Read-only</span><span role="columnheader">Writing</span><span role="columnheader">Consultant</span><span role="columnheader">Always load<span class="sub">Claude only</span></span><span role="columnheader"></span></div>
      <For each={visible()} fallback={<div class="ws-empty">No tool matches.</div>}>{(r: ToolRow, i) => {
        const lvl = () => r.level, read = () => lvl() === "read_only" || lvl() === "coordinator", coord = () => lvl() === "coordinator";
        const coordinatorOff = () => props.off ?? noCoordinator() ?? (s().coordinatorTools ? undefined : "Follows read-only. Choose coordinator tools to set this.");
        return <div class="gr omxrow" role="row">
          <span role="gridcell"><Chk label={`Select ${r.name}`} checked={chosen().includes(r.name)} data={`${i()}:0`} onChange={on => setChosen(c => on ? [...c, r.name] : c.filter(x => x !== r.name))} /></span>
          <span role="rowheader" class="trunc"><span class="mono w500 trunc">{r.name}</span><Show when={r.description}><span class="sub trunc">{r.description}</span></Show></span>
          <span role="gridcell"><Chk label={`${r.name}, coordinator, ${coord() ? "granted" : "not granted"}`} checked={coord()} disabled={coordinatorOff()} data={`${i()}:1`} onChange={on => tick(r.name, "coordinator", on)} /></span>
          <span role="gridcell"><Chk label={`${r.name}, read-only workers, ${read() ? "granted" : "not granted"}`} checked={read()} disabled={props.off} data={`${i()}:2`} onChange={on => tick(r.name, "read_only", on)} /></span>
          <span role="gridcell"><Chk label={`${r.name}, writing workers, ${lvl() !== "off" ? "granted" : "not granted"}`} checked={lvl() !== "off"} disabled={props.off} data={`${i()}:3`} onChange={on => tick(r.name, "writing", on)} /></span>
          <span role="gridcell" class="cap faint">Never</span>
          <span role="gridcell"><Chk label={`${r.name}, always load, ${r.always ? "on" : "off"}`} checked={r.always} disabled={props.off ?? (lvl() === "off" ? "Grant the tool first" : undefined)} data={`${i()}:4`} onChange={on => tick(r.name, "always", on)} /></span>
          <span class="ws-mstate">
            <Show when={r.state === "new"}><span class="chip ws-static">New</span></Show>
            <Show when={r.state === "not_offered"}><span class="chip ws-static st-warn">Not offered</span><button class="ws-att-x" aria-label={`Remove ${r.name}`} onClick={() => { if (!props.off) props.set(setLevel(s(), [r.name], "off")); }}><I n="x" s={12} /></button></Show>
            <Show when={r.state === "granted" && !r.checked}><span class="cap faint">Not checked</span></Show>
            <Show when={flagged(r.name)}><span class="st-warn" aria-label={warnings().find(w => w.tool === r.name)?.text}><I n="alert" s={12} /></span></Show>
          </span>
        </div>;
      }}</For>
    </div></div>
    <Err problems={props.problems} path={`${props.prefix}.allowed_tools`} />
    <Show when={warnings().length > 0}><div class="ws-warns" role="status"><For each={warnings()}>{w => <div class="note-box"><I n="alert" s={14} /><span><Show when={w.tool}><span class="mono">{w.tool}: </span></Show>{w.text}</span></div>}</For></div></Show>
  </>;
}

function EnvRows(props: { items: McpEnv[]; off?: string; set: (items: McpEnv[]) => void; problems: Problem[]; prefix: string }) {
  const at = (i: number, patch: Partial<McpEnv>) => props.set(props.items.map((v, n) => n === i ? { ...v, ...patch } : v));
  return <div class="ws-lines">
    <For each={props.items}>{(v, i) => <div class="ws-line">
      <div class="ws-line-r">
        <input class="inp mono" aria-label={`Variable ${i() + 1} name`} value={v.name} disabled={!!props.off || v.stored} onChange={e => at(i(), { name: e.currentTarget.value })} />
        <Show when={v.stored} fallback={<input class="inp mono" type="password" autocomplete="off" aria-label={`Value of ${v.name || "variable"}`} value={v.value} disabled={!!props.off} onChange={e => at(i(), { value: e.currentTarget.value, action: "replace" })} />}>
          <Seg label={`Value of ${v.name}`} value={v.action} disabled={props.off} options={[{ id: "keep", label: "Keep" }, { id: "replace", label: "Replace" }, { id: "remove", label: "Remove" }] as const} onChange={action => at(i(), { action, value: "" })} />
        </Show>
        <Show when={!v.stored}><button class="ib ws-ib" aria-label={`Remove variable ${i() + 1}`} onClick={() => props.set(props.items.filter((_, n) => n !== i()))}><I n="x" s={14} /></button></Show>
      </div>
      <Show when={v.stored && v.action === "keep"}><span class="cap muted">Value stored, never shown</span></Show>
      <Show when={v.stored && v.action === "replace"}><input class="inp mono" type="password" autocomplete="off" aria-label={`New value of ${v.name}`} value={v.value} disabled={!!props.off} onChange={e => at(i(), { value: e.currentTarget.value })} /></Show>
      <Err problems={props.problems} path={`${props.prefix}.env[${i()}].name`} /><Err problems={props.problems} path={`${props.prefix}.env[${i()}].value`} />
    </div>}</For>
    <Show when={!props.off}><div><button class="btn btn-line btn-sm" onClick={() => props.set([...props.items, { name: "", stored: false, action: "replace", value: "" }])}><I n="plus" s={12} />Add variable</button></div></Show>
  </div>;
}

function SandboxFields(props: { s: McpServer; set: (s: McpServer) => void; off?: string; problems: Problem[]; prefix: string }) {
  const missing = (p: string) => pathMissing(p) ? "The server is dropped from sessions until this exists." : null;
  return <>
    <Srow t="Network" d="Off by default."><Sw label="Network" checked={props.s.network} disabled={props.off} onChange={network => props.set({ ...props.s, network })} /></Srow>
    <div class="ws-pad ws-fld"><span class="lab">Read paths</span><Lines label="Read path" add="Add read path" items={props.s.readPaths} disabled={props.off} note={missing} problems={props.problems} path={`${props.prefix}.read_paths`} onChange={readPaths => props.set({ ...props.s, readPaths })} /></div>
    <div class="ws-pad ws-fld"><span class="lab">Write paths</span><span class="cap muted">Only writable state beyond the task scope.</span><Lines label="Write path" add="Add write path" items={props.s.writePaths} disabled={props.off} note={missing} problems={props.problems} path={`${props.prefix}.write_paths`} onChange={writePaths => props.set({ ...props.s, writePaths })} /></div>
    <div class="ws-pad ws-fld"><span class="lab">Runtime directory variables</span><Tags label="Runtime directory variable" mono hint="NAME_OF_VARIABLE" items={props.s.runtimeDirEnv} disabled={props.off} onChange={runtimeDirEnv => props.set({ ...props.s, runtimeDirEnv })} /><Err problems={props.problems} path={`${props.prefix}.runtime_dir_env`} /></div>
  </>;
}
const REDACTED = (a: string) => a === "[redacted]" ? "Keeps its stored value. Replace the whole part to change it." : null;
function ConnectionFields(props: { s: McpServer; set: (s: McpServer) => void; off?: string; problems: Problem[]; prefix: string; isNew?: boolean }) {
  const [line, setLine] = createSignal("");
  const [refused, setRefused] = createSignal("");
  const paste = () => {
    const r = parseCommandLine(line());
    if (!r.ok) { setRefused(r.message); return; }
    props.set({ ...props.s, command: r.command, args: r.args }); setLine(""); setRefused("");
  };
  return <>
    <Show when={props.isNew} fallback={<Srow t="Name" d="Duplicate to make a copy under a new name."><span class="mono">{props.s.name}</span></Srow>}>
      <Srow t="Name" d="Letters, digits, dot, dash or underscore."><input class="inp mono ws-path" aria-label="Server name" value={props.s.name} disabled={!!props.off} onInput={e => props.set({ ...props.s, name: e.currentTarget.value })} /></Srow>
      <div class="ws-errline"><Err problems={props.problems} path={`${props.prefix}.name`} /></div>
    </Show>
    <Srow t="Command" d="The program that starts the server."><input class="inp mono ws-path" aria-label="Command" value={props.s.command} disabled={!!props.off} onInput={e => props.set({ ...props.s, command: e.currentTarget.value })} /></Srow>
    <div class="ws-errline"><Err problems={props.problems} path={`${props.prefix}.command`} /></div>
    <div class="ws-pad ws-fld"><span class="lab">Arguments</span><Lines label="Argument" add="Add argument" items={props.s.args} disabled={props.off} note={REDACTED} onChange={args => props.set({ ...props.s, args })} />
      <Show when={!props.off}><div class="ws-line-r"><textarea class="inp mono" rows={1} wrap="off" aria-label="Paste a command line" aria-invalid={refused() ? "true" : undefined} placeholder="Paste a command line" value={line()} onInput={e => { setLine(e.currentTarget.value); setRefused(""); }} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); paste(); } }} /><button class="btn btn-line btn-sm" onClick={paste}>Split</button></div><Show when={refused()}><span class="ws-err" role="alert">{refused()}</span></Show></Show>
    </div>
    <div class="ws-pad ws-fld"><span class="lab">Environment<Hint text={HELP.mcpServer} label="MCP server fields" /></span><EnvRows items={props.s.env} off={props.off} problems={props.problems} prefix={props.prefix} set={env => props.set({ ...props.s, env })} /></div>
    <Srow t="Transport" d="Servers start as local processes."><span>Local process (stdio)</span><Lock /></Srow>
  </>;
}

/** Connecting is a state of its own, so a slow start reads as such and can be left. */
function useConnect(server: () => McpServer, done: (r: ConnectResult) => void) {
  const ws = useWs(), off = useReadOnly();
  const [busy, setBusy] = createSignal(false);
  const [forced, setForced] = createSignal<ConnectOutcome | "auto">("auto");
  const [result, setResult] = createSignal<ConnectResult | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cancel = () => { ++attempt; clearTimeout(timer); setBusy(false); };
  let attempt = 0;
  const start = () => {
    if (off()) { const r: ConnectResult = { ok: false, code: "offline", message: ws.machine().connection === "connected" ? ws.t("noConnect") : "This machine is offline.", manual: true }; setResult(r); done(r); return; }
    const connect = ws.demo?.connect;
    if (!connect && ws.store.discoverMcp) {
      const id = ++attempt, draft = structuredClone(server()), identity = JSON.stringify(draft);
      setBusy(true); setResult(null);
      void ws.store.discoverMcp(draft).then(r => {
        if (id !== attempt) return;
        if (JSON.stringify(server()) !== identity) {
          setResult({ok:false,code:"not_supported",message:"The connection changed while checking. Retry the current draft."});
          return;
        }
        setResult(r); done(r);
      }).catch(() => { if (id === attempt) setResult({ok:false,code:"not_supported",message:"Tool discovery did not answer. The sandbox deadline stops the server; retry after checking the connection."}); })
        .finally(() => { if (id === attempt) setBusy(false); });
      return;
    }
    if (!connect) { const r: ConnectResult = { ok: false, code: "not_supported", message: ws.t("noConnect"), manual: true }; setResult(r); done(r); return; }
    setBusy(true); setResult(null);
    timer = setTimeout(() => { setBusy(false); const f = forced(); const r = connect.run(server(), f === "auto" ? null : f, ws.store.clock()); setResult(r); done(r); }, connect.delayMs);
  };
  onCleanup(() => { ++attempt; clearTimeout(timer); });
  return { busy, result, start, cancel, forced, setForced };
}
const Outcome = (props: { c: ReturnType<typeof useConnect>; id: string }) => { const ws = useWs(); return <>{ws.demo?.connect.outcome(props.c.forced, props.c.setForced, props.id)}</>; };

const ADD_TABS = [{ id: "define", label: "1 Define" }, { id: "connect", label: "2 Connect" }, { id: "access", label: "3 Tool access" }];
/** A server cannot be saved before its tools are known, so the whole flow runs on a draft that exists only here. */
function AddServer(props: { tab?: string }) {
  const ws = useWs();
  const d = useMcp(), off = useReadOnly();
  const m = () => ws.machine();
  const key = () => `${m().id}:mcp_new`, seenKey = () => `${m().id}:mcp_new_seen`;
  const s = () => (ws.settings.draft(key()) as McpServer | undefined) ?? newMcpServer();
  const set = (next: McpServer) => ws.settings.setDraft(key(), next);
  const seen = () => (ws.settings.draft(seenKey()) as McpObservation | undefined) ?? null;
  const templates = () => ws.demo?.connect.templates() ?? [];
  const [template, setTemplate] = createSignal("blank");
  const [hand, setHand] = createSignal(false);
  const problems = createMemo(() => mcpProblems(s(), d.value(), true).map(p => ({ ...p, path: `new.${p.path}` })));
  const defined = () => !problems().some(p => /\.(name|command)$/.test(p.path));
  const tab = () => ADD_TABS.some(t => t.id === props.tab) ? props.tab! : "define";
  const go = (id: string) => ws.go(orchTo(m().id, "tools", { server: "new", tab: id }));
  const c = useConnect(s, r => {
    if (r.ok) ws.settings.setDraft(seenKey(), r.observation);
    else if (r.manual) setHand(true);
  });
  const pick = (id: string) => { setTemplate(id); const t = templates().find(x => x.name === id); set(t ? { ...t } : newMcpServer()); ws.settings.setDraft(seenKey(), undefined); };
  const others = () => d.count();
  const finish = async (enabled: boolean) => {
    if (problems().length || off()) return;
    const name = s().name, value = { ...s(), enabled }, observed = seen();
    d.set([...d.value(), value]);
    if (await d.save()) {
      if (observed && ws.demo) ws.run({ type: "record_mcp_tools", machineId: m().id, server: name, observation: observed });
      ws.settings.setDraft(key(), undefined); ws.settings.setDraft(seenKey(), undefined);
      ws.go(orchTo(m().id, "tools", { server: name, tab: "tools" }));
    } else d.set(d.value().filter(x => x.name !== name));
  };
  const blocked = () => off() ?? problems()[0]?.message;
  return <Shell wide title="Add a server" lead="Define it, connect once to list its tools, then decide who may use each one.">
    <Tabs label="Add a server" tabs={ADD_TABS.map(t => ({ ...t, off: t.id !== "define" && !defined() ? "Name the server and its command first" : undefined }))} current={tab()} route={id => orchTo(m().id, "tools", { server: "new", tab: id })} />
    <Switch>
      <Match when={tab() === "define"}>
        <div class="surface">
          <Srow t="Start from" d="Fills the fields with a known server."><Pick id="m-add-template" label="Start from" width="200px" value={template()} options={[{ id: "blank", label: "Blank" }, ...templates().map(t => ({ id: t.name, label: t.name }))]} onPick={pick} /></Srow>
          <ConnectionFields s={s()} set={set} off={off()} problems={problems()} prefix="new" isNew />
        </div>
        <Adv id="add-sandbox" label="Sandbox" force={s().readPaths.length > 0 || s().writePaths.length > 0 || s().network}><div class="surface"><SandboxFields s={s()} set={set} off={off()} problems={problems()} prefix="new" /></div></Adv>
        <ActionBar class="ws-form-f"><button class="btn btn-primary" aria-disabled={defined() ? undefined : "true"} data-why={defined() ? undefined : "Name the server and its command first"} onClick={() => { if (defined()) go("connect"); }}>Next: Connect</button></ActionBar>
      </Match>
      <Match when={tab() === "connect"}>
        <div class="surface ws-pad ws-connect">
          <Switch>
            <Match when={c.busy()}><div class="row"><I n="refresh" s={14} class="spin" /><span>Starting the server…</span><span class="ws-push"></span><button class="btn btn-line btn-sm" onClick={c.cancel}>{ws.demo ? "Cancel" : "Dismiss (server stops at deadline)"}</button></div></Match>
            <Match when={c.result()?.ok && seen()}>
              <div class="row"><span class="st st-ok"><I n="check-circle" s={12} />Connected</span><span class="w500">{seen()!.name}</span><span class="cap muted tnum">{seen()!.version}</span><span class="tnum">{seen()!.tools.length} tools</span><span class="cap muted tnum">Observed {clock(seen()!.at)}</span><span class="ws-push"></span><button class="btn btn-primary btn-sm" onClick={() => go("access")}>Go to Tool access</button></div>
            </Match>
            <Match when={c.result() && !c.result()!.ok}>
              <div class="note-box bad" role="alert"><I n="x-circle" s={14} /><span>{(c.result() as { message: string }).message}</span></div>
              <div class="row ws-gap4"><button class="btn btn-line btn-sm" onClick={() => go("define")}>Edit</button><button class="btn btn-line btn-sm" aria-disabled={!ws.demo && !ws.store.discoverMcp ? "true" : undefined} data-why={!ws.demo && !ws.store.discoverMcp ? ws.t("noConnect") : undefined} onClick={c.start}>Retry</button></div>
            </Match>
            <Match when={true}><p>Starts the server in its sandbox and lists its tools. It calls none of them.</p><div><button class="btn btn-primary" aria-disabled={!ws.demo && !ws.store.discoverMcp ? "true" : undefined} data-why={!ws.demo && !ws.store.discoverMcp ? ws.t("noConnect") : undefined} onClick={c.start}>Connect and list tools</button></div></Match>
          </Switch>
          <Outcome c={c} id="m-add-outcome" />
          <p class="cap muted">{ws.t("connectNote")}</p>
        </div>
        <Show when={hand() || (c.result() && !c.result()!.ok)}>
          <SetH title="Enter tool names by hand" />
          <div class="surface ws-pad ws-fld"><span class="cap muted">These rows carry Not checked until a check sees them.</span><Tags label="Tool name" mono hint="exact_tool_name" items={s().handTools} onChange={items => set(setLevel({ ...s(), handTools: items }, items, "writing"))} /><div><button class="btn btn-line btn-sm" aria-disabled={s().handTools.length ? undefined : "true"} data-why={s().handTools.length ? undefined : "Type a tool name first"} onClick={() => { if (s().handTools.length) go("access"); }}>Go to Tool access</button></div></div>
        </Show>
      </Match>
      <Match when={tab() === "access"}>
        <Matrix server={s()} seen={seen()} set={set} off={off()} problems={problems()} prefix="new" />
        {/* Why saving is refused takes the bar's words slot; otherwise it says what else the save takes along. */}
        <ActionBar class="ws-form-f" hint={others() > 0 ? `Also saves ${others()} other unsaved MCP change${others() === 1 ? "" : "s"}.` : undefined}>
          <button class="btn btn-line" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} onClick={() => finish(false)}>Save, keep off</button>
          <button class="btn btn-primary" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} onClick={() => finish(true)}>Save and turn on</button>
        </ActionBar>
      </Match>
    </Switch>
  </Shell>;
}

const EDIT_TABS = [{ id: "connection", label: "Connection" }, { id: "tools", label: "Tools" }, { id: "sandbox", label: "Sandbox" }];
function EditServer(props: { name: string; tab?: string }) {
  const ws = useWs();
  const d = useMcp(), off = useReadOnly();
  const m = () => ws.machine();
  const s = createMemo(() => d.value().find(x => x.name === props.name));
  const [observed, setObserved] = createSignal<McpObservation | null>(null);
  const seen = () => observed() ?? m().config.mcpObserved[props.name] ?? null;
  const set = (next: McpServer) => d.set(patchServer(d.value(), props.name, () => next));
  const tab = () => EDIT_TABS.some(t => t.id === props.tab) ? props.tab! : "tools";
  const locked = () => off() ?? (s()?.unsupportedKeys.length ? "Has settings this app does not manage. Turn on, off or remove only." : undefined);
  const problems = () => d.problems();
  const [diff, setDiff] = createSignal<string | null>(null);
  const [failed, setFailed] = createSignal<string | null>(null);
  const c = useConnect(() => s()!, r => {
    if (r.ok) {
      setObserved(r.observation);
      if (ws.demo) ws.run({ type: "record_mcp_tools", machineId: m().id, server: props.name, observation: r.observation });
      const x = toolsDiff(mcpTools(s()!, r.observation));
      setDiff(x.fresh || x.gone ? `${x.fresh} new, ${x.gone} no longer offered` : "Nothing changed on the server"); setFailed(null);
    } else { setFailed(r.message); setDiff(null); }
  });
  createEffect(() => { props.name; setObserved(null); setDiff(null); setFailed(null); });
  const remove = () => ws.open({ kind: "confirm", title: `Remove ${props.name}?`, danger: true, action: "Remove", note: "It leaves the draft. Save to write that. Running tasks keep their tools.", go() { d.set(d.value().filter(x => x.name !== props.name)); ws.go(orchTo(m().id, "tools")); return true; } });
  const duplicate = () => ws.open({ kind: "prompt", title: `Duplicate ${props.name}`, label: "New server name", action: "Duplicate", hint: "Use this to give the same server different engines.", go(name) { const copy = { ...structuredClone(s()!), name, enabled: false, env: [] }; if (mcpProblems(copy, d.value(), true).some(p => p.path === "name")) return false; d.set([...d.value(), copy]); ws.go(orchTo(m().id, "tools", { server: name, tab: "tools" })); return true; } });
  return <Show when={s()} fallback={<Shell wide title="No such server" lead={`${props.name} is not on this machine.`}><div><A class="btn btn-line" to={orchTo(m().id, "tools")}>Back to Tools and MCP</A></div></Shell>}>
    {server => <Shell wide title={server().name} lead={server().command} bar={<Bar d={d} />} head={<div class="row ws-gap4">
      <span class={`st ${serverStatus(server()).tone}`}>{serverStatus(server()).text}</span>
      <Sw label={`${server().name} enabled`} checked={server().enabled} disabled={off()} onChange={enabled => set({ ...server(), enabled })} />
      <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) duplicate(); }}>Duplicate…</button>
      <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) remove(); }}>Remove…</button>
    </div>}>
      <div class="row"><A class="lnk" to={orchTo(m().id, "tools")}><I n="arrow-left" s={12} />All servers</A></div>
      <Tabs label={server().name} tabs={EDIT_TABS} current={tab()} route={id => orchTo(m().id, "tools", { server: props.name, tab: id })} />
      <Show when={locked() && !off()}><div class="note-box"><I n="lock" s={14} /><span>{locked()} Unsupported: {server().unsupportedKeys.join(", ")}.</span></div></Show>
      <Switch>
        <Match when={tab() === "connection"}>
          <div class="surface"><ConnectionFields s={server()} set={set} off={locked()} problems={problems()} prefix={props.name} /></div>
          <div class="cap faint">Command, arguments and environment are not part of profiles.</div>
        </Match>
        <Match when={tab() === "tools"}>
          <Show when={failed()}>{f => <div class="note-box bad" role="alert"><I n="x-circle" s={14} /><span>{f()} Stored grants stay, and the matrix stays editable.</span><button class="btn btn-line btn-sm" disabled={c.busy() || !!locked()} onClick={c.start}>Retry</button></div>}</Show>
          <Show when={c.busy()}><div class="note-box plain"><I n="refresh" s={14} class="spin" /><span>Starting the server…</span></div></Show>
          <Show when={diff()}>{x => <div class="note-box plain" role="status"><I n="check" s={14} /><span>{x()}. Nothing else changed.</span></div>}</Show>
          <div class="row ws-gap4"><Show when={seen()}>{o => <span class="cap muted tnum">Last check {clock(o().at)}, {o().name} {o().version}, {o().tools.length} tools</span>}</Show><span class="ws-push"></span><Outcome c={c} id="m-edit-outcome" /></div>
          <Matrix server={server()} seen={seen()} set={set} off={locked()} check={c.start} checking={c.busy()} problems={problems()} prefix={props.name} />
          <div class="cap faint">Saved in profiles. {ws.t("connectNote")}</div>
        </Match>
        <Match when={tab() === "sandbox"}>
          <div class="surface"><SandboxFields s={server()} set={set} off={locked()} problems={problems()} prefix={props.name} /></div>
          <div class="cap faint">Saved in profiles.</div>
        </Match>
      </Switch>
    </Shell>}
  </Show>;
}

function ServerList() {
  const ws = useWs();
  const d = useMcp(), off = useReadOnly();
  const m = () => ws.machine();
  const open = (name: string) => ws.go(orchTo(m().id, "tools", { server: name, tab: "tools" }));
  const check = (s: McpServer) => { const r: ConnectResult = ws.demo?.connect.run(s, null, ws.store.clock()) ?? { ok: false, code: "not_supported", message: ws.t("noConnect") }; if (r.ok) ws.run({ type: "record_mcp_tools", machineId: m().id, server: s.name, observation: r.observation }); else open(s.name); };
  const remove = (s: McpServer) => ws.open({ kind: "confirm", title: `Remove ${s.name}?`, danger: true, action: "Remove", note: "It leaves the draft. Save to write that. Running tasks keep their tools.", go() { d.set(d.value().filter(x => x.name !== s.name)); return true; } });
  const duplicate = (s: McpServer) => ws.open({ kind: "prompt", title: `Duplicate ${s.name}`, label: "New server name", action: "Duplicate", hint: "Use this to give the same server different engines.", go(name) { const copy = { ...structuredClone(s), name, enabled: false, env: [] }; if (mcpProblems(copy, d.value(), true).some(p => p.path === "name")) return false; d.set([...d.value(), copy]); return true; } });
  const add = () => { if (!off()) ws.go(orchTo(m().id, "tools", { server: "new" })); };
  return <Shell wide title="Tools and MCP" lead="External tools available on this machine, and who may use each one. Servers are per machine: a project cannot add its own." bar={<Bar d={d} />}>
    <SetH title="Servers"><Hint text={HELP.mcpIntro} label="MCP servers" /><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={add}><I n="plus" s={12} />Add server</button></SetH>
    <Show when={d.value().length > 0} fallback={<div class="surface ws-empty">No servers. Add one to give workers extra tools.</div>}>
      <div class="surface"><div class="gt">
        <div class="gr osrow th"><span>Server</span><span>Tools</span><span>Engines</span><span>Network</span><span>Checked</span><span>Enabled</span><span></span></div>
        <For each={d.value()}>{s => {
          const status = () => serverStatus(s), seen = () => m().config.mcpObserved[s.name];
          return <div class="gr osrow ws-pointer" onClick={() => open(s.name)}>
            <span class="trunc"><A class="ws-cellbtn w500 trunc" to={orchTo(m().id, "tools", { server: s.name, tab: "tools" })} onClick={e => e.stopPropagation()}>{s.name}</A><span class="sub mono trunc">{basename(s.command)}</span>
              <Show when={status().tone !== "st-ok" && status().tone !== "st-idle"}><span class={`sub trunc ${status().tone}`}>{status().text}</span></Show></span>
            <span class="cap muted tnum ws-wrap">{counts(s)}</span>
            <span class="cap muted trunc">{allServer(s)}</span>
            <span class="cap muted">{s.network ? "On" : "Off"}</span>
            <span class="cap muted tnum">{seen() ? clock(seen()!.at) : ""}</span>
            <span onClick={e => e.stopPropagation()}><Sw label={`${s.name} enabled`} checked={s.enabled} disabled={off()} onChange={enabled => d.set(patchServer(d.value(), s.name, x => ({ ...x, enabled })))} /></span>
            <span onClick={e => e.stopPropagation()}><button class="ib ws-ib" popovertarget={off() ? undefined : `m-srv-${slug(s.name)}`} aria-label={`More for ${s.name}`} aria-disabled={off() ? "true" : undefined} data-why={off()}><I n="more" s={14} /></button>
              <Pop id={`m-srv-${slug(s.name)}`} class="pop-right" align="right" label={s.name}>
                <MenuItem icon="pencil" label="Edit" onSelect={() => open(s.name)} />
                <MenuItem icon="refresh" label="Check tools" needs="record_mcp_tools" onSelect={() => check(s)} />
                <MenuItem icon="copy" label="Duplicate…" onSelect={() => duplicate(s)} />
                <MenuItem icon="trash" label="Remove…" danger onSelect={() => remove(s)} />
              </Pop></span>
          </div>;
        }}</For>
      </div></div>
    </Show>
    <div class="note-box plain"><I n="lock" s={14} /><span>The senior consultant never receives external servers. A grant is per server and per access class; engines apply to the whole server.</span></div>
  </Shell>;
}

export function Tools(props: { server?: string; tab?: string }) {
  return <Switch fallback={<ServerList />}>
    <Match when={props.server === "new"}><AddServer tab={props.tab} /></Match>
    <Match when={props.server}>{name => <EditServer name={name()} tab={props.tab} />}</Match>
  </Switch>;
}
