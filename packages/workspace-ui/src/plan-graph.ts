import type { PlanItem } from "@cortex/workspace-domain";

/*
 * The large dependency graph of a sprint plan, as pure data. Layout depends only on plan keys and dependencies:
 * states, zoom, search and filters never move a task, so the same plan always draws the same picture.
 * Sizes are world pixels at 100%.
 */
export const GRAPH = { nodeW: 200, nodeH: 44, layerGap: 48, row: 56, passLine: 14, bandW: 120, bandH: 160, head: 28, channel: 72, pad: 28, laneGap: 16, margin: 24 } as const;
const { nodeW: NW, nodeH: NH, layerGap: GAP, row: ROW, passLine: DUMMY, bandW: BANDW, bandH: BANDH, head: HEAD, channel: CHAN, pad: PAD, laneGap: LANEGAP } = GRAPH;
/** Band wrapping is chosen for this canvas, so resizing the window never reshuffles the graph. */
const REFERENCE = { width: 1400, height: 744 };

export type Box = { x: number; y: number; w: number; h: number };
/** A task box. `id` is its vertex id, `t:<key>`; `no` is its 1-based plan number. */
export type GraphTask = Box & { id: string; key: string; no: number; layer: number; lane: number };
/** A collapsed layer drawn as one box, `b:<layer>`. */
export type GraphBand = Box & { id: string; layer: number; keys: string[] };
/**
 * One drawn edge between visible vertices, from the dependency side to the dependent side. `real` lists the plan
 * dependencies it stands for as [dependency key, dependent key]; `count` above 1 means merged edges (show `x3`
 * at `labelAt`). `back` marks an edge a dependency cycle turned backwards.
 */
export type GraphEdge = { id: string; from: string; to: string; real: [string, string][]; count: number; d: string; labelAt: { x: number; y: number }; back: boolean };
/** A layer column. `layer` is 0-based; people read it as `Layer ${layer + 1}`. `band` is the wrapped row it sits in. */
export type GraphLayer = { layer: number; x: number; top: number; w: number; keys: string[]; collapsed: boolean; lanes: number; band: number };
export type GraphLayout = {
  tasks: GraphTask[]; bands: GraphBand[]; edges: GraphEdge[]; layers: GraphLayer[];
  width: number; height: number; bandCount: number;
  /** Plan key to 0-based layer. */
  layerOf: Map<string, number>;
  /** Plan key to the id of the vertex that shows it: its own box, or its collapsed layer. */
  vertexOf: Map<string, string>;
  /** Vertex id to its box, for tasks and collapsed layers. */
  boxes: Map<string, Box>;
  /** Edge crossings with tasks in plan order, and after ordering. */
  crossings: { planOrder: number; swept: number };
};

type Vert = { id: string; kind: "task" | "band" | "dummy"; layer: number; num: number; key: string | null; keys: string[] | null; w: number; h: number; x: number; y: number; lane: number };
type Col = { L: number; lanes: number; width: number; rows: number; tasks: Vert[]; dummies: Vert[]; band: Vert | null; estH: number; x: number; top: number };

const round = (n: number) => Math.round(n * 10) / 10;
const median = (values: number[]) => { const a = [...values].sort((x, y) => x - y), m = a.length >> 1; return a.length % 2 ? a[m] : (a[m - 1] + a[m]) / 2; };
const firstOf = (items: readonly PlanItem[]) => { const out: PlanItem[] = [], seen = new Set<string>(); for (const i of items) if (!seen.has(i.key)) { seen.add(i.key); out.push(i); } return out; };

/** A cheap signature of what layout reads, for memoizing it: keys and dependencies, never titles. */
export const topologyKey = (items: readonly PlanItem[]) => JSON.stringify(items.map(i => [i.key, i.dependsOn]));

/**
 * Plan key to 0-based layer by longest dependency path: a task sits one layer past its last dependency. The cycle
 * guard is today's: a dependency already on the current path counts as layer 0. Unknown dependency keys are ignored,
 * and layers a cycle leaves empty are closed up.
 */
export function planLayers(items: readonly PlanItem[]): Map<string, number> {
  const byKey = new Map(firstOf(items).map(i => [i.key, i]));
  const depth = new Map<string, number>(), path = new Set<string>();
  const visit = (key: string): number => {
    const known = depth.get(key);
    if (known !== undefined) return known;
    if (path.has(key)) return 0;
    path.add(key);
    let d = 0;
    for (const dep of byKey.get(key)!.dependsOn) if (byKey.has(dep)) d = Math.max(d, visit(dep) + 1);
    path.delete(key);
    depth.set(key, d);
    return d;
  };
  for (const key of byKey.keys()) visit(key);
  const used = new Map([...new Set(depth.values())].sort((a, b) => a - b).map((d, i) => [d, i]));
  for (const [key, d] of depth) depth.set(key, used.get(d)!);
  return depth;
}

