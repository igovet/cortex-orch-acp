#!/usr/bin/env node
// Text-only sweep of the whole app's interface rules in real WebKitGTK. jsdom has no layout or cascade, so Vitest cannot answer this.
//
// It builds the demo workspace, opens every screen in a real WebKit view on a private X display inside the rendered-check image,
// and reads text, rectangles and computed styles. Both themes, two window sizes, every popover, and the dialogs a screen opens with
// a "…" button. No screenshot is taken and no image is read. Exit code 1 means a rule was broken somewhere.
//
//     node packages/workspace-ui/test/sweep.measure.mjs
//     SWEEP_ONLY=settings node packages/workspace-ui/test/sweep.measure.mjs        (SWEEP_QUICK=1: dark theme, one size)
import {spawnSync} from "node:child_process";
import {fileURLToPath} from "node:url";
import {mkdirSync, readFileSync} from "node:fs";
import {dirname, join, resolve} from "node:path";

const IMAGE = process.env.CW_RENDERED_IMAGE ?? "cortex-workspace-rendered:ubuntu26.04";
const here = dirname(fileURLToPath(import.meta.url));

if (!process.argv.includes("--inside")) {
  const root = resolve(here, "../../..");
  const site = join(root, ".workspace-build/sweep-measure/site");
  mkdirSync(dirname(site), {recursive: true});
  const built = spawnSync("npx", ["vite", "build", "--logLevel", "error", "--emptyOutDir", "--outDir", site], {cwd: join(root, "prototypes/cortex-workspace/desktop"), stdio: "inherit"});
  if (built.status !== 0) process.exit(built.status ?? 1);
  const run = spawnSync("docker", ["run", "--rm", "--network", "none", "-e", "HOME=/tmp", "-e", "XDG_RUNTIME_DIR=/tmp", "-e", `SWEEP_ONLY=${process.env.SWEEP_ONLY ?? ""}`, "-e", `SWEEP_QUICK=${process.env.SWEEP_QUICK ?? ""}`, "-v", `${site}:/site:ro`, "-v", `${here}:/driver:ro`, IMAGE, "node", "/driver/sweep.measure.mjs", "--inside"], {stdio: "inherit", timeout: 1700000});
  process.exit(run.status ?? 1);
}

const {Host, startXvfb, engineEnv, main, sleep, xdotool, startOpenbox, serveSite, probe} = await import("./rendered/webkit.mjs");
const WINDOW = [1880, 1100];
const SMALL = [1280, 800];
// Every rule is one question put to the rendered page. A hit is a broken rule, never a warning.
const AUDIT = probe("sweep.measure.AUDIT.js");

