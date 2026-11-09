#!/usr/bin/env node
/**
 * Text-only layout check for every composer bar, in a real browser. jsdom has no layout, so Vitest cannot answer this.
 * It starts its own Vite server and a headless Chrome, talks to Chrome over the DevTools protocol, and prints numbers.
 * No screenshot is ever taken. Exit code 1 means a control left its composer, overlapped another one, or was clipped.
 *
 *   node src/workspace/ui/composer-fit.measure.mjs            (from prototypes/cortex-workspace/desktop)
 *
 * CHROME picks the browser, default /usr/bin/google-chrome. FIT_PORT picks the Vite port, default 5197.
 * Chrome runs with --no-sandbox because it only ever loads this local server and the local mockup files.
 * Every bar is measured with the longest label of each selector and the widest figures, at every window width below,
 * with the inspector closed and open, with an empty and a typed draft, and at normal and doubled text sizes.
 * A window of 640x400 is what 1280x800 becomes at 200% zoom, and 960x540 is 1920x1080 at 200%.
 */
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const desktop = resolve(here, "../../..");
const mockups = resolve(desktop, "../mockups");
const port = Number(process.env.FIT_PORT ?? 5197);
const origin = `http://127.0.0.1:${port}`;
const WINDOWS = [[1920, 1080], [1600, 900], [1440, 900], [1366, 768], [1280, 800], [1100, 700], [960, 540], [800, 600], [720, 500], [640, 400], [560, 400], [480, 400]];
const sleep = ms => new Promise(r => setTimeout(r, ms));

/** Runs in the page. */
const PAGE = String.raw`(() => {
  const LONGEST = { "i-cpu": "Local model", "i-gauge": "Medium", "i-shield": "Development", "i-globe": "Russian" };
  const seen = el => el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
  window.__fit = {
    longest() {
      for (const sel of document.querySelectorAll(".c-bar .sel")) {
        const icon = [...sel.querySelectorAll("i")].map(i => [...i.classList].find(c => LONGEST[c])).find(Boolean);
        if (!icon) continue;
        const label = sel.querySelector(".sel-t") ?? [...sel.childNodes].find(n => n.nodeType === 3);
        label.textContent = LONGEST[icon];
      }
      for (const ctx of document.querySelectorAll(".c-bar .ctx")) {
        const text = ctx.querySelector(".ctx-t") ?? [...ctx.childNodes].find(n => n.nodeType === 3 && n.textContent.trim());
        if (text) text.textContent = ctx.querySelector(".ring") ? "100%" : "9999k";
      }
    },
    text(on) {
      const s = document.documentElement.style;
      for (const [k, v] of [["--t-cap", 12], ["--t-sec", 13], ["--t-ui", 14], ["--t-content", 15]]) { if (on) s.setProperty(k, v * 2 + "px"); else s.removeProperty(k); }
    },
    type(value) {
      for (const area of document.querySelectorAll(".composer textarea:not(:disabled)")) {
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set.call(area, value);
        area.dispatchEvent(new InputEvent("input", { bubbles: true }));
      }
    },
    pick(kind, label) {
      const trigger = document.querySelector('.c-bar [popovertarget^="m-' + kind + '-"]');
      trigger.click();
      const item = [...document.getElementById(trigger.getAttribute("popovertarget")).querySelectorAll(".mi")].find(m => m.textContent.startsWith(label));
      item.click();
    },
    menu() {
      const trigger = [...document.querySelectorAll(".c-bar [popovertarget]")].find(seen), pop = document.getElementById(trigger.getAttribute("popovertarget"));
      trigger.click();
      const a = trigger.getBoundingClientRect(), b = pop.getBoundingClientRect(), open = pop.matches(":popover-open");
      pop.hidePopover();
      return { open, inWindow: b.left >= 0 && b.top >= 0 && b.right <= innerWidth && b.bottom <= innerHeight, gap: Math.round(Math.min(Math.abs(a.top - b.bottom), Math.abs(b.top - a.bottom))) };
    },
    measure() {
      const bars = [];
      for (const bar of document.querySelectorAll(".c-bar")) {
        if (!seen(bar)) continue;
        const box = bar.closest(".composer").getBoundingClientRect(), bad = [];
        const kids = [...bar.querySelectorAll("button, .ctx, .cap")].filter(el => seen(el) && !el.closest("[popover]"));
        const name = el => (el.getAttribute("aria-label") ?? el.title ?? "") .slice(0, 28) || el.textContent.trim().slice(0, 28);
        const rects = kids.map(el => ({ el, r: el.getBoundingClientRect() }));
        for (const { el, r } of rects) {
          if (r.right > box.right + .5 || r.left < box.left - .5 || r.bottom > box.bottom + .5 || r.top < box.top - .5) bad.push("outside: " + name(el) + " by " + Math.round(Math.max(r.right - box.right, box.left - r.left, r.bottom - box.bottom)) + "px");
          const clip = el.closest(".c-route");
          if (clip) { const c = clip.getBoundingClientRect(); if (r.right > c.right + .5) bad.push("clipped: " + name(el)); }
        }
        for (let i = 0; i < rects.length; i++) for (let j = i + 1; j < rects.length; j++) {
          const a = rects[i].r, b = rects[j].r;
          if (a.left < b.right - .5 && b.left < a.right - .5 && a.top < b.bottom - .5 && b.top < a.bottom - .5) bad.push("overlap: " + name(rects[i].el) + " / " + name(rects[j].el));
        }
        if (box.right > innerWidth + .5) bad.push("composer past the window by " + Math.round(box.right - innerWidth) + "px");
        if (bar.scrollWidth > bar.clientWidth + 1) bad.push("bar scrolls by " + (bar.scrollWidth - bar.clientWidth) + "px");
        const sels = [...bar.querySelectorAll(".c-route .sel")].filter(seen);
        const state = sels.map(s => { const t = s.querySelector(".sel-t"); return !t || !seen(t) ? "icon" : t.scrollWidth > t.clientWidth ? "cut" : "full"; }).join(",");
        const labels = sels.map(s => (s.querySelector(".sel-t")?.textContent ?? "") + " " + Math.round(s.getBoundingClientRect().width) + "px").join(", ");
        const route = bar.querySelector(".c-route")?.getBoundingClientRect().width ?? 0;
        bars.push({ width: Math.round(bar.getBoundingClientRect().width), height: Math.round(bar.getBoundingClientRect().height), route: Math.round(route), labels, state, bad });
      }
      return bars;
    },
  };
})()`;