/** Inversions between two adjacent layers, counted with a Fenwick tree. Edges sharing an end never cross. */
function crossBetween(upper: Vert[], lowerSize: number, down: Map<string, string[]>, pos: Map<string, number>) {
  const tree = new Array<number>(lowerSize + 1).fill(0);
  let seen = 0, count = 0;
  for (const v of upper) {
    const ends = down.get(v.id)!.map(w => pos.get(w)!).sort((a, b) => a - b);
    for (const p of ends) { let s = 0; for (let i = p + 1; i > 0; i -= i & -i) s += tree[i]; count += seen - s; }
    for (const p of ends) { for (let i = p + 1; i <= lowerSize; i += i & -i) tree[i]++; seen++; }
  }
  return count;
}

/**
 * The whole drawing. `collapsed` holds 0-based layers shown as one band each, with their edges merged and counted.
 * Small plans keep their intrinsic size: lanes appear only in a layer of more than 12 tasks, wrapping only when it
 * improves the fit, and a one-task plan is a single 200x44 box at the top left.
 */
export function layout(input: readonly PlanItem[], collapsed: Iterable<number> = []): GraphLayout {
  const items = firstOf(input), N = items.length;
  const no = new Map(items.map((i, n) => [i.key, n + 1]));
  const layerOf = planLayers(items);
  if (!N) return { tasks: [], bands: [], edges: [], layers: [], width: 0, height: 0, bandCount: 0, layerOf, vertexOf: new Map(), boxes: new Map(), crossings: { planOrder: 0, swept: 0 } };
  let nL = 0;
  for (const d of layerOf.values()) nL = Math.max(nL, d + 1);
  const layerKeys: string[][] = Array.from({ length: nL }, () => []);
  for (const i of items) layerKeys[layerOf.get(i.key)!].push(i.key);
  const fold = new Set([...collapsed].filter(L => Number.isInteger(L) && L >= 0 && L < nL));

  const verts = new Map<string, Vert>();
  const vert = (id: string, kind: Vert["kind"], layer: number, num: number, w: number, h: number, key: string | null = null): Vert => {
    const v: Vert = { id, kind, layer, num, key, keys: null, w, h, x: 0, y: 0, lane: 0 };
    verts.set(id, v);
    return v;
  };
  let V: Vert[][] = layerKeys.map((keys, L) => {
    if (!fold.has(L)) return keys.map(k => vert(`t:${k}`, "task", L, no.get(k)!, NW, NH, k));
    const b = vert(`b:${L}`, "band", L, no.get(keys[0])!, BANDW, BANDH);
    b.keys = keys;
    return [b];
  });
  const vertexOf = new Map(items.map(i => { const L = layerOf.get(i.key)!; return [i.key, fold.has(L) ? `b:${L}` : `t:${i.key}`]; }));

  // Edges between visible vertices, merged in plan order.
  type Agg = { u: string; v: string; real: [string, string][] };
  const aggs: Agg[] = [], aggOf = new Map<string, Map<string, Agg>>();
  for (const item of items) for (const dep of new Set(item.dependsOn)) {
    if (!no.has(dep)) continue;
    const u = vertexOf.get(dep)!, v = vertexOf.get(item.key)!;
    if (u === v) continue;
    let row = aggOf.get(u);
    if (!row) aggOf.set(u, row = new Map());
    let agg = row.get(v);
    if (!agg) { row.set(v, agg = { u, v, real: [] }); aggs.push(agg); }
    agg.real.push([dep, item.key]);
  }
  // A long edge gets a hidden point in every layer it passes, so it counts in crossings and routes around tasks.
  const chains = aggs.map((e, i) => {
    const a = verts.get(e.u)!, b = verts.get(e.v)!, chain = [a];
    if (b.layer > a.layer) for (let l = a.layer + 1; l < b.layer; l++) { const d = vert(`d${i}_${l}`, "dummy", l, a.num, 0, 2); V[l].push(d); chain.push(d); }
    chain.push(b);
    return chain;
  });
  const up = new Map<string, string[]>(), down = new Map<string, string[]>();
  for (const id of verts.keys()) { up.set(id, []); down.set(id, []); }
  for (const c of chains) if (c[c.length - 1].layer > c[0].layer) for (let i = 1; i < c.length; i++) { down.get(c[i - 1].id)!.push(c[i].id); up.get(c[i].id)!.push(c[i - 1].id); }

  // Order inside layers: 8 alternating median sweeps, ties by plan number, keeping the best.
  const pos = new Map<string, number>();
  const index = () => { for (const col of V) col.forEach((v, i) => pos.set(v.id, i)); };
  const crossings = () => { let c = 0; for (let L = 0; L + 1 < V.length; L++) c += crossBetween(V[L], V[L + 1].length, down, pos); return c; };
  const sortCol = (L: number, nb: Map<string, string[]>) => {
    const keyed = V[L].map((v, i) => { const ns = nb.get(v.id)!; return { v, k: ns.length ? median(ns.map(x => pos.get(x)!)) : i }; });
    keyed.sort((a, b) => a.k - b.k || a.v.num - b.v.num);
    V[L] = keyed.map(x => x.v);
    V[L].forEach((v, i) => pos.set(v.id, i));
  };
  index();
  const planOrder = crossings();
  let best = V.map(c => c.slice()), bestC = planOrder;
  for (let s = 0; s < 8; s++) {
    if (s % 2 === 0) for (let L = 1; L < V.length; L++) sortCol(L, up);
    else for (let L = V.length - 2; L >= 0; L--) sortCol(L, down);
    const c = crossings();
    if (c < bestC) { bestC = c; best = V.map(col => col.slice()); }
  }
  V = best;
  index();

  // Tall layers split into lanes, filled row by row so the order holds.
  const R = Math.max(12, Math.round(Math.sqrt(N * 0.5625 * 248 / 56)));
  const cols: Col[] = V.map((col, L) => {
    const tasks = col.filter(v => v.kind === "task"), band = col.find(v => v.kind === "band") ?? null, dummies = col.filter(v => v.kind === "dummy");
    const lanes = !band && tasks.length > R ? Math.min(8, Math.ceil(tasks.length / R)) : 1;
    const width = band ? BANDW : lanes * NW + (lanes - 1) * LANEGAP, rows = lanes > 1 ? Math.ceil(tasks.length / lanes) : 0;
    const laneCap = lanes > 1 ? 2 * Math.max(0, rows - 1) : 0;
    const estH = band ? BANDH + dummies.length * DUMMY : lanes > 1 ? rows * ROW + Math.max(0, dummies.length - laneCap) * DUMMY : tasks.length * ROW + dummies.length * DUMMY;
    return { L, lanes, width, rows, tasks, dummies, band, estH, x: 0, top: 0 };
  });

  // Wide graphs wrap into bands like lines of text: the fewest bands whose Fit is within 95% of the best.
  const partition = (k: number) => {
    const total = cols.reduce((s, c) => s + c.width, 0) + GAP * (cols.length - 1);
    let cap = Math.max(total / k, ...cols.map(c => c.width));
    for (;;) {
      const res: Col[][] = [[]];
      let cur = -GAP;
      for (const c of cols) {
        if (res[res.length - 1].length && cur + GAP + c.width > cap) { res.push([]); cur = -GAP; }
        res[res.length - 1].push(c);
        cur += GAP + c.width;
      }
      if (res.length <= k) return res;
      cap += 16;
    }
  };
  const fitOf = (bs: Col[][]) => {
    let w = 0, h = 0;
    bs.forEach((b, i) => {
      w = Math.max(w, b.reduce((t, c) => t + c.width, 0) + GAP * (b.length - 1));
      h += HEAD + Math.max(...b.map(c => c.estH)) + (i ? CHAN : 0);
    });
    return Math.min(1, (REFERENCE.width - 48) / (w + 2 * PAD), (REFERENCE.height - 48) / h);
  };
  const candidates: { bs: Col[][]; f: number }[] = [];
  let bestFit = 0;
  for (let k = 1; k <= Math.min(20, cols.length); k++) { const bs = partition(k), f = fitOf(bs); candidates.push({ bs, f }); bestFit = Math.max(bestFit, f); }
  const bands = candidates.find(c => c.f >= 0.95 * bestFit)!.bs;
  const bandOf = new Map<number, number>();
  bands.forEach((b, bi) => b.forEach(c => bandOf.set(c.L, bi)));
  const colOf = new Map(cols.map(c => [c.L, c]));

  // Coordinates: stack each column, then align tasks to the median of their neighbours in 3 passes.
  for (const c of cols) {
    let y = 0;
    if (c.band) { c.band.y = 0; y = BANDH + 12; for (const d of c.dummies) { d.y = y; y += DUMMY; } continue; }
    if (c.lanes > 1) {
      c.tasks.forEach((v, i) => { v.lane = i % c.lanes; v.y = Math.floor(i / c.lanes) * ROW; });
      // Passing lines run through the row gutters, two per gutter; only overflow goes below the block.
      const cap = 2 * Math.max(0, c.rows - 1), dn = c.dummies.length, used = new Map<number, number>();
      y = c.rows * ROW;
      c.dummies.forEach((d, i) => {
        if (i < cap) {
          let gut = Math.min(c.rows - 2, Math.floor(i * (c.rows - 1) / Math.min(dn, cap)));
          while ((used.get(gut) ?? 0) >= 2) gut = (gut + 1) % (c.rows - 1);
          const slot = used.get(gut) ?? 0;
          used.set(gut, slot + 1);
          d.y = (gut + 1) * ROW - 6 + (slot ? 3 : -3) - 1;
        } else { d.y = y; y += DUMMY; }
      });
      continue;
    }
    for (const v of V[c.L]) { v.lane = 0; v.y = y; y += v.kind === "task" ? ROW : DUMMY; }
  }
  const center = (id: string) => { const v = verts.get(id)!; return v.y + v.h / 2; };
  const sameBand = (L: number, id: string) => bandOf.get(verts.get(id)!.layer) === bandOf.get(L);
  const place = (c: Col, nb: Map<string, string[]>) => {
    if (c.band) return;
    if (c.lanes > 1) {
      const cs: number[] = [];
      for (const v of c.tasks) for (const id of nb.get(v.id)!) if (sameBand(c.L, id)) cs.push(center(id));
      if (!cs.length) return;
      const dy = median(cs) - (c.tasks[0].y + (c.rows * ROW - 12) / 2);
      for (const v of c.tasks) v.y += dy;
      for (const d of c.dummies) d.y += dy;
      return;
    }
    const col = V[c.L];
    const want = col.map(v => { const ns = nb.get(v.id)!.filter(id => sameBand(c.L, id)); return (ns.length ? median(ns.map(center)) : v.y + v.h / 2) - v.h / 2; });
    const dn = want.slice(), upv = want.slice();
    for (let i = 1; i < col.length; i++) dn[i] = Math.max(dn[i], dn[i - 1] + col[i - 1].h + 12);
    for (let i = col.length - 2; i >= 0; i--) upv[i] = Math.min(upv[i], upv[i + 1] - col[i].h - 12);
    col.forEach((v, j) => { v.y = (dn[j] + upv[j]) / 2; });
  };
  for (let p = 0; p < 3; p++) {
    if (p % 2 === 0) for (const c of cols) place(c, up);
    else for (let l = cols.length - 1; l >= 0; l--) place(cols[l], down);
  }
  // A column spread past 1.1 times its natural height closes up around its centre, so Fit is not spent on space.
  for (const c of cols) {
    if (c.band || c.lanes > 1) continue;
    const col = V[c.L];
    if (col.length < 2) continue;
    let natural = 0;
    for (let i = 0; i + 1 < col.length; i++) natural += col[i].h + 12;
    const first = col[0].y, last = col[col.length - 1].y;
    if (last - first <= natural * 1.1) continue;
    let y = (first + last) / 2 - natural / 2;
    for (const v of col) { v.y = y; y += v.h + 12; }
  }
  const tops: { top: number; bottom: number }[] = [];
  let top = 0, width = 0;
  for (const b of bands) {
    let minY = Infinity, maxY = -Infinity;
    for (const c of b) for (const v of V[c.L]) { minY = Math.min(minY, v.y); maxY = Math.max(maxY, v.y + v.h); }
    let x = PAD;
    for (const c of b) {
      c.x = x; c.top = top;
      for (const v of V[c.L]) {
        v.y = v.y - minY + top + HEAD;
        if (v.kind === "dummy") { v.x = x; v.w = c.width; }
        else v.x = x + (v.kind === "task" && c.lanes > 1 ? v.lane * (NW + LANEGAP) : 0);
      }
      x += c.width + GAP;
    }
    width = Math.max(width, x - GAP + PAD);
    const bottom = top + HEAD + (maxY - minY);
    tops.push({ top, bottom });
    top = bottom + CHAN;
  }
  const height = top - CHAN + 8;

  // Routes: today's curve between adjacent layers, hidden points for long edges, row gutters out of lanes, and the
  // channel between bands (up to 8 tracks, 8 px apart).
  const trackN = new Map<number, number>();
  const edges: GraphEdge[] = chains.map((ch, ei) => {
    const e = aggs[ei], a = ch[0], last = ch[ch.length - 1], col = colOf.get(a.layer)!;
    const base = { id: `${e.u}>${e.v}`, from: e.u, to: e.v, real: e.real, count: e.real.length };
    if (last.layer <= a.layer) {
      const x1 = a.x + a.w, y1 = a.y + a.h / 2, x2 = last.x, y2 = last.y + last.h / 2, mx = (x1 + x2) / 2;
      return { ...base, d: `M${round(x1)} ${round(y1)}C${round(mx)} ${round(y1)} ${round(mx)} ${round(y2)} ${round(x2)} ${round(y2)}`, labelAt: { x: round(x1 + 10), y: round(y1 - 4) }, back: true };
    }
    let path: string, px: number, py: number;
    if (a.kind === "task" && col.lanes > 1 && a.lane < col.lanes - 1) {
      const gy = a.y + NH + 6, right = col.x + col.width, sx = a.x + NW;
      path = `M${round(sx)} ${round(a.y + NH / 2)}L${round(sx + 8)} ${round(a.y + NH / 2)}L${round(sx + 8)} ${round(gy)}L${round(right)} ${round(gy)}`;
      px = right; py = gy;
    } else { px = a.x + a.w; py = a.y + a.h / 2; path = `M${round(px)} ${round(py)}`; }
    const labelAt = { x: round(px + 10), y: round(py - 4) };
    for (let i = 1; i < ch.length; i++) {
      const pv = ch[i - 1], b = ch[i], cl = colOf.get(b.layer)!;
      let qx: number, qy: number, tail = "";
      const lane = b.kind === "task" && cl.lanes > 1 && b.lane > 0;
      if (b.kind === "dummy") { qx = b.x; qy = b.y + 1; tail = `L${round(b.x + b.w)} ${round(qy)}`; }
      else if (lane) { qy = b.y - 6; qx = cl.x; tail = `L${round(b.x - 8)} ${round(qy)}L${round(b.x - 8)} ${round(b.y + NH / 2)}L${round(b.x)} ${round(b.y + NH / 2)}`; }
      else { qx = b.x; qy = b.y + b.h / 2; }
      if (bandOf.get(pv.layer) !== bandOf.get(b.layer)) {
        const bi = bandOf.get(pv.layer)!, t = trackN.get(bi) ?? 0;
        trackN.set(bi, t + 1);
        const cy = tops[bi].bottom + 8 + (t % 8) * 8, gR = px + 16 + (t % 3) * 5, gL = qx - 16 - (t % 3) * 5;
        path += `L${round(gR)} ${round(py)}L${round(gR)} ${round(cy)}L${round(gL)} ${round(cy)}L${round(gL)} ${round(qy)}L${round(qx)} ${round(qy)}`;
      } else {
        const mx = (px + qx) / 2;
        path += `C${round(mx)} ${round(py)} ${round(mx)} ${round(qy)} ${round(qx)} ${round(qy)}`;
      }
      path += tail;
      if (b.kind === "dummy") { px = b.x + b.w; py = qy; }
      else if (lane) { px = b.x; py = qy; }
      else { px = qx; py = qy; }
    }
    return { ...base, d: path, labelAt, back: false };
  });

  const box = (v: Vert): Box => ({ x: round(v.x), y: round(v.y), w: v.w, h: v.h });
  const tasks: GraphTask[] = [], boxes = new Map<string, Box>();
  for (const i of items) {
    const v = verts.get(`t:${i.key}`);
    if (!v) continue;
    const b = box(v);
    boxes.set(v.id, b);
    tasks.push({ ...b, id: v.id, key: i.key, no: v.num, layer: v.layer, lane: v.lane });
  }
  const bandBoxes: GraphBand[] = [];
  for (let L = 0; L < nL; L++) {
    const v = verts.get(`b:${L}`);
    if (!v) continue;
    const b = box(v);
    boxes.set(v.id, b);
    bandBoxes.push({ ...b, id: v.id, layer: L, keys: v.keys! });
  }
  const layers: GraphLayer[] = cols.map(c => ({ layer: c.L, x: round(c.x), top: round(c.top), w: c.width, keys: layerKeys[c.L], collapsed: !!c.band, lanes: c.lanes, band: bandOf.get(c.L)! }));
  return { tasks, bands: bandBoxes, edges, layers, width: round(width), height: round(height), bandCount: bands.length, layerOf, vertexOf, boxes, crossings: { planOrder, swept: bestC } };
}