await main(async () => {
  const display = await startXvfb();
  startOpenbox();
  await sleep(.5);
  const origin = await serveSite("/site");
  const host = await Host.start({width: WINDOW[0], height: WINDOW[1], decorated: false, env: engineEnv(display), timeout: 20});
  await host.move(0, 0);
  const broken = {}, facts = {}, visited = [];
  const only = process.env.SWEEP_ONLY ?? "";
  const quick = !!process.env.SWEEP_QUICK;
  const js = async body => { try { return await host.js(body); } catch (error) { if (error.timeout) throw Error("TimeoutError: page did not answer"); return {error: error.message}; } };
  const x = async (...args) => { xdotool(...args); await sleep(.05); };

  async function load(path) {
    await host.load(origin + path);
    let ready = false;
    for (let i = 0; i < 100 && !ready; i++) {
      await sleep(.08);
      ready = await js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') && document.querySelectorAll('.nav, .snav').length > 0") === true;
    }
    if (!ready) throw Error("TimeoutError: no workspace at " + path);
    await sleep(.25);
    await js(AUDIT + "document.querySelector('.ws-demo-banner')?.remove(); return 1");
  }
  const size = async (width, height) => { await host.resize(width, height); await sleep(.4); };
  const note = (where, hits) => {
    for (const hit of Array.isArray(hits) ? hits : []) {
      const rule = broken[hit.rule] ??= {};
      const key = `${hit.at} ${hit.detail}`;
      rule[key] ??= where;
    }
  };
  const audit = async (where, root = "document.body") => note(where, await js(`return __audit(${root}, location.pathname)`));

  async function popovers(where) {
    const ids = await js("return [...document.querySelectorAll('[popover]')].map((p, i) => { p.dataset.sw = i; return i; })");
    for (const i of ids) {
      const hits = await js(`const p = document.querySelector('[data-sw="${i}"]'); if (!p) return []; try { p.showPopover(); } catch { return []; } const out = __audit(p, location.pathname); p.hidePopover(); return out;`);
      note(`${where} popover`, hits);
    }
  }

  async function dialogs(path) {
    const names = (await js("return [...document.querySelectorAll('.main button, .set-main button, .inspector button, .titlebar button')].filter(b => /…$/.test(b.textContent.trim()) && b.getAttribute('aria-disabled') !== 'true' && !b.disabled).map(b => b.textContent.trim()).slice(0, 6)")) || [];
    for (const name of new Set(names)) {
      await load(path);
      const opened = await js(`const b = [...document.querySelectorAll('.main button, .set-main button, .inspector button, .titlebar button')].find(b => b.textContent.trim() === ${JSON.stringify(name)}); if (!b) return false; b.click(); return true;`);
      if (!opened) continue;
      await sleep(.2);
      if (await js("return !!document.querySelector('dialog[open]')")) {
        await audit(`${path} dialog ${name}`, "document.querySelector('dialog[open]')");
        facts.dialogs = (facts.dialogs ?? 0) + 1;
      }
    }
  }

  async function screens() {
    await load("/");
    const machine = await js("return location.pathname.split('/')[2]");
    const project = await js("return document.querySelector('.proj').getAttribute('href').split('/')[4]");
    const at = `/m/${machine}`, pj = `/m/${machine}/p/${project}`;
    await load(`${pj}/board`);
    const tasks = (await js("const seen = new Map(); for (const c of document.querySelectorAll('.card[data-task]')) { const col = c.closest('.col')?.dataset.col ?? ''; if (!seen.has(col)) seen.set(col, c.dataset.task); } return [...seen.values()];")) || [];
    await load(at);
    const sprint = await js("const a = [...document.querySelectorAll('.task-row')].find(a => /\\/s\\//.test(a.getAttribute('href') ?? '')); return a ? a.getAttribute('href').replace(/\\/(board|list)$/, '') : null");
    const settings = `/settings/machine/${machine}`;
    const pages = [at, `${at}/inbox`, `${at}/archive`, `${at}/new`, `${at}/new?kind=sprint`];
    pages.push(pj, `${pj}/list`, `${pj}/code`, `${pj}/git`, `${pj}/worktrees`, `${pj}/knowledge`, `${pj}/settings`);
    if (sprint) pages.push(`${sprint}/planning`, `${sprint}/summary`, `${sprint}/board`, `${sprint}/list`);
    for (const task of tasks) { const base = `${at}/t/${task}`; pages.push(base, `${base}/workers`, `${base}/changes`, `${base}/code`); }
    pages.push(...["appearance", "voice", "updates", "shortcuts", "machines"].map(s => `/settings/app/${s}`));
    pages.push(...["overview", "orchestration", "connections", "worktrees", "storage", "diagnostics", "remote"].map(s => `${settings}/${s}`));
    pages.push(...["defaults", "models", "routing", "routing?tab=kinds", "routing?tab=quota", "routing?tab=rules", "effective", "policy", "tools", "profiles", "files"].map(p => `${settings}/orchestration/${p}`));
    return pages.filter(p => p.includes(only));
  }

  // Ctrl+C after a real selection, read back from the real clipboard.
  async function text_copies() {
    await load("/");
    const machine = await js("return location.pathname.split('/')[2]");
    const project = await js("return document.querySelector('.proj').getAttribute('href').split('/')[4]");
    await load(`/m/${machine}/p/${project}/board`);
    const task = await js("return document.querySelector('.card[data-task]')?.dataset.task ?? null");
    const spots = [[`/m/${machine}/t/${task ?? "None"}`, ".tl-in .md, .tl-in .ai, .tl-in p, .tl-in .brief, .tl-in div"], [`/settings/machine/${machine}/overview`, ".srow .d"], ["/settings/app/appearance", ".set-lead"]];
    for (const [path, selector] of spots) {
      await load(path);
      const at = await js(`const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find(e => !e.children.length && e.textContent.trim().length > 12 && e.getClientRects().length); if (!el) return null; el.scrollIntoView({ block: 'center' }); const range = document.createRange(); range.selectNodeContents(el); const r = range.getClientRects()[0]; return [Math.round(r.x + 6), Math.round(r.y + r.height / 2), Math.round(Math.min(r.right - 4, r.x + 200)), el.textContent.trim().slice(0, 40)];`);
      if (at === null) { (facts.no_copy_spot ??= []).push(path); continue; }
      await x("mousemove", at[0], at[1]);
      await x("mousedown", 1);
      await x("mousemove", at[2], at[1]);
      await x("mouseup", 1);
      const picked = await js("return getSelection().toString()");
      await x("key", "ctrl+c");
      await sleep(.3);
      const got = await host.clipboard();
      const ok = !!picked && got === picked;
      (facts.copied ??= []).push({path: path.split("/").at(-1), selected: (picked ?? "").length, same: ok});
      if (!ok) (broken["Ctrl+C does not copy"] ??= {})[`${path} selected ${JSON.stringify(picked)} clipboard ${JSON.stringify(got)}`] = path;
    }
  }

  // Whether a reserved scrollbar is really reserved, so a gap measured here holds with and without a thumb.
  async function scrollers() {
    await load("/");
    const machine = await js("return location.pathname.split('/')[2]");
    const project = await js("return document.querySelector('.proj').getAttribute('href').split('/')[4]");
    await load(`/m/${machine}/p/${project}/board`);
    facts.scrollers = await js("return [...document.querySelectorAll('.sb-scroll, .col-b')].slice(0, 3).map(e => { const s = getComputedStyle(e); return { el: e.className, gutter: e.offsetWidth - e.clientWidth, overflow: s.overflowY, sg: s.scrollbarGutter, pl: s.paddingLeft, pr: s.paddingRight }; })");
  }

  let code = 1;
  try {
    await sleep(1);
    await load("/");
    for (const theme of quick ? ["dark"] : ["dark", "light"]) {
      await load("/");
      await js(`localStorage.setItem('cw.theme', ${JSON.stringify(theme)}); return 1`);
      for (const path of await screens()) {
        await load(path);
        await audit(`${theme} ${path}`);
        await popovers(`${theme} ${path}`);
        visited.push(path);
        if (theme === "dark") await dialogs(path);
      }
    }
    if (!quick) {
      await size(...SMALL);
      await load("/");
      await js("localStorage.setItem('cw.theme', 'dark'); return 1");
      for (const path of await screens()) { await load(path); await audit(`small ${path}`); visited.push(path); }
      await size(...WINDOW);
    }
    await scrollers();
    await text_copies();
    code = Object.keys(broken).length ? 1 : 0;
  } catch (error) { // A broken run is a failed run, never a pass.
    (broken.harness ??= {})[`${error.name}(${JSON.stringify(error.message)})`] = "run";
  }
  const report = Object.fromEntries(Object.entries(broken).map(([rule, hits]) => [rule, {count: Object.keys(hits).length, first: Object.fromEntries(Object.entries(hits).slice(0, 40))}]));
  console.log(JSON.stringify({engine: `WebKitGTK ${host.version.join(".")}`, screens: visited.length, distinct: new Set(visited).size, broken_rules: Object.keys(broken).length, facts, broken: report}, null, 1));
  return code;
});
