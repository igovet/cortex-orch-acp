import { createMemo, For, Show } from "solid-js";
import { addCoordinatorModel, coordinatorSeed, denyTokens, EFFORT_LEVELS, effortLabel, engineLabel, ENGINE_KEYS, modelLabel, nearest, orderedEfforts, removeCoordinatorModel, setCoordinatorTier, startCoordinatorList, TIERS, type CoordinatorModel, type EffortRange, type Tier } from "@cortex/workspace-domain";
import { workspaceCopy } from "./copy";
import { useWs } from "./ctx";
import { A, I } from "./kit";
import { HELP } from "./orch-help";
import { Bar, orchTo, Shell, slug, SourceStrips, useRouting, useRoutingOff } from "./orch-shared";
import { Chk, Err, Hint, Pick, SetH } from "./settings-kit";

/** The coordinator's names for the tiers. Frontier is the flagship tier of worker routing. */
export const CATEGORY_LABELS: Record<Tier, string> = { flagship: "Frontier", deep: "Deep", standard: "Standard", light: "Light" };
/** Where sprint planning starts each category's effort before moving it into the range. */
const START: Record<Tier, string> = { flagship: "high", deep: "high", standard: "medium", light: "low" };

/**
 * Section 2: the models the coordinator may use, apart from the worker tiers. Each listed model coordinates in the categories
 * ticked here, at the efforts of their ranges; the pickers for a task's coordinator offer only these models and efforts, and
 * sprint planning assigns each task a model from this list by its complexity and the providers' limits.
 */
