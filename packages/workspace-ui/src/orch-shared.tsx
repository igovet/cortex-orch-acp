import { createEffect, createMemo, createUniqueId, For, onMount, Show, type JSX } from "solid-js";
import { defaultsProblems, mcpProblems, missingSections, profileChanges, profileDifferences, profileState, PROFILE_SECTION_LABELS, PROFILE_SECTIONS, projectsWithRouting, routingFor, routingProblems, routingSource, sessionsOf, SURFACE_LABELS, type McpServer, type RoutingRules, type UserDefaults } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I, MenuItem, Pop } from "./kit";
import { Page, revealCutIn, StatusLane, Toolbar, type StripItem } from "./layout";
import { ORCHESTRATION_LABELS, ORCHESTRATION_SECTIONS, orchestrationSection, SCOPED_SECTIONS, type OrchestrationPage, type OrchestrationSection, type Route } from "./router";
import { clock, projectsOn } from "./selectors";
import { Pick, SaveBar } from "./settings-kit";
import { share } from "./stable";
import { useDraft, type Draft } from "./settings-state";

export const slug = (text: string) => text.replace(/[^a-zA-Z0-9]+/g, "-");
export const orchTo = (machine: string, page?: OrchestrationPage, extra: { server?: string; tab?: string } = {}): Route => ({ name: "settings", section: "orchestration", machine, ...(page ? { page } : {}), ...extra });
/** Where a section lives: Connections keeps the machine's own `connections` route, so old links still land on it. */
export const sectionTo = (machine: string, section: OrchestrationSection): Route => section === "connections" ? { name: "settings", section: "connections", machine } : orchTo(machine, section);
const onPages = (machine: () => string, pages: OrchestrationPage[]) => (to: Route) => to.name === "settings" && to.section === "orchestration" && to.machine === machine() && !!to.page && pages.includes(to.page);

export function useReadOnly() {
  const ws = useWs();
  return () => ws.machine().connection === "connected" ? ws.gate("save_surface") : `${ws.machine().name} is offline, so settings are read-only`;
}
/** Machine routing can also be switched off by the environment, which wins over the file and cannot be written from here. */
export function useRoutingOff() {
  const ws = useWs(), off = useReadOnly();
  return () => off() ?? (!ws.settings.scope() && ws.machine().config.files.envRouting ? "Routing uses an environment override and is read-only here" : undefined);
}
export const useProjectName = () => {
  const ws = useWs();
  return (id: string | null) => id ? ws.store.state().projects.find(p => p.id === id)?.name ?? id : null;
};
function notSaved() {
  const ws = useWs();
  return (where: string) => { const c = ws.machine().config, p = c.profiles.find(x => x.id === c.active); return p ? `Saved on ${where}. Not yet in profile ${p.name}.` : `Saved on ${where}.`; };
}
/** Models, Engines and models, Routing and Route preview are views of one file, so they share this one draft and one save. */
export function useRouting() {
  const ws = useWs(), name = useProjectName(), saved = notSaved();
  const m = () => ws.machine(), scope = () => ws.settings.scope();
  const where = () => name(scope()) ?? m().name;
  return useDraft<RoutingRules>({
    key: () => `${m().id}:${scope() ?? "machine"}:routing`, saved: () => routingFor(m(), scope()), name: () => SURFACE_LABELS.routing, where,
    revision: () => (scope() ? m().config.projects[scope()!]?.revisions?.routing : m().config.revisions.routing) ?? 0, onConflict: () => ws.demo?.touchConfig(m().id, "routing"),
    commit: (value, revision) => ws.run({ type: "save_surface", machineId: m().id, projectId: scope(), surface: "routing", value, revision: revision ?? m().config.revisions.routing }),
    problems: v => routingProblems(v, m().config.catalogue), stays: onPages(() => m().id, ["coordinator", "models", "engines", "routing", "effective"]),
    saveNote() {
      const hidden = projectsWithRouting(m()).map(id => name(id));
      if (scope()) return `Saved on ${where()}.`;
      return hidden.length ? `Saved, but the project file still wins in ${hidden.join(", ")}.` : saved(where());
    },
  });
}
/** Defaults holds the coordinator, language, new-task choices and the machine ceiling: all of them are defaults.json. */
export function useDefaults() {
  const ws = useWs(), saved = notSaved();
  const m = () => ws.machine();
  return useDraft<UserDefaults>({
    key: () => `${m().id}:user_defaults`, saved: () => m().config.surfaces.user_defaults, name: () => SURFACE_LABELS.user_defaults, where: () => m().name,
    revision: () => m().config.revisions.user_defaults, onConflict: () => ws.demo?.touchConfig(m().id, "user_defaults"),
    commit: (value, revision) => ws.run({ type: "save_surface", machineId: m().id, surface: "user_defaults", value, revision: revision ?? m().config.revisions.user_defaults }),
    problems: defaultsProblems, stays: onPages(() => m().id, ["defaults", "policy"]), saveNote: () => saved(m().name),
  });
}
/** What one MCP write holds: the servers, and in a project the machine servers it switches on or off by name. */
export type McpDoc = { servers: McpServer[]; overrides: Record<string, boolean> };
/**
 * Every list and server edit is one write, so what is saved was always valid as a whole. With a project chosen it edits that
 * project's own registry (its own servers plus its on or off choices for machine servers) instead of the machine's. `value`
 * and `set` are the server list in both, so the pages that edit servers need not know which one they edit.
 */
