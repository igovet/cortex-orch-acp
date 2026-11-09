// Concept-local presentation data for concept B.
// The shared fixture holds tasks, decisions, checks, results, the conflict file and Git state.
// It has no workspace file bodies, result file lists or terminal transcript, which a code-first
// concept needs to render. Everything below is synthetic and carries no task, decision or check facts.

export const LOCAL_DATA_NOTE = "File bodies, result file changes and the terminal transcript are concept-local synthetic presentation data, not part of the shared fixture.";

export type WsFile = { path: string; text: string };
export type DiffLine = { t: " " | "+" | "-"; s: string };
export type Change = { path: string; type: "Modified" | "Added"; lines: DiffLine[] };

const lines = (...l: string[]) => l.join("\n") + "\n";

export const START_FILE = "src/auth/limiter.ts";

export const WORKSPACE_FILES: WsFile[] = [
  { path: "README.md", text: lines("# Synthetic Mosaic", "", "Synthetic workspace for the concept B prototype.", "It contains no real source code, credentials or private data.") },
  { path: "package.json", text: lines("{", '  "name": "synthetic-mosaic",', '  "private": true,', '  "scripts": { "test": "echo synthetic fixture" }', "}") },
  { path: "src/auth/limiter.ts", text: lines(
    "// Synthetic rate limiter used only by the prototype fixture.",
    "export type Window = { limit: number; perSeconds: number };",
    "",
    "const buckets = new Map<string, number[]>();",
    "",
    "export function allow(key: string, window: Window, now: number): boolean {",
    "  const since = now - window.perSeconds * 1000;",
    "  const hits = (buckets.get(key) ?? []).filter(t => t > since);",
    "  if (hits.length >= window.limit) return false;",
    "  hits.push(now);",
    "  buckets.set(key, hits);",
    "  return true;",
    "}",
  ) },
  { path: "src/auth/session.ts", text: lines(
    "// Synthetic session helper.",
    "export type Session = { id: string; startedAt: number };",
    "",
    "export function isFresh(session: Session, now: number, maxAgeMs: number): boolean {",
    "  return now - session.startedAt < maxAgeMs;",
    "}",
  ) },
  { path: "src/routes/login.ts", text: lines(
    "// Synthetic login route.",
    'import { allow } from "../auth/limiter";',
    "",
    "export function login(key: string, now: number): \"ok\" | \"limited\" {",
    "  return allow(key, { limit: 5, perSeconds: 60 }, now) ? \"ok\" : \"limited\";",
    "}",
  ) },
  { path: "tests/limiter.test.ts", text: lines(
    "// Synthetic test file. Nothing here is executed by the prototype.",
    'import { allow } from "../src/auth/limiter";',
    "",
    "export const cases = [",
    '  { name: "allows the first request", run: () => allow("k", { limit: 1, perSeconds: 60 }, 0) },',
    "];",
  ) },
];

export const RESULT_CHANGES: Record<string, Change[]> = {
  "mock-result-complete": [
    { path: "src/auth/limiter.ts", type: "Modified", lines: [
      { t: " ", s: "export function allow(key: string, window: Window, now: number): boolean {" },
      { t: " ", s: "  const since = now - window.perSeconds * 1000;" },
      { t: "-", s: "  const hits = buckets.get(key) ?? [];" },
      { t: "+", s: "  const hits = (buckets.get(key) ?? []).filter(t => t > since);" },
      { t: " ", s: "  if (hits.length >= window.limit) return false;" },
      { t: "+", s: "  hits.push(now);" },
      { t: "+", s: "  buckets.set(key, hits);" },
      { t: " ", s: "  return true;" },
      { t: " ", s: "}" },
    ] },
    { path: "tests/limiter.test.ts", type: "Added", lines: [
      { t: "+", s: "// Synthetic test file. Nothing here is executed by the prototype." },
      { t: "+", s: 'import { allow } from "../src/auth/limiter";' },
      { t: "+", s: "" },
      { t: "+", s: "export const cases = [" },
      { t: "+", s: '  { name: "allows the first request", run: () => allow("k", { limit: 1, perSeconds: 60 }, 0) },' },
      { t: "+", s: "];" },
    ] },
  ],
  "mock-result-stale": [
    { path: "synthetic/layout.ts", type: "Modified", lines: [
      { t: "-", s: "layout = compact" },
      { t: "+", s: "layout = comfortable" },
    ] },
  ],
};

export const TERMINAL_TRANSCRIPT = [
  "Fixture transcript. No shell process is attached to this prototype.",
  "$ npm test",
  "synthetic fixture: output is static text; nothing was executed",
];

export function changeCounts(c: Change) {
  return { add: c.lines.filter(l => l.t === "+").length, del: c.lines.filter(l => l.t === "-").length };
}

export type SplitCell = { n: number; s: string; t: " " | "+" | "-" };
export function splitRows(src: DiffLine[]) {
  const rows: { left?: SplitCell; right?: SplitCell }[] = [];
  let a = 1, b = 1, i = 0;
  while (i < src.length) {
    if (src[i].t === " ") { rows.push({ left: { n: a++, s: src[i].s, t: " " }, right: { n: b++, s: src[i].s, t: " " } }); i++; continue; }
    const dels: DiffLine[] = [], adds: DiffLine[] = [];
    while (i < src.length && src[i].t === "-") dels.push(src[i++]);
    while (i < src.length && src[i].t === "+") adds.push(src[i++]);
    for (let k = 0; k < Math.max(dels.length, adds.length); k++) {
      rows.push({ left: dels[k] ? { n: a++, s: dels[k].s, t: "-" } : undefined, right: adds[k] ? { n: b++, s: adds[k].s, t: "+" } : undefined });
    }
  }
  return rows;
}

export function unifiedRows(src: DiffLine[]) {
  let a = 1, b = 1;
  return src.map(l => ({ t: l.t, s: l.s, a: l.t === "+" ? undefined : a++, b: l.t === "-" ? undefined : b++ }));
}

export type TreeRow = { path: string; name: string; dir: boolean; level: number };
export function treeRows(paths: string[], expanded: Record<string, boolean>): TreeRow[] {
  const dirs = new Set<string>();
  for (const p of paths) { const parts = p.split("/"); for (let i = 1; i < parts.length; i++) dirs.add(parts.slice(0, i).join("/")); }
  const nodes = [...dirs].map(path => ({ path, dir: true })).concat(paths.map(path => ({ path, dir: false })));
  const parentOf = (p: string) => p.split("/").slice(0, -1).join("/");
  const out: TreeRow[] = [];
  const walk = (parent: string, level: number) => {
    const kids = nodes.filter(n => parentOf(n.path) === parent).sort((x, y) => x.dir === y.dir ? x.path.localeCompare(y.path) : x.dir ? -1 : 1);
    for (const n of kids) {
      out.push({ ...n, name: n.path.split("/").pop()!, level });
      if (n.dir && expanded[n.path]) walk(n.path, level + 1);
    }
  };
  walk("", 1);
  return out;
}

export const fmtUtc = (iso: string, seconds = false) => iso.replace("T", " ").slice(0, seconds ? 19 : 16) + " UTC";
// Fixture digests differ only in their suffix, so they are shown in full up to 24 characters.
export const shortDigest = (d: string) => d.length > 24 ? d.slice(0, 24) + "…" : d;
