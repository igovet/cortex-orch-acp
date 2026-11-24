// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ReasonTip, Whole } from "../src/layout";
import { Tooltip, tooltipPlacement } from "../src/tooltip";

let dispose: (() => void) | undefined;
beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  dispose?.();
  dispose = undefined;
  document.body.replaceChildren();
  document.documentElement.removeAttribute("data-keyboard");
  vi.useRealTimers();
});
const tooltip = () => document.querySelector<HTMLElement>('[role="tooltip"]:not([hidden])');
const hover = (el: Element, type = "pointerover", pointerType = "mouse") =>
  el.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerType }));
function fixture() {
  dispose = render(() => <>
    <button data-tooltip="Sidebar · Ctrl+B" aria-label="Sidebar" aria-describedby="existing" />
    <span id="existing">Navigation</span>
    <button data-tooltip="" aria-label="Attach files" />
    <button>Save</button>
    <div data-shell="phone"><button data-tooltip="" aria-label="Phone icon" /></div>
    <Whole text="An actually truncated name" />
    <Whole text="Short" />
    <ReasonTip />
    <Tooltip />
  </>, document.body);
  const [sidebar, attach, save, phone] = document.querySelectorAll("button");
  const [cut, fit] = document.querySelectorAll<HTMLElement>("[data-whole]");
  Object.defineProperties(cut, { scrollWidth: { value: 160 }, clientWidth: { value: 60 } });
  // jsdom does not lay out the page; supply stable boxes rather than bypassing placement.
  [sidebar, attach, save, phone, cut, fit].forEach((el, index) => {
    el.getBoundingClientRect = () => new DOMRect(24 + index * 60, 40, 24, 28);
  });
  document.querySelector("[role='tooltip']")!.getBoundingClientRect = () => new DOMRect(0, 0, 120, 24);
  return { sidebar, attach, save, phone, cut, fit };
}

it("delays hover, adds a description without replacing an existing one, and removes it on leave", () => {
  const { sidebar } = fixture();
  hover(sidebar);
  vi.advanceTimersByTime(499);
  expect(tooltip()).toBeNull();
  expect(sidebar.getAttribute("aria-describedby")).toBe("existing");
  vi.advanceTimersByTime(1);
  const tip = tooltip()!;
  expect(tip.textContent).toBe("Sidebar · Ctrl+B");
  expect(sidebar.getAttribute("aria-describedby")).toBe(`existing ${tip.id}`);
  hover(sidebar, "pointerout");
  expect(tooltip()).toBeNull();
  expect(sidebar.getAttribute("aria-describedby")).toBe("existing");
});

it("shows on keyboard focus after the same delay; Escape and blur dismiss it without moving focus themselves", () => {
  const { attach, save } = fixture();
  document.documentElement.setAttribute("data-keyboard", "");
  attach.focus();
  vi.advanceTimersByTime(499);
  expect(tooltip()).toBeNull();
  vi.advanceTimersByTime(1);
  expect(tooltip()?.textContent).toBe("Attach files");
  attach.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  vi.advanceTimersByTime(500);
  expect(tooltip()).toBeNull();
  expect(document.activeElement).toBe(attach);
  save.focus();
  attach.focus();
  vi.advanceTimersByTime(500);
  expect(tooltip()).not.toBeNull();
  attach.blur();
  expect(tooltip()).toBeNull();
});

it("keeps the visible tooltip and description when the pointer leaves a keyboard-focused trigger", () => {
  const { sidebar, attach } = fixture();
  attach.focus();
  hover(sidebar);
  vi.advanceTimersByTime(500);
  const tip = tooltip()!;
  document.documentElement.setAttribute("data-keyboard", "");
  sidebar.focus();
  hover(sidebar, "pointerout");
  expect(document.activeElement).toBe(sidebar);
  expect(tooltip()).toBe(tip);
  expect(sidebar.getAttribute("aria-describedby")).toBe(`existing ${tip.id}`);
  sidebar.blur();
  expect(tooltip()).toBeNull();
  expect(sidebar.getAttribute("aria-describedby")).toBe("existing");
});

it("describes the focused control rather than its hovered child and does not reopen on leave after Escape", () => {
  const { save } = fixture();
  const label = document.createElement("span");
  label.setAttribute("data-tooltip", "Full name");
  save.append(label);
  label.getBoundingClientRect = () => save.getBoundingClientRect();
  hover(label);
  vi.advanceTimersByTime(500);
  expect(label.getAttribute("aria-describedby")).toBe(tooltip()?.id);
  document.documentElement.setAttribute("data-keyboard", "");
  save.focus();
  vi.advanceTimersByTime(300);
  hover(label, "pointerout");
  vi.advanceTimersByTime(200);
  const tip = tooltip()!;
  expect(tip.textContent).toBe("Full name");
  expect(save.getAttribute("aria-describedby")).toBe(tip.id);
  expect(label.hasAttribute("aria-describedby")).toBe(false);
  save.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  hover(label, "pointerout");
  vi.advanceTimersByTime(500);
  expect(tooltip()).toBeNull();
  expect(save.hasAttribute("aria-describedby")).toBe(false);
  expect(document.activeElement).toBe(save);
});

it("ignores labelled buttons, touch, phone controls and text that fits; cut text keeps its press box", () => {
  const { save, phone, attach, cut, fit } = fixture();
  for (const el of [save, phone, fit]) {
    hover(el);
    vi.advanceTimersByTime(500);
    expect(tooltip()).toBeNull();
  }
  hover(attach, "pointerover", "touch");
  vi.advanceTimersByTime(500);
  expect(tooltip()).toBeNull();
  hover(cut);
  vi.advanceTimersByTime(500);
  expect(tooltip()?.textContent).toBe("An actually truncated name");
  cut.click();
  expect(tooltip()).toBeNull();
  expect(document.querySelector(".l-why-tip")?.textContent).toBe("An actually truncated name");
});

it("flips and shifts at viewport edges, avoids an adjacent control, and skips an impossible fit", () => {
  const anchor = { left: 270, right: 294, top: 164, bottom: 192 };
  const adjacent = { left: 190, right: 224, top: 125, bottom: 153 };
  const point = tooltipPlacement(anchor, 100, 28, 300, 200, [adjacent]);
  expect(tooltipPlacement(anchor, 100, 28, 300, 200, [])).toEqual({ left: 192, top: 130 });
  expect(point).toEqual({ left: 164, top: 164 });
  expect(point!.left + 100).toBeLessThan(anchor.left);
  expect(point!.top).toBeGreaterThan(adjacent.bottom);
  expect(tooltipPlacement(anchor, 100, 28, 300, 200, [{ left: 0, top: 0, right: 300, bottom: 200 }])).toBeNull();
});
