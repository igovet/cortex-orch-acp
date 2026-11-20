import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { ACCESS_LABELS, EFFORT_BY_TIER, effortLabel, engineLabel, ENGINE_KEYS, ENGINES, FALLBACK_ORDER, HOT_PERCENT, kindLabel, modelLabel, orphanTokens, policyOf, projectsWithRouting, RANK_ORDER, RESERVED_ROLE, ROLE_CATALOG, routingSource, setAffinity, setDeny, setPin, setPreferred, setStretch, TIER_LABELS, TIERS, tierFor, unmappedWindows, WORK_KINDS, type EngineKey, type QuotaWindow, type RoutePin, type RoutingRules } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I, MenuItem, Pop } from "./kit";
import { HELP } from "./orch-help";
import { ModelPolicyFields } from "./orch-models";
import { Bar, orchTo, Shell, slug, SourceStrips, useProjectName, useRouting, useRoutingOff } from "./orch-shared";
import { Adv, Err, Hint, Lock, Pick, Rec, Seg, SetH, Srow, Tags, tierWords } from "./settings-kit";
import { share } from "./stable";

type Ctx = { rules: () => RoutingRules; set: (r: RoutingRules) => void; off: () => string | undefined; problems: () => { path: string; message: string }[] };
const pinText = (p: RoutePin, label: string) => [engineLabel(p.engine), label, p.effort ? effortLabel(p.effort) : null].filter(Boolean).join(" · ");

function PinEditor(props: { role: string; label: string; c: Ctx; done: () => void }) {
  const ws = useWs();
  const r = () => props.c.rules(), pin = () => r().pins[props.role];
  const cat = () => ws.machine().config.catalogue;
  const on = (e: EngineKey) => r().engines[e].enabled;
  const first = () => ENGINE_KEYS.find(e => on(e).length) ?? "claude-agent-acp";
  const cur = (): RoutePin => pin() ?? { engine: first(), model: on(first())[0] ?? "", effort: null };
  const set = (p: RoutePin) => props.c.set(setPin(r(), props.role, p));
  const efforts = () => cat()[cur().engine].models.find(m => m.id === cur().model)?.efforts ?? [];
  const key = slug(props.role);
  return <div class="ws-pin">
    <div class="row ws-gap4">
      <Pick id={`m-pin-e-${key}`} label={`${props.label} pin engine`} width="110px" value={cur().engine} disabled={props.c.off()} options={ENGINES.filter(e => on(e.id).length).map(e => ({ id: e.id, label: e.label }))} onPick={engine => set({ engine: engine as EngineKey, model: on(engine as EngineKey)[0], effort: null })} />
      <Pick id={`m-pin-m-${key}`} label={`${props.label} pin model`} width="170px" value={cur().model} disabled={props.c.off()} options={on(cur().engine).map(id => ({ id, label: modelLabel(cat(), cur().engine, id), sub: id }))} onPick={model => set({ ...cur(), model, effort: null })} />
      <Pick id={`m-pin-f-${key}`} label={`${props.label} pin effort`} width="130px" value={cur().effort ?? ""} disabled={props.c.off()} options={[{ id: "", label: "Engine default" }, ...efforts().map(e => ({ id: e, label: effortLabel(e) }))]} onPick={effort => set({ ...cur(), effort: effort || null })} />
      <span class="ws-push"></span>
      <Show when={pin()}><button class="btn btn-sm" aria-disabled={props.c.off() ? "true" : undefined} data-why={props.c.off()} onClick={() => { if (!props.c.off()) { props.c.set(setPin(r(), props.role, null)); props.done(); } }}>Remove pin</button></Show>
      <button class="btn btn-line btn-sm" onClick={() => { if (!pin() && !props.c.off()) set(cur()); props.done(); }}>Done</button>
    </div>
    <div class="cap st-warn">Strict. If this route is unavailable the work is refused, not moved.</div>
  </div>;
}

