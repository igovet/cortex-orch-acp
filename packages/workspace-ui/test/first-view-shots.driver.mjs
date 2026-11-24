// Captures of a conversation's first reveal in WebKitGTK on a private virtual screen: the skeleton while its transcript is
// still being read, and after the first-view budget the confirmed part with its notice. Run through first-view-shots.mjs.
import {Host, startXvfb, engineEnv, main, sleep, join} from "./rendered/webkit.mjs";

const evidence = "/evidence";
const base = process.argv[2];
const SIZES = [[1280, 800], [1920, 1080], [960, 540], [390, 844]];
// What each capture reports, read from the rendered page: the skeleton's status line, the busy region, the notice, and whether
// anything is wider than the window.
const FACTS = `return {
  skeleton: !!document.querySelector(".timeline .ws-skel"),
  status: document.querySelector(".timeline .ws-skel [role=status]")?.textContent ?? null,
  busy: document.querySelector(".timeline")?.getAttribute("aria-busy") ?? null,
  notice: document.querySelector(".ws-first-notice")?.textContent ?? null,
  sideways: document.documentElement.scrollWidth > innerWidth + 1,
  skeletonWidth: Math.round(document.querySelector(".ws-skel")?.getBoundingClientRect().width ?? 0),
}`;

await main(async () => {
  const display = await startXvfb();
  const host = await Host.start({width: 1280, height: 800, env: engineEnv(display)});
  let loads = 0;
  const made = [];
  const shot = async (name, extra) => {
    await sleep(.4);
    const size = await host.size();
    await host.capture(join(evidence, name + ".png"));
    made.push({file: name + ".png", window: size, ...extra, ...(await host.js(FACTS))});
  };
  for (const theme of ["", "&dark=1"]) {
    for (const [width, height] of theme ? [[1280, 800]] : SIZES) {
      const phone = width < 600 ? "&phone=1" : "";
      loads++;
      await host.resize(width, height);
      await host.load(base + "?world=conversation&loading=1&route=" + encodeURIComponent("/m/<M>/t/c-work") + theme + phone + `&load=${loads}`);
      await host.wait(`window.fixture === '${loads}'`);
      await host.wait(`document.querySelector(".timeline .ws-skel")`);
      await shot(`skeleton-${width}x${height}` + (theme ? "-dark" : ""), {state: "skeleton"});
      if (theme || width === 960) continue;
      // The budget runs out with the transcript still unread: the confirmed part shows with its notice.
      await host.wait(`document.querySelector(".ws-first-notice")`, 120);
      await shot(`notice-${width}x${height}`, {state: "notice"});
    }
  }
  console.log(JSON.stringify(made));
  console.error("capture exit 0");
  return 0;
});
