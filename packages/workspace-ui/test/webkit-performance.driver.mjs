// Opt-in static-bundle WebKitGTK measurements; fake data, private virtual screen. Run through webkit-performance.mjs.
import {Host, startXvfb, engineEnv, main, sleep, join, writeJson} from "./rendered/webkit.mjs";
import assert from "node:assert/strict";

const base = process.argv[2], label = process.argv[3], sources = JSON.parse(process.argv[4]), evidence = "/evidence";

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1440, height: 1000, env: engineEnv(display), timeout: 30});
  const js = code => host.js(code), wait = code => host.wait(code, 150);
  const load = async (query = "") => { await host.load(base + query); await wait("window.fixture && document.querySelector('.titlebar')"); };
  const capture = async name => { await host.capture(join(evidence, label + "-" + name + ".png")); };

  async function bench(selector) {
    await js(`const el=document.querySelector(${JSON.stringify(selector)});
      window.stats=null; const intervals=[],costs=[];let mutations=0,frames=0,last=performance.now();
      const observer=new MutationObserver(r=>mutations+=r.length);observer.observe(el,{subtree:true,childList:true,characterData:true,attributes:true});
      const reverse=getComputedStyle(el).flexDirection==='column-reverse';
      const tick=now=>{intervals.push(now-last);last=now;
        if(frames===30 || frames===90) fixture.emit(true);
        const start=performance.now();el.scrollTop=(reverse?-1:1)*((frames/119)*(el.scrollHeight-el.clientHeight));
        el.getBoundingClientRect();costs.push(performance.now()-start);
        if(++frames<120)requestAnimationFrame(tick);else{observer.disconnect();const sorted=[...intervals].sort((a,b)=>a-b);
          window.stats={frames,slowFrames:intervals.flatMap((ms,frame)=>ms>50?[{frame,ms}]:[]),p50:sorted[Math.floor(sorted.length*.5)],p95:sorted[Math.floor(sorted.length*.95)],max:Math.max(...sorted),over50:sorted.filter(n=>n>50).length,scrollCostMax:Math.max(...costs),mutations,nodes:el.querySelectorAll('*').length,height:el.scrollHeight};}};
      requestAnimationFrame(tick);`);
    await wait("window.stats");
    const result = await js("return window.stats");
    assert.ok(result.frames === 120 && result.height > 10000 && result.nodes > 1000, JSON.stringify(result));
    return result;
  }

  async function reportAt(width, height) {
    await host.resize(width, height);
    await wait(`innerWidth===${width} && innerHeight===${height}`);
    await js("document.querySelector('dialog[open] .dlg-b').scrollTop=0");
    await sleep(.1);
    await capture("report-" + width);
    const geometry = await js(`const b=document.querySelector('dialog[open] .dlg-b');
      const copy=b.querySelector('.ws-result-copy'), r=copy.getBoundingClientRect();
      return {width:innerWidth,height:innerHeight,pageOverflow:document.documentElement.scrollWidth-innerWidth,
        reportOverflow:b.scrollWidth-b.clientWidth,copyWidth:r.width,bodyWidth:b.clientWidth,
        font:getComputedStyle(copy).fontFamily,lineHeight:getComputedStyle(copy).lineHeight};`);
    assert.ok(geometry.reportOverflow <= 1, JSON.stringify(geometry));
    await js("const b=document.querySelector('dialog[open] .dlg-b'); b.scrollTop=b.scrollHeight");
    await sleep(.1);
    await capture("report-end-" + width);
    return geometry;
  }

  try {
    const results = {engine: "WebKitGTK", version: host.version, sources, label};
    await load();
    await wait("document.querySelector('.ws-worker-result button')");
    results.worker = await bench(".stream");
    await capture("worker-1440");
    await js("fixture.openedAt=performance.now(); document.querySelector('.ws-worker-result button').click(); const painted=()=>{if(document.querySelector('dialog[open] .ws-result-copy')) fixture.openMs=performance.now()-fixture.openedAt; else requestAnimationFrame(painted)}; requestAnimationFrame(painted)");
    await wait("document.querySelector('dialog[open] .ws-result-copy') && fixture.openMs");
    results.openMs = await js("return fixture.openMs");
    results.report = await bench("dialog[open] .dlg-b");
    results.reportShape = await js("const d=document.querySelector('dialog[open]');return {characters:fixture.characters,reads:fixture.reads.length,headings:d.querySelectorAll('h1,h2,h3').length,tables:d.querySelectorAll('table').length,lists:d.querySelectorAll('ol').length,codes:d.querySelectorAll('pre').length,last:d.textContent.includes('Final report paragraph.')}");
    const shape = results.reportShape;
    assert.ok(shape.tables === 1800 && shape.lists === 1800 && shape.codes === 1800 && shape.last, JSON.stringify(shape));
    assert.equal(await js("return getComputedStyle(document.querySelector('dialog[open] ol')).listStyleType"), "decimal");
    assert.equal(await js("return getComputedStyle(document.querySelector('.ev-note ul')).listStyleType"), "disc");
    assert.equal(await js("return getComputedStyle(document.querySelector('dialog[open] .ws-result-copy h1')).marginTop"), "8px");
    assert.ok(shape.reads > 1 && !(await js("return document.querySelector('dialog[open]').textContent.includes('Next page')")));
    results.viewports = [];
    for (const [w, h] of [[1440, 1000], [900, 1000], [390, 844]]) results.viewports.push(await reportAt(w, h));
    await host.resize(1440, 1000);
    await wait("innerWidth===1440");
    await load("?view=conversation");
    await wait("document.querySelectorAll('.timeline .ai').length===2400");
    results.conversation = await bench(".timeline");
    await capture("conversation-1440");
    // A same-content read must retain the node and the reader's place.
    results.stableRefresh = await js("const box=document.querySelector('.timeline'), row=box.querySelector('.ai'), at=box.scrollTop; fixture.emit(); return row===box.querySelector('.ai') && box.scrollTop===at");
    assert.ok(results.stableRefresh);
    writeJson(join(evidence, label + "-metrics.json"), results, 2);
    console.log(JSON.stringify(results));
    return 0;
  } catch (error) {
    console.error(error.name + ": " + error.message);
    return 1;
  }
});
