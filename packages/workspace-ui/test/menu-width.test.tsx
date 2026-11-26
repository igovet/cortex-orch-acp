// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { WsContext, type Ws } from "../src/ctx";
import { briefReason, MenuItem, Pop, REASON_HINT_MAX } from "../src/kit";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.replaceChildren(); });
const ws = { gate: () => undefined, keys: () => undefined } as unknown as Ws;
const LONG = "Close sprint is blocked by: Task A: started but not done; Task B: started but not done; Task C: started but not done.";

it("keeps a menu to a sane width in the stylesheet", () => {
  const css = readFileSync(join(process.cwd(), "src/mockup-styles.css"), "utf8");
  expect(css).toMatch(/\[popover\]\[role="menu"\]\s*\{[^}]*max-width:\s*min\(360px,\s*calc\(100vw - 16px\)\)/);
});

it("shows a short hint for a long disabled reason and keeps the whole reason in data-why", () => {
  dispose = render(() => <WsContext.Provider value={ws}><MenuItem label="Close sprint…" disabled={LONG} /></WsContext.Provider>, document.body);
  const item = document.querySelector<HTMLElement>("[role='menuitem']")!;
  expect(item.getAttribute("data-why")).toBe(LONG);
  const hint = item.querySelector(".mi-why")!.textContent!;
  expect(hint.length).toBeLessThanOrEqual(REASON_HINT_MAX);
  expect(hint.endsWith("…")).toBe(true);
  expect(briefReason("Short reason")).toBe("Short reason");
});

it("anchors a right-aligned menu under its trigger and keeps it in the window", () => {
  Object.defineProperties(window, { innerWidth: { value: 1000, configurable: true }, innerHeight: { value: 800, configurable: true } });
  dispose = render(() => <WsContext.Provider value={ws}>
    <button popovertarget="m-x" id="t">more</button>
    <Pop id="m-x" align="right"><MenuItem label="One" /></Pop>
  </WsContext.Provider>, document.body);
  const trigger = document.getElementById("t")!, pop = document.getElementById("m-x")!;
  trigger.getBoundingClientRect = () => new DOMRect(900, 40, 28, 28);
  pop.getBoundingClientRect = () => new DOMRect(572, 72, 356, 60);
  pop.dispatchEvent(Object.assign(new Event("beforetoggle"), { newState: "open" }));
  expect(pop.style.top).toBe("72px");
  expect(pop.style.right).toBe("72px");
  expect(pop.style.left).toBe("");
  expect(pop.getAttribute("role")).toBe("menu");
  pop.dispatchEvent(Object.assign(new Event("toggle"), { newState: "open" }));
  expect(pop.style.translate).toBe("");
  pop.getBoundingClientRect = () => new DOMRect(990, 72, 356, 60);
  pop.dispatchEvent(Object.assign(new Event("toggle"), { newState: "open" }));
  expect(pop.style.translate).toBe("-354px 0px");
});
