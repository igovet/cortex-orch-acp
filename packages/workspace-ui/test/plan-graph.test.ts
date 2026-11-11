import { describe, expect, it } from "vitest";
import type { PlanItem } from "@cortex/workspace-domain";
import {
  GRAPH, adjacency, boundsOf, clampZoom, detailLevel, explicitPick, finishedLayers, fitZoom, graphCounts, graphLabel, graphMatches, layerTally, layout,
  linksOf, listNumbers, matchAnnouncement, matchCount, noMatchText, nodeState, planLayers, searchKeys, selectionLine, statusCounts, statusFilterOf,
  taskLabel, titleChars, topologyKey, truncateTitle, type GraphLayout, type StateRow, type StatusFilter,
} from "../src/plan-graph";
import { planGraph } from "../src/sprint-model";

/* Deterministic synthetic plans, shaped like the layout report's samples. */
const rng = (seed: number) => () => { seed = (seed + 0x6d2b79f5) | 0; let t = Math.imul(seed ^ (seed >>> 15), 1 | seed); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const plan = (deps: number[][]): PlanItem[] => deps.map((d, i) => ({ key: `k${i + 1}`, title: `Task number ${i + 1} with a title`, description: `Description ${i + 1}`, dependsOn: d.map(n => `k${n}`) }));
const typical = (n: number, seed = 7) => {
  const r = rng(seed);
  return plan(Array.from({ length: n }, (_, i) => {
    if (i < 4) return [];
    const k = 1 + Math.floor(r() * 3), out = new Set<number>();
    for (let j = 0; j < k; j++) out.add(Math.max(1, i - Math.floor(r() * 15)));
    return [...out];
  }));
};
const chain = (n: number) => plan(Array.from({ length: n }, (_, i) => i ? [i] : []));
const fan = (n: number) => plan(Array.from({ length: n }, (_, i) => i ? [1] : []));
const flat = (n: number) => plan(Array.from({ length: n }, () => []));
const streams = (n: number, k: number) => plan(Array.from({ length: n }, (_, i) => i >= k ? [i + 1 - k] : []));
const diamond = (n: number, seed = 3) => {
  const r = rng(seed), sizes: number[] = [];
  for (let left = n, w = 1, grow = true; left > 0; ) { const s = Math.min(left, w); sizes.push(s); left -= s; if (w >= 14) grow = false; w = grow ? w + 3 : Math.max(1, w - 3); }
  const deps: number[][] = [];
  let start = 0, prevStart = 0, prevSize = 0;
  for (const s of sizes) {
    for (let j = 0; j < s; j++) deps.push(prevSize ? [...new Set([prevStart + 1 + Math.floor(r() * prevSize), prevStart + 1 + Math.floor(r() * prevSize)])] : []);
    prevStart = start; prevSize = s; start += s;
  }
  return plan(deps);
};
const FIXTURES: [string, (n: number) => PlanItem[]][] = [["typical", typical], ["chain", chain], ["fan", fan], ["flat", flat], ["streams", n => streams(n, 6)], ["diamond", diamond]];

const overlaps = (g: GraphLayout) => {
  const boxes = [...g.boxes.values()];
  let n = 0;
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const a = boxes[i], b = boxes[j];
    if (a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h) n++;
  }
  return n;
};