/* ---------- Links ---------- */

/** Direct links, Map based. `needs` keeps each task's own dependency order, `unblocks` plan order. Unknown keys and self links are left out. */
export type Adjacency = { no: Map<string, number>; needs: Map<string, string[]>; unblocks: Map<string, string[]> };
export function adjacency(input: readonly PlanItem[]): Adjacency {
  const items = firstOf(input), no = new Map(items.map((i, n) => [i.key, n + 1]));
  const needs = new Map<string, string[]>(), unblocks = new Map<string, string[]>();
  for (const i of items) { needs.set(i.key, []); unblocks.set(i.key, []); }
  for (const i of items) for (const dep of new Set(i.dependsOn)) {
    if (dep === i.key || !no.has(dep)) continue;
    needs.get(i.key)!.push(dep);
    unblocks.get(dep)!.push(i.key);
  }
  return { no, needs, unblocks };
}

export type LinkMode = "direct" | "chain";
/** What a task needs and what it unblocks: direct links, or the whole chain up and down. Null for a key not in this plan. */
export function linksOf(adj: Adjacency, key: string | null, mode: LinkMode = "direct"): { needs: Set<string>; unblocks: Set<string> } | null {
  if (key === null || !adj.no.has(key)) return null;
  const walk = (next: Map<string, string[]>) => {
    const out = new Set<string>(), stack = [...next.get(key)!];
    if (mode === "direct") return new Set(stack);
    while (stack.length) { const k = stack.pop()!; if (k === key || out.has(k)) continue; out.add(k); stack.push(...next.get(k)!); }
    return out;
  };
  return { needs: walk(adj.needs), unblocks: walk(adj.unblocks) };
}