/** The built-in ladder in words: the tier at low, normal and high complexity, from the same rule the router uses. */
export function ladderWords(role: string) {
  const at = (["low", "normal", "high"] as const).map(c => TIER_LABELS[tierFor(role, false, c)]);
  return at.every(t => t === at[0]) ? `${at[0]} at every complexity` : `Low: ${at[0]}, normal: ${at[1]}, high: ${at[2]}`;
}
function Roles(props: { c: Ctx }) {
  const ws = useWs();
  const [open, setOpen] = createSignal<string | null>(null);
  const row = (id: string, name: string, sub: string, kind: string | null, tier: string | null) => {
    const pin = () => props.c.rules().pins[id], reserved = id === RESERVED_ROLE;
    return <>
      <div class="gr orrow">
        <span class="trunc"><span class="w500">{name}</span><span class="sub trunc">{sub}</span></span>
        <span class="cap muted trunc">{kind ?? ""}</span>
        <span class="cap muted ws-wrap">{tier ?? ""}</span>
        <span class="trunc"><Show when={!reserved} fallback={<span class="cap muted">Built in</span>}><span class={pin() ? "cap" : "cap muted"}>{pin() ? pinText(pin()!, modelLabel(ws.machine().config.catalogue, pin()!.engine, pin()!.model)) : "Automatic"}</span></Show>
          <Err problems={props.c.problems()} path={`pin.${id}`} /></span>
        <Show when={!reserved} fallback={<span></span>}><button class="btn btn-line btn-sm" aria-label={`${pin() ? "Edit pin" : "Pin"}: ${name}`} aria-expanded={open() === id} aria-disabled={props.c.off() ? "true" : undefined} data-why={props.c.off()} onClick={() => { if (!props.c.off()) setOpen(open() === id ? null : id); }}>{pin() ? "Edit pin" : "Pin…"}</button></Show>
      </div>
      <Show when={open() === id}><PinEditor role={id} label={name} c={props.c} done={() => setOpen(null)} /></Show>
    </>;
  };
  const group = (title: string, access: "writing" | "read_only" | "consultant") => <>
    <div class="gr orrow th ws-group"><span>{title}</span><span></span><span></span><span></span><span></span></div>
    <For each={ROLE_CATALOG.filter(r => r.access === access)}>{r => {
      return row(r.id, r.label, ACCESS_LABELS[r.access], kindLabel(r.kind), ladderWords(r.id));
    }}</For>
  </>;
  return <>
    <div class="surface"><div class="gt">
      <div class="gr orrow th"><span>Role<Hint text={HELP.pins} label="role pins" /></span><span class="row ws-gap4">Work kind<Lock text="" /></span><span class="row ws-gap4">Tier<Lock text="" /></span><span>Pin</span><span></span></div>
      {row("*", "All roles", "Pin for every role without its own", null, null)}
      {group("Writing", "writing")}{group("Read-only", "read_only")}{group("Consultant", "consultant")}
    </div></div>
    <div class="note-box plain"><I n="lock" s={14} /><span>Flagship is reached only by escalation, a confirmed request of yours, or a pin. Ordinary work tops out at deep, and low complexity moves a role one tier lighter.</span></div>
  </>;
}