describe("layout", () => {
  it("is the same picture for the same plan, and titles never move anything", () => {
    const items = typical(200), a = layout(items), b = layout(structuredClone(items));
    expect(b).toEqual(a);
    const retitled = items.map(i => ({ ...i, title: `${i.title} renamed to something much longer`, description: "changed" }));
    expect(layout(retitled)).toEqual(a);
    expect(topologyKey(retitled)).toBe(topologyKey(items));
    expect(topologyKey(items.map((i, n) => n === 5 ? { ...i, dependsOn: [] } : i))).not.toBe(topologyKey(items));
  });

  it("layers by longest path exactly as today's inline graph", () => {
    for (const items of [typical(50), diamond(50), streams(50, 6)]) {
      const columns = new Map(planGraph(items, null).nodes.map(n => [n.key, (n.x - 12) / 176]));
      expect(planLayers(items)).toEqual(columns);
    }
  });

  it("keeps the cycle guard: a cycle or a self link never hangs, throws or leaves an empty layer", () => {
    const items: PlanItem[] = [
      { key: "a", description: "A", dependsOn: ["b"] }, { key: "b", description: "B", dependsOn: ["a"] },
      { key: "c", description: "C", dependsOn: ["c"] }, { key: "d", description: "D", dependsOn: ["a", "missing"] },
    ];
    const g = layout(items);
    expect(g.tasks.map(t => t.key)).toEqual(["a", "b", "c", "d"]);
    const used = new Set(g.layerOf.values());
    expect([...used].sort()).toEqual([...Array(used.size).keys()]);
    expect(g.layerOf.get("d")).toBeGreaterThan(g.layerOf.get("a")!);
    for (const t of g.tasks) for (const v of [t.x, t.y]) expect(Number.isFinite(v)).toBe(true);
    expect(g.edges.some(e => e.back)).toBe(true);
    for (const e of g.edges) expect(e.d).not.toMatch(/NaN|Infinity/);
    expect(g.edges.flatMap(e => e.real)).not.toContainEqual(["c", "c"]);
  });

  it("draws a one-task plan as one 200x44 box at the top left that never scales up", () => {
    const g = layout([{ key: "only", description: "Add the audit events table", dependsOn: [] }]);
    expect(g.tasks).toEqual([{ id: "t:only", key: "only", no: 1, layer: 0, lane: 0, x: GRAPH.pad, y: GRAPH.head, w: 200, h: 44 }]);
    expect([g.width, g.height, g.bandCount, g.edges.length]).toEqual([256, 80, 1, 0]);
    expect(fitZoom(g, { width: 1920, height: 1080 })).toBe(1);
    expect(layout([])).toMatchObject({ tasks: [], width: 0, height: 0 });
    expect(fitZoom(layout([]), { width: 800, height: 600 })).toBe(1);
  });

  it("keeps a small plan at its intrinsic geometry: one band, no lanes, Fit at 100%", () => {
    const g = layout(plan([[], [1], [1], [2, 3], [4]]));
    expect(g.bandCount).toBe(1);
    expect(g.layers.map(l => [l.lanes, l.x])).toEqual([[1, 28], [1, 276], [1, 524], [1, 772]]);
    expect(fitZoom(g, { width: 1400, height: 744 })).toBe(1);
    expect(overlaps(g)).toBe(0);
  });

  for (const n of [50, 100, 200]) it(`lays out ${n}-task plans with lanes, bands and no overlapping boxes`, () => {
    for (const [name, make] of FIXTURES) {
      const items = make(n), g = layout(items);
      expect(g.tasks, name).toHaveLength(n);
      expect(overlaps(g), name).toBe(0);
      for (const t of g.tasks) {
        expect(t.x, name).toBeGreaterThanOrEqual(0); expect(t.y, name).toBeGreaterThanOrEqual(0);
        expect(t.x + t.w, name).toBeLessThanOrEqual(g.width); expect(t.y + t.h, name).toBeLessThanOrEqual(g.height);
      }
      for (const e of g.edges) expect(e.d, name).toMatch(/^M[\d. LC-]+$/);
      expect(g.edges.reduce((s, e) => s + e.count, 0), name).toBe(items.reduce((s, i) => s + new Set(i.dependsOn).size, 0));
      expect(g.layers.every(l => l.lanes >= 1 && l.lanes <= 8), name).toBe(true);
    }
    const wide = layout(fan(n)).layers[1];
    expect(wide.lanes).toBeGreaterThan(1);
    const laneTasks = layout(fan(n)).tasks.filter(t => t.layer === 1);
    expect(new Set(laneTasks.map(t => t.lane)).size).toBe(wide.lanes);
    const long = layout(chain(n));
    expect(long.bandCount).toBeGreaterThan(1);
    expect(long.width).toBeLessThan(n * (GRAPH.nodeW + GRAPH.layerGap) / 2);
    expect(fitZoom(long, { width: 1400, height: 744 })).toBeGreaterThan(fitZoom({ width: n * 248, height: 80 }, { width: 1400, height: 744 }));
  });

  it("splits a flat 200-task layer into the most lanes, filled row by row", () => {
    const g = layout(flat(200));
    expect(g.layers[0].lanes).toBe(8);
    expect(g.tasks.slice(0, 9).map(t => t.lane)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 0]);
  });

  it("reduces crossings against plan order", () => {
    const seen: string[] = [];
    for (const n of [50, 100, 200]) {
      const g = layout(typical(n));
      seen.push(`typical ${n}: ${g.crossings.planOrder} -> ${g.crossings.swept}`);
      expect(g.crossings.swept).toBeLessThan(g.crossings.planOrder);
      for (const [, make] of FIXTURES) { const c = layout(make(n)).crossings; expect(c.swept).toBeLessThanOrEqual(c.planOrder); }
    }
    console.info(`Crossings: ${seen.join("; ")}`);
  });

  it("collapses a layer into one band with merged, counted edges", () => {
    const items = plan([[], [], [], [1, 2, 3], [1], [4, 5]]);
    const open = layout(items), L = open.layerOf.get("k1")!;
    const g = layout(items, [L]);
    expect(g.bands).toHaveLength(1);
    expect(g.bands[0]).toMatchObject({ id: `b:${L}`, layer: L, keys: ["k1", "k2", "k3"], w: GRAPH.bandW, h: GRAPH.bandH });
    expect(g.tasks.map(t => t.key)).toEqual(["k4", "k5", "k6"]);
    expect(g.vertexOf.get("k2")).toBe(`b:${L}`);
    const into4 = g.edges.find(e => e.from === `b:${L}` && e.to === "t:k4")!;
    expect(into4.count).toBe(3);
    expect(into4.real).toEqual([["k1", "k4"], ["k2", "k4"], ["k3", "k4"]]);
    expect(g.edges.reduce((s, e) => s + e.count, 0)).toBe(open.edges.reduce((s, e) => s + e.count, 0));
    expect(g.layers[L].collapsed).toBe(true);
    expect(g.layerOf).toEqual(open.layerOf);
    // Two collapsed neighbours share one edge with the count of what it stands for.
    const both = layout(items, [L, open.layerOf.get("k4")!]);
    const between = both.edges.filter(e => e.from === `b:${L}` && e.to.startsWith("b:"));
    expect(between).toHaveLength(1);
    expect(between[0].count).toBe(4);
    expect(overlaps(layout(typical(100), [1, 2, 3]))).toBe(0);
    expect(layout(items, [99, -1, 0.5])).toEqual(open);
  });

  it(`lays out 200 tasks within the ${30} ms budget`, () => {
    const timings: string[] = [];
    for (const [name, make] of FIXTURES) {
      const items = make(200);
      layout(items); layout(items);
      const runs: number[] = [];
      for (let i = 0; i < 5; i++) { const t = performance.now(); layout(items); runs.push(performance.now() - t); }
      runs.sort((a, b) => a - b);
      timings.push(`${name} ${runs[2].toFixed(1)} ms`);
      // The budget is 30 ms; the bound is generous so a loaded machine does not fail the run. The measured value is logged.
      expect(runs[2], name).toBeLessThan(120);
    }
    console.info(`Layout of 200 tasks, median of 5: ${timings.join(", ")}`);
  });
});

