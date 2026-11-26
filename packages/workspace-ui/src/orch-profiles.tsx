import { createMemo, createSignal, For, Show } from "solid-js";
import { matchesProfile, missingSections, PROFILE_SECTION_LABELS, PROFILE_SECTIONS, projectsWithRouting, routingSource, sessionsOf, type Profile } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { NameEdit } from "./field-edit";
import { A, I, Keyed, MenuItem, Pop } from "./kit";
import { HELP, profilesLead } from "./orch-help";
import { orchTo, Shell, useProjectName, useReadOnly, useSwitchProfile } from "./orch-shared";
import { clock } from "./selectors";
import { Chk, Hint, Lock, SetH, Srow } from "./settings-kit";
import { share } from "./stable";

const dayOf = (at: number | null) => at === null ? "" : clock(at);
export function Profiles() {
  const ws = useWs(), off = useReadOnly(), switchTo = useSwitchProfile();
  const m = () => ws.machine(), c = () => m().config, sessions = () => sessionsOf(m());
  const run = (command: Parameters<typeof ws.run>[0]) => ws.run(command).then(r => r.ok);
  const stored = (p: Profile) => PROFILE_SECTIONS.filter(s => p.sections[s] !== undefined).map(s => PROFILE_SECTION_LABELS[s]).join(", ");
  const btn = (label: string, go: () => void) => <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) go(); }}>{label}</button>;
  return <Shell section="profiles" wide title="Profiles" lead={profilesLead()}>
    <SetH title="Saved profiles" count={c().profiles.length || undefined}><Hint text={HELP.profilesIntro} label="profiles" /></SetH>
    <Show when={c().profiles.length > 0} fallback={<div class="surface ws-empty">No saved profiles. Current settings still apply; use Save as new in the bar above to keep them.</div>}>
    <div class="surface"><div class="gt">
      <div class="gr ofrow th"><span>Name</span><span>Description</span><span>Stored sections</span><span>Updated</span><span>Matches</span><span></span></div>
      <Keyed each={c().profiles} by={p => p.id}>{p => {
        const isActive = () => p().id === c().active, matches = () => matchesProfile(c(), p(), sessions()), missing = () => missingSections(p());
        return <div class="gr ofrow">
          <span class="trunc"><NameEdit class="w500 mono" label="Profile name" hint="Lowercase letters, digits and dashes." value={p().name} disabled={off()} onSave={name => ws.run({ type: "rename_profile", machineId: m().id, profileId: p().id, name })} /><Show when={isActive()}><span class="sub st-ok">Active</span></Show></span>
          <span class="cap muted trunc">{p().description}</span>
          <span class="cap muted ws-wrap">{stored(p())}<Show when={missing().length > 0}><span class="sub st-warn">Stores only {stored(p())}. Save to store every section.</span></Show></span>
          <span class="cap muted tnum">{dayOf(p().updatedAt)}</span>
          <span class="cap">{matches() ? "Matches" : ""}</span>
          <span class="row ws-gap4">
            <Show when={!isActive()}>{btn("Activate…", () => switchTo(p().id))}</Show>
            <button data-tooltip="" class="ib ws-ib" popovertarget={off() ? undefined : `m-prof-${p().id}`} aria-label={`More for ${p().name}`} aria-disabled={off() ? "true" : undefined} data-why={off()}><I n="more" s={14} /></button>
            <Pop id={`m-prof-${p().id}`} class="pop-right" align="right" label={p().name}>
              <MenuItem icon="pencil" label="Edit description…" onSelect={() => ws.open({ kind: "prompt", title: `Describe ${p().name}`, label: "Description", value: p().description, optional: true, action: "Save", go: description => run({ type: "describe_profile", machineId: m().id, profileId: p().id, description }) })} />
              <Show when={isActive()}><MenuItem icon="x" label="Detach" sub="Keeps the settings, with no active profile" needs="deactivate_profile" onSelect={() => run({ type: "deactivate_profile", machineId: m().id })} /></Show>
              <MenuItem icon="trash" label="Delete…" danger disabled={isActive() ? "Activate another profile first" : undefined} onSelect={() => ws.open({ kind: "confirm", title: `Delete the profile ${p().name}?`, note: "Only this snapshot is removed.", action: "Delete", danger: true, go: () => run({ type: "delete_profile", machineId: m().id, profileId: p().id }) })} />
            </Pop>
          </span>
        </div>;
      }}</Keyed>
    </div></div>
    </Show>
    <div class="note-box plain"><I n="lock" s={14} /><span>Profiles never hold project settings, MCP commands, arguments, environment or credentials.</span></div>
    <Show when={routingSource(m(), null).kind === "environment"}><div class="note-box"><I n="alert" s={14} /><span>An environment override takes priority over this profile's routing here.</span></div></Show>
  </Shell>;
}