/**
 * The selection that may highlight and dim the graph: only what the owner explicitly picked, and only while it is
 * in the version shown. Never pass a default such as the first task; with nothing picked, nothing is dimmed.
 */
export const explicitPick = (adj: Adjacency, picked: string | null | undefined): string | null => picked != null && adj.no.has(picked) ? picked : null;

/* ---------- Search and status ---------- */

const segmenter = typeof Intl !== "undefined" && "Segmenter" in Intl ? new Intl.Segmenter() : null;
const graphemes = (s: string) => segmenter ? Array.from(segmenter.segment(s), g => g.segment) : Array.from(s);
const firstLine = (s: string) => s.split("\n", 1)[0];
/** The line a task shows: its title, or the first line of its description when it has none. */
export const graphTitle = (item: PlanItem) => firstLine(item.title ?? item.description);

/** Keys whose number or shown title matches, case-insensitive; a number query such as `17` or `#17` puts task 17 first. Null when the query is empty. */
export function searchKeys(input: readonly PlanItem[], query: string): string[] | null {
  const q = query.trim().toLowerCase();
  if (!q) return null;
  const items = firstOf(input), num = /^#?\d+$/.test(q) ? Number(q.replace("#", "")) : null;
  const out: string[] = [];
  let first: string | null = null;
  items.forEach((item, i) => {
    if (num === i + 1) first = item.key;
    else if (graphTitle(item).toLowerCase().includes(q)) out.push(item.key);
  });
  return first === null ? out : [first, ...out];
}

