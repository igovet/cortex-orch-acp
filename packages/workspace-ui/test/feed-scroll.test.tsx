import { createRoot, createSignal } from "solid-js";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { createFollow } from "../src/kit";
import { flush } from "./support";
import { render } from "solid-js/web";
import { Pill } from "../src/worker";

it.each(["task conversation", "sprint planning", "worker activity"])("%s shares follow, pause, resume and missed counts", async () => {
  const box = document.createElement("div"), inner = document.createElement("div"), controls = document.createElement("div");
  box.append(inner); document.body.append(box, controls);
  let height = 1000, top = 0, resize!: () => void;
  Object.defineProperties(box, { clientHeight: { get: () => 400 }, scrollHeight: { get: () => height }, scrollTop: { get: () => top, set: v => { top = v; } } });
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { resize = cb; } observe() {} disconnect() {} });
  const [count, setCount] = createSignal(1);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, count); });
  const unmount = render(() => <Pill follow={follow} />, controls);
  try {
    await flush();
    expect(top).toBe(600);
    height += 100; setCount(2); resize(); expect(top).toBe(700);
    height += 80; resize(); expect(top).toBe(780);
    // Layout can move the scroll position before ResizeObserver fires.
    height += 100; top = 750; follow.onScroll();
    expect(follow.following()).toBe(true);
    resize(); expect(top).toBe(880);
    // Input arrives before the queued resize, during streaming.
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
    top = 500; height += 100; resize(); follow.onScroll();
    expect(top).toBe(500); expect(follow.following()).toBe(false);
    setCount(3); await flush(); height += 100; resize();
    expect(top).toBe(500); expect(controls.textContent).toContain("1 new");
    height += 50; resize(); expect(top).toBe(500);
    top = height - 400; follow.onScroll(); await flush();
    expect(follow.following()).toBe(true); expect(controls.querySelector("button")).toBeNull();
    height += 50; resize(); expect(top).toBe(height - 400);
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp" }));
    top -= 200; follow.onScroll(); await flush();
    expect(controls.textContent).toContain("Latest");
    controls.querySelector("button")!.click(); await flush();
    expect(top).toBe(height - 400); expect(follow.following()).toBe(true);
    height += 50; resize(); expect(top).toBe(height - 400);
  } finally { unmount(); }
});

let dispose: (()=>void)|undefined;
afterEach(()=>{dispose?.(); document.body.innerHTML=""; vi.unstubAllGlobals();});
it("pins streaming growth, preserves the reading anchor, and never writes during a scrollbar drag",async()=>{
  const box=document.createElement("div"), inner=document.createElement("div"), row=document.createElement("div"); inner.append(row);box.append(inner);document.body.append(box);
  let height=1000, top=600, rowY=0, resize!: ()=>void;
  Object.defineProperties(box,{clientHeight:{get:()=>400},scrollHeight:{get:()=>height},scrollTop:{get:()=>top,set:(v:number)=>{top=v;}}});
  box.getBoundingClientRect=()=>({top:0,bottom:400}) as DOMRect;
  row.getBoundingClientRect=()=>({top:rowY,bottom:rowY+100}) as DOMRect;
  vi.stubGlobal("ResizeObserver",class {constructor(cb:()=>void){resize=cb;}observe(){}disconnect(){}});
  const [n,setN]=createSignal(1);
  const follow=createRoot(d=>{dispose=d;return createFollow(()=>box,n);});
  await flush();
  height=1100; resize();
  expect(top).toBe(700);
  box.dispatchEvent(new WheelEvent("wheel",{deltaY:-100}));top=200;follow.onScroll();expect(follow.following()).toBe(false);
  height=1200;setN(2);resize();expect(top).toBe(200);
  rowY=50;resize();expect(top).toBe(250);
  box.dispatchEvent(new MouseEvent("pointerdown",{bubbles:true}));
  height=1300;rowY=150;resize();expect(top).toBe(250);
  top=900;follow.onScroll();document.dispatchEvent(new MouseEvent("pointerup",{bubbles:true}));
  expect(follow.following()).toBe(true);
  height=1400;resize();expect(top).toBe(1000);
});

