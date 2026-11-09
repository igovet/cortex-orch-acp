import { createMemo, createSignal, For, Show } from "solid-js";
import { EFFORT_BY_TIER, effortLabel, FALLBACK_ORDER, kindLabel, previewContext, previewGrid, previewRoute, RANK_ORDER, ROLE_CATALOG, TIER_LABELS, TIERS, WORK_KINDS, type Complexity, type RoutingRules } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { I } from "./kit";
import { Bar, Shell, SourceStrips, useRouting } from "./orch-shared";
import { Adv, Lock, Pick, Seg, Srow } from "./settings-kit";

const COMPLEXITIES = [{ id: "low", label: "Low" }, { id: "normal", label: "Normal" }, { id: "high", label: "High" }] as const;
/** Answers "what would run this?" from the rules alone, for saved rules or the draft before it is saved. */
export function RoutePreview() {
  const ws = useWs();
  const d = useRouting();
  const m = () => ws.machine(), scope = () => ws.settings.scope();
  const [role, setRole] = createSignal("worker-backend-dev");
  const [kind, setKind] = createSignal("implementation_backend");
  const [complexity, setComplexity] = createSignal<Complexity>("normal");
  const [attempt, setAttempt] = createSignal<"first" | "escalated">("first");
  const [using, setUsing] = createSignal<"saved" | "draft">("saved");
  const [view, setView] = createSignal<"one" | "all">("one");
  const rules = (): RoutingRules | undefined => d.dirty() && using() === "draft" ? d.value() : undefined;
  const ctx = createMemo(() => previewContext(m(), scope(), rules()));
  const result = createMemo(() => previewRoute({ role: role(), kind: kind(), complexity: complexity(), attempt: attempt() }, ctx()));
  const grid = createMemo(() => view() === "all" ? previewGrid(attempt(), ctx()) : []);
  const pickRole = (id: string) => { setRole(id); setKind(ROLE_CATALOG.find(r => r.id === id)?.kind ?? kind()); };
  const open = (id: string, c: Complexity) => { pickRole(id); setComplexity(c); setView("one"); };
  return <Shell scoped wide title="Route preview" lead="What would run this work, and why the other routes are out. Read-only." bar={<Bar d={d} />}>
    <SourceStrips />
    <div class="surface ws-pad ws-pv-in">
      <span class="row ws-gap4"><span class="cap muted">Role</span><Pick id="m-pv-role" label="Role" width="170px" value={role()} options={ROLE_CATALOG.map(r => ({ id: r.id, label: r.label }))} onPick={pickRole} /></span>
      <span class="row ws-gap4"><span class="cap muted">Work kind</span><Pick id="m-pv-kind" label="Work kind" width="200px" value={kind()} options={WORK_KINDS.map(k => ({ id: k.id, label: k.label }))} onPick={setKind} /></span>
      <span class="row ws-gap4"><span class="cap muted">Complexity</span><Seg label="Complexity" value={complexity()} options={COMPLEXITIES} onChange={setComplexity} /></span>
      <span class="row ws-gap4"><span class="cap muted">Attempt</span><Seg label="Attempt" value={attempt()} options={[{ id: "first", label: "First" }, { id: "escalated", label: "Escalated" }] as const} onChange={setAttempt} /></span>
      <Show when={d.dirty()}><span class="row ws-gap4"><span class="cap muted">Using</span><Seg label="Using" value={using()} options={[{ id: "saved", label: "Saved" }, { id: "draft", label: "Draft" }] as const} onChange={setUsing} /></span></Show>
      <span class="row ws-gap4 ws-ml-auto"><Seg label="View" value={view()} options={[{ id: "one", label: "This work" }, { id: "all", label: "All roles" }] as const} onChange={setView} /></span>
    </div>
    <Show when={view() === "one"} fallback={
      <div class="surface"><div class="gt">
        <div class="gr ogrow th"><span>Role</span><span>Low</span><span>Normal</span><span>High</span></div>
        <For each={grid()}>{r => <div class="gr ogrow"><span class="w500 trunc">{r.label}</span><For each={r.cells}>{c => <button class={`ws-cellbtn cap trunc ${c.blocked ? "st-bad" : ""}`} aria-label={`Fill the inputs: ${r.label}, ${c.complexity}: ${c.text}`} onClick={() => open(r.role, c.complexity)}>{c.text}</button>}</For></div>}</For>
      </div></div>
    }>
      <Show when={result().blocked}>{b => <div class="note-box bad" role="status"><I n="x-circle" s={14} /><span><b class="mono">{b().code}</b>: {b().text}</span></div>}</Show>
      <Show when={!result().blocked}>
        <div class="surface">
          <div class="srow"><span><span class="t">Tier path</span><span class="d">{TIER_LABELS[result().baseTier]} role, {complexity()} complexity, {kindLabel(kind())}</span></span><span class="ctl"><span>{TIER_LABELS[result().tier]}</span><Show when={result().served && result().served !== result().tier}><span class="cap muted">served at {TIER_LABELS[result().served!]}</span></Show></span></div>
          <div class="srow"><span><span class="t">Would run</span><span class="d">{result().rows[0]?.decidedBy}</span></span><span class="ctl"><span class="w500">{result().rows[0]?.label}</span><span>{effortLabel(result().rows[0]?.effort ?? "")}</span></span></div>
        </div>
        <div class="surface"><div class="gt">
          <div class="gr oprow th"><span>#</span><span>Route</span><span>Tier</span><span>Effort</span><span>Usage</span><span>Decided by</span></div>
          <For each={result().rows}>{(r, i) => <div class="gr oprow"><span class="tnum muted">{i() + 1}</span><span class="w500 trunc">{r.label}</span><span class="cap">{TIER_LABELS[r.tier]}</span><span class="cap">{effortLabel(r.effort)}</span><span class="cap tnum">{r.usage === null ? "" : `${r.usage}%`}</span><span class="cap muted trunc">{r.decidedBy}</span></div>}</For>
        </div></div>
      </Show>
      <Show when={result().excluded.length > 0}>
        <div class="set-h">Excluded<span class="cap faint ws-w400 tnum">{result().excluded.length}</span></div>
        <div class="surface"><div class="gt"><For each={result().excluded}>{x => <div class="gr oxrow"><span class="trunc">{x.label}</span><span class="cap muted trunc">{x.reason}</span></div>}</For></div></div>
      </Show>
    </Show>
    <Adv id="preview-rules" label="How the router decides">
      <div class="surface">
        <Srow t="Tier fallback order" d={TIERS.map(t => `${TIER_LABELS[t]}: ${FALLBACK_ORDER[t].join(", ")}`).join(". ")}><Lock /></Srow>
        <Srow t="Effort by tier" d={TIERS.map(t => `${TIER_LABELS[t]}: ${EFFORT_BY_TIER[t]}`).join(", ")}><Lock /></Srow>
        <Srow t="Rank order" d={RANK_ORDER.join(", ")}><Lock /></Srow>
      </div>
    </Adv>
    <p class="hint">Live availability, retries and plan hints are only known at dispatch. Usage is a snapshot.{ws.t("previewNote")}</p>
  </Shell>;
}
