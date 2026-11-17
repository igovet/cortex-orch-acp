// The scroll gate in WebKitGTK, on the layout gate's fake data made long (`scroll` in layout-fit-fixture.tsx). Run through scroll-perf.mjs.
//
// Each surface below is opened at 1440x900 with WebKit's compositing on, as the desktop window runs it (software GL on a private
// virtual screen here, not the owner's GPU). Every element that scrolls on it is found and scrolled twice:
//
// - drag: the page moves the scroll position from one end to the other in 120 frames and back in 60, as a scrollbar drag does;
// - wheel: xdotool turns the real mouse wheel 24 notches each way over it, through GTK and WebKit's own wheel handling.
//
// A script installed before the page's own records, while it scrolls: the time between animation frames, how long style and
// layout took when forced at the start of each frame, every scroll and wheel handler of the page and how long it ran, every
// ResizeObserver callback, and every DOM change (a change while only scrolling is a render the scroll caused). WebKitGTK gives
// page script no style-recalc or layout counters, so "layout" is the forced flush time per frame, counted where it took over 1 ms.
//
// The `-live` surfaces scroll while the task streams an update every 400 ms, and also time each update the page takes in.
//
// A breach of LIMITS on any surface fails the run; KNOWN lists, per surface, what does not meet them yet and how far it may
// go, and each use is printed. Numbers go to <label>-metrics.json, captures to <label>-<surface>.png. Arguments after the
// label: `flat` turns compositing off (as the layout gate runs), `idle` instead watches each picked page untouched for 70 s
// (past the 30 s facts poll) for long frames and DOM changes, and any other word picks surfaces by name.
import {Host, startXvfb, engineEnv, main, sleep, xdotool, probe, quote, join, writeJson, sub} from "./rendered/webkit.mjs";

const base = process.argv[2], label = process.argv[3], args = process.argv.slice(4), evidence = "/evidence";
const picked = args.filter(a => a !== "flat" && a !== "idle");
const [WIDTH, HEIGHT] = [1440, 900];
const INSTRUMENT = probe("scroll-perf.INSTRUMENT.js"), FIND = probe("scroll-perf.FIND.js"), DRAG = probe("scroll-perf.DRAG.js");
const FACTS_TEXTS = probe("scroll-perf.FACTS_TEXTS.js"), IDLE = probe("scroll-perf.IDLE.js");
const POSITION = "const el=document.querySelector('[data-perf=\"ID\"]');return AXIS==='y'?el.scrollTop:el.scrollLeft";

