#!/usr/bin/env node
/**
 * Text-only layout check for the orchestration settings, in a real browser. jsdom has no layout, so Vitest cannot answer this.
 * It starts its own Vite server and a headless Chrome, talks to Chrome over the DevTools protocol, and prints numbers.
 * No screenshot is ever taken. Exit code 1 means something overflowed, was clipped, or moved when a control changed.
 *
 *   node src/workspace/ui/orch-fit.measure.mjs            (from prototypes/cortex-workspace/desktop)
 *
 * CHROME picks the browser, default /usr/bin/google-chrome. FIT_PORT picks the Vite port, default 5198.
 * Chrome runs with --no-sandbox because it only ever loads this local server.
 */
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const desktop = resolve(here, "../../..");
const port = Number(process.env.FIT_PORT ?? 5198);
const origin = `http://127.0.0.1:${port}`;
const M = "mock-machine-00000";
const base = `${origin}/settings/machine/${M}/orchestration`;
const PAGES = ["", "/defaults", "/models", "/routing", "/routing?tab=kinds", "/routing?tab=quota", "/routing?tab=rules", "/effective", "/policy", "/tools", "/tools/new", "/tools/codebase-memory-mcp?tab=tools", "/tools/docs-search?tab=connection", "/tools/playwright?tab=sandbox", "/profiles", "/files"];
const WINDOWS = [[1920, 1080], [1440, 900], [1280, 800], [1100, 700], [1000, 640]];
const sleep = ms => new Promise(r => setTimeout(r, ms));

/** Runs in the page. */
const PAGE = String.raw`(() => {
  const seen = el => el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
  const name = el => (el.getAttribute("aria-label") || el.textContent || el.className || el.tagName).trim().slice(0, 40);
  window.__fit = {
    audit() {
      const out = [];
      const main = document.querySelector(".set-main");
      if (main && main.scrollWidth > main.clientWidth + 1) {
        const edge = main.getBoundingClientRect().right, wide = [...main.querySelectorAll("*")].filter(el => seen(el) && el.getBoundingClientRect().right > edge + 1 && !el.closest(".ws-mwrap")).slice(0, 3).map(name);
        out.push("page scrolls sideways by " + (main.scrollWidth - main.clientWidth) + ": " + wide.join(" | "));
      }
      const ctx = document.querySelector(".set-ctx");
      if (ctx && ctx.scrollWidth > ctx.clientWidth + 1) out.push("context bar overflows by " + (ctx.scrollWidth - ctx.clientWidth));
      const bar = document.querySelector(".savebar");
      if (bar && bar.scrollWidth > bar.clientWidth + 1) out.push("save bar overflows by " + (bar.scrollWidth - bar.clientWidth));
      for (const row of document.querySelectorAll(".gr, .srow")) {
        if (!seen(row) || row.closest(".ws-mwrap")) continue;
        const box = row.getBoundingClientRect(), cells = [...row.children].filter(seen);
        for (const c of cells) {
          const r = c.getBoundingClientRect();
          if (r.right > box.right + 1) out.push("cell leaves its row: " + name(c));
        }
        for (let i = 1; i < cells.length; i++) {
          const a = cells[i - 1].getBoundingClientRect(), b = cells[i].getBoundingClientRect();
          if (row.classList.contains("gr") && a.right > b.left + 1) out.push("cells overlap: " + name(cells[i - 1]) + " / " + name(cells[i]));
        }
      }
      for (const el of document.querySelectorAll(".surface, .gr, .srow, .chip, .btn, .pick")) {
        const s = getComputedStyle(el).boxShadow;
        if (s !== "none" && /\b([6-9]|\d\d)px\b/.test(s)) out.push("heavy shadow: " + name(el));
      }
      return out;
    },
    /** Every row's box, so a change can be compared with what was there before. */
    boxes(only) {
      return [...document.querySelectorAll(only ?? ".gr, .srow, .set-h, .set-ctx, .tabs")].filter(seen).map(el => { const r = el.getBoundingClientRect(); return [Math.round(r.top), Math.round(r.left), Math.round(r.width), Math.round(r.height)].join(","); });
    },
    click(selector) { const el = document.querySelector(selector); if (!el) return false; el.click(); return true; },
    weight(selector) { const el = document.querySelector(selector); return el ? getComputedStyle(el).fontWeight + "/" + getComputedStyle(el).fontSize + "/" + getComputedStyle(el).paddingLeft : null; },
  };
})()`;

