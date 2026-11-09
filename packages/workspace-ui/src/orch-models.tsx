import { createMemo, createSignal, For, Show } from "solid-js";
import { autoOn, autoVerdict, denyTokens, EFFORT_LEVELS, effortLabel, engineInUse, engineLabel, ENGINES, kindLabel, notOffered, policyOf, setEngineUse, setModelAuto, setModelEnabled, setPolicy, setPreferred, setProvider, setStretch, setTier, TIER_LABELS, TIERS, WORK_KINDS, type CatalogueModel, type EngineKey, type ModelPolicy, type RoutingRules, type Tier } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { I } from "./kit";
import { HELP, PREFERRED_WHEN_HELP } from "./orch-help";
import { Bar, Shell, slug, SourceStrips, useReadOnly, useRouting, useRoutingOff } from "./orch-shared";
import { clock } from "./selectors";
import { Adv, Chk, Err, Hint, Pick, Sw, Tags } from "./settings-kit";

type Selected = { engine: EngineKey; model: string };
const effortRange = (p: ModelPolicy) => p.effortFloor && p.maxEffort ? `${p.effortFloor} to ${p.maxEffort}` : p.maxEffort ? `up to ${p.maxEffort}` : p.effortFloor ? `from ${p.effortFloor}` : "";

function Tiers(props: { label: string; on: Tier[]; onChange: (tier: Tier, on: boolean) => void; disabled?: string }) {
  return <span class="tier" role="group" aria-label={`${props.label} tiers`}><For each={[...TIERS].reverse()}>{t =>
    <button aria-pressed={props.on.includes(t)} aria-label={`${props.label}: ${TIER_LABELS[t]}`} data-why={props.disabled} aria-disabled={props.disabled ? "true" : undefined} onClick={e => { e.stopPropagation(); if (!props.disabled) props.onChange(t, !props.on.includes(t)); }}>{TIER_LABELS[t][0]}</button>
  }</For></span>;
}

