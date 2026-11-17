// Disposable WebKitGTK screenshots and real-input checks; fake port, no model calls.
// Run inside the rendered-check container with /site (a built workspace bundle) and /evidence mounted, with this directory at /driver:
//   docker run --rm --network host -v SITE:/site:ro -v EVIDENCE:/evidence -v packages/workspace-ui/test:/driver:ro \
//     cortex-workspace-rendered:ubuntu26.04 node /driver/timeline-clarity.render.mjs
import {Host, startXvfb, engineEnv, main, sleep, xdotool, startOpenbox, serveSite, join} from "./rendered/webkit.mjs";

await main(async () => {
  const display = await startXvfb();
  startOpenbox();
  const origin = await serveSite("/site");
  const host = await Host.start({width: 1440, height: 1000, decorated: false, env: engineEnv(display), timeout: 12});
  await host.move(0, 0);
  const facts = {};
  const js = async body => { try { return await host.js(body); } catch (error) { if (error.timeout) throw Error("TimeoutError: WebKit did not answer"); return {error: error.message}; } };
  const num = n => { const f = Math.floor(n), d = n - f; return d < .5 ? f : d > .5 ? f + 1 : f % 2 === 0 ? f : f + 1; }; // Python's round(): halves go to the even integer
  const shot = async name => { await host.capture(join("/evidence", name + ".png"), {root: true}); };

  async function load(path) {
    await host.load(origin + path);
    for (let i = 0; i < 70; i++) {
      await sleep(.1);
      if (await js("return !!window.fixture && !!document.querySelector('.titlebar')") === true) { await sleep(.2); return; }
    }
    throw Error("TimeoutError: Fixture failed to load: " + JSON.stringify(await js("return {errors:window.fixtureErrors,fixture:!!window.fixture,body:document.body.textContent.slice(0,300)}")));
  }
  async function click(selector) {
    const point = await js("const el=document.querySelector(" + JSON.stringify(selector) + "); el.scrollIntoView({block:'nearest'}); const r=el.getBoundingClientRect(); return [r.x+r.width/2,r.y+r.height/2]");
    xdotool("mousemove", num(point[0]), num(point[1]), "click", "1");
    await sleep(.15);
  }
  const assert = (ok, message) => { if (!ok) throw Error("AssertionError: " + message); };

  try {
    await load("/m/local/p/p/board");
    await click(".card-a .btn-primary");
    facts.busy = await js("return {busy:!!document.querySelector('.card [aria-busy=true]'),commands:fixture.commands.length}");
    await shot("desktop-board-busy");
    assert(facts.busy.busy, "Start did not stay busy: " + JSON.stringify(facts.busy));
    await sleep(3);
    await shot("desktop-board-refused");
    facts.refusal = await js("return {inline:document.querySelector('.card [role=alert]')?.textContent,global:document.querySelector('.ws-receipt')?.textContent,errors:fixtureErrors}");
    assert(facts.refusal.global, "No visible refusal: " + JSON.stringify(facts.refusal));
    facts.board_start_commands = await js("return fixture.commands.length");
    assert(facts.board_start_commands === 1, "board start commands " + facts.board_start_commands);
    // Real drag events: retain the explicit image geometry when WebKit requests it.
    await js("window.dragFacts=[];const original=DataTransfer.prototype.setDragImage;DataTransfer.prototype.setDragImage=function(el,x,y){const r=el.getBoundingClientRect();dragFacts.push([r.width,r.height]);return original.call(this,el,x,y)};return true");
    const point = await js("const el=document.querySelector('.card-link'); const r=el.getBoundingClientRect();window.beforeDrag=[el.closest('.card').getBoundingClientRect().width,el.closest('.card').getBoundingClientRect().height];return[r.x+20,r.y+20]");
    xdotool("mousemove", num(point[0]), num(point[1]), "mousedown", "1", "mousemove", "--sync", num(point[0] + 90), num(point[1] + 40));
    await sleep(.3);
    await shot("desktop-board-drag");
    facts.drag_image = await js("return {source:beforeDrag,images:dragFacts}");
    const drop = await js("const r=document.querySelector('[data-col=done]').getBoundingClientRect();return[r.x+r.width/2,r.y+40]");
    xdotool("mousemove", num(drop[0]), num(drop[1]), "mouseup", "1");
    await sleep(.8);
    xdotool("key", "Escape");
    await sleep(.3);
    assert(facts.drag_image.images.length, "No native drag event observed");
    assert(Math.abs(facts.drag_image.source[0] - facts.drag_image.images[0][0]) < 2, "drag image width differs: " + JSON.stringify(facts.drag_image));
    await load("/m/local/t/live");
    await shot("desktop-conversation");
    await click(".wrow");
    facts.worker_open = await js("return {path:location.pathname,stream:!!document.querySelector('.livepane .stream'),errors:fixtureErrors}");
    assert(facts.worker_open.stream, "Worker did not open: " + JSON.stringify(facts.worker_open));
    await click(".ws-step-detail details summary");
    await shot("desktop-worker");
    facts.worker = await js("return {steps:document.querySelectorAll('.ev').length,raw:document.querySelector('.livepane').textContent.includes('mcp__'),command:document.querySelector('.livepane').textContent.includes('python3 -B check.py'),viewport:[innerWidth,innerHeight]}");
    assert(!facts.worker.raw && facts.worker.command, "worker text: " + JSON.stringify(facts.worker));
    await click(".ws-step-path");
    await sleep(.5);
    facts["file-open"] = await js(`return {path:location.pathname+location.search,selected:!!document.querySelector('[role=tab][aria-label="index.html"]'),panel:document.querySelector('.task-panel')?.textContent.slice(0,200),errors:fixtureErrors}`);
    await shot("desktop-read-file");
    assert(facts["file-open"].selected, "Read did not open its file: " + JSON.stringify(facts["file-open"]));
    await load("/m/local/t/live/workers/w");
    await click(".ws-step-detail details summary");
    await click('[aria-label="Sidebar"]');
    for (const [width, name] of [[768, "tablet-worker"], [390, "mobile-worker"]]) {
      await host.resize(width, 1000);
      await sleep(.4);
      xdotool("mousemove", "1900", "1190");
      await sleep(.3);
      await shot(name);
      facts[name] = await js("return {viewport:[innerWidth,innerHeight],client:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth}");
      assert(facts[name].client === facts[name].scroll, "Page overflow at " + name);
    }
    await host.resize(1440, 1000);
    await load("/m/local/p/p/s/sp/planning");
    facts["single-node-graph"] = await js("const g=document.querySelector('.depg svg');const r=g?.getBoundingClientRect(); return {width:r?.width,height:r?.height,viewBox:g?.getAttribute('viewBox'),errors:fixtureErrors}");
    assert(facts["single-node-graph"].width > 0 && facts["single-node-graph"].width < 400, "Graph was enlarged: " + JSON.stringify(facts["single-node-graph"]));
    await shot("desktop-single-node-graph");
    console.log(JSON.stringify(facts, null, 2));
    return 0;
  } catch (error) {
    console.log(JSON.stringify({failure: `${error.name}(${JSON.stringify(error.message)})`, facts}));
    return 1;
  }
});
