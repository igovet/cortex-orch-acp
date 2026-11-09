import { createMemo, createSignal, For, Show } from "solid-js";
import { configIssues, engineLabel, ENGINE_KEYS, matchesProfile, missingSections, PROFILE_SECTION_LABELS, PROFILE_SECTIONS, profileChanges, profileState, projectsWithRouting, routingSource, sessionsOf, type Profile } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I, MenuItem, NameEdit, Pop } from "./kit";
import { HELP } from "./orch-help";
import { orchTo, Shell, useProjectName, useReadOnly, useSwitchProfile } from "./orch-shared";
import { clock } from "./selectors";
import { Chk, Hint, Lock, Srow } from "./settings-kit";

/** The landing page: what is wrong first, then one line per page. */
export function Overview() {
  const ws = useWs(), name = useProjectName();
  const m = () => ws.machine(), c = () => m().config;
  const issues = createMemo(() => configIssues(m()));
  const rules = () => c().surfaces.routing;
  const engineRow = (e: typeof ENGINE_KEYS[number]) => { const r = rules().engines[e], total = c().catalogue[e].models.length; return `${engineLabel(e)}: ${r.enabled.length} of ${total} models on, ${r.auto.filter(id => r.enabled.includes(id)).length} automatic`; };
  const servers = () => c().surfaces.mcp_servers, on = () => servers().filter(s => s.enabled);
  const granted = () => on().reduce((n, s) => n + s.allowedTools.length, 0);
  const pins = () => Object.keys(rules().pins).length, leans = () => Object.keys(rules().affinity).length;
  const ceiling = () => c().surfaces.user_defaults.ceiling;
  const source = () => routingSource(m(), null), own = () => projectsWithRouting(m()).map(id => name(id));
  const coord = () => c().surfaces.user_defaults.coordinator;
  const rows = (): [Parameters<typeof orchTo>[1], string, string][] => [
    ["defaults", "Coordinator", coord() ? `${engineLabel(coord()!.engine)} ${coord()!.model}, ${coord()!.effort} effort` : "Exact route"],
    ["models", "Models", ENGINE_KEYS.filter(e => c().catalogue[e].models.length).map(engineRow).join(". ")],
    ["routing", "Routing", `${pins() ? `${pins()} pin${pins() === 1 ? "" : "s"}` : "No pins"}, ${leans() ? `${leans()} custom work kind${leans() === 1 ? "" : "s"}` : "built-in work kinds"}`],
    ["policy", "Limits and fallback", `${ceiling().attempts === null ? "No attempt limit" : `Attempts ${ceiling().attempts}`}, fallback ${ceiling().consent ? "allowed" : "off"}`],
    ["tools", "Tools and MCP", `${on().length} of ${servers().length} servers on, ${granted()} tools granted`],
  ];
  return <Shell title="Orchestration" lead="Everything needed to run tasks on this machine.">
    <Show when={issues().length > 0}>
      <div class="set-h">Needs attention</div>
      <div class="surface"><For each={issues()}>{i => <A class="srow ws-link-row" to={orchTo(m().id, i.page, i.server ? { server: i.server, tab: "sandbox" } : {})}><span><span class="t ws-w400">{i.text}</span></span><span class="ctl"><I n="chevron-right" s={14} class="faint" /></span></A>}</For></div>
    </Show>
    <div class="set-h">Summary</div>
    <div class="surface"><For each={rows()}>{([page, t, d]) => <A class="srow ws-link-row" to={orchTo(m().id, page)}><span><span class="t">{t}</span><span class="d">{d}</span></span><span class="ctl"><I n="chevron-right" s={14} class="faint" /></span></A>}</For></div>
    <div class="set-h">Source</div>
    <div class="surface"><A class="srow ws-link-row" to={orchTo(m().id, "files")}><span><span class="t">Routing file in use</span><span class="d mono">{source().path}</span><Show when={own().length > 0}><span class="d">Own routing file: {own().join(", ")}</span></Show></span><span class="ctl"><I n="chevron-right" s={14} class="faint" /></span></A></div>
  </Shell>;
}

