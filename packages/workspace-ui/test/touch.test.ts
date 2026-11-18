// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { dropHover, touchSelector } from "../src/touch";

describe("hover on a touch screen", () => {
  it("drops a highlight, and only the hovered half of a shared rule", () => {
    expect(touchSelector(".nav:hover", false)).toBeNull();
    expect(touchSelector(".nav:hover, .nav[aria-current=\"page\"]", false)).toBe(".nav[aria-current=\"page\"]");
    expect(touchSelector(".card", false)).toBe(".card");
    expect(touchSelector(".btn-primary:hover", true)).toBeNull();
    expect(touchSelector(".a:is(.b, .c):hover, .d", false)).toBe(".d");
  });
  it("keeps what is true without a pointer, and what a hover only revealed", () => {
    expect(touchSelector(".need[aria-current=\"true\"]:not(:hover)", false)).toBe(".need[aria-current=\"true\"]");
    expect(touchSelector(".card:hover .card-a .ib, .card:focus-within .card-a .ib", true)).toBe(".card .card-a .ib, .card:focus-within .card-a .ib");
    // The same shape that only recolours is still a highlight.
    expect(touchSelector(".card:hover .card-a .ib", false)).toBeNull();
  });
  it("rewrites the loaded style sheets, inside groups too, and later ones as they arrive", async () => {
    const style = document.createElement("style");
    style.textContent = `.row:hover { background: red; } .row:hover .act { opacity: 1; } .row { color: blue; }
      @media (max-width: 900px) { .x:hover, .x.on { color: red; } }`;
    document.head.append(style);
    const stop = dropHover(document);
    const text = () => Array.from(document.styleSheets).map(sheet => Array.from(sheet.cssRules).map(rule => rule.cssText).join(" ")).join(" ");
    expect(text()).not.toContain(":hover");
    expect(text()).toContain(".row .act");
    expect(text()).toContain(".x.on");
    expect(text()).toContain(".row {");
    const later = document.createElement("style");
    later.textContent = ".late:hover { color: red; } .late { color: blue; }";
    document.head.append(later);
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(text()).not.toContain(":hover");
    expect(text()).toContain(".late {");
    stop();
    style.remove(); later.remove();
  });
});