/** Everything per model that does not fit a row. Stored values stay visible even when the model no longer offers them. */
function Detail(props: { engine: EngineKey; model: CatalogueModel; rules: RoutingRules; set: (r: RoutingRules) => void; off?: string; problems: ReturnType<ReturnType<typeof useRouting>["problems"]>; back: () => void }) {
  const ws = useWs();
  const e = () => props.engine, id = () => props.model.id;
  const p = () => policyOf(props.rules, e(), id());
  const at = () => `models.${e()}.policy.${id()}`;
  const patch = (x: Partial<ModelPolicy>) => props.set(setPolicy(props.rules, e(), id(), x));
  const light = () => !!props.rules.engines[e()].tiers[id()]?.includes("light");
  const auto = () => props.rules.engines[e()].auto.includes(id());
  const offered = (value: string | null) => [{ id: "", label: "Neutral" }, ...props.model.efforts.map(x => ({ id: x, label: effortLabel(x) })), ...(value && !props.model.efforts.includes(value) ? [{ id: value, label: effortLabel(value), sub: "Not offered by this model" }] : [])];
  const level = (label: string, value: string | null, key: "effortFloor" | "maxEffort") => <Pick id={`m-${key}-${slug(id())}`} label={label} width="120px" value={value ?? ""} options={offered(value)} disabled={props.off} onPick={x => patch({ [key]: x || null })} />;
  const legacy = () => (p().preferredFor ?? []).filter(k => !WORK_KINDS.some(w => w.id === k));
  const ladder = () => p().effortLadder ?? [];
  const setRung = (i: number, value: string) => { const next = ladder().slice(0, i); if (value) next[i] = value; const rest = ladder().slice(i + 1); patch({ effortLadder: [...next, ...(value ? rest : [])].length ? [...next, ...(value ? rest : [])] : null }); };
  const verdict = () => autoVerdict(props.rules, ws.machine().config.catalogue, e(), id());
  const tokens = () => denyTokens(props.rules, e(), id());
  return <div class="surface ws-detail">
    <button class="lnk ws-detail-back" onClick={() => props.back()}><I n="arrow-left" s={12} />Back to models</button>
    <div class="ws-detail-h"><span class="t">{props.model.label}</span><span class="sub mono">{id()}</span></div>
    <div class="ws-detail-b">
      <Show when={props.model.contextWindow}><div class="ws-fld"><span class="lab">Observed context window</span><span>{props.model.contextWindow?.toLocaleString()} tokens</span></div></Show>
      <div class="ws-fld"><span class="lab">Effort floor<Hint text={HELP.modelPolicy} label="model policy" /></span>{level("Effort floor", p().effortFloor, "effortFloor")}</div>
      <div class="ws-fld"><span class="lab">Max effort</span>{level("Max effort", p().maxEffort, "maxEffort")}<Err problems={props.problems} path={`${at()}.max_effort`} /></div>
      <div class="ws-fld"><span class="lab">Preferred for</span>
        <div class="ws-checks">
          <For each={WORK_KINDS}>{k => <label class="ws-check"><Chk label={`${props.model.label} preferred for ${k.label}`} checked={!!p().preferredFor?.includes(k.id)} disabled={props.off ?? (auto() ? undefined : "Turn on Auto first.")} onChange={on => props.set(setPreferred(props.rules, e(), id(), k.id, on))} /><span>{k.label}</span></label>}</For>
          <For each={legacy()}>{k => <label class="ws-check"><Chk label={`${props.model.label} preferred for ${k}`} checked disabled={props.off} onChange={on => props.set(setPreferred(props.rules, e(), id(), k, on))} /><span>{k} (legacy, kept)</span></label>}</For>
        </div>
        <Err problems={props.problems} path={`${at()}.preferred_for`} />
      </div>
      <Adv id="models-detail" force={!!(p().preferredWhen || p().effortLadder || p().alsoStandardFor || p().requestNames) || props.problems.some(x => x.path.startsWith(`${at()}.`))}>
        <div class="ws-fld"><span class="lab">Prefer when<Hint text={PREFERRED_WHEN_HELP} label="Prefer when" /></span>
          <textarea class="inp ws-when" rows="4" aria-label="Prefer when" disabled={!!props.off} value={p().preferredWhen ?? ""} onInput={ev => patch({ preferredWhen: ev.currentTarget.value || null })} />
          <span class={`cap tnum ${(p().preferredWhen?.length ?? 0) > 500 ? "st-bad" : "faint"}`}>{p().preferredWhen?.length ?? 0} of 500. The coordinator judges this. Retries ignore it.</span>
          <Err problems={props.problems} path={`${at()}.preferred_when`} />
        </div>
        <Show when={light()}>
          <div class="ws-fld"><span class="lab">Effort ladder</span>
            <div class="row ws-gap4"><For each={[0, 1, 2]}>{i => <Pick id={`m-ladder-${i}-${slug(id())}`} label={`Ladder step ${i + 1}`} width="84px" value={ladder()[i] ?? ""} disabled={props.off ?? (i > ladder().length ? "Set the step before it" : undefined)} options={[{ id: "", label: "None" }, ...EFFORT_LEVELS.map(x => ({ id: x as string, label: effortLabel(x) }))]} onPick={x => setRung(i, x)} />}</For></div>
            <Err problems={props.problems} path={`${at()}.effort_ladder`} />
          </div>
          <div class="ws-fld"><span class="lab">Also standard for</span>
            <div class="ws-checks"><For each={WORK_KINDS}>{k => <label class="ws-check"><Chk label={`${props.model.label} also standard for ${k.label}`} checked={!!p().alsoStandardFor?.includes(k.id)} disabled={props.off} onChange={on => props.set(setStretch(props.rules, e(), id(), k.id, on))} /><span>{k.label}</span></label>}</For></div>
          </div>
        </Show>
        <div class="ws-fld"><span class="lab">Request names</span>
          <Tags label="Request name" hint="Names you type, for example opus" items={p().requestNames ?? []} disabled={props.off} onChange={items => patch({ requestNames: items.length ? items : null })} />
          <Err problems={props.problems} path={`${at()}.request_names`} />
        </div>
      </Adv>
      <div class="ws-fld"><span class="lab">Route</span>
        <div class="ws-kv"><span class="cap muted">Engine</span><span class="cap trunc">{engineLabel(e())}</span><span class="cap muted">Automatic</span><span class="cap">{verdict()}</span>
          <Show when={tokens().length > 0}><span class="cap muted">Deny rules</span><span class="cap mono trunc">{tokens().join(", ")}</span></Show></div>
      </div>
    </div>
  </div>;
}

