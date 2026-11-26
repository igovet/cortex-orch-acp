import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { ACCESS_MODES, CONNECTION_ENGINE, configIssues, coordinatorAllowed, coordinatorEfforts, coordinatorResolution, denyTokens, effortLabel, engineLabel, ENGINE_KEYS, LANGUAGES, modelLabel, resolveEffort, routeIdOf, TIERS, type CommandReceipt, type EngineKey } from "@cortex/workspace-domain";
import { workspaceCopy } from "./copy";
import { CATEGORY_LABELS } from "./orch-coordinator";
import { useWs } from "./ctx";
import { A, I } from "./kit";
import { HELP } from "./orch-help";
import { MachineCeiling } from "./orch-limits";
import { Bar, orchTo, Shell, useDefaults, useReadOnly } from "./orch-shared";
import { Adv, Err, Hint, KeyValues, Pick, Rec, Seg, SetH, Srow } from "./settings-kit";
import { share } from "./stable";

const FALLBACK_EFFORTS = "Not observed: this is a general list.";
/** Apply and Reset run at once; what came of the last one stays under the group until the next. */
function Preset() {
  const ws = useWs(), off = useReadOnly();
  const m = () => ws.machine(), f = () => m().config.files;
  const [outcome, setOutcome] = createSignal<{ ok: boolean; text: string; conflict: boolean } | null>(null);
  const take = (done: string) => (r: CommandReceipt) => { setOutcome(r.ok ? { ok: true, text: done, conflict: false } : { ok: false, text: r.message || "The machine refused this. Nothing was changed.", conflict: ["revision_conflict", "stale_version"].includes(r.outcome) }); return r.ok; };
  const apply = () => { if (!off() && f().missing.length) ws.run({ type: "apply_preset", machineId: m().id }).then(take(`Created ${f().missing.join(", ")} from the default preset.`)); };
  const simulated = () => ws.store.mode === "demo" ? " Simulated: nothing is written." : "";
  const reset = () => ws.open({
    kind: "prompt", title: "Reset to the default preset?", label: "Type reset to confirm", action: "Reset", hint: `Replaces routing.json and defaults.json: all routing rules, the coordinator, the language and the machine ceiling. Each replaced file is kept beside it as <name>.json.cortex-backup-<time>. Project routing and the environment override are not changed.${simulated()}`,
    go: text => text.trim().toLowerCase() === "reset" ? ws.run({ type: "reset_preset", machineId: m().id, confirmed: true }).then(take("Routing and defaults now match the default preset. The replaced files are kept as backups.")) : false,
  });
  return <section class="set-grp" aria-labelledby="set-preset-h">
    <SetH title={<span id="set-preset-h">Default preset</span>}><Hint text={HELP.defaultPreset} label="default preset" /></SetH>
    <div class="surface">
      <Srow t="Apply preset" d={f().missing.length ? `Creates only the files that are missing: ${f().missing.join(", ")}. It never replaces one.` : "Creates only the files that are missing. It never replaces one."}>
        <button class="btn btn-line btn-sm" aria-disabled={f().missing.length && !off() ? undefined : "true"} data-why={off() ?? (f().missing.length ? undefined : "Every file exists, so there is nothing to create")} {...ws.needs("apply_preset")} onClick={apply}>Apply preset</button>
      </Srow>
      <Srow t="Reset to default" d={f().backups ? `Replaces routing and defaults with the preset, after a warning. ${f().backups} private backups are kept.` : "Replaces routing and defaults with the preset, after a warning. A backup of each file is kept."}>
        <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs("reset_preset")} onClick={() => { if (!off()) reset(); }}>Reset to default…</button>
      </Srow>
    </div>
    <Show when={outcome()}>{o => <div class={`note-box${o().ok ? " plain" : ""}`} role="status"><I n={o().ok ? "check-circle" : "alert"} s={14} /><span>{o().text}</span><Show when={o().conflict}><span class="ws-push"></span><button class="btn btn-line btn-sm" onClick={() => setOutcome(null)}>Read again</button></Show></div>}</Show>
  </section>;
}

