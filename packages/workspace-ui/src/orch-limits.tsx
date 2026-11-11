import { createMemo, For, Show } from "solid-js";
import { CONNECTION_ENGINE, allRoutes, ceilingProblems, ceilingState, coordinatorEfforts, coordinatorResolution, effectiveCeiling, effortLabel, engineLabel, ENGINE_KEYS, ENGINES, FAILURE_CLASSES, HARD_DENIALS, modelLabel, LANGUAGES, ACCESS_MODES, routeIdOf, SURFACE_LABELS, type Ceiling, type CeilingFieldState, type EngineKey } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I } from "./kit";
import { HELP } from "./orch-help";
import { Bar, orchTo, Shell, useDefaults, useProjectName, useReadOnly } from "./orch-shared";
import { Adv, Chk, Err, Hint, KeyValues, Lock, NumField, Pick, Seg, SetH, Srow, Sw } from "./settings-kit";
import { useDraft } from "./settings-state";

const FALLBACK_EFFORTS = "Not observed: this is a general list.";
export function Coordinator() {
  const ws = useWs();
  const d = useDefaults(), off = useReadOnly();
  const v = () => d.value(), m = () => ws.machine(), c = () => m().config, set = (patch: Partial<ReturnType<typeof v>>) => d.set({ ...v(), ...patch });
  const models = createMemo(() => ENGINE_KEYS.flatMap(e => c().surfaces.routing.engines[e].enabled.map(id => ({ id: `${e}|${id}`, label: modelLabel(c().catalogue, e, id), sub: `${engineLabel(e)}, ${id}` }))));
  const coord = () => v().coordinator;
  const efforts = () => coord() ? coordinatorEfforts(c().catalogue, coord()!.engine, coord()!.model) : { levels: [], observed: true };
  const pick = (value: string) => { const [engine, model] = value.split("|") as [EngineKey, string]; const levels = coordinatorEfforts(c().catalogue, engine, model).levels, was = coord()?.effort ?? "medium"; set({ coordinator: { engine, model, effort: levels.includes(was) ? was : levels[0], parameters: coord()?.parameters ?? {} } }); };
  const bound = createMemo(() => coordinatorResolution({ ...m(), config: { ...c(), surfaces: { ...c().surfaces, user_defaults: v() } } }));
  const routes = createMemo(() => ENGINE_KEYS.flatMap(e => c().catalogue[e].models.map(x => ({ id: x.routeId ?? routeIdOf(e, x.id), label: x.label, sub: engineLabel(e), engine: e, model: x.id }))));
  const first = () => { const e = coord()?.engine ?? "claude-agent-acp", id = coord()?.model ?? c().surfaces.routing.engines[e].enabled[0]; return { routeId: routes().find(r => r.engine === e && r.model === id)?.id ?? routeIdOf(e, id), connectionId: m().connections.find(x => CONNECTION_ENGINE[x.engine] === e)?.detail.connectionId ?? "", model: id, parameters: {} }; };
  const route = () => v().routeDefault;
  // A saved route the engines no longer offer still shows, by its model, so it can be seen and replaced.
  const routeChoices = () => routes().some(r => r.id === route()?.routeId) ? routes() : [{ id: route()?.routeId ?? "", label: route()?.model || "No longer offered", sub: "Not offered now" }, ...routes()];
  const connectionChoices = () => { const list = m().connections.map(x => ({ id: x.detail.connectionId, label: x.label })), now = route()?.connectionId ?? ""; return list.some(x => x.id === now) ? list : [{ id: now, label: "No longer registered" }, ...list]; };
  const patchRoute = (x: Partial<NonNullable<ReturnType<typeof route>>>) => set({ routeDefault: { ...route()!, ...x } });
  return <Shell title="Coordinator" lead="What a new task on this machine starts with. You can still change each one in the composer for a single task." bar={<Bar d={d} />}>
    <div class="surface">
      <Srow t={<>Model<Hint text={HELP.coordinator} label="coordinator" /></>} d="Enabled models, grouped by engine."><Pick id="m-def-model" label="Main model" width="260px" value={coord() ? `${coord()!.engine}|${coord()!.model}` : ""} empty="Choose a model" options={models()} disabled={off()} onPick={pick} /></Srow>
      <Show when={coord()}><Srow t="Effort" d={efforts().observed ? "The levels this model reports." : FALLBACK_EFFORTS}><Seg label="Effort" value={coord()!.effort} disabled={off()} options={efforts().levels.map(e => ({ id: e, label: effortLabel(e) }))} onChange={effort => set({ coordinator: { ...coord()!, effort } })} /></Srow></Show>
      <Srow t="Access mode" d="How much a task may do without asking."><Seg label="Access mode" value={v().mode} disabled={off()} options={ACCESS_MODES} onChange={mode => set({ mode })} /></Srow>
      <Srow t={<>Language<Hint text={HELP.language} label="language" /></>} d="Language of messages. It does not guarantee the language of model replies."><Seg label="Language" value={v().language} disabled={off()} options={LANGUAGES.map(l => ({ id: l.id, label: l.id === "auto" ? "Automatic" : l.label as string }))} onChange={language => set({ language })} /></Srow>
      <Srow t="Checkout for new tasks" d="Preselected in New task. You choose each time."><Seg label="Default checkout" value={v().checkout} disabled={off()} options={[{ id: "worktree", label: "Worktree" }, { id: "main", label: "Project folder" }] as const} onChange={checkout => set({ checkout })} /></Srow>
      <Srow t="Status" d="What this resolves to now."><Show when={bound().ok} fallback={<><span class="st st-bad"><I n="x-circle" s={12} />{(bound() as { text: string }).text}</span><A class="lnk" to={orchTo(m().id, "models")}>Fix in Models</A></>}><span class="st st-ok"><I n="check-circle" s={12} />Bound to {(bound() as { route: string }).route}</span></Show></Srow>
    </div>
    <Adv id="coordinator" force={!!route() || Object.keys(coord()?.parameters ?? {}).length > 0 || d.problems().length > 0}>
      <Show when={coord()}><div class="surface ws-pad"><div class="ws-fld"><span class="lab">Other coordinator parameters<Hint text={HELP.parameters} label="parameters" /></span><KeyValues label="Parameter" values={coord()!.parameters} disabled={off()} onChange={parameters => set({ coordinator: { ...coord()!, parameters } })} /></div></div></Show>
      <SetH title="Exact default route"><Hint text={HELP.defaultRoute} label="exact route" /></SetH>
      <Show when={route()} fallback={<div class="surface"><Srow t="Portable coordinator is in use" d="An exact route changes if a connection is registered again, so the portable form is better."><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) set({ routeDefault: first(), coordinator: null }); }}>Use an exact route</button></Srow></div>}>
        <div class="surface">
          <Srow t="Model" d="An exact route changes if a connection is registered again."><Pick id="m-def-route" label="Route model" width="260px" value={route()!.routeId} disabled={off()} options={routeChoices()} onPick={id => { const r = routes().find(x => x.id === id); if (r) patchRoute({ routeId: id, model: r.model }); }} /></Srow>
          <div class="ws-errline"><Err problems={d.problems()} path="route_default.route_id" /></div>
          <Srow t="Connection"><Pick id="m-def-conn" label="Route connection" width="200px" value={route()!.connectionId} disabled={off()} options={connectionChoices()} onPick={connectionId => patchRoute({ connectionId })} /></Srow>
          <div class="ws-pad"><KeyValues label="Route parameter" values={route()!.parameters} disabled={off()} onChange={parameters => patchRoute({ parameters })} /></div>
          <Srow t="Clear the exact route" d="Goes back to the portable coordinator."><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) set({ routeDefault: null, coordinator: coord() ?? { engine: "claude-agent-acp", model: route()!.model, effort: "medium", parameters: {} } }); }}>Clear</button></Srow>
        </div>
      </Show>
      <Err problems={d.problems()} path="route_default" />
    </Adv>
  </Shell>;
}

