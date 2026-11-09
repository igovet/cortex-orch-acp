import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { HELP } from "../src/orch-help";

/** The rules a source file can answer on its own. What needs layout is in sweep.measure.py. */
const files = (dir: string): string[] => readdirSync(dir).flatMap(name => { const path = join(dir, name); return statSync(path).isDirectory() ? files(path) : /\.tsx?$/.test(name) ? [path] : []; });
const source = files("src").map(path => ({ path, text: readFileSync(path, "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "").replace(/\s\/\/ .*$/gm, "") }));
const hits = (rule: RegExp) => source.flatMap(f => f.text.split("\n").filter(line => rule.test(line)).map(line => `${f.path}: ${line.trim().slice(0, 120)}`));

describe("interface rules", () => {
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
  it("counts through Count, so an empty list never reads zero", () => {
    expect(hits(/<span class="n">\{[^}]*\.length\}<\/span>/)).toEqual([]);
  });
});
