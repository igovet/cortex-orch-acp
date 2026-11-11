import { render } from "solid-js/web";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { draftTask, flush, projection, scriptedPort } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const TASK = `/m/${LOCAL_MACHINE_ID}/t/t`;
const withTask = (): WorkspaceState => {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push({ id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" });
  state.tasks.push(draftTask());
  return state;
};
async function open(path: string, mode: "real" | "demo" = "demo") {
  history.replaceState(null, "", path);
  const scripted = scriptedPort({ mode }), root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={scripted.port} />, root);
  await flush(); await flush();
  scripted.emit(projection("1", withTask()));
  await flush(); await flush();
}
const toggle = (label: string) => document.querySelector<HTMLButtonElement>(`.panel-t[aria-label="${label}"]`);
const app = () => document.querySelector<HTMLElement>(".ws-app")!;
const keyboard = () => document.documentElement.hasAttribute("data-keyboard");

describe("panel toggles", () => {
  it("every panel has one, and each reports the panel's real state", async () => {
    await open(TASK);
    expect(["Sidebar", "Terminal", "Inspector"].map(l => !!toggle(l))).toEqual([true, true, true]);
    expect(toggle("Sidebar")!.getAttribute("aria-pressed")).toBe("true");
    expect(app().dataset.sidebar).toBe("shown");
  });

  it("brings a collapsed panel back from the same icon", async () => {
    await open(TASK);
    for (const [label, attribute, shown, hidden] of [["Sidebar", "sidebar", "shown", "hidden"]] as const) {
      const start = toggle(label)!.getAttribute("aria-pressed");
      toggle(label)!.click(); await flush();
      expect(app().dataset[attribute]).toBe(hidden);
      expect(toggle(label)!.getAttribute("aria-pressed")).toBe(start === "true" ? "false" : "true");
      toggle(label)!.click(); await flush();
      expect(app().dataset[attribute]).toBe(shown);
      expect(toggle(label)!.getAttribute("aria-pressed")).toBe(start);
    }
  });

  it.each([TASK, `${TASK}/workers`])("has no intermediate worker pane toggle on %s", async path => {
    await open(path);
    expect(toggle("Worker pane")).toBeNull();
  });

  it("marks a panel this screen cannot offer as closed and off", async () => {
    await open("/settings/app/appearance");
    expect(toggle("Inspector")!.classList.contains("ws-hide")).toBe(true);
    expect(toggle("Inspector")!.getAttribute("aria-pressed")).toBe("false");
  });
});

describe("history shortcuts", () => {
  it("handles Alt+arrows itself rather than relying on a webview default", async () => {
    await open(TASK);
    const home = document.querySelector<HTMLAnchorElement>('a[href="/m/local"]')!;
    home.click(); await flush();
    const key = new KeyboardEvent("keydown", { key: "ArrowLeft", altKey: true, bubbles: true, cancelable: true });
    document.dispatchEvent(key);
    expect(key.defaultPrevented).toBe(true);
    await new Promise(resolve => setTimeout(resolve, 30));
    expect(location.pathname).toBe(TASK);
    const forward = new KeyboardEvent("keydown", { key: "ArrowRight", altKey: true, bubbles: true, cancelable: true });
    document.dispatchEvent(forward);
    expect(forward.defaultPrevented).toBe(true);
    await new Promise(resolve => setTimeout(resolve, 30));
    expect(location.pathname).toBe("/m/local");
  });
});

describe("focus line", () => {
  const key = (name: string) => window.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true }));
  it("is on only after keyboard movement, and any pointer press clears it", async () => {
    await open(TASK);
    expect(keyboard()).toBe(false);
    key("a"); key("Enter");
    expect(keyboard()).toBe(false);
    key("Tab");
    expect(keyboard()).toBe(true);
    window.dispatchEvent(new Event("pointerdown"));
    expect(keyboard()).toBe(false);
    key("ArrowDown");
    expect(keyboard()).toBe(true);
  });
  it("leaves nothing behind when the workspace goes", async () => {
    await open(TASK);
    key("Tab");
    dispose?.(); dispose = undefined;
    expect(keyboard()).toBe(false);
    key("Tab");
    expect(keyboard()).toBe(false);
  });
});