const dayOf = (at: number | null) => at === null ? "" : clock(at);
export function Profiles() {
  const ws = useWs(), off = useReadOnly(), switchTo = useSwitchProfile();
  const m = () => ws.machine(), c = () => m().config, sessions = () => sessionsOf(m());
  const state = () => profileState(c(), sessions());
  const diffs = createMemo(() => profileChanges(m()));
  const run = (command: Parameters<typeof ws.run>[0]) => ws.run(command).then(r => r.ok);
  const saveNew = () => ws.open({ kind: "prompt", title: "Save as a new profile", label: "Profile name", action: "Save as new", hint: "Lowercase letters, digits and dashes.", go: name => run({ type: "create_profile", machineId: m().id, name, description: "" }) });
  const stored = (p: Profile) => PROFILE_SECTIONS.filter(s => p.sections[s] !== undefined).map(s => PROFILE_SECTION_LABELS[s]).join(", ");
  const active = () => c().profiles.find(p => p.id === c().active);
  const btn = (label: string, go: () => void) => <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) go(); }}>{label}</button>;
  return <Shell wide title="Profiles" lead="Saved snapshots of routing, defaults and ceiling, MCP usage policy and session limits." head={btn("Save as new…", saveNew)}>
    <Show when={state() === "unavailable"}><div class="note-box bad"><I n="alert" s={14} /><span>{c().unavailable}</span><span class="ws-push"></span><button class="btn btn-line btn-sm" onClick={() => ws.demo?.reloadProfiles(m().id)}>Reload</button></div></Show>
    <Show when={state() === "interrupted"}><div class="note-box"><I n="alert" s={14} /><span>A switch to {active()?.name} was interrupted, so the files may be a mix.</span><span class="ws-push"></span><span class="row ws-gap4">{btn("Activate again", () => run({ type: "activate_profile", machineId: m().id, profileId: c().active!, keep: "discard" }))}{btn("Save current settings to it", () => run({ type: "save_profile", machineId: m().id }))}{btn("Detach", () => run({ type: "deactivate_profile", machineId: m().id }))}</span></div></Show>
    <Show when={state() === "modified"}><div class="surface"><Srow t={`Modified from ${active()?.name}`} d={diffs().map(d => `${PROFILE_SECTION_LABELS[d.section]}: ${d.lines.join(", ")}`).join(". ")}>{btn("Save to profile", () => run({ type: "save_profile", machineId: m().id }))}{btn("Revert to saved", () => run({ type: "revert_profile", machineId: m().id }))}</Srow></div></Show>
    <Show when={state() === "none"}><div class="note-box plain"><I n="pencil" s={14} /><span>No profile is active. Save as new to keep these settings.</span></div></Show>
    <div class="set-h">Saved<Hint text={HELP.profilesIntro} label="profiles" /></div>
    <div class="surface"><div class="gt">
      <div class="gr ofrow th"><span>Name</span><span>Description</span><span>Stored sections</span><span>Updated</span><span>Matches</span><span></span></div>
      <For each={c().profiles}>{p => {
        const isActive = () => p.id === c().active, matches = () => matchesProfile(c(), p, sessions()), missing = () => missingSections(p);
        return <div class="gr ofrow">
          <span class="trunc"><NameEdit class="w500 mono" label="Profile name" hint="Lowercase letters, digits and dashes." value={p.name} disabled={off()} onSave={name => { void run({ type: "rename_profile", machineId: m().id, profileId: p.id, name }); }} /><Show when={isActive()}><span class="sub st-ok">Active</span></Show></span>
          <span class="cap muted trunc">{p.description}</span>
          <span class="cap muted ws-wrap">{stored(p)}<Show when={missing().length > 0}><span class="sub st-warn">Stores only {stored(p)}. Save to store every section.</span></Show></span>
          <span class="cap muted tnum">{dayOf(p.updatedAt)}</span>
          <span class="cap">{matches() ? "Matches" : ""}</span>
          <span class="row ws-gap4">
            <Show when={!isActive()}>{btn("Activate…", () => switchTo(p.id))}</Show>
            <button class="ib ws-ib" popovertarget={off() ? undefined : `m-prof-${p.id}`} aria-label={`More for ${p.name}`} aria-disabled={off() ? "true" : undefined} data-why={off()}><I n="more" s={14} /></button>
            <Pop id={`m-prof-${p.id}`} class="pop-right" align="right" label={p.name}>
              <MenuItem icon="pencil" label="Edit description…" onSelect={() => ws.open({ kind: "prompt", title: `Describe ${p.name}`, label: "Description", value: p.description, optional: true, action: "Save", go: description => run({ type: "describe_profile", machineId: m().id, profileId: p.id, description }) })} />
              <Show when={isActive()}><MenuItem icon="x" label="Detach" sub="Keeps the settings, with no active profile" needs="deactivate_profile" onSelect={() => run({ type: "deactivate_profile", machineId: m().id })} /></Show>
              <MenuItem icon="trash" label="Delete…" danger disabled={isActive() ? "Activate another profile first" : undefined} onSelect={() => ws.open({ kind: "confirm", title: `Delete the profile ${p.name}?`, note: "Only this snapshot is removed.", action: "Delete", danger: true, go: () => run({ type: "delete_profile", machineId: m().id, profileId: p.id }) })} />
            </Pop>
          </span>
        </div>;
      }}</For>
    </div></div>
    <div class="note-box plain"><I n="lock" s={14} /><span>Profiles never hold project settings, MCP commands, arguments, environment or credentials.</span></div>
    <Show when={routingSource(m(), null).kind === "environment"}><div class="note-box"><I n="alert" s={14} /><span>Routing from a profile is shadowed by $CORTEX_ROUTING here.</span></div></Show>
  </Shell>;
}