export function useMcp() {
  const ws = useWs(), saved = notSaved(), name = useProjectName();
  const m = () => ws.machine(), scope = () => ws.settings.scope();
  const stored = createMemo<McpDoc>(prev => share(prev, scope()
    ? { servers: m().config.projects[scope()!]?.mcp?.servers ?? [], overrides: m().config.projects[scope()!]?.mcp?.overrides ?? {} }
    : { servers: m().config.surfaces.mcp_servers, overrides: {} }), { servers: [], overrides: {} });
  const d = useDraft<McpDoc>({
    key: () => scope() ? `${m().id}:${scope()}:mcp_servers` : `${m().id}:mcp_servers`, saved: stored, name: () => SURFACE_LABELS.mcp_servers, where: () => name(scope()) ?? m().name,
    revision: () => (scope() ? m().config.projects[scope()!]?.revisions?.mcp_servers : m().config.revisions.mcp_servers) ?? 0, onConflict: () => ws.demo?.touchConfig(m().id, "mcp_servers"),
    commit: (doc, revision) => scope()
      ? ws.run({ type: "save_surface", machineId: m().id, projectId: scope(), surface: "mcp_servers", value: doc, revision: revision ?? m().config.projects[scope()!]?.revisions?.mcp_servers ?? 0 })
      : ws.run({ type: "save_surface", machineId: m().id, surface: "mcp_servers", value: doc.servers, revision: revision ?? m().config.revisions.mcp_servers }),
    problems: doc => doc.servers.flatMap(s => mcpProblems(s, doc.servers.filter(x => x !== s), !stored().servers.some(o => o.name === s.name)).map(p => ({ ...p, path: `${s.name}.${p.path}` }))),
    stays: onPages(() => m().id, ["tools"]), saveNote: () => scope() ? `Saved on ${name(scope()) ?? m().name}.` : saved(m().name),
  });
  const readOnly = useReadOnly();
  /** Why nothing can be edited here: the machine is offline, or the chosen project's file cannot be changed. */
  const off = () => readOnly() ?? (!scope() ? undefined : !m().config.projects[scope()!]?.mcp ? "This project's MCP settings could not be read" : m().config.projects[scope()!]!.mcp!.problem);
  return {
    ...d, raw: d, scope, off,
    value: () => d.value().servers,
    set: (servers: McpServer[]) => d.set({ ...d.value(), servers }),
    overrides: () => d.value().overrides,
    /** `undefined` removes the project's choice, so the machine's own setting applies again. */
    setOverride(server: string, on: boolean | undefined) {
      const next = { ...d.value().overrides };
      if (on === undefined) delete next[server]; else next[server] = on;
      d.set({ ...d.value(), overrides: next });
    },
  };
}
export function Bar<T>(props: { d: Draft<T>; shadow?: boolean }) {
  const ws = useWs();
  const active = () => !!ws.machine().config.active && /^Saved on/.test(props.d.note() ?? "") && /Not yet/.test(props.d.note() ?? "");
  const shadowed = () => /still wins/.test(props.d.note() ?? "");
  return <SaveBar d={props.d} onProfile={active() ? () => ws.run({ type: "save_profile", machineId: ws.machine().id }) : undefined} source={shadowed() ? () => ws.go(orchTo(ws.machine().id, "files")) : undefined} />;
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);
export function useSwitchProfile() {
  const ws = useWs();
  return (profileId: string) => {
    const m = ws.machine(), c = m.config, target = c.profiles.find(p => p.id === profileId);
    if (!target || profileId === c.active) return;
    const changes = profileDifferences(c, sessionsOf(m)).map(x => PROFILE_SECTION_LABELS[x.section]), drafts = ws.settings.draftsOn(m.id);
    if (changes.length || drafts) ws.open({ kind: "switch-profile", profileId, name: target.name, changes, drafts });
    else ws.run({ type: "activate_profile", machineId: m.id, profileId, keep: "discard" });
  };
}