export function CoordinatorModels() {
  const ws = useWs();
  const d = useRouting(), off = useRoutingOff();
  const v = () => d.value(), m = () => ws.machine(), cat = () => m().config.catalogue;
  const list = () => v().coordinatorModels;
  const levelsOf = (c: CoordinatorModel) => orderedEfforts(cat()[c.engine].models.find(x => x.id === c.model)?.efforts ?? []).filter(x => (EFFORT_LEVELS as readonly string[]).includes(x));
  /** What keeps a listed model from coordinating now. It stays listed, so turning it on again brings it back. */
  const status = (c: CoordinatorModel) => denyTokens(v(), c.engine, c.model).length ? "Refused by a deny rule" : !v().engines[c.engine].enabled.includes(c.model) ? "Off in Models: not offered to the coordinator" : !cat()[c.engine].models.some(x => x.id === c.model) ? "Not offered by the engine now" : null;
  const addable = createMemo(() => ENGINE_KEYS.flatMap(e => cat()[e].models.filter(x => v().engines[e].enabled.includes(x.id) && !denyTokens(v(), e, x.id).length && !list()?.some(c => c.engine === e && c.model === x.id)).map(x => ({ id: `${e}|${x.id}`, label: x.label, sub: engineLabel(e) }))));
  const add = (value: string) => { const [engine, model] = value.split("|") as [CoordinatorModel["engine"], string]; d.set(addCoordinatorModel(v(), cat(), engine, model)); };
  const seed = () => coordinatorSeed(v(), cat());
  return <Shell section="coordinator" wide title={workspaceCopy("coordinatorModels")} lead={workspaceCopy("coordinatorModelsLead")} bar={<Bar d={d} />}
    head={<A class="btn btn-line btn-sm" to={orchTo(m().id, "defaults")}><I n="chevron-right" s={12} />Coordinator default</A>}>
    <SourceStrips />
    <Show when={list()} fallback={
      <section class="set-grp" aria-labelledby="set-cm-legacy-h">
        <SetH title={<span id="set-cm-legacy-h">{workspaceCopy("coordinatorModelsNone")}</span>}><Hint text={HELP.coordinatorModels} label="coordinator models" /></SetH>
        <div class="note-box plain"><I n="alert" s={14} /><span>{workspaceCopy("coordinatorModelsLegacy")}</span></div>
        <div class="surface"><div class="srow"><span><span class="t">{workspaceCopy("coordinatorModelsStart")}</span><span class="d">{seed().length ? `Starts with ${seed().map(c => modelLabel(cat(), c.engine, c.model)).join(", ")}, at the categories and efforts their tiers allow now. Edit it before you save.` : "Starts an empty list: add the models the coordinator may use."}</span></span>
          <span class="ctl"><button class="btn btn-line btn-sm" aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) d.set(startCoordinatorList(v(), cat())); }}>{workspaceCopy("coordinatorModelsStart")}</button></span></div></div>
      </section>}>
      {models => <>
        <section class="set-grp" aria-labelledby="set-cm-h">
          <SetH title={<span id="set-cm-h">{workspaceCopy("coordinatorModelsListed")}</span>} count={models().length}><Hint text={HELP.coordinatorModels} label="coordinator models" /></SetH>
          <Err problems={d.problems()} path="coordinator_models" />
          <Show when={models().length > 0} fallback={<div class="surface ws-empty">No model is listed, so no task can start. Add one below.</div>}>
            <For each={models()}>{c => {
              const at = () => `coordinator_models.${c.engine}.${c.model}`;
              const label = () => modelLabel(cat(), c.engine, c.model);
              const levels = () => levelsOf(c);
              const choices = (value: string) => [...levels().map(x => ({ id: x, label: effortLabel(x) })), ...(value && !levels().includes(value) ? [{ id: value, label: effortLabel(value), sub: "Not offered by this model" }] : [])];
              const tick = (t: Tier, on: boolean) => {
                if (!on) return d.set(setCoordinatorTier(v(), c.engine, c.model, t, null));
                const start = levels().length ? nearest(START[t], levels()) : START[t];
                d.set(setCoordinatorTier(v(), c.engine, c.model, t, { min: start, max: start }));
              };
              // Moving one end past the other takes the other with it, so a range never turns inside out.
              const pick = (t: Tier, end: "min" | "max", value: string) => {
                const now = c.tiers[t], at = (x?: string) => (EFFORT_LEVELS as readonly string[]).indexOf(x ?? "");
                const next: EffortRange = end === "min" ? { min: value, max: now && at(now.max) >= at(value) ? now.max : value } : { min: now && at(now.min) <= at(value) ? now.min : value, max: value };
                d.set(setCoordinatorTier(v(), c.engine, c.model, t, next));
              };
              return <div class="surface ws-cm" role="group" aria-label={`${label()} as coordinator`}>
                <div class="srow"><span><span class="t">{label()}</span><span class="d"><span class="mono">{c.model}</span>, {engineLabel(c.engine)}<Show when={status(c)}>{s => <span class="ws-block st-warn">{s()}</span>}</Show></span></span>
                  <span class="ctl"><button class="btn btn-line btn-sm" aria-label={`Remove ${label()} from coordinator models`} aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) d.set(removeCoordinatorModel(v(), c.engine, c.model)); }}>Remove</button></span></div>
                <div class="ws-pad"><div class="ws-fld"><span class="lab">Categories and effort</span>
                  <For each={TIERS}>{t => {
                    const range = () => c.tiers[t];
                    const end = (which: "min" | "max", word: string) => <Pick id={`m-cm-${which}-${t}-${slug(`${c.engine}-${c.model}`)}`} label={`${word} effort on ${CATEGORY_LABELS[t]} for ${label()} as coordinator`} width="110px" value={range()?.[which] ?? ""} options={choices(range()?.[which] ?? "")} disabled={off() ?? (range() ? undefined : `Tick ${CATEGORY_LABELS[t]} first`)} onPick={x => pick(t, which, x)} />;
                    return <div class="row ws-gap4"><label class="ws-check ws-range-t"><Chk label={`${label()} coordinates ${CATEGORY_LABELS[t]}`} checked={!!range()} disabled={off()} onChange={on => tick(t, on)} /><span>{CATEGORY_LABELS[t]}</span></label>
                      <Show when={range()}>{end("min", "Lowest")}<span class="cap faint">to</span>{end("max", "Highest")}</Show>
                      <Err problems={d.problems()} path={`${at()}.${t}`} /></div>;
                  }}</For>
                  <Err problems={d.problems()} path={at()} />
                </div></div>
              </div>;
            }}</For>
          </Show>
          <div class="surface"><div class="srow"><span><span class="t">{workspaceCopy("coordinatorModelsAdd")}</span><span class="d">Models switched on under Models. A new one starts on Standard; tick its categories and set each range.</span></span>
            <span class="ctl"><Pick id="m-cm-add" label={workspaceCopy("coordinatorModelsAdd")} width="220px" value="" empty={addable().length ? "Choose a model" : "Every model is listed"} options={addable()} disabled={off() ?? (addable().length ? undefined : "Every switched-on model is listed")} onPick={add} /></span></div></div>
        </section>
        <section class="set-grp" aria-labelledby="set-cm-sprint-h">
          <SetH title={<span id="set-cm-sprint-h">{workspaceCopy("coordinatorModelsSprint")}</span>} />
          <div class="surface ws-pad ws-legend">
            <p>Low complexity: Light, then Standard. Normal: Standard, then Deep, then Light. High: Frontier, then Deep.</p>
            <p>Among the models of a category, a provider whose limit is used up is skipped, one above 95% goes last, then the least used goes first. The effort starts at the category's usual level (Light low, Standard medium, Deep and Frontier high) and moves into the model's range.</p>
          </div>
        </section>
      </>}
    </Show>
  </Shell>;
}
