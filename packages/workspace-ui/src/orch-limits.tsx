import { createMemo, For, Show } from "solid-js";
import { allRoutes, ceilingProblems, ceilingState, effectiveCeiling, ENGINES, FAILURE_CLASSES, HARD_DENIALS, SURFACE_LABELS, type Ceiling, type CeilingFieldState, type UserDefaults } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { I, Keyed } from "./kit";
import { share } from "./stable";
import { HELP } from "./orch-help";
import { Bar, Shell, useProjectName, useReadOnly } from "./orch-shared";
import { Adv, Chk, Err, Hint, Lock, NumField, Rec, SetH, Srow, Sw } from "./settings-kit";
import { useDraft, type Draft } from "./settings-state";

const recipients = () => [...new Set(ENGINES.map(e => e.recipient))].map(id => ({ id, label: id }));
/** Off means no restriction. A list means only these, and an empty list means nothing at all. */
function ListLimit(props: { label: string; value: string[] | null; options: { id: string; label: string }[]; off?: string; onChange: (v: string[] | null) => void }) {
  const flip = (id: string, on: boolean) => { const now = props.value ?? []; props.onChange(on ? [...now, id] : now.filter(x => x !== id)); };
  return <div class="ws-limit">
    <label class="sw-l">Restrict<Sw label={`Restrict ${props.label}`} checked={props.value !== null} disabled={props.off} onChange={on => props.onChange(on ? [] : null)} /></label>
    <Show when={props.value !== null}>
      <div class="ws-checks"><Keyed each={props.options} by={o => o.id}>{o => <label class="ws-check"><Chk label={`${props.label}: ${o().label}`} checked={!!props.value?.includes(o().id)} disabled={props.off} onChange={on => flip(o().id, on)} /><span class="trunc">{o().label}</span></label>}</Keyed></div>
      <Show when={props.value?.length === 0}><span class="cap st-warn">Nothing would be allowed.</span></Show>
    </Show>
  </div>;
}
function StoredLimits(props: { c: Ceiling }) {
  const s = () => props.c.stored;
  // Read again at every machine read: equal rows stay the old ones, so their lines are not drawn again.
  const rows = createMemo<[string, string | number][]>(prev => share(prev, [["Permission level", s().permissionLevel], ["Calls", s().calls], ["Tokens", s().tokens], ["Concurrency", s().concurrency], ["Spend limit", s().spendLimit], ["Spend limit enforceable", s().spendLimit === null ? null : s().spendEnforceable ? "Yes" : "No"]].filter(([, v]) => v !== null) as [string, string | number][]), []);
  return <Show when={rows().length > 0}>
    <SetH title="Stored, not applied" />
    <div class="surface"><For each={rows()}>{([t, v]) => <Srow t={t} d="Saved in the configuration, but not enforced."><span class="tnum">{v}</span><Lock text="Read-only" /></Srow>}</For></div>
  </Show>;
}

