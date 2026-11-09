import { createEffect, createMemo, For, Show, type JSX } from "solid-js";
import { defaultsProblems, mcpProblems, profileChanges, profileDifferences, profileState, PROFILE_SECTION_LABELS, projectsWithRouting, routingFor, routingProblems, routingSource, sessionsOf, SURFACE_LABELS, type McpServer, type RoutingRules, type UserDefaults } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I, MenuItem, Pop } from "./kit";
import type { OrchestrationPage, Route } from "./router";
import { clock, projectsOn } from "./selectors";
import { Pick, SaveBar } from "./settings-kit";
import { useDraft, type Draft } from "./settings-state";

export const slug = (text: string) => text.replace(/[^a-zA-Z0-9]+/g, "-");
export const orchTo = (machine: string, page?: OrchestrationPage, extra: { server?: string; tab?: string } = {}): Route => ({ name: "settings", section: "orchestration", machine, ...(page ? { page } : {}), ...extra });
const onPages = (machine: () => string, pages: OrchestrationPage[]) => (to: Route) => to.name === "settings" && to.section === "orchestration" && to.machine === machine() && !!to.page && pages.includes(to.page);

export function useReadOnly() {
  const ws = useWs();
  return () => ws.machine().connection === "connected" ? ws.gate("save_surface") : `${ws.machine().name} is offline, so settings are read-only`;
}
/** Machine routing can also be switched off by the environment, which wins over the file and cannot be written from here. */
export function useRoutingOff() {
  const ws = useWs(), off = useReadOnly();
  return () => off() ?? (!ws.settings.scope() && ws.machine().config.files.envRouting ? "Routing comes from $CORTEX_ROUTING here, which is read-only" : undefined);
}
export const useProjectName = () => {
  const ws = useWs();
  return (id: string | null) => id ? ws.store.state().projects.find(p => p.id === id)?.name ?? id : null;
};
function notSaved() {
  const ws = useWs();
  return (where: string) => { const c = ws.machine().config, p = c.profiles.find(x => x.id === c.active); return p ? `Saved on ${where}. Not yet in profile ${p.name}.` : `Saved on ${where}.`; };
}
/** Models and Routing are two views of one file, so they share this one draft and one save. */
export function useRouting() {
  const ws = useWs(), name = useProjectName(), saved = notSaved();
  const m = () => ws.machine(), scope = () => ws.settings.scope();
  const where = () => name(scope()) ?? m().name;
  return useDraft<RoutingRules>({
    key: () => `${m().id}:${scope() ?? "machine"}:routing`, saved: () => routingFor(m(), scope()), name: () => SURFACE_LABELS.routing, where,
    revision: () => (scope() ? m().config.projects[scope()!]?.revisions?.routing : m().config.revisions.routing) ?? 0, onConflict: () => ws.demo?.touchConfig(m().id, "routing"),
    commit: (value, revision) => ws.run({ type: "save_surface", machineId: m().id, projectId: scope(), surface: "routing", value, revision: revision ?? m().config.revisions.routing }),
    problems: v => routingProblems(v, m().config.catalogue), stays: onPages(() => m().id, ["models", "routing", "effective"]),
    saveNote() {
      const hidden = projectsWithRouting(m()).map(id => name(id));
      if (scope()) return `Saved on ${where()}.`;
      return hidden.length ? `Saved, but the project file still wins in ${hidden.join(", ")}.` : saved(where());
    },
  });
}
/** Coordinator and Limits both write defaults.json. */
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
/** Every list and server edit is one write, so what is saved was always valid as a whole. */
export function useMcp() {
  const ws = useWs(), saved = notSaved();
  const m = () => ws.machine();
  return useDraft<McpServer[]>({
    key: () => `${m().id}:mcp_servers`, saved: () => m().config.surfaces.mcp_servers, name: () => SURFACE_LABELS.mcp_servers, where: () => m().name,
    revision: () => m().config.revisions.mcp_servers, onConflict: () => ws.demo?.touchConfig(m().id, "mcp_servers"),
    commit: (value, revision) => ws.run({ type: "save_surface", machineId: m().id, surface: "mcp_servers", value, revision: revision ?? m().config.revisions.mcp_servers }),
    problems: list => list.flatMap(s => mcpProblems(s, list.filter(x => x !== s), !m().config.surfaces.mcp_servers.some(o => o.name === s.name)).map(p => ({ ...p, path: `${s.name}.${p.path}` }))),
    stays: onPages(() => m().id, ["tools"]), saveNote: () => saved(m().name),
  });
}
export function Bar<T>(props: { d: Draft<T>; shadow?: boolean }) {
  const ws = useWs();
  const active = () => !!ws.machine().config.active && /^Saved on/.test(props.d.note() ?? "") && /Not yet/.test(props.d.note() ?? "");
  const shadowed = () => /still wins/.test(props.d.note() ?? "");
  return <SaveBar d={props.d} onProfile={active() ? () => ws.run({ type: "save_profile", machineId: ws.machine().id }) : undefined} source={shadowed() ? () => ws.go(orchTo(ws.machine().id, "files")) : undefined} />;
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);
const STATE_TEXT = { in_sync: "Applied", none: "No profile", interrupted: "Interrupted", unavailable: "Unavailable" } as const;
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
/** Which machine, which profile, which scope. It reads the same on every page, so nothing moves between them. */
function Context(props: { scoped?: boolean }) {
  const ws = useWs(), off = useReadOnly(), switchTo = useSwitchProfile();
  const m = () => ws.machine(), scope = () => ws.settings.scope(), c = () => m().config;
  const projects = createMemo(() => projectsOn(ws.store.index(), m().id));
  const state = () => profileState(c(), sessionsOf(m()));
  const diffs = createMemo(() => profileChanges(m()));
  const count = () => diffs().reduce((n, d) => n + d.lines.length, 0);
  createEffect(() => { const id = ws.settings.scope(); if (id && !projects().some(p => p.project.id === id)) ws.settings.setScope(null); });
  return <div class="set-ctx">
    <span><b>{m().name}</b></span>
    <Show when={m().connection === "connected"} fallback={<span class="st st-idle"><I n="wifi-off" s={12} />Offline</span>}><span class="st st-ok"><I n="check-circle" s={12} />Connected</span></Show>
    <span class="r">
      <span class="faint">Profile</span>
      <Pick id="m-ctx-profile" label="Profile" value={c().active ?? ""} width="120px" disabled={off()} options={c().profiles.map(p => ({ id: p.id, label: p.name, sub: p.description || undefined }))} onPick={switchTo} />
      <Show when={state() === "modified"} fallback={<span class={`chip ws-static ${state() === "in_sync" ? "" : "st-warn"}`}>{STATE_TEXT[state() as keyof typeof STATE_TEXT]}</span>}>
        <button class="chip st-warn" popovertarget="m-ctx-diff">Modified ({count()})</button>
        <Pop id="m-ctx-diff" class="pop-right ws-hint-pop" align="right" label="Differences from the profile">
          <For each={diffs()}>{d => <div class="ws-diff"><b>{sentence(PROFILE_SECTION_LABELS[d.section])}</b><For each={d.lines}>{line => <span class="cap">{line}</span>}</For></div>}</For>
          <div class="mi-sep"></div>
          <MenuItem label="Save to profile" disabled={off()} needs="save_profile" onSelect={() => ws.run({ type: "save_profile", machineId: m().id })} />
          <MenuItem label="Revert to saved" disabled={off()} needs="revert_profile" onSelect={() => ws.run({ type: "revert_profile", machineId: m().id })} />
        </Pop>
      </Show>
      <Show when={props.scoped}>
        <span class="faint">Scope</span>
        <div class="seg" role="group" aria-label="Scope">
          <button aria-current={scope() ? "false" : "true"} onClick={() => ws.settings.setScope(null)}>Machine</button>
          <button aria-current={scope() ? "true" : "false"} aria-disabled={projects().length ? undefined : "true"} data-why={projects().length ? undefined : "This machine has no projects"} onClick={() => { if (projects().length) ws.settings.setScope(scope() ?? projects()[0].project.id); }}>Project</button>
        </div>
        <Show when={scope()}><Pick id="m-scope-project" label="Project" width="130px" value={scope()!} options={projects().map(p => ({ id: p.project.id, label: p.project.name }))} onPick={id => ws.settings.setScope(id)} /></Show>
      </Show>
    </span>
  </div>;
}