export type StatusFilter = "needs" | "problem" | "working" | "stopped" | "review" | "finish" | "ready" | "waiting" | "done" | "unknown";
/** The Status popover rows, in order. */
export const STATUS_FILTERS: readonly { id: StatusFilter; label: string; icon: string }[] = [
  { id: "needs", label: "Needs you", icon: "question" }, { id: "problem", label: "Problem", icon: "alert" }, { id: "working", label: "Working", icon: "loader" },
  { id: "stopped", label: "Stopped", icon: "pause" }, { id: "review", label: "In review", icon: "eye" }, { id: "finish", label: "Ready to finish", icon: "merge" },
  { id: "ready", label: "Ready to start", icon: "play" }, { id: "waiting", label: "Waiting", icon: "hourglass" }, { id: "done", label: "Done", icon: "check" },
  { id: "unknown", label: "Unknown", icon: "help" },
];
const LABEL = new Map(STATUS_FILTERS.map(f => [f.id, f.label]));

/** The parts of a plan row that state reads (`PlanRow` from sprint-model fits). `backed` says the row's version is the one its tasks were created from. */
export type StateRow = { backed: boolean; view: { unknown: boolean } | null; word: { text: string; group: string; tone: string; icon: string; spin?: boolean } | null; waits: readonly number[] };

