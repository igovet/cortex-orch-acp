import { createEffect, createMemo, createSignal, createUniqueId, For, Match, on, onCleanup, onMount, Show, Switch, untrack, type Accessor } from "solid-js";
import type { PlanItem } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, I, Keyed, MenuItem, place, Pop } from "./kit";
import { ActionBar, Toolbar } from "./layout";
import {
  adjacency, boundsOf, clampZoom, detailLevel, finishedLayers, fitZoom, GRAPH, graphCounts, graphLabel, graphMatches, graphTitle, layerHeading,
  layerOpened, layerTally, layout, linksOf, listNumbers, matchAnnouncement, matchCount, noMatchText, nodeState, numberIconZoom, selectionLine,
  statusCounts, statusFilterOf, taskLabel, topologyKey, truncateTitle, ZOOM_STEP, type Box, type GraphLayout, type StatusFilter,
} from "./plan-graph";
import type { PlanRow } from "./sprint-model";
import { mapWanted, toolbarTier } from "./sprint-space";

/*
 * The Graph view of a sprint plan: toolbar, canvas and status band, stacked and never layered. Only the minimap sits on
 * the canvas. Pan and zoom write one transform per frame and touch no signal; detail level, labels and the minimap's
 * visibility follow when a gesture settles. Selection and search set classes through one shared memo, so only the nodes
 * whose class changes are written. Nothing in the canvas animates.
 */

export type GraphMemory = { z: number; x: number; y: number; fit: boolean; collapsed: number[] };
export type GraphApi = { fit: () => void };
export type GraphViewProps = {
  items: PlanItem[]; rows: PlanRow[]; title: string;
  /** A past version and a draft carry no task states and no Status filter. */
  mode: "plan" | "past" | "draft"; created: boolean; manual: boolean;
  /** Only an explicit pick highlights and dims; never a default. */
  picked: string | null; onPick: (key: string | null) => void;
  details: string | null; onDetails: (key: string) => void;
  full: boolean; onFull: (on: boolean) => void;
  request: { shown: boolean; why: string | null; run: () => void };
  onOpenList: () => void;
  /** Zoom, pan and collapsed layers, kept for the tab per sprint and plan version. */
  memory: Map<string, GraphMemory>; memoryKey: string;
  active: Accessor<boolean>;
  api?: (api: GraphApi | null) => void;
};

const NW = GRAPH.nodeW, NH = GRAPH.nodeH, BANDW = GRAPH.bandW, BANDH = GRAPH.bandH, MARGIN = GRAPH.margin;
const MAP = { w: 176, h: 112, iw: 174, ih: 110, inset: 8 };
const circle = (cx: number, cy: number, r: number) => `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0`;
const HELP = "M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3M12 17h.01";
/** The state icons a node can show, drawn in the canvas because the app's mask icons cannot sit inside SVG text. */
const ICONS: Record<string, string> = {
  question: `M7.9 20A9 9 0 1 0 4 16.1L2 22Z${HELP}`, alert: "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3M12 9v4M12 17h.01",
  "x-circle": `${circle(12, 12, 10)}M15 9l-6 6M9 9l6 6`, loader: "M21 12a9 9 0 1 1-6.22-8.56", pause: `${circle(12, 12, 10)}M10 15V9M14 15V9`,
  hourglass: "M5 22h14M5 2h14M17 22v-4.17a2 2 0 0 0-.59-1.41L12 12l-4.41 4.42A2 2 0 0 0 7 17.83V22M7 2v4.17a2 2 0 0 0 .59 1.41L12 12l4.41-4.42A2 2 0 0 0 17 6.17V2",
  eye: `M2.06 12.35a1 1 0 0 1 0-.7 10.75 10.75 0 0 1 19.88 0 1 1 0 0 1 0 .7 10.75 10.75 0 0 1-19.88 0${circle(12, 12, 3)}`,
  merge: `${circle(18, 18, 3)}${circle(6, 6, 3)}M6 21V9a9 9 0 0 0 9 9`, play: "M6 3l14 9-14 9z", check: "M20 6 9 17l-5-5", circle: circle(12, 12, 9),
  help: `${circle(12, 12, 10)}${HELP}`, "check-circle": `${circle(12, 12, 10)}M9 12l2 2 4-4`, down: "m6 9 6 6 6-6", right: "m9 18 6-6-6-6",
};
const iconOf = (name: string) => ICONS[name] ? name : "circle";

// Text is measured once per level change, with the canvas font. Where no measuring surface exists the width is estimated.
const measurer = (() => { try { return typeof OffscreenCanvas === "undefined" ? null : new OffscreenCanvas(1, 1).getContext("2d"); } catch { return null; } })();
let family = "sans-serif";
const textWidth = (s: string, px: number, weight = 400) => {
  if (!measurer) return s.length * px * 0.55;
  measurer.font = `${weight} ${px}px ${family}`;
  return measurer.measureText(s).width;
};
/** The longest end-truncated first line that fits `avail` screen pixels, ellipsis included. */
function fitText(text: string, avail: number, px: number, weight = 400) {
  const whole = text.split("\n", 1)[0];
  if (textWidth(whole, px, weight) <= avail) return whole;
  let lo = 0, hi = Array.from(whole).length;
  while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (textWidth(truncateTitle(whole, mid), px, weight) <= avail) lo = mid; else hi = mid - 1; }
  return lo > 1 ? truncateTitle(whole, lo) : "";
}
const typing = (el: EventTarget | null) => el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement
  || (el instanceof HTMLInputElement && !["checkbox", "radio", "button"].includes(el.type)) || (el as HTMLElement | null)?.isContentEditable === true;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
/** An open popover keeps its own keys. An engine without the popover selector has none open. */
const popoverOpen = () => { try { return !!document.querySelector("[popover]:popover-open"); } catch { return false; } };
const afterText = (deps: number[]) => deps.length ? `After ${listNumbers(deps)}` : "Starts first";
type Rect = { l: number; t: number; r: number; b: number };
const sameList = (a: readonly (string | number)[], b: readonly (string | number)[]) => a.length === b.length && a.every((x, i) => x === b[i]);
const sameItems = (a: readonly PlanItem[], b: readonly PlanItem[]) => a.length === b.length && a.every((x, i) => x === b[i] || (x.key === b[i].key && graphTitle(x) === graphTitle(b[i]) && sameList(x.dependsOn, b[i].dependsOn)));
type Info = { filter: StatusFilter | null; state: ReturnType<typeof nodeState>; deps: number[] };
const sameInfo = (a: ReadonlyMap<string, Info>, b: ReadonlyMap<string, Info>) => a.size === b.size && [...a].every(([k, x]) => {
  const y = b.get(k);
  return !!y && x.filter === y.filter && x.state?.text === y.state?.text && x.state?.tone === y.state?.tone && x.state?.icon === y.state?.icon && sameList(x.deps, y.deps);
});