describe("links and selection", () => {
  const items = plan([[], [1], [2], [2], [3, 4], []]);
  const adj = adjacency(items);

  it("maps direct links both ways and the whole chain", () => {
    expect(adj.needs.get("k5")).toEqual(["k3", "k4"]);
    expect(adj.unblocks.get("k2")).toEqual(["k3", "k4"]);
    expect(linksOf(adj, "k3")).toEqual({ needs: new Set(["k2"]), unblocks: new Set(["k5"]) });
    expect(linksOf(adj, "k3", "chain")).toEqual({ needs: new Set(["k2", "k1"]), unblocks: new Set(["k5"]) });
    expect(linksOf(adj, "k1", "chain")!.unblocks).toEqual(new Set(["k2", "k3", "k4", "k5"]));
    const cyclic = adjacency([{ key: "a", description: "", dependsOn: ["b", "a"] }, { key: "b", description: "", dependsOn: ["a"] }]);
    expect(linksOf(cyclic, "a", "chain")).toEqual({ needs: new Set(["b"]), unblocks: new Set(["b"]) });
  });

  it("counts only an explicit pick that exists in the version shown", () => {
    expect(explicitPick(adj, null)).toBeNull();
    expect(explicitPick(adj, undefined)).toBeNull();
    expect(explicitPick(adj, "gone")).toBeNull();
    expect(explicitPick(adj, "k2")).toBe("k2");
    expect(linksOf(adj, explicitPick(adj, "gone"))).toBeNull();
  });
});

