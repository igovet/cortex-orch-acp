// The sprint plan's Graph view in WebKitGTK, on the layout gate's fake data and private virtual screen. Run through sprint-graph-render.mjs.
//
// It captures the screens the owner reviews (Graph, Full view, a search match, a selection, a collapsed layer and the light
// theme at four window sizes), checks that no two regions of the screen overlap, and times pan, zoom, selection and filter on
// a 200-task plan. Captures and numbers go to the evidence folder; a region overlap or a missing screen fails the run. The
// frame times are measured, not judged: they are for this software-rendered virtual screen, not for the owner's desktop.
import {Host, startXvfb, engineEnv, main, sleep, probe, join, writeJson} from "./rendered/webkit.mjs";
import assert from "node:assert/strict";

const base = process.argv[2], modes = process.argv.slice(3), evidence = "/evidence";
// `bench` alone times the 200-task plan without the captures; `composited` keeps WebKit's accelerated compositing on (software
// GL here), as the desktop window has it, where the layout gate turns it off for stable pixels.
const REGIONS = probe("sprint-graph-render.REGIONS.js"), BENCH = probe("sprint-graph-render.BENCH.js");
const route = sprint => "route=" + encodeURIComponent(`/m/<M>/p/p/s/${sprint}/plan`);
const READY = "document.querySelector('.gsv .n') && document.querySelector('.gv-band') && !document.querySelector('.l-startup')";
const PRESS = sel => `const n=document.querySelector(${sel});for(const t of ['pointerdown','pointerup'])n.dispatchEvent(new PointerEvent(t,{bubbles:true,button:0,clientX:0,clientY:0}));return 1`;
const CLICK = (sel, text) => `[...document.querySelectorAll(${sel})].find(e=>e.textContent.trim()===${text}).click();return 1`;
const SEARCH = text => `const f=document.querySelector('[aria-label="Search tasks in the graph"]');f.value=${text};f.dispatchEvent(new Event('input',{bubbles:true}));return 1`;

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1440, height: 900, env: engineEnv(display, {compositing: modes.includes("composited")}), timeout: 30});
  const js = code => host.js(code), wait = (code, tries = 150) => host.wait(code, tries);
  let loads = 0;
  const openPage = async (query, width, height, ready) => {
    loads++;
    await host.resize(width, height);
    await host.load(base + `?${query}&load=${loads}`);
    await wait(`window.fixture === '${loads}'`);
    await wait(ready);
    await sleep(.6);
    const actual = await js("return [innerWidth, innerHeight]");
    assert.deepEqual(actual, [width, height], JSON.stringify([actual, width, height]));
  };
  const capture = async name => { await sleep(.4); const size = await host.size(); await host.capture(join(evidence, name + ".png")); return size; };

  const results = {screens: {}}, problems = [];
  try {
    const sizes = modes.includes("bench") ? [] : [[1920, 1080], [1440, 900], [1280, 720], [1024, 640]];
    for (const [width, height] of sizes) {
      const tag = `${width}x${height}`;
      const dark = "graph=1&dark=1&" + route("big");
      await openPage(dark, width, height, READY);
      const shot = async name => {
        const found = await js(REGIONS);
        results.screens[`${name}-${tag}`] = found;
        problems.push(...found.overlaps.map(o => `${name}-${tag}: ${o}`));
        await capture(`${name}-${tag}`);
      };
      await shot("graph");
      await js(CLICK(JSON.stringify(".gv-bar button"), JSON.stringify("Full view")));
      await wait("document.querySelector('.sp-body[data-full]')");
      await sleep(.4);
      await shot("full-view");
      await js(CLICK(JSON.stringify(".gv-bar button"), JSON.stringify("Exit full view")));
      await wait("!document.querySelector('.sp-body[data-full]')");
      await js(SEARCH(JSON.stringify("export")));
      await wait("document.querySelector('.gsv .n.cur')");
      await sleep(.4);
      await shot("search-match");
      await js(SEARCH(JSON.stringify("")));
      await wait("!document.querySelector('.gsv .n.dim')");
      await js(PRESS(JSON.stringify('.gsv .n[data-id="t:big-12"]')));
      await wait("document.querySelector('.gsv .n.picked')");
      await shot("selection");
      await js(PRESS(JSON.stringify(".gv-canvas")));
      await wait("!document.querySelector('.gsv .n.picked')");
      await js(PRESS(JSON.stringify('.gsv .lh[data-l="2"] rect')));
      await wait("document.querySelector('.gsv .band')");
      await shot("collapsed-layer");
      await openPage("graph=1&" + route("big"), width, height, READY);
      await shot("light-theme");
    }
    // Frame times on 200 tasks, in the pane at 1440x900 with the planning conversation beside it.
    await openPage("graph=200&dark=1&" + route("huge"), 1440, 900, READY);
    await js("const t0=performance.now();document.querySelector('.sp-plan .seg button').click();requestAnimationFrame(()=>requestAnimationFrame(()=>{window.listAt=performance.now()-t0}));return 1");
    await wait("window.listAt !== undefined");
    await js("window.graphAt=undefined;const t0=performance.now();[...document.querySelectorAll('.sp-plan .seg button')].find(b=>b.textContent==='Graph').click();requestAnimationFrame(()=>requestAnimationFrame(()=>{window.graphAt=performance.now()-t0}));return 1");
    await wait("window.graphAt !== undefined && document.querySelector('.gsv .n')");
    results.open_graph_200_ms = await js("return +window.graphAt.toFixed(1)");
    await sleep(.5);
    await js(BENCH);
    await wait("window.bench", 600);
    results.bench_200 = await js("return window.bench");
    await capture("bench-200-1440x900" + modes.map(m => "-" + m).join(""));
    results.problems = problems;
    writeJson(join(evidence, "render-metrics" + modes.map(m => "-" + m).join("") + ".json"), results, 2);
    const screens = Object.fromEntries(Object.entries(results.screens).map(([k, v]) => [k, {zoom: v.zoom, lod: v.lod, sidebar: v.sidebar, full: v.full, map: v.map, canvas: v.regions.canvas ?? null}]));
    console.log(JSON.stringify({bench_200: results.bench_200, open_graph_200_ms: results.open_graph_200_ms, screens, problems}, null, 1));
    return problems.length ? 1 : 0;
  } catch (error) {
    console.error(error.name + ": " + error.message);
    return 1;
  }
});