describe("focus and panel styles", () => {
  const source = (name: string) => readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src", name), "utf8");
  const rules = (css: string) => [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(m => ({ selector: m[1].trim(), body: m[2] }));
  const shared = source("mockup-styles.css"), own = source("workspace.css"), all = rules(shared + own);
  const token = (theme: "dark" | "light", name: string) => {
    const block = all.find(r => r.selector.endsWith(`[data-theme="${theme}"]`) && r.body.includes("--bg-canvas"))!.body;
    return new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`).exec(block)![1];
  };
  const luminance = (hex: string) => {
    const [r, g, b] = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(c => c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4);
    return .2126 * r + .7152 * g + .0722 * b;
  };
  const contrast = (a: string, b: string) => { const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p); return (x + .05) / (y + .05); };

  it("has no rule that answers a press", () => {
    expect(all.filter(r => /:active\b/.test(r.selector)).map(r => r.selector)).toEqual([]);
  });
  it("changes no surface for focus: only one quiet line, or revealing a control", () => {
    const focused = all.filter(r => /:focus|:focus-within|:focus-visible/.test(r.selector));
    expect(focused.length).toBeGreaterThan(3);
    for (const rule of focused) expect(`${rule.selector} { ${rule.body} }`).not.toMatch(/box-shadow|background|border|color:/);
    for (const rule of focused.filter(r => /outline:/.test(r.body))) expect(rule.body).toMatch(/outline: (none|var\(--focus-line\))/);
  });
  it("gives a text field only its caret", () => {
    const line = all.find(r => r.selector.includes("textarea:focus-visible"))!;
    expect(line.selector).toContain('input:not([type="checkbox"], [type="radio"]):focus-visible');
    expect(line.body).toContain("outline: none");
  });
  it("keeps the line for keyboard movement alone", () => {
    const set = all.filter(r => r.body.includes("--focus-line:"));
    expect(set.filter(r => /--focus-line:\s*none/.test(r.body)).map(r => r.selector)).toEqual(["html:not([data-keyboard])"]);
    const line = set.filter(r => /--focus-line:\s*1px solid var\(--fg-3\)/.test(r.body)).map(r => r.selector);
    // The two themes define it. The one exception gives it back to the control a dialog or a panel focused as it opened (placeFocus).
    expect(line.filter(s => s.includes("data-theme"))).toHaveLength(2);
    expect(line.filter(s => !s.includes("data-theme"))).toEqual(["html:not([data-keyboard]) [data-placed-focus]"]);
  });
  it("draws the line in a colour that stands out from every surface, in both themes", () => {
    for (const theme of ["dark", "light"] as const) for (const surface of ["bg-canvas", "bg-sidebar", "bg-raised", "bg-input", "bg-selected"]) {
      expect(contrast(token(theme, "fg-3"), token(theme, surface))).toBeGreaterThanOrEqual(3);
    }
  });
  it("leaves no native box to paint the engine's own ring", () => {
    expect(shared + own).not.toContain("accent-color");
    expect(all.find(r => r.selector.startsWith(".ck input, .chk"))!.body).toContain("appearance: none");
    expect(shared).toContain("-webkit-tap-highlight-color: transparent");
  });
  it("lets a click anywhere on a switch reach its hidden input, with or without a label", () => {
    // Real clicks are measured in focus.measure.py; this keeps the two rules that make them land.
    const body = (selector: string) => all.find(r => r.selector === selector)!.body;
    expect(body(".sw i")).toContain("pointer-events: none");
    expect(body(".sw input")).toMatch(/inset: 0;.*width: 100%;.*height: 100%;/);
  });
  it("makes an open panel's icon lighter than a closed one's in both themes, and keeps both readable", () => {
    for (const theme of ["dark", "light"] as const) {
      const on = token(theme, "panel-on"), off = token(theme, "panel-off");
      expect(luminance(on)).toBeGreaterThan(luminance(off));
      expect(contrast(on, off)).toBeGreaterThan(3);
      for (const surface of ["bg-canvas", "bg-sidebar"]) for (const icon of [on, off]) expect(contrast(icon, token(theme, surface))).toBeGreaterThanOrEqual(3);
    }
  });
  it("holds the toggle's colour through hover, with no background of its own", () => {
    const off = all.find(r => r.selector.startsWith(".panel-t, .panel-t:hover"))!, on = all.find(r => r.selector.startsWith('.panel-t[aria-pressed="true"]'))!;
    expect(off.body).toContain("background: none");
    expect(off.body).toContain("var(--panel-off)");
    expect(on.selector).toContain('.panel-t[aria-pressed="true"]:hover');
    expect(on.body).toContain("var(--panel-on)");
  });
});
