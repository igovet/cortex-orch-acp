// Opt-in render of the tab strip in WebKitGTK on a private virtual screen: real layout, scroll and hit testing, fake data.
import {Host, startXvfb, engineEnv, main, sleep, join} from "./rendered/webkit.mjs";
import assert from "node:assert/strict";

const base = process.argv[2], evidence = "/evidence";
const TIMELINE = ".ws-pane:not([data-parked]) .timeline";

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1440, height: 1000, env: engineEnv(display)});
  const js = code => host.js(code), wait = code => host.wait(code, 100);
  const capture = async name => { await sleep(.4); const size = await host.size(); await host.capture(join(evidence, name + ".png")); return size; };
  const facts = {};
  let exit = 0;
  try {
    await host.load(base + "?view=conversation");
    await wait("window.fixture && document.querySelector('.ws-tabstrip')");
    await js("document.documentElement.dataset.theme='dark'");
    await wait(`document.querySelector('${TIMELINE}')`);
    await sleep(.4);
    await js(`const t=document.querySelector('${TIMELINE}');const f=t.querySelector('button.fold');if(f)f.click();t.scrollTop=90`);
    await sleep(.3);
    facts.before = await js(`const t=document.querySelector('${TIMELINE}');return {scroll:t.scrollTop,height:t.scrollHeight,open:t.querySelectorAll('button.fold[aria-expanded="true"]').length}`);
    await js("document.querySelector('.sidebar a.nav').click()");
    await wait("document.querySelector('.ws-pane:not([data-parked]) textarea')");
    await js("const a=document.querySelector('.ws-pane:not([data-parked]) textarea');a.focus();document.execCommand('insertText',false,'A draft I will come back to')");
    await js(`document.querySelector('button[aria-label="New tab"]').click()`);
    await wait("document.querySelectorAll('.ws-tab').length===2");
    await sleep(.2);
    await js(`document.querySelector('.sidebar a.nav[href*="inbox"]').click()`);
    await sleep(.3);
    await js("const n=document.querySelectorAll('.ws-tab-name')[0];n.click()");
    await sleep(.4);
    facts.strip = await js("return {titles:[...document.querySelectorAll('.ws-tab-name')].map(e=>e.textContent),selected:[...document.querySelectorAll('.ws-tab-name')].map(e=>e.getAttribute('aria-selected')),rect:(()=>{const r=document.querySelector('.ws-tabstrip').getBoundingClientRect();return [r.left,r.top,r.width,r.height]})()}");
    facts.size_new_task = await capture("strip-new-task-1440");
    facts.draft = await js("return {value:document.querySelector('.ws-pane:not([data-parked]) textarea')?.value}");
    assert.equal(facts.draft.value, "A draft I will come back to", JSON.stringify(facts));
    // The first tab, on the conversation, was moved to the new-task route in place by the sidebar: open a task in tab 2 to prove the scroll.
    await js("document.querySelectorAll('.ws-tab-name')[1].click()");
    await sleep(.2);
    await js("const a=document.querySelector('.sidebar a.task-row');if(a)a.click()");
    await wait(`document.querySelector('${TIMELINE}')`);
    await sleep(.4);
    await host.resize(1200, 460); // Short enough that the conversation scrolls.
    await sleep(.4);
    await js(`const t=document.querySelector('${TIMELINE}');const f=t.querySelector('button.fold');if(f&&f.getAttribute('aria-expanded')!=='true')f.click();t.scrollTop=90`);
    await sleep(.3);
    facts.task_before = await js(`const t=document.querySelector('${TIMELINE}');return {scroll:t.scrollTop,open:t.querySelectorAll('button.fold[aria-expanded="true"]').length}`);
    facts.size_task = await capture("strip-task-1440");
    await js("document.querySelectorAll('.ws-tab-name')[0].click()");
    await sleep(.4);
    facts.parked = await js("const p=document.querySelector('.ws-pane[data-parked]');return {visibility:getComputedStyle(p).visibility,width:p.offsetWidth>0}");
    await js("document.querySelectorAll('.ws-tab-name')[1].click()");
    await sleep(.4);
    facts.task_after = await js(`const t=document.querySelector('${TIMELINE}');return {scroll:t.scrollTop,open:t.querySelectorAll('button.fold[aria-expanded="true"]').length}`);
    assert.equal(facts.task_after.open, facts.task_before.open, JSON.stringify(facts));
    assert.ok(facts.task_before.scroll > 0 && Math.abs(facts.task_after.scroll - facts.task_before.scroll) < 2, JSON.stringify(facts));
  } catch (error) {
    facts.failure = `${error.name}(${JSON.stringify(error.message)})`;
    exit = 1;
    try { await capture("failure"); } catch { /* the page may be gone */ }
  }
  console.log(JSON.stringify(facts, null, 1));
  return exit;
});
