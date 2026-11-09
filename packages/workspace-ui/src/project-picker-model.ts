import type { DirectoryEntry } from "@cortex/workspace-domain";

/** What a typed line is, and which rows it offers. The machine does the searching; this decides only what is shown of it. */
export type PickerPath = { path: string; git: boolean; literal: boolean };

/** A path as the owner reads it, with their home as `~`. A path outside it stays whole. */
export function shortenPath(path: string, home: string | null): string {
  if (!home || home === "/") return path;
  const root = home.replace(/\/+$/, "");
  return path === root ? "~" : path.startsWith(`${root}/`) ? `~${path.slice(root.length)}` : path;
}
/** The path typed text names on the machine, when it names one. */
function expanded(query: string, home: string | null): string | null {
  const text = query.trim();
  if (text === "~" || text.startsWith("~/")) return home ? `${home.replace(/\/+$/, "")}${text.slice(1)}`.replace(/\/+$/, "") || "/" : null;
  return text.startsWith("/") ? text.replace(/\/+$/, "") || "/" : null;
}
export const isOpenablePath = (query: string) => { const text = query.trim(); return text.startsWith("/") || text === "~" || text.startsWith("~/"); };

/**
 * Whether a known path still fits what is typed. It narrows rows and ranks nothing, so it is strict: a typed path must lead
 * to it, and a fragment must appear in it. Paseo also accepts letters in order, which let an unrelated project outrank the
 * folder being typed.
 */
export function matchesPath(query: string, path: string, home: string | null): boolean {
  const text = query.trim();
  if (!text || text === "~" || text === "~/") return true;
  const lowered = path.toLowerCase();
  if (!isOpenablePath(text)) return lowered.includes(text.toLowerCase());
  const typed = expanded(text, home);
  return typed !== null && lowered.startsWith(typed.toLowerCase());
}

/**
 * Registered projects come first, narrowed by the text, then what the machine found. A typed path that is not among them
 * is offered last, as itself, so a folder the search cannot list can still be opened by name. Paseo puts it first, where
 * Enter on half a path adds a folder that is not there.
 */
export function pickerPaths(input: { query: string; recent: string[]; found: DirectoryEntry[]; home: string | null }): PickerPath[] {
  const rows = new Map<string, PickerPath>();
  const add = (path: string, git: boolean) => { if (!rows.has(path)) rows.set(path, { path, git, literal: false }); };
  const gitOf = new Map(input.found.map(entry => [entry.path, entry.git]));
  for (const path of input.recent) if (matchesPath(input.query, path, input.home)) add(path, gitOf.get(path) ?? false);
  for (const entry of input.found) add(entry.path, entry.git);
  const text = input.query.trim(), named = expanded(text, input.home), exact = named === null ? undefined : rows.get(named);
  // The folder the text names exactly comes before anything inside it, so Enter takes what was typed.
  const paths = exact ? [exact, ...[...rows.values()].filter(row => row !== exact)] : [...rows.values()];
  if (!isOpenablePath(text) || exact || rows.has(text)) return paths;
  return [...paths, { path: text, git: false, literal: true }];
}

/** The last part of a path, which names the project until the owner renames it. */
export function folderName(path: string, home: string | null): string {
  const named = expanded(path, home) ?? path.trim();
  const trimmed = named.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || named;
}
export const joinPath = (parent: string, name: string) => `${parent.replace(/\/+$/, "")}/${name}`;

/** Why a name cannot be a new folder, or nothing when it can. */
export function folderNameProblem(raw: string): string | null {
  const name = raw.trim();
  if (!name || name === "." || name === "..") return "Enter a folder name.";
  if (/[\\/\0]/.test(name)) return "A folder name cannot contain a slash.";
  return null;
}

/** The next row that can be chosen, wrapping at either end. */
export function moveActive(from: number, enabled: boolean[], step: 1 | -1): number {
  const count = enabled.length;
  if (!enabled.some(Boolean)) return 0;
  let at = from;
  for (let i = 0; i < count; i += 1) {
    at = (at + step + count) % count;
    if (enabled[at]) return at;
  }
  return from;
}
