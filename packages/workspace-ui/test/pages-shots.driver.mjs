// Captures of every page in WebKitGTK on a private virtual screen, with measurements. Run through pages-shots.mjs.
//
// Optional arguments after the URL name the pages to capture; without them every page is captured. CW_SET=polish captures five
// window sizes in both themes; CW_LABEL_FIT is label-fit.js, the rule for wrapped and off-centre labels.
import {Host, startXvfb, engineEnv, main, sleep, probe, join, writeJson, sub} from "./rendered/webkit.mjs";
import {readFileSync, rmSync} from "node:fs";

const base = process.argv[2], WANTED = new Set(process.argv.slice(3)), evidence = "/evidence";
const M = "/settings/machine/<M>";
const PAGES = [
  ["home", "/m/<M>"], ["inbox", "/m/<M>/inbox"], ["archive", "/m/<M>/archive"],
  ["new-task", "/m/<M>/new?project=p"], ["new-sprint", "/m/<M>/new?kind=sprint&project=p"],
  ["board", "/m/<M>/p/p/board"], ["list", "/m/<M>/p/p/list"],
  ["sprint-planning", "/m/<M>/p/p/s/sp/planning"], ["sprint-plan", "/m/<M>/p/p/s/sp/plan"], ["sprint-summary", "/m/<M>/p/p/s/sp/summary"],
  ["workers", "/m/<M>/t/c-work/workers"], ["worker", "/m/<M>/t/c-work/workers/w-be2"],
  ["changes", "/m/<M>/t/c-done/changes"], ["ready", "/m/<M>/t/c-accepted"],
  ["knowledge", "/m/<M>/p/p/knowledge"], ["project-settings", "/m/<M>/p/p/settings"],
  ["appearance", "/settings/app/appearance"], ["voice", "/settings/app/voice"], ["updates", "/settings/app/updates"],
  ["shortcuts", "/settings/app/shortcuts"], ["machines", "/settings/app/machines"],
  ["machine", M + "/overview"], ["connections", M + "/connections"], ["worktrees", M + "/worktrees"],
  ["storage", M + "/storage"], ["diagnostics", M + "/diagnostics"], ["remote", M + "/remote"],
  ["orch-defaults", M + "/orchestration/defaults"], ["orch-models", M + "/orchestration/models"],
  ["orch-routing", M + "/orchestration/routing"], ["orch-preview", M + "/orchestration/effective"],
  ["orch-limits", M + "/orchestration/policy"], ["orch-tools", M + "/orchestration/tools"],
  ["orch-profiles", M + "/orchestration/profiles"], ["orch-files", M + "/orchestration/files"],
  ["not-found", "/m/<M>/no-such-page"],
];
// CW_SET=review-fixes: the screens a review asked to see, some after a press: (name, route, more query, what to press).
// `sprint=running` is a sprint already at work with a task not yet started; `sprint=closed` is the same sprint after Close sprint.
const CLICK = (selector, fallback) => `const e = [...document.querySelectorAll(${selector})].find(x => x.getClientRects().length || ${fallback}); e.click(); return !!e;`;
const MORE_AREAS = "const e = [...document.querySelectorAll(\"[aria-label='More project areas']\")].find(x => x.getClientRects().length); if (e) e.click(); return true;";
const REVIEW = [
  ["board", "/m/<M>/p/p/board", "", null], ["list", "/m/<M>/p/p/list", "", null],
  // Only where the menu button is shown (a narrow pane); a wide pane shows every area as a tab.
  ["areas-menu", "/m/<M>/p/p/board", "", MORE_AREAS],
  ["knowledge", "/m/<M>/p/p/knowledge", "", null], ["project-settings", "/m/<M>/p/p/settings", "", null],
  ["code-areas-menu", "/m/<M>/p/p/code", "", MORE_AREAS],
  ["sprint-running", "/m/<M>/p/p/s/sp/board", "&sprint=running", null],
  // A task not started yet, in a sprint already at work: Edit task… in its card's menu and on its own page.
  ["edit-task", "/m/<M>/p/p/s/sp/board", "&sprint=running", "document.querySelector(\"[aria-label='Actions for Add rate limits to the login endpoint']\").click(); const e = [...document.querySelectorAll('.mi')].find(x => x.textContent.startsWith('Edit task') && x.getClientRects().length); e.click(); return !!e;"],
  ["card-menu", "/m/<M>/p/p/s/sp/board", "&sprint=running", CLICK(JSON.stringify("[aria-label='Actions for Add rate limits to the login endpoint']"), "false")],
  ["draft-task", "/m/<M>/t/c-draft", "&sprint=running", null],
  ["sprint-summary-running", "/m/<M>/p/p/s/sp/summary", "&sprint=running", null],
  ["sprint-menu", "/m/<M>/p/p/s/sp/summary", "&sprint=running", "document.querySelector(\"[aria-label='Sprint actions']\").click(); return true;"],
  // Close sprint… stands in the band when it has room, and in the sprint's menu when it does not.
  ["close-sprint", "/m/<M>/p/p/s/sp/summary", "&sprint=running", "const shown = () => [...document.querySelectorAll('button')].find(x => x.textContent.trim().startsWith('Close sprint') && x.getClientRects().length); if (!shown()) document.querySelector(\"[aria-label='Sprint actions']\").click(); const e = shown(); e.click(); return !!e;"],
  ["archive-closed-sprint", "/m/<M>/archive", "&sprint=closed", null],
  ["sprint-closed", "/m/<M>/p/p/s/sp/summary", "&sprint=closed", null],
];
const REVIEWING = process.env.CW_SET === "review-fixes";
const POLISH = process.env.CW_SET === "polish" || REVIEWING;
// CW_SET=ui-fixes: the owner's three widths in both themes.
const BOTH = POLISH || process.env.CW_SET === "ui-fixes";
const SIZES = POLISH ? [[1920, 1080], [1280, 800], [960, 540], [820, 1180], [390, 844]] : [[1280, 800], [960, 540], [390, 844]];
const THEMES = BOTH ? [["", ""], ["-dark", "&dark=1"]] : [["", ""]];
const LABEL_FIT = process.env.CW_LABEL_FIT;
if (!LABEL_FIT) throw Error("CW_LABEL_FIT is required");
const MEASURE = sub(probe("pages-shots.MEASURE.js"), "LABEL_FIT", LABEL_FIT);

