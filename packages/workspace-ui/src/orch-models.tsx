import { createMemo, createSignal, For, Show } from "solid-js";
import { autoOn, autoVerdict, CONNECTION_ENGINE, denyTokens, EFFORT_LEVELS, effortLabel, engineInUse, ENGINES, notOffered, orphanTokens, policyOf, ROLE_CATALOG, setDeny, setEngineUse, setModelAuto, setModelEnabled, setPolicy, setPreferred, setProvider, setStretch, setTier, WORK_KINDS, type CatalogueModel, type EngineKey, type ModelPolicy, type Problem, type RoutingRules } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I } from "./kit";
import { HELP, PREFERRED_WHEN_HELP, PREFERRED_WORK_HELP, workKindHelp } from "./orch-help";
import { Bar, orchTo, sectionTo, Shell, slug, SourceStrips, useReadOnly, useRouting, useRoutingOff } from "./orch-shared";
import { clock } from "./selectors";
import { Adv, Chk, Err, Hint, Pick, Rec, SetH, Srow, Sw, Tags, Tiers } from "./settings-kit";

type Fields = { engine: EngineKey; model: CatalogueModel; rules: RoutingRules; set: (r: RoutingRules) => void; off?: string; problems: Problem[] };
const kindName = (id: string) => WORK_KINDS.find(k => k.id === id)?.label ?? `${id} (legacy, kept)`;
/** Preferred work in words, for a cell that has no room for the checklist. */
export const preferredWords = (p: ModelPolicy) => (p.preferredFor ?? []).map(kindName).join(", ");

/** The twelve work kinds as a checklist, each with the roles it covers. Kinds an older file still names stay, marked as kept. */
export function PreferredWork(props: Fields & { needsAuto?: boolean }) {
  const e = () => props.engine, id = () => props.model.id;
  const p = () => policyOf(props.rules, e(), id());
  const auto = () => props.rules.engines[e()].auto.includes(id());
  const legacy = () => (p().preferredFor ?? []).filter(k => !WORK_KINDS.some(w => w.id === k));
  const why = () => props.off ?? (props.needsAuto !== false && !auto() ? "Turn on Automatic first" : undefined);
  return <div class="ws-fld"><span class="lab">Preferred work<Hint text={PREFERRED_WORK_HELP} label="preferred work" /></span>
    <div class="ws-checks ws-kinds">
      <For each={WORK_KINDS}>{k => <label class="ws-check"><Chk label={`${props.model.label} preferred for ${k.label}`} checked={!!p().preferredFor?.includes(k.id)} disabled={why()} onChange={on => props.set(setPreferred(props.rules, e(), id(), k.id, on))} /><span><span class="ws-kind-t">{k.label}</span><span class="ws-kind-d">{ROLE_CATALOG.filter(r => r.kind === k.id).map(r => r.label).join(", ") || workKindHelp(k.id, [])}</span></span></label>}</For>
      <For each={legacy()}>{k => <label class="ws-check"><Chk label={`${props.model.label} preferred for ${k}`} checked disabled={props.off} onChange={on => props.set(setPreferred(props.rules, e(), id(), k, on))} /><span>{k} (legacy, kept)</span></label>}</For>
    </div>
    <Err problems={props.problems} path={`models.${e()}.policy.${id()}.preferred_for`} />
  </div>;
}
export function PreferWhen(props: Fields) {
  const p = () => policyOf(props.rules, props.engine, props.model.id);
  const patch = (x: Partial<ModelPolicy>) => props.set(setPolicy(props.rules, props.engine, props.model.id, x));
  return <div class="ws-fld"><span class="lab">Prefer {props.model.label} when<Hint text={PREFERRED_WHEN_HELP} label="Prefer when" /></span>
    <textarea class="inp ws-when" rows="3" aria-label={`Prefer ${props.model.label} when`} disabled={!!props.off} value={p().preferredWhen ?? ""} onInput={ev => patch({ preferredWhen: ev.currentTarget.value || null })} />
    <span class={`cap tnum ${(p().preferredWhen?.length ?? 0) > 500 ? "st-bad" : "faint"}`}>{p().preferredWhen?.length ?? 0} of 500. The coordinator judges this. Retries ignore it.</span>
    <Err problems={props.problems} path={`models.${props.engine}.policy.${props.model.id}.preferred_when`} />
  </div>;
}

