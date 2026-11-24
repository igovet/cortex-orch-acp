// A created sprint's live graph in WebKitGTK, on the layout gate's fake data with `live` (the service's graph, two drafts added
// after the plan) and a private virtual screen. Run through live-graph-render.mjs.
//
// It captures what the owner reviews at three window sizes: the Live sprint graph, Edit links with its worded preview, the Live
// list with priority arrows, a later draft's Details with its links, the approved plan, and the sprint's Board. Each screen is
// checked for overlapping regions; an overlap or a screen that never reaches its state fails the run.
import {Host, startXvfb, engineEnv, main, sleep, probe, join, writeJson} from "./rendered/webkit.mjs";

const base = process.argv[2], evidence = "/evidence";
const REGIONS = probe("sprint-graph-render.REGIONS.js");
const route = path => "route=" + encodeURIComponent(path);
const PRESS = sel => `const n=document.querySelector(${JSON.stringify(sel)});for(const t of ['pointerdown','pointerup'])n.dispatchEvent(new PointerEvent(t,{bubbles:true,button:0,clientX:0,clientY:0}));return 1`;
const CLICK = (sel, text) => `[...document.querySelectorAll(${JSON.stringify(sel)})].find(e=>e.textContent.trim()===${JSON.stringify(text)}).click();return 1`;
const ESC = "document.body.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));return 1";
const EDITABLE = "[...document.querySelectorAll('.gv-bar button')].some(b=>b.textContent.trim()==='Edit links'&&!b.hasAttribute('aria-disabled'))";

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1440, height: 900, env: engineEnv(display), timeout: 30});
  const js = code => host.js(code), wait = (code, tries = 150) => host.wait(code, tries);
  let loads = 0;
  const openPage = async (query, width, height, ready) => {
    loads++;
    await host.resize(width, height);
    await host.load(base + `?${query}&load=${loads}`);
    await wait(`window.fixture === '${loads}'`);
    await wait(ready);
    await sleep(.6);
  };
  const results = {screens: {}}, problems = [];
  const shot = async name => {
    await sleep(.4);
    const found = await js(REGIONS);
    results.screens[name] = {size: await host.size(), regions: found.regions, overlaps: found.overlaps};
    problems.push(...found.overlaps.map(o => `${name}: ${o}`));
    await host.capture(join(evidence, name + ".png"));
  };
  try {
    for (const [width, height] of [[1920, 1080], [1440, 900], [1024, 640]]) {
      const tag = `${width}x${height}`;
      await openPage("graph=1&live=1&dark=1&" + route("/m/<M>/p/p/s/big/plan"), width, height, `document.querySelector('.gsv .n') && ${EDITABLE}`);
      await shot(`live-graph-${tag}`);
      await js(CLICK(".gv-bar button", "Edit links"));
      await wait("document.querySelector('.gv-bar button[aria-pressed=\"true\"]')");
      await js(PRESS('.gsv .n[data-id="t:big-3"]'));
      await js(PRESS('.gsv .n[data-id="t:big-later-1"]'));
      await wait("document.querySelector('.gv-band')?.textContent.includes('Add a link')");
      await shot(`edit-links-preview-${tag}`);
      for (let i = 0; i < 3; i++) await js(ESC);
      await wait("document.querySelector('.gv-bar button[aria-pressed=\"false\"]')");
      await js(CLICK(".sp-plan .seg button", "List"));
      await wait("document.querySelector('.sp-plan .ws-prio')");
      await shot(`live-list-${tag}`);
      await js("document.querySelector('button[aria-label=\"Links and details of task 61\"]').click();return 1");
      await wait("document.querySelector('.ws-read .ws-live-links')");
      await shot(`live-details-${tag}`);
      await js(CLICK(".sp-plan .seg button", "Approved plan v1"));
      await wait("document.querySelector('.sp-plan[aria-label=\"Plan v1\"]')");
      await shot(`approved-plan-${tag}`);
      await openPage("graph=1&live=1&dark=1&" + route("/m/<M>/p/p/s/big/board"), width, height, "document.querySelector('.board-wrap .card')");
      await shot(`sprint-board-${tag}`);
    }
    results.problems = problems;
    writeJson(join(evidence, "render-metrics.json"), results, 2);
    console.log(JSON.stringify({screens: Object.keys(results.screens), problems}, null, 1));
    return problems.length ? 1 : 0;
  } catch (error) {
    console.error(error.name + ": " + error.message);
    return 1;
  }
});
