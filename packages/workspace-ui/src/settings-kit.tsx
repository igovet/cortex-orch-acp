import { createMemo, createSignal, For, Match, Show, Switch, type JSX } from "solid-js";
import type { Problem } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I, MenuItem, Pop } from "./kit";
import type { HelpText } from "./orch-help";
import type { Route } from "./router";
import type { Draft } from "./settings-state";

export const Srow = (props: { t: JSX.Element; d?: JSX.Element; children?: JSX.Element }) => <div class="srow">
  <span><span class="t">{props.t}</span><Show when={props.d}><span class="d">{props.d}</span></Show></span>
  <span class="ctl">{props.children}</span>
</div>;

export function Seg<T extends string>(props: { label: string; value: T; options: readonly { id: T; label: string; icon?: string; off?: string }[]; onChange: (id: T) => void; disabled?: string }) {
  return <div class="seg tall" role="group" aria-label={props.label}><For each={props.options}>{o => {
    const off = () => o.off ?? props.disabled;
    return <button aria-current={props.value === o.id ? "true" : "false"} aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) props.onChange(o.id); }}>
      <Show when={o.icon}><I n={o.icon!} s={14} /></Show>{o.label}
    </button>;
  }}</For></div>;
}
/** The box shows what the draft says, so a refused change snaps back. Read through a memo: a conditional prop read in a handler would build a computation with no owner. */
export function Sw(props: { label: string; checked: boolean; onChange: (on: boolean) => void; disabled?: string }) {
  const checked = createMemo(() => props.checked);
  return <><span class="sw">
    <input type="checkbox" aria-label={props.label} checked={checked()} disabled={!!props.disabled} onChange={e => { const on = e.currentTarget.checked; e.currentTarget.checked = checked(); props.onChange(on); }} /><i></i>
  </span><Show when={props.disabled}>{why => <span class="ws-why">{why()}</span>}</Show></>;
}
export function Stepper(props: { label: string; value: number; suffix?: string; min: number; max: number; onChange: (n: number) => void; disabled?: string }) {
  const step = (by: number) => { const next = props.value + by; if (!props.disabled && next >= props.min && next <= props.max) props.onChange(next); };
  return <span class="stepper" role="group" aria-label={props.label}>
    <button aria-label={`Fewer: ${props.label}`} aria-disabled={props.disabled || props.value <= props.min ? "true" : undefined} data-why={props.disabled} onClick={() => step(-1)}><I n="minus" s={14} /></button>
    <span class="v">{props.value}{props.suffix ? ` ${props.suffix}` : ""}</span>
    <button aria-label={`More: ${props.label}`} aria-disabled={props.disabled || props.value >= props.max ? "true" : undefined} data-why={props.disabled} onClick={() => step(1)}><I n="plus" s={14} /></button>
  </span>;
}
export function Pick(props: { id: string; label: string; value: string; options: readonly { id: string; label: string; sub?: string; off?: string }[]; onPick: (id: string) => void; width?: string; disabled?: string }) {
  const text = () => props.options.find(o => o.id === props.value)?.label ?? props.value;
  return <>
    <button class="pick" style={props.width ? { "min-width": props.width } : undefined} popovertarget={props.disabled ? undefined : props.id} aria-disabled={props.disabled ? "true" : undefined} data-why={props.disabled} aria-label={`${props.label}: ${text()}`}>
      <span class="trunc">{text()}</span><I n="chevron-down" s={12} />
    </button>
    <Pop id={props.id} class="pop-right" align="right" label={props.label}>
      <For each={props.options}>{o => <MenuItem label={o.label} sub={o.sub ?? o.off} disabled={o.off} check={o.id === props.value} onSelect={() => props.onPick(o.id)} />}</For>
    </Pop>
  </>;
}

/** Marked `data-problem`, so the save bar can jump to the first one. It takes no room until there is something to say. */
export const Err = (props: { problems: Problem[]; path: string }) => <Show when={props.problems.find(p => p.path === props.path)}>{p => <span class="ws-err" data-problem="">{p().message}</span>}</Show>;
export const Lock = (props: { text?: string }) => <span class="lock"><I n="lock" s={12} />{props.text ?? "Built in"}</span>;
export function Chk(props: { label: string; checked: boolean; onChange: (on: boolean) => void; disabled?: string; title?: string; data?: string }) {
  const checked = createMemo(() => props.checked);
  return <><input type="checkbox" class="chk" aria-label={props.label} checked={checked()} disabled={!!props.disabled} data-cell={props.data} onChange={e => { const on = e.currentTarget.checked; e.currentTarget.checked = checked(); props.onChange(on); }} /><Show when={props.disabled}>{why => <span class="ws-why">{why()}</span>}</Show></>;
}

