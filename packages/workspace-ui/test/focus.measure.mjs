#!/usr/bin/env node
// Text-only focus and panel-toggle check in real WebKitGTK. jsdom has no style cascade or focus heuristics, so Vitest cannot answer this.
//
// It builds the demo workspace and opens it in a real WebKit view on a private X display inside the rendered-check image, drives
// it with real mouse and keyboard events from xdotool, and reads computed styles and rectangles. No screenshot is taken and no
// image is read. Exit code 1 means a press, click or route change left a highlight or moved something, or a panel toggle lied.
//
//     node packages/workspace-ui/test/focus.measure.mjs            (MEASURE_ONLY=<page> limits the pages)
import {spawnSync} from "node:child_process";
import {fileURLToPath} from "node:url";
import {mkdirSync} from "node:fs";
import {dirname, join, resolve} from "node:path";

const IMAGE = process.env.CW_RENDERED_IMAGE ?? "cortex-workspace-rendered:ubuntu26.04";
const here = dirname(fileURLToPath(import.meta.url));

if (!process.argv.includes("--inside")) {
  const root = resolve(here, "../../..");
  const site = join(root, ".workspace-build/focus-measure/site");
  mkdirSync(dirname(site), {recursive: true});
  const built = spawnSync("npx", ["vite", "build", "--logLevel", "error", "--emptyOutDir", "--outDir", site], {cwd: join(root, "prototypes/cortex-workspace/desktop"), stdio: "inherit"});
  if (built.status !== 0) process.exit(built.status ?? 1);
  const run = spawnSync("docker", ["run", "--rm", "--network", "none", "-e", "HOME=/tmp", "-e", "XDG_RUNTIME_DIR=/tmp", "-e", `MEASURE_ONLY=${process.env.MEASURE_ONLY ?? ""}`, "-v", `${site}:/site:ro`, "-v", `${here}:/driver:ro`, IMAGE, "node", "/driver/focus.measure.mjs", "--inside"], {stdio: "inherit", timeout: 1500000});
  process.exit(run.status ?? 1);
}

const {Host, startXvfb, engineEnv, main, sleep, xdotool, startOpenbox, serveSite} = await import("./rendered/webkit.mjs");
const WINDOW = [1880, 1100];
const PARK = [1915, 1195]; // Outside the window, so nothing is hovered.
const LOOK = ["outlineStyle", "outlineWidth", "outlineColor", "boxShadow", "borderTopColor", "borderTopWidth", "backgroundColor", "color", "fontWeight"];
// Real clicks on what a click leaves unchanged. Every other control is pressed and released away, so it takes focus and :active without acting.
const CLICKS = [
  ["home", ".nav[aria-current]", null], ["board", ".proj[aria-current]", null], ["task", '.tab[aria-selected="true"]', null],
  ["task", ".composer textarea", ".composer"], ["new", ".composer textarea", ".composer"], ["new", "textarea", null], ["archive", ".inp", null], ["settings_machine", ".inp", null],
];
// Moving the pointer with the button down would resize or reorder these.
const DRAGGED = ".ws-resize, .grip, [draggable='true'], [role='separator']";

