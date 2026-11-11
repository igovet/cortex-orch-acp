import { afterEach, expect, it } from "vitest";
import { createPress } from "../src/press";

let hidden = false;
Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
const flip = (to: boolean) => { hidden = to; document.dispatchEvent(new Event("visibilitychange")); };
const marked = () => document.documentElement.hasAttribute("data-hidden");
afterEach(() => { hidden = false; });

it("marks the page while the window is hidden, so the stylesheet pauses every loader, and clears the mark on dispose", () => {
  const press = createPress();
  expect(marked()).toBe(false);
  flip(true);
  expect(marked()).toBe(true);
  flip(false);
  expect(marked()).toBe(false);
  flip(true);
  press.dispose();
  expect(marked()).toBe(false);
  flip(true);
  expect(marked()).toBe(false);
});

it("holds a pressed control's loader only while its work is pending", async () => {
  const press = createPress();
  const button = document.createElement("button");
  document.body.append(button);
  let finish!: () => void;
  const work = press.track(new Promise<void>(resolve => { finish = resolve; }), button);
  expect(button.hasAttribute("data-busy")).toBe(true);
  finish(); await work; await Promise.resolve();
  expect(button.hasAttribute("data-busy")).toBe(false);
  button.remove(); press.dispose();
});