function WorkKinds(props: { c: Ctx }) {
  const ws = useWs();
  const r = () => props.c.rules(), cat = () => ws.machine().config.catalogue;
  // One list for every reader, held as the old objects while it reads the same: the chips and the open Add menu are not drawn
  // again at each machine read, and `preferred().includes(x)` compares the same objects.
  const models = createMemo<{ engine: EngineKey; id: string; label: string }[]>(prev => share(prev, ENGINE_KEYS.flatMap(e => r().engines[e].enabled.map(id => ({ engine: e, id, label: modelLabel(cat(), e, id) })))), []);
  const lean = (kind: string, builtIn: EngineKey | null) => <Pick id={`m-lean-${slug(kind)}`} label={`Lean to engine for ${kind === "*" ? "all kinds" : kindLabel(kind)}`} width="min(160px, 100%)" value={kind in r().affinity ? r().affinity[kind] ?? "none" : "builtin"} disabled={props.c.off()}
    options={[{ id: "builtin", label: builtIn ? `Built in (${engineLabel(builtIn)})` : "Built in (no lean)" }, ...ENGINES.map(e => ({ id: e.id as string, label: e.label })), { id: "none", label: "No lean" }]} onPick={x => props.c.set(setAffinity(r(), kind, x === "builtin" ? "builtin" : x === "none" ? null : x as EngineKey))} />;
  return <>
    <div class="surface"><div class="gt">
      <div class="gr okrow th"><span>Work kind</span><span>Lean to engine<Hint text={HELP.affinity} label="lean to engine" /></span><span>Preferred models</span><span>Light models also standard</span></div>
      <div class="gr okrow"><span class="w500">All kinds</span><span>{lean("*", null)}</span><span></span><span></span></div>
      <For each={WORK_KINDS}>{k => {
        const roles = () => ROLE_CATALOG.filter(x => x.kind === k.id);
        const preferred = () => models().filter(x => policyOf(r(), x.engine, x.id).preferredFor?.includes(k.id));
        const lights = () => models().filter(x => r().engines[x.engine].tiers[x.id]?.includes("light"));
        const key = slug(k.id);
        return <div class="gr okrow">
          <span class="trunc"><span class="w500">{k.label}</span><span class="sub">{roles().length} role{roles().length === 1 ? "" : "s"}</span>
            <Show when={k.bound}><span class="chip ws-static ws-mt4">Requires its engine</span></Show></span>
          <span>{lean(k.id, k.lean)}</span>
          <span class="ws-tags">
            <For each={preferred()}>{x => <span class="chip ws-static">{x.label}<Show when={!props.c.off()}><button class="ws-att-x" aria-label={`Remove ${x.label} from ${k.label}`} onClick={() => props.c.set(setPreferred(r(), x.engine, x.id, k.id, false))}><I n="x" s={12} /></button></Show></span>}</For>
            <Show when={!props.c.off()}><button class="chip" popovertarget={`m-pref-${key}`} aria-label={`Add a preferred model for ${k.label}`}><I n="plus" s={12} />Add</button>
              <Pop id={`m-pref-${key}`} class="pop-right" align="right" label={`Preferred for ${k.label}`}>
                <For each={models().filter(x => !preferred().includes(x))}>{x => <MenuItem label={x.label} sub={x.id} disabled={r().engines[x.engine].auto.includes(x.id) ? undefined : "Turn on Auto first."} onSelect={() => props.c.set(setPreferred(r(), x.engine, x.id, k.id, true))} />}</For>
              </Pop></Show>
          </span>
          <span class="ws-tags"><For each={lights()}>{x => {
            const on = () => !!policyOf(r(), x.engine, x.id).alsoStandardFor?.includes(k.id);
            return <button class="chip" aria-pressed={on()} aria-label={`${x.label} stretches to ${k.label}`} aria-disabled={props.c.off() ? "true" : undefined} data-why={props.c.off()} onClick={() => { if (!props.c.off()) props.c.set(setStretch(r(), x.engine, x.id, k.id, !on())); }}><Show when={on()}><I n="check" s={12} /></Show>{x.label}</button>;
          }}</For></span>
        </div>;
      }}</For>
    </div></div>
    <div class="note-box plain"><I n="alert" s={14} /><span>Preferred models are a set: their order is not a priority.</span></div>
  </>;
}

