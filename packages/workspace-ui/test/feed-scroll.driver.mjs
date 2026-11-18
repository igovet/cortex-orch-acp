// Real WebKitGTK startup/streaming regression; only disposable content and Xvfb. Run through feed-scroll.mjs.
import {Host, startXvfb, engineEnv, main, sleep, xdotool, have, join, writeJson, sorted} from "./rendered/webkit.mjs";
import assert from "node:assert/strict";

const base = process.argv[2], label = process.argv[3], evidence = "/evidence";

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1440, height: 1000, env: engineEnv(display)});
  const js = code => host.js(code), wait = code => host.wait(code, 180);
  const load = async (name, width = 1440, height = 1000, length = 240, more = "") => {
    await host.resize(width, height);
    await host.load(base + "?view=" + name + "&length=" + length + more);
    await wait("window.fixture && document.querySelector('.titlebar')");
    await sleep(.4);
  };
  const capture = async name => { await sleep(.15); await host.capture(join(evidence, name + ".png")); };
  const measure = selector => js("const b=document.querySelector(" + JSON.stringify(selector) + ");return {top:b.scrollTop,height:b.scrollHeight,client:b.clientHeight,bottom:(getComputedStyle(b).display==='flex'&&getComputedStyle(b).flexDirection==='column-reverse')?Math.abs(b.scrollTop):Math.abs(b.scrollHeight-b.clientHeight-b.scrollTop),following:document.body.dataset.follow};");
  const p95 = values => values[Math.floor(values.length * .95)];
  // The feed's box ends at the dock's top edge, so its scrollbar does too: nothing of it lies beside or under the dock. Probed
  // at the scrollbar's own column, below the dock's top nothing is the feed; above it, under a shown Latest band, the feed's
  // scrollbar is still what is there.
  const geometry = (feed, dock) => js("const b=document.querySelector(" + JSON.stringify(feed) + "),d=document.querySelector(" + JSON.stringify(dock) + "),B=b.getBoundingClientRect(),D=d.getBoundingClientRect(),x=B.right-4,band=d.querySelector('.pill-band');" +
    "const hit=y=>{const e=document.elementFromPoint(x,y);return !!e&&b.contains(e);};const ys=[D.top+1,D.top+8,(D.top+D.bottom)/2,D.bottom-2].filter(y=>y>D.top&&y<Math.min(D.bottom,innerHeight));" +
    "const P=band&&band.getClientRects().length?band.getBoundingClientRect():null;" +
    "return {feedBottom:Math.round(B.bottom*10)/10,dockTop:Math.round(D.top*10)/10,gap:Math.round((D.top-B.bottom)*10)/10,scrollbar:b.offsetWidth-b.clientWidth,belowPoints:ys.length,feedBelow:ys.filter(hit).length,band:P?{top:Math.round(P.top),bottom:Math.round(P.bottom),scrollbarClear:hit((P.top+P.bottom)/2)}:null,bottom:Math.round((b.scrollHeight-b.clientHeight-b.scrollTop)*10)/10,pill:!!d.querySelector('.new-pill')?.getClientRects().length};");
  const geometryOk = (g, scrollbar = true) => Math.abs(g.gap) <= 1 && g.feedBottom <= g.dockTop + .05 && g.belowPoints >= 2 && g.feedBelow === 0 && (!scrollbar || g.scrollbar > 0) && (!g.band || g.band.scrollbarClear || !scrollbar);

  async function streamTest(name) {
    await load(name);
    const selector = name === "worker" ? ".stream" : ".timeline";
    await wait("document.querySelector(" + JSON.stringify(selector) + ")");
    await sleep(.2);
    await js("window.b=document.querySelector(" + JSON.stringify(selector) + ");window.reverse=(getComputedStyle(b).display==='flex'&&getComputedStyle(b).flexDirection==='column-reverse');");
    const dockSel = name === "worker" ? ".dock-w" : ".task-panel .dock";
    const results = {initial: await measure(selector), geometryEnd: await geometry(selector, dockSel)};
    await js("fixture.stream();fixture.append();");
    await sleep(.2);
    results.pinned = await measure(selector);
    // The reader scrolls up with the real wheel: a scroll without input is the engine's, and a following feed pins its end again.
    assert.ok(have("xdotool"), "XTEST pointer driver unavailable");
    const [ox, oy] = await host.origin();
    const at = await js("const r=b.getBoundingClientRect();return [Math.round(r.left+r.width/2),Math.round(r.top+160)];");
    xdotool("mousemove", "--sync", ox + at[0], oy + at[1]);
    xdotool("click", "--repeat", "5", "--delay", "40", "4");
    await sleep(.6);
    await js("const y=b.getBoundingClientRect().top;window.anchor=[...b.firstElementChild.children].find(r=>r.getBoundingClientRect().bottom>y+80);window.anchorY=anchor.getBoundingClientRect().top;window.readTop=b.scrollTop;fixture.stream();fixture.append();");
    await sleep(.2);
    results.reading = await js("return {topDelta:b.scrollTop-readTop,anchorDelta:anchor.getBoundingClientRect().top-anchorY};");
    await js("window.anchorY=anchor.getBoundingClientRect().top;fixture.expand();");
    await sleep(.2);
    results.expand = await js("return {anchorDelta:anchor.getBoundingClientRect().top-anchorY};");
    results.geometryReading = await geometry(selector, dockSel);
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
      assert.ok(geometryOk(results.geometryEnd) && geometryOk(results.geometryReading) && results.geometryEnd.bottom <= 1 && !results.geometryEnd.pill && results.geometryReading.pill, "feed and dock: " + text);
    }
    return results;
  }

  // A finished task: the feed ends in a tall result card, over the workers bar and the result composer. The reader reaches the
  // end by the real wheel, by the End key and by the Latest pill, then background reads arrive while every frame is recorded.
  const STATE = "const b=document.querySelector('.timeline'),d=document.querySelector('.task-panel .dock'),p=document.querySelector('.task-panel .new-pill');" +
    "return {top:Math.round(b.scrollTop*10)/10,height:b.scrollHeight,client:b.clientHeight,bottom:Math.round((b.scrollHeight-b.clientHeight-b.scrollTop)*10)/10,pad:getComputedStyle(b.firstElementChild).paddingBottom,dock:d?d.offsetHeight:null,gap:d?Math.round((d.getBoundingClientRect().top-b.getBoundingClientRect().bottom)*10)/10:null,pill:!!p&&p.getClientRects().length>0};";
  async function finishedTest(name) {
    await load(name);
    await wait("document.querySelector('.timeline .rcard, .timeline .ws-prev-result')&&document.querySelector('.ws-team')&&document.querySelector('.task-panel textarea')");
    await sleep(.6);
    assert.ok(have("xdotool"), "XTEST pointer driver unavailable");
    const [ox, oy] = await host.origin();
    const box = await js("const r=document.querySelector('.timeline').getBoundingClientRect();return [r.left,r.top,r.width,r.height];");
    const x = Math.trunc(ox + box[0] + box[2] / 2), y = Math.trunc(oy + box[1] + 160);
    xdotool("mousemove", "--sync", x, y);
    await sleep(.3);
    const state = () => js(STATE);
    // Every frame for about five seconds while background reads arrive (equal facts, changed facts, a tail read with no new rows,
    // a worker line, and in the live variant a streamed reply) and the relative times tick on.
    const observe = async () => {
      // A frame callback reads the page before that frame's layout: content a read grew between frames is measured before the
      // follow controller's ResizeObserver, which runs after layout and before paint, has pinned the end. So each frame is
      // sampled again from a ResizeObserver made after the controller's (observers are told in the order they were made), and
      // the frame's last sample is the state that was painted. The pre-layout reads are kept as a diagnostic.
      await js("window.track=[];window.early=[];window.tracking=true;let frame=0;const t0=performance.now();const s=(" + JSON.stringify(STATE) + ");const take=()=>Object.assign(new Function(s)(),{ms:Math.round(performance.now()-t0),frame});" +
        "const f=()=>{if(!tracking)return;frame++;const x=take();track.push(x);early.push(x);requestAnimationFrame(f);};requestAnimationFrame(f);" +
        "window.settle?.disconnect();window.settle=new ResizeObserver(()=>{if(!tracking)return;const x=Object.assign(take(),{settled:true});if(track.at(-1)?.frame===frame)track[track.length-1]=x;else track.push(x);});settle.observe(document.querySelector('.timeline').firstElementChild);");
      for (const step of ["same", "facts", "tail", "team", ...(name === "finished-live" ? ["stream"] : []), "same", "facts"]) { await js("fixture." + step + "();"); await sleep(.5); }
      await sleep(1.5);
      await js("tracking=false;");
      const track = await js("settle.disconnect();return track;"), early = await js("return early;");
      const bad = track.filter(s => s.bottom > 1 || s.pill || Math.abs(s.gap) > 1);
      return {frames: track.length, maxBottom: Math.max(...track.map(s => s.bottom)), maxGap: Math.max(...track.map(s => Math.abs(s.gap))), pillFrames: track.filter(s => s.pill).length, settledFrames: track.filter(s => s.settled).length,
        preLayout: {maxBottom: Math.max(...early.map(s => s.bottom)), frames: early.filter(s => s.bottom > 1).map(s => s.frame), pill: early.filter(s => s.pill).length}, heights: [...new Set(track.map(s => s.height))], pads: [...new Set(track.map(s => s.pad))], docks: [...new Set(track.map(s => s.dock))], firstBad: bad.slice(0, 4), last: track.at(-1)};
    };
    const out = results[name] = {initial: await state(), geometryEnd: await geometry(".timeline", ".task-panel .dock")};
    // (i) the wheel: up, then down past the end.
    xdotool("click", "--repeat", "8", "--delay", "40", "4");
    await sleep(.6);
    out.wheelUp = await state();
    xdotool("click", "--repeat", "60", "--delay", "25", "5");
    await sleep(.8);
    out.wheelReached = await state();
    out.wheel = await observe();
    // (ii) the keyboard: up by wheel, then End. A press on the feed gives focus to the page's main region, whose End WebKitGTK
    // does not apply to the feed (recorded, not judged); with focus in the feed itself, End scrolls it.
    xdotool("click", "--repeat", "8", "--delay", "40", "4");
    await sleep(.6);
    xdotool("click", "1");
    await sleep(.2);
    const pressed = await state();
    xdotool("key", "End");
    await sleep(.6);
    out.clickEnd = {focused: await js("const a=document.activeElement;return a.tagName+'.'+a.className;"), moved: (await state()).top - pressed.top};
    await js("const b=document.querySelector('.timeline');b.tabIndex=-1;b.focus({preventScroll:true});");
    xdotool("key", "End");
    await sleep(.8);
    out.keyReached = await state();
    await js("const b=document.querySelector('.timeline');b.blur();b.removeAttribute('tabindex');");
    out.key = await observe();
    // (iii) the Latest pill, clicked through X.
    xdotool("click", "--repeat", "8", "--delay", "40", "4");
    await sleep(.6);
    out.pillUp = await state();
    const pill = await js("const r=document.querySelector('.task-panel .new-pill')?.getBoundingClientRect();return r?[r.left+r.width/2,r.top+r.height/2]:null;");
    if (label !== "before") assert.ok(pill, "No Latest pill after scrolling up: " + JSON.stringify(out.pillUp));
    if (pill) {
      xdotool("mousemove", "--sync", Math.trunc(ox + pill[0]), Math.trunc(oy + pill[1]));
      xdotool("click", "1");
    }
    xdotool("mousemove", "--sync", x, y);
    await sleep(.8);
    out.pillReached = await state();
    out.pill = await observe();
    // (iv) a reader who scrolled up stays where they are while the same reads arrive.
    xdotool("click", "--repeat", "8", "--delay", "40", "4");
    await sleep(.6);
    const before = await state();
    await observe();
    const after = await state();
    out.reading = {before, after, topDelta: after.top - before.top};
    out.geometryReading = await geometry(".timeline", ".task-panel .dock");
    // (v) scrolled up, the tall card runs on behind the dock: below the dock's top edge, beside, between and under the workers
    // bar and the composer and at the composer's corner, the dock's own surface is what is there, never a feed entry.
    out.covered = await js("const d=document.querySelector('.task-panel .dock'),c=d.querySelector('.composer'),t=d.querySelector('.ws-team'),f=document.querySelector('.timeline'),col=f.firstElementChild.getBoundingClientRect(),D=d.getBoundingClientRect(),C=c.getBoundingClientRect(),T=t?t.getBoundingClientRect():null,mx=(C.left+C.right)/2,my=(C.top+C.bottom)/2;" +
      "const pts=[[C.left-6,my],[C.right+6,my],[mx,C.bottom+4],[C.left+2,C.top+2],[C.right-2,C.bottom-2],[col.left+30,D.top+3],[col.right-30,D.bottom-3],...(T?[[T.left-6,(T.top+T.bottom)/2],[T.right+6,(T.top+T.bottom)/2],[mx,(T.bottom+C.top)/2]]:[])].filter(([x,y])=>x>col.left&&x<col.right&&y>D.top&&y<D.bottom);" +
      "return {points:pts.length,feed:pts.filter(([x,y])=>{const e=document.elementFromPoint(x,y);return !e||f.contains(e);}).map(p=>p.map(Math.round)),under:!!document.elementFromPoint(mx,D.top+2)&&f.contains(document.elementFromPoint(mx,D.top-8))};");
    await capture(name + "-end-1440");
    if (label !== "before") {
      const text = JSON.stringify(out);
      assert.ok(out.wheelUp.pill && out.wheelUp.bottom > 40, text);
      for (const k of ["wheel", "key", "pill"]) assert.ok(out[k].maxBottom <= 1 && out[k].maxGap <= 1 && out[k].pillFrames === 0, k + ": " + text);
      assert.ok(geometryOk(out.geometryEnd) && geometryOk(out.geometryReading) && out.geometryReading.pill, "feed and dock: " + text);
      assert.ok(Math.abs(out.reading.topDelta) <= 1 && after.pill, text);
      assert.ok(out.covered.points >= 5 && out.covered.feed.length === 0, "feed shows through the dock: " + JSON.stringify(out.covered));
    }
    return out;
  }

  // The other docks: a finished worker's page (no composer, only the Latest band), the planning conversation, the task
  // Conversation and a working worker, at the smallest window with the sidebar at its widest, in both themes.
  async function dockTest(view, feed, dock, width, height, theme) {
    const tag = view + "-" + width + "-" + theme;
    await load(view, width, height, 240, "&theme=" + theme + (width < 1440 ? "&sidebar=360" : ""));
    await wait("document.querySelector(" + JSON.stringify(feed) + ")&&document.querySelector(" + JSON.stringify(dock) + ")&&document.querySelector(" + JSON.stringify(feed) + ").scrollHeight>document.querySelector(" + JSON.stringify(feed) + ").clientHeight+200");
    await sleep(.6);
    const out = {theme: await js("return document.documentElement.dataset.theme;"), end: await geometry(feed, dock)};
    await capture(tag + "-end");
    const [ox, oy] = await host.origin();
    const at = await js("const r=document.querySelector(" + JSON.stringify(feed) + ").getBoundingClientRect();return [Math.round(r.left+r.width/2),Math.round(r.top+Math.min(120,r.height/2))];");
    xdotool("mousemove", "--sync", ox + at[0], oy + at[1]);
    xdotool("click", "--repeat", "6", "--delay", "40", "4");
    await sleep(.6);
    out.reading = await geometry(feed, dock);
    await capture(tag + "-reading");
    if (label !== "before") {
      const text = tag + ": " + JSON.stringify(out);
      assert.equal(out.theme, theme, text);
      assert.ok(geometryOk(out.end) && out.end.bottom <= 1 && !out.end.pill, text);
      assert.ok(geometryOk(out.reading) && out.reading.bottom > 40 && out.reading.pill && out.reading.band, text);
    }
    return out;
  }
  const DOCKS = [["finished-worker", ".stream", ".dock-w"], ["sprint", ".sp-conv .timeline", ".sp-conv .dock"], ["finished", ".timeline", ".task-panel .dock"], ["worker", ".stream", ".dock-w"]];
  const dockViews = async () => {
    const out = {};
    for (const [view, feed, dock] of DOCKS) for (const [width, height] of [[1440, 1000], [960, 540]]) for (const theme of ["light", "dark"])
      out[view + "-" + width + "-" + theme] = await dockTest(view, feed, dock, width, height, theme);
    return out;
  };

  const wrong = c => c.includes("Planning fixture") || c.includes("Archived fixture");
  const results = {engine: "WebKitGTK", version: host.version, label};
  let exit = 0;
  if (process.argv[4] === "finished") {
    try {
      await finishedTest("finished");
      await finishedTest("finished-live");
      results.docks = await dockViews();
    } catch (error) { exit = 1; results.error = error.name + ": " + error.message; }
    writeJson(join(evidence, "metrics.json"), results, 2);
    console.log(JSON.stringify(results));
    return exit;
  }
  try {
    // `feeds` runs the scrolling scenarios alone, without the startup scenarios and the workspace service they read.
    if (process.argv[4] !== "feeds") {
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
    }
    results.worker = await streamTest("worker");
    results.conversation = await streamTest("conversation");
    await finishedTest("finished");
    await finishedTest("finished-live");
    results.docks = await dockViews();
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
      // Every one of the six calls is work the reader may want to see (timeline-visibility.ts hides only bookkeeping), Read
      // skill and Submit result included, and each reads by its person-facing name, never the engine's raw tool name.
      assert.ok(rows.length === 6 && !rows.some(r => r.includes("mcp__")) && rows.some(r => r.includes("Read skill")) && rows.some(r => r.includes("Submit result")), JSON.stringify(rows));
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
