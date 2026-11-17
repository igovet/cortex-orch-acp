// Opt-in static-bundle render of the feed screens in WebKitGTK; fake data, private virtual screen. Run through feed-visual.mjs.
import {Host, startXvfb, engineEnv, main, sleep, join, writeJson} from "./rendered/webkit.mjs";
import assert from "node:assert/strict";

const base = process.argv[2], label = process.argv[3], evidence = "/evidence";

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1440, height: 1000, env: engineEnv(display)});
  const js = code => host.js(code), wait = code => host.wait(code, 100);
  const load = async (name, width = 1440, height = 1000, theme = "dark") => {
    await host.resize(width, height);
    await host.load(base + "?view=" + name);
    await wait("window.fixture && document.querySelector('.titlebar')");
    await js("document.documentElement.dataset.theme=" + JSON.stringify(theme));
    await sleep(.4);
  };
  const capture = async name => { await sleep(.4); const size = await host.size(); await host.capture(join(evidence, label + "-" + name + ".png")); return size; };
  // Every visible scrollbar sits clear of the outer 5px the window keeps for edge resizing.
  const scan = () => js(`const W=innerWidth,H=innerHeight;const out=[];
      for(const el of document.querySelectorAll('*')){const s=getComputedStyle(el);if(el.offsetWidth===0)continue;
        const r=el.getBoundingClientRect();
        if(/auto|scroll/.test(s.overflowY)&&el.scrollHeight>el.clientHeight+1){if(r.right>W-5) out.push({c:el.className,axis:'y',gap:W-r.right});}
        if(/auto|scroll/.test(s.overflowX)&&el.scrollWidth>el.clientWidth+1){if(r.bottom>H-5) out.push({c:el.className,axis:'x',gap:H-r.bottom});}}
      return out`);
  const results = {};
  try {
    await load("worker");
    await wait("document.querySelector('.ws-worker-result')");
    results.order = await js("const k=[...document.querySelector('.stream-in').children];return k[k.length-1].className");
    assert.ok(results.order.includes("ws-worker-result"));
    results.agent_labels = await js(`return [...document.querySelectorAll(".wcard .l3")].filter(e=>/opus|luna/.test(e.textContent)).map(e=>({text:e.textContent,visible:e.getBoundingClientRect().height>0&&getComputedStyle(e).display!=="none"}))`);
    assert.ok(results.agent_labels.length === 2 && results.agent_labels.every(e => e.visible), JSON.stringify(results.agent_labels));
    results.worker_size = await capture("worker-collapsed-1440");
    await js("document.querySelector('.stream button.fold').click()");
    await sleep(.2);
    await js("document.querySelector('.step-row').scrollIntoView()");
    await capture("worker-open-1440");
    results.steps = await js("return [...document.querySelectorAll('.step-row')].slice(0,3).map(r=>r.textContent)");
    results.scan_worker = await scan();
    await js("document.querySelectorAll('.stream .step-row')[2].click()");
    await sleep(.2);
    await capture("worker-shell-open-1440");
    await load("worker", 1440, 1000, "light");
    await capture("worker-light-1440");
    await load("working");
    await capture("worker-running-1440");
    await load("coordinator");
    await wait("document.querySelector('.livepane .ev-note')");
    results.coordinator = await js("return {text:document.querySelector('.livepane').textContent.includes('Planning the page'),nothing:document.querySelector('.livepane').textContent.includes('Nothing to report'),model:document.querySelector('.d-grid').textContent}");
    await capture("coordinator-1440");
    results.scan_coordinator = await scan();
    await load("conversation");
    await js("document.querySelectorAll('.timeline button.fold')[0].click();document.querySelectorAll('.timeline button.fold')[1].click()");
    await sleep(.2);
    await capture("conversation-1440");
    results.scan_conversation = await scan();
    await load("review");
    await wait("document.querySelector('.rv-bar')");
    const openOutput = "document.querySelectorAll('details').forEach(e=>{if(e.querySelector('summary')?.textContent.includes('Command and output'))e.open=true})";
    await js(openOutput);
    await capture("review-failed");
    results.review_failed = await js(`return document.querySelector('.rv-bar').textContent.includes('1 passed, 1 failed') && !!document.querySelector('[aria-label=\\"Rerun Focused tests\\"]')`);
    assert.ok(results.review_failed);
    await js(`document.querySelector('[aria-label=\\"Rerun Focused tests\\"]').click()`);
    await wait("document.body.textContent.includes('Fresh rerun: all assertions passed')");
    await js(openOutput);
    results.manual_accept = await js("const b=[...document.querySelectorAll('button')].find(e=>e.textContent==='Accept v1');return !!b && !b.disabled && b.getAttribute('aria-disabled')!=='true' && document.body.textContent.includes('Check manually')");
    assert.ok(results.manual_accept);
    await js("const box=document.querySelector('.evid-b');box.scrollTop=box.scrollHeight");
    await capture("review-fresh-output");
    await js("document.querySelector('.evid-b').scrollTop=0");
    await capture("review-rerun-passed");
    await js("[...document.querySelectorAll('button')].find(e=>e.textContent==='Accept v1').click()");
    await wait("document.querySelector('.ready') && [...document.querySelectorAll('.ready button')].some(e=>e.getClientRects().length && e.textContent==='Commit to main…')");
    results.check_output_widths = await js("return [...document.querySelectorAll('.ready details.nv > summary')].map(e=>e.getBoundingClientRect().width)");
    assert.ok(results.check_output_widths.length === 2 && Math.min(...results.check_output_widths) > 200, JSON.stringify(results.check_output_widths));
    await capture("same-branch-commit");
    results.same_branch = await js("return ![...document.querySelectorAll('button')].some(b=>b.textContent==='Merge…')");
    assert.ok(results.same_branch);
    await load("settings");
    results.scan_settings = await scan();
    await load("board");
    results.scan_board = await scan();
    for (const [width, height] of [[900, 1000], [390, 844]]) {
      await load("worker", width, height);
      await capture("worker-" + width);
    }
    results.titles = await js("return document.querySelectorAll('[title]').length");
    assert.equal(results.titles, 0);
    writeJson(join(evidence, label + "-metrics.json"), results, 2);
    console.log(JSON.stringify(results));
    for (const name of ["scan_worker", "scan_coordinator", "scan_conversation", "scan_settings", "scan_board"])
      assert.deepEqual(results[name], [], name + ": " + JSON.stringify(results[name]));
    return 0;
  } catch (error) {
    console.error(error.name + ": " + error.message);
    return 1;
  }
});