export function GraphView(props: GraphViewProps) {
  const ws = useWs();
  const uid = createUniqueId();
  let root!: HTMLDivElement, canvas!: HTMLDivElement, layer!: HTMLDivElement, svg!: SVGSVGElement, world!: SVGGElement, search!: HTMLInputElement, pct!: HTMLSpanElement, mv!: SVGRectElement, map!: HTMLDivElement;
  const els = new Map<string, SVGGElement>();
  const saved = props.memory.get(props.memoryKey);

  // ---------- Model ----------
  // A machine read builds new objects for the same plan: these hold still until what they show really changes.
  const items = createMemo(() => props.items, undefined, { equals: sameItems });
  const topo = createMemo(() => topologyKey(items()));
  const [collapsed, setCollapsed] = createSignal<ReadonlySet<number>>(new Set(saved?.collapsed ?? []));
  const g = createMemo<GraphLayout>(() => { topo(); return layout(untrack(items), collapsed()); });
  const adj = createMemo(() => { topo(); return adjacency(untrack(items)); });
  const byKey = createMemo(() => new Map(items().map(i => [i.key, i])));
  const rowOf = createMemo(() => new Map(props.rows.map(r => [r.item.key, r])));
  const states = createMemo(() => props.mode === "plan" && props.rows.some(r => r.backed));
  const info = createMemo(() => new Map(props.rows.map(r => [r.item.key, { filter: states() ? statusFilterOf(r) : null, state: states() ? nodeState(r) : null, deps: r.deps }] as const)), undefined, { equals: sameInfo });
  const statusOf = createMemo(() => new Map([...info()].map(([k, v]) => [k, v.filter] as const)));
  const stateOf = (key: string) => info().get(key)?.state ?? null;
  const depsOf = (key: string) => info().get(key)?.deps ?? [];
  const n = () => items().length;
  const hasSearch = () => n() >= 8, hasStatus = () => states() && n() >= 8, hasLayers = () => n() >= 13 && g().layers.length >= 2, hasMap = () => n() > 1;
  const numbers = (keys: Iterable<string>) => [...keys].map(k => adj().no.get(k) ?? 0).filter(Boolean);

  // ---------- View: plain numbers, written once per frame ----------
  const VW = { z: saved?.z ?? 1, x: saved?.x ?? MARGIN, y: saved?.y ?? MARGIN };
  let fitLocked = saved?.fit ?? true;
  const CANVAS = { w: 0, h: 0 };
  const [view, setView] = createSignal({ z: VW.z, tick: 0 });
  const [canvasW, setCanvasW] = createSignal(0);
  const [offscreen, setOffscreen] = createSignal(false);
  const [tier, setTier] = createSignal<0 | 2 | 3>(0);
  const [said, setSaid] = createSignal("");
  const announce = (text: string) => setSaid(text);
  let raf = 0, settleTimer: ReturnType<typeof setTimeout> | undefined;

  const fit = () => fitZoom(g(), { width: CANVAS.w, height: CANVAS.h });
  const clampView = () => {
    const L = g(), z = VW.z;
    if (!CANVAS.w || !CANVAS.h) return;
    VW.x = clamp(VW.x, 48 - L.width * z, CANVAS.w - 48);
    VW.y = clamp(VW.y, 48 - L.height * z, CANVAS.h - 48);
  };
  const mini = createMemo(() => {
    const L = g(), s = L.width && L.height ? Math.min((MAP.iw - 8) / L.width, (MAP.ih - 8) / L.height) : 1;
    const ox = (MAP.iw - L.width * s) / 2, oy = (MAP.ih - L.height * s) / 2;
    const paths: Record<"m0" | "m-needs" | "m-problem" | "m-unknown", string> = { m0: "", "m-needs": "", "m-problem": "", "m-unknown": "" };
    const rect = (b: Box, min = 2) => `M${ox + b.x * s} ${oy + b.y * s}h${Math.max(min, b.w * s)}v${Math.max(min, b.h * s)}h${-Math.max(min, b.w * s)}z`;
    for (const t of L.tasks) { const f = statusOf().get(t.key); paths[f === "needs" ? "m-needs" : f === "problem" ? "m-problem" : f === "unknown" ? "m-unknown" : "m0"] += rect(t); }
    for (const b of L.bands) paths.m0 += rect(b);
    return { s, ox, oy, paths, rect };
  });
  // The drawing is rendered at one scale and moved as one composited layer: a pan or a zoom gesture writes only the layer's
  // transform, and the drawing is rendered again at the new scale once the gesture settles.
  let rendered = VW.z;
  const [drawn, setDrawn] = createSignal(VW.z);
  const draw = () => { if (rendered === VW.z || !world) return; rendered = VW.z; world.setAttribute("transform", `scale(${rendered})`); setDrawn(rendered); };
  const apply = () => {
    raf = 0;
    if (!layer) return;
    const k = VW.z / rendered;
    layer.style.transform = k === 1 ? `translate(${Math.round(VW.x)}px, ${Math.round(VW.y)}px)` : `translate(${VW.x}px, ${VW.y}px) scale(${k})`;
    const m = mini(), z = VW.z;
    if (mv) { mv.setAttribute("x", String(m.ox + -VW.x / z * m.s)); mv.setAttribute("y", String(m.oy + -VW.y / z * m.s)); mv.setAttribute("width", String(CANVAS.w / z * m.s)); mv.setAttribute("height", String(CANVAS.h / z * m.s)); }
    if (pct) pct.textContent = `${Math.round(VW.z * 100)}%`;
  };
  const isOff = () => {
    const z = VW.z, L = g(), x = -VW.x / z, y = -VW.y / z, w = CANVAS.w / z, h = CANVAS.h / z;
    return CANVAS.w > 0 && (x > 1 || y > 1 || x + w < L.width - 1 || y + h < L.height - 1);
  };
  const settle = () => {
    clearTimeout(settleTimer);
    origin = null;
    draw(); apply();
    delete svg?.dataset.moving;
    setOffscreen(isOff());
    setView(v => ({ z: VW.z, tick: v.tick + 1 }));
    guardMap();
    const t = tip(); if (t) showTip(t.id, t.at);
  };
  const kick = (moving: boolean) => {
    clampView();
    if (moving && svg) svg.dataset.moving = "";
    if (!raf) raf = requestAnimationFrame(apply);
    clearTimeout(settleTimer);
    settleTimer = setTimeout(settle, 120);
  };
  const now = () => { clearTimeout(settleTimer); if (raf) { cancelAnimationFrame(raf); raf = 0; } clampView(); apply(); settle(); };
  const focusInCanvas = () => !!(document.activeElement && svg?.contains(document.activeElement));
  const screenRect = (id: string): Rect | null => {
    const b = g().boxes.get(id); if (!b) return null;
    const z = VW.z; return { l: b.x * z + VW.x, t: b.y * z + VW.y, r: (b.x + b.w) * z + VW.x, b: (b.y + b.h) * z + VW.y };
  };
  const mapShown = () => hasMap() && mapWanted(ws.panels.minimap(), canvasW()) && offscreen();
  const mapRect = (): Rect | null => untrack(mapShown) ? { l: CANVAS.w - MAP.inset - MAP.w, t: CANVAS.h - MAP.inset - MAP.h, r: CANVAS.w - MAP.inset, b: CANVAS.h - MAP.inset } : null;
  const overlapsMap = (r: Rect) => { const m = mapRect(); return m && r.l < m.r && r.r > m.l && r.t < m.b && r.b > m.t ? m : null; };
  /** Pans just enough to show a box with `margin` around it, and never leaves it under the minimap. */
  const reveal = (id: string, margin: number) => {
    const r = screenRect(id); if (!r || !CANVAS.w) return;
    let dx = 0, dy = 0;
    const fw = r.r - r.l, fh = r.b - r.t;
    if (fw + 2 * margin > CANVAS.w) dx = (CANVAS.w - fw) / 2 - r.l; else if (r.l < margin) dx = margin - r.l; else if (r.r > CANVAS.w - margin) dx = CANVAS.w - margin - r.r;
    if (fh + 2 * margin > CANVAS.h) dy = (CANVAS.h - fh) / 2 - r.t; else if (r.t < margin) dy = margin - r.t; else if (r.b > CANVAS.h - margin) dy = CANVAS.h - margin - r.b;
    VW.x += dx; VW.y += dy;
    const m = overlapsMap({ l: r.l + dx, r: r.r + dx, t: r.t + dy, b: r.b + dy });
    if (m) { const left = r.r + dx - m.l + 8, up = r.b + dy - m.t + 8; if (up < left) VW.y -= up; else VW.x -= left; }
    if (dx || dy || m) { fitLocked = false; now(); }
  };
  const guardMap = () => { const f = focus(); if (!f || !focusInCanvas()) return; const r = screenRect(f); if (r && overlapsMap(r)) reveal(f, 0); };
  const setZoom = (z: number, cx?: number, cy?: number, moving = false) => {
    z = clampZoom(z, fit());
    let keep = false;
    if (cx === undefined || cy === undefined) {
      const f = focus(), r = f && focusInCanvas() ? screenRect(f) : null;
      if (r) { cx = (r.l + r.r) / 2; cy = (r.t + r.b) / 2; keep = true; } else { cx = CANVAS.w / 2; cy = CANVAS.h / 2; }
    }
    const wx = (cx - VW.x) / VW.z, wy = (cy - VW.y) / VW.z;
    VW.z = z; VW.x = cx - wx * z; VW.y = cy - wy * z;
    fitLocked = false;
    if (moving) kick(true); else now();
    if (keep) reveal(focus()!, 48);
  };
  const fitAll = () => { VW.z = fit(); VW.x = MARGIN; VW.y = MARGIN; fitLocked = true; now(); };
  const fitBox = (b: Box | null) => {
    if (!b) return;
    const z = clampZoom(Math.min(1, (CANVAS.w - 48) / b.w, (CANVAS.h - 48) / b.h), fit());
    VW.z = CANVAS.w ? z : 1; VW.x = (CANVAS.w - b.w * VW.z) / 2 - b.x * VW.z; VW.y = (CANVAS.h - b.h * VW.z) / 2 - b.y * VW.z;
    fitLocked = false; now();
  };
  const fitMatches = () => { const m = matches(); if (m?.length) fitBox(boundsOf(g(), m)); };
  const pan = (dx: number, dy: number) => { VW.x += dx; VW.y += dy; fitLocked = false; now(); };

  // ---------- Selection, search and filter ----------
  const [query, setQuery] = createSignal("");
  const [filter, setFilter] = createSignal<ReadonlySet<StatusFilter>>(new Set());
  const [cur, setCur] = createSignal(-1);
  const [focus, setFocus] = createSignal<string | null>(null);
  const chain = () => ws.panels.links() === "chain";
  const matches = createMemo(() => graphMatches(items(), query(), filter(), statusOf()), null, { equals: (a, b) => a === b || (!!a && !!b && sameList(a, b)) });
  const active = () => matches() !== null;
  const matchSet = createMemo(() => new Set(matches() ?? []));
  const current = () => { const m = matches(); return m && cur() >= 0 && cur() < m.length ? m[cur()] : null; };
  const links = createMemo(() => linksOf(adj(), props.picked, chain() ? "chain" : "direct"));
  const linked = createMemo(() => { const l = links(); return new Set([...(l?.needs ?? []), ...(l?.unblocks ?? [])]); });
  const bright = (k: string) => props.picked ? k === props.picked || linked().has(k) || (active() && matchSet().has(k)) : !active() || matchSet().has(k);
  const nodeClass = createMemo(() => {
    const out = new Map<string, string>(), c = current();
    for (const t of g().tasks) {
      const k = t.key, parts = ["n"];
      if (k === props.picked) parts.push("picked"); else if (linked().has(k)) parts.push("lk");
      if (active() && matchSet().has(k)) parts.push("mt");
      if (k === c) parts.push("cur");
      if (!bright(k)) parts.push("dim");
      out.set(k, parts.join(" "));
    }
    return out;
  });
  const bandClass = createMemo(() => new Map(g().bands.map(b => {
    const lk = !!props.picked && b.keys.some(k => k === props.picked || linked().has(k));
    return [b.id, lk ? "band lk" : b.keys.some(bright) ? "band" : "band dim"] as const;
  })));
  const edgeRole = createMemo(() => {
    const out = new Map<string, string>(), sel = props.picked, l = links(), ms = matchSet(), on = active();
    for (const e of g().edges) {
      let role = "";
      if (sel && l) for (const [a, b] of e.real) {
        if (chain()) { if ((b === sel || l.needs.has(b)) && l.needs.has(a)) role = "need"; else if ((a === sel || l.unblocks.has(a)) && l.unblocks.has(b)) role = "block"; }
        else if (b === sel) role = "need"; else if (a === sel) role = "block";
        if (role) break;
      }
      if (!role && !e.real.some(([a, b]) => on ? ms.has(a) && ms.has(b) : !sel)) role = "dim";
      out.set(e.id, role);
    }
    return out;
  });
  const marker = (role: string) => `url(#${uid}-a${role === "need" || role === "block" ? 1 : role === "dim" ? 2 : 0})`;

  // ---------- Words on the canvas, once per settle ----------
  const level = () => detailLevel(view().z);
  const low = () => level() === "number" || level() === "overview";
  const unit = () => Math.min(view().z, 1);
  const labels = createMemo(() => {
    const lv = level(), u = unit(), out = new Map<string, { l1: string; w: string; ts: string }>();
    if (lv !== "detail" && lv !== "title") return out;
    const box = NW * u, pad = 9;
    for (const item of items()) {
      const no = adj().no.get(item.key) ?? 0, st = stateOf(item.key), title = graphTitle(item), numW = textWidth(`${no} `, 12, 600), icon = st ? 16 : 0;
      if (lv === "detail") out.set(item.key, { l1: fitText(title, box - pad - numW - pad, 12), w: fitText(st?.text ?? afterText(depsOf(item.key)), box - pad - icon - pad, 11), ts: "" });
      else out.set(item.key, { l1: "", w: "", ts: fitText(title, box - pad - icon - numW - pad, 12) });
    }
    return out;
  });
  /** Number and title pins at the Number and Overview levels, for the pick, the current match, keyboard focus and matches; at most 12. */
  const pins = createMemo(() => {
    view();
    if (!low()) return [];
    const ids: string[] = [], push = (k: string | null) => { if (k && !ids.includes(k) && ids.length < 12 && byKey().has(k)) ids.push(k); };
    push(props.picked); push(current());
    const f = focus();
    if (f?.startsWith("t:") && document.documentElement.hasAttribute("data-keyboard") && untrack(focusInCanvas)) push(f.slice(2));
    for (const k of matches() ?? []) push(k);
    const z = VW.z, u = Math.min(z, 1), placed: { x: number; y: number; w: number; h: number }[] = [], out: { key: string; x: number; y: number; w: number; h: number; text: string; u: number }[] = [];
    const free = (x: number, y: number, w: number, h: number) => x >= 2 && y >= 0 && x + w <= CANVAS.w - 2 && y + h <= CANVAS.h - 2 && !placed.some(p => x < p.x + p.w && x + w > p.x && y < p.y + p.h && y + h > p.y);
    for (const k of ids) {
      const b = g().boxes.get(`t:${k}`); if (!b) continue;
      const text = `${adj().no.get(k)} ${fitText(graphTitle(byKey().get(k)!), 240, 12)}`, w = textWidth(text, 12) + 10, h = 18;
      const sx = b.x * z + VW.x, sy = b.y * z + VW.y, sb = (b.y + b.h) * z + VW.y, sr = (b.x + b.w) * z + VW.x;
      let at = [[sx, sy - h - 3], [sr - w, sy - h - 3], [sx, sb + 3], [sr - w, sb + 3]].find(([x, y]) => free(x, y, w, h));
      if (!at) {
        if (k !== props.picked && k !== current()) continue;
        at = [clamp(sx, 2, Math.max(2, CANVAS.w - w - 2)), clamp(sy - h - 3, 0, Math.max(0, CANVAS.h - h - 2))];
      }
      placed.push({ x: at[0], y: at[1], w, h });
      out.push({ key: k, x: (at[0] - VW.x) / z, y: (at[1] - VW.y) / z, w, h, text, u });
    }
    return out;
  });
  const bandRows = (layer: number, keys: string[]) => {
    const u = unit(), hits = keys.filter(k => matchSet().has(k) && active()).length, lk = props.picked ? keys.filter(k => linked().has(k) || k === props.picked).length : 0;
    if (low()) return [{ t: String(keys.length), c: "bt0" }];
    const avail = BANDW * u - 16, rows = [{ t: fitText(`Layer ${layer + 1}`, avail - 14, 12, 500), c: "bt0" }, { t: textWidth(`${keys.length} tasks`, 12) <= avail ? `${keys.length} tasks` : String(keys.length), c: "" }];
    if (hits) rows.push({ t: fitText(`${hits} ${hits === 1 ? "match" : "matches"}`, avail, 12), c: "" });
    if (lk) rows.push({ t: fitText(`${lk} linked`, avail, 12), c: "" });
    for (const t of layerTally(keys, statusOf()).slice(0, 3)) rows.push({ t: fitText(`${t.n} ${t.label}`, avail, 12), c: t.id === "needs" ? "bwarn" : t.id === "problem" ? "bbad" : "" });
    return rows.slice(0, Math.floor((BANDH * u - 8) / 16));
  };
  const bandLabel = (layer: number, keys: string[]) => {
    const hits = active() ? keys.filter(k => matchSet().has(k)).length : 0, lk = props.picked ? keys.filter(k => linked().has(k)).length : 0;
    return `Layer ${layer + 1}, collapsed, ${keys.length} tasks${hits ? `, ${hits} ${hits === 1 ? "match" : "matches"}` : ""}${lk ? `, ${lk} linked` : ""}`;
  };
  const nodeLabel = (key: string) => {
    const no = adj().no.get(key) ?? 0, item = byKey().get(key);
    return taskLabel(no, item ? graphTitle(item) : "", stateOf(key)?.text ?? null, numbers(adj().needs.get(key) ?? []), numbers(adj().unblocks.get(key) ?? []));
  };

  // ---------- Focus and keyboard movement ----------
  const firstVertex = () => { const k = items()[0]?.key; return k ? g().vertexOf.get(k) ?? null : null; };
  const focusId = (id: string | null) => { if (!id) return; setFocus(id); els.get(id)?.focus({ preventScroll: true }); };
  const keysOf = (id: string) => id.startsWith("t:") ? [id.slice(2)] : g().bands.find(b => b.id === id)?.keys ?? [];
  const neighbours = (id: string, dir: 1 | -1) => {
    const out = new Set<string>(), from = g().boxes.get(id)!;
    for (const k of keysOf(id)) for (const m of (dir > 0 ? adj().unblocks : adj().needs).get(k) ?? []) { const v = g().vertexOf.get(m); if (v && v !== id) out.add(v); }
    const cx = from.x + from.w / 2, cy = from.y + from.h / 2;
    const no = (v: string) => v.startsWith("t:") ? adj().no.get(v.slice(2)) ?? 0 : 0;
    return [...out].map(v => { const b = g().boxes.get(v)!; return { v, d: Math.hypot(b.x + b.w / 2 - cx, b.y + b.h / 2 - cy) }; }).sort((a, b) => a.d - b.d || no(a.v) - no(b.v)).map(x => x.v);
  };
  const sameLayer = (id: string) => {
    const L = id.startsWith("b:") ? Number(id.slice(2)) : g().layerOf.get(id.slice(2));
    const all = [...g().tasks.filter(t => t.layer === L), ...g().bands.filter(b => b.layer === L)];
    return all.sort((a, b) => a.y - b.y || a.x - b.x).map(v => v.id);
  };
  let cycle: { origin: string; dir: number; i: number } | null = null;
  const navigate = (key: string, shift: boolean) => {
    const id = focus(); if (!id || !g().boxes.has(id)) return focusId(firstVertex());
    if (key === "ArrowLeft" || key === "ArrowRight") {
      const dir = key === "ArrowRight" ? 1 : -1;
      if (shift) {
        if (!cycle || cycle.dir !== dir) cycle = { origin: id, dir, i: -1 };
        const list = neighbours(cycle.origin, dir); if (!list.length) return;
        cycle.i = (cycle.i + 1) % list.length; return focusId(list[cycle.i]);
      }
      cycle = null;
      const list = neighbours(id, dir); if (list.length) focusId(list[0]);
      return;
    }
    cycle = null;
    const list = sameLayer(id), j = list.indexOf(id) + (key === "ArrowUp" ? -1 : 1);
    if (j >= 0 && j < list.length) focusId(list[j]);
  };

  // ---------- Layers ----------
  let relayout = false;
  const setColl = (L: number, fold: boolean, quiet = false) => {
    const before = g().layers.find(l => l.layer === L); if (!before) return;
    const sx = before.x * VW.z + VW.x, sy = before.top * VW.z + VW.y;
    const had = focus(), wasIn = focusInCanvas();
    const next = new Set(collapsed()); if (fold) next.add(L); else next.delete(L);
    relayout = true; setCollapsed(next); relayout = false;
    const after = g().layers.find(l => l.layer === L)!;
    VW.x = sx - after.x * VW.z; VW.y = sy - after.top * VW.z;
    if (fitLocked) fitAll(); else now();
    fitLocked = false;
    // Focus never falls out of the canvas: a task folded away hands it to its layer's band.
    const target = had && g().boxes.has(had) ? had : fold ? `b:${L}` : focus();
    if (wasIn) focusId(target); else setFocus(target);
    if (!quiet) announce(`Layer ${L + 1} ${fold ? "collapsed" : "expanded"}.`);
  };
  const setAll = (pick: (layer: number) => boolean) => {
    const next = new Set(g().layers.map(l => l.layer).filter(pick));
    if (next.size === collapsed().size && [...next].every(l => collapsed().has(l))) return;
    relayout = true; setCollapsed(next); relayout = false;
    fitAll();
    const f = focus(); if (f && !g().boxes.has(f)) setFocus(firstVertex());
  };
  const expandBand = (L: number) => { const keys = g().bands.find(b => b.layer === L)?.keys ?? []; setColl(L, false); focusId(keys[0] ? g().vertexOf.get(keys[0]) ?? null : null); };
  // A dependency edit in a draft moves tasks: the pick, or else the view's centre, stays where it was on screen.
  createEffect(on(g, (next, prev) => {
    if (!prev || relayout) return;
    if (fitLocked) { fitAll(); return; }
    const anchor = props.picked, a = anchor ? prev.boxes.get(prev.vertexOf.get(anchor) ?? "") : null, b = anchor ? next.boxes.get(next.vertexOf.get(anchor) ?? "") : null;
    if (a && b) { VW.x += (a.x - b.x) * VW.z; VW.y += (a.y - b.y) * VW.z; }
    now();
    const f = untrack(focus); if (f && !next.boxes.has(f)) setFocus(firstVertex());
  }, { defer: true }));
  createEffect(() => { const f = focus(); if (!f || !g().boxes.has(f)) setFocus(firstVertex()); });

  // ---------- Search ----------
  let typed: ReturnType<typeof setTimeout> | undefined;
  createEffect(on(matches, m => {
    setCur(m?.length ? 0 : -1);
    const c = m?.[0];
    if (c && query().trim()) {
      const v = g().vertexOf.get(c); if (v) reveal(v, 12);
      announce(matchAnnouncement(0, m!.length, adj().no.get(c) ?? 0, graphTitle(byKey().get(c)!)));
    } else if (m && !m.length) announce(noMatchText(query(), filter().size > 0));
  }, { defer: true }));
  const step = (dir: 1 | -1) => {
    const m = matches(); if (!m?.length) return;
    const i = (cur() + dir + m.length) % m.length, k = m[i], L = g().layerOf.get(k)!;
    setCur(i);
    if (collapsed().has(L)) { setColl(L, false, true); announce(layerOpened(L)); }
    reveal(`t:${k}`, 12);
    if (query().trim() && !collapsed().has(L)) announce(matchAnnouncement(i, m.length, adj().no.get(k) ?? 0, graphTitle(byKey().get(k)!)));
  };
  const clearSearch = () => { clearTimeout(typed); search.value = ""; setQuery(""); };
  const toggleFilter = (f: StatusFilter, on: boolean) => { const next = new Set(filter()); if (on) next.add(f); else next.delete(f); setFilter(next); };
  const focusSearch = () => { if (!hasSearch() || !search) return; search.focus(); search.select(); };

  // ---------- Commands ----------
  const select = (key: string | null) => {
    props.onPick(key);
    if (key) announce(`${selectionLine(adj().no.get(key) ?? 0, numbers(adj().needs.get(key) ?? []), numbers(adj().unblocks.get(key) ?? []))}.`);
  };
  const toggleMap = () => { ws.panels.setMinimap(!mapWanted(ws.panels.minimap(), canvasW())); setOffscreen(isOff()); guardMap(); };
  const toggleChain = () => { if (!props.picked) return; ws.panels.setLinks(chain() ? "direct" : "chain"); announce(chain() ? "Whole chain." : "Direct links."); };
  const toggleFull = () => props.onFull(!props.full);
  createEffect(on(() => props.full, on => { announce(on ? "Full view." : "Left full view."); queueMicrotask(() => { if (!document.activeElement || document.activeElement === document.body) focusId(focus()); }); }, { defer: true }));

  // ---------- Tooltip ----------
  const [tip, setTip] = createSignal<{ id: string; at?: { x: number; y: number }; left: number; top: number } | null>(null);
  let tipTimer: ReturnType<typeof setTimeout> | undefined, tipEl: HTMLDivElement | undefined;
  const hideTip = () => { clearTimeout(tipTimer); setTip(null); };
  const showTip = (id: string, at?: { x: number; y: number }) => {
    if (!id.startsWith("t:")) return hideTip();
    const r = screenRect(id); if (!r) return hideTip();
    setTip({ id, at, left: at ? at.x + 12 : r.l, top: at ? at.y + 16 : r.t - 40 });
    const w = tipEl?.offsetWidth ?? 0, h = tipEl?.offsetHeight ?? 0;
    let top = at ? at.y + 16 : r.t - h - 6;
    if (top < 4) top = (at ? at.y : r.b) + 6;
    setTip({ id, at, left: clamp(at ? at.x + 12 : r.l, 4, Math.max(4, CANVAS.w - w - 4)), top: clamp(top, 4, Math.max(4, CANVAS.h - h - 4)) });
  };
  const tipText = () => { const t = tip(); if (!t) return null; const k = t.id.slice(2), item = byKey().get(k); return item ? { head: `${adj().no.get(k)} · ${graphTitle(item)}`, sub: stateOf(k)?.text ?? afterText(depsOf(k)) } : null; };

  // ---------- Pointer ----------
  let down: { x: number; y: number; vx: number; vy: number; moved: boolean; target: Element; id: number } | null = null, lastClick = { id: "", t: 0 };
  // The canvas's place on the page, read once per gesture: reading it on every wheel event would force a layout each time.
  let origin: DOMRect | null = null;
  const canvasBox = () => origin ??= canvas.getBoundingClientRect();
  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0 || (e.target as Element).closest?.(".gv-map")) return;
    hideTip();
    down = { x: e.clientX, y: e.clientY, vx: VW.x, vy: VW.y, moved: false, target: e.target as Element, id: e.pointerId };
    try { canvas.setPointerCapture?.(e.pointerId); } catch { /* a synthetic press has no pointer to capture */ }
  };
  const onPointerMove = (e: PointerEvent) => {
    if (down) {
      const dx = e.clientX - down.x, dy = e.clientY - down.y;
      if (!down.moved && Math.hypot(dx, dy) < 4) return;
      if (!down.moved) { down.moved = true; canvas.dataset.pan = ""; }
      VW.x = down.vx + dx; VW.y = down.vy + dy; fitLocked = false;
      kick(true);
      return;
    }
    const node = (e.target as Element).closest?.<SVGGElement>(".n");
    if (!node) { hideTip(); return; }
    const box = canvasBox(), at = { x: e.clientX - box.left, y: e.clientY - box.top }, id = node.dataset.id!;
    if (tip()?.id === id) showTip(id, at);
    else { clearTimeout(tipTimer); setTip(null); tipTimer = setTimeout(() => showTip(id, at), 300); }
  };
  const onPointerUp = () => {
    const d = down; if (!d) return;
    down = null; delete canvas.dataset.pan;
    try { canvas.releasePointerCapture?.(d.id); } catch { /* nothing was captured */ }
    if (d.moved) return;
    const t = d.target, head = t.closest?.<SVGGElement>(".lh"), band = t.closest?.<SVGGElement>(".band"), node = t.closest?.<SVGGElement>(".n");
    if (head) return setColl(Number(head.dataset.l), true);
    if (band) return expandBand(Number(band.dataset.l));
    if (node) {
      const id = node.dataset.id!, key = id.slice(2), at = Date.now();
      focusId(id);
      if (lastClick.id === id && at - lastClick.t < 350) { lastClick = { id: "", t: 0 }; props.onDetails(key); }
      else { select(key); lastClick = { id, t: at }; }
      return;
    }
    if (props.picked) select(null);
  };
  const onWheel = (e: WheelEvent) => {
    e.preventDefault(); hideTip();
    if (e.ctrlKey) { const box = canvasBox(); setZoom(VW.z * Math.exp(-e.deltaY * 0.0022), e.clientX - box.left, e.clientY - box.top, true); }
    else if (e.shiftKey) { VW.x -= e.deltaY; fitLocked = false; kick(true); }
    else { VW.y -= e.deltaY; VW.x -= e.deltaX; fitLocked = false; kick(true); }
  };
  let mapDrag: number | null = null;
  const mapTo = (e: PointerEvent, el: HTMLElement) => {
    const r = el.getBoundingClientRect(), m = mini(), wx = (e.clientX - r.left - m.ox) / m.s, wy = (e.clientY - r.top - m.oy) / m.s;
    VW.x = CANVAS.w / 2 - wx * VW.z; VW.y = CANVAS.h / 2 - wy * VW.z; fitLocked = false; kick(true);
  };

  // ---------- Keys: Esc in its order, search, and the canvas's own keys ----------
  // Focus on the page itself, as after a view replaces the focused control, still reaches the canvas keys.
  const loose = (t: EventTarget | null) => t === document.body || (t instanceof HTMLElement && t.matches("main.main"));
  const inGraph = (t: EventTarget | null) => loose(t) || (t instanceof Node && canvas.contains(t));
  const onKey = (e: KeyboardEvent) => {
    if (!props.active() || !root.isConnected || document.querySelector("dialog[open]")) return;
    const t = e.target, k = e.key, ctrl = e.ctrlKey || e.metaKey;
    // Only keys pressed on this screen: its pane, or the page with nothing focused.
    const pane = root.closest(".ws-pane") ?? document.body;
    if (!loose(t) && !(t instanceof Node && pane.contains(t))) return;
    if (popoverOpen()) return;
    if (k === "Escape") {
      if (t === search || (typing(t) && !root.contains(t as Node))) return;
      const act = (run: () => void) => { e.preventDefault(); e.stopPropagation(); run(); };
      if (query().trim()) return act(clearSearch);
      if (props.details) return; // Details closes itself, and its focus returns to the task.
      if (props.picked) return act(() => select(null));
      if (props.full) return act(() => props.onFull(false));
      return;
    }
    if (((k === "/" && !ctrl && !e.altKey) || (ctrl && !e.altKey && k.toLowerCase() === "f")) && !typing(t)) { if (hasSearch()) { e.preventDefault(); focusSearch(); } return; }
    if (typing(t) || !inGraph(t) || e.altKey) return;
    // The minimap pans with its own arrows and keeps the focus, so they never reach the tasks behind it.
    if (k.startsWith("Arrow") && t instanceof Node && map.contains(t)) return;
    if (ctrl) {
      const by: Record<string, [number, number]> = { ArrowLeft: [80, 0], ArrowRight: [-80, 0], ArrowUp: [0, 80], ArrowDown: [0, -80] };
      if (by[k]) { e.preventDefault(); pan(...by[k]); }
      return;
    }
    const id = focus(), key = id?.startsWith("t:") ? id.slice(2) : null;
    const act = (run: () => void) => { e.preventDefault(); run(); };
    if (k.startsWith("Arrow")) return act(() => { if (!svg.contains(t as Node)) focusId(id ?? firstVertex()); else navigate(k, e.shiftKey); });
    switch (k) {
      case "Home": return act(() => focusId(firstVertex()));
      case "End": return act(() => { const last = items()[items().length - 1]?.key; focusId(last ? g().vertexOf.get(last) ?? null : null); });
      case " ": return act(() => { if (key) select(key); });
      case "Enter": return act(() => { if (id?.startsWith("b:")) expandBand(Number(id.slice(2))); else if (key) props.onDetails(key); });
      case "c": case "C": return act(() => { if (id?.startsWith("b:")) setColl(Number(id.slice(2)), false); else if (key) setColl(g().layerOf.get(key)!, true); });
      case "+": case "=": return act(() => setZoom(VW.z * ZOOM_STEP));
      case "-": case "_": return act(() => setZoom(VW.z / ZOOM_STEP));
      case "0": return act(() => setZoom(1));
      case "f": return act(fitAll);
      case "F": return act(fitMatches);
      case "m": case "M": return act(() => { if (hasMap()) toggleMap(); });
      case "t": case "T": return act(toggleChain);
      case "e": case "E": return act(toggleFull);
    }
  };

  // ---------- Size, start and memory ----------
  const measure = () => {
    const r = canvas.getBoundingClientRect();
    if (!r.width || !r.height || (Math.abs(r.width - CANVAS.w) < .5 && Math.abs(r.height - CANVAS.h) < .5)) return;
    CANVAS.w = r.width; CANVAS.h = r.height; setCanvasW(r.width); origin = null;
    if (fitLocked) fitAll(); else now();
  };
  const fitBar = () => {
    const bar = root?.querySelector<HTMLElement>(".gv-bar"); if (!bar?.clientWidth) return;
    let t = toolbarTier(bar.clientWidth);
    setTier(t);
    while (bar.scrollWidth > bar.clientWidth + 1 && t < 3) { t = t === 0 ? 2 : 3; setTier(t); }
  };
  onMount(() => {
    family = getComputedStyle(svg).fontFamily || family;
    apply();
    measure();
    if (!CANVAS.w) settle();
    fitBar();
    document.addEventListener("keydown", onKey, true);
    canvas.addEventListener("wheel", onWheel, { passive: false });
    const sizes = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => { measure(); fitBar(); });
    sizes?.observe(canvas); const bar = root.querySelector(".gv-bar"); if (bar) sizes?.observe(bar);
    props.api?.({ fit: fitAll });
    onCleanup(() => {
      sizes?.disconnect();
      document.removeEventListener("keydown", onKey, true);
      canvas.removeEventListener("wheel", onWheel);
      if (raf) cancelAnimationFrame(raf);
      clearTimeout(settleTimer); clearTimeout(tipTimer); clearTimeout(typed);
      props.memory.set(props.memoryKey, { z: VW.z, x: VW.x, y: VW.y, fit: fitLocked, collapsed: [...collapsed()] });
    });
  });

  // ---------- Pieces ----------
  const ids = { status: `${uid}-status`, layers: `${uid}-layers`, zoom: `${uid}-zoom`, more: `${uid}-more` };
  const openFrom = (id: string, trigger: HTMLElement | null) => {
    const pop = document.getElementById(id) as HTMLElement | null; if (!pop || !trigger) return;
    pop.showPopover?.(); place(pop, trigger, "left");
    pop.querySelector<HTMLElement>("input, button")?.focus({ preventScroll: true });
  };
  const moreButton = () => root.querySelector<HTMLElement>(".t-more");
  const counts = () => statusCounts(statusOf());
  const countText = () => {
    const m = matches();
    if (m === null) return "";
    if (!m.length) return "No matches";
    return query().trim() ? matchCount(cur(), m.length) : `${m.length} of ${n()}`;
  };
  // Previous and Next keep their place while nothing is searched, and once a search finds nothing the count beside them says why.
  const stepWhy = () => matches()?.length === 0 ? "No tasks match" : undefined;
  const pickedRow = () => props.picked ? rowOf().get(props.picked) : undefined;
  const pickedView = createMemo(() => pickedRow()?.view ?? null, null, { equals: (a, b) => a?.id === b?.id && a?.machine.id === b?.machine.id });
  const startable = createMemo(() => { const r = pickedRow(); return props.manual && r?.view?.task.status === "draft" && r.waits.length === 0; });
  const reqShown = createMemo(() => props.request.shown), reqWhy = createMemo(() => props.request.why);
  const kind = createMemo(() => matches()?.length === 0 ? "none" : props.picked && byKey().has(props.picked) ? "pick" : props.mode === "draft" ? "draft" : "counts");
  const Icon = (p: { name: string; x: string; y: string; size: string }) => <use class="ic" href={`#${uid}-i-${iconOf(p.name)}`} x={p.x} y={p.y} width={p.size} height={p.size} />;

  const bar = <Toolbar small class="gv-bar" label="Graph controls"
    start={<>
      <Show when={props.full}><button class="btn btn-sm btn-line" onClick={() => props.onFull(false)}>Exit full view</button><span class="gv-title">{props.title}</span></Show>
      <Show when={hasSearch()}>
        <div class="gv-search"><I n="search" s={14} /><input ref={search} class="inp" type="text" placeholder="Search tasks" aria-label="Search tasks in the graph" spellcheck={false} autocomplete="off"
          onInput={e => { const v = e.currentTarget.value; clearTimeout(typed); typed = setTimeout(() => setQuery(v), 80); }}
          on:keydown={e => {
            if (e.key === "Enter") { e.preventDefault(); clearTimeout(typed); if (query() !== search.value) setQuery(search.value); step(e.shiftKey ? -1 : 1); }
            else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); if (search.value) clearSearch(); else { document.documentElement.toggleAttribute("data-keyboard", true); focusId(focus() ?? firstVertex()); } }
          }} /></div>
        <span id={`${uid}-count`} class="gv-count">{countText()}</span>
        <For each={[[-1, "Previous match", "arrow-left"], [1, "Next match", "arrow-right"]] as const}>{([dir, name, icon]) =>
          <button class="ib" classList={{ off: matches() === null }} aria-hidden={matches() === null ? "true" : undefined} tabindex={matches() === null ? -1 : 0} aria-label={name}
            aria-disabled={stepWhy() ? "true" : undefined} data-why={stepWhy()} aria-describedby={stepWhy() ? `${uid}-count` : undefined} onClick={() => step(dir)}><I n={icon} s={14} /></button>}</For>
        <button class="btn btn-sm gv-fitm" classList={{ off: !matches()?.length }} aria-hidden={matches()?.length ? undefined : "true"} tabindex={matches()?.length ? 0 : -1} onClick={fitMatches}>Fit matches</button>
        <Show when={hasStatus() || hasLayers()}><span class="vr t-tier2" /></Show>
      </Show>
      <Show when={hasStatus()}><button class="chip t-tier3" popovertarget={ids.status}>{filter().size ? `Status · ${filter().size}` : "Status"}<I n="chevron-down" s={12} /></button></Show>
      <Show when={hasLayers()}><button class="chip t-tier2" popovertarget={ids.layers}>Layers<I n="chevron-down" s={12} /></button></Show>
    </>}
    end={<>
      <button class="ib" aria-label="Zoom out" onClick={() => setZoom(VW.z / ZOOM_STEP)}><I n="minus" s={14} /></button>
      <button class="btn btn-sm gv-zoom" popovertarget={ids.zoom} aria-label="Zoom presets"><span ref={pct}>100%</span></button>
      <button class="ib" aria-label="Zoom in" onClick={() => setZoom(VW.z * ZOOM_STEP)}><I n="plus" s={14} /></button>
      <button class="btn btn-sm" onClick={fitAll}>Fit</button>
      <Show when={hasMap()}><button class="ib gv-tog t-tier2" aria-label="Minimap" aria-pressed={mapWanted(ws.panels.minimap(), canvasW())} onClick={toggleMap}><I n="board" s={14} /></button></Show>
      <Show when={!props.full}><button class="btn btn-sm" onClick={toggleFull}><I n="expand" s={14} />Full view</button></Show>
      <Show when={tier() > 0 && (hasStatus() || hasLayers() || hasMap())}><button class="ib t-more" aria-label="More graph controls" popovertarget={ids.more}><I n="more" s={14} /></button></Show>
    </>} />;

  const RequestButton = () => <Show when={reqShown()}><button class="btn btn-line btn-sm" aria-disabled={reqWhy() ? "true" : undefined} data-why={reqWhy() ?? undefined}
    onClick={() => { const why = reqWhy(); if (why) announce(`${why}.`); else props.request.run(); }}>Request changes</button></Show>;
  const pickLine = () => { const k = props.picked!; return selectionLine(adj().no.get(k) ?? 0, numbers(adj().needs.get(k) ?? []), numbers(adj().unblocks.get(k) ?? [])); };
  // One bar for each state, kept while its state lasts, so a machine read rebuilds none of it.
  const band = <Switch>
    <Match when={kind() === "none"}><ActionBar class="gv-band" label="Graph status" start={<span class="tx">{noMatchText(query(), filter().size > 0)}</span>}>
      <Show when={query().trim()}><button class="btn btn-sm btn-line" onClick={clearSearch}>Clear search</button></Show>
      <Show when={filter().size}><button class="btn btn-sm btn-line" onClick={() => setFilter(new Set())}>Clear status</button></Show>
    </ActionBar></Match>
    <Match when={kind() === "pick"}><ActionBar class="gv-band" label="Graph status" start={<><span class="tx">{pickLine()}</span><span class="trunc muted gv-ti">{graphTitle(byKey().get(props.picked!) ?? { key: "", description: "", dependsOn: [] })}</span></>}>
      <div class="seg" role="group" aria-label="Links shown">
        <button aria-current={chain() ? "false" : "true"} onClick={() => ws.panels.setLinks("direct")}>Direct</button>
        <button aria-current={chain() ? "true" : "false"} onClick={() => ws.panels.setLinks("chain")}>Whole chain</button>
      </div>
      <Show when={pickedView()}>{v => <>
        <Show when={startable()}><button class="btn btn-line btn-sm" {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })}>Start</button></Show>
        <A class="btn btn-line btn-sm" to={{ name: "task", machine: v().machine.id, task: v().id }}>Open task</A>
      </>}</Show>
      <button class="btn btn-line btn-sm" onClick={() => { if (props.picked) props.onDetails(props.picked); }}>Details</button>
      <RequestButton />
    </ActionBar></Match>
    <Match when={kind() === "draft"}><ActionBar class="gv-band" label="Graph status" start={<span class="tx">Change titles and dependencies in List.</span>}><button class="btn btn-line btn-sm" onClick={() => props.onOpenList()}>Open list</button></ActionBar></Match>
    <Match when={kind() === "counts"}><ActionBar class="gv-band" label="Graph status" start={<span class="tx">{graphCounts(n(), g().layers.length)}</span>} hint="Every task waits for your final confirmation."><RequestButton /></ActionBar></Match>
  </Switch>;

  return <div ref={root} class="gv" data-tier={tier()}>
    {bar}
    <div ref={canvas} class="gv-canvas" onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerLeave={hideTip}>
      <div ref={layer} class="gv-layer"><svg ref={svg} class="gsv" width={Math.max(1, Math.ceil(g().width * drawn()))} height={Math.max(1, Math.ceil(g().height * drawn()))} role="group" aria-roledescription="dependency graph" aria-label={graphLabel(n())} data-lod={level()} data-low={low() ? "" : undefined} data-ni={level() === "number" && view().z >= numberIconZoom ? "" : undefined}
        onFocusIn={e => { const el = (e.target as Element).closest?.<SVGGElement>(".n, .band"); if (!el) return; setFocus(el.dataset.id!); if (document.documentElement.hasAttribute("data-keyboard")) { reveal(el.dataset.id!, 48); showTip(el.dataset.id!); } }}
        onFocusOut={e => { if (!(e.relatedTarget instanceof Node && svg.contains(e.relatedTarget))) hideTip(); }}>
        <defs>
          <For each={["var(--line-ctl)", "var(--fg)", "var(--line)"]}>{(fill, i) =>
            <marker id={`${uid}-a${i()}`} viewBox="0 0 8 8" refX="7.5" refY="4" markerUnits="userSpaceOnUse" markerWidth={8 / unit()} markerHeight={8 / unit()} orient="auto"><path d="M0 0L8 4L0 8z" style={{ fill }} /></marker>}</For>
          <For each={Object.keys(ICONS)}>{name => <symbol id={`${uid}-i-${name}`} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d={ICONS[name]} /></symbol>}</For>
        </defs>
        <g ref={world} transform={`scale(${untrack(drawn)})`} style={{ "font-size": `${12 / unit()}px` }}>
          <g class="eg"><Keyed each={g().edges} by={e => e.id}>{e => <>
            <path class={`e ${edgeRole().get(e().id) ?? ""}`} data-e={e().id} d={e().d} marker-end={marker(edgeRole().get(e().id) ?? "")} />
            <Show when={e().count > 1}><text class="ec" x={e().labelAt.x} y={e().labelAt.y}>x{e().count}</text></Show>
          </>}</Keyed></g>
          <Show when={hasLayers()}><g class="lg"><Keyed each={g().layers.filter(l => !l.collapsed)} by={l => String(l.layer)}>{l =>
            <g class="lh" data-l={l().layer} transform={`translate(${l().x} ${l().top})`}>
              <rect width={l().w} height="24" fill="transparent" />
              <text class="ht1" y="20">{layerHeading(l().layer, l().keys.length)}</text><text class="ht2" y="20">Layer {l().layer + 1}</text>
              <use class="ic" href={`#${uid}-i-down`} width={12 / unit()} height={12 / unit()} y={22 - 12 / unit()} x={l().w - 12 / unit()} />
            </g>}</Keyed></g></Show>
          <g class="ng">
            <Keyed each={g().tasks} by={t => t.id}>{t => {
              const key = t().key, st = () => stateOf(key), no = () => adj().no.get(key) ?? 0, words = () => labels().get(key);
              // The id is the row's key, read once: a row being disposed must not read the list it leaves.
              const id = t().id;
              return <g ref={el => { els.set(id, el); onCleanup(() => { if (els.get(id) === el) els.delete(id); }); }} class={nodeClass().get(key) ?? "n"} data-id={t().id} data-f={st()?.filter ?? ""} data-t={st()?.tone ?? ""}
                tabindex={focus() === t().id ? 0 : -1} role="button" aria-pressed={props.picked === key ? "true" : "false"} aria-label={nodeLabel(key)} transform={`translate(${t().x} ${t().y})`}>
                <rect class="bx" width={NW} height={NH} rx="6" />
                <g transform={`translate(0 ${NH / 2})`}>
                  <text class="l1" x=".75em" y="-.25em"><tspan class="no">{no()}</tspan> {words()?.l1 ?? ""}</text>
                  <g class="l2"><Show when={st()}>{s => <Icon name={s().icon} x=".75em" y=".2em" size="1em" />}</Show><text class="w" x={st() ? "2.1em" : ".82em"} y="1.12em">{words()?.w ?? ""}</text></g>
                  <g class="ls"><Show when={st()}>{s => <Icon name={s().icon} x=".75em" y="-.5em" size="1em" />}</Show><text class="ts" x={st() ? "2.1em" : ".75em"} y=".35em"><tspan class="no">{no()}</tspan> {words()?.ts ?? ""}</text></g>
                  <g class="ln"><Show when={st()}>{s => <Icon name={s().icon} x=".6em" y="-.46em" size=".92em" />}</Show><text class="num" x={NW / 2} y=".35em" text-anchor="middle">{no()}</text></g>
                </g>
              </g>;
            }}</Keyed>
            <Keyed each={g().bands} by={b => b.id}>{b => { const id = b().id; return (
              <g ref={el => { els.set(id, el); onCleanup(() => { if (els.get(id) === el) els.delete(id); }); }} class={bandClass().get(b().id) ?? "band"} data-id={b().id} data-l={b().layer}
                tabindex={focus() === b().id ? 0 : -1} role="button" aria-expanded="false" aria-label={bandLabel(b().layer, b().keys)} transform={`translate(${b().x} ${b().y})`}>
                <rect class="bx" width={BANDW} height={BANDH} rx="6" />
                <For each={bandRows(b().layer, b().keys)}>{(r, i) => low()
                  ? <text class={r.c} x={BANDW / 2} y={BANDH / 2} text-anchor="middle" dominant-baseline="middle">{r.t}</text>
                  : <text class={r.c} x=".7em" y={`${1.5 + i() * 1.33}em`}>{r.t}</text>}</For>
                <Show when={!low()}><use class="ic" href={`#${uid}-i-right`} width={12 / unit()} height={12 / unit()} x={BANDW - 4 / unit() - 12 / unit()} y={6 / unit()} /></Show>
              </g>); }}</Keyed>
          </g>
          <g class="pins"><For each={pins()}>{p => <g class="pin" transform={`translate(${p.x} ${p.y})`}><rect width={p.w / p.u} height={p.h / p.u} rx={4 / p.u} /><text x={5 / p.u} y={13 / p.u}>{p.text}</text></g>}</For></g>
          <Show when={focus() ? g().boxes.get(focus()!) : undefined}>{b => { const o = () => 2 / unit(); return <rect class="ring" rx="8" x={b().x - o()} y={b().y - o()} width={b().w + 2 * o()} height={b().h + 2 * o()} />; }}</Show>
        </g>
      </svg></div>
      <div ref={map} class="gv-map" tabindex="0" role="group" aria-label="Minimap. Arrow keys move the view." hidden={!mapShown()}
        onPointerDown={e => { e.stopPropagation(); mapDrag = e.pointerId; try { e.currentTarget.setPointerCapture?.(e.pointerId); } catch { /* synthetic */ } mapTo(e, e.currentTarget); }}
        onPointerMove={e => { if (mapDrag !== null) mapTo(e, e.currentTarget); }} onPointerUp={() => { mapDrag = null; }}
        on:keydown={e => { const by: Record<string, [number, number]> = { ArrowLeft: [80, 0], ArrowRight: [-80, 0], ArrowUp: [0, 80], ArrowDown: [0, -80] }; if (by[e.key]) { e.preventDefault(); e.stopPropagation(); pan(...by[e.key]); } }}>
        <svg viewBox={`0 0 ${MAP.iw} ${MAP.ih}`} preserveAspectRatio="none" aria-hidden="true">
          <For each={["m0", "m-needs", "m-problem", "m-unknown"] as const}>{k => <path class={k} d={mini().paths[k]} />}</For>
          <path class="m-match" d={(matches() ?? []).map(k => g().boxes.get(g().vertexOf.get(k) ?? "")).filter((b): b is Box => !!b).map(b => mini().rect(b, 3)).join("")} />
          <rect ref={mv} class="mv" />
        </svg>
      </div>
      <Show when={tip() && tipText()}>{_ => <div ref={tipEl} class="gv-tip" aria-hidden="true" style={{ left: `${tip()!.left}px`, top: `${tip()!.top}px` }}>{tipText()!.head}<span class="s">{tipText()!.sub}</span></div>}</Show>
    </div>
    {band}
    <div class="ws-live" role="status" aria-live="polite" aria-atomic="true">{said()}</div>

    <Pop id={ids.status} class="gv-pop" align="left" label="Show tasks by status">
      <div class="mi-h">Show tasks by status</div>
      <For each={counts()}>{c => <label class="mi"><input type="checkbox" class="ws-ck" checked={filter().has(c.id)} onChange={e => toggleFilter(c.id, e.currentTarget.checked)} /><I n={c.icon} s={14} /><span class="grow">{c.label}</span><span class="ct">{c.n}</span></label>}</For>
      <div class="mi-sep" />
      <button class="mi" onClick={() => setFilter(new Set())}><span class="grow">Clear status</span></button>
    </Pop>
    <Pop id={ids.layers} class="gv-pop gv-layers" align="left" label="Dependency layers">
      <div class="mi-h">Dependency layers</div>
      <div class="gv-pop-scroll"><For each={g().layers}>{l => <label class="mi"><input type="checkbox" class="ws-ck" checked={!l.collapsed} onChange={e => setColl(l.layer, !e.currentTarget.checked)} /><span class="grow">Layer {l.layer + 1}</span><span class="ct">{l.keys.length} tasks</span></label>}</For></div>
      <div class="mi-sep" />
      <button class="mi" onClick={() => setAll(() => true)}><span class="grow">Collapse all</span></button>
      <button class="mi" onClick={() => setAll(() => false)}><span class="grow">Expand all</span></button>
      <Show when={states()}><button class="mi" onClick={() => { const done = new Set(finishedLayers(g().layers, statusOf())); setAll(l => done.has(l)); }}><span class="grow">Collapse finished layers</span></button></Show>
    </Pop>
    <Pop id={ids.zoom} class="gv-pop" align="right" label="Zoom presets">
      <MenuItem label="Fit" keys="F" onSelect={fitAll} />
      <MenuItem label="Fit matches" keys="Shift+F" disabled={matches()?.length ? undefined : "Nothing matches yet"} onSelect={fitMatches} />
      <MenuItem label="100%" keys="0" onSelect={() => setZoom(1)} />
      <MenuItem label="75%" onSelect={() => setZoom(0.75)} />
      <MenuItem label="50%" onSelect={() => setZoom(0.5)} />
    </Pop>
    <Pop id={ids.more} class="gv-pop" align="right" label="More graph controls">
      <Show when={hasStatus() && tier() === 3}><MenuItem label="Status" onSelect={() => openFrom(ids.status, moreButton())} /></Show>
      <Show when={hasLayers()}><MenuItem label="Layers" onSelect={() => openFrom(ids.layers, moreButton())} /></Show>
      <Show when={hasMap()}><MenuItem label="Minimap" keys="M" check={mapWanted(ws.panels.minimap(), canvasW())} onSelect={toggleMap} /></Show>
    </Pop>
  </div>;
}
