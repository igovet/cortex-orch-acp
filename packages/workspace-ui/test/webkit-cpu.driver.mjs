// Opt-in /proc CPU measurements; fake data on an owned software virtual screen. Run through webkit-cpu.mjs, inside the rendered image.
import {Host, startXvfb, engineEnv, main, sleep, join, onCleanup, screen} from "./rendered/webkit.mjs";
import assert from "node:assert/strict";
import {mkdtempSync, mkdirSync, readdirSync, readFileSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {execFileSync} from "node:child_process";

const base = process.argv[2];

await main(async () => {
  const scratch = mkdtempSync(join(tmpdir(), "webkit-cpu-"));
  onCleanup(() => rmSync(scratch, {recursive: true, force: true}));
  for (const [key, sub] of [["HOME", "home"], ["XDG_CONFIG_HOME", "config"], ["XDG_STATE_HOME", "state"], ["XDG_CACHE_HOME", "cache"], ["XDG_DATA_HOME", "data"], ["XDG_RUNTIME_DIR", "runtime"]]) {
    const path = join(scratch, sub);
    mkdirSync(path, {mode: 0o700});
    process.env[key] = path;
  }
  const display = await startXvfb("1440x1000x24");
  // Software rendering with compositing off. The container has no user namespace for WebKit's sandbox, so it is off here.
  const env = {...engineEnv(display), LIBGL_ALWAYS_SOFTWARE: "1"};
  const host = await Host.start({width: 1440, height: 1000, env, timeout: 5});
  await host.load(base);
  const js = code => host.js(code);

  const snapshot = () => {
    const rows = new Map();
    for (const name of readdirSync("/proc")) {
      if (!/^\d+$/.test(name)) continue;
      try {
        const raw = readFileSync(`/proc/${name}/stat`, "utf8");
        const fields = raw.slice(raw.lastIndexOf(")") + 2).split(" ");
        rows.set(+name, [+fields[1], raw.slice(raw.indexOf("(") + 1, raw.lastIndexOf(")")), +fields[11] + +fields[12]]);
      } catch { /* the process is gone */ }
    }
    return rows;
  };
  const xvfbPid = screen.xvfbPid;
  const measure = async (state, seconds = 3) => {
    const a = snapshot();
    const owned = new Set([process.pid, xvfbPid]);
    for (let i = 0; i < 6; i++) for (const [p, v] of a) if (owned.has(v[0])) owned.add(p);
    const start = performance.now();
    await sleep(seconds);
    const elapsed = (performance.now() - start) / 1000;
    const b = snapshot();
    const hz = +execFileSync("getconf", ["CLK_TCK"], {encoding: "utf8"}).trim();
    console.log(JSON.stringify({state, seconds: elapsed, hz, processes: [...owned].sort((x, y) => x - y).filter(p => a.has(p) && b.has(p)).map(p => ({pid: p, name: b.get(p)[1], cpuPercent: 100 * (b.get(p)[2] - a.get(p)[2]) / hz / elapsed}))}));
  };

  try {
    let ready = false;
    for (let i = 0; i < 100 && !ready; i++) { ready = await js("return !!(window.fixture && document.querySelector('.titlebar'))"); if (!ready) await sleep(.1); }
    if (!ready) throw Error("TimeoutError: fixture readiness");
    const running = "return document.getAnimations().filter(a=>a.playState==='running').map(a=>a.animationName)";
    // Nothing pending: the task has settled, so no animation may run at all.
    await js("fixture.working(false)");
    await sleep(1.5);
    console.log(JSON.stringify({engine: "WebKitGTK", version: host.version, nodes: await js("return document.querySelectorAll('*').length"),
      animations: await js("return document.getAnimations().map(a=>({name:a.animationName,state:a.playState}))")}));
    assert.deepEqual(await js(running), [], "An animation runs with nothing pending");
    await measure("idle-nothing-pending");
    // A pressed control awaiting its reply: its bar sweeps after 150 ms, and stops the moment the reply settles.
    assert.ok(await js(`const b = [...document.querySelectorAll('.titlebar .btn, .main .btn')].find(x => x.getClientRects().length);
          if (!b) return false; b.setAttribute('data-busy', ''); b.setAttribute('aria-busy', 'true'); window.__busy = b; return true;`), "No visible button");
    await sleep(.4);
    const pressed = await js(running);
    console.log(JSON.stringify({state: "button-pending", running: pressed}));
    assert.ok(pressed.includes("ws-busy-run"), "A pending button shows no moving loader");
    await measure("button-pending");
    await js("__busy.removeAttribute('data-busy'); __busy.removeAttribute('aria-busy')");
    await sleep(.2);
    assert.deepEqual(await js(running), [], "A settled button keeps animating");
    // A running task: its long-running working status turns slowly, and only while the window is shown.
    await js("fixture.working(true)");
    await sleep(1);
    const work = await js(`return document.getAnimations().filter(a=>a.playState==='running').map(a=>({name:a.animationName,
          ms:a.effect.getTiming().duration,easing:a.effect.getTiming().easing}))`);
    console.log(JSON.stringify({state: "working-visible", running: work}));
    assert.ok(work.length && work.every(a => a.name === "spin"), "A working task shows no slow loader");
    await measure("working-visible");
    await host.hide();
    for (let i = 0; i < 30; i++) { if (await js("return document.hidden")) break; await sleep(.1); }
    if (await js("return document.hidden")) {
      assert.deepEqual(await js(running), [], "A hidden window keeps animating");
      await measure("working-hidden");
    } else {
      console.log(JSON.stringify({state: "working-hidden", result: "not_observable", reason: "document.hidden did not become true after hiding the window"}));
    }
    await host.show();
    await sleep(.5);
    await js("window.cpuUpdates=0;window.cpuTimer=setInterval(()=>{fixture.emit(true);window.cpuUpdates++},1000)");
    await measure("working-updates-one-per-second", 4);
    assert.ok((await js("clearInterval(window.cpuTimer);return window.cpuUpdates")) >= 3);
    return 0;
  } catch (error) {
    console.error(error.name + ": " + error.message);
    return 1;
  }
});