/** A whole number or nothing: empty is its own value here, so it is never shown as a zero. */
export function NumField(props: { label: string; value: number | null; onChange: (n: number | null) => void; disabled?: string; placeholder?: string }) {
  return <><input class="inp ws-num tnum" inputmode="numeric" aria-label={props.label} placeholder={props.placeholder} value={props.value ?? ""} disabled={!!props.disabled}
    onInput={e => { const t = e.currentTarget.value.trim(); props.onChange(t === "" ? null : Number(t)); }} /><Show when={props.disabled}>{why => <span class="ws-why">{why()}</span>}</Show></>;
}
/** Short free values: type and press Enter. */
export function Tags(props: { label: string; items: string[]; onChange: (items: string[]) => void; disabled?: string; mono?: boolean; hint?: string }) {
  const [text, setText] = createSignal("");
  const add = () => { const t = text().trim(); if (t && !props.items.includes(t)) props.onChange([...props.items, t]); setText(""); };
  return <div class="ws-tags">
    <For each={props.items}>{t => <span class={`chip ws-static${props.mono ? " mono" : ""}`}>{t}<Show when={!props.disabled}><button class="ws-att-x" aria-label={`Remove ${t}`} onClick={() => props.onChange(props.items.filter(x => x !== t))}><I n="x" s={12} /></button></Show></span>}</For>
    <Show when={!props.disabled}><input class={`inp ws-tag-in${props.mono ? " mono" : ""}`} aria-label={props.label} placeholder={props.hint ?? "Add"} value={text()} onInput={e => setText(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); add(); } }} onBlur={add} /></Show>
  </div>;
}
/** An ordered list, one per line: paths and arguments. */
export function Lines(props: { label: string; items: string[]; onChange: (items: string[]) => void; disabled?: string; problems?: Problem[]; path?: string; note?: (item: string) => string | null; add?: string }) {
  const at = (i: number, value: string) => props.onChange(props.items.map((x, n) => n === i ? value : x));
  return <div class="ws-lines">
    <For each={props.items}>{(item, i) => <div class="ws-line">
      <div class="ws-line-r"><input class="inp mono" aria-label={`${props.label} ${i() + 1}`} value={item} disabled={!!props.disabled} onChange={e => at(i(), e.currentTarget.value)} />
        <Show when={!props.disabled}><button class="ib ws-ib" aria-label={`Remove ${props.label} ${i() + 1}`} onClick={() => props.onChange(props.items.filter((_, n) => n !== i()))}><I n="x" s={14} /></button></Show></div>
      <Show when={props.note?.(item)}>{n => <span class="cap st-warn">{n()}</span>}</Show>
      <Show when={props.path}><Err problems={props.problems ?? []} path={`${props.path}[${i()}]`} /></Show>
    </div>}</For>
    <Show when={!props.disabled}><div><button class="btn btn-line btn-sm" onClick={() => props.onChange([...props.items, ""])}><I n="plus" s={12} />{props.add ?? "Add"}</button></div></Show>
  </div>;
}
export function KeyValues(props: { label: string; values: Record<string, string>; onChange: (v: Record<string, string>) => void; disabled?: string }) {
  const rows = () => Object.entries(props.values);
  const rename = (from: string, to: string) => props.onChange(Object.fromEntries(rows().map(([k, v]) => [k === from ? to : k, v])));
  const fresh = () => { let n = 1; while (`name_${n}` in props.values) n++; return `name_${n}`; };
  return <div class="ws-lines">
    <For each={rows()}>{([k, v]) => <div class="ws-line-r">
      <input class="inp mono" aria-label={`${props.label} name`} value={k} disabled={!!props.disabled} onChange={e => rename(k, e.currentTarget.value)} />
      <input class="inp mono" aria-label={`${props.label} value: ${k}`} value={v} disabled={!!props.disabled} onChange={e => props.onChange({ ...props.values, [k]: e.currentTarget.value })} />
      <Show when={!props.disabled}><button class="ib ws-ib" aria-label={`Remove ${k}`} onClick={() => props.onChange(Object.fromEntries(rows().filter(([x]) => x !== k)))}><I n="x" s={14} /></button></Show>
    </div>}</For>
    <Show when={!props.disabled}><div><button class="btn btn-line btn-sm" onClick={() => props.onChange({ ...props.values, [fresh()]: "" })}><I n="plus" s={12} />Add parameter</button></div></Show>
  </div>;
}