function useLimitOptions() {
  const ws = useWs();
  return {
    connections: () => ws.machine().connections.map(c => ({ id: c.id, label: c.label })),
    routes: () => allRoutes(ws.machine().config.surfaces.routing, ws.machine().config.catalogue),
  };
}
/** Defaults › Machine ceiling: the widest limits on this machine, in the same draft as the rest of Defaults. */
export function MachineCeiling(props: { d: Draft<UserDefaults> }) {
  const off = useReadOnly(), o = useLimitOptions();
  const d = props.d;
  const c = () => d.value().ceiling, set = (x: Partial<Ceiling>) => d.set({ ...d.value(), ceiling: { ...c(), ...x } });
  return <section class="set-grp" id="set-machine-ceiling" aria-labelledby="set-machine-ceiling-h">
    <SetH title={<span id="set-machine-ceiling-h">Machine ceiling</span>}><Hint text={HELP.ownerCeiling} label="machine ceiling" /></SetH>
    <p class="set-grp-lead">The widest limits any project or task may use. A project can only narrow them.</p>
    <Rec>Leave every restriction off unless you need a hard lock; switch models on and off under Models instead.</Rec>
    <div class="surface">
      <Srow t={<>Attempts per task<Hint text={HELP.limits} label="attempts per task" /></>} d="Most model runs one task may start. Blank means no limit." r="Blank, or about 8 to 12 to stop runaway retries."><NumField label="Attempts per task" value={c().attempts} placeholder="No limit" disabled={off()} onChange={attempts => set({ attempts })} /><Err problems={d.problems()} path="attempts" /></Srow>
      <Srow t={<>Connections<Hint text={HELP.allowedConnections} label="connections" /></>} d="Which engine accounts tasks may use."><ListLimit label="Connections" value={c().allowedConnections} options={o.connections()} off={off()} onChange={allowedConnections => set({ allowedConnections })} /></Srow>
      <Srow t={<>Data recipients<Hint text={HELP.allowedRecipients} label="data recipients" /></>} d="Which providers may receive project data."><ListLimit label="Data recipients" value={c().allowedRecipients} options={recipients()} off={off()} onChange={allowedRecipients => set({ allowedRecipients })} /></Srow>
    </div>
    <SetH title="Fallback"><Hint text={HELP.fallback} label="fallback" /></SetH>
    <p class="set-grp-lead">Off unless all of these allow it.</p>
    <Rec>Leave fallback off. If you want it: both failure classes, budget 1, consent only if another provider may see the code.</Rec>
    <div class="surface">
      <Srow t="Failure classes" d="What may trigger a move to another route."><ListLimit label="Failure classes" value={c().failureClasses} options={FAILURE_CLASSES} off={off()} onChange={failureClasses => set({ failureClasses })} /></Srow>
      <Srow t="Budget" d="How many moves one task may make. Blank means no limit."><NumField label="Fallback budget" value={c().budget} placeholder="No limit" disabled={off()} onChange={budget => set({ budget })} /><Err problems={d.problems()} path="fallback.budget" /></Srow>
      <Srow t="Allow work to move to another provider" d="Required for any fallback. Cross-provider fallback also needs a consent record per recipient."><Sw label="Allow work to move to another provider" checked={c().consent} disabled={off()} onChange={consent => set({ consent })} /></Srow>
    </div>
    <div class="note-box plain"><I n="lock" s={14} /><span>Never a reason to fall back: {HARD_DENIALS.join(", ")}.</span></div>
    <Adv id="limits" force={c().allowedRoutes !== null || c().targetRoutes !== null}>
      <div class="surface">
        <Srow t={<>Allowed routes<Hint text={HELP.allowedRoutes} label="allowed routes" /></>} d="Exact routes tasks may use."><ListLimit label="Allowed routes" value={c().allowedRoutes} options={o.routes()} off={off()} onChange={allowedRoutes => set({ allowedRoutes })} /></Srow>
        <Srow t="Fallback target routes" d="Exact routes fallback may move to."><ListLimit label="Fallback target routes" value={c().targetRoutes} options={o.routes()} off={off()} onChange={targetRoutes => set({ targetRoutes })} /></Srow>
      </div>
    </Adv>
    <StoredLimits c={c()} />
  </section>;
}

