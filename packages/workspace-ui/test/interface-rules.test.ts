import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { HELP } from "../src/orch-help";

/** The rules a source file can answer on its own. What needs layout is in sweep.measure.py. */
const files = (dir: string): string[] => readdirSync(dir).flatMap(name => { const path = join(dir, name); return statSync(path).isDirectory() ? files(path) : /\.tsx?$/.test(name) ? [path] : []; });
// This package's sources, from the package or the repository root, where a bare "src" is another tree.
const SRC = ["src", join("packages", "workspace-ui", "src")].find(dir => existsSync(join(dir, "workspace.css")) && existsSync(join(dir, "composer.tsx")))!;
const source = files(SRC).map(path => ({ path, text: readFileSync(path, "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "").replace(/\s\/\/ .*$/gm, "") }));
const hits = (rule: RegExp) => source.flatMap(f => f.text.split("\n").filter(line => rule.test(line)).map(line => `${f.path}: ${line.trim().slice(0, 120)}`));

describe("interface rules", () => {
  it("working indicators move only while pending, by transform and opacity, and pause when nobody sees them", () => {
    const css = readFileSync(join(SRC, "workspace.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    // Every repeating animation sits on an indicator that exists only while something is pending.
    const pending = /^(\.spin|\.ws-loading \.spin|\.spin\.ws-wait|\.shimmer|\[data-busy\]::after|\.btn\[aria-busy="true"\]::after)$/;
    const looping = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(m => /animation[^;]*infinite/.test(m[2]));
    expect(looping.length).toBeGreaterThan(0);
    for (const m of looping) for (const selector of m[1].split(",").map(s => s.trim())) expect(selector).toMatch(pending);
    // What they run changes transform or opacity only: no layout, paint, shadow or filter per frame.
    const used = new Set(looping.flatMap(m => [...m[2].matchAll(/animation(?:-name)?:\s*([^;]+)/g)].flatMap(a => a[1].split(/,(?![^(]*\))/).map(part => part.trim().split(/\s+/)[0]))));
    const frames = new Map([...readFileSync(join(SRC, "mockup-styles.css"), "utf8").concat(css).matchAll(/@keyframes ([\w-]+) \{((?:[^{}]*\{[^}]*\})*)\s*\}/g)].map(k => [k[1], k[2]]));
    for (const name of used) {
      expect(frames.has(name), name).toBe(true);
      for (const p of frames.get(name)!.matchAll(/([\w-]+)\s*:/g)) expect(p[1], name).toMatch(/^(transform|opacity)$/);
    }
    // A hidden window and a parked tab pause them, pseudo-element bars included.
    expect(css).toMatch(/html\[data-hidden\] \*, html\[data-hidden\] \*::after \{[^}]*animation-play-state: paused/);
    expect(css).toMatch(/\.ws-pane\[data-parked\] \*, \.ws-pane\[data-parked\] \*::after \{[^}]*animation-play-state: paused/);
    // Reduced motion still shows life, as a slow pulse.
    expect(css).toMatch(/prefers-reduced-motion: reduce\)\s*\{[^}]*\[data-busy\]::after[^{]*\{[^}]*animation: ws-pulse/);
    expect(css).toMatch(/prefers-reduced-motion: reduce\)\s*\{\s*\.spin[^{]*\{[^}]*animation: ws-pulse/);
  });

  it("has hints and labels in English only", () => {
    expect(hits(/[\u0400-\u04ff]/)).toEqual([]);
    expect(JSON.stringify(HELP)).not.toMatch(/[\u0400-\u04ff]/);
  });
  it("does not name the program inside the program", () => {
    expect(hits(/\bCortex\b/)).toEqual([]);
  });
  it("has no second control for renaming what a click on its title edits", () => {
    expect(hits(/label="Rename|>Rename<|Title generated|Title set/)).toEqual([]);
  });
  it("never spells out a route or connection id in words", () => {
    expect(hits(/\b(?:xr|cn)_/).filter(line => !line.includes(".test("))).toEqual([]);
  });
  it("has no native hover titles, the owner reads no popups", () => {
    // The tag a title= sits on, walking back past balanced braces so props inside handlers do not count.
    const tagOf = (text: string, at: number) => { let depth = 0; for (let i = at; i >= 0; i--) { const c = text[i]; if (c === "}") depth++; else if (c === "{") depth--; else if (c === "<" && depth === 0 && /[A-Za-z]/.test(text[i + 1] ?? "")) return /^[\w.]+/.exec(text.slice(i + 1))![0]; } return ""; };
    const native = source.flatMap(f => [...f.text.matchAll(/\stitle=/g)].flatMap(m => { const tag = tagOf(f.text, m.index!); return tag === "A" || /^[a-z]/.test(tag) ? [`${f.path}: <${tag} title=`] : []; }));
    expect(native).toEqual([]);
    expect(hits(/["']?title["']?\s*:\s*reason|\.title\s*=/)).toEqual([]);
  });
  it("gives every disabled control its reason in visible words, never only a state", () => {
    const opened = (text: string) => { const out: string[] = []; for (const m of text.matchAll(/<(?:[a-z][\w-]*|A)\b/g)) { let depth = 0, i = m.index!; for (; i < text.length; i++) { const c = text[i]; if (c === "{") depth++; else if (c === "}") depth--; else if (c === ">" && depth === 0 && text[i - 1] !== "=") break; } out.push(text.slice(m.index!, i)); } return out; };
    const unexplained = source.flatMap(f => opened(f.text).filter(tag => /aria-disabled=/.test(tag) && !/data-why=|ws\.needs\(/.test(tag)).map(tag => `${f.path}: ${tag.slice(0, 80)}`));
    expect(unexplained).toEqual([]);
  });
  it("never draws a disabled reason inside a control", () => {
    const sheets = readdirSync(SRC).filter(name => name.endsWith(".css")).map(name => ({ name, css: readFileSync(join(SRC, name), "utf8").replace(/\/\*[\s\S]*?\*\//g, "") }));
    expect(sheets.map(s => s.name)).toEqual(expect.arrayContaining(["layout.css", "mockup-styles.css", "workspace.css"]));
    const rules = sheets.flatMap(s => [...s.css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(m => ({ sheet: s.name, selector: m[1].trim(), body: m[2] })));
    // data-why is a declaration for ReasonSlot and ReasonTip to read. No sheet may style by it, or print it with content: attr().
    expect(rules.filter(r => r.selector.includes("data-why")).map(r => `${r.sheet}: ${r.selector}`)).toEqual([]);
    // The one printed attribute is a settings table's column name over a cell on a phone (data-label), never a reason.
    expect(sheets.flatMap(s => s.css.match(/content:\s*attr\((?!data-label\))[^)]*\)/g) ?? [])).toEqual([]);
    // Nor may a rule grow the control itself, however it gets there.
    expect(rules.filter(r => /\.btn\b|\.ib\b/.test(r.selector) && /height:\s*auto|flex-wrap:\s*wrap|flex-flow:[^;]*\bwrap|white-space:\s*normal/.test(r.body)).map(r => `${r.sheet}: ${r.selector}`)).toEqual([]);
  });
  it("keeps the layout primitives in one layer that holds every class they use", () => {
    const css = readFileSync(join(SRC, "layout.css"), "utf8");
    expect(css.replace(/\/\*[\s\S]*?\*\//g, "").trim()).toMatch(/^@layer layout \{[\s\S]*\}$/);
    const tsx = source.find(f => f.path.endsWith("layout.tsx"))!.text;
    const used = [...new Set([...tsx.matchAll(/\bl-[a-z-]+/g)].map(m => m[0]))].filter(name => !name.endsWith("-"));
    expect(used.length).toBeGreaterThan(15);
    expect(used.filter(name => !css.includes(`.${name}`))).toEqual([]);
  });
  it("counts through Count, so an empty list never reads zero", () => {
    expect(hits(/<span class="n">\{[^}]*\.length\}<\/span>/)).toEqual([]);
  });
});