/**
 * The Status filter row a task belongs to, by its state word. Null when the rows carry no states (history, drafts,
 * plans before tasks exist). A task that is not loaded, or whose process state is unknown, is Unknown: never Done or Ready.
 */
export function statusFilterOf(row: StateRow): StatusFilter | null {
  if (!row.backed) return null;
  const w = row.word;
  if (!row.view || !w) return "unknown";
  if (w.text === "Conflict" || w.text === "Run failed") return "problem";
  if (row.view.unknown) return "unknown";
  if (w.group === "ready") return "finish";
  if (w.group === "merged" || w.group === "closed") return "done";
  switch (w.text) {
    case "Needs you": return "needs";
    case "Working": return "working";
    case "Paused": case "Stopping": case "Run cancelled": return "stopped";
    case "Review": return "review";
    case "Blocked": return "waiting";
    case "Next": return "ready";
    case "Queued": return row.waits.length ? "waiting" : "ready";
  }
  return "unknown";
}

/** A node's state line: today's list wording, plus `Not loaded` for a created task that is missing. Null when the rows carry no states. */
export function nodeState(row: StateRow): { text: string; tone: string; icon: string; spin?: boolean; filter: StatusFilter } | null {
  const filter = statusFilterOf(row), w = row.word;
  if (!filter) return null;
  if (!w || !row.view) return { text: "Not loaded", tone: "st-idle", icon: "help", filter };
  if ((w.text === "Blocked" || w.text === "Queued") && row.waits.length) return { text: `Waits for ${listNumbers(row.waits)}`, tone: "st-idle", icon: "hourglass", filter };
  const ready = w.text === "Next" || w.text === "Queued";
  return { text: ready ? "Ready" : w.text, tone: w.tone, icon: w.text === "Next" ? "play" : w.icon, spin: w.spin, filter };
}

