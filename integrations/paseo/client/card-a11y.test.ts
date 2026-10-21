import assert from "node:assert/strict";
import test from "node:test";
import { contrastRatio, MIN_TEXT_CONTRAST, parseColor, readableTextColor } from "./card-contrast.ts";
import { createActivationGuard, createSpaceActivation, isSpaceActivationKey, type CardKeyEvent } from "./card-keyboard.ts";

const ratio = (a: string, b: string) => contrastRatio(parseColor(a)!, parseColor(b)!);

test("the reported light hover pair is below 4.5:1 and the card's derived colour is not", () => {
  // Observed in the installed light theme: foregroundMuted on surface2.
  const muted = "rgb(113,113,122)";
  assert.ok(Math.abs(ratio(muted, "rgb(244,244,245)") - 4.3969) < 0.001);
  const backgrounds = ["#ffffff", "#fafafa", "rgb(244,244,245)"];
  const readable = readableTextColor(muted, "rgb(9,9,11)", backgrounds);
  assert.notEqual(readable, muted);
  for (const background of backgrounds) assert.ok(ratio(readable, background) >= MIN_TEXT_CONTRAST, `${readable} on ${background}`);
  // The smallest sufficient step: still visibly secondary, not the full foreground.
  assert.ok(ratio(readable, "rgb(244,244,245)") < 6, "keeps the muted hierarchy");
});

test("a theme whose muted text already passes keeps it unchanged, in light and dark", () => {
  assert.equal(readableTextColor("#52525b", "#09090b", ["#ffffff", "#f4f4f5"]), "#52525b");
  // Dark: light muted text on dark surfaces, including the lighter hover surface.
  assert.equal(readableTextColor("#a1a1aa", "#fafafa", ["#09090b", "#18181b", "#27272a"]), "#a1a1aa");
  const darkLow = readableTextColor("#71717a", "#fafafa", ["#18181b", "#27272a"]);
  for (const background of ["#18181b", "#27272a"]) assert.ok(ratio(darkLow, background) >= MIN_TEXT_CONTRAST, darkLow);
});

test("an unparseable token never counts as readable", () => {
  assert.equal(readableTextColor("var(--muted)", "#09090b", ["#ffffff"]), "#09090b");
  assert.equal(readableTextColor("#71717a", "#09090b", ["hsl(0 0% 96%)"]), "#09090b");
});

// Token values read from the installed Paseo 0.12.0 bundle's light and
// default dark themes (surface0/1/2, foreground, foregroundMuted, accent).
const INSTALLED = {
  light: { surfaces: ["#ffffff", "#fafafa", "#f4f4f5"], foreground: "#1a1a1e", muted: "#71717a", accent: "#20744A" },
  dark: { surfaces: ["#181B1A", "#1E2120", "#272A29"], foreground: "#fafafa", muted: "#A1A5A4", accent: "#20744A" },
};

test("every card text colour reaches 4.5:1 on normal, hover, focus and pressed backgrounds in both installed themes", () => {
  for (const [name, theme] of Object.entries(INSTALLED)) {
    for (const token of [theme.muted, theme.accent]) {
      const color = readableTextColor(token, theme.foreground, theme.surfaces);
      for (const surface of theme.surfaces) {
        assert.ok(ratio(color, surface) >= MIN_TEXT_CONTRAST, `${name}: ${token} -> ${color} on ${surface}`);
      }
    }
  }
  // Light accent already passes and stays the host's own token; the dark
  // accent (about 2.5:1) is lifted toward the foreground.
  assert.equal(readableTextColor(INSTALLED.light.accent, INSTALLED.light.foreground, INSTALLED.light.surfaces), INSTALLED.light.accent);
  assert.notEqual(readableTextColor(INSTALLED.dark.accent, INSTALLED.dark.foreground, INSTALLED.dark.surfaces), INSTALLED.dark.accent);
  assert.equal(readableTextColor(INSTALLED.dark.muted, INSTALLED.dark.foreground, INSTALLED.dark.surfaces), INSTALLED.dark.muted);
  assert.equal(parseColor("#abc")?.r, 0xaa);
  assert.deepEqual(parseColor("rgba(1, 2, 3, 0.5)"), { r: 1, g: 2, b: 3 });
});

function key(key: string, target: unknown, extra: Partial<CardKeyEvent> = {}) {
  const event = { key, target, prevented: false, stopped: false,
    preventDefault() { event.prevented = true; }, stopPropagation() { event.stopped = true; }, ...extra };
  return event;
}

test("Space on the focused card opens it exactly once, on release, without scrolling", () => {
  const card = {};
  let opened = 0;
  const space = createSpaceActivation({ isCard: (target) => target === card, activate: () => { opened += 1; } });
  const down = key(" ", card);
  space.keydown(down);
  assert.equal(down.prevented, true, "keydown claims Space: no page scroll");
  assert.equal(opened, 0, "a button activates on release");
  space.keydown(key(" ", card, { repeat: true }));
  const up = key(" ", card);
  space.keyup(up);
  assert.equal(opened, 1);
  assert.equal(up.prevented, true, "no native button click follows");
  assert.equal(up.stopped, true, "no other key handler activates it again");
  space.keyup(key(" ", card));
  assert.equal(opened, 1, "a stray release never re-opens");
});

test("Space elsewhere, with a modifier, after focus left, or other keys does nothing", () => {
  const card = {};
  let opened = 0;
  const space = createSpaceActivation({ isCard: (target) => target === card, activate: () => { opened += 1; } });
  const elsewhere = key(" ", {});
  space.keydown(elsewhere);
  space.keyup(key(" ", {}));
  assert.equal(elsewhere.prevented, false, "the composer and other controls keep Space");
  space.keydown(key(" ", card, { ctrlKey: true }));
  space.keyup(key(" ", card, { ctrlKey: true }));
  space.keydown(key(" ", card));
  space.cancel();
  space.keyup(key(" ", card));
  const enter = key("Enter", card);
  space.keydown(enter);
  space.keyup(key("Enter", card));
  assert.equal(enter.prevented, false, "Enter keeps the native button activation");
  assert.equal(opened, 0);
  assert.equal(isSpaceActivationKey({ key: "Spacebar" }), true);
});

test("Enter, click and Space share one activation guard: one gesture never navigates twice", () => {
  let now = 1000;
  let opened = 0;
  const activate = createActivationGuard(() => { opened += 1; }, 400, () => now);
  assert.equal(activate(), true);
  assert.equal(activate(), false, "a second path in the same gesture is absorbed");
  now += 401;
  assert.equal(activate(), true, "a later deliberate activation opens again");
  assert.equal(opened, 2);
});