/**
 * Which machine, and on sections 2 to 5 which project. Below 720 px the sidebar is held away, so the bar starts with a choice of
 * the eight sections. It has no room for reasons: `offId` and `scopeId` name the strips under it that say why its controls are off.
 * A narrow bar drops its caption words and the machine's name, never the connection state: at phone width nothing else shows it.
 * Beside the Project picker on a phone the state keeps only its icon on screen. Assistive technology reads the word from its own
 * unseen copy at every width, so hiding the drawn word never changes what is read.
 */
const ContextState = (props: { tone: string; icon: string; word: string }) => <>
  <span class={`st ${props.tone} ws-ctx-state`}><I n={props.icon} s={12} /><span class="ws-ctx-word" aria-hidden="true">{props.word}</span></span>
  <span class="ws-live">{props.word}</span>
</>;
function Context(props: { section: OrchestrationSection; scopeId: string }) {
  const ws = useWs();
  const m = () => ws.machine(), scope = () => ws.settings.scope();
  const projects = createMemo(() => projectsOn(ws.store.index(), m().id));
  const scoped = () => SCOPED_SECTIONS.includes(props.section);
  createEffect(() => { const id = ws.settings.scope(); if (id && !projects().some(p => p.project.id === id)) ws.settings.setScope(null); });
  return <Toolbar small class="set-ctx" label="Machine and project"
    mid={<>
      <span class="l-tight-only"><Pick id="m-ctx-section" label="Section" width="150px" value={props.section} options={ORCHESTRATION_SECTIONS.map(s => ({ id: s, label: ORCHESTRATION_LABELS[s] }))} onPick={id => ws.go(sectionTo(m().id, id as OrchestrationSection))} /></span>
      <b class="trunc l-tight-hide">{m().name}</b>
      <Show when={m().connection === "connected"} fallback={<ContextState tone="st-idle" icon="wifi-off" word="Offline" />}><ContextState tone="st-ok" icon="check-circle" word="Connected" /></Show>
    </>}
    end={<Show when={scoped()}>
      <span class="faint l-compact-hide">Project</span>
      <Pick id="m-scope-project" label="Project" width="150px" value={scope() ?? ""} describedBy={projects().length ? undefined : props.scopeId}
        options={[{ id: "", label: "Machine only" }, ...projects().map(p => ({ id: p.project.id, label: p.project.name }))]} onPick={id => ws.settings.setScope(id || null)} />
    </Show>} />;
}

const STATE_WORD = { in_sync: ["Applied", "check-circle", "st-ok"], none: ["No profile", "minus", "muted"], interrupted: ["Interrupted", "alert", "st-warn"], unavailable: ["Unavailable", "x-circle", "st-bad"] } as const;
/**
 * The saved profile these settings belong to, on every section: which one, whether the files still match it, and what to do
 * about it. A state is a word with an icon, never a colour alone.
 */