const STATE_NOTE: Record<CeilingFieldState, { text: string; tone: string } | null> = {
  not_set: { text: "Starts from the machine ceiling", tone: "muted" }, narrows: null, no_longer_narrows: { text: "No longer narrows", tone: "st-warn" }, violation: { text: "Must stay within the machine limit.", tone: "st-bad" },
};
const listText = (v: string[] | null, of?: { id: string; label: string }[]) => v === null ? "Any" : v.length ? v.map(x => of?.find(o => o.id === x)?.label ?? x).join(", ") : "Nothing";
const numText = (n: number | null) => n === null ? "No limit" : String(n);
const LEAD = "Narrows the machine ceiling for one project. It is stored in that project and never in a profile.";
const RECOMMEND = "Only for projects that must stay on specific models or providers.";
function ProjectLimits(props: { projectId: string }) {
  const ws = useWs(), name = useProjectName(), off = useReadOnly(), o = useLimitOptions();
  const m = () => ws.machine(), machine = () => m().config.surfaces.user_defaults.ceiling, own = () => m().config.projects[props.projectId]?.ceiling;
  const seed = (): Ceiling => ({ ...machine(), attempts: null, budget: null, allowedConnections: null, allowedRecipients: null, allowedRoutes: null, targetRoutes: null, failureClasses: null });
  const d = useDraft<Ceiling>({
    key: () => `${m().id}:${props.projectId}:project_ceiling`, saved: () => own() ?? seed(), name: () => SURFACE_LABELS.project_ceiling, where: () => name(props.projectId) ?? "",
    revision: () => m().config.projects[props.projectId]?.revisions?.project_ceiling ?? 0, commit: (value, revision) => ws.run({ type: "save_surface", machineId: m().id, projectId: props.projectId, surface: "project_ceiling", value, revision: revision ?? m().config.revisions.project_ceiling }),
    problems: v => ceilingProblems(v, machine()),
  });
  const c = () => d.value(), set = (x: Partial<Ceiling>) => d.set({ ...c(), ...x });
  const state = createMemo(() => ceilingState(machine(), c()));
  const eff = createMemo(() => effectiveCeiling({ ...m(), config: { ...m().config, projects: { ...m().config.projects, [props.projectId]: { ...m().config.projects[props.projectId], ceiling: c() } } } }, props.projectId));
  const Note = (p: { s: CeilingFieldState }) => <Show when={STATE_NOTE[p.s]}>{n => <span class={`cap ${n().tone}`} data-problem={p.s === "violation" ? "" : undefined}>{n().text}</span>}</Show>;
  const remove = () => ws.open({ kind: "confirm", title: "Remove the project ceiling?", danger: true, action: "Remove", note: "This project goes back to the machine's limits.", go() { d.discard(); return ws.run({ type: "save_surface", machineId: m().id, projectId: props.projectId, surface: "project_ceiling", value: null, revision: m().config.projects[props.projectId]?.revisions?.project_ceiling ?? 0 }).then(r => r.ok); } });
  // Components, not calls: a call redrew each row, and the field being typed in, at every machine read.
  const List = (p: { t: string; k: "allowedConnections" | "allowedRecipients" | "allowedRoutes" | "targetRoutes" | "failureClasses"; s: CeilingFieldState; options: { id: string; label: string }[] }) => <div class="gr olrow">
    <span class="w500">{p.t}</span><span class="cap muted trunc">{listText(machine()[p.k], p.options)}</span>
    <span><ListLimit label={p.t} value={c()[p.k]} options={p.options} off={off()} onChange={value => set({ [p.k]: value })} /><Note s={p.s} /></span>
    <span class="cap trunc">{listText(eff()[p.k], p.options)}</span>
  </div>;
  const Num = (p: { t: string; k: "attempts" | "budget"; s: CeilingFieldState }) => <div class="gr olrow">
    <span class="w500">{p.t}</span><span class="cap muted tnum">{numText(machine()[p.k])}</span>
    <span><NumField label={`Project ${p.t}`} value={c()[p.k]} placeholder="Not set" disabled={off()} onChange={value => set({ [p.k]: value })} /><Note s={p.s} /></span>
    <span class="cap tnum">{numText(eff()[p.k])}</span>
  </div>;
  return <Shell section="ceiling" title="Project ceiling" lead={LEAD} bar={<Bar d={d} />}
    head={<Show when={own()}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("save_surface")} onClick={() => { if (!off()) remove(); }}>Remove project ceiling…</button></Show>}>
    <Rec>{RECOMMEND}</Rec>
    <Show when={!own()}><div class="note-box plain"><I n="pencil" s={14} /><span>This project has no ceiling of its own. Saving creates one.</span></div></Show>
    <SetH title={<>Limits for {name(props.projectId)}<Hint text={HELP.projectCeiling} label="project ceiling" /></>} />
    <div class="surface"><div class="gt">
      <div class="gr olrow th"><span>Limit</span><span>Machine</span><span>Project</span><span>Effective</span></div>
      <Num t="Attempts per task" k="attempts" s={state().attempts} />
      <List t="Connections" k="allowedConnections" s={state().connections} options={o.connections()} />
      <List t="Data recipients" k="allowedRecipients" s={state().recipients} options={recipients()} />
      <List t="Failure classes" k="failureClasses" s={state().classes} options={FAILURE_CLASSES} />
      <Num t="Fallback budget" k="budget" s={state().budget} />
      <div class="gr olrow"><span class="w500">Allow moving to another provider</span><span class="cap muted">{machine().consent ? "Allowed" : "Not allowed"}</span>
        <span><Sw label="Project: allow work to move to another provider" checked={c().consent} disabled={off() ?? (machine().consent ? undefined : "This machine does not allow it")} onChange={consent => set({ consent })} /></span><span class="cap">{eff().consent ? "Allowed" : "Not allowed"}</span></div>
      <List t="Allowed routes" k="allowedRoutes" s={state().routes} options={o.routes()} />
      <List t="Fallback target routes" k="targetRoutes" s={state().targets} options={o.routes()} />
    </div></div>
  </Shell>;
}
/** Section 4. With no project chosen there is nothing to edit, so it says what to do instead of showing empty fields. */
export function ProjectCeiling() {
  const ws = useWs();
  const none = () => !ws.store.state().projects.some(p => p.machineId === ws.machine().id);
  return <Show when={ws.settings.scope()} keyed fallback={
    <Shell section="ceiling" title="Project ceiling" lead={LEAD}>
      <Rec>{RECOMMEND}</Rec>
      <div class="surface ws-empty" role="status">{none() ? "This machine has no projects." : "Choose a project in the bar above to read or set its ceiling."}</div>
    </Shell>
  }>{id => <ProjectLimits projectId={id} />}</Show>;
}