function Quota(props: { c: Ctx }) {
  const ws = useWs();
  const r = () => props.c.rules(), m = () => ws.machine();
  const edit = (e: EngineKey, fn: (q: QuotaWindow[]) => QuotaWindow[]) => { const next = structuredClone(r()); next.engines[e].quota = fn(next.engines[e].quota); props.c.set(next); };
  const percent = (e: EngineKey, w: string) => m().config.quotaObserved[e].find(x => x.window === w)?.percent;
  return <>
    <div class="note-box plain"><I n="lock" s={14} /><span>Hot at {HOT_PERCENT}%: ranked behind cooler routes. 100%: excluded. There is no setting for this.</span></div>
    <For each={ENGINES.filter(e => e.usage)}>{e => {
      const rows = () => r().engines[e.id].quota, unmapped = createMemo<ReturnType<typeof unmappedWindows>>(prev => share(prev, unmappedWindows(m(), e.id, r())), []);
      return <>
        <SetH title={<>{e.label}</>}><Hint text={HELP.quotaWindows} label="quota windows" /><button class="btn btn-line btn-sm" aria-disabled={props.c.off() ? "true" : undefined} data-why={props.c.off()} onClick={() => { if (!props.c.off()) edit(e.id, q => [...q, { window: "", models: null }]); }}><I n="plus" s={12} />Add window</button></SetH>
        <Show when={rows().length > 0} fallback={<div class="surface ws-empty">No windows mapped. Every reported window counts for every model.</div>}>
          <div class="surface"><div class="gt">
            <div class="gr oqrow th"><span>Window</span><span>Applies to</span><span>Used now</span><span></span></div>
            <For each={rows()}>{(q, i) => {
              const mode = () => q.models === null ? "all" : q.models.length ? "exact" : "none", used = () => percent(e.id, q.window);
              return <div class="gr oqrow">
                <span><input class="inp mono" aria-label={`${e.label} window ${i() + 1}`} value={q.window} disabled={!!props.c.off()} onChange={ev => { const window = ev.currentTarget.value; edit(e.id, list => list.map((x, n) => n === i() ? { ...x, window } : x)); }} /><Err problems={props.c.problems()} path={`quota_windows.${e.id}[${i()}].window`} /></span>
                <span class="ws-quota"><Seg label={`${q.window} applies to`} value={mode()} disabled={props.c.off()} options={[{ id: "all", label: "All models" }, { id: "exact", label: "Exact models" }, { id: "none", label: "No models" }] as const} onChange={x => edit(e.id, list => list.map((w, n) => n === i() ? { ...w, models: x === "all" ? null : x === "none" ? [] : (w.models?.length ? w.models : r().engines[e.id].enabled.slice(0, 1)) } : w))} />
                  <Show when={mode() === "exact"}><Tags label={`${q.window} models`} mono items={q.models ?? []} disabled={props.c.off()} onChange={items => edit(e.id, list => list.map((w, n) => n === i() ? { ...w, models: items } : w))} /></Show></span>
                <span class="u-used"><Show when={used() !== undefined}><span class="bar"><i style={{ width: `${used()}%` }}></i></span><span class="tnum">{used()}%</span></Show></span>
                <button class="ib ws-ib" aria-label={`Remove window ${q.window}`} aria-disabled={props.c.off() ? "true" : undefined} data-why={props.c.off()} onClick={() => { if (!props.c.off()) edit(e.id, list => list.filter((_, n) => n !== i())); }}><I n="x" s={14} /></button>
              </div>;
            }}</For>
          </div></div>
        </Show>
        <Show when={unmapped().length > 0}>
          <SetH title="Reported, not mapped" />
          <div class="surface"><For each={unmapped()}>{w => <div class="srow"><span><span class="t mono">{w.window}</span><span class="d">Counts for every model of this engine. {w.percent}% used.</span></span><span class="ctl"><button class="btn btn-line btn-sm" aria-disabled={props.c.off() ? "true" : undefined} data-why={props.c.off()} onClick={() => { if (!props.c.off()) edit(e.id, q => [...q, { window: w.window, models: null }]); }}>Map…</button></span></div>}</For></div>
        </Show>
      </>;
    }}</For>
  </>;
}