/** Filter rows with their counts, in popover order; rows with no task are left out. */
export function statusCounts(statusOf: ReadonlyMap<string, StatusFilter | null>): { id: StatusFilter; label: string; icon: string; n: number }[] {
  const n = new Map<StatusFilter, number>();
  for (const s of statusOf.values()) if (s) n.set(s, (n.get(s) ?? 0) + 1);
  return STATUS_FILTERS.filter(f => n.has(f.id)).map(f => ({ ...f, n: n.get(f.id)! }));
}

/**
 * Search and status filter combined with AND, in match order. Null when neither is active, so nothing dims.
 * An empty list means both are active and nothing matches.
 */
export function graphMatches(items: readonly PlanItem[], query: string, filter: ReadonlySet<StatusFilter>, statusOf: ReadonlyMap<string, StatusFilter | null>): string[] | null {
  const found = searchKeys(items, query);
  if (!filter.size) return found;
  const pass = (key: string) => { const s = statusOf.get(key); return !!s && filter.has(s); };
  return (found ?? firstOf(items).map(i => i.key)).filter(pass);
}

/** A collapsed layer's tally, worst first: Needs you, Problem, Unknown, then the rest. */
export function layerTally(keys: readonly string[], statusOf: ReadonlyMap<string, StatusFilter | null>): { id: StatusFilter; label: string; n: number }[] {
  const n = new Map<StatusFilter, number>();
  for (const k of keys) { const s = statusOf.get(k); if (s) n.set(s, (n.get(s) ?? 0) + 1); }
  const order: StatusFilter[] = ["needs", "problem", "unknown", ...STATUS_FILTERS.map(f => f.id).filter(id => id !== "needs" && id !== "problem" && id !== "unknown")];
  return order.filter(id => n.has(id)).map(id => ({ id, label: LABEL.get(id)!, n: n.get(id)! }));
}

/** Layers where every task is Done. Unknown never counts, and a plan without states has none. */
export const finishedLayers = (layers: readonly GraphLayer[], statusOf: ReadonlyMap<string, StatusFilter | null>) =>
  layers.filter(l => l.keys.length > 0 && l.keys.every(k => statusOf.get(k) === "done")).map(l => l.layer);

/* ---------- Zoom and detail ---------- */

export type DetailLevel = "detail" | "title" | "number" | "overview";
/** What a node shows at this zoom: number and title with a state line, one title line, the number, or only its box. */
export const detailLevel = (zoom: number): DetailLevel => zoom >= 0.8 ? "detail" : zoom >= 0.5 ? "title" : zoom >= 0.3 ? "number" : "overview";
/** At the Number level the state icon joins the number from this zoom on. */
export const numberIconZoom = 0.42;