export function Models() {
  const ws = useWs();
  const d = useRouting(), off = useRoutingOff(), offline = useReadOnly();
  const v = () => d.value(), m = () => ws.machine(), cat = () => m().config.catalogue;
  const [filter, setFilter] = createSignal("");
  const [picked, setPicked] = createSignal<Selected | null>(null);
  const shown = (e: EngineKey) => { const q = filter().trim().toLowerCase(); return cat()[e].models.filter(x => !q || `${x.label} ${x.id}`.toLowerCase().includes(q)); };
  const bulk = (e: EngineKey, on: boolean) => d.set(shown(e).reduce((r, x) => setModelEnabled(r, e, x.id, on), v()));
  const selected = createMemo(() => { const s = picked(); const model = s && cat()[s.engine].models.find(x => x.id === s.model); return s && model ? { ...s, entry: model } : null; });
  const refresh = () => { if (!offline()) ws.run({ type: "refresh_models", machineId: m().id, now: ws.store.clock() }); };
  return <Shell scoped wide title="Models" lead="What may run, at which tier and effort. Auto means routing may pick it without you asking." bar={<Bar d={d} />}
    head={<div class="row ws-gap4"><input class="inp ws-filter" aria-label="Filter models" placeholder="Filter models" value={filter()} onInput={e => setFilter(e.currentTarget.value)} /><button class="btn btn-line btn-sm" aria-disabled={offline() ? "true" : undefined} data-why={offline()} {...ws.needs("refresh_models")} onClick={refresh}><I n="refresh" s={12} />Refresh model lists</button><Hint text={HELP.modelLists} label="model lists" /></div>}>
    <SourceStrips />
    <div class="ws-split" data-open={selected() ? "true" : "false"}>
      <div class="ws-split-l">
        <For each={ENGINES}>{e => {
          const rules = () => v().engines[e.id], used = () => engineInUse(v(), e.id), auto = () => v().providers.includes(e.id), c = () => cat()[e.id];
          const last = () => auto() && v().providers.length === 1;
          const gone = () => notOffered(v(), cat(), e.id);
          return <>
            <div class="set-h">{e.label}
              <Show when={c().observedAt !== null}><span class="cap faint ws-w400 tnum">Observed {clock(c().observedAt)}</span></Show>
              <Show when={c().failed}><span class="cap st-warn ws-w400">List could not be read, previous one kept</span></Show>
              <span class="r"><label class="sw-l">Use<Sw label={`Use ${e.label}`} checked={used()} disabled={off()} onChange={on => d.set(setEngineUse(v(), e.id, on))} /></label>
                <label class="sw-l">Automatic routing<Sw label={`${e.label} in automatic routing`} checked={auto()} disabled={off() ?? (last() ? "The last engine stays: automatic routing needs one" : undefined)} onChange={on => d.set(setProvider(v(), e.id, on))} /></label></span>
            </div>
            <Show when={!used()}><div class="cap muted">Hidden from selection, refused when pinned. Connection and login stay.</div></Show>
            <Show when={c().models.length > 0} fallback={<div class="surface ws-empty">{e.id === "opencode" ? "No models. Enable OpenCode in Engine connections first. Its usage is not tracked." : "The engine listed no models."}</div>}>
              <div class="surface"><div class="gt">
                <div class="gr omrow th"><span>Model</span><span>Tiers</span><span>Effort</span><span>On</span><span>Auto</span><span>Preferred</span><span></span></div>
                <For each={shown(e.id)}>{x => {
                  const p = () => policyOf(v(), e.id, x.id), tierPath = () => `models.${e.id}.tiers.${x.id}`;
                  const prefs = () => p().preferredFor?.length ?? 0, hasError = () => d.problems().some(q => q.path === tierPath());
                  const isSel = () => picked()?.engine === e.id && picked()?.model === x.id;
                  return <div class="gr omrow ws-pointer" role="row" aria-selected={isSel() ? "true" : "false"} onClick={() => setPicked({ engine: e.id, model: x.id })}>
                    <span class="trunc"><button class="ws-cellbtn w500 trunc" onClick={ev => { ev.stopPropagation(); setPicked({ engine: e.id, model: x.id }); }}>{x.label}</button><span class="sub mono trunc">{x.id}</span>
                      <Show when={!x.observed}><span class="sub st-warn">Not confirmed by the engine yet</span></Show>
                      <Show when={hasError()}><span class="ws-err" data-problem="">Choose a tier.</span></Show></span>
                    <span><Tiers label={x.label} on={rules().tiers[x.id] ?? []} disabled={off()} onChange={(t, on) => d.set(setTier(v(), e.id, x.id, t, on))} /></span>
                    <span class="cap muted tnum trunc">{effortRange(p())}</span>
                    <span onClick={ev => ev.stopPropagation()}><Sw label={`${x.label} enabled`} checked={rules().enabled.includes(x.id)} disabled={off()} onChange={on => d.set(setModelEnabled(v(), e.id, x.id, on))} /></span>
                    <span onClick={ev => ev.stopPropagation()}><Sw label={`${x.label} in automatic routing`} checked={autoOn(v(), e.id, x.id)} disabled={off()} onChange={on => d.set(setModelAuto(v(), e.id, x.id, on))} /></span>
                    <span class="cap muted tnum">{prefs() ? `${prefs()} kind${prefs() === 1 ? "" : "s"}` : ""}</span>
                    <I n="chevron-right" s={14} class="faint" />
                  </div>;
                }}</For>
              </div></div>
              <div class="row ws-gap4"><button class="btn btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) bulk(e.id, true); }}>Turn on all shown</button><button class="btn btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) bulk(e.id, false); }}>Turn off all shown</button></div>
            </Show>
            <Show when={gone().length > 0}>
              <div class="set-h">Not offered now</div>
              <div class="surface"><For each={gone()}>{id => <div class="srow"><span><span class="t mono">{id}</span><span class="d">Enabled here, but {e.label} does not list it. Nothing is inferred.</span></span><span class="ctl"><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} aria-label={`Remove ${id}`} onClick={() => { if (!off()) d.set(setModelEnabled(v(), e.id, id, false)); }}>Remove</button></span></div>}</For></div>
            </Show>
          </>;
        }}</For>
      </div>
      <div class="ws-split-r">
        <Show when={selected()} keyed fallback={<div class="surface ws-empty">Pick a model to see its policy.</div>}>
          {s => <Detail engine={s.engine} model={s.entry} rules={v()} set={r => d.set(r)} off={off()} problems={d.problems()} back={() => setPicked(null)} />}
        </Show>
      </div>
    </div>
  </Shell>;
}