export function Files() {
  const ws = useWs(), off = useReadOnly(), name = useProjectName();
  const m = () => ws.machine(), c = () => m().config, f = () => c().files;
  const [tick, setTick] = createSignal(false);
  const [preview, setPreview] = createSignal(false);
  const run = (command: Parameters<typeof ws.run>[0]) => ws.run(command).then(r => r.ok);
  const own = () => projectsWithRouting(m()).map(id => name(id));
  const active = () => routingSource(m(), null).kind;
  type Source = { t: string; path: string | null; label: string; exists: boolean; sub: string; on: boolean; rev: number | null };
  const sources = createMemo<Source[]>(prev => share(prev, [
    // Friendly names on screen; the exact location stays behind an explicit Copy action, never in a label.
    { t: "Project routing file", path: "<project>/.cortex/routing.json", label: "In each project", exists: own().length > 0, sub: own().join(", "), on: false, rev: null as number | null },
    { t: "Environment routing file", path: f().envRouting ?? null, label: f().envRouting ? "Set by the environment" : "No environment override", exists: !!f().envRouting, sub: "Read-only", on: active() === "environment", rev: null },
    { t: "User routing file", path: "~/.config/cortex/routing.json", label: "In your user settings", exists: !f().missing.includes("routing.json"), sub: "", on: active() === "machine", rev: c().revisions.routing },
  ]), []);
  const others = createMemo<{ t: string; d: string; rev: number | null }[]>(prev => share(prev, [
    { t: "defaults.json", d: "Coordinator default, language and ceiling.", rev: c().revisions.user_defaults }, { t: "mcp-servers.json", d: "External servers for the whole machine. A project's own servers and choices are kept apart, in a private file per project.", rev: c().revisions.mcp_servers },
    { t: "connections.json", d: "Registered engine accounts.", rev: null }, { t: "routing-profiles.json", d: "Saved profiles.", rev: null },
  ]), []);
  const envs = createMemo<[string, string, string][]>(prev => share(prev, [["Engine updates", "CORTEX_ENGINE_UPDATE", f().env.engineUpdate ?? "on"], ["Engine version following", "CORTEX_ENGINE_FOLLOW", f().env.engineFollow ?? "on"], ["OpenCode executable", "CORTEX_OPENCODE_BINARY", f().env.opencodeBinary]].filter(([, , v]) => v) as [string, string, string][]), []);
  const copy = (value: string) => { void navigator.clipboard?.writeText(value).catch(() => undefined); };
  const preserved = () => [...f().preservedRouting, ...f().preservedDefaults, ...c().surfaces.user_defaults.preserved];
  return <Shell section="routing" wide back={{ to: orchTo(m().id, "routing"), label: "Routing" }} title="Files and sources" lead={ws.t("filesLead")}>
    <SetH title="Routing sources"><span class="cap faint ws-w400">First existing file wins. Files never merge.</span></SetH>
    <div class="surface"><div class="gt">
      <div class="gr oyrow th"><span>Source</span><span>Path</span><span>Exists</span><span>Valid</span><span>Revision</span><span></span></div>
      <For each={sources()}>{s => <div class="gr oyrow"><span class="trunc"><span class="w500">{s.t}</span><Show when={s.sub}><span class="sub trunc">{s.sub}</span></Show></span><span class="cap trunc">{s.label}<Show when={s.path}>{p => <button type="button" class="lnk ws-ml4" aria-label={`Copy file location: ${s.t}`} onClick={() => copy(p())}>Copy file location</button>}</Show></span><span class="cap">{s.exists ? "Yes" : "No"}</span><span class="cap">{s.exists ? (s.on && f().invalidRouting ? <span class="st-bad">Invalid</span> : "Yes") : ""}</span><span class="cap tnum muted">{s.rev === null ? "" : `rev ${s.rev}`}</span><span class="cap st-ok">{s.on ? "In use" : ""}</span></div>}</For>
    </div></div>
    <SetH title="Other files" />
    <div class="surface"><For each={others()}>{o => <Srow t={<span class="mono">{o.t}</span>} d={o.d}><Show when={o.rev !== null}><span class="cap tnum muted">rev {o.rev}</span></Show></Srow>}</For>
      <Srow t={<span class="mono">ssh-askpass.json</span>} d="Written by hand, off by default."><span class="cap">{f().askpass ? "On" : "Off"}</span><Lock text="Read-only" /></Srow></div>
    <Show when={preserved().length > 0}><SetH title="Preserved keys" /><div class="surface"><Srow t={<span class="mono">{preserved().join(", ")}</span>} d="Keys this app does not manage. They are kept on save."><Lock text="Read-only" /></Srow></div></Show>
    <Show when={envs().length > 0}><SetH title="Environment overrides" /><div class="surface"><For each={envs()}>{([label, name, v]) => <Srow t={label}><span class="cap mono">{v}</span><button type="button" class="lnk" aria-label={`Copy variable name: ${label}`} onClick={() => copy(name)}>Copy variable name</button><Lock text="Read-only" /></Srow>}</For></div></Show>
    <div class="note-box plain"><I n="pencil" s={14} /><span>The default preset is applied or reset under <A class="lnk" to={orchTo(m().id, "defaults")}>Defaults</A>.</span></div>
    <SetH title="Model policy migration" />
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
