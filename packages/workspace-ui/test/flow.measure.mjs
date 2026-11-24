#!/usr/bin/env node
// Text-only check of the task and board flows in real WebKitGTK. jsdom has no layout, cascade or animation, so Vitest cannot answer this.
//
// It builds the demo workspace, opens it in a real WebKit view on a private X display inside the rendered-check image, drives it
// with real mouse events from xdotool, and reads rectangles, computed styles and running animations. No screenshot is taken and
// no image is read. Exit code 1 means something blinked, moved, wrapped or vanished.
//
//     node packages/workspace-ui/test/flow.measure.mjs
import {spawnSync} from "node:child_process";
import {fileURLToPath} from "node:url";
import {mkdirSync} from "node:fs";
import {dirname, join, resolve} from "node:path";

const IMAGE = process.env.CW_RENDERED_IMAGE ?? "cortex-workspace-rendered:ubuntu26.04";
const here = dirname(fileURLToPath(import.meta.url));

if (!process.argv.includes("--inside")) {
  // The host half: build the prototype and run this same file inside the rendered image without a network.
  const root = resolve(here, "../../..");
  const site = join(root, ".workspace-build/flow-measure/site");
  mkdirSync(dirname(site), {recursive: true});
  const built = spawnSync("npx", ["vite", "build", "--logLevel", "error", "--emptyOutDir", "--outDir", site], {cwd: join(root, "prototypes/cortex-workspace/desktop"), stdio: "inherit"});
  if (built.status !== 0) process.exit(built.status ?? 1);
  const run = spawnSync("docker", ["run", "--rm", "--network", "none", "-e", "HOME=/tmp", "-e", "XDG_RUNTIME_DIR=/tmp", "-v", `${site}:/site:ro`, "-v", `${here}:/driver:ro`, IMAGE, "node", "/driver/flow.measure.mjs", "--inside"], {stdio: "inherit", timeout: 300000});
  process.exit(run.status ?? 1);
}

const {Host, startXvfb, engineEnv, main, sleep, xdotool, startOpenbox, serveSite} = await import("./rendered/webkit.mjs");
const WINDOW = [1880, 1100];
const PARK = [1915, 1195]; // Outside the window, so nothing is hovered.