/** Routing › Model policy: every per-model field. Stored values stay visible even when the model no longer offers them. */
export function ModelPolicyFields(props: Fields) {
  const e = () => props.engine, id = () => props.model.id;
  const p = () => policyOf(props.rules, e(), id());
  const at = () => `models.${e()}.policy.${id()}`;
  const patch = (x: Partial<ModelPolicy>) => props.set(setPolicy(props.rules, e(), id(), x));
  const tiers = () => props.rules.engines[e()].tiers[id()] ?? [];
  const light = () => tiers().includes("light");
  const offered = (value: string | null) => [{ id: "", label: "Neutral" }, ...props.model.efforts.map(x => ({ id: x, label: effortLabel(x) })), ...(value && !props.model.efforts.includes(value) ? [{ id: value, label: effortLabel(value), sub: "Not offered by this model" }] : [])];
  const level = (label: string, value: string | null, key: "effortFloor" | "maxEffort") => <Pick id={`m-${key}-${slug(`${e()}-${id()}`)}`} label={`${label} for ${props.model.label}`} width="120px" value={value ?? ""} options={offered(value)} disabled={props.off} onPick={x => patch({ [key]: x || null })} />;
  const ladder = () => p().effortLadder ?? [];
  const setRung = (i: number, value: string) => { const next = ladder().slice(0, i); if (value) next[i] = value; const rest = ladder().slice(i + 1); patch({ effortLadder: [...next, ...(value ? rest : [])].length ? [...next, ...(value ? rest : [])] : null }); };
  return <div class="ws-detail-b">
    <div class="ws-fld"><span class="lab">Tiers</span><Tiers label={props.model.label} on={tiers()} disabled={props.off} onChange={(t, on) => props.set(setTier(props.rules, e(), id(), t, on))} /><Err problems={props.problems} path={`models.${e()}.tiers.${id()}`} /></div>
    <div class="ws-row2">
      <div class="ws-fld"><span class="lab">Effort floor<Hint text={HELP.modelPolicy} label="model policy" /></span>{level("Effort floor", p().effortFloor, "effortFloor")}<span class="cap faint">Never run it below this level.</span></div>
      <div class="ws-fld"><span class="lab">Max effort</span>{level("Max effort", p().maxEffort, "maxEffort")}<span class="cap faint">Never run it above this level.</span><Err problems={props.problems} path={`${at()}.max_effort`} /></div>
    </div>
    <PreferredWork {...props} />
    <PreferWhen {...props} />
    <Show when={light()}>
      <div class="ws-fld"><span class="lab">Effort ladder</span><span class="cap faint">Effort for low, normal and high complexity, for light models.</span>
        <div class="row ws-gap4"><For each={[0, 1, 2]}>{i => <Pick id={`m-ladder-${i}-${slug(`${e()}-${id()}`)}`} label={`Ladder step ${i + 1} for ${props.model.label}`} width="96px" value={ladder()[i] ?? ""} disabled={props.off ?? (i > ladder().length ? "Set the step before it" : undefined)} options={[{ id: "", label: "None" }, ...EFFORT_LEVELS.map(x => ({ id: x as string, label: effortLabel(x) }))]} onPick={x => setRung(i, x)} />}</For></div>
        <Err problems={props.problems} path={`${at()}.effort_ladder`} />
      </div>
      <div class="ws-fld"><span class="lab">Also standard for</span><span class="cap faint">Work kinds where this light model may also serve standard work.</span>
        <div class="ws-checks"><For each={WORK_KINDS}>{k => <label class="ws-check"><Chk label={`${props.model.label} also standard for ${k.label}`} checked={!!p().alsoStandardFor?.includes(k.id)} disabled={props.off} onChange={on => props.set(setStretch(props.rules, e(), id(), k.id, on))} /><span>{k.label}</span></label>}</For></div>
      </div>
    </Show>
    <div class="ws-fld"><span class="lab">Request names</span><span class="cap faint">Short names you can type to ask for this model, for example opus.</span>
      <Tags label={`Request name for ${props.model.label}`} hint="Add a name" items={p().requestNames ?? []} disabled={props.off} onChange={items => patch({ requestNames: items.length ? items : null })} />
      <Err problems={props.problems} path={`${at()}.request_names`} />
    </div>
  </div>;
}