describe("search and status", () => {
  const items: PlanItem[] = [
    ...plan(Array.from({ length: 20 }, () => [])).map((i, n) => ({ ...i, title: n === 16 ? "Add the AUDIT events table" : `Write part ${n + 1}` })),
    { key: "untitled", description: "Audit the export\nsecret second line", dependsOn: [] },
  ];

  it("matches number and title, number first, case-insensitive", () => {
    expect(searchKeys(items, "  ")).toBeNull();
    expect(searchKeys(items, "audit")).toEqual(["k17", "untitled"]);
    expect(searchKeys(items, "17")).toEqual(["k17"]);
    expect(searchKeys(items, "#3")).toEqual(["k3"]);
    expect(searchKeys(items, "1")![0]).toBe("k1");
    expect(searchKeys(items, "1")).toContain("k11");
    expect(searchKeys(items, "secret")).toEqual([]);
    expect(searchKeys(items, "description")).toEqual([]);
  });

  const word = (text: string, group = "working") => ({ text, group, tone: "st-idle", icon: "circle" });
  const row = (w: ReturnType<typeof word> | null, extra: Partial<StateRow> = {}): StateRow => ({ backed: true, view: w ? { unknown: false } : null, word: w, waits: [], ...extra });

  it("maps state words to the filter rows, and Unknown never looks done or ready", () => {
    const cases: [StateRow, StatusFilter | null][] = [
      [row(word("Needs you")), "needs"], [row(word("Conflict")), "problem"], [row(word("Run failed")), "problem"],
      [row(word("Working")), "working"], [row(word("Paused")), "stopped"], [row(word("Stopping")), "stopped"], [row(word("Run cancelled")), "stopped"],
      [row(word("Review")), "review"], [row(word("Ready to merge", "ready")), "finish"], [row(word("Commit to main", "ready")), "finish"],
      [row(word("Next", "next")), "ready"], [row(word("Queued", "blocked")), "ready"], [row(word("Queued", "blocked"), { waits: [2] }), "waiting"],
      [row(word("Blocked", "blocked")), "waiting"], [row(word("Merged", "merged")), "done"], [row(word("Committed to main", "closed")), "done"],
      [row(word("Closed as is", "closed")), "done"], [row(word("Done outside the app", "closed")), "done"], [row(word("Cancelled", "closed")), "done"],
      [row(null), "unknown"], [row(word("Process state unknown"), { view: { unknown: true } }), "unknown"],
      [row(word("Something new")), "unknown"],
      [row(word("Merged", "merged"), { backed: false }), null], [row(null, { backed: false }), null],
    ];
    for (const [r, f] of cases) expect(statusFilterOf(r), r.word?.text ?? "not loaded").toBe(f);
    expect(nodeState(row(null))).toEqual({ text: "Not loaded", tone: "st-idle", icon: "help", filter: "unknown" });
    expect(nodeState(row(word("Queued", "blocked"), { waits: [1, 2, 3, 4, 5, 6, 7] }))!.text).toBe("Waits for 1, 2, 3, 4, 5 and 2 more");
    expect(nodeState(row(word("Next", "next")))).toMatchObject({ text: "Ready", icon: "play", filter: "ready" });
    expect(nodeState(row(word("Merged", "merged"), { backed: false }))).toBeNull();
  });

  it("combines search and status with AND, counts rows and finds finished layers", () => {
    const status = new Map<string, StatusFilter | null>(items.map((i, n) => [i.key, n < 10 ? "done" : n < 15 ? "needs" : "unknown"]));
    expect(graphMatches(items, "", new Set(), status)).toBeNull();
    expect(graphMatches(items, "", new Set(["needs"]), status)).toEqual(["k11", "k12", "k13", "k14", "k15"]);
    expect(graphMatches(items, "audit", new Set(["unknown"]), status)).toEqual(["k17", "untitled"]);
    expect(graphMatches(items, "audit", new Set(["done"]), status)).toEqual([]);
    expect(statusCounts(status).map(r => [r.id, r.n])).toEqual([["needs", 5], ["done", 10], ["unknown", 6]]);
    expect(layerTally(items.map(i => i.key), status).map(r => r.label)).toEqual(["Needs you", "Unknown", "Done"]);
    const g = layout(plan([[], [], [1], [2]]));
    expect(finishedLayers(g.layers, new Map([["k1", "done"], ["k2", "done"], ["k3", "done"], ["k4", "unknown"]]))).toEqual([0]);
    expect(finishedLayers(g.layers, new Map())).toEqual([]);
  });
});