await main(async () => {
  const display = await startXvfb();
  startOpenbox();
  await sleep(.5);
  const origin = await serveSite("/site");
  const host = await Host.start({width: WINDOW[0], height: WINDOW[1], decorated: false, env: engineEnv(display), timeout: 15});
  await host.move(0, 0);
  const failures = [], passed = [], facts = {};
  // An error thrown in the page comes back as {error}, so a check reports it instead of stopping the run.
  const js = async body => { try { return await host.js(body); } catch (error) { if (error.timeout) throw Error("TimeoutError: page did not answer"); return {error: error.message}; } };
  const call = async (f, ...args) => js(`return (${f.toString()})(${args.map(a => JSON.stringify(a ?? null)).join(",")});`);
  const x = async (...args) => { xdotool(...args); await sleep(.05); };

  async function load(path) {
    await host.load(origin + path);
    let ready = false;
    for (let i = 0; i < 100 && !ready; i++) {
      await sleep(.1);
      ready = await js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') && document.querySelectorAll('.nav').length > 0") === true;
    }
    if (!ready) throw Error("TimeoutError: no workspace at " + path);
    await sleep(.4);
    await js("document.querySelector('.ws-demo-banner')?.remove(); window.__box = el => { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(n => Math.round(n * 100) / 100); }; return 1");
  }
  const check = (name, ok, details = {}) => { if (ok) passed.push(name); else failures.push({check: name, ...details}); };
  const centre = selector => call(selector => { const el = document.querySelector(selector); if (!el) return null; el.scrollIntoView({block: "nearest"}); const r = el.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)]; }, selector);
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

  async function board(pj) {
    await load(`${pj}/board`);
    const seen = await call(() => {
      const groups = [...document.querySelectorAll(".col-grp")].map(g => g.textContent), named = [...document.querySelectorAll(".card .ws-card-age .trunc")].map(s => s.textContent);
      const tint = c => getComputedStyle(c).backgroundColor, need = document.querySelector(".card.need-you"), plain = document.querySelector(".card:not(.need-you):not(.bad)");
      return {groups, named: named.length, need: need && tint(need), plain: plain && tint(plain), review: !![...document.querySelectorAll('.col[data-col="review"] .card')].length, reviewMarked: [...document.querySelectorAll('.col[data-col="review"] .card')].every(c => c.classList.contains("need-you")), stamps: /Confirmed|\bold\b/.test(document.querySelector(".main").textContent)};
    });
    check("all work names its sprints", seen.groups.length > 0 && seen.named > 0, {seen});
    check("a task waiting for the owner reads differently", seen.need !== null && seen.need !== seen.plain, {seen});
    check("every Review card is marked", !seen.review || seen.reviewMarked, {seen});
    check("no stamp on a live board", !seen.stamps, {seen});
    facts.sprint_heads = seen.groups;
  }

  async function move(pj) {
    await load(`${pj}/board`);
    await call(() => {
      window.__flight = false;
      const watch = new MutationObserver(records => {
        if (records.some(r => r.attributeName === "data-flying" ||
          [...r.addedNodes].some(n => n instanceof Element && n.matches(".ws-fly")))) window.__flight = true;
      });
      watch.observe(document.body, {subtree: true, childList: true, attributes: true, attributeFilter: ["data-flying"]});
      return 1;
    });
    const at = await call(() => { const b = [...document.querySelectorAll(".card-a button")].find(b => b.textContent === "Start"); if (!b) return null; const card = b.closest(".card"); window.__id = card.dataset.task; window.__from = card.closest(".col").dataset.col; window.__start = __box(card); const r = b.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)]; });
    if (at === null) { (facts.not_on_screen ??= []).push("a startable card"); return; }
    await x("mousemove", ...at);
    await x("click", 1);
    await sleep(.7);
    await x("mousemove", ...PARK);
    const after = await call(() => { const card = document.querySelector(`.card[data-task="${__id}"]`); return {flight: __flight, from: __from, to: card?.closest(".col")?.dataset.col, left: document.querySelectorAll(".ws-fly").length, hidden: card && getComputedStyle(card).visibility, flying: document.querySelector(".board-wrap").hasAttribute("data-flying")}; });
    check("the card changes columns", !!after.to && after.from !== after.to, {after});
    check("the destination is visible without a card flight", !after.flight && after.left === 0 && after.hidden === "visible" && !after.flying, {after});
    facts.move = after;
  }

  async function plan(sprint) {
    await load(sprint);
    const rows = await call(() => { const rows = [...document.querySelectorAll(".pt-row:not(.th)")]; return rows.length ? {n: rows.length, tall: Math.max(...rows.map(r => r.getBoundingClientRect().height)), more: document.querySelectorAll(".ws-more-l").length, text: rows[0].textContent.length} : null; });
    if (rows === null) { (facts.not_on_screen ??= []).push("a plan"); return; }
    check("a plan row is one line, however long its task", rows.tall <= 40, {rows});
    check("every row offers Read more", rows.more === rows.n, {rows});
    await x("mousemove", ...await centre(".pt-row:not(.th)"));
    await call(() => { window.__rows = [...document.querySelectorAll(".pt-row:not(.th)")]; window.__changes = 0; window.__watch = new MutationObserver(l => { __changes += l.filter(m => m.type === "childList").length; }); __watch.observe(document.querySelector(".sp-plan"), {subtree: true, childList: true}); return 1; });
    await sleep(4);
    const rest = await call(() => { const now = [...document.querySelectorAll(".pt-row:not(.th)")]; return {changes: __changes, same: now.length === __rows.length && now.every((r, i) => r === __rows[i]), hover: now.some(r => r.matches(":hover"))}; });
    check("the plan does not redraw under a resting pointer", rest.changes === 0 && rest.same, {same: rest});
    check("the pointer really rests on it", rest.hover, {same: rest});
    const before = await call(() => ({conv: __box(document.querySelector(".sp-conv"))}));
    await x("mousemove", ...await centre(".ws-more-l"));
    await x("click", 1);
    await sleep(.3);
    const read = await call(() => { const p = document.querySelector(".ws-read"); if (!p) return null; const plan = document.querySelector(".sp-plan").getBoundingClientRect(), r = p.getBoundingClientRect(), b = document.querySelector(".ws-read-b"); return {right: r.left >= plan.right - 1, inside: r.right <= innerWidth, conv: __box(document.querySelector(".sp-conv")), title: document.querySelector(".ws-read-t").textContent.length > 0, scrolls: getComputedStyle(b).overflowY}; });
    check("Read more opens a right sidebar", read !== null && read.right && read.inside && read.title, {read});
    check("the conversation stays put", read !== null && same(read.conv, before.conv), {read, before});
    check("the full text scrolls on its own", read !== null && ["auto", "scroll"].includes(read.scrolls), {read});
    await x("key", "Escape");
    await sleep(.2);
    check("Escape closes it", await js("return !document.querySelector('.ws-read')"));
    await x("mousemove", ...PARK);
  }

  // The pieces no demo state reaches, drawn with the app's own stylesheet.
  async function css() {
    const form = await call(() => {
      const host = document.createElement("div"); host.style.width = "380px"; host.innerHTML = `<div class="dc"><div class="dc-panel" id="p1"><div class="dc-n">Question 1 of 2</div><div class="dc-q">A short one</div></div><div class="dc-panel" id="p2"><div class="dc-n">Question 2 of 2</div><div class="dc-q">${"A much longer question that wraps over several lines. ".repeat(4)}</div></div><div class="dc-send"><button class="btn btn-primary" id="go">Send answers</button></div></div>`; document.querySelector(".main").append(host);
      const p1 = host.querySelector("#p1").getBoundingClientRect(), p2 = host.querySelector("#p2").getBoundingClientRect(), go = host.querySelector("#go").getBoundingClientRect(), shown = getComputedStyle(host.querySelector("#p2")).visibility; host.remove(); return {stacked: p2.top >= p1.bottom, below: go.top >= p2.bottom, shown};
    });
    check("questions asked together stand one under another, every one on view", form.stacked && form.shown === "visible", {form});
    check("the one Send answers button follows the last question", form.below, {form});
    const qa = await call(() => {
      const host = document.createElement("div"); host.style.width = "380px"; host.innerHTML = `<div class="qa"><i class="i s14 i-question"></i><div class="qa-b"><div class="qa-q">Какой визуальный стиль ближе?</div><div class="qa-a">Тёплый и домашний: кофейные оттенки, уют, и ещё немного длинного текста, чтобы он занял две строки</div></div><span class="qa-t">23:22</span></div>`; document.querySelector(".main").append(host);
      const b = host.querySelector(".qa-b").getBoundingClientRect(), t = host.querySelector(".qa-t").getBoundingClientRect(), q = host.querySelector(".qa-q").getBoundingClientRect(), a = host.querySelector(".qa-a").getBoundingClientRect(); host.remove(); return {stacked: a.top >= q.bottom, beside: t.left >= b.right, width: b.width};
    });
    check("a settled question stacks its answer under it, the time beside both", qa.stacked && qa.beside && qa.width > 250, {qa});
    const busy = await call(() => { const b = [...document.querySelectorAll(".titlebar .btn-primary, .main .btn")].find(x => x.getClientRects().length); if (!b) return null; const before = __box(b); b.setAttribute("data-busy", ""); b.setAttribute("aria-busy", "true"); window.__b = b; window.__was = before; return {before, now: __box(b), label: b.textContent}; });
    if (busy === null) (facts.not_on_screen ??= []).push("a button");
    else {
      await sleep(.3);
      const late = await call(() => { const s = getComputedStyle(__b), a = getComputedStyle(__b, "::after"); const out = {box: __box(__b), color: s.color, after: a.position, opacity: a.opacity, width: a.width, spin: a.animationName}; __b.removeAttribute("data-busy"); __b.removeAttribute("aria-busy"); return out; });
      check("a loader never changes its button's box", same(busy.before, busy.now) && same(busy.now, late.box), {busy, late});
      check("a loader shows once it is worth showing", late.after === "absolute" && late.opacity === "1" && late.spin.includes("ws-busy-run"), {late});
      check("a busy button keeps its label readable", !["transparent", "rgba(0, 0, 0, 0)"].includes(late.color), {late});
      check("the label gives way to the loader", late.color === "rgba(0, 0, 0, 0)", {late});
    }
  }

  async function inbox(at) {
    await load(`${at}/inbox`);
    const rows = await call(() => { const rows = [...document.querySelectorAll(".need")]; return {n: rows.length, ages: rows.map(r => { const a = r.querySelector(".age"); return {text: a.textContent, tall: a.getBoundingClientRect().height, wide: a.scrollWidth <= a.clientWidth + 1}; }), row: rows.length ? Math.max(...rows.map(r => r.getBoundingClientRect().height)) : 0}; });
    check("an Inbox row holds one line", rows.n > 0 && rows.row <= 44 && rows.ages.every(a => a.tall <= 20 && a.wide && !a.text.includes("Confirmed")), {rows});
  }

  async function boxes() {
    const drawn = await call(() => {
      const host = document.createElement("div"); host.style.width = "300px"; host.innerHTML = `<input type="checkbox" class="chk" id="c1"><input type="checkbox" class="ws-ck" id="c2"><label class="ck"><input type="checkbox" id="c3"></label><div class="chk" id="row"><i class="i s14 i-help"></i><span class="trunc">Visual review</span><span class="src">Unverified</span></div>`; document.querySelector(".main").append(host);
      const size = id => { const r = host.querySelector("#" + id).getBoundingClientRect(); return [r.width, r.height]; }, row = host.querySelector("#row"), s = getComputedStyle(row), out = {boxes: ["c1", "c2", "c3"].map(size), row: size("row"), border: s.borderTopWidth, name: host.querySelector(".trunc").getBoundingClientRect().width}; host.remove(); return out;
    });
    check("a checkbox is square", drawn.boxes.every(([w, h]) => w === h), {drawn});
    check("a check row is a row with its name showing", drawn.row[0] === 300 && drawn.border === "0px" && drawn.name > 60, {drawn});
  }

  let code = 1;
  try {
    await sleep(1);
    await load("/");
    const machine = await js("return location.pathname.split('/')[2]");
    const project = await js("return document.querySelector('.proj').getAttribute('href').split('/')[4]");
    const at = `/m/${machine}`, pj = `/m/${machine}/p/${project}`;
    for (const theme of ["dark", "light"]) {
      await load("/");
      await js(`localStorage.setItem('cw.theme', ${JSON.stringify(theme)}); return 1`);
      await board(pj);
      await load(at);
      const sprint = await js("const a = [...document.querySelectorAll('.task-row')].find(a => /\\/s\\//.test(a.getAttribute('href') ?? '')); return a ? a.getAttribute('href').replace(/\\/(board|list)$/, '/planning') : null");
      if (sprint) await plan(sprint); else (facts.not_on_screen ??= []).push("a sprint");
      await load(pj);
      await css();
      await inbox(at);
      await boxes();
    }
    await move(pj);
    code = failures.length ? 1 : 0;
  } catch (error) { // A broken run is a failed run, never a pass.
    failures.push({check: "harness", error: `${error.name}(${JSON.stringify(error.message)})`});
  }
  console.log(JSON.stringify({engine: `WebKitGTK ${host.version.join(".")}`, passed: passed.length, failed: failures.length, facts, failures}, null, 1));
  return code;
});