/** A deny rule broader than this model's own: switching it on here would not help, so it says which rule and offers to remove it. */
const blockingRule = (rules: RoutingRules, engine: EngineKey, model: string) => denyTokens(rules, engine, model).find(t => t !== `${engine}/*`);

/** Section 2: which models tasks may use at all. Everything about how routing uses them is under Engines and models. */
export function Models() {
  const ws = useWs();
  const d = useRouting(), off = useRoutingOff(), offline = useReadOnly();
  const v = () => d.value(), m = () => ws.machine(), cat = () => m().config.catalogue;
  const [filter, setFilter] = createSignal("");
  const shown = (e: EngineKey) => { const q = filter().trim().toLowerCase(); return cat()[e].models.filter(x => !q || `${x.label} ${x.id}`.toLowerCase().includes(q)); };
  const bulk = (e: EngineKey, on: boolean) => d.set(shown(e).filter(x => !blockingRule(v(), e, x.id)).reduce((r, x) => setModelEnabled(r, e, x.id, on), v()));
  const refresh = () => { if (!offline()) ws.run({ type: "refresh_models", machineId: m().id, now: ws.store.clock() }); };
  const connections = () => m().connections;
  return <Shell section="models" wide title="Models" lead="Switch on the models tasks may use. Only these appear in the model selector and in automatic routing. New models stay off until you switch them on." bar={<Bar d={d} />}
    head={<div class="row ws-gap4 ws-head-acts"><input class="inp ws-filter" aria-label="Filter models" placeholder="Filter models" value={filter()} onInput={e => setFilter(e.currentTarget.value)} /><button class="btn btn-line btn-sm" aria-disabled={offline() ? "true" : undefined} data-why={offline()} {...ws.needs("refresh_models")} onClick={refresh}><I n="refresh" s={12} />Refresh model lists</button></div>}>
    <SourceStrips />
    <section class="set-grp" aria-labelledby="set-lists-h">
      <SetH title={<span id="set-lists-h">Model lists</span>}><Hint text={HELP.modelLists} label="model lists" /></SetH>
      <Rec>After an engine update, refresh to see new models.</Rec>
      <Show when={connections().length > 0} fallback={<div class="surface ws-empty">No connection is registered. Add one under <A class="lnk" to={sectionTo(m().id, "connections")}>Connections</A>.</div>}>
        <div class="surface"><For each={connections()}>{x => {
          const c = () => cat()[CONNECTION_ENGINE[x.engine]];
          return <Srow t={`${ENGINES.find(e => e.id === CONNECTION_ENGINE[x.engine])?.label ?? x.engine} · ${x.label}`} d={c()?.failed ? "List could not be read; the previous one is kept." : c()?.observedAt ? `Observed ${clock(c()!.observedAt)}, ${c()!.models.length} model${c()!.models.length === 1 ? "" : "s"} listed.` : "Not read yet."}>
            <Show when={c()?.failed}><span class="st st-warn"><I n="alert" s={12} />Not read</span></Show>
          </Srow>;
        }}</For></div>
      </Show>
    </section>
    <For each={ENGINES}>{e => {
      const rules = () => v().engines[e.id], c = () => cat()[e.id], used = () => engineInUse(v(), e.id);
      const on = () => c().models.filter(x => rules().enabled.includes(x.id)).length;
      const gone = () => notOffered(v(), cat(), e.id);
      const hasConnection = () => connections().some(x => CONNECTION_ENGINE[x.engine] === e.id);
      const engineOff = () => used() ? undefined : `${e.label} is switched off under Engines and models`;
      return <section class="set-grp" aria-labelledby={`set-eng-${slug(e.id)}`}>
        <SetH title={<span id={`set-eng-${slug(e.id)}`}>{e.label}</span>}>
          <Show when={c().models.length > 0}><span class="cap faint ws-w400 tnum">{on()} of {c().models.length} on</span></Show>
        </SetH>
        {/* On their own line, so the engine's name always has the heading's width, also on a phone. */}
        <Show when={shown(e.id).length > 1}>
          <div class="row ws-gap4 ws-head-acts">
            <button class="btn btn-line btn-sm" aria-disabled={off() ?? engineOff() ? "true" : undefined} data-why={off() ?? engineOff()} onClick={() => { if (!off() && !engineOff()) bulk(e.id, true); }}>Turn on all shown</button>
            <button class="btn btn-line btn-sm" aria-disabled={off() ?? engineOff() ? "true" : undefined} data-why={off() ?? engineOff()} onClick={() => { if (!off() && !engineOff()) bulk(e.id, false); }}>Turn off all shown</button>
          </div>
        </Show>
        <Show when={!used()}><div class="note-box plain"><I n="alert" s={14} /><span>{e.label} is switched off, so its models are hidden and never routed. <A class="lnk" to={orchTo(m().id, "engines")}>Engines and models</A></span></div></Show>
        <Show when={c().models.length > 0} fallback={<div class="surface ws-empty">{e.id === "opencode" ? "No models. Enable OpenCode under Connections first." : hasConnection() ? "No models known yet: refresh the model lists." : "No connection for this engine."}</div>}>
          <Show when={shown(e.id).length > 0} fallback={<div class="surface ws-empty">No model matches the filter.</div>}>
            <div class="surface"><For each={shown(e.id)}>{x => {
              const rule = () => blockingRule(v(), e.id, x.id);
              return <Srow t={x.label} d={<><span class="mono">{x.id}</span>{x.observed ? x.contextWindow ? `, context window ${x.contextWindow.toLocaleString("en")} tokens` : "" : ". Not confirmed by the engine yet"}<Show when={rule()}>{r => <span class="ws-block">Off because of "{r()}", which can cover other models too.</span>}</Show></>}>
                <Show when={rule()}>{r => <button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) d.set(setDeny(v(), v().deny.filter(t => t !== r()))); }}>Remove rule "{r()}"</button>}</Show>
                <Sw label={`${x.label} enabled`} checked={rules().enabled.includes(x.id) && !rule()} disabled={off() ?? engineOff() ?? (rule() ? `Off because of "${rule()}"` : undefined)} onChange={on => d.set(setModelEnabled(v(), e.id, x.id, on))} />
              </Srow>;
            }}</For></div>
          </Show>
        </Show>
        <Show when={gone().length > 0}>
          <SetH title={`${e.label}: not offered now`} />
          <div class="surface"><For each={gone()}>{id => <Srow t={<span class="mono">{id}</span>} d={`Switched on here, but ${e.label} does not list it now. Nothing is inferred.`}><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} aria-label={`Remove ${id}`} onClick={() => { if (!off()) d.set(setModelEnabled(v(), e.id, id, false)); }}>Remove</button></Srow>}</For></div>
        </Show>
      </section>;
    }}</For>
  </Shell>;
}