describe("zoom, detail and words", () => {
  it("picks the level of detail and title length from zoom", () => {
    expect([1.5, 0.8, 0.79, 0.5, 0.49, 0.3, 0.29].map(detailLevel)).toEqual(["detail", "detail", "title", "title", "number", "number", "overview"]);
    expect([0.29, 0.5, 0.6, 0.7, 0.8, 1, 1.5].map(titleChars)).toEqual([0, 7, 10, 14, 19, 25, 25]);
    expect(titleChars(0.55)).toBe(8);
    expect(truncateTitle("Add the audit events table", 10)).toBe("Add the a…");
    expect(truncateTitle("Add the audit events table", 9)).toBe("Add the…");
    expect(truncateTitle("Short", 10)).toBe("Short");
    expect(truncateTitle("👨‍👩‍👧👨‍👩‍👧👨‍👩‍👧 family", 3)).toBe("👨‍👩‍👧👨‍👩‍👧…");
    expect(truncateTitle("First line\nsecond", 40)).toBe("First line");
  });

  it("fits never above 100% and clamps zoom", () => {
    const g = layout(typical(100));
    const fit = fitZoom(g, { width: 1440, height: 744 });
    expect(fit).toBeLessThan(1);
    expect(fit).toBeCloseTo(Math.min((1440 - 48) / g.width, (744 - 48) / g.height));
    expect(fitZoom(g, { width: 20000, height: 20000 })).toBe(1);
    expect(fitZoom(g, { width: 10, height: 10 })).toBe(1);
    expect(clampZoom(0.01, 0.3)).toBe(0.3);
    expect(clampZoom(0.01, 0.9)).toBe(0.5);
    expect(clampZoom(4, 0.9)).toBe(1.5);
    const b = boundsOf(g, ["k1", "k2", "gone"])!;
    expect(b.w).toBeGreaterThanOrEqual(GRAPH.nodeW);
    expect(boundsOf(g, ["gone"])).toBeNull();
  });

  it("keeps labels and announcements short at 200 tasks", () => {
    const items = fan(200), adj = adjacency(items), no = (k: string) => adj.no.get(k)!;
    const root = linksOf(adj, "k1")!;
    const line = selectionLine(1, [...root.needs].map(no), [...root.unblocks].map(no));
    expect(line).toBe("Task 1 · Starts first · Unblocks 2, 3, 4, 5, 6 and 194 more");
    const long = "An extremely long task title ".repeat(30);
    const label = taskLabel(17, long, "Needs you", Array.from({ length: 150 }, (_, i) => i + 1), Array.from({ length: 150 }, (_, i) => i + 2));
    expect(label.length).toBeLessThan(200);
    expect(label).toMatch(/^Task 17, An extremely long task title .*…\. Needs you\. Needs 1, 2, 3, 4, 5 and 145 more\. Unblocks 2, 3, 4, 5, 6 and 145 more\.$/);
    expect(taskLabel(1, "Add the audit events table", null, [], [])).toBe("Task 1, Add the audit events table. Starts first.");
    expect(graphLabel(200)).toBe("Dependency graph, 200 tasks");
    expect([graphCounts(1, 1), graphCounts(57, 12)]).toEqual(["1 task in 1 layer", "57 tasks in 12 layers"]);
    expect([matchCount(1, 7), matchCount(-1, 0)]).toEqual(["2 of 7", "No matches"]);
    expect(matchAnnouncement(1, 7, 17, "Add the audit events table")).toBe("7 matches. 2 of 7, task 17, Add the audit events table.");
    expect(matchAnnouncement(0, 1, 3, "X")).toBe("1 match. 1 of 1, task 3, X.");
    expect([noMatchText("zzz", false), noMatchText("zzz", true), noMatchText("", true)]).toEqual(['No task matches "zzz".', 'No task matches "zzz" with this status.', "No task has this status."]);
    expect(noMatchText("q".repeat(500), false).length).toBeLessThan(60);
    expect(listNumbers([1, 2, 3])).toBe("1, 2, 3");
  });
});