// Rendered in WebKitGTK: a background read replaced rows inside the result card at the end, and the engine moved scrollTop up by
// itself (16 px, no height change, no script write). Taken as the reader scrolling up, following stopped and Latest stayed.
it("pins the end again when the engine moves the scroll position without reader input, and leaves it only for the reader", async () => {
  const box = document.createElement("div"), inner = document.createElement("div"); box.append(inner); document.body.append(box);
  let height = 2000, top = 0, resize!: () => void, now = 10_000;
  Object.defineProperties(box, { clientHeight: { get: () => 500 }, scrollHeight: { get: () => height }, scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, height - 500)); } } });
  box.getBoundingClientRect = () => ({ top: 0, bottom: 500 }) as DOMRect;
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { resize = cb; } observe() {} disconnect() {} });
  vi.spyOn(performance, "now").mockImplementation(() => now);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  expect(top).toBe(1500);
  // The engine's own move: same height, no input.
  top = 1484; follow.onScroll();
  expect(top).toBe(1500); expect(follow.following()).toBe(true);
  // Content that grew and shrank in one update, clamping scrollTop first.
  top = 1392; height = 2028; follow.onScroll();
  expect(top).toBe(1528); expect(follow.following()).toBe(true);
  resize(); expect(top).toBe(1528);
  // A smooth wheel scroll up whose first step still lies within the end follows again, then leaves it as the reader goes on.
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
  top = 1527; follow.onScroll(); expect(follow.following()).toBe(true);
  now += 40; top = 1460; follow.onScroll(); expect(follow.following()).toBe(false);
  // Reaching the end by any means follows again; a later engine move keeps it there.
  now += 5000; top = 1528; follow.onScroll(); expect(follow.following()).toBe(true);
  top = 1500; follow.onScroll(); expect(top).toBe(1528); expect(follow.following()).toBe(true);
  // Focus moved by the keyboard onto a control above is the reader's move.
  inner.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
  top = 900; follow.onScroll(); expect(top).toBe(900); expect(follow.following()).toBe(false);
  // Back at the end, following again.
  now += 5000; top = 1528; follow.onScroll(); expect(follow.following()).toBe(true);
});

// The pill's band showed in the dock, whose height is the feed's bottom clearance: each time Latest came or went the end moved
// by the band's height, so the reader who had just reached it was no longer there.
it("lays the follow pill's band over the feed above the dock, out of the dock's flow and clear of the feed's scrollbar", () => {
  const css = readFileSync(join(__dirname, "..", "src", "workspace.css"), "utf8");
  expect(css).toMatch(/\.dock > \.pill-band, \.dock-w > \.pill-band \{[^}]*position: absolute;[^}]*bottom: 100%;[^}]*right: calc\(11px \+ var\(--ws-edge\)\)/);
});

// The dock lay over the feed's bottom part, with its measured height as the feed's bottom padding: the feed's scrollbar ran on
// beside the composer. The dock is now the feed's flow sibling below it, so the feed's box and scrollbar end at the dock's top.
it("puts each dock below its feed in the panel's column, with no measured clearance", () => {
  const read = (name: string) => readFileSync(join(__dirname, "..", "src", name), "utf8");
  const css = read("mockup-styles.css") + read("workspace.css");
  for (const dock of ["dock", "dock-w"]) {
    const rule = css.match(new RegExp(`\\n\\.${dock} \\{([^}]*)\\}`))![1];
    expect(rule).toContain("flex: none"); expect(rule).not.toMatch(/position: absolute|bottom: 0/);
  }
  expect(css).not.toMatch(/dock-clear|\.dock(-w)?::before/);
  for (const file of ["session.tsx", "sprint.tsx", "worker.tsx", "composer.tsx", "follow.ts"]) expect(read(file)).not.toMatch(/dock-clear|reportHeight|MutationObserver/);
});

it("keeps the end, or the reader's row, when the dock below resizes the feed", async () => {
  const box = document.createElement("div"), inner = document.createElement("div"), row = document.createElement("div"); inner.append(row); box.append(inner); document.body.append(box);
  let client = 500, top = 0, resize!: () => void;
  Object.defineProperties(box, { clientHeight: { get: () => client }, scrollHeight: { get: () => 2000 }, scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, 2000 - client)); } } });
  box.getBoundingClientRect = () => ({ top: 0, bottom: client }) as DOMRect;
  row.getBoundingClientRect = () => ({ top: 10, bottom: 110 }) as DOMRect;
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { resize = cb; } observe() {} disconnect() {} });
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  expect(top).toBe(1500);
  // The composer grows a line, the workers bar appears, a queued row comes: the feed is shorter and its end stays in view.
  client = 420; resize(); expect(top).toBe(1580); expect(follow.following()).toBe(true);
  // The dock shrinks back: the engine clamps scrollTop, which is no reader input, and the end stays pinned.
  client = 500; top = 1500; follow.onScroll(); resize(); expect(top).toBe(1500); expect(follow.following()).toBe(true);
  // A reader who scrolled up keeps their row where it was whichever way the dock resizes.
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 })); top = 600; follow.onScroll();
  client = 380; resize(); expect(top).toBe(600); client = 520; resize(); expect(top).toBe(600); expect(follow.following()).toBe(false);
});