const TOKEN = /^([^\s/*]+|(claude-agent-acp|codex-acp|opencode)\/([^\s*]+|\*))$/;
function Rules(props: { c: Ctx }) {
  const ws = useWs();
  const r = () => props.c.rules(), m = () => ws.machine();
  const orphans = createMemo(() => orphanTokens(r(), m().config.catalogue));
  const [bad, setBad] = createSignal<string | null>(null);
  const setTokens = (deny: string[]) => { const wrong = deny.find(t => !TOKEN.test(t)); setBad(wrong ? `"${wrong}": use a model id, engine/model or engine/*. A wildcard needs an engine.` : null); if (!wrong) props.c.set(setDeny(r(), deny)); };
  return <>
    <div class="surface ws-pad"><Tags label="Deny token" mono hint="model, engine/model or engine/*" items={r().deny} disabled={props.c.off()} onChange={setTokens} /><Show when={bad()}><span class="ws-err" data-problem="">{bad()}</span></Show></div>
    <Show when={orphans().length > 0}>
      <SetH title="Other entries" />
      <div class="surface"><For each={orphans()}>{t => <div class="srow"><span><span class="t mono">{t}</span><span class="d">Matches no model this machine knows.</span></span><span class="ctl"><button class="btn btn-line btn-sm" aria-label={`Remove ${t}`} aria-disabled={props.c.off() ? "true" : undefined} data-why={props.c.off()} onClick={() => { if (!props.c.off()) props.c.set(setDeny(r(), r().deny.filter(x => x !== t))); }}>Remove</button></span></div>}</For></div>
    </Show>
    <Show when={r().autoAllow.length > 0}>
      <SetH title="Auto-allow"><Hint text={HELP.autoAllow} label="auto-allow" /></SetH>
      <div class="surface ws-pad"><p class="cap muted">Exact routes that automatic routing may use although they are left out by default, such as OpenCode routes. Each OpenCode model also has its own switch under Engines and models.</p><Tags label="Auto-allow token" mono items={r().autoAllow} disabled={props.c.off()} onChange={autoAllow => props.c.set({ ...r(), autoAllow })} /></div>
    </Show>
    <Show when={r().legacyTiers.length > 0}>
      <SetH title="Legacy tiers"><Hint text={HELP.tiers} label="legacy tiers" /></SetH>
      <div class="surface"><div class="srow"><span><span class="t mono">{r().legacyTiers.join(", ")}</span><span class="d">Refused under model policy 2. Migrate to move them into the per-model tiers.</span></span><span class="ctl"><A class="btn btn-line btn-sm" to={orchTo(m().id, "files")}>Migrate…</A></span></div></div>
    </Show>
  </>;
}

const SOURCE_WORDS = { project: "Project routing file", environment: "Environment routing file", machine: "User routing file" } as const;
/** Routing › Routing files: which file is in use and where a save goes, read-only. */
function RoutingFiles() {
  const ws = useWs(), name = useProjectName();
  const m = () => ws.machine(), scope = () => ws.settings.scope(), r = () => m().config.surfaces.routing;
  const source = () => routingSource(m(), scope());
  const saving = () => scope() ? `${name(scope())}'s own routing file` : source().kind === "environment" ? "Nothing: the environment file is read-only here" : "Your user routing file";
  return <div class="surface">
    <Srow t={<>Rules in use<Hint text={HELP.routingFile} label="routing files" /></>} d={`${SOURCE_WORDS[source().kind]}${m().config.files.invalidRouting && source().kind !== "project" ? " (invalid)" : ""}${!scope() && projectsWithRouting(m()).length ? `. Projects with their own file: ${projectsWithRouting(m()).map(id => name(id)).join(", ")}` : ""}`}><Lock text="Read-only" /></Srow>
    <Srow t="Precedence" d="Project, then environment, then user. Files never merge."><Lock /></Srow>
    <Srow t="Model policy version" d={r().version === 2 && !m().config.files.migration ? "2, the current format." : `${r().version}: it needs migration before work can run.`}><Show when={r().version !== 2 || m().config.files.migration}><A class="btn btn-line btn-sm" to={orchTo(m().id, "files")}>Migrate…</A></Show></Srow>
    <Srow t="Saving to" d={saving()} />
  </div>;
}
/** Routing › Model policy: one closed block per model, grouped by engine. A deep link opens and scrolls to one. */
function ModelPolicies(props: { c: Ctx }) {
  const ws = useWs();
  const cat = () => ws.machine().config.catalogue;
  return <For each={ENGINES.filter(e => cat()[e.id].models.length > 0)}>{e => <div class="ws-pol-eng">
    <h3 class="set-sub">{e.label}</h3>
    <For each={cat()[e.id].models}>{x => {
      const key = `${e.id}:${x.id}`, tiers = () => props.c.rules().engines[e.id].tiers[x.id] ?? [];
      const errors = () => props.c.problems().some(p => p.path.startsWith(`models.${e.id}.policy.${x.id}.`) || p.path === `models.${e.id}.tiers.${x.id}`);
      return <div class="surface ws-pol" id={`set-policy-${slug(key)}`}>
        <Adv id={`policy-${key}`} label={`${x.label}${tiers().length ? `: ${tierWords(tiers())}` : ": no tier"}`} force={errors()}>
          <ModelPolicyFields engine={e.id} model={x} rules={props.c.rules()} set={props.c.set} off={props.c.off()} problems={props.c.problems()} />
        </Adv>
      </div>;
    }}</For>
  </div>}</For>;
}
/** Locked rows: what the router always does. Route preview shows the same. */
export function RouterRules() {
  return <div class="surface">
    <Srow t="Tier fallback order" d={[...TIERS].reverse().map(t => `${TIER_LABELS[t]}: ${FALLBACK_ORDER[t].map(x => TIER_LABELS[x as keyof typeof TIER_LABELS] ?? x).join(", ")}`).join(". ")}><Lock /></Srow>
    <Srow t="Effort by tier" d={[...TIERS].reverse().map(t => `${TIER_LABELS[t]}: ${effortLabel(EFFORT_BY_TIER[t])}`).join(", ")}><Lock /></Srow>
    <Srow t="Rank order" d={RANK_ORDER.join(", ")}><Lock /></Srow>
    <Srow t="Hot band" d={`At ${HOT_PERCENT}% of a quota window a route ranks behind cooler ones; at 100% it is left out.`}><Lock /></Srow>
    <Srow t="Flagship" d="Reached only by escalation, a confirmed request of yours, or a pin. Ordinary work tops out at deep, and low complexity moves a role one tier lighter."><Lock /></Srow>
  </div>;
}

/** Old tab links land on the group they named. */
const GROUP_OF: Record<string, string> = { roles: "set-roles", kinds: "set-kinds", quota: "set-quota", rules: "set-deny", policy: "set-policy" };
/** Section 5: who runs what, in the order of the Cortex settings panel. */
export function Routing(props: { tab?: string }) {
  const ws = useWs();
  const d = useRouting(), off = useRoutingOff();
  const c: Ctx = { rules: () => d.value(), set: r => d.set(r), off, problems: () => d.problems() };
  const m = () => ws.machine(), r = () => d.value();
  const focus = () => props.tab?.startsWith("policy:") ? props.tab.slice(7) : null;
  const windows = () => ENGINES.some(e => e.usage && (r().engines[e.id].quota.length > 0 || unmappedWindows(m(), e.id, r()).length > 0));
  onMount(() => {
    const key = focus(), id = key ? `set-policy-${slug(key)}` : props.tab ? GROUP_OF[props.tab] : undefined;
    if (key) ws.panels.setAdvanced(`policy-${key}`, true);
    if (id) queueMicrotask(() => document.getElementById(id)?.scrollIntoView?.({ block: "start" }));
  });
  const Group = (p: { id: string; title: string; hint?: [string, string]; children: import("solid-js").JSX.Element }) => <section class="set-grp" id={p.id} aria-labelledby={`${p.id}-h`}>
    <SetH title={<span id={`${p.id}-h`}>{p.title}</span>}><Show when={p.hint}>{h => <Hint text={h()[0]} label={h()[1]} />}</Show></SetH>
    {p.children}
  </section>;
  return <Shell section="routing" wide title="Routing" lead="Who runs what. Pins are strict; leans and preferences only rank what is eligible." bar={<Bar d={d} />}
    head={<div class="row ws-gap4 ws-head-acts"><A class="btn btn-line btn-sm" to={orchTo(m().id, "effective")}><I n="eye" s={12} />Preview routes</A><A class="btn btn-line btn-sm" to={orchTo(m().id, "files")}><I n="file" s={12} />Files and sources</A></div>}>
    <SourceStrips />
    <Group id="set-files" title="Routing files"><RoutingFiles /></Group>
    <Group id="set-policy" title="Model policy" hint={[HELP.modelPolicy, "model policy"]}>
      <Rec>Most owners only need Engines and models; set these for models that work badly at low effort or that you call by a short name.</Rec>
      <Show when={ENGINES.some(e => m().config.catalogue[e.id].models.length > 0)} fallback={<div class="surface ws-empty">No models known yet: refresh the model lists under Models.</div>}><ModelPolicies c={c} /></Show>
    </Group>
    <Group id="set-roles" title="Roles and pins" hint={[HELP.pins, "role pins"]}>
      <Rec>No pins: automatic routing balances cost, quota and quality.</Rec>
      <Roles c={c} />
    </Group>
    <Group id="set-kinds" title="Work kinds" hint={[HELP.affinity, "work kinds"]}>
      <Rec>Leave every lean on Built in.</Rec>
      <WorkKinds c={c} />
    </Group>
    <section class="set-grp" id="set-quota" aria-label="Quota windows"><Adv id="routing-quota" label="Quota windows" force={windows() || props.tab === "quota"}><Quota c={c} /></Adv></section>
    <section class="set-grp" id="set-deny" aria-label="Deny list"><Adv id="routing-deny" label="Deny list" force={r().deny.length > 0 || r().autoAllow.length > 0 || r().legacyTiers.length > 0 || props.tab === "rules"}><p class="set-grp-lead">Models or engines never to use: hidden from the model selector, refused when chosen or pinned, never routed.<Hint text={HELP.deny} label="deny rules" /></p><Rules c={c} /></Adv></section>
    <Group id="set-router" title="How the router decides"><RouterRules /></Group>
  </Shell>;
}
