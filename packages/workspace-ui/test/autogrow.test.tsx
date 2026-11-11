import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRoot, createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import { WsContext, type Ws } from "../src/ctx";
import { autoGrow } from "../src/autogrow";
import { createAttachments, createDictation, MessageComposer } from "../src/composer";

function Answer(props: { onSend: (text: string, files: unknown[]) => void }) {
  const [text, setText] = createSignal("");
  const files = createAttachments(), mic = createDictation();
  return <MessageComposer label="Your answer" placeholder="Your answer" text={text()} onText={setText} files={files} mic={mic}
    send={{ label: "Send answer", why: text().trim() ? undefined : "Write an answer first", go: () => props.onSend(text(), files.files()) }} />;
}

// jsdom has no layout, so this stands in for the box a browser would compute. It proves the logic, not the rendered result: layout-fit.py has that.
const FLOOR = 46, CAP = 200, LINE = 22;
function laidOut(el: HTMLTextAreaElement) {
  const state = { hidden: false, width: 600, measured: () => {} };
  const box = () => state.hidden ? 0 : Math.min(el.style.height ? parseFloat(el.style.height) : FLOOR, CAP);
  Object.defineProperties(el, {
    offsetHeight: { get: box }, clientHeight: { get: box }, offsetWidth: { get: () => state.hidden ? 0 : state.width },
    scrollHeight: { get: () => { state.measured(); return state.hidden ? 0 : Math.max(el.value.split("\n").length * LINE, box()); } },
  });
  el.getClientRects = () => (state.hidden ? [] : [{}]) as unknown as DOMRectList;
  return state;
}
const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i}`).join("\n");

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.unstubAllGlobals(); });

function mount(initial = "") {
  const [text, setText] = createSignal(initial);
  const holder = document.body.appendChild(document.createElement("div"));
  const el = holder.appendChild(document.createElement("textarea"));
  const state = laidOut(el);
  // The value reaches the field first, as Solid's render effect does before a user effect.
  const set = (value: string) => { el.value = value; setText(value); };
  el.value = initial;
  createRoot(d => { dispose = d; autoGrow(el, text); });
  return { el, holder, set, state };
}
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
const observed = () => { let run = () => {}; vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { run = cb; } observe() {} disconnect() {} }); vi.stubGlobal("requestAnimationFrame", (cb: () => void) => { cb(); return 1; }); return () => run(); };

describe("autoGrow", () => {
  it("keeps the stylesheet's height while the text fits it", async () => {
    const { el } = mount();
    await tick();
    expect(el.style.height).toBe(`${FLOOR}px`);
    expect(el.style.overflowY).toBe("hidden");
  });

  it("grows with the text and shrinks back when it is deleted", async () => {
    const { el, set } = mount();
    await tick();
    set(lines(5));
    expect(el.style.height).toBe(`${5 * LINE}px`);
    expect(el.style.overflowY).toBe("hidden");
    set(lines(3));
    expect(el.style.height).toBe(`${3 * LINE}px`);
    set("");
    expect(el.style.height).toBe(`${FLOOR}px`);
  });

  it("scrolls inside only past the cap", async () => {
    const { el, set } = mount();
    await tick();
    set(lines(9));
    expect(el.offsetHeight).toBe(9 * LINE);
    expect(el.style.overflowY).toBe("hidden");
    set(lines(40));
    expect(el.offsetHeight).toBe(CAP);
    expect(el.style.overflowY).toBe("auto");
    set(lines(9));
    expect(el.style.overflowY).toBe("hidden");
  });

  it("fits a value set without typing: a restored draft, then a cleared one", async () => {
    const { el, set } = mount(lines(6));
    await tick();
    expect(el.style.height).toBe(`${6 * LINE}px`);
    set("");
    expect(el.style.height).toBe(`${FLOOR}px`);
  });

  it("leaves a hidden field alone and fits it once shown", async () => {
    const run = observed();
    const { el, set, state } = mount(lines(4));
    await tick();
    state.hidden = true; run();
    set(lines(7));
    expect(el.style.height).toBe(`${4 * LINE}px`);
    state.hidden = false; run();
    expect(el.style.height).toBe(`${7 * LINE}px`);
  });

  it("refits for a new width, and not for its own height change", async () => {
    const run = observed();
    const { el, state } = mount(lines(4));
    await tick();
    const measure = vi.spyOn(el, "getClientRects");
    run();
    expect(measure).not.toHaveBeenCalled();
    state.width = 300; run();
    expect(measure).toHaveBeenCalled();
  });

  it("puts a scrolled page back after measuring, and holds nothing around the field", async () => {
    const { holder, set, state } = mount();
    await tick();
    let top = 40, held: string | undefined;
    Object.defineProperty(holder, "scrollTop", { get: () => top, set: (v: number) => { top = v; } });
    // The folded field shortens the page, which pulls its scroll back. A held parent would be a grid's stretch: the rendered gate measures that.
    state.measured = () => { top = 0; held = holder.style.minHeight; };
    set(lines(5));
    expect(held).toBe("");
    expect(top).toBe(40);
    expect(holder.style.minHeight).toBe("");
  });
});

describe("the fields that grow", () => {
  it("goes through the one helper in every creation and message form", () => {
    // Left out on purpose: the Git pane's commit box, a settings field and a one-line command paste.
    const fixed = new Set(["git.tsx", "orch-models.tsx", "orch-mcp.tsx"]);
    const src = join(dirname(fileURLToPath(import.meta.url)), "../src");
    const fields = readdirSync(src).filter(name => name.endsWith(".tsx")).flatMap(name => [...readFileSync(join(src, name), "utf8").matchAll(/<textarea\b[\s\S]*?\/>/g)].map(m => ({ name, tag: m[0] })));
    // Every message draft now draws the one shared composer, whose single writing area grows through the helper.
    expect(fields.length).toBeGreaterThan(5);
    expect(fields.filter(f => !f.tag.includes("autoGrow(el,") && !fixed.has(f.name)).map(f => `${f.name}: ${f.tag.slice(0, 60)}`)).toEqual([]);
    const users = readdirSync(src).filter(name => name.endsWith(".tsx") && /<MessageComposer\b/.test(readFileSync(join(src, name), "utf8")));
    expect(users.sort()).toEqual(["composer.tsx", "dialogs.tsx", "diff.tsx", "newtask.tsx", "questions.tsx", "sprint.tsx", "task-edit.tsx", "worker.tsx"]);
  });

  it("applies to the shared composer: Shift+Enter breaks the line, Enter sends", async () => {
    const send = vi.fn(), root = document.body.appendChild(document.createElement("div"));
    const ws = { can: () => false, t: () => "", store: {} } as unknown as Ws;
    dispose = render(() => <WsContext.Provider value={ws}><Answer onSend={send} /></WsContext.Provider>, root);
    const box = document.querySelector<HTMLTextAreaElement>("textarea")!;
    expect(box.dataset.grow).toBe("");
    box.value = "two\nlines"; box.dispatchEvent(new Event("input", { bubbles: true }));
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true }));
    expect(send).not.toHaveBeenCalled();
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(send).toHaveBeenCalledWith("two\nlines", []);
  });
});