async function main() {
  const profile = mkdtempSync(join(tmpdir(), "orch-fit-"));
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
    const open = async url => {
      await cdp("Page.navigate", { url });
      for (let i = 0; i < 100; i++) {
        await sleep(50);
        if (await run(`!!document.querySelector(".set-title")`).catch(() => false)) { await run(PAGE); await settle(); return true; }
      }
      return false;
    };
    await cdp("Page.enable");
    await cdp("Runtime.enable");
    const failures = [];
    let checked = 0;
    for (const [width, height] of WINDOWS) {
      await cdp("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
      for (const page of PAGES) {
        if (!await open(base + page)) { failures.push(`${width}: ${page}: did not open`); continue; }
        checked++;
        for (const f of await run("__fit.audit()")) failures.push(`${width}x${height} ${page || "/"}: ${f}`);
      }
    }
    // Changing a value must move nothing. A save bar is a new row at the bottom of the window, so the page's own rows are compared.
    await cdp("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    const stable = async (page, selector, label, only) => {
      await open(base + page);
      const boxes = `__fit.boxes(${JSON.stringify(only ?? null) === "null" ? "undefined" : JSON.stringify(only)})`;
      const before = await run(boxes);
      if (!await run(`__fit.click(${JSON.stringify(selector)})`)) { failures.push(`stability ${label}: no control`); return; }
      await settle();
      const after = await run(boxes);
      const moved = before.filter((b, i) => after[i] !== b).length;
      console.log(`  ${label}: ${before.length} boxes, ${moved} moved`);
      if (moved || before.length !== after.length) failures.push(`stability ${label}: ${moved} of ${before.length} moved, ${after.length - before.length} added`);
    };
    await stable("/models", '[aria-label="Sonnet: Light"]', "tier toggle");
    await stable("/models", '[aria-label="Haiku enabled"]', "model switch");
    await stable("/tools/codebase-memory-mcp?tab=tools", '[aria-label^="delete_project, writing workers,"]', "matrix tick");
    await stable("/tools/codebase-memory-mcp?tab=tools", '[aria-label^="search_graph, always load,"]', "always load tick");
    await stable("/tools", '[aria-label="vivaldi enabled"]', "server switch");
    await stable("/tools/codebase-memory-mcp?tab=tools", '#m-as-role-codebase-memory-mcp .mi:first-child', "check as role");
    await stable("/tools/codebase-memory-mcp?tab=tools", '[aria-label="codebase-memory-mcp enabled"]', "check as, server switch");
    await stable("/effective", '[aria-label="Complexity"] button:nth-child(3)', "complexity choice", ".ws-pv-in, .ws-pv-in > *");
    // Selected and unselected read at the same weight, size and padding.
    await open(`${base}/routing`);
    const on = await run(`__fit.weight('.tabs [aria-selected="true"]')`), off = await run(`__fit.weight('.tabs [aria-selected="false"]')`);
    console.log(`  tab selected ${on}, unselected ${off}`);
    if (on !== off) failures.push(`selected tab differs: ${on} against ${off}`);
    await open(`${base}/models`);
    const t1 = await run(`__fit.weight('.tier [aria-pressed="true"]')`), t0 = await run(`__fit.weight('.tier [aria-pressed="false"]')`);
    console.log(`  tier pressed ${t1}, not pressed ${t0}`);
    if (t1 !== t0) failures.push(`pressed tier differs: ${t1} against ${t0}`);
    // The paste box keeps pasted lines, so it is a textarea, and it must still read as one input row.
    await open(`${base}/tools/docs-search?tab=connection`);
    const paste = await run(`Math.round(document.querySelector('.ws-line-r textarea').getBoundingClientRect().height)`);
    console.log(`  paste box height ${paste}`);
    if (paste !== 32) failures.push(`paste box is ${paste} high, not 32`);
    console.log(`${checked} page and window pairs measured, ${failures.length} problems`);
    for (const f of [...new Set(failures)].slice(0, 60)) console.log(`  ${f}`);
    process.exitCode = failures.length ? 1 : 0;
    socket.close();
  } finally {
    stop();
    await sleep(200);
    rmSync(profile, { recursive: true, force: true });
  }
}
main().catch(e => { console.error(e); process.exit(2); });