function ProfileBar(props: { offId: string }) {
  const ws = useWs(), off = useReadOnly(), switchTo = useSwitchProfile();
  const m = () => ws.machine(), c = () => m().config;
  const state = () => profileState(c(), sessionsOf(m()));
  const active = () => c().profiles.find(p => p.id === c().active);
  // Equal differences stay the old objects, so a machine read leaves the open list and its scroll alone.
  const diffs = createMemo<ReturnType<typeof profileChanges>>(prev => share(prev, profileChanges(m())), []);
  const count = () => diffs().reduce((n, d) => n + d.lines.length, 0);
  const run = (command: Parameters<typeof ws.run>[0]) => ws.run(command).then(r => r.ok);
  const partial = () => { const p = active(); return p && missingSections(p).length ? PROFILE_SECTIONS.filter(s => p.sections[s] !== undefined).map(s => PROFILE_SECTION_LABELS[s]).join(", ") : null; };
  const saveWhy = () => off() ?? (!active() ? "No profile is active" : state() === "in_sync" ? `Nothing differs from ${active()!.name}` : undefined);
  const pick = (id: string) => { if (off()) return; if (id) switchTo(id); else if (c().active) run({ type: "deactivate_profile", machineId: m().id }); };
  const saveNew = () => ws.open({ kind: "prompt", title: "Save as a new profile", label: "Profile name", action: "Save as new", hint: "Lowercase letters, digits and dashes. Add a description afterwards in Profiles.", go: name => run({ type: "create_profile", machineId: m().id, name, description: "" }) });
  const btn = (label: string, why: string | undefined, go: () => void, needs?: string) => <button class="btn btn-line btn-sm" {...(needs ? ws.needs(needs as Parameters<typeof ws.needs>[0]) : {})} {...(why ? { "aria-disabled": "true", "data-why": why } : {})} onClick={e => { if (!why && e.currentTarget.getAttribute("aria-disabled") !== "true") go(); }}>{label}</button>;
  return <div class="set-prof" role="group" aria-label="Profile">
    <div class="set-prof-r">
      <span class="faint l-compact-hide">Profile</span>
      <Pick id="m-ctx-profile" label="Profile" value={c().active ?? ""} empty="No profile" width="140px" disabled={off()} describedBy={off() ? props.offId : undefined}
        options={[{ id: "", label: "No profile", sub: "Keeps the current settings" }, ...c().profiles.map(p => ({ id: p.id, label: p.name, sub: p.description || undefined }))]} onPick={pick} />
      <Show when={state() === "modified"} fallback={<span class={`st ${STATE_WORD[state() as keyof typeof STATE_WORD][2]}`}><I n={STATE_WORD[state() as keyof typeof STATE_WORD][1]} s={12} />{STATE_WORD[state() as keyof typeof STATE_WORD][0]}</span>}>
        <button class="chip st-warn" popovertarget="m-ctx-diff" aria-label={`Modified, ${count()} difference${count() === 1 ? "" : "s"} from ${active()?.name ?? "the profile"}`}><I n="pencil" s={12} />Modified ({count()})</button>
        <Pop id="m-ctx-diff" class="pop-anchor ws-hint-pop" align="left" label="Differences from the profile">
          <For each={diffs()}>{d => <div class="ws-diff"><b>{sentence(PROFILE_SECTION_LABELS[d.section])}</b><For each={d.lines}>{line => <span class="cap">{line}</span>}</For></div>}</For>
          <div class="mi-sep"></div>
          <MenuItem label="Save to profile" disabled={off()} needs="save_profile" onSelect={() => ws.run({ type: "save_profile", machineId: m().id })} />
          <MenuItem label="Revert to saved" disabled={off()} needs="revert_profile" onSelect={() => ws.run({ type: "revert_profile", machineId: m().id })} />
        </Pop>
      </Show>
    </div>
    <div class="set-prof-r set-prof-a">
      {btn("Save to profile", saveWhy(), () => run({ type: "save_profile", machineId: m().id }), "save_profile")}
      <Show when={state() === "modified"}>{btn("Revert to saved", off(), () => run({ type: "revert_profile", machineId: m().id }), "revert_profile")}</Show>
      {btn("Save as new…", off(), saveNew, "create_profile")}
    </div>
    <Show when={state() === "interrupted"}><div class="note-box set-prof-note"><I n="alert" s={14} /><span>A switch to {active()?.name} was interrupted, so the files may be a mix.</span><span class="ws-push"></span><span class="row ws-gap4">{btn("Activate again", off(), () => run({ type: "activate_profile", machineId: m().id, profileId: c().active!, keep: "discard" }))}{btn("Save current settings to it", off(), () => run({ type: "save_profile", machineId: m().id }))}{btn("Detach", off(), () => run({ type: "deactivate_profile", machineId: m().id }))}</span></div></Show>
    <Show when={state() === "unavailable"}><div class="note-box bad set-prof-note"><I n="alert" s={14} /><span>{c().unavailable} <Show when={!ws.demo}>The next read of this machine tries again.</Show></span><Show when={ws.demo}><span class="ws-push"></span><button class="btn btn-line btn-sm" onClick={() => ws.demo?.reloadProfiles(m().id)}>Reload</button></Show></div></Show>
    <Show when={partial()}>{sections => <div class="note-box plain set-prof-note"><I n="alert" s={14} /><span>{active()?.name} stores only {sections()}. Save to profile stores every section.</span></div>}</Show>
  </div>;
}