let hints = 0;
/** One line stays on the row; the full text lives behind this. */
export function Hint(props: { text: HelpText; label: string }) {
  const id = `m-hint-${++hints}`;
  return <>
    <button class="ws-hint" popovertarget={id} aria-label={`About ${props.label}`}>?</button>
    <Pop id={id} class="pop-anchor ws-hint-pop" align="left" label={`About ${props.label}`}>
      <p class="ws-pre">{props.text}</p>
    </Pop>
  </>;
}
/** Closed by default and remembered per page. It opens by itself when it holds something, so nothing configured is hidden. */
export function Adv(props: { id: string; label?: string; force?: boolean; children: JSX.Element }) {
  const ws = useWs();
  const open = () => !!props.force || !!ws.panels.advanced()[props.id];
  return <div class="ws-adv">
    <button class="ws-adv-h" aria-expanded={open()} aria-disabled={props.force ? "true" : undefined} data-why={props.force ? "Stays open: it holds a value or an error" : undefined} onClick={() => { if (!props.force) ws.panels.setAdvanced(props.id, !open()); }}>
      <I n={open() ? "chevron-down" : "chevron-right"} s={12} />{props.label ?? "Advanced"}
    </button>
    <Show when={open()}><div class="ws-adv-b">{props.children}</div></Show>
  </div>;
}
export function Tabs(props: { label: string; tabs: readonly { id: string; label: string; n?: number; off?: string }[]; current: string; route: (id: string) => Route }) {
  return <div class="tabs ws-tabs" role="tablist" aria-label={props.label}><For each={props.tabs}>{t => <Show when={!t.off} fallback={<span class="tab" role="tab" aria-selected="false" aria-disabled="true" data-why={t.off}>{t.label}</span>}><A class="tab" role="tab" aria-selected={props.current === t.id ? "true" : "false"} to={props.route(t.id)}>{t.label}<Show when={t.n !== undefined}><span class="n">{t.n}</span></Show></A></Show>}</For></div>;
}

function Review<T>(props: { d: Draft<T> }) {
  const c = createMemo(() => props.d.changes());
  const line = (key: string) => <li class={c().both.includes(key) ? "st-warn" : undefined}><span class="mono">{key}</span><Show when={c().both.includes(key)}><span> collision</span></Show></li>;
  return <div class="ws-review" role="status">
    <div><b>Changed elsewhere</b><Show when={c().there.length > 0} fallback={<p class="cap muted">No value changed there.</p>}><ul><For each={c().there}>{line}</For></ul></Show></div>
    <div><b>Changed by you</b><ul><For each={c().mine}>{line}</For></ul></div>
  </div>;
}
/** Save is the only way a draft reaches the machine. A conflict is reviewed before anything is written over it. */
export function SaveBar<T>(props: { d: Draft<T>; onProfile?: () => void; source?: () => void }) {
  const d = props.d;
  const n = () => d.count();
  const errors = () => d.problems().length;
  const jump = () => { const el = document.querySelector<HTMLElement>("[data-problem]"); el?.scrollIntoView?.({ block: "center" }); el?.closest(".srow, .gr, .ws-line, .ws-adv-b")?.querySelector<HTMLElement>("input, textarea, button")?.focus?.(); };
  return <Show when={d.dirty() || d.note() || d.state()}>
    <Show when={d.state() === "review"}><Review d={d} /></Show>
    <Show when={d.failure()}><div class="note-box" role="alert"><I n="alert" s={14} /><span>{d.failure()}</span></div></Show>
    <div class="savebar">
      <Switch>
        <Match when={d.state() === "conflict"}>
          <I n="alert" s={14} class="st-warn" /><span role="alert">Changed elsewhere since you opened it. Nothing was written.</span>
          <span class="r"><button class="btn btn-line" onClick={() => d.discard()}>Discard mine</button><button class="btn btn-primary" onClick={() => d.review()}>Review changes</button></span>
        </Match>
        <Match when={d.state() === "review"}>
          <I n="alert" s={14} class="st-warn" /><span>Reapplying writes your keys on top of what is there now.</span>
          <span class="r"><button class="btn btn-line" onClick={() => d.discard()}>Discard mine</button><button class="btn btn-primary" onClick={() => d.reapply()}>Reapply mine</button></span>
        </Match>
        <Match when={d.state() === "unknown"}>
          <I n="help" s={14} class="st-warn" /><span role="alert">No answer. It may have been saved.</span>
          <span class="r"><button class="btn btn-primary" onClick={() => d.reread()}>Read again</button></span>
        </Match>
        <Match when={d.dirty() && errors() > 0}>
          <I n="alert" s={14} class="st-bad" /><span role="status">Fix {errors()} field{errors() === 1 ? "" : "s"}</span>
          <span class="r"><button class="btn btn-line" onClick={() => d.discard()}>Discard</button><button class="btn btn-line" onClick={jump}>Go to the first</button><button class="btn btn-primary" aria-disabled="true" data-why="Fix the fields first">Save {d.name()} on {d.where()}</button></span>
        </Match>
        <Match when={d.dirty()}>
          <I n="pencil" s={14} class="faint" /><span>{n()} unsaved change{n() === 1 ? "" : "s"} in {d.name()}</span><span class="cap faint">Running tasks keep their configuration.</span>
          <span class="r"><button class="btn btn-line" onClick={() => d.discard()}>Discard</button><button class="btn btn-primary" onClick={() => d.save()}>Save {d.name()} on {d.where()}</button></span>
        </Match>
        <Match when={d.note()}>
          <I n="check" s={14} class="st-ok" /><span>{d.note()}</span>
          <span class="r">
            <Show when={props.source}><button class="btn btn-line" onClick={() => props.source!()}>Open source</button></Show>
            <Show when={props.onProfile}><button class="btn btn-line" onClick={() => { props.onProfile!(); d.clearNote(); }}>Save to profile</button></Show>
          </span>
        </Match>
      </Switch>
    </div>
  </Show>;
}
