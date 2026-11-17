// Captures of the task conversation in WebKitGTK on a private virtual screen. Run through conversation-shots.mjs, inside the rendered image.
import {Host, startXvfb, engineEnv, main, sleep, probe, join} from "./rendered/webkit.mjs";

const evidence = "/evidence";
const base = process.argv[2];
const ROWS = probe("conversation-shots.ROWS.js");
// The three states the owner reads a task in, the coordinator's streamed record in Work details, and the sizes: the desktop
// window's usual, large and smallest, and a phone.
const STATES = [
  ["question", "/m/<M>/t/c-ask", ".dc .dc-q"],
  ["working", "/m/<M>/t/c-work", ".queue .qrow"],
  ["result", "/m/<M>/t/c-done", ".rcard"],
  ["coordinator", "/m/<M>/t/c-work/workers/cortex", ".livepane .ev-note"],
];
const SIZES = [[1280, 800], [1920, 1080], [960, 540], [390, 844]];

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1280, height: 800, env: engineEnv(display)});
  let loads = 0;
  const capture = async name => { await sleep(.4); return host.size().then(async size => { await host.capture(join(evidence, name + ".png")); return size; }); };
  const made = [];
  for (const theme of ["", "&dark=1"]) {
    for (const [state, route, ready] of STATES) {
      for (const [width, height] of theme ? [[1280, 800]] : SIZES) {
        loads++;
        await host.resize(width, height);
        await host.load(base + "?world=conversation&route=" + encodeURIComponent(route) + theme + `&load=${loads}`);
        await host.wait(`window.fixture === '${loads}'`);
        await host.wait(`document.querySelector(${JSON.stringify(ready)})`);
        await sleep(.6);
        const actual = await host.js("return [innerWidth, innerHeight, document.documentElement.scrollWidth > innerWidth + 1]");
        const rows = await host.js(ROWS);
        const name = `${state}-${width}x${height}` + (theme ? "-dark" : "");
        const size = await capture(name);
        made.push({file: name + ".png", window: size, viewport: actual.slice(0, 2), page_scrolls_sideways: actual[2], rows});
      }
    }
  }
  console.log(JSON.stringify(made));
  console.error("capture exit 0");
  return 0;
});