/** Section 1: what a new task starts with, and the widest limits. One draft of defaults.json for everything but the preset. */
export function Defaults(props: { focus?: "ceiling" }) {
  const ws = useWs();
  const d = useDefaults(), off = useReadOnly();
  const v = () => d.value(), m = () => ws.machine(), c = () => m().config, set = (patch: Partial<ReturnType<typeof v>>) => d.set({ ...v(), ...patch });
  // Every machine read rebuilds these lines: equal ones stay the old objects, so their rows (and focus on them) stay.
  const issues = createMemo<ReturnType<typeof configIssues>>(prev => share(prev, configIssues(m())), []);
  const available = (e: EngineKey, id: string) => c().setup[e].status === "connected" && !c().catalogue[e].failed && c().catalogue[e].models.some(m => m.id === id) && c().surfaces.routing.engines[e].enabled.includes(id) && denyTokens(c().surfaces.routing, e, id).length === 0;
  // Only models of Coordinator models may coordinate (a profile without that list: models with a tier). A saved one outside stays shown, marked.
  const inProfile = (e: EngineKey, id: string) => coordinatorAllowed(c().surfaces.routing, e, id, c().catalogue[e].models.find(x => x.id === id)?.efforts ?? [])?.length !== 0;
  const coord = () => v().coordinator;
  const listed = () => !!c().surfaces.routing.coordinatorModels;
  /** The coordinator list in words: each model with its categories. */
  const listWords = () => (c().surfaces.routing.coordinatorModels ?? []).map(x => `${modelLabel(c().catalogue, x.engine, x.model)} (${TIERS.filter(t => x.tiers[t]).map(t => CATEGORY_LABELS[t]).join(", ")})`).join("; ");
  const models = createMemo(() => {
    const list = ENGINE_KEYS.flatMap(e => c().surfaces.routing.engines[e].enabled.filter(id => available(e, id) && inProfile(e, id)).map(id => ({ id: `${e}|${id}`, label: modelLabel(c().catalogue, e, id), sub: engineLabel(e) })));
    const now = coord(), key = now ? `${now.engine}|${now.model}` : "";
    const outside = listed() ? "Not in Coordinator models" : "No tier in Models";
    return !now || list.some(x => x.id === key) || !available(now.engine, now.model) ? list : [{ id: key, label: modelLabel(c().catalogue, now.engine, now.model), sub: outside, off: listed() ? "Unavailable: this model is not in Coordinator models" : "Unavailable: this model has no tier in Models" }, ...list];
  });
  const efforts = () => coord() ? coordinatorEfforts(c().catalogue, coord()!.engine, coord()!.model, c().surfaces.routing) : { levels: [], observed: true };
  const pick = (value: string) => {
    const [engine, model] = value.split("|") as [EngineKey, string];
    const levels = coordinatorEfforts(c().catalogue, engine, model, c().surfaces.routing).levels;
    const { thought_level, ...parameters } = coord()?.parameters ?? {};
    set({ coordinator: { engine, model, effort: resolveEffort(coord()?.effort, thought_level, levels), parameters } });
  };
  const bound = createMemo(() => coordinatorResolution({ ...m(), config: { ...c(), surfaces: { ...c().surfaces, user_defaults: v() } } }));
  const routes = createMemo(() => ENGINE_KEYS.flatMap(e => c().catalogue[e].models.filter(x => available(e, x.id)).map(x => ({ id: x.routeId ?? routeIdOf(e, x.id), label: x.label, sub: engineLabel(e), engine: e, model: x.id }))));
  const first = () => { const e = coord()?.engine ?? "claude-agent-acp", id = coord()?.model ?? c().surfaces.routing.engines[e].enabled[0]; return { routeId: routes().find(r => r.engine === e && r.model === id)?.id ?? routeIdOf(e, id), connectionId: m().connections.find(x => CONNECTION_ENGINE[x.engine] === e)?.detail.connectionId ?? "", model: id, parameters: {} }; };
  const route = () => v().routeDefault;
  const clearRoute = () => {
    const exact = route();
    if (off() || !exact) return;
    const offered = routes().find(r => r.id === exact.routeId);
    const connection = m().connections.find(x => x.detail.connectionId === exact.connectionId);
    const engine = offered?.engine ?? (connection && CONNECTION_ENGINE[connection.engine]) ?? "claude-agent-acp";
    set({ routeDefault: null, coordinator: coord() ?? { engine, model: exact.model, effort: "medium", parameters: {} } });
  };
  // A saved route the engines no longer offer still shows, by its model, so it can be seen and replaced.
  const routeChoices = () => routes().some(r => r.id === route()?.routeId) ? routes() : [{ id: route()?.routeId ?? "", label: route()?.model || "No longer offered", sub: "Not offered now", off: "Unavailable: choose another route" }, ...routes()];
  const connectionChoices = () => { const list = m().connections.map(x => ({ id: x.detail.connectionId, label: x.label })), now = route()?.connectionId ?? ""; return list.some(x => x.id === now) ? list : [{ id: now, label: "No longer registered" }, ...list]; };
  const patchRoute = (x: Partial<NonNullable<ReturnType<typeof route>>>) => set({ routeDefault: { ...route()!, ...x } });
  const kept = () => [...c().files.preservedDefaults, ...v().preserved];
  const routeProblem = () => d.problems().some(p => p.path.startsWith("route_default"));
  // The old Limits and fallback link lands on Machine ceiling: the keyboard goes where the page scrolled, onto its heading.
  onMount(() => {
    if (props.focus !== "ceiling") return;
    document.getElementById("set-machine-ceiling")?.scrollIntoView?.({ block: "start" });
    const heading = document.getElementById("set-machine-ceiling-h")?.closest<HTMLElement>("h2, h3");
    if (heading) { heading.tabIndex = -1; heading.focus({ preventScroll: true }); }
  });
  return <Shell section="defaults" title="Defaults" lead="What a new task on this machine starts with, and the widest limits any task may use." bar={<Bar d={d} />}>
    <Show when={issues().length > 0}>
      <section class="set-grp" aria-labelledby="set-issues-h">
        <SetH title={<span id="set-issues-h">Needs attention</span>} count={issues().length} />
        <div class="surface"><For each={issues()}>{i => <A class="srow ws-link-row" to={orchTo(m().id, i.page, i.server ? { server: i.server, tab: "sandbox" } : {})}><span><span class="t ws-w400">{i.text}</span></span><span class="ctl"><I n="chevron-right" s={14} class="faint" /></span></A>}</For></div>
      </section>
    </Show>
    <Preset />
    <section class="set-grp" aria-labelledby="set-coord-h">
      <SetH title={<span id="set-coord-h">Coordinator</span>}><Hint text={HELP.coordinator} label="coordinator" /></SetH>
      <div class="surface">
        <Srow t="Model" d="The agent you talk to. Enabled, available models, grouped by engine." r="A strong general model; it plans and checks the work."><Pick id="m-def-model" label="Main model" width="240px" value={coord() ? `${coord()!.engine}|${coord()!.model}` : ""} empty="Choose a model" options={models()} disabled={off()} onPick={pick} /></Srow>
        <Show when={coord()}><Srow t="Effort" d={efforts().outside ? (listed() ? "This model is not in Coordinator models: choose another model." : "This model has no tier in Models: choose another model.") : !efforts().observed ? FALLBACK_EFFORTS : coordinatorAllowed(c().surfaces.routing, coord()!.engine, coord()!.model, []) === null ? "The levels this model reports." : listed() ? "The levels of its ranges in Coordinator models." : "The levels its tiers allow in Models."}><Seg label="Effort" value={resolveEffort(coord()!.effort, coord()!.parameters.thought_level, efforts().levels)} disabled={off()} options={efforts().levels.map(e => ({ id: e, label: effortLabel(e) }))} onChange={effort => set({ coordinator: { ...coord()!, effort } })} /></Srow></Show>
        <Srow t={workspaceCopy("coordinatorModels")} d={listed() ? (listWords() || "No model is listed, so no task can start.") : "No list yet: the coordinator may use any model with a tier under Engines and models."}><A class="btn btn-line btn-sm" to={orchTo(m().id, "coordinator")}>{listed() ? "Edit" : "Set up"}</A></Srow>
        <Srow t="Status" d="What this resolves to now."><Show when={bound().ok} fallback={<><span class="st st-bad"><I n="x-circle" s={12} />{(bound() as { text: string }).text}</span><Show when={listed()} fallback={<A class="lnk" to={orchTo(m().id, "models")}>Fix in Models</A>}><A class="lnk" to={orchTo(m().id, "coordinator")}>Fix in Coordinator models</A></Show></>}><span class="st st-ok"><I n="check-circle" s={12} />Bound to {(bound() as { route: string }).route}</span></Show></Srow>
      </div>
      <Show when={coord()}><Adv id="coordinator" label="More coordinator parameters" force={Object.keys(coord()?.parameters ?? {}).length > 0}>
        <div class="surface ws-pad"><div class="ws-fld"><span class="lab">Name and value of each parameter<Hint text={HELP.parameters} label="parameters" /></span><KeyValues label="Parameter" values={coord()!.parameters} disabled={off()} onChange={parameters => set({ coordinator: { ...coord()!, parameters } })} /></div></div>
      </Adv></Show>
    </section>
    <section class="set-grp" aria-labelledby="set-lang-h">
      <SetH title={<span id="set-lang-h">Language</span>}><Hint text={HELP.language} label="language" /></SetH>
      <div class="surface">
        <Srow t="Language of replies" d="Replies, plans and reports. Automatic follows the language of your message. It does not guarantee the language of model replies." r="Automatic, or Russian to always get Russian."><Seg label="Language" value={v().language} disabled={off()} options={LANGUAGES.map(l => ({ id: l.id, label: l.id === "auto" ? "Automatic" : l.label as string }))} onChange={language => set({ language })} /></Srow>
      </div>
    </section>
    <section class="set-grp" aria-labelledby="set-new-h">
      <SetH title={<span id="set-new-h">New tasks</span>} />
      <div class="surface">
        <Srow t="Access mode" d="What engine tools may do without asking. In every mode a command can read anything your account can read."><Seg label="Access mode" value={v().mode} disabled={off()} options={ACCESS_MODES} onChange={mode => set({ mode })} /></Srow>
        <Srow t="Checkout for new tasks" d="Preselected in New task. You choose each time."><Seg label="Default checkout" value={v().checkout} disabled={off()} options={[{ id: "worktree", label: "Worktree" }, { id: "main", label: "Project folder" }] as const} onChange={checkout => set({ checkout })} /></Srow>
      </div>
    </section>
    <section class="set-grp" aria-label="Exact default route">
      <Adv id="default-route" label="Exact default route" force={!!route() || routeProblem()}>
        <Rec>Leave this off and use the coordinator above.</Rec>
        <Show when={route()} fallback={<div class="surface"><Srow t={<>Use an exact route<Hint text={HELP.defaultRoute} label="exact route" /></>} d="Pins new tasks to one connection and model. It changes if the connection is registered again."><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) set({ routeDefault: first(), coordinator: null }); }}>Use an exact route</button></Srow></div>}>
          <div class="surface">
            <Srow t="Model" d="An exact route changes if a connection is registered again."><Pick id="m-def-route" label="Route model" width="240px" value={route()!.routeId} disabled={off()} options={routeChoices()} onPick={id => { const r = routes().find(x => x.id === id); if (r) patchRoute({ routeId: id, model: r.model }); }} /></Srow>
            <div class="ws-errline"><Err problems={d.problems()} path="route_default.route_id" /></div>
            <Srow t="Connection" d="The engine account the route runs through."><Pick id="m-def-conn" label="Route connection" width="200px" value={route()!.connectionId} disabled={off()} options={connectionChoices()} onPick={connectionId => patchRoute({ connectionId })} /></Srow>
            <div class="ws-pad"><div class="ws-fld"><span class="lab">Route parameters</span><KeyValues label="Route parameter" values={route()!.parameters} disabled={off()} onChange={parameters => patchRoute({ parameters })} /></div></div>
            <Srow t="Clear the exact route" d="Goes back to the coordinator above."><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={clearRoute}>Clear</button></Srow>
          </div>
        </Show>
        <Err problems={d.problems()} path="route_default" />
      </Adv>
    </section>
    <MachineCeiling d={d} />
    <Show when={kept().length > 0}>
      <div class="note-box plain"><I n="lock" s={14} /><span>Kept unchanged, not edited here: <span class="mono">{kept().join(", ")}</span>. <A class="lnk" to={orchTo(m().id, "files")}>Files and sources</A></span></div>
    </Show>
  </Shell>;
}