async function main() {
  const profile = mkdtempSync(join(tmpdir(), "composer-fit-"));
  const vite = spawn(join(desktop, "node_modules/.bin/vite"), ["--host", "127.0.0.1", "--port", String(port), "--strictPort"], { cwd: desktop, stdio: "ignore" });
  const chrome = spawn(process.env.CHROME ?? "/usr/bin/google-chrome", ["--headless=new", "--no-sandbox", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
  const stop = () => { chrome.kill("SIGKILL"); vite.kill("SIGKILL"); };
  try {
    const debug = await new Promise((ok, fail) => {
      let log = "";
      const timer = setTimeout(() => fail(new Error("Chrome did not open its debugging port")), 20000);
      chrome.stderr.on("data", d => { log += d; const m = /DevTools listening on ws:\/\/([^/]+)\//.exec(log); if (m) { clearTimeout(timer); ok(m[1]); } });
    });
    for (let i = 0; ; i++) {
      try { if ((await fetch(origin)).ok) break; } catch { /* still starting */ }
      if (i > 100) throw new Error("Vite did not start");
      await sleep(100);
    }
    const target = await (await fetch(`http://${debug}/json/new?about:blank`, { method: "PUT" })).json();
    const socket = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((ok, fail) => { socket.onopen = ok; socket.onerror = fail; });
    let next = 0;
    const waiting = new Map();
    socket.onmessage = e => { const m = JSON.parse(e.data); if (m.id && waiting.has(m.id)) { const w = waiting.get(m.id); waiting.delete(m.id); if (m.error) w.fail(new Error(m.error.message)); else w.ok(m.result); } };
    const cdp = (method, params = {}) => new Promise((ok, fail) => { waiting.set(++next, { ok, fail }); socket.send(JSON.stringify({ id: next, method, params })); });
    const run = async expression => {
      const { result, exceptionDetails } = await cdp("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
      return result.value;
    };
    const settle = () => run("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))");
    const open = async (url, ready = ".c-bar") => {
      await cdp("Page.navigate", { url });
      for (let i = 0; i < 100; i++) {
        await sleep(50);
        if (await run(`!!document.querySelector(${JSON.stringify(ready)})`).catch(() => false)) return true;
      }
      return false;
    };
    await cdp("Page.enable");
    await cdp("Runtime.enable");
    await cdp("Emulation.setDeviceMetricsOverride", { width: 1600, height: 900, deviceScaleFactor: 1, mobile: false });
    await open(`${origin}/`, "#root > *");
    const data = await run(`(async () => {
      const { WORKSPACE_CASES: C } = await import("/src/workspace/adapter/fixtures.ts");
      const { WORKSPACE_SESSION_WORKERS: W } = await import("/src/workspace/adapter/session-fixture.ts");
      const { loadFixtures } = await import("/src/fixtures.ts");
      const w = loadFixtures().workspace;
      return { C, W, draft: w.tasks.find(t => t.status === "draft")?.id ?? null, sprint: w.sprints.find(s => s.title === "Audit log export")?.id ?? null };
    })()`);
    const M = "mock-machine-00000", task = id => `${origin}/m/${M}/t/${id}`, S = task(data.C.session);
    const pages = [
      ["live", S], ["worker side", `${S}/workers/${data.W.backend}`], ["worker live", `${S}/workers/${data.W.backend}/live`],
      ["review", task(data.C.review)], ["paused", task(data.C.paused)], ["stopping", task(data.C.stopping)],
      ["closed", task(data.C.closedAsIs)], ["archived", task(data.C.archive12)], ["offline", task(data.C.offline)],
      ["new task", `${origin}/m/${M}/new`], ["new sprint", `${origin}/m/${M}/new?kind=sprint`],
      ["sprint planning", `${origin}/m/${M}/p/mock-project-00001/s/${data.sprint}/planning`],
      ["other answer", task(data.C.question), `[...document.querySelectorAll(".opt")].find(o => o.textContent.includes("Other answer"))?.click()`],
    ];
    if (data.draft) pages.push(["draft", task(data.draft)]);
    for (const name of ["task", "result", "stop", "new-task", "sprint", "worker"]) pages.push([`mockup ${name}`, `${pathToFileURL(join(mockups, `${name}.html`))}`]);

    let checks = 0, measured = 0;
    const failures = new Map(), widths = new Map(), missing = [];
    for (const [name, url, prepare] of pages) {
      await cdp("Emulation.setDeviceMetricsOverride", { width: 1600, height: 900, deviceScaleFactor: 1, mobile: false });
      if (!await open(url)) { if (!prepare) { missing.push(name); continue; } }
      await run(PAGE);
      if (prepare) { await run(prepare); await sleep(100); }
      if (name === "live" || name === "mockup task") {
        const menu = await run("__fit.menu()");
        console.log(`${name}: first menu ${JSON.stringify(menu)}`);
        if (!menu.open || !menu.inWindow || menu.gap > 12) failures.set(`${name}: menu not beside its trigger`, { at: ["1600x900"], n: 1 });
      }
      if (name === "live") {
        for (const [kind, label] of [["model", "GPT-6.1 Terra"], ["effort", "Medium"], ["mode", "Development"], ["lang", "English"]]) { await run(`__fit.pick(${JSON.stringify(kind)}, ${JSON.stringify(label)})`); await settle(); }
        for (const [width, height] of [[1920, 1080], [1280, 800], [640, 400]]) {
          await cdp("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
          await settle();
          const [bar] = await run("__fit.measure()");
          console.log(`live, labels picked through the real menus, ${width}x${height}: bar ${bar.width}px ${bar.height}px high, route ${bar.route}px: ${bar.labels}; selectors ${bar.state}, problems ${bar.bad.length}`);
          if (bar.bad.length) failures.set(`live real labels: ${bar.bad[0]}`, { at: [`${width}x${height}`], n: 1 });
        }
      }
      const hasInspector = !name.startsWith("mockup") && await run(`!!document.querySelector('[aria-label="Inspector"]')`);
      for (const [width, height] of WINDOWS) {
        await cdp("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
        for (const inspector of hasInspector ? [false, true] : [false]) {
          if (inspector) { await run(`document.querySelector('[aria-label="Inspector"]').click()`); }
          for (const doubled of [false, true]) for (const typed of [false, true]) {
            if (name.startsWith("mockup") && typed) continue;
            await run(`__fit.text(${doubled}); __fit.type(${JSON.stringify(typed ? "A draft" : "")}); __fit.longest()`);
            await settle();
            await run("__fit.longest()");
            await settle();
            const bars = await run("__fit.measure()");
            checks++;
            measured += bars.length;
            for (const bar of bars) {
              const key = `${name} ${doubled ? "2x text" : "1x text"}`;
              if (!doubled && !typed && !inspector) widths.set(`${name} ${width}`, `bar ${bar.width}px ${bar.height}px high, selectors ${bar.state || "none"}`);
              for (const problem of bar.bad) { const k = `${key}: ${problem.replace(/ by \d+px/, "")}`; const f = failures.get(k) ?? { at: [], n: 0 }; f.n++; if (!f.at.includes(`${width}x${height}${inspector ? "+inspector" : ""}`)) f.at.push(`${width}x${height}${inspector ? "+inspector" : ""}`); failures.set(k, f); }
            }
          }
          if (inspector) { await run(`document.querySelector('[aria-label="Inspector"]').click()`); }
        }
      }
      await run(`__fit.text(false); __fit.type("")`);
    }
    for (const [k, v] of widths) if (/ (1920|1280|640)$/.test(k)) console.log(`${k}: ${v}`);
    console.log(`\n${checks} layouts, ${measured} bars measured across ${pages.length} screens.`);
    if (missing.length) console.log(`No composer bar found on: ${missing.join(", ")}`);
    for (const [k, f] of [...failures].slice(0, 40)) console.log(`FAIL ${k} (${f.n}x) at ${f.at.slice(0, 4).join(", ")}${f.at.length > 4 ? ", ..." : ""}`);
    console.log(failures.size || missing.length ? `${failures.size} kinds of failure.` : "Every control stayed inside its composer.");
    process.exitCode = failures.size || missing.length ? 1 : 0;
    socket.close();
  } finally {
    stop();
    await sleep(200);
    rmSync(profile, { recursive: true, force: true });
  }
}
main().catch(e => { console.error(e); process.exit(2); });
