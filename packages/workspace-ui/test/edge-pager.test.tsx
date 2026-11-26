import { createRoot, createSignal, For } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { createRevealer, edgeReach, nearEdge } from "../src/edge-pager";
import { createFollow } from "../src/follow";
import { createRowSlots } from "../src/row-slots";
import { flush } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

/** A 400 px feed whose children are laid out in normal flow, each as tall as `heights` says (a reserve as its style says). */
function feed(heights: (row: HTMLElement) => number) {
  const box = document.createElement("div"), inner = document.createElement("div");
  box.append(inner); document.body.append(box);
  let top = 0;
  const height = () => [...inner.children].reduce((n, row) => n + heights(row as HTMLElement), 0);
  Object.defineProperties(box, { clientHeight: { get: () => 400 }, scrollHeight: { get: () => Math.max(400, height()) },
    scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, height() - 400)); } } });
  box.getBoundingClientRect = () => ({ top: 0, bottom: 400, height: 400 }) as DOMRect;
  const place = (row: Element) => { let y = -top; for (const r of inner.children) { if (r === row) break; y += heights(r as HTMLElement); } return y; };
  const patch = (row: HTMLElement) => { row.getBoundingClientRect = () => { const y = place(row); return { top: y, bottom: y + heights(row), height: heights(row) } as DOMRect; }; return row; };
  return { box, inner, patch, top: () => top, set: (v: number) => { top = v; } };
}
const row = (text: string, h = 50) => { const r = document.createElement("div"); r.textContent = text; r.dataset.h = String(h); return r; };
const sized = (r: HTMLElement) => r.classList.contains("ws-reserve") ? parseFloat(r.style.height) || 0 : Number(r.dataset.h ?? 50);

it("reads the earlier part only within one viewport of the feed's top edge, never from the middle", () => {
  const f = feed(sized);
  const edge = f.patch(row("edge", 30));
  f.inner.append(edge);
  for (let i = 0; i < 200; i++) f.inner.append(f.patch(row(`row ${i}`)));
  expect(edgeReach(400)).toBe(400);
  // The edge line's bottom is 30 px into the content: more than one viewport above, the reader is in the middle.
  f.set(30 + 401); expect(nearEdge(f.box, edge)).toBe(false);
  f.set(30 + 3000); expect(nearEdge(f.box, edge)).toBe(false);
  // Within one viewport of it, the read starts.
  f.set(30 + 399); expect(nearEdge(f.box, edge)).toBe(true);
  f.set(0); expect(nearEdge(f.box, edge)).toBe(true);
});

it("gives rows put above the reader the reserve's room: the row on screen stays still and the position is not written", async () => {
  const f = feed(sized);
  const reserve = f.patch(document.createElement("div"));
  reserve.className = "ws-reserve"; reserve.dataset.followSkip = "";
  const edge = f.patch(row("edge", 30)); edge.dataset.followSkip = "";
  f.inner.append(reserve, edge);
  for (let i = 0; i < 100; i++) f.inner.append(f.patch(row(`row ${i}`)));
  let keep = true;
  const follow = createRoot(d => { dispose = d; return createFollow(() => f.box, () => 1, undefined, undefined, { reserve: () => reserve, keep: () => keep }); });
  await flush();
  // Opened at the end: the reserve took its size unseen, at least eight viewports and four times the rows held (a page).
  const size = parseFloat(reserve.style.height);
  expect(size).toBe(4 * (30 + 100 * 50));
  // The reader scrolls up to row 10.
  f.box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
  f.set(size + 30 + 500); f.box.dispatchEvent(new Event("scroll"));
  const seen = f.inner.children[12] as HTMLElement, y = seen.getBoundingClientRect().top, at = f.top();
  // A page of 20 rows (1,000 px) goes in above, just below the edge line.
  const page = Array.from({ length: 20 }, (_, i) => f.patch(row(`earlier ${i}`)));
  edge.after(...page);
  follow.sync();
  expect(seen.getBoundingClientRect().top).toBe(y);
  expect(f.top()).toBe(at);
  expect(reserve.style.height).toBe(`${size - 1000}px`);
  expect(follow.room()).toBe(true);
  // A page taller than what is left: with no wheel animation running, the reserve grows in one step and the row stays.
  const big = Array.from({ length: (size - 1000) / 50 + 20 }, (_, i) => f.patch(row(`older ${i}`)));
  await new Promise(resolve => setTimeout(resolve, 450));
  edge.after(...big);
  follow.sync();
  expect(seen.getBoundingClientRect().top).toBe(y);
  // Nothing more can come: once the reader holds still the reserve goes, unseen.
  keep = false;
  follow.sync();
  await new Promise(resolve => setTimeout(resolve, 400));
  expect(reserve.style.height).toBe("0px");
  expect(seen.getBoundingClientRect().top).toBe(y);
});

it("keeps every drawn row's computations when a page is put above: only the new rows are evaluated", () => {
  type Entry = { id: string; text: string };
  const made = (n: number, from = 0) => Array.from({ length: n }, (_, i) => ({ id: `e${from + i}`, text: `Entry ${from + i}` }));
  const [entries, setEntries] = createSignal<Entry[]>(made(50, 100));
  let evaluated = 0;
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => {
    const slot = createRowSlots(() => new Map(entries().map(e => [e.id, e])));
    return <For each={entries().map(e => e.id)}>{id => { const entry = slot(id); return <p>{(evaluated++, entry()?.text)}</p>; }}</For>;
  }, root);
  const first = root.querySelector("p")!;
  expect(evaluated).toBe(50);
  // A new page of 30 entries above; the 50 held entries are the same records, as the store keeps them (stable.ts).
  setEntries([...made(30, 70), ...entries()]);
  expect(evaluated).toBe(80);
  expect(root.querySelectorAll("p")[30]).toBe(first);
  // A changed record wakes its own row alone.
  setEntries(entries().map(e => e.id === "e149" ? { ...e, text: "Changed" } : e));
  expect(evaluated).toBe(81);
  expect(root.lastElementChild!.textContent).toBe("Changed");
});

it("draws held rows in steps, and waits while the reserve has no room", async () => {
  let hidden = 40, room = true;
  const steps: number[] = [];
  const kick = createRoot(d => { dispose = d; return createRevealer({ hidden: () => hidden, room: () => room, reveal: n => { steps.push(n); hidden -= n; } }); });
  room = false; kick(); await flush();
  expect(steps).toEqual([]);
  room = true;
  await new Promise(resolve => setTimeout(resolve, 200));
  expect(hidden).toBe(0);
  expect(steps.every(n => n >= 8)).toBe(true);
});
