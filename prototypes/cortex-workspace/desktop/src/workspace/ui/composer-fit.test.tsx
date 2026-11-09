import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import { WORKSPACE_CASES } from "../adapter/fixtures";
import { WORKSPACE_SESSION_WORKERS as W } from "../adapter/session-fixture";
import Workspace from "./index";

// Vitest empties CSS imports, so the stylesheets are read from disk. The specifier stays a variable because the project has no Node types.
const fs = "node:fs";
const { readFileSync } = await import(/* @vite-ignore */ fs);
const read = (path: string): string => readFileSync(new URL(path, import.meta.url).pathname.replace("/@fs", ""), "utf8");
const css = read("../../../../../../packages/workspace-ui/src/mockup-styles.css");

// jsdom has no layout, so this pins the structure and the rules the layout rests on.
// The real measurement, in a browser, is composer-fit.measure.mjs.
const LOCAL = "mock-machine-00000";
const task = (id: string) => `/m/${LOCAL}/t/${id}`;
const SESSION = task(WORKSPACE_CASES.session);

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
async function mount(path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace autoplay={false} />, root);
  await new Promise(r => setTimeout(r, 0));
}
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const bars = () => all(".composer > .c-bar");

describe("composer bar", () => {
  const screens: [string, string][] = [
    ["a running task", SESSION], ["a result in review", task(WORKSPACE_CASES.review)], ["a paused task", task(WORKSPACE_CASES.paused)],
    ["a closed task", task(WORKSPACE_CASES.closedAsIs)], ["a new task", `/m/${LOCAL}/new`], ["a worker", `${SESSION}/workers/${W.backend}/live`],
  ];
  it.each(screens)("keeps every right-hand control in the group that never gives up width, on %s", async (_, path) => {
    await mount(path);
    expect(bars().length).toBeGreaterThan(0);
    for (const bar of bars()) {
      const end = bar.querySelector(":scope > .c-end")!;
      expect(end, "right-hand group").toBeTruthy();
      expect(end.querySelector('[aria-label="Voice input"]'), "mic").toBeTruthy();
      expect(bar.querySelector(":scope > .grow"), "no spacer left to squeeze").toBeNull();
      for (const sel of bar.querySelectorAll(".sel")) expect(sel.closest(".c-route"), "selector sits in the shrinking half").toBeTruthy();
    }
  });

  it("orders the selectors by what stays longest, and an icon-only selector still names its value", async () => {
    await mount(SESSION);
    const sels = all(".dock .composer .c-route > .sel");
    expect(sels.map(s => s.getAttribute("aria-label")!.split(":")[0])).toEqual(["Coordinator model", "Reasoning effort", "Access mode", "Message language"]);
    for (const sel of sels) {
      const label = sel.querySelector(".sel-t")!.textContent!;
      expect(sel.getAttribute("aria-label")).toContain(label);
      expect(sel.title).toContain(label);
    }
  });

  it("collapses language first, then mode, effort and model, and lets nothing clip the bar", () => {
    const widths = ["nth-of-type(4)", "nth-of-type(3)", "nth-of-type(2):not(:last-of-type)", "first-of-type:not(:only-of-type):not(:nth-last-of-type(2))"]
      .map(sel => { const at = css.indexOf(`.sel:${sel} { --sel-min`), rule = css.lastIndexOf("@container c-route (max-width: ", at); return parseFloat(css.slice(rule + 31)); });
    expect(widths.every(Number.isFinite)).toBe(true);
    expect(widths).toEqual([...widths].sort((a, b) => b - a));
    const rule = (name: string) => new RegExp(`^${name} \\{(.*)\\}$`, "m").exec(css)![1];
    expect(rule("\\.c-bar")).toContain("flex-wrap: wrap");
    expect(rule("\\.c-bar")).not.toMatch(/(^|[^-])height:|overflow/);
    expect(rule("\\.c-end")).toContain("max-width: 100%");
    expect(rule("\\.c-end")).not.toContain("overflow");
    expect(rule("\\.c-route")).toContain("min-width: 0");
  });

  it("keeps the app's copy of the mockup stylesheet identical", () => {
    expect(css === read("../../../../mockups/styles.css")).toBe(true);
  });
});

describe("figures with no data", () => {
  const placeholder = /(^|\s)[—–-](\s|$)|unavailable/i;
  it.each([["closed", WORKSPACE_CASES.closedAsIs], ["paused", WORKSPACE_CASES.paused], ["stopping", WORKSPACE_CASES.stopping], ["offline", WORKSPACE_CASES.offline]])(
    "a %s task with no context or usage shows neither a meter nor a placeholder", async (_, id) => {
      await mount(task(id));
      expect(document.querySelector(".composer .ctx"), "composer figures").toBeNull();
      expect(document.querySelector(".composer .ring"), "context ring").toBeNull();
      expect(document.querySelector(".planbar .meter"), "plan bar meter").toBeNull();
      expect(document.querySelector(".planbar")!.textContent).not.toMatch(/Context|Usage/);
      for (const el of all(".composer .c-end, .planbar")) expect(el.textContent).not.toMatch(placeholder);
    });

  it("shows the meter and figures as soon as there is data", async () => {
    await mount(SESSION);
    expect(document.querySelector(".composer .ring")).toBeTruthy();
    expect(document.querySelector(".composer .ctx-t")!.textContent).toBe("41%");
    expect(document.querySelector(".composer .ctx-n")!.textContent).toBe("182k");
    expect(document.querySelector(".planbar .meter")).toBeTruthy();
  });

  it("leaves unreported figures out of the inspector and the worker details", async () => {
    await mount(task(WORKSPACE_CASES.closedAsIs));
    const inspector = document.querySelector(".inspector")!.textContent!;
    expect(inspector).not.toMatch(/Unavailable|Task tokens/);
    dispose?.(); document.body.innerHTML = "";
    await mount(`${SESSION}/workers/${W.migration}`);
    expect(document.querySelector(".detail .d-grid")!.textContent).not.toMatch(/Unavailable|Context|Usage/);
  });
});