const TITLE_CHARS: [number, number][] = [[0.5, 7], [0.6, 10], [0.7, 14], [0.8, 19], [1, 25]];
/** About how many title characters fit, the ellipsis included, while text is held at 12 px on screen. 0 below the Title level. */
export function titleChars(zoom: number): number {
  if (!(zoom >= 0.5)) return 0;
  if (zoom >= 1) return 25;
  const i = TITLE_CHARS.findIndex(([z]) => z > zoom), [z0, c0] = TITLE_CHARS[i - 1], [z1, c1] = TITLE_CHARS[i];
  return Math.floor(c0 + (zoom - z0) / (z1 - z0) * (c1 - c0) + 1e-9);
}
/** End truncation on whole graphemes, within `chars` including the ellipsis. Only the first line is used. */
export function truncateTitle(text: string, chars: number): string {
  const g = graphemes(firstLine(text));
  if (g.length <= chars) return g.join("");
  if (chars < 1) return "";
  return `${g.slice(0, chars - 1).join("").trimEnd()}…`;
}

/** Zoom that shows the whole box with a 24 px margin, never above 100%. A canvas not yet measured counts as 100%. */
export function fitZoom(size: { width: number; height: number }, canvas: { width: number; height: number }, margin: number = GRAPH.margin): number {
  if (size.width <= 0 || size.height <= 0 || canvas.width < 60 || canvas.height < 60) return 1;
  return Math.max(0.01, Math.min(1, (canvas.width - 2 * margin) / size.width, (canvas.height - 2 * margin) / size.height));
}
/** Zoom runs from the lower of Fit and 50% to 150%; buttons step by 1.25. */
export const ZOOM_STEP = 1.25, ZOOM_MAX = 1.5;
export const zoomMin = (fit: number) => Math.min(fit, 0.5);
export const clampZoom = (zoom: number, fit: number) => Math.min(ZOOM_MAX, Math.max(zoomMin(fit), zoom));
/** The box around some tasks, for Fit matches. Null when none of them is drawn. */
export function boundsOf(g: GraphLayout, keys: Iterable<string>): Box | null {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const k of keys) {
    const id = g.vertexOf.get(k), b = id ? g.boxes.get(id) : undefined;
    if (!b) continue;
    x0 = Math.min(x0, b.x); y0 = Math.min(y0, b.y); x1 = Math.max(x1, b.x + b.w); y1 = Math.max(y1, b.y + b.h);
  }
  return x0 === Infinity ? null : { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

/* ---------- Words: short, and bounded however large the plan ---------- */

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
/** At most five numbers, then `and 4 more`. */
export const listNumbers = (nos: readonly number[], max = 5) => nos.length <= max ? nos.join(", ") : `${nos.slice(0, max).join(", ")} and ${nos.length - max} more`;
export const graphLabel = (tasks: number) => `Dependency graph, ${plural(tasks, "task")}`;
export const graphCounts = (tasks: number, layers: number) => `${plural(tasks, "task")} in ${plural(layers, "layer")}`;
export const layerHeading = (layer: number, tasks: number) => `Layer ${layer + 1} · ${tasks}`;
export const layerOpened = (layer: number) => `Layer ${layer + 1} opened.`;
/** The status band for an explicit pick: `Task 17 · Needs 3, 5 · Unblocks 20, 21`. */
export const selectionLine = (no: number, needs: readonly number[], unblocks: readonly number[]) =>
  [`Task ${no}`, needs.length ? `Needs ${listNumbers(needs)}` : "Starts first", ...(unblocks.length ? [`Unblocks ${listNumbers(unblocks)}`] : [])].join(" · ");
/** A task's accessible name: `Task 17, Add the audit events table. Needs you. Needs 3, 5. Unblocks 20.` */
export const taskLabel = (no: number, title: string, state: string | null, needs: readonly number[], unblocks: readonly number[]) =>
  `Task ${no}, ${truncateTitle(title, 80)}.${state ? ` ${state}.` : ""} ${needs.length ? `Needs ${listNumbers(needs)}.` : "Starts first."}${unblocks.length ? ` Unblocks ${listNumbers(unblocks)}.` : ""}`;
/** The fixed-width count beside search: `2 of 7`, or `No matches`. `current` is 0-based. */
export const matchCount = (current: number, total: number) => total ? `${Math.min(Math.max(current, 0), total - 1) + 1} of ${total}` : "No matches";
/** One polite announcement per search step: `7 matches. 2 of 7, task 17, Add the audit events table.` */
export const matchAnnouncement = (current: number, total: number, no: number, title: string) =>
  `${total} ${total === 1 ? "match" : "matches"}. ${current + 1} of ${total}, task ${no}, ${truncateTitle(title, 80)}.`;
/** Why everything is dimmed: `No task matches "zzz".`, with `with this status` when a filter is on too. */
export const noMatchText = (query: string, filtered: boolean) => {
  const q = query.trim();
  if (!q) return "No task has this status.";
  return `No task matches "${truncateTitle(q, 40)}"${filtered ? " with this status." : "."}`;
};