const at = route => "route=" + quote(route);
const FRAME = "document.querySelector('.titlebar') && !document.querySelector('.l-startup')";
const on = selector => FRAME + " && document.querySelector(" + JSON.stringify(selector) + ")";
// Surface name, query, ready expression, and for the sprint Graph the canvas the wheel pans (it scrolls nothing).
const SURFACES = [
  ["conversation", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work"), on(".timeline .ai"), null],
  ["worker-feed", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work/workers/w-be2"), on(".stream .stream-in > *"), null],
  // The same while the task streams: an update every 400 ms, as a working task sends them (LIVE).
  ["conversation-live", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work"), on(".timeline .ai"), null],
  ["worker-feed-live", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work/workers/w-be2"), on(".stream .stream-in > *"), null],
  ["sprint-plan-list", "scroll=1&" + at("/m/<M>/p/p/s/big/plan"), on(".sp-plan:not([hidden]) .pt-row"), null],
  ["sprint-graph", "scroll=1&graph=1&" + at("/m/<M>/p/p/s/big/plan"), on(".gsv .n") + " && document.querySelector('.gv-band')", ".gv-canvas"],
  ["sprint-board", "scroll=1&" + at("/m/<M>/p/p/s/big/board"), on(".board-wrap .card"), null],
  ["project-board", "scroll=1&" + at("/m/<M>/p/p/board"), on(".board-wrap .card"), null],
  ["project-list", "scroll=1&" + at("/m/<M>/p/p/list"), on(".l-work .l-page-body > *"), null],
  ["git", "scroll=1&" + at("/m/<M>/p/p/git"), on(".git-sum") + " && document.querySelector('.hrow')", null],
  ["code", "scroll=1&" + at("/m/<M>/p/p/code?file=<FILE>"), on(".cm-content"), null],
  // The same 4,000 lines with no grammar: what is left of the editor's cost once syntax parsing is out of it.
  ["code-plain", "scroll=1&" + at("/m/<M>/p/p/code?file=docs%2Flong-notes.txt"), on(".cm-content"), null],
  // The 200-task sprint's board: rows rendered in flow must stay smooth at this size too.
  ["sprint-board-200", "scroll=1&graph=200&" + at("/m/<M>/p/p/s/huge/board"), on(".board-wrap .card"), null],
  // xterm scrolls its own buffer on the wheel; nothing in the page scrolls.
  ["terminal", "scroll=1&terminal=1&" + at("/m/<M>/p/p/git"), on(".xterm-rows > div"), ".terminal-screen"],
  ["review-changes", "world=review&" + at("/m/<M>/t/rg/changes"), on(".fhead"), null],
];
// Scrollers every page of the frame has, measured on the first page they are found on.
const SHARED = ["div.sb-scroll"];
const LIVE = ["conversation-live", "worker-feed-live"];
// The gate. Frame intervals cannot fall below the 16.7 ms of a 60 Hz display (page timers here count whole milliseconds);
// a frame over 25 ms dropped at least one frame, one over 50 ms is a long task. p95 22 leaves room for the host's own load
// on this software-rendered screen, which is slower than the owner's GPU.
const LIMITS = {p95: 22, max: 50, long: 0, dropped_share: .03, handlerMax: 4, layoutMax: 8, resizeMax: 4, updateMax: 8};
// What does not meet LIMITS yet, measured, with room for run-to-run spread. These are ratchets, not passes: each use is
// printed as KNOWN, and going past one fails the run. Key: surface|scroller (empty for the surface)|pass.
const FIRST_JUMP = {max: 170, long: 1, handlerMax: 10}; // one frame painting the far end of a long history no frame has painted yet
// 1,500 notes with an update every 400 ms, five times the service's pace; the scroll handler then finds its row after a change.
const STREAM = {p95: 34, max: 60, dropped_share: .10, handlerMax: 8};
// CodeMirror renders and measures the lines it brings into view inside each scroll event; a drag moves 660 px a frame.
const EDITOR = {p95: 45, max: 80, long: 2, handlerMax: 30, dropped_share: .50};
// One frame painting part of a long card column no frame has shown yet; every other frame is on time.
const COLUMN = {max: 75, long: 1};
const KNOWN = {
  "conversation|div.timeline|drag": FIRST_JUMP, "worker-feed|div.stream|drag": FIRST_JUMP,
  "conversation-live|div.timeline|drag": {...FIRST_JUMP, dropped_share: .06},
  "conversation-live|div.timeline|dragAgain": {p95: 26, dropped_share: .06}, "conversation-live|div.timeline|wheel": {p95: 26, dropped_share: .06},
  "conversation-live||update": {updateMax: 14},
  "worker-feed-live|div.stream|drag": {...STREAM, ...FIRST_JUMP}, "worker-feed-live|div.stream|dragAgain": STREAM,
  "worker-feed-live|div.stream|wheel": STREAM, "worker-feed-live||update": {updateMax: 26},
  "code|div.cm-scroller|drag": EDITOR, "code|div.cm-scroller|dragAgain": EDITOR,
  "code|div.cm-scroller|wheel": {p95: 24, max: 45, handlerMax: 16, dropped_share: .06},
  "code-plain|div.cm-scroller|drag": EDITOR, "code-plain|div.cm-scroller|dragAgain": EDITOR,
  "code-plain|div.cm-scroller|wheel": {p95: 22, max: 45, handlerMax: 16, dropped_share: .03},
  "sprint-board|div.col-b|wheel": COLUMN, "project-board|div.col-b|wheel": COLUMN, "sprint-board-200|div.col-b|wheel": COLUMN,
};

await main(async () => {
  const display = await startXvfb();
  const script = INSTRUMENT + (args.includes("idle") ? FACTS_TEXTS : "");
  const host = await Host.start({width: WIDTH, height: HEIGHT, script, env: engineEnv(display, {compositing: !args.includes("flat")}), timeout: 30});
  const js = code => host.js(code), wait = (code, tries = 200) => host.wait(code, tries);
  let loads = 0;
  const openPage = async (query, ready) => {
    loads++;
    await host.load(base + `?${query}&load=${loads}`);
    await wait(`window.fixture === '${loads}'`);
    await wait(ready);
    await sleep(1.5);
    const size = await js("return [innerWidth, innerHeight]");
    if (size[0] !== WIDTH || size[1] !== HEIGHT) throw Error("AssertionError: " + JSON.stringify(size));
  };
  const capture = async name => { await host.capture(join(evidence, `${label}-${name}.png`)); };
  const settle = async (code, still = 3) => {
    let last = null, same = 0;
    for (let i = 0; i < 100; i++) {
      const now = await js(code);
      same = now === last ? same + 1 : 0;
      if (same >= still) return now;
      last = now;
      await sleep(.1);
    }
    return last;
  };
  const drag = async s => {
    await js(sub(sub(sub(sub(DRAG, "ID", String(s.id)), "AXIS", s.axis), "RANGE", String(s.range)), "REVERSE", s.reverse ? "true" : "false"));
    await wait("window.__bench", 300);
    return js("return window.__bench");
  };
  const wheel = async (box, axis, position) => {
    // The real wheel through X: 24 notches one way, a breath, 24 back. Buttons 5/4 scroll down/up, 7/6 right/left.
    const [ox, oy] = await host.origin();
    const x = Math.trunc(ox + box[0] + box[2] / 2), y = Math.trunc(oy + box[1] + box[3] / 2);
    xdotool("mousemove", "--sync", x, y);
    await sleep(.3);
    const start = position ? await js(position) : 0;
    await js("__perf.start(); return 1");
    const [forward, back] = axis === "y" ? ["5", "4"] : ["7", "6"];
    xdotool("click", "--repeat", "24", "--delay", "40", forward);
    const middle = position ? await settle(position) : 0;
    xdotool("click", "--repeat", "24", "--delay", "40", back);
    if (position) await settle(position); else await sleep(1);
    const result = await js("return __perf.stop()");
    result.moved = position ? Math.abs(middle - start) : null;
    return result;
  };
  const brief = r => `p95 ${r.p95} max ${r.max} dropped ${r.dropped}/${r.frames} long ${r.long} handlers ${r.handlerMs}ms(max ${r.handlerMax}) layout max ${r.layoutMax} resize ${r.resizeMs}ms dom ${r.mutations}`;

  const knownUsed = [];
  const limitsFor = (key, where, values) => {
    const allow = KNOWN[key] ?? {};
    for (const [k, v] of Object.entries(allow))
      if ((values[k] ?? 0) > LIMITS[k]) knownUsed.push(`${where}: ${k} ${values[k]} (target ${LIMITS[k]}, allowed ${v})`);
    return {...LIMITS, ...allow};
  };
  const judge = (where, r, problems, key) => {
    if (r.seen && r.seen.slice(1, -1).some(d => Math.abs(d) > 2)) problems.push(`${where}: the scroll position did not follow the drag: ${JSON.stringify(r.seen)}`);
    const share = r.frames ? r.dropped / r.frames : 0;
    const limits = limitsFor(key, where, {...r, dropped_share: Math.round(share * 1000) / 1000});
    for (const k of ["p95", "max", "handlerMax", "layoutMax", "resizeMax"])
      if (r[k] > limits[k]) problems.push(`${where}: ${k} ${r[k]} > ${limits[k]}`);
    if (r.long > limits.long) problems.push(`${where}: ${r.long} long frames (over 50 ms): ${JSON.stringify(r.slow)}`);
    if (share > limits.dropped_share) problems.push(`${where}: ${r.dropped} of ${r.frames} frames dropped one or more frames: ${JSON.stringify(r.slow)}`);
  };

  const idle = async (results, problems) => {
    for (const [name, query, ready] of SURFACES) {
      if (picked.length && !picked.includes(name)) continue;
      await openPage(query, ready);
      await js(IDLE);
      await wait("window.__idle.done", 900);
      const found = await js("return window.__idle");
      // The first difference between the last two texts of each project, if the poll sent the same project twice.
      found.factsChanged = await js(String.raw`return Object.entries(window.__facts ?? {}).flatMap(([k, l]) => { if (l.length < 2 || l[0].text === l[1].text) return [];
          const a = l[0].text, b = l[1].text; let i = 0; while (i < a.length && a[i] === b[i]) i++; return [{project: k, at: [l[0].at, l[1].at], length: [a.length, b.length], before: a.slice(Math.max(0, i - 160), i + 120), after: b.slice(Math.max(0, i - 160), i + 120)}]; })`);
      (results.idle ??= {})[name] = found;
      found.factsCost = await js("const c = window.__factsCost ?? []; const by = {}; for (const [at, ms, n] of c) { const k = Math.round(at / 1000); (by[k] ??= [0, 0, 0]); by[k][0]++; by[k][1] += ms; by[k][2] += n; } return by");
      console.log(`idle ${name}: facts changed between polls: ${JSON.stringify(found.factsChanged.slice(0, 2))}; facts compared per second (count, ms, chars): ${JSON.stringify(found.factsCost)}`);
      for (const b of found.bursts) b.targets = Object.fromEntries(Object.entries(b.targets).sort((x, y) => y[1] - x[1]).slice(0, 8));
      console.log(`idle ${name}: slow frames ${JSON.stringify(found.slow.slice(0, 20))} DOM bursts ${JSON.stringify(found.bursts.filter(b => b.n > 20).slice(0, 12))}`);
      for (const [t, ms] of found.slow) if (ms > 50) problems.push(`idle ${name}: a ${ms} ms frame at ${t} s with nothing touched`);
    }
  };

  const results = {engine: "WebKitGTK", version: host.version, compositing: !args.includes("flat"), window: [WIDTH, HEIGHT], limits: LIMITS, surfaces: {}};
  const problems = [];
  let exit;
  try {
    await host.resize(WIDTH, HEIGHT);
    if (args.includes("idle")) await idle(results, problems);
    const shared = new Set();
    for (const [name, query, ready, pan] of args.includes("idle") ? [] : SURFACES) {
      if (picked.length && !picked.includes(name)) continue;
      await openPage(query, ready);
      await capture(name);
      const found = await js(FIND);
      if (LIVE.includes(name)) await js("window.__updates=0;window.__live=setInterval(()=>{liveUpdate();__updates++},400);return 1");
      results.engineFeatures ??= {requestIdleCallback: found.idleCallback, contentVisibility: found.contentVisibility};
      const surface = results.surfaces[name] = {nodes: found.nodes, scrollers: []};
      for (const s of found.scrollers) {
        if (SHARED.includes(s.name)) {
          if (shared.has(s.name)) continue;
          shared.add(s.name);
        }
        const where = `${name} ${s.name}`;
        const position = sub(sub(POSITION, "ID", String(s.id)), "AXIS", JSON.stringify(s.axis));
        // The first pass is the owner's first scroll through it; the second shows what is left once it has been seen.
        const entry = {...s, drag: await drag(s)};
        await sleep(.3);
        entry.dragAgain = await drag(s);
        await sleep(.3);
        entry.wheel = await wheel(s.box, s.axis, position);
        surface.scrollers.push(entry);
        for (const kind of ["drag", "dragAgain", "wheel"]) judge(`${where} ${kind}`, entry[kind], problems, `${name}|${s.name}|${kind}`);
        if (!entry.wheel.moved) problems.push(`${where}: the wheel did not move it`);
        for (const kind of ["drag", "dragAgain", "wheel"]) console.log(`${where} (${s.range} px) ${kind}: ${brief(entry[kind])}`);
      }
      if (pan) {
        const box = await js(`const r=document.querySelector(${JSON.stringify(pan)}).getBoundingClientRect();return [r.left,r.top,r.width,r.height].map(Math.round)`);
        surface.pan = await wheel(box, "y", null);
        judge(`${name} ${pan} wheel`, surface.pan, problems, `${name}|${pan}|wheel`);
        console.log(`${name} ${pan} wheel: ${brief(surface.pan)}`);
      }
      if (!surface.scrollers.length && !pan) problems.push(`${name}: nothing on it scrolls far enough to measure`);
      if (LIVE.includes(name)) {
        surface.updates = await js("clearInterval(window.__live);return window.__updates");
        surface.updateCost = await js("const c=window.liveCost;return {updates:c.updates,dataMs:Math.round(c.data),appMs:Math.round(c.app),appMax:Math.round(c.appMax)}");
        console.log(`${name}: ${surface.updates} updates arrived while scrolling; cost (fixture data, app) ${JSON.stringify(surface.updateCost)}`);
        const most = limitsFor(`${name}||update`, name, {updateMax: surface.updateCost.appMax}).updateMax;
        if (surface.updateCost.appMax > most) problems.push(`${name}: an update took ${surface.updateCost.appMax} ms to take in > ${most}`);
        if (surface.updates < 10) problems.push(`${name}: only ${surface.updates} updates arrived`);
      }
    }
    results.problems = problems; results.known = knownUsed;
    writeJson(join(evidence, `${label}-metrics.json`), results, 2);
    for (const k of knownUsed) console.log("KNOWN " + k);
    for (const p of problems) console.log("PROBLEM " + p);
    console.log(`scroll gate: ${problems.length} problems, ${knownUsed.length} known shortfalls within their ratchets`);
    exit = problems.length ? 1 : 0;
  } catch (error) {
    exit = 1;
    console.error(error.name + ": " + error.message);
  }
  return exit;
});
