// Real WebKitGTK startup/streaming regression; only disposable content and Xvfb. Run through feed-scroll.mjs.
import {Host, startXvfb, engineEnv, main, sleep, xdotool, have, join, writeJson, sorted} from "./rendered/webkit.mjs";
import assert from "node:assert/strict";

const base = process.argv[2], label = process.argv[3], evidence = "/evidence";

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1440, height: 1000, env: engineEnv(display)});
  const js = code => host.js(code), wait = code => host.wait(code, 180);
  const load = async (name, width = 1440, height = 1000, length = 240) => {
    await host.resize(width, height);
    await host.load(base + "?view=" + name + "&length=" + length);
    await wait("window.fixture && document.querySelector('.titlebar')");
    await sleep(.4);
  };
  const capture = async name => { await sleep(.15); await host.capture(join(evidence, name + ".png")); };
  const measure = selector => js("const b=document.querySelector(" + JSON.stringify(selector) + ");return {top:b.scrollTop,height:b.scrollHeight,client:b.clientHeight,bottom:(getComputedStyle(b).display==='flex'&&getComputedStyle(b).flexDirection==='column-reverse')?Math.abs(b.scrollTop):Math.abs(b.scrollHeight-b.clientHeight-b.scrollTop),following:document.body.dataset.follow};");
  const p95 = values => values[Math.floor(values.length * .95)];

  async function streamTest(name) {
    await load(name);
    const selector = name === "worker" ? ".stream" : ".timeline";
    await wait("document.querySelector(" + JSON.stringify(selector) + ")");
    await sleep(.2);
    await js("window.b=document.querySelector(" + JSON.stringify(selector) + ");window.reverse=(getComputedStyle(b).display==='flex'&&getComputedStyle(b).flexDirection==='column-reverse');");
    const results = {initial: await measure(selector)};
    await js("fixture.stream();fixture.append();");
    await sleep(.2);
    results.pinned = await measure(selector);
    await js("b.scrollTop=reverse?-550:b.scrollHeight-b.clientHeight-550;");
    await sleep(.15);
    await js("const y=b.getBoundingClientRect().top;window.anchor=[...b.firstElementChild.children].find(r=>r.getBoundingClientRect().bottom>y+80);window.anchorY=anchor.getBoundingClientRect().top;window.readTop=b.scrollTop;fixture.stream();fixture.append();");
    await sleep(.2);
    results.reading = await js("return {topDelta:b.scrollTop-readTop,anchorDelta:anchor.getBoundingClientRect().top-anchorY};");
    await js("window.anchorY=anchor.getBoundingClientRect().top;fixture.expand();");
    await sleep(.2);
    results.expand = await js("return {anchorDelta:anchor.getBoundingClientRect().top-anchorY};");
    await capture(name + "-reading-1440");
    await js("b.scrollTop=reverse?0:b.scrollHeight-b.clientHeight;");
    await sleep(.1);
    await js("fixture.stream();");
    await sleep(.2);
    results.resumed = await measure(selector);
    // Real XTEST input grabs the scrollbar, rather than calling its handlers.
    assert.ok(have("xdotool"), "XTEST pointer driver unavailable");
    const xy = await js("const r=b.getBoundingClientRect();return [Math.round(r.right-5),Math.round(r.bottom-80)];");
    xdotool("mousemove", xy[0], xy[1], "mousedown", "1");
    await sleep(.1);
    const dragTop = await js("return b.scrollTop;");
    await js("fixture.stream();fixture.append();");
    await sleep(.15);
    results.dragAppendDelta = (await js("return b.scrollTop;")) - dragTop;
    xdotool("mousemove", xy[0], xy[1] - 120);
    await sleep(.15);
    const moved = await js("return b.scrollTop;");
    xdotool("mouseup", "1");
    results.dragMoved = Math.abs(moved - dragTop);
    // Frame intervals while content grows; no timing claim inferred from exit code.
    await js("window.frames=null;const intervals=[];let i=0,last=performance.now();const tick=t=>{intervals.push(t-last);last=t;if(i%20===0)fixture.stream();if(++i<120)requestAnimationFrame(tick);else frames=intervals;};requestAnimationFrame(tick);");
    await wait("Array.isArray(window.frames)");
    const values = sorted(await js("return frames;"));
    results.frames = {p95: p95(values), max: Math.max(...values), count: values.length};
    await capture(name + "-bottom-1440");
    if (label !== "before") {
      const text = JSON.stringify(results);
      assert.ok(results.pinned.bottom <= 1, text);
      assert.ok(Math.abs(results.reading.topDelta) <= 1 && Math.abs(results.reading.anchorDelta) <= 1, text);
      assert.ok(Math.abs(results.expand.anchorDelta) <= 1, text);
      assert.ok(results.resumed.bottom <= 1, text);
      assert.ok(Math.abs(results.dragAppendDelta) <= 1 && results.dragMoved > 5, text);
    }
    return results;
  }

  const wrong = c => c.includes("Planning fixture") || c.includes("Archived fixture");
  const results = {engine: "WebKitGTK", version: host.version, label};
  let exit = 0;
  try {
    await load("startup");
    await capture("startup-pending-1440");
    await wait("fixture.done?.()");
    results.startup = await js("return {samples:fixture.samples,taskCount:fixture.info.taskCount,errors:fixture.errors??[]};");
    assert.ok(results.startup.samples.some(s => s.cards.length), JSON.stringify(results.startup.errors));
    await capture("startup-ready-1440");
    if (label !== "before") {
      // Planning and archived tasks must not appear even for one observed frame.
      for (const sample of results.startup.samples)
        assert.ok(!sample.cards.some(wrong), JSON.stringify(sample));
    }
    results.startup.firstCardsMs = results.startup.samples.find(s => s.cards.length)?.ms ?? null;
    const samples = results.startup.samples;
    delete results.startup.samples;
    writeJson(join(evidence, "startup-samples.json"), samples);
    results.startup.frames = samples.length;
    results.startup.wrongFrames = samples.filter(s => s.cards.some(wrong)).length;
    await js("fixture.refresh();");
    await wait("fixture.done?.()");
    results.refresh = await js("return {frames:fixture.samples.length,wrongFrames:fixture.samples.filter(s=>s.cards.some(c=>/Planning fixture|Archived fixture/.test(c))).length,emptyFrames:fixture.samples.filter(s=>!s.cards.length).length};");
    if (label !== "before") assert.ok(results.refresh.wrongFrames === 0 && results.refresh.emptyFrames === 0, JSON.stringify(results.refresh));
    if (label !== "before") {
      await js("fixture.failRefresh();");
      await wait("document.querySelector('[role=alert]')?.textContent.includes('Workspace refresh failed')");
      await capture("refresh-error-1440");
      results.refreshError = await js("return {cards:document.querySelectorAll('.card').length,connected:document.querySelector('.machine .st')?.textContent};");
      assert.deepEqual(results.refreshError, {cards: 3, connected: "Connected"}, JSON.stringify(results.refreshError));
      await js("fixture.recover();");
      await wait("!document.querySelector('[role=alert]')");
      await load("startup-error");
      await wait("document.querySelector('[role=status]')?.textContent.includes('could not be loaded')");
      await capture("startup-error-1440");
      await js("fixture.recover();");
      await wait("document.querySelectorAll('.card').length===3");
      results.retryRecovered = true;
    }
    results.worker = await streamTest("worker");
    results.conversation = await streamTest("conversation");
    for (const [width, height] of [[900, 1000], [390, 844]]) {
      await load("conversation", width, height);
      await capture("conversation-" + width);
      results["viewport" + width] = await js("return {width:innerWidth,height:innerHeight,overflow:document.documentElement.scrollWidth-innerWidth};");
    }
    for (const name of ["tools", "conversation-tools"]) {
      await load(name);
      await wait("document.querySelector('button.fold')");
      await capture(name + "-collapsed-1440");
      await js("document.querySelector('button.fold').click();");
      await sleep(.2);
      await capture(name + "-open-1440");
      const rows = await js("return [...document.querySelectorAll('.step-row')].map(e=>e.textContent);");
      assert.ok(rows.length === 4 && !rows.some(r => r.includes("mcp__") || r.includes("Read skill") || r.includes("Submit result")), JSON.stringify(rows));
      results[name] = {rows};
    }
    await load("conversation", 1440, 1000, 2400);
    await wait("document.querySelectorAll('.timeline .ai').length===2400");
    await js("window.frames=null;const b=document.querySelector('.timeline'),intervals=[];let i=0,last=performance.now();const reverse=getComputedStyle(b).display==='flex';const tick=t=>{intervals.push(t-last);last=t;b.scrollTop=(reverse?-1:1)*((++i*591)%(b.scrollHeight-b.clientHeight));if(i<150)requestAnimationFrame(tick);else frames=intervals;};requestAnimationFrame(tick);");
    await wait("Array.isArray(window.frames)");
    const values = sorted(await js("return frames;"));
    results.longHistory = {entries: 2400, p95: p95(values), max: Math.max(...values), frames: values.length};
    await capture("conversation-long-1440");
  } catch (error) {
    exit = 1;
    results.error = error.name + ": " + error.message;
  } finally {
    if (results.startup?.samples) {
      const samples = results.startup.samples;
      delete results.startup.samples;
      writeJson(join(evidence, "startup-samples.json"), samples);
      Object.assign(results.startup, {frames: samples.length, wrongFrames: samples.filter(s => s.cards.some(wrong)).length});
    }
    writeJson(join(evidence, "metrics.json"), results, 2);
    console.log(JSON.stringify(results));
  }
  return exit;
});