const recipients = () => [...new Set(ENGINES.map(e => e.recipient))].map(id => ({ id, label: id }));
/** Off means no restriction. A list means only these, and an empty list means nothing at all. */
function ListLimit(props: { label: string; value: string[] | null; options: { id: string; label: string }[]; off?: string; onChange: (v: string[] | null) => void }) {
  const flip = (id: string, on: boolean) => { const now = props.value ?? []; props.onChange(on ? [...now, id] : now.filter(x => x !== id)); };
  return <div class="ws-limit">
    <label class="sw-l">Restrict<Sw label={`Restrict ${props.label}`} checked={props.value !== null} disabled={props.off} onChange={on => props.onChange(on ? [] : null)} /></label>
    <Show when={props.value !== null}>
      <div class="ws-checks"><For each={props.options}>{o => <label class="ws-check"><Chk label={`${props.label}: ${o.label}`} checked={!!props.value?.includes(o.id)} disabled={props.off} onChange={on => flip(o.id, on)} /><span class="trunc">{o.label}</span></label>}</For></div>
      <Show when={props.value?.length === 0}><span class="cap st-warn">Nothing would be allowed.</span></Show>
    </Show>
  </div>;
}
function StoredLimits(props: { c: Ceiling }) {
  const s = () => props.c.stored;
  const rows = () => [["Permission level", s().permissionLevel], ["Calls", s().calls], ["Tokens", s().tokens], ["Concurrency", s().concurrency], ["Spend limit", s().spendLimit], ["Spend limit enforceable", s().spendLimit === null ? null : s().spendEnforceable ? "Yes" : "No"]].filter(([, v]) => v !== null) as [string, string | number][];
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
function MachineLimits() {
  const d = useDefaults(), off = useReadOnly(), o = useLimitOptions();
  const c = () => d.value().ceiling, set = (x: Partial<Ceiling>) => d.set({ ...d.value(), ceiling: { ...c(), ...x } });
  return <Shell scoped title="Limits and fallback" lead="The hard ceiling for this machine. A project can narrow it, never widen it." bar={<Bar d={d} />}>
    <SetH title="Permission to run"><Hint text={HELP.limits} label="limits" /></SetH>
    <div class="surface">
      <Srow t="Attempts per task" d="Empty means no limit."><NumField label="Attempts per task" value={c().attempts} placeholder="No limit" disabled={off()} onChange={attempts => set({ attempts })} /><Err problems={d.problems()} path="attempts" /></Srow>
      <Srow t={<>Connections<Hint text={HELP.allowedConnections} label="connections" /></>} d="Which engine accounts tasks may use."><ListLimit label="Connections" value={c().allowedConnections} options={o.connections()} off={off()} onChange={allowedConnections => set({ allowedConnections })} /></Srow>
      <Srow t={<>Data recipients<Hint text={HELP.allowedRecipients} label="data recipients" /></>} d="Which providers may receive project data."><ListLimit label="Data recipients" value={c().allowedRecipients} options={recipients()} off={off()} onChange={allowedRecipients => set({ allowedRecipients })} /></Srow>
    </div>
    <SetH title="Fallback"><Hint text={HELP.fallback} label="fallback" /></SetH>
    <div class="note-box plain"><I n="alert" s={14} /><span>Off unless all of these allow it.</span></div>
    <div class="surface">
      <Srow t="Failure classes" d="What may trigger a move to another route."><ListLimit label="Failure classes" value={c().failureClasses} options={FAILURE_CLASSES} off={off()} onChange={failureClasses => set({ failureClasses })} /></Srow>
      <Srow t="Budget" d="How many moves one task may make. Empty means no limit."><NumField label="Fallback budget" value={c().budget} placeholder="No limit" disabled={off()} onChange={budget => set({ budget })} /><Err problems={d.problems()} path="fallback.budget" /></Srow>
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
  </Shell>;
}

const STATE_NOTE: Record<CeilingFieldState, { text: string; tone: string } | null> = {
  not_set: { text: "Starts from the machine ceiling", tone: "muted" }, narrows: null, no_longer_narrows: { text: "No longer narrows", tone: "st-warn" }, violation: { text: "Must stay within the machine limit.", tone: "st-bad" },
};
const listText = (v: string[] | null, of?: { id: string; label: string }[]) => v === null ? "Any" : v.length ? v.map(x => of?.find(o => o.id === x)?.label ?? x).join(", ") : "Nothing";
const numText = (n: number | null) => n === null ? "No limit" : String(n);
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
  return <Shell scoped wide title="Limits and fallback" lead={`What ${name(props.projectId)} may do. It can narrow the machine ceiling, never widen it.`} bar={<Bar d={d} />}
    head={<Show when={own()}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("save_surface")} onClick={() => { if (!off()) remove(); }}>Remove project ceiling…</button></Show>}>
    <Show when={!own()}><div class="note-box plain"><I n="pencil" s={14} /><span>This project has no ceiling of its own. Saving creates one.</span></div></Show>
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
export function Limits() {
  const ws = useWs();
  return <Show when={ws.settings.scope()} keyed fallback={<MachineLimits />}>{id => <ProjectLimits projectId={id} />}</Show>;
}
