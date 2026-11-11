import { afterEach, expect, it } from "vitest";
import { keepInside } from "../src/kit";

const width0 = window.innerWidth, height0 = window.innerHeight;
const view = (w: number, h: number) => {
  Object.defineProperty(window, "innerWidth", { value: w, configurable: true, writable: true });
  Object.defineProperty(window, "innerHeight", { value: h, configurable: true, writable: true });
};
afterEach(() => { view(width0, height0); document.body.innerHTML = ""; });
/** A popover whose box (as the engine would lay it out before any correction) is the given rectangle. */
function pop(left: number, top: number, width: number, height: number) {
  const el = document.createElement("div");
  document.body.append(el);
  el.getBoundingClientRect = () => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) });
  return el;
}

it("moves a menu opened against the right edge back inside the window, as the graph's Layers menu was", () => {
  view(1088, 760);
  const el = pop(1025, 170, 240, 420);
  keepInside(el);
  expect(el.style.translate).toBe(`${1088 - 8 - 240 - 1025}px 0px`);
});

it("leaves a menu that fits alone, and clears an earlier correction when it fits again", () => {
  view(1280, 800);
  const el = pop(100, 100, 240, 200);
  el.style.translate = "-50px 0px";
  keepInside(el);
  expect(el.style.translate).toBe("");
});

it("lifts a menu that runs under the bottom edge and lets one taller than the window scroll inside it", () => {
  view(390, 600);
  const low = pop(20, 500, 200, 160);
  keepInside(low);
  expect(low.style.translate).toBe(`0px ${600 - 8 - 160 - 500}px`);
  const tall = pop(0, 40, 300, 900);
  keepInside(tall);
  expect(tall.style.maxHeight).toBe("584px");
  expect(tall.style.overflowY).toBe("auto");
  expect(tall.style.translate).toBe("8px -32px");
});