// The letters as drawn: a label with capitals or ascenders and nothing below its baseline inks from its top to its baseline, so
// the middle of its inked rows is the middle the eye sees. It must lie within 1 px of the middle of its box.
const NO_DESCENDER = /^[^gjpqyQ,;()\[\]{}_|@$дуфрщцДЦЩ]+$/u;
const TALL = /[A-Z0-9bdfhkltА-ЯЁ]/u;
const chars = text => [...text].length; // Python's len() counts code points

function inkCentres(file, entries) {
  const [width, height, stride, channels] = file.shape, data = file.data;
  const found = [];
  for (const e of entries) {
    if (found.length >= 160 || chars(e.text) < 2 || !NO_DESCENDER.test(e.text) || !TALL.test(e.text)) continue;
    const x0 = Math.max(0, Math.trunc(e.run[0])), x1 = Math.min(width, Math.trunc(e.run[2] + .999));
    // The run's own box holds all its letters; a box's border or fill edge just outside it is not ink.
    const y0 = Math.max(0, Math.trunc(e.box[1]) + 1, Math.trunc(e.run[1]) - 1), y1 = Math.min(height, Math.trunc(e.box[3]) - 1, Math.trunc(e.run[3] + .999) + 1);
    if (x1 - x0 < 4 || y1 - y0 < 8) continue;
    const at = (x, y) => { const i = y * stride + x * channels; return [data[i], data[i + 1], data[i + 2]]; };
    const counts = new Map();
    for (let y = y0; y < y1; y++) for (let x = x0; x < x1; x++) { const i = y * stride + x * channels, key = (data[i] << 16) | (data[i + 1] << 8) | data[i + 2]; counts.set(key, (counts.get(key) ?? 0) + 1); }
    let ground = null, best = -1;
    for (const [key, n] of counts) if (n > best) { best = n; ground = key; }
    const g = [(ground >> 16) & 255, (ground >> 8) & 255, ground & 255];
    const rows = [];
    for (let y = y0; y < y1; y++) {
      for (let x = x0; x < x1; x++) { const p = at(x, y); if (Math.max(Math.abs(p[0] - g[0]), Math.abs(p[1] - g[1]), Math.abs(p[2] - g[2])) > 64) { rows.push(y); break; } }
    }
    if (rows.length) found.push({kind: e.kind, label: e.label, rows: [rows[0], rows.at(-1) + 1], mid: Math.round(e.mid * 100) / 100, off: Math.round(((rows[0] + rows.at(-1) + 1) / 2 - e.mid) * 100) / 100, metric: e.ink});
  }
  return found;
}

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1280, height: 800, env: engineEnv(display)});
  const js = code => host.js(code), wait = code => host.wait(code, 100);
  let loads = 0;
  const made = [];
  let problems = 0;
  try {
    for (const [page, route, query, press] of REVIEWING ? REVIEW : PAGES.map(([p, r]) => [p, r, "", null])) {
      if (WANTED.size && !WANTED.has(page)) continue;
      for (const [width, height] of SIZES) for (const [theme, more] of THEMES) {
        loads++;
        await host.resize(width, height);
        await host.load(base + "?world=pages&route=" + encodeURIComponent(route) + query + more + `&load=${loads}`);
        await wait(`window.fixture === '${loads}'`);
        await wait("document.querySelector('.ws-main, main, .l-page') && !document.querySelector('.l-startup')");
        await sleep(.8);
        if (press) {
          if (!(await js(press))) throw Error(`${page}: nothing to press`);
          await sleep(.6);
        }
        const facts = await js(MEASURE);
        const name = `${page}-${width}x${height}${theme}`;
        facts.file = name + ".png";
        await sleep(.3);
        const rawPath = join(evidence, ".pixels-" + process.pid);
        const shape = await host.capture(join(evidence, name + ".png"), {raw: rawPath});
        const pixels = {shape, data: readFileSync(rawPath)};
        rmSync(rawPath);
        facts.window = [shape[0], shape[1]];
        // An open menu is painted over the page: a label it partly covers has the menu's pixels in its box, not its own ink.
        const covers = facts.covers; delete facts.covers;
        const under = (b, c) => b[0] < c[2] && b[2] > c[0] && b[1] < c[3] && b[3] > c[1] && !(b[0] >= c[0] && b[2] <= c[2] && b[1] >= c[1] && b[3] <= c[3]);
        const entries = facts.entries; delete facts.entries;
        facts.drawn = inkCentres(pixels, entries.filter(e => !covers.some(c => under(e.box, c))));
        facts.labels.push(...facts.drawn.filter(d => Math.abs(d.off) > 1).map(d => `letters drawn off centre: ${d.kind} "${d.label}" ${d.off > 0 ? "low" : "high"} by ${Math.abs(d.off)}px`));
        const bad = facts.sideways || facts.mixed.length || facts.named.length || facts.labels.length;
        problems += bad ? 1 : 0;
        made.push(facts);
        if (bad) console.log(JSON.stringify({file: facts.file, sideways: facts.sideways, mixed: facts.mixed, named: facts.named, labels: facts.labels}));
      }
    }
    writeJson(join(evidence, "measurements.json"), made, 1);
    console.log(`pages ${made.length} problems ${problems}`);
    console.error(`capture exit ${problems ? 1 : 0}`);
    return problems ? 1 : 0;
  } catch (error) {
    console.error(`${error.name}: ${error.message}`);
    console.error("capture exit 2");
    return 2;
  }
});