export function Files() {
  const ws = useWs(), off = useReadOnly(), name = useProjectName();
  const m = () => ws.machine(), c = () => m().config, f = () => c().files;
  const [tick, setTick] = createSignal(false);
  const [preview, setPreview] = createSignal(false);
  const run = (command: Parameters<typeof ws.run>[0]) => ws.run(command).then(r => r.ok);
  const reset = () => ws.open({ kind: "confirm", title: "Reset to the default preset?", danger: true, action: "Reset", note: "Replaces all routing rules, the coordinator default, the language and the owner ceiling, in routing.json and defaults.json. A private backup of each is kept beside it. Project and $CORTEX_ROUTING files are not changed." + (ws.store.mode === "demo" ? " Simulated: nothing is written." : ""), go: () => run({ type: "reset_preset", machineId: m().id, confirmed: true }) });
  const own = () => projectsWithRouting(m()).map(id => name(id));
  const active = () => routingSource(m(), null).kind;
  const sources = () => [
    { t: "Project routing", path: "<project>/.cortex/routing.json", exists: own().length > 0, sub: own().join(", "), on: false, rev: null as number | null },
    { t: "Environment", path: f().envRouting ?? "$CORTEX_ROUTING", exists: !!f().envRouting, sub: "Read-only", on: active() === "environment", rev: null },
    { t: "Machine routing", path: "~/.config/cortex/routing.json", exists: !f().missing.includes("routing.json"), sub: "", on: active() === "machine", rev: c().revisions.routing },
  ];
  const others = () => [
    { t: "defaults.json", d: "Coordinator default, language and ceiling.", rev: c().revisions.user_defaults }, { t: "mcp-servers.json", d: "External servers. Per machine only.", rev: c().revisions.mcp_servers },
    { t: "connections.json", d: "Registered engine accounts.", rev: null }, { t: "routing-profiles.json", d: "Saved profiles.", rev: null },
  ];
  const envs = () => [["CORTEX_ENGINE_UPDATE", f().env.engineUpdate ?? "on"], ["CORTEX_ENGINE_FOLLOW", f().env.engineFollow ?? "on"], ["CORTEX_OPENCODE_BINARY", f().env.opencodeBinary]].filter(([, v]) => v) as [string, string][];
  const preserved = () => [...f().preservedRouting, ...f().preservedDefaults, ...c().surfaces.user_defaults.preserved];
  return <Shell wide title="Files and presets" lead={ws.t("filesLead")}>
    <div class="set-h">Routing sources<span class="cap faint ws-w400">First existing file wins. Files never merge.</span></div>
    <div class="surface"><div class="gt">
      <div class="gr oyrow th"><span>Source</span><span>Path</span><span>Exists</span><span>Valid</span><span>Revision</span><span></span></div>
      <For each={sources()}>{s => <div class="gr oyrow"><span class="trunc"><span class="w500">{s.t}</span><Show when={s.sub}><span class="sub trunc">{s.sub}</span></Show></span><span class="cap mono trunc">{s.path}</span><span class="cap">{s.exists ? "Yes" : "No"}</span><span class="cap">{s.exists ? (s.on && f().invalidRouting ? <span class="st-bad">Invalid</span> : "Yes") : ""}</span><span class="cap tnum muted">{s.rev === null ? "" : `rev ${s.rev}`}</span><span class="cap st-ok">{s.on ? "In use" : ""}</span></div>}</For>
    </div></div>
    <div class="set-h">Other files</div>
    <div class="surface"><For each={others()}>{o => <Srow t={<span class="mono">{o.t}</span>} d={o.d}><Show when={o.rev !== null}><span class="cap tnum muted">rev {o.rev}</span></Show></Srow>}</For>
      <Srow t={<span class="mono">ssh-askpass.json</span>} d="Written by hand, off by default."><span class="cap">{f().askpass ? "On" : "Off"}</span><Lock text="Read-only" /></Srow></div>
    <Show when={preserved().length > 0}><div class="set-h">Preserved keys</div><div class="surface"><Srow t={<span class="mono">{preserved().join(", ")}</span>} d="Keys this app does not manage. They are kept on save."><Lock text="Read-only" /></Srow></div></Show>
    <Show when={envs().length > 0}><div class="set-h">Environment overrides</div><div class="surface"><For each={envs()}>{([k, v]) => <Srow t={<span class="mono">{k}</span>}><span class="cap mono">{v}</span><Lock text="Read-only" /></Srow>}</For></div></Show>
    <div class="set-h">Default preset<Hint text={HELP.defaultPreset} label="default preset" /></div>
    <div class="surface">
      <Srow t="Apply" d={f().missing.length ? `Seeds what is absent: ${f().missing.join(", ")}.` : "Every file exists, so there is nothing to seed."}><button class="btn btn-line btn-sm" aria-disabled={f().missing.length && !off() ? undefined : "true"} data-why={off()} onClick={() => { if (f().missing.length && !off()) run({ type: "apply_preset", machineId: m().id }); }}>Apply preset</button></Srow>
      <Srow t="Reset" d={f().backups ? `${f().backups} private backups kept.` : "Replaces routing.json and defaults.json, with private backups."}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) reset(); }}>Reset to default…</button></Srow>
    </div>
    <div class="set-h">Model policy migration</div>
    <div class="surface">
      <Srow t="Older model policy" d={f().migration || c().surfaces.routing.legacyTiers.length ? "This machine needs it. Preview first." : `This machine is on model policy ${c().surfaces.routing.version}.`}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) setPreview(!preview()); }}>Preview</button></Srow>
      <Show when={preview()}>
        <Srow t="Preview" d={`${f().migration ? "1 saved policy moves" : "No saved policy moves"} to the current format, and no route changes. ${c().surfaces.routing.legacyTiers.length ? `Legacy tiers go: ${c().surfaces.routing.legacyTiers.join(", ")}.` : ""}`}>
          <label class="ws-check"><Chk label="I reviewed the migration" checked={tick()} onChange={setTick} /><span>I reviewed it</span></label>
          <button class="btn btn-primary btn-sm" aria-disabled={tick() ? undefined : "true"} data-why={tick() ? undefined : "Tick the box first"} onClick={() => { if (tick()) ws.ok({ type: "migrate_policy", machineId: m().id, confirmed: true }, () => { setPreview(false); setTick(false); }); }}>Apply</button>
        </Srow>
      </Show>
    </div>
  </Shell>;
}