const SETUP_WORDS = { not_downloaded: "Not downloaded", downloaded_no_login: "Downloaded, sign in on this machine", ready_to_connect: "Ready to connect", connected: "Connected" } as const;
const STATE_WORDS = { connected: "configured", needs_login: "sign in needed" } as Record<string, string>;
/** Section 3: how automatic routing uses the models that are on. Every model stays selectable by hand. */
export function EnginesModels() {
  const ws = useWs();
  const d = useRouting(), off = useRoutingOff();
  const v = () => d.value(), m = () => ws.machine(), cat = () => m().config.catalogue;
  const [open, setOpen] = createSignal<string | null>(null);
  const orphans = createMemo(() => [...orphanTokens(v(), cat()).map(t => ({ t, list: "deny" as const })), ...v().autoAllow.filter(t => !ENGINES.some(e => cat()[e.id].models.some(x => `${e.id}/${x.id}` === t))).map(t => ({ t, list: "auto" as const }))]);
  const dropToken = (x: { t: string; list: "deny" | "auto" }) => d.set(x.list === "deny" ? setDeny(v(), v().deny.filter(t => t !== x.t)) : { ...v(), autoAllow: v().autoAllow.filter(t => t !== x.t) });
  return <Shell section="engines" wide title="Engines and models" lead="How automatic routing uses the models you switched on. Every model stays selectable by hand." bar={<Bar d={d} />}
    head={<A class="btn btn-line btn-sm" to={orchTo(m().id, "effective")}><I n="eye" s={12} />Preview routes</A>}>
    <SourceStrips />
    <section class="set-grp" aria-labelledby="set-legend-h">
      <SetH title={<span id="set-legend-h">What these switches change</span>}><Hint text={HELP.switchesLegend} label="engine and model switches" /></SetH>
      <div class="surface ws-pad ws-legend">
        <p>Use: off hides every model of the engine and refuses it when chosen or pinned. Automatic routing: off keeps its models selectable by hand only.</p>
        <p>Automatic: the router may pick the model by itself, at the tiers you mark. Preferred work ranks it first for those kinds of work.</p>
      </div>
    </section>
    <For each={ENGINES}>{e => {
      const rules = () => v().engines[e.id], used = () => engineInUse(v(), e.id), auto = () => v().providers.includes(e.id), c = () => cat()[e.id];
      const last = () => auto() && v().providers.length === 1;
      const own = () => m().connections.filter(x => CONNECTION_ENGINE[x.engine] === e.id);
      const rows = () => c().models.filter(x => rules().enabled.includes(x.id));
      const hasConnection = () => own().length > 0;
      return <section class="set-grp" aria-labelledby={`set-em-${slug(e.id)}`}>
        <SetH title={<span id={`set-em-${slug(e.id)}`}>{e.label}<Show when={e.id === "opencode"}><span class="faint ws-w400"> optional engine</span></Show></span>} />
        <div class="surface">
          <Srow t="Now" d={`${SETUP_WORDS[m().config.setup[e.id].status]}${own().length ? `. ${own().map(x => `${x.label}, ${STATE_WORDS[x.state] ?? "unavailable"}`).join("; ")}` : ". No connection."}`} />
          <Srow t={`Use ${e.label}`} d={used() ? "Its models can be chosen and routed." : `Off: every ${e.label} model is hidden from the selector, refused when chosen or pinned, and never routed. Connections and logins stay.`} r="On for every engine you pay for.">
            <Sw label={`Use ${e.label}`} checked={used()} disabled={off()} onChange={on => d.set(setEngineUse(v(), e.id, on))} />
          </Srow>
          <Srow t={`Automatic routing may use ${e.label}`} d={auto() ? "Workers may be routed to its models automatically." : "Its models stay selectable by hand; automatic routing, fallback and pins skip it."} r="On, so workers can use both plans.">
            <Sw label={`Automatic routing may use ${e.label}`} checked={auto()} disabled={off() ?? (last() ? "At least one engine must stay available to automatic routing" : undefined)} onChange={on => d.set(setProvider(v(), e.id, on))} />
            <Err problems={d.problems()} path="providers" />
          </Srow>
        </div>
        <Show when={rows().length > 0} fallback={<div class="surface ws-empty">{!hasConnection() && !c().models.length ? "No models: no connection for this engine." : !c().models.length ? "No models known yet: refresh the model lists under Models." : <>No model of this engine is switched on (see <A class="lnk" to={orchTo(m().id, "models")}>Models</A>).</>}</div>}>
          <div class="surface"><div class="gt">
            <div class="gr oerow th"><span>Model</span><span>Automatic</span><span>Tiers</span><span>Preferred work</span><span></span></div>
            <For each={rows()}>{x => {
              const key = `${e.id}:${x.id}`, isOpen = () => open() === key;
              const isAuto = () => autoOn(v(), e.id, x.id);
              const p = () => policyOf(v(), e.id, x.id);
              const tierError = () => d.problems().some(q => q.path === `models.${e.id}.tiers.${x.id}`);
              const tokens = () => denyTokens(v(), e.id, x.id);
              const allowToken = () => `${e.id}/${x.id}`;
              return <>
                <div class="gr oerow">
                  <span class="trunc"><span class="w500 trunc">{x.label}</span><span class="sub mono trunc">{x.id}</span></span>
                  <span><Sw label={`Pick ${x.label} automatically`} checked={isAuto()} disabled={off() ?? (rules().enabled.includes(x.id) ? undefined : "Switched off under Models")} onChange={on => d.set(setModelAuto(v(), e.id, x.id, on))} /><Show when={tierError()}><span class="ws-err" data-problem="">Choose a tier.</span></Show></span>
                  <span><Tiers label={x.label} on={rules().tiers[x.id] ?? []} disabled={off() ?? (isAuto() ? undefined : "Turn on Automatic first")} onChange={(t, on) => d.set(setTier(v(), e.id, x.id, t, on))} /></span>
                  <span class="cap muted trunc">{preferredWords(p())}</span>
                  <button class="ib ws-ib" aria-label={`Details of ${x.label}`} aria-expanded={isOpen()} onClick={() => setOpen(isOpen() ? null : key)}><I n={isOpen() ? "chevron-down" : "chevron-right"} s={14} /></button>
                </div>
                <Show when={isOpen()}>
                  {/* A row of one cell across the table, so the table stays a table with the details open. */}
                  <div role="row"><div class="ws-mdetail" role="cell" aria-colspan={5}>
                    <div class="ws-kv"><span class="cap muted">Now</span><span class="cap">{autoVerdict(v(), cat(), e.id, x.id)}<Show when={tokens().length > 0}>, matching <span class="mono">{tokens().join(", ")}</span></Show></span></div>
                    <PreferredWork engine={e.id} model={x} rules={v()} set={r => d.set(r)} off={off()} problems={d.problems()} />
                    <PreferWhen engine={e.id} model={x} rules={v()} set={r => d.set(r)} off={off()} problems={d.problems()} />
                    <Show when={e.id === "opencode"}>
                      <Srow t={`Allow ${x.label} in automatic routing`} d="OpenCode routes stay out of automatic routing unless allowed one by one. This writes an auto-allow entry for this exact route.">
                        <Sw label={`Allow ${x.label} in automatic routing`} checked={v().autoAllow.includes(allowToken())} disabled={off()} onChange={on => d.set({ ...v(), autoAllow: on ? [...v().autoAllow, allowToken()] : v().autoAllow.filter(t => t !== allowToken()) })} />
                      </Srow>
                    </Show>
                    <A class="lnk" to={orchTo(m().id, "routing", { tab: `policy:${e.id}:${x.id}` })}>Effort and policy for {x.label}</A>
                  </div></div>
                </Show>
              </>;
            }}</For>
          </div></div>
        </Show>
      </section>;
    }}</For>
    <Show when={orphans().length > 0}>
      <section class="set-grp" aria-labelledby="set-other-h">
        <SetH title={<span id="set-other-h">Other entries in this file</span>} />
        <div class="surface"><For each={orphans()}>{x => <Srow t={<span class="mono">{x.t}</span>} d={x.list === "deny" ? "Deny entry that matches no model this machine knows." : "Auto-allow entry that matches no model this machine knows."}><button class="btn btn-line btn-sm" aria-label={`Remove ${x.t}`} aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) dropToken(x); }}>Remove</button></Srow>}</For></div>
      </section>
    </Show>
  </Shell>;
}