/** What changes for this source of rules, said once at the top of the page. */
export function SourceStrips() {
  const ws = useWs(), name = useProjectName();
  const m = () => ws.machine(), scope = () => ws.settings.scope();
  const others = () => projectsWithRouting(m()).map(id => name(id));
  const source = () => routingSource(m(), scope());
  return <>
    <Show when={m().config.files.invalidRouting}><div class="note-box bad"><I n="alert" s={14} /><span>The routing file is invalid. Every dispatch is refused until this is repaired. <A class="lnk" to={orchTo(m().id, "files")}>Open Files and presets</A></span></div></Show>
    <Show when={m().config.files.migration}><div class="note-box"><I n="alert" s={14} /><span>The model policy needs migration before work can run. <A class="lnk" to={orchTo(m().id, "files")}>Open Files and presets</A></span></div></Show>
    <Show when={scope() && source().kind !== "project"}><div class="note-box plain"><I n="pencil" s={14} /><span>Saving creates this project's own routing file. It replaces machine rules for this project, so it starts from them.</span></div></Show>
    <Show when={scope() && source().kind === "project"}><div class="note-box plain"><I n="check" s={14} /><span>Editing the project's own routing file. Machine rules do not apply here.</span></div></Show>
    <Show when={!scope() && source().kind === "environment"}><div class="note-box"><I n="lock" s={14} /><span>Routing comes from $CORTEX_ROUTING ({source().path}), which is read-only here.</span></div></Show>
    <Show when={!scope() && others().length > 0}><div class="note-box"><I n="alert" s={14} /><span>Project routing wins in {others().join(", ")}, so machine changes do not apply there.</span></div></Show>
  </>;
}

export const Shell = (props: { scoped?: boolean; wide?: boolean; title: string; lead: string; head?: JSX.Element; children: JSX.Element; bar?: JSX.Element }) => {
  const readOnly = useReadOnly();
  const ws = useWs();
  return <>
    <Context scoped={props.scoped} />
    <Show when={readOnly()}>{r => <div class="strip info"><I n="wifi-off" s={14} class="faint" /><span>{r()}. Last confirmed {clock(ws.machine().lastConfirmedAt)}. Nothing is queued.</span></div>}</Show>
    <Show when={!readOnly() && ws.machine().kind === "remote" && ws.t("remoteWrites")}><div class="strip info"><I n="cloud" s={14} class="faint" /><span>{ws.t("remoteWrites")}</span></div></Show>
    <div class="set-main"><div class={`set-in${props.wide ? " wide" : ""}`}>
      <div class="ws-title-row"><div><h1 class="set-title">{props.title}</h1><p class="set-lead">{props.lead}</p></div>{props.head}</div>
      {props.children}
    </div></div>
    {props.bar}
  </>;
};