await main(async () => {
  const display = await startXvfb();
  startOpenbox();
  await sleep(.5);
  const origin = await serveSite("/site");
  const host = await Host.start({width: WINDOW[0], height: WINDOW[1], decorated: false, env: engineEnv(display), timeout: 15});
  await host.move(0, 0);
  const failures = [], passed = [], facts = {};
  const js = async body => { try { return await host.js(body); } catch (error) { if (error.timeout) throw Error("TimeoutError: page did not answer"); return {error: error.message}; } };
  const call = (f, ...args) => js(`return (${f.toString()})(${args.map(a => JSON.stringify(a ?? null)).join(",")});`);
  const x = async (...args) => { xdotool(...args); await sleep(.07); };
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

  // The page-side helpers every measurement uses.
  const install = () => call(LOOK => {
    window.__m = {
      look(el) { const s = getComputedStyle(el); return Object.fromEntries(LOOK.map(k => [k, s[k]])); },
      box(el) { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(n => Math.round(n * 100) / 100).join(","); },
      check(el) {
        const r = el.getBoundingClientRect(), s = getComputedStyle(el);
        if (r.width < 4 || r.height < 4 || s.visibility === "hidden" || r.top < 0 || r.left < 0 || r.right > innerWidth || r.bottom > innerHeight) return false;
        const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
        return !!hit && (hit === el || el.contains(hit) || hit.closest("label")?.contains(el));
      },
      find(selector) { return [...document.querySelectorAll(selector)].find(el => this.check(el)) ?? null; },
      frame() { return [...document.querySelectorAll(".titlebar button, .sidebar .nav, .sidebar .proj, .main [role=tab]")].map(el => this.box(el)).join(";"); },
    };
    return 1;
  }, LOOK);
  const SWEEP = () => call(DRAGGED => {
    const seen = new Map();
    for (const el of document.querySelectorAll("button, a, input, textarea, label, summary, [tabindex], [role='tab'], [role='option'], [role='button'], [role='treeitem']")) {
      if (el.matches(DRAGGED) || el.closest(DRAGGED) || el.matches("main[tabindex], [type=file], .pt")) continue;
      const sign = [el.tagName.toLowerCase(), ...[...el.classList].sort().map(c => "." + c), ...["aria-current", "aria-selected", "aria-pressed", "type"].filter(a => el.hasAttribute(a)).map(a => `[${a}="${el.getAttribute(a)}"]`)].join("");
      if (seen.has(sign) || !__m.check(el)) continue;
      el.dataset.m = String(seen.size);
      seen.set(sign, `[data-m="${seen.size}"]`);
    }
    return [...seen];
  }, DRAGGED);

  async function load(path) {
    await host.load(origin + path);
    let ready = false;
    for (let i = 0; i < 100 && !ready; i++) {
      await sleep(.1);
      ready = await js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') && document.querySelectorAll('.nav').length + document.querySelectorAll('.snav').length > 0") === true;
    }
    if (!ready) throw Error("TimeoutError: no workspace at " + path);
    await sleep(.3);
    await install();
    await js("document.querySelector('.ws-demo-banner')?.remove(); return 1"); // The product has no demo row above its title band.
  }
  const size = async (width, height) => { await host.resize(width, height); await sleep(.5); };
  const check = (name, ok, details = {}) => { if (ok) passed.push(name); else failures.push({check: name, ...details}); };
  const diff = (a, b) => Object.fromEntries(Object.keys(a).filter(k => a[k] !== b[k]).map(k => [k, [a[k], b[k]]]));
  const empty = o => Object.keys(o).length === 0;

  async function measure(theme, page, selector, watch, click, label = null) {
    const name = `${theme} ${page} ${label ?? selector}`;
    await x("mousemove", ...PARK);
    await js("document.activeElement?.blur(); return 1");
    const point = await call((selector, watch) => {
      const el = __m.find(selector); if (!el) return null; el.scrollIntoView({block: "nearest"}); window.__el = el;
      window.__w = watch ? (el.closest(watch) ?? el.parentElement.querySelector(watch)) : el;
      const r = el.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)];
    }, selector, watch);
    if (point === null) { (facts.not_on_screen ??= []).push(name); return; }
    const sample = () => js("return { look: __m.look(__w), own: __m.box(__el), frame: __m.frame(), active: document.activeElement.tagName, ring: __m.look(document.activeElement).outlineStyle, visible: document.activeElement.matches(':focus-visible'), kb: document.documentElement.hasAttribute('data-keyboard') };");
    const rest = await sample();
    await x("mousemove", ...point);
    const hover = await sample();
    let held;
    if (click) {
      await x("click", 1);
      held = await sample();
      check(name + " click on hover", empty(diff(hover.look, held.look)), {changed: diff(hover.look, held.look)});
      await x("mousemove", ...PARK);
    } else {
      await x("mousedown", 1);
      held = await sample();
      check(name + " press", empty(diff(hover.look, held.look)), {changed: diff(hover.look, held.look)});
      await x("mousemove", ...PARK);
      await x("mouseup", 1);
    }
    const after = await sample();
    check(name + " after", empty(diff(rest.look, after.look)), {changed: diff(rest.look, after.look)});
    check(name + " focus line", after.ring === "none" && !after.kb, {active: after.active, ring: after.ring, kb: after.kb});
    check(name + " no shift", rest.own === after.own && rest.frame === after.frame, {own: [rest.own, after.own]});
    facts.mouse_left_focus_on = [...new Set([...(facts.mouse_left_focus_on ?? []), after.active])].sort();
    facts.controls_measured = (facts.controls_measured ?? 0) + 1;
    if (after.visible) (facts.engine_says_focus_visible_after_mouse ??= []).push(name);
  }

  const linear = channel => { const c = channel / 255; return c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4; };
  const luminance = colour => { const [r, g, b] = colour.slice(colour.indexOf("(") + 1, colour.indexOf(")")).replace(/,/g, " ").split(/\s+/).filter(Boolean).slice(0, 3).map(Number); return .2126 * linear(r) + .7152 * linear(g) + .0722 * linear(b); };
  const ratio = (a, b) => { const [hi, lo] = [luminance(a), luminance(b)].sort((p, q) => q - p); return Math.round((hi + .05) / (lo + .05) * 100) / 100; };

  async function panel(theme, label, panelSelector, width) {
    const name = `${theme} ${label} @${width}`;
    const probe = () => call((label, panelSelector) => {
      const b = document.querySelector(`.panel-t[aria-label=${JSON.stringify(label)}]`); if (!b) return null;
      const p = document.querySelector(panelSelector), r = b.getBoundingClientRect(), s = getComputedStyle(b), box = p ? p.getBoundingClientRect() : null;
      let under = b.parentElement; while (under && ["rgba(0, 0, 0, 0)", "transparent"].includes(getComputedStyle(under).backgroundColor)) under = under.parentElement;
      return {pressed: b.getAttribute("aria-pressed"), disabled: b.getAttribute("aria-disabled"), color: s.color, background: s.backgroundColor, outline: s.outlineStyle, shadow: s.boxShadow, opacity: s.opacity, at: __m.box(b), point: [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)],
        open: !!box && box.width > 0 && box.height > 0 && getComputedStyle(p).display !== "none", under: getComputedStyle(under ?? document.body).backgroundColor};
    }, label, panelSelector);
    const first = await probe();
    if (first === null) { (facts.toggle_absent ??= []).push(name); return; }
    if (first.disabled) { (facts.toggle_disabled ??= []).push(name); return; }
    await x("mousemove", ...first.point);
    await x("click", 1);
    const hovering = await probe();
    await x("mousemove", ...PARK);
    const second = await probe();
    await x("mousemove", ...first.point);
    await x("click", 1);
    await x("mousemove", ...PARK);
    const third = await probe();
    const states = {[first.pressed]: first, [second.pressed]: second};
    check(name + " state is the panel's", [first, second, third].every(s => (s.pressed === "true") === s.open), {states: [first, second, third].map(s => [s.pressed, s.open])});
    check(name + " comes back", third.pressed === first.pressed && third.open === first.open && third.color === first.color, {states: [first, second, third].map(s => [s.pressed, s.open, s.color])});
    check(name + " hover adds nothing", hovering.color === second.color && hovering.background === second.background, {hover: hovering.color, rest: second.color});
    check(name + " no other mark", [first, second, third, hovering].every(s => s.outline === "none" && s.shadow === "none" && s.background === "rgba(0, 0, 0, 0)"));
    check(name + " stays put", first.at === second.at && second.at === third.at, {at: [first.at, second.at]});
    if (Object.keys(states).sort().join() === "false,true") {
      const on = states.true, off = states.false;
      check(name + " lighter when open", luminance(on.color) > luminance(off.color), {on: on.color, off: off.color});
      (facts.toggles ??= []).push({toggle: name, open: on.color, closed: off.color, between: ratio(on.color, off.color), open_on_band: ratio(on.color, on.under), closed_on_band: ratio(off.color, off.under)});
    } else check(name + " changes state", false, {states: Object.keys(states)});
  }

  // A switch hides its input under the track it paints. A real click anywhere on that track, with or without a
  // label around the switch, must turn it; a second click turns it back.
  async function switches(theme, page) {
    // Found again by name before every read: a toggle may render its row anew.
    const named = await js("const seen = {}; return [...document.querySelectorAll('.sw')].flatMap(s => { const i = s.querySelector('input'), name = i?.getAttribute('aria-label'); if (!name) return []; const k = seen[name] = (seen[name] ?? -1) + 1; return !i.disabled && __m.check(s) ? [[name, k]] : []; });");
    for (const [n, [label, k]] of named.entries()) {
      const probe = () => call((label, k) => {
        const i = [...document.querySelectorAll(".sw input")].filter(e => e.getAttribute("aria-label") === label)[k]; if (!i) return null;
        const s = i.parentElement; s.scrollIntoView({block: "nearest"}); const r = s.getBoundingClientRect(), y = Math.round(r.y + r.height / 2);
        return {on: i.checked, name: i.getAttribute("aria-label"), at: [[Math.round(r.x + r.width / 2), y], [Math.round(r.right - 3), y]]};
      }, label, k);
      const states = [await probe()];
      if (states[0] === null) { (facts.not_on_screen ??= []).push(`${theme} ${page} switch ${n}`); continue; }
      for (const point of states[0].at) {
        await x("mousemove", ...point);
        await x("click", 1);
        await sleep(.15);
        states.push(await probe());
      }
      await x("mousemove", ...PARK);
      const turned = !states.includes(null) && states[1].on !== states[0].on && states[2].on === states[0].on;
      check(`${theme} ${page} switch ${states[0].name} turns on a click`, turned, {states: states.map(s => s && s.on)});
      facts.switches_clicked = (facts.switches_clicked ?? 0) + 1;
    }
  }

  // What Tab landed on. A switch hides its input and draws the line on the track beside it, so the mark is read where it is painted.
  const STOP = "const a = document.activeElement, text = a.matches('textarea, [contenteditable], input:not([type=checkbox], [type=radio])');\n"
    + "const mark = a.matches('.sw input') ? a.nextElementSibling : a, s = getComputedStyle(mark), r = mark.getBoundingClientRect(), reach = (parseFloat(s.outlineWidth) || 0) + (parseFloat(s.outlineOffset) || 0), clipped = [];\n"
    + "for (let p = mark.parentElement; p; p = p.parentElement) {\n"
    + "  const c = getComputedStyle(p), b = p.getBoundingClientRect(), left = b.x + p.clientLeft, top = b.y + p.clientTop;\n"
    + "  if (c.overflowX !== 'visible' && (r.left - reach < left - .5 || r.right + reach > left + p.clientWidth + .5)) clipped.push(p.className || p.tagName);\n"
    + "  if (c.overflowY !== 'visible' && (r.top - reach < top - .5 || r.bottom + reach > top + p.clientHeight + .5)) clipped.push(p.className || p.tagName);\n"
    + "}\n"
    + "return { tag: a.tagName, name: a.getAttribute('aria-label') || a.textContent.trim().slice(0, 40), text, style: s.outlineStyle, width: s.outlineWidth, colour: s.outlineColor, clipped, kb: document.documentElement.hasAttribute('data-keyboard') };";
  const drawn = state => state.kb && (state.text ? state.style === "none" : state.style === "solid" && state.width === "1px" && !state.clipped.length);

  async function keyboard(theme, path, start) {
    await load(path);
    await x("mousemove", ...PARK);
    await js(start);
    const seen = {};
    for (let i = 0; i < 45; i++) {
      await x("key", "Tab");
      const state = await js(STOP);
      if (state.tag === "BODY") continue;
      const kind = state.text ? "text field" : "control";
      (seen[kind] ??= new Set()).add(`${state.style} ${state.width} ${state.colour}`);
      check(`${theme} ${path} Tab ${kind}`, drawn(state), {state});
    }
    for (const [kind, looks] of Object.entries(seen)) {
      const kinds = ((facts.keyboard ??= {})[theme] ??= {});
      kinds[kind] = new Set([...(kinds[kind] ?? []), ...looks]);
    }
    await js("return 1");
    await x("mousemove", 900, 700);
    await x("click", 1);
    const cleared = await js("const a = document.activeElement; return { kb: document.documentElement.hasAttribute('data-keyboard'), ring: getComputedStyle(a).outlineStyle };");
    check(`${theme} a click ends the line`, !cleared.kb && cleared.ring === "none", {state: cleared});
    await x("mousemove", ...PARK);
  }

  let code = 1;
  try {
    await sleep(1);
    await load("/");
    const machine = await js("return location.pathname.split('/')[2]");
    const project = await js("return document.querySelector('.proj').getAttribute('href').split('/')[4]");
    await load(`/m/${machine}/p/${project}/board`);
    const task = await js("return document.querySelector('.card-link').getAttribute('href').split('/')[4]");
    const at = `/m/${machine}`, pj = `/m/${machine}/p/${project}`, tk = `/m/${machine}/t/${task}`, st = `/settings/machine/${machine}`;
    const routes = {home: at, inbox: `${at}/inbox`, archive: `${at}/archive`, new: `${at}/new`, new_sprint: `${at}/new?kind=sprint`, board: `${pj}/board`, list: `${pj}/list`, code: `${pj}/code`, git: `${pj}/git`,
      worktrees: `${pj}/worktrees`, project_settings: `${pj}/settings`, task: tk, task_workers: `${tk}/workers`, task_changes: `${tk}/changes`, task_code: `${tk}/code`,
      appearance: "/settings/app/appearance", voice: "/settings/app/voice", shortcuts: "/settings/app/shortcuts", machines: "/settings/app/machines", settings_machine: `${st}/overview`,
      connections: `${st}/connections`, storage: `${st}/storage`, machine_worktrees: `${st}/worktrees`, defaults: `${st}/orchestration/defaults`, models: `${st}/orchestration/models`,
      routing: `${st}/orchestration/routing`, tools: `${st}/orchestration/tools`, profiles: `${st}/orchestration/profiles`, policy: `${st}/orchestration/policy`};
    const only = process.env.MEASURE_ONLY;
    for (const theme of ["dark", "light"]) {
      await load("/");
      await js(`localStorage.setItem('cw.theme', ${JSON.stringify(theme)}); return 1`);
      for (const [page, path] of Object.entries(routes)) {
        if (only && page !== only) continue;
        await load(path);
        check(`${theme} theme applied`, await js("return document.documentElement.dataset.theme") === theme);
        for (const [shown, selector, watch] of CLICKS) if (shown === page) await measure(theme, page, selector, watch, true);
        for (const [sign, selector] of await SWEEP()) await measure(theme, page, selector, null, false, sign);
        await switches(theme, page);
      }

      // Route change by a real click: focus falls to the main area, which draws nothing.
      await load(routes.home);
      const nav = await js("const r = __m.find('.nav:not([aria-current])').getBoundingClientRect(); return [Math.round(r.x + 20), Math.round(r.y + r.height / 2)]");
      await x("mousemove", ...nav);
      await x("click", 1);
      await sleep(.3);
      await x("mousemove", ...PARK);
      const routed = await js("const a = document.activeElement, s = getComputedStyle(a); return { tag: a.tagName, ring: s.outlineStyle, shadow: s.boxShadow };");
      check(`${theme} route change`, routed.ring === "none" && routed.shadow === "none", {state: routed});
      facts.route_change_focus = routed.tag;

      // A popover menu and a dialog opened with the mouse.
      await load(routes.home);
      await x("mousemove", ...await js("const r = __m.find('.machine').getBoundingClientRect(); return [Math.round(r.x + 40), Math.round(r.y + 14)]"));
      await x("click", 1);
      const item = await js("const el = __m.find('[popover]:popover-open .mi'); if (!el) return null; window.__el = window.__w = el; const r = el.getBoundingClientRect(); return [Math.round(r.x + 40), Math.round(r.y + r.height / 2)];");
      if (item) {
        await x("mousemove", ...item);
        const hover = await js("return __m.look(__w)");
        await x("mousedown", 1);
        const held = await js("return __m.look(__w)");
        check(`${theme} menu item press`, empty(diff(hover, held)), {changed: diff(hover, held)});
        await x("mousemove", ...PARK);
        await x("mouseup", 1);
        const ring = await js("return { ring: getComputedStyle(document.activeElement).outlineStyle, kb: document.documentElement.hasAttribute('data-keyboard') }");
        check(`${theme} menu item focus`, ring.ring === "none" && !ring.kb, {state: ring});
      } else (facts.not_on_screen ??= []).push(`${theme} menu item`);
      await x("key", "Escape");

      await load("/settings/app/machines");
      const opener = await js("const el = [...document.querySelectorAll('.main button')].find(b => /connect|add/i.test(b.textContent)); if (!el) return null; const r = el.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)];");
      if (opener) {
        await x("mousemove", ...opener);
        await x("click", 1);
        await sleep(.2);
        const shownDialog = await js("const a = document.activeElement, s = getComputedStyle(a); return { open: !!document.querySelector('dialog[open]'), tag: a.tagName, ring: s.outlineStyle, visible: a.matches(':focus-visible') };");
        check(`${theme} dialog from a click`, shownDialog.open && shownDialog.ring === "none", {state: shownDialog});
        facts.dialog_focus = shownDialog;
        let insideDialog;
        for (let i = 0; i < 4; i++) { // Past a dialog's last control the engine rests on the document and on the dialog itself, which draw nothing.
          await x("key", "Tab");
          insideDialog = await js(STOP);
          if (!["BODY", "DIALOG"].includes(insideDialog.tag)) break;
        }
        check(`${theme} Tab inside a dialog`, drawn(insideDialog), {state: insideDialog});
        await x("key", "Escape");
      } else (facts.not_on_screen ??= []).push(`${theme} dialog opener`);

      // Panels, against the state of the real panel.
      await load(routes.task);
      for (const [label, selector] of [["Sidebar", ".sidebar"], ["Inspector", ".inspector"], ["Worker pane", ".detail"], ["Terminal", ".terminal"]]) await panel(theme, label, selector, WINDOW[0]);
      await size(1300, 900);
      await load(routes.task);
      for (const [label, selector] of [["Sidebar", ".sidebar"], ["Inspector", ".inspector"]]) await panel(theme, label, selector, 1300);
      await size(...WINDOW);
      await load(routes.board);
      await panel(theme, "Terminal", ".terminal", WINDOW[0]);
      // From the top of the window, then from the page itself, because the sidebar alone takes most of the walk.
      for (const page of ["new", "home", "board", "task", "git", "appearance", "routing"]) await keyboard(theme, routes[page], "document.activeElement?.blur(); return 1");
      for (const page of ["home", "inbox", "board", "list", "code", "git", "worktrees", "task", "task_workers", "task_changes", "settings_machine", "routing", "tools", "profiles"]) await keyboard(theme, routes[page], "document.querySelector('main').focus(); return 1");
    }
    code = failures.length ? 1 : 0;
  } catch (error) { // A broken run is a failed run, never a pass.
    failures.push({check: "harness", error: `${error.name}(${JSON.stringify(error.message)})`});
  }
  const replacer = (key, value) => value instanceof Set ? [...value].sort() : value;
  console.log(JSON.stringify({engine: `WebKitGTK ${host.version.join(".")}`, passed: passed.length, failed: failures.length, facts, failures}, replacer, 1));
  return code;
});