/** What changes for this source of rules, said once at the top of the page. */
export function SourceStrips() {
  const ws = useWs(), name = useProjectName();
  const m = () => ws.machine(), scope = () => ws.settings.scope();
  const others = () => projectsWithRouting(m()).map(id => name(id));
  const source = () => routingSource(m(), scope());
  return <>
    <Show when={m().config.files.invalidRouting}><div class="note-box bad"><I n="alert" s={14} /><span>The routing file is invalid. Every dispatch is refused until this is repaired. <A class="lnk" to={orchTo(m().id, "files")}>Open Files and sources</A></span></div></Show>
    <Show when={m().config.files.migration}><div class="note-box"><I n="alert" s={14} /><span>The model policy needs migration before work can run. <A class="lnk" to={orchTo(m().id, "files")}>Open Files and sources</A></span></div></Show>
    <Show when={scope() && source().kind !== "project"}><div class="note-box plain"><I n="pencil" s={14} /><span>Saving creates this project's own routing file. It replaces machine rules for this project, so it starts from them.</span></div></Show>
    <Show when={scope() && source().kind === "project"}><div class="note-box plain"><I n="check" s={14} /><span>Editing the project's own routing file. Machine rules do not apply here.</span></div></Show>
    <Show when={!scope() && source().kind === "environment"}><div class="note-box"><I n="lock" s={14} /><span>Routing uses an environment override and is read-only here.</span></div></Show>
    <Show when={!scope() && others().length > 0}><div class="note-box"><I n="alert" s={14} /><span>Project routing wins in {others().join(", ")}, so machine changes do not apply there.</span></div></Show>
  </>;
}

/** A "Back to <section>" link at the top of a secondary page. */
export const Back = (props: { to: Route; label: string }) => <div class="row"><A class="lnk ws-back" to={props.to}><I n="arrow-left" s={12} />Back to {props.label}</A></div>;

/**
 * Every orchestration section and secondary page: the context bar, the profile bar, the strips that apply to the whole page,
 * the page, and its save bar. Which section it is decides the project picker and the Section choice; `back` marks a page
 * that opens from a section.
 */
export const Shell = (props: { section?: OrchestrationSection; wide?: boolean; title: string; lead: JSX.Element; head?: JSX.Element; children: JSX.Element; bar?: JSX.Element; back?: { to: Route; label: string } }) => {
  const readOnly = useReadOnly();
  const ws = useWs();
  const offId = createUniqueId(), scopeId = createUniqueId();
  const section = () => props.section ?? orchestrationSection(ws.router.route(), ws.settings.scope()) ?? "defaults";
  const scoped = () => SCOPED_SECTIONS.includes(section());
  let title: HTMLHeadingElement | undefined;
  // A section chosen in the sidebar puts the keyboard on its title, so reading starts at the top of the new section.
  onMount(() => { if (document.activeElement?.closest?.('nav[aria-label="Settings"]')) title?.focus(); });
  // Each strip is one item for the life of the page, so a change of words redraws only the words.
  const offline: StripItem = { id: "offline", tone: "info", domId: offId, icon: <I n="wifi-off" s={14} class="faint" />, get text() { return `${readOnly()}. Last confirmed ${clock(ws.machine().lastConfirmedAt)}. Nothing is queued.`; } };
  const remote: StripItem = { id: "remote", tone: "info", icon: <I n="cloud" s={14} class="faint" />, get text() { return ws.t("remoteWrites"); } };
  const noProjects: StripItem = { id: "scope", tone: "info", domId: scopeId, icon: <I n="help" s={14} class="faint" />, text: "This machine has no projects, so only machine settings can be chosen." };
  const status = createMemo<StripItem[]>(() => [
    ...(readOnly() ? [offline] : []),
    ...(!readOnly() && ws.machine().kind === "remote" && ws.t("remoteWrites") ? [remote] : []),
    ...(scoped() && projectsOn(ws.store.index(), ws.machine().id).length === 0 ? [noProjects] : []),
  ]);
  return <Page class="l-work set-page" scroll="none" head={<><Context section={section()} scopeId={scopeId} /><ProfileBar offId={offId} /></>} status={<StatusLane items={status()} />} foot={props.bar}>
    <div class="set-main" ref={revealCutIn}><div class={`set-in${props.wide ? " wide" : ""}`}>
      <Show when={props.back}>{b => <Back to={b().to} label={b().label} />}</Show>
      <div class="ws-title-row"><div><h1 class="set-title" tabIndex={-1} ref={title}>{props.title}</h1><p class="set-lead">{props.lead}</p></div>{props.head}</div>
      {props.children}
    </div></div>
  </Page>;
};
